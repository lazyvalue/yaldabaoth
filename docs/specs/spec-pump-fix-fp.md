# Spec: Pump Architecture Fix — Event-Driven Draining & Incremental Render

**Status:** Draft
**Date:** 2026-05-28
**Scope:** `src/bin/yalda-gpui/main.rs` — server pump, ACP pump, render path
**Bugs addressed:** Typing latency regression, server pump dropping/delaying agent content

---

## 1. Root-Cause Analysis

There are four independent failure modes that compound into the observed behavior.

### 1.1 Lock Convoy on `this.update(cx, ...)`

Both pumps call `this.update(cx, |this, cx| { ... })`. This acquires a mutable
borrow on the entire `YaldaGpuiView` model. While held:

- **Keystroke handlers cannot run.** GPUI dispatches input actions by calling
  `this.update()` on the same model. If the pump holds the lock processing a
  batch of 256 events, every keystroke queued during that window stalls until
  the batch completes.
- **Render cannot proceed.** `render(&mut self, cx)` also requires the model
  borrow. A pump batch that takes 5ms to process means 5ms of render stall,
  which at 60fps is a third of the frame budget.

The server pump is the worst offender because it calls `this.update()` *twice*
per batch iteration — once to drain from the channel, once to route. Each call
re-acquires the lock. Between the two calls, another actor (render, keystroke)
could sneak in, but inside each call the lock is held for the entire batch.

**Key insight:** The drain call (`server.try_recv()` in a loop) does not need
the model lock at all. It only needs a reference to `this.session_server`,
which is an `Arc`-wrapped, thread-safe server handle. The lock is taken
unnecessarily.

### 1.2 Polling Without Wake Signal (Server Pump)

The server pump is pure polling: sleep 16ms, try to drain. This means:

- **Minimum 16ms latency** on every server notification, even when the app is
  otherwise idle.
- **Throughput ceiling.** At 256 events per batch and one batch per 16ms cycle
  (with yield delays between continuation batches), maximum throughput is ~16k
  events/sec. A fast-streaming agent producing 50k+ tokens/sec easily exceeds
  this.
- **Starvation under load.** When the queue backs up, the drain loop processes
  batches of 256, yields 1ms, then continues. But each batch requires two
  `this.update()` calls. Under sustained high throughput, the pump spends more
  time acquiring/releasing locks and yielding than actually processing events.

Compare with the ACP pump: it has a `wake_rx` channel and uses
`select_biased!` to wake immediately on new data. The server pump has no
equivalent mechanism.

### 1.3 Excessive `cx.notify()` Calls

Both pumps call `cx.notify()` after each batch that had work. `cx.notify()`
schedules a re-render. If the drain loop processes 10 batches of 256 events,
that is 10 `cx.notify()` calls in rapid succession. GPUI coalesces
back-to-back notifications to some degree, but each notification that does
fire triggers the O(n) render path described below.

The correct behavior is: notify *once* after the pump has drained everything it
is going to drain in this cycle, not once per sub-batch.

### 1.4 O(n) Render Path on Every Frame

The render method for the agent/Claude tile does this on every `cx.notify()`:

1. Extracts ALL lines from the editor into a `Vec<String>` — O(n) allocation + copy
2. Calls `highlight_markdown_lines()` on ALL lines — O(n) tokenization
3. Calls `highlight_markdown_lines_stripped()` on ALL lines — second O(n) pass
4. Builds per-line gutter tags for ALL lines — O(n) iteration
5. Iterates ALL frozen ranges for block detection — O(n) or O(n*m) iteration
6. Finally builds the virtualized list (only visible items rendered) — O(visible)

Steps 1-5 are O(total_lines) regardless of what changed. During agent
streaming, the editor might have 5000+ lines. Each `cx.notify()` re-runs all
five passes. With the pump firing `cx.notify()` per batch, this means multiple
full re-tokenizations per 16ms cycle.

**This is the primary cause of typing latency.** A single character insertion
triggers `cx.notify()`, which triggers a full re-highlight of thousands of
lines, consuming the entire frame budget and then some.

---

## 2. Why Previous Fixes Failed

The drain-loop approach (the current code) attempted to fix throughput by
batching more events per cycle. It failed for three structural reasons:

### 2.1 Lock Granularity Is Wrong

Batching 256 events instead of 1 does not help when the fundamental problem is
that **the lock is held for the entire batch**. A batch of 256 events processed
inside `this.update()` blocks keystrokes for the entire batch duration. Making
the batch bigger makes latency worse, not better. Making it smaller improves
latency but kills throughput.

This is a false dilemma created by coupling "read from channel" with "mutate
model" inside the same lock scope.

### 2.2 Polling Cannot Achieve Both Low Latency and Low CPU

A 16ms polling interval means 16ms minimum latency. Reducing the interval
improves latency but increases CPU usage (spinning). An event-driven wake
signal achieves zero-latency wake with zero idle CPU. The ACP pump already
proves this works; the server pump simply lacks the same mechanism.

### 2.3 Notify-Per-Batch Amplifies Render Cost

Each `cx.notify()` triggers O(total_lines) work. Calling it once per 256-event
batch means the render path runs multiple times during a single drain cycle.
The fix is not "batch more aggressively" — it is "notify exactly once per
drain cycle" and "make the render path O(changed) instead of O(total)."

---

## 3. Proposed Fix

The fix has three layers, each independently valuable, each buildable
incrementally.

### Layer 1: Shrink Lock Scope — Extract-Then-Apply

**Principle:** Pure data extraction (reading from a channel) requires no model
lock. Separate "extract from channel" from "apply to model."

#### Server pump — new structure

```rust
fn start_server_pump(&self, cx: &mut Context<Self>) -> Task<()> {
    // Grab an Arc clone of the session_server handle ONCE,
    // outside the loop. This is safe because SessionServer is
    // Send + Sync and try_recv() only needs &self.
    let server: Arc<SessionServer> = {
        // One-time this.update to clone the Arc
        let srv = this.update(cx, |this, _cx| {
            this.session_server.as_ref().map(Arc::clone)
        });
        match srv {
            Ok(Some(s)) => s,
            _ => return,
        }
    };

    cx.spawn(async move |this, cx| {
        let idle_delay = Duration::from_millis(16);
        let yield_delay = Duration::from_millis(1);

        loop {
            // Phase 1: WAIT (no lock held)
            // TODO: replace with wake_rx in Layer 2
            cx.background_executor().timer(idle_delay).await;

            // Phase 2: EXTRACT (no lock held)
            // Pull everything available from the channel into a local Vec.
            // No model lock. No contention with keystrokes or render.
            let mut all_notes: Vec<ServerNotification> = Vec::new();
            loop {
                match server.try_recv() {
                    Some(note) => all_notes.push(note),
                    None => break,
                }
                // Safety valve: if we've pulled 4096 events, yield
                // to avoid starving other background tasks.
                if all_notes.len() >= 4096 {
                    break;
                }
            }
            if all_notes.is_empty() {
                continue;
            }

            // Phase 3: TRANSFORM (no lock held)
            // Pre-process / classify notifications into an apply-ready form.
            // This is a pure function over the extracted data.
            // Example: group by session_index, parse content deltas,
            // pre-compute string appends, etc.
            let grouped: HashMap<usize, Vec<ServerNotification>> =
                group_by_session(&all_notes);

            // Phase 4: APPLY (lock held, minimal duration)
            // Single this.update() call for the entire cycle.
            // Only mutates model state; no I/O, no channel reads.
            let should_notify = this.update(cx, |this, cx| {
                let mut changed = false;
                for (session_idx, notes) in grouped {
                    for note in notes {
                        changed |= this.apply_server_notification(
                            session_idx, note, cx
                        );
                    }
                }
                changed
            });

            // Phase 5: NOTIFY (outside lock)
            // Exactly ONE notify per drain cycle.
            match should_notify {
                Ok(true) => cx.notify(),
                Err(_) => return, // view dropped
                _ => {}
            }

            // If we hit the 4096 cap, loop immediately to drain more.
            // Otherwise the outer loop sleeps again.
            if all_notes.len() >= 4096 {
                cx.background_executor().timer(yield_delay).await;
                continue; // skip the idle sleep, drain more
            }
        }
    })
}
```

**Critical changes:**

1. `server.try_recv()` is called **outside** `this.update()`. The `Arc<SessionServer>` is cloned once at startup. Channel reads happen with zero model contention.

2. One `this.update()` call per cycle, not two per batch. The lock is held only for the apply phase, which is pure state mutation (no I/O, no allocation, no channel reads).

3. Exactly one `cx.notify()` per drain cycle, called **after** the lock is released.

4. The 4096 cap prevents unbounded memory growth if the agent streams faster than we can apply. When hit, we yield briefly and loop again immediately (no idle sleep).

#### ACP pump — same refactor

Apply the same extract-then-apply pattern to the ACP pump. Currently
`pump_session()` is called inside `this.update()` and it both reads from the
channel and mutates state. Split it:

```rust
// Outside lock: extract up to budget events
let events = session_handle.drain_events(64); // pure channel read, no model lock

// Inside lock: apply extracted events
this.update(cx, |this, cx| {
    let changed = this.apply_session_events(session_index, &events, cx);
    if changed { cx.notify(); }
    events.len() == 64 // return "more pending" flag
});
```

This requires that the ACP session handle (or its recv channel) is accessible
outside the model lock, same as the server pump fix. Wrap it in an `Arc` or
extract the receiver into a local variable before entering the loop.

**Important:** Move `cx.notify()` outside the `this.update()` closure here too.
Collect a `did_change` bool, then call `cx.notify()` after the lock is released.
Actually — `cx.notify()` inside `this.update(cx, |this, cx| ...)` calls it on
the `cx` that is part of the model borrow context, so it merely schedules; it
does not immediately re-enter render. This is safe. But for clarity, the
single-notify-per-cycle invariant should still hold: accumulate all changes,
notify once.

### Layer 2: Event-Driven Wake for Server Pump

**Principle:** Replace polling with an event-driven wake signal, matching the
ACP pump's existing `wake_rx` pattern.

#### Implementation

Add a `futures::channel::mpsc::unbounded()` pair to `SessionServer`:

```rust
pub struct SessionServer {
    // existing fields...
    notification_tx: std::sync::mpsc::Sender<ServerNotification>,
    notification_rx: std::sync::mpsc::Receiver<ServerNotification>,

    // NEW: wake signal
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
}

impl SessionServer {
    pub fn take_wake_rx(&self) -> futures::channel::mpsc::UnboundedReceiver<()> {
        // Return the receiver, to be owned by the pump task.
        // Only called once at pump startup.
    }

    pub fn send_notification(&self, note: ServerNotification) {
        self.notification_tx.send(note).ok();
        // Wake the pump — non-blocking, bounded cost.
        // send() on an unbounded mpsc never blocks.
        let _ = self.wake_tx.unbounded_send(());
    }
}
```

Update the server pump wait phase:

```rust
// Phase 1: WAIT (no lock held)
// Event-driven: wake immediately on new data, or timeout for
// housekeeping (session expiry checks, etc.)
let timer = cx.background_executor().timer(Duration::from_millis(100));
futures::select_biased! {
    _ = wake_rx.next().fuse() => {
        // Drain any coalesced wake signals
        while wake_rx.next().now_or_never().flatten().is_some() {}
    }
    _ = timer.fuse() => {}
}
```

This eliminates the 16ms polling latency. The pump wakes within microseconds
of a notification being sent. The 100ms timeout is a safety net, not a
performance-critical path.

### Layer 3: Incremental Render — Dirty-Range Highlighting

**Principle:** The render path should do O(visible + changed) work, not
O(total_lines) work.

#### 3.1 Highlight Cache with Dirty Tracking

Introduce a highlight cache that persists across frames:

```rust
struct HighlightCache {
    /// Cached highlighted spans per line.
    /// Index = line number, value = highlighted spans for that line.
    lines: Vec<Option<HighlightedLine>>,

    /// Lines that need re-highlighting.
    /// Set on edit, cleared after re-highlight.
    dirty: RangeSet,  // or BitVec, or Vec<Range<usize>>

    /// Generation counter. Bumped on every edit.
    /// Compared with editor's generation to detect full invalidation.
    generation: u64,
}

struct HighlightedLine {
    /// The source text at the time of highlighting (for staleness check).
    source_hash: u64,
    /// Pre-computed styled spans.
    spans: Vec<StyledSpan>,
    /// Pre-computed stripped spans (for the second highlight pass).
    stripped_spans: Vec<StyledSpan>,
    /// Gutter tag, if any.
    gutter_tag: Option<GutterTag>,
}
```

#### 3.2 Dirty Marking

When the editor content changes (keystroke, paste, agent streaming append):

```rust
impl HighlightCache {
    fn mark_dirty(&mut self, range: Range<usize>) {
        // Mark affected lines dirty.
        // For markdown, also mark surrounding context lines because
        // block-level constructs (code fences, lists) can change
        // meaning of adjacent lines.
        let context_start = range.start.saturating_sub(3);
        let context_end = (range.end + 3).min(self.lines.len());
        self.dirty.insert(context_start..context_end);
    }

    fn mark_appended(&mut self, old_len: usize, new_len: usize) {
        // Agent streaming appends lines at the end.
        // Only the new lines (and a small context window before them)
        // need highlighting.
        self.lines.resize(new_len, None);
        let context_start = old_len.saturating_sub(3);
        self.dirty.insert(context_start..new_len);
    }
}
```

#### 3.3 Render Path Changes

Replace the current all-lines highlight with incremental update:

```rust
// In render():

// Step 1: Check if full invalidation needed (e.g., theme change, first render)
if self.highlight_cache.generation != self.editor.generation() {
    self.highlight_cache.invalidate_all();
    self.highlight_cache.generation = self.editor.generation();
}

// Step 2: Re-highlight only dirty lines
let dirty_ranges = self.highlight_cache.take_dirty_ranges();
for range in dirty_ranges {
    let lines = self.editor.lines_in_range(range.clone());
    let highlighted = highlight_markdown_lines(&lines);
    let stripped = highlight_markdown_lines_stripped(&lines);
    for (i, line_idx) in range.enumerate() {
        self.highlight_cache.lines[line_idx] = Some(HighlightedLine {
            source_hash: hash(&lines[i]),
            spans: highlighted[i].clone(),
            stripped_spans: stripped[i].clone(),
            gutter_tag: compute_gutter_tag(line_idx, &lines[i]),
        });
    }
}

// Step 3: Build virtualized list from cache (only visible lines accessed)
let visible_range = self.compute_visible_range(scroll_offset, viewport_height);
for line_idx in visible_range {
    let cached = &self.highlight_cache.lines[line_idx];
    // Build element from cached spans — no re-tokenization
}
```

#### 3.4 Where to Hook Dirty Marking

- **Keystroke insertion:** After `editor.insert_char()` or `editor.insert_text()`,
  call `highlight_cache.mark_dirty(edited_line..edited_line+1)`.
  If the edit adds/removes lines, also update the cache length.

- **Agent streaming (pump apply):** After appending content from the agent,
  call `highlight_cache.mark_appended(old_line_count, new_line_count)`.

- **Paste / bulk edit:** Mark the entire affected range dirty.

- **Theme change:** Call `highlight_cache.invalidate_all()`.

#### 3.5 Cost Analysis

| Scenario | Before | After |
|---|---|---|
| Single character typed | O(total_lines) | O(1) — 1 line re-highlighted |
| Agent streams 100 chars | O(total_lines) per notify | O(appended_lines) once per cycle |
| Scroll (no edit) | O(total_lines) | O(0) — cache hit, no re-highlight |
| Theme change | O(total_lines) | O(total_lines) — unavoidable, but rare |

---

## 4. Specific Code Changes

### 4.1 New Types / Structs

**File:** `main.rs` (or extracted to a new module `highlight_cache.rs`)

```rust
/// Tracks which line ranges need re-highlighting.
/// Internally a sorted, merged Vec<Range<usize>>.
struct DirtyRanges {
    ranges: Vec<Range<usize>>,
}

impl DirtyRanges {
    fn new() -> Self { Self { ranges: Vec::new() } }
    fn insert(&mut self, range: Range<usize>) { /* merge overlapping */ }
    fn take(&mut self) -> Vec<Range<usize>> { std::mem::take(&mut self.ranges) }
    fn is_empty(&self) -> bool { self.ranges.is_empty() }
}

/// Per-line cached highlight result.
struct CachedLineHighlight {
    source_hash: u64,
    spans: Vec<(Range<usize>, HighlightStyle)>,
    stripped_spans: Vec<(Range<usize>, HighlightStyle)>,
    gutter_tag: Option<GutterTag>,
}

/// Full highlight cache for an editor tile.
struct HighlightCache {
    lines: Vec<Option<CachedLineHighlight>>,
    dirty: DirtyRanges,
    editor_generation: u64,
}
```

### 4.2 Modified Functions

#### `start_server_pump` (lines ~7637-7821)

**Before:** Two `this.update()` calls per batch, channel read inside lock,
`cx.notify()` per batch.

**After:** Channel read outside lock (via `Arc<SessionServer>`), single
`this.update()` per drain cycle, single `cx.notify()` after lock release.
Full pseudocode in Section 3, Layer 1.

#### `pump_session` (called from ACP pump)

**Before:** Monolithic function that reads channel + mutates state, called
inside `this.update()`.

**After:** Split into two functions:

- `drain_session_events(handle: &SessionHandle, budget: usize) -> Vec<SessionEvent>`
  — pure channel drain, no model dependency, called **outside** lock.
- `apply_session_events(&mut self, idx: usize, events: &[SessionEvent], cx: &mut Context<Self>) -> bool`
  — pure state mutation, called **inside** lock. Returns whether anything changed.

#### ACP pump loop (lines ~7439-7489)

**Before:** Calls `this.update(cx, |this, cx| this.pump_session(...))` in inner loop.

**After:**

```rust
loop {
    // Wait for wake or timeout (unchanged)
    // ...

    // Drain outside lock
    let events = drain_session_events(&session_handle, 64);
    if events.is_empty() { continue; }
    let had_full_batch = events.len() == 64;

    // Apply inside lock
    let changed = this.update(cx, |this, cx| {
        this.apply_session_events(session_index, &events, cx)
    });

    // Notify outside lock, once
    if let Ok(true) = changed {
        cx.notify();
    }

    if had_full_batch {
        cx.background_executor().timer(yield_delay).await;
        continue; // drain more without sleeping
    }
    // else: fall through to outer loop wait
}
```

#### `render()` — Claude/Agent tile section

**Before:** Extracts all lines, highlights all, builds virtualised list.

**After:** Updates highlight cache incrementally, builds virtualised list from cache.
See Section 3, Layer 3 for full pseudocode.

### 4.3 New Helper: `group_by_session`

```rust
/// Pure function: groups server notifications by target session index.
/// No side effects, no locks.
fn group_by_session(
    notes: &[ServerNotification],
) -> HashMap<usize, Vec<&ServerNotification>> {
    let mut map = HashMap::new();
    for note in notes {
        map.entry(note.session_index()).or_default().push(note);
    }
    map
}
```

### 4.4 SessionServer Changes

Add wake channel:

```rust
impl SessionServer {
    pub fn new(/* ... */) -> (Self, futures::channel::mpsc::UnboundedReceiver<()>) {
        let (wake_tx, wake_rx) = futures::channel::mpsc::unbounded();
        let server = Self {
            // ...existing fields...
            wake_tx,
        };
        (server, wake_rx)
    }

    /// Called by the session-server worker thread when a notification is ready.
    pub fn post_notification(&self, note: ServerNotification) {
        self.notification_tx.send(note).ok();
        let _ = self.wake_tx.unbounded_send(()); // wake the pump
    }
}
```

The `wake_rx` is passed to `start_server_pump` at construction time and owned
by the pump task.

---

## 5. Implementation Order

The three layers are independent and can be landed separately. Recommended order:

### Phase 1: Lock Scope (highest impact, lowest risk)

1. Clone `Arc<SessionServer>` outside the pump loop.
2. Move `server.try_recv()` calls outside `this.update()`.
3. Collapse to single `this.update()` + single `cx.notify()` per cycle.
4. Apply same pattern to ACP pump (split `pump_session`).

**Expected impact:** Eliminates lock convoy. Keystroke handlers and render no
longer stall behind batch processing. Typing latency should drop to
sub-frame levels.

**Validation:** Add a debug timer around the `this.update()` call in each pump.
Log if any single `this.update()` exceeds 2ms. Before the fix, you will see
5-50ms holds. After, they should be <1ms for typical batches.

### Phase 2: Wake Signal (medium impact, low risk)

1. Add `wake_tx`/`wake_rx` to `SessionServer`.
2. Wire `wake_tx.unbounded_send(())` into the notification posting path.
3. Replace the server pump's `timer(idle_delay).await` with
   `select_biased!` on `wake_rx` + timeout.

**Expected impact:** Server pump latency drops from 16ms (polling) to
<1ms (event-driven). Agent streaming content appears immediately.

**Validation:** Stream a large agent response. Before the fix, content
arrives in visible 16ms-cadence chunks. After, it streams smoothly.

### Phase 3: Incremental Render (medium-high impact, medium risk)

1. Introduce `HighlightCache` struct.
2. Add dirty-marking hooks to editor mutation paths.
3. Replace the all-lines highlight calls in `render()` with cache lookups.
4. Add `mark_appended()` calls in the pump apply path.

**Expected impact:** Render cost drops from O(total_lines) to O(changed_lines)
for typical frames. Combined with Phase 1 (fewer spurious notifies), this
eliminates the multiplicative cost of pump * render.

**Validation:** Open a 5000-line document. Type a character. Measure frame
time. Before: tens of ms. After: <2ms.

---

## 6. Invariants to Maintain

1. **Single-writer on model state.** All mutations to `YaldaGpuiView` still
   go through `this.update()`. We are not introducing interior mutability or
   concurrent writes. We are only moving *reads from external channels*
   outside the lock.

2. **No event reordering.** Events are drained in FIFO order from the channel
   and applied in the same order. The grouping by session in the server pump
   preserves per-session ordering (HashMap iteration order does not matter
   because events within each session's Vec are ordered).

   **Correction:** If cross-session ordering matters (e.g., session A's event
   must be applied before session B's event because they arrived in that
   order), do NOT group by session. Instead, apply the flat `all_notes` Vec
   in order. The grouping is an optimization for when sessions are independent,
   which they typically are. If in doubt, skip the grouping and apply linearly.

3. **Notify-after-release.** `cx.notify()` is always called after the
   `this.update()` closure returns, never inside it. This is not strictly
   required by GPUI (notification is deferred anyway), but it makes the
   invariant auditable: the lock is never held when we schedule re-render.

4. **Highlight cache coherence.** The cache generation counter must be
   compared against the editor's generation on every render. If they diverge
   (e.g., undo/redo that the cache did not observe), full invalidation fires.
   This is the safety net; the dirty-marking hooks are the fast path.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `Arc<SessionServer>` outlives the view | The pump task already returns when `this.update()` returns `Err` (view dropped). The Arc will be dropped when the task exits. |
| Dirty range tracking misses an edit | Generation counter forces full re-highlight on mismatch. Worst case: one frame at O(n), then back to incremental. |
| `highlight_markdown_lines` is not pure (internal state) | Wrap it: pass a slice of lines, get back a Vec of spans. If it uses a stateful parser, the cache's context window (3 lines before/after) handles re-sync. For fenced code blocks spanning many lines, widen the context or track open-fence state in the cache. |
| Wake signal floods (thousands of `unbounded_send` per second) | The pump drains wake_rx with `now_or_never()` after the first wake, collapsing all pending signals. The channel is unbounded so sends never block the producer. Memory is bounded by the number of unprocessed signals, which is bounded by the number of unprocessed notifications, which we drain every cycle. |
| Large batch in a single `this.update()` still takes too long | The 4096-event cap plus the apply-only lock scope means the lock holds only for state mutation, not I/O. If 4096 apply ops still exceed 2ms, reduce the cap. Profile the apply path and optimize hot spots (e.g., batch string appends into a single buffer operation). |

---

## 8. Summary of the FP Perspective

The current code conflates three concerns into one locked critical section:

1. **Data acquisition** (reading from a channel) — a side effect that touches external state
2. **Data transformation** (grouping, parsing) — a pure function over the acquired data
3. **State mutation** (applying to the model) — a controlled side effect on internal state

The fix factors these into a pipeline:

```
acquire (outside lock) -> transform (outside lock) -> apply (inside lock) -> notify (outside lock)
```

Each stage is independently testable. The lock boundary is minimal. The wake
signal replaces polling with a reactive event stream. The highlight cache
replaces full recomputation with incremental, demand-driven evaluation.

This is not a novel architecture. It is the standard functional-reactive
pattern: events arrive on a stream, are transformed by pure functions, and
applied to a model in a single atomic step. The current code simply does not
follow this pattern — it mixes acquisition and mutation inside the same lock,
polls instead of reacting, and recomputes everything on every frame. Fixing
these three things fixes both bugs.
