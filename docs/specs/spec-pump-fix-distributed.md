# Spec: Pump Contention & Render Amplification Fix

**Status:** Draft
**Date:** 2026-05-28
**Problem:** Multi-second typing latency + dropped/delayed agent streaming content

---

## 1. Root-Cause Analysis

There are three independent failure modes that compound into the observed bugs. All three must be fixed; addressing only one or two will not resolve the symptoms.

### 1.1 Lock convoy on `this.update(cx, ...)`

Both pump paths call `this.update(cx, |this, cx| { ... })` which acquires a mutable borrow on the entire `SketchGpuiView`. This is effectively an exclusive lock. Three producers compete for it:

| Producer | Frequency during streaming | Lock hold time |
|----------|---------------------------|----------------|
| Keystroke handler (main thread) | 5-15 Hz (typing) | ~0.1 ms (insert char) |
| Direct ACP pump (Path A) | up to 62 Hz (budget=64, 1ms yield) | **1-5 ms** (64 events + cx.notify) |
| Server pump (Path B) | up to 62 Hz (budget=256, 1ms yield) | **2-10 ms** (256 events + cx.notify) |

During active agent streaming, the two pumps alternate holding the lock. The keystroke handler on the main thread must wait for whichever pump currently holds it. With two pumps each holding 2-10 ms and cycling at ~16 ms, the lock is contended roughly **25-60% of wall time**. A keystroke that arrives during a pump's critical section waits the remainder of that section -- average wait is half the hold time, so **1-5 ms per keystroke just from lock contention**.

But it's worse: `cx.notify()` is called *inside* the lock, which schedules a re-render. The render path also needs to read the model state. If a pump's `this.update()` fires `cx.notify()` and then continues processing more events in the same closure, the render is queued but cannot start until the lock is released. Multiple `cx.notify()` calls coalesce into one render, but only if they happen before the render actually starts. When pumps yield for 1ms between batches and re-acquire the lock, they can trigger *separate* render passes per batch.

### 1.2 Render amplification: O(n) highlighting on every notify

The agent tile's render method does this every frame:

1. `editor.lines()` -> `Vec<String>` of ALL lines (allocation + copy)
2. `highlight_markdown_lines(&all_lines)` -> full tokenization pass
3. `highlight_markdown_lines_stripped(&all_lines)` -> second full tokenization
4. Iterate ALL frozen ranges for block detection
5. Build virtualised list (only this step is O(visible))

For a 1000-line agent conversation, steps 1-4 are roughly:
- Step 1: ~1000 string copies, ~50 KB allocation = ~0.05 ms
- Step 2: tokenize 1000 lines, ~0.1 ms/line = **~100 ms**
- Step 3: another ~100 ms
- Step 4: linear scan = ~0.5 ms
- **Total: ~200 ms per frame**

During streaming, `cx.notify()` fires at minimum once per pump cycle (16 ms). The server pump with 256-event batches and continued drain can fire multiple notifies per second. Even at a conservative 10 notifies/second, the render path consumes **2000 ms of main-thread time per second** -- the main thread is fully saturated and has zero budget left for keystroke processing.

This is the **primary cause** of multi-second typing latency. The lock contention from section 1.1 adds another 1-5 ms per keystroke on top, but the render saturation is what makes the app unusable.

### 1.3 Server pump: polling without wake signal

The server pump (Path B) uses pure timer-driven polling: sleep 16 ms, then `try_recv()`. During fast agent streaming (1000+ events/sec), the producer can enqueue ~16 events per sleep cycle. The drain loop processes them, but there are two problems:

**Latency floor:** Every event waits an average of 8 ms in the queue before the pump wakes. For bursty producers that send 50 events in 2 ms, the first event waits the full 16 ms.

**Throughput ceiling:** The drain loop pulls 256 events, processes them inside a `this.update()` lock, yields 1 ms, then re-acquires. With a min_cycle floor of 16 ms, the maximum throughput is:
- Best case: 256 events / 16 ms = 16,000 events/sec
- But the drain loop breaks on `Ok(false)` (no work), not on empty channel. If the routing of notifications produces `did_work = false` for certain notification types, the drain exits early even with events still queued.
- Actual throughput during contention: likely 5,000-8,000 events/sec

A fast-streaming agent producing 10,000+ events/sec will outpace the consumer. The unbounded `std::sync::mpsc` channel grows without bound, and events accumulate with increasing latency.

### 1.4 Why previous fixes failed

**The drain loop approach** (current code): Adding an inner `'drain` loop that pulls multiple batches was intended to increase throughput. It does -- but it also increases lock hold time proportionally. Processing 4 batches of 256 events means holding the lock for ~10 ms. This trades throughput for latency: more events processed per cycle, but each cycle blocks the main thread longer. The fundamental invariant violated is: **lock hold time must be bounded by the frame budget (16 ms), and ideally by a fraction of it (~2 ms)**.

**The conditional notify approach** (notify only when `did_work`): This reduces *some* spurious renders, but during active streaming `did_work` is almost always true. The real problem isn't spurious notifies -- it's that each legitimate notify triggers O(n) render work. Reducing notifies from 60/sec to 40/sec still leaves 40 * 200 ms = 8000 ms of render work per second.

Both fixes address symptoms, not root causes. The drain loop addresses throughput without addressing contention. The conditional notify addresses notification count without addressing per-notification cost.

---

## 2. Quantitative Breakdown

### Steady-state during agent streaming (1000 lines accumulated, 500 events/sec)

| Component | Current | Budget |
|-----------|---------|--------|
| Render (per frame) | ~200 ms | < 4 ms |
| Pump lock hold (per cycle, Path A) | 1-5 ms | < 1 ms |
| Pump lock hold (per cycle, Path B) | 2-10 ms | < 1 ms |
| Notifies per second | 30-60 | 1-2 (coalesced) |
| Main thread utilization (render) | > 100% (overloaded) | < 50% |
| Keystroke latency (p99) | 200-2000 ms | < 16 ms |

### Why it collapses

At 60 fps target, the per-frame budget is 16.6 ms. The render path alone consumes ~200 ms -- **12x the budget**. The frame rate drops to ~5 fps. Keystrokes that arrive during render are queued and processed after the current render completes, adding 200 ms of latency per queued frame. If 3 notifies are pending, the keystroke waits behind 3 render passes: **600 ms**.

During heavy streaming (2000+ events/sec), the pump fires notify more frequently, render queue depth grows, and keystroke latency exceeds 1 second.

---

## 3. Proposed Fix

Three orthogonal changes, each independently valuable but all three needed for full fix:

### 3.1 Decouple pump processing from the view lock

**Principle:** The pump should hold the view lock only to *deliver processed results*, never to *receive and parse raw events*.

**Current flow (Path B):**
```
this.update(cx, |this, _cx| {
    // INSIDE LOCK: receive from channel
    server.try_recv() ...
});
// yield
this.update(cx, |this, cx| {
    // INSIDE LOCK: route + apply notifications
    // cx.notify()
});
```

**New flow:**
```
// OUTSIDE LOCK: receive from channel (channel is Send, doesn't need model)
let batch = recv_batch(&server_rx, 256);
if batch.is_empty() { continue; }

// OUTSIDE LOCK: pre-process / classify notifications
let processed = classify_batch(batch);

// INSIDE LOCK: apply pre-processed results (minimal work)
this.update(cx, |this, cx| {
    this.apply_server_batch(processed, cx);
    // single cx.notify() at end
});
```

**Key insight:** `server.try_recv()` does not need `&mut self` -- it needs access to the server handle. Extract the receiver end of the channel into a value owned by the pump task, not by the view model.

#### Concrete changes

**Step 1: Extract channel receiver from view model.**

Currently `session_server` lives on `SketchGpuiView` and the pump accesses it via `this.update()`. Change `start_server_pump` to:

```rust
fn start_server_pump(&self, cx: &mut Context<Self>) -> Task<()> {
    // Extract the receiver ONCE at pump creation.
    let server_rx = self.session_server.as_ref()
        .expect("start_server_pump called without server")
        .take_receiver();

    cx.spawn(async move |this, cx| {
        let idle_delay = Duration::from_millis(16);
        let yield_delay = Duration::from_millis(1);

        loop {
            cx.background_executor().timer(idle_delay).await;

            loop {
                // OUTSIDE LOCK: drain channel directly
                let batch = recv_batch_sync(&server_rx, 256);
                if batch.is_empty() { break; }

                // INSIDE LOCK: apply only
                let cont = this.update(cx, |this, cx| {
                    let did_work = this.apply_server_batch(&batch, cx);
                    if did_work {
                        cx.notify();
                    }
                    did_work
                });

                match cont {
                    Err(_) => return,
                    Ok(false) => break,
                    Ok(true) => {
                        cx.background_executor().timer(yield_delay).await;
                    }
                }
            }
        }
    })
}

/// Drain up to `limit` items from a std::sync::mpsc::Receiver without blocking.
fn recv_batch_sync<T>(rx: &std::sync::mpsc::Receiver<T>, limit: usize) -> Vec<T> {
    let mut batch = Vec::with_capacity(limit);
    while batch.len() < limit {
        match rx.try_recv() {
            Ok(item) => batch.push(item),
            Err(_) => break,
        }
    }
    batch
}
```

If the `SessionServer` type wraps the receiver and doesn't allow extraction, refactor it:

```rust
struct SessionServer {
    // ... other fields ...
    rx: Option<std::sync::mpsc::Receiver<ServerNotification>>,
}

impl SessionServer {
    /// Take the receiver out for use in a dedicated pump task.
    /// Panics if called twice.
    fn take_receiver(&mut self) -> std::sync::mpsc::Receiver<ServerNotification> {
        self.rx.take().expect("receiver already taken")
    }
}
```

This one change alone should cut the server pump's lock hold time from 2-10 ms (receive + process) to ~0.5-2 ms (apply only), because the channel drain happens outside the lock.

**Step 2: Same treatment for Path A (direct ACP pump).**

Apply the same receiver-extraction pattern to the ACP session's `mpsc` channel. `pump_session()` currently takes `&mut self` to access the session's channel. Refactor so the channel receiver is owned by the pump task:

```rust
fn start_acp_pump(
    &mut self,
    session_index: usize,
    cx: &mut Context<Self>,
) -> Task<()> {
    let event_rx = self.sessions[session_index].take_event_receiver();
    let wake_rx = self.sessions[session_index].take_wake_receiver();

    cx.spawn(async move |this, cx| {
        let mut wake_rx = Some(wake_rx);
        loop {
            // ... wake/timer select as before ...

            loop {
                // OUTSIDE LOCK: drain
                let batch = recv_batch_sync(&event_rx, 64);
                if batch.is_empty() { break; }

                // INSIDE LOCK: apply
                let more = this.update(cx, |this, cx| {
                    this.apply_acp_batch(session_index, &batch, cx)
                });

                match more {
                    Err(_) => return,
                    Ok(false) => break,
                    Ok(true) => {
                        cx.background_executor().timer(Duration::from_millis(1)).await;
                    }
                }
            }
        }
    })
}
```

**Step 3: Single `cx.notify()` per pump cycle, not per batch.**

Both pumps currently call `cx.notify()` per inner-loop batch. When the drain loop iterates 4 times, that's 4 notifies. GPUI may coalesce these, but only if no render starts between them. With the 1ms yield between batches, renders *can* interleave.

Fix: accumulate a `did_any_work` flag across the entire drain loop. Call `cx.notify()` once, after the drain loop exits:

```rust
loop {
    let mut did_any_work = false;

    loop {
        let batch = recv_batch_sync(&server_rx, 256);
        if batch.is_empty() { break; }

        let did_work = this.update(cx, |this, _cx| {
            // DO NOT call cx.notify() here
            this.apply_server_batch(&batch, _cx)
        }).unwrap_or(false);

        did_any_work |= did_work;
        if !did_work { break; }
        cx.background_executor().timer(yield_delay).await;
    }

    // SINGLE notify after all batches in this cycle
    if did_any_work {
        let _ = this.update(cx, |_this, cx| cx.notify());
    }

    // ... idle delay ...
}
```

This bounds the number of notifies from the server pump to at most 1 per cycle (~60/sec), regardless of how many batches were drained.

### 3.2 Add wake signal to server pump

The server pump's 16 ms polling interval adds unnecessary latency and creates a throughput ceiling. Add an `futures::channel::mpsc::UnboundedSender<()>` wake signal, mirroring what Path A already has.

**Changes to SessionServer:**

```rust
struct SessionServer {
    // existing fields...
    wake_tx: futures::channel::mpsc::UnboundedSender<()>,
}

impl SessionServer {
    fn new() -> (Self, futures::channel::mpsc::UnboundedReceiver<()>,
                       std::sync::mpsc::Receiver<ServerNotification>) {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let (wake_tx, wake_rx) = futures::channel::mpsc::unbounded();
        let server = SessionServer {
            event_tx,
            wake_tx,
            // ...
        };
        (server, wake_rx, event_rx)
    }

    /// Called by the producer thread when sending a notification.
    fn send(&self, note: ServerNotification) {
        let _ = self.event_tx.send(note);
        let _ = self.wake_tx.unbounded_send(());
    }
}
```

**Updated pump loop:**

```rust
cx.spawn(async move |this, cx| {
    let mut wake_rx = wake_rx;
    loop {
        // Event-driven: wait for wake OR 50ms timeout (heartbeat)
        let timer = cx.background_executor().timer(Duration::from_millis(50));
        futures::select_biased! {
            _ = wake_rx.next().fuse() => {}
            _ = timer.fuse() => {}
        }
        // Drain any coalesced wakes
        while wake_rx.next().now_or_never().flatten().is_some() {}

        // ... drain loop as in 3.1 ...
    }
})
```

The timeout increases from 16 ms to 50 ms because the wake signal handles the latency-sensitive path. The timeout is now only a heartbeat for robustness.

### 3.3 Fix the render path: incremental highlighting

This is the highest-impact change. The current O(n) render path must become O(visible + changed).

#### 3.3.1 Cached highlight state

Introduce a `HighlightCache` struct on the agent tile:

```rust
struct HighlightCache {
    /// Per-line highlighted spans, indexed by line number.
    spans: Vec<Option<Vec<HighlightSpan>>>,
    /// Per-line stripped highlight spans.
    stripped_spans: Vec<Option<Vec<HighlightSpan>>>,
    /// Per-line gutter tags.
    gutter_tags: Vec<Option<GutterTag>>,
    /// Frozen range block assignments.
    frozen_blocks: Vec<(usize, usize, BlockType)>,
    /// The line count at last full recompute.
    line_count: usize,
    /// Dirty range: lines [dirty_start, dirty_end) need re-highlighting.
    dirty_range: Option<(usize, usize)>,
}
```

#### 3.3.2 Dirty tracking

When an edit occurs (keystroke, paste, agent append), mark the affected line range dirty:

```rust
impl HighlightCache {
    fn mark_dirty(&mut self, start: usize, end: usize) {
        match &mut self.dirty_range {
            Some((s, e)) => {
                *s = (*s).min(start);
                *e = (*e).max(end);
            }
            None => {
                self.dirty_range = Some((start, end));
            }
        }
    }

    fn mark_all_dirty(&mut self) {
        self.dirty_range = Some((0, self.line_count));
    }
}
```

For agent streaming, the append pattern is: new content is always appended to the end of the buffer. So `mark_dirty(old_line_count, new_line_count)` -- only the new lines need highlighting. This makes the common case O(new_lines), not O(total_lines).

For typing in the chatbox editor, the edit is at the cursor position: `mark_dirty(cursor_line, cursor_line + 1)`.

#### 3.3.3 Incremental re-highlight in render

```rust
fn render_agent_tile(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let editor = &self.agent_editor;
    let cache = &mut self.highlight_cache;

    if let Some((start, end)) = cache.dirty_range.take() {
        let lines = editor.lines_range(start, end);
        let highlighted = highlight_markdown_lines(&lines);
        let stripped = highlight_markdown_lines_stripped(&lines);

        cache.spans.resize_with(editor.line_count(), || None);
        cache.stripped_spans.resize_with(editor.line_count(), || None);

        for (i, (h, s)) in highlighted.into_iter().zip(stripped).enumerate() {
            cache.spans[start + i] = Some(h);
            cache.stripped_spans[start + i] = Some(s);
        }
    }

    build_virtualised_list(cache, visible_range, cx)
}
```

**Handling the mutability problem:** GPUI's `render()` takes `&self`, not `&mut self`. Wrap `HighlightCache` in `RefCell<HighlightCache>`:

```rust
struct SketchGpuiView {
    // ...
    highlight_cache: RefCell<HighlightCache>,
}
```

The render method calls `self.highlight_cache.borrow_mut()` for the incremental update, then `self.highlight_cache.borrow()` for reading during element construction. Since render is single-threaded, this is safe.

#### 3.3.4 Quantitative impact

| Scenario | Current | With cache |
|----------|---------|------------|
| Agent appends 10 lines (streaming) | 200 ms (rehighlight 1000 lines) | ~2 ms (highlight 10 lines) |
| User types 1 char in chatbox | 200 ms | ~0.2 ms (highlight 1 line) |
| Scroll (no content change) | 200 ms | ~0 ms (cache fully valid) |
| Initial load / full invalidation | 200 ms | 200 ms (same, cold cache) |

---

## 4. Implementation Plan

Ordered by impact and independence. Each phase is independently shippable.

### Phase 1: Render cache (highest impact, zero pump changes)

**Files changed:** `main.rs` (agent tile render methods)

1. Add `HighlightCache` struct (~80 lines).
2. Add `RefCell<HighlightCache>` field to `SketchGpuiView`.
3. Modify the agent tile render method to check `dirty_range`, re-highlight only dirty lines, and use cached spans.
4. Add `mark_dirty()` calls to all content mutation sites: `apply_server_batch()`, chatbox keystroke handler, `pump_session()` event application, session switch (`mark_all_dirty`).

**Risk:** Low. Read-path optimization, no write-path semantic changes. Fallback: `mark_all_dirty()` degrades to current behavior.

**Testing:** `#[cfg(debug_assertions)]` mode that runs both paths and asserts equality.

### Phase 2: Decouple pump from view lock

**Files changed:** `main.rs`, `session_server.rs`, `session_client.rs`

1. Add `take_receiver()` to `SessionServer`.
2. Refactor `start_server_pump()` to receive outside the lock.
3. Add `take_event_receiver()` / `take_wake_receiver()` to ACP session.
4. Refactor ACP pump similarly.
5. Coalesce `cx.notify()` to once per cycle.

**Risk:** Medium. Ownership change for receivers. Pump lifetime already tied to sender drop (channel disconnection exits pump).

### Phase 3: Wake signal for server pump

**Files changed:** `session_server.rs`, `main.rs`

1. Add `(wake_tx, wake_rx)` channel pair to `SessionServer`.
2. Send wake signal on every `send()`.
3. Replace poll timer with `select_biased!` + 50 ms heartbeat.

**Risk:** Low. Mirrors existing Path A pattern.

### Phase 4 (optional): Backpressure

Replace unbounded `std::sync::mpsc` with bounded channel (`crossbeam::channel::bounded(8192)`). Producer blocks when full, applying backpressure. Safe because producer is on a background thread.

---

## 5. Interaction Between Fixes

| Fix | Notify rate | Per-notify cost | Main-thread utilization |
|-----|-------------|-----------------|------------------------|
| Current | 30-60/sec | ~200 ms | > 100% (saturated) |
| + Render cache only | 30-60/sec | ~2 ms | ~6-12% |
| + Lock decoupling | 30-60/sec | ~2 ms + shorter lock wait | ~5-10% |
| + Notify coalescing | ~2-5/sec | ~2 ms | ~1-2% |
| All three | ~2-5/sec | ~2 ms | ~1-2% |

The render cache is by far the highest-leverage fix.

---

## 6. Critical Invariants

1. **Lock hold time < 2 ms.** No `this.update()` closure should do work proportional to queued event count.
2. **Render cost = O(visible + changed), not O(total).** Cache invalidated only for changed lines.
3. **At most 1 `cx.notify()` per pump cycle.** Multiple batches produce one notify.
4. **Pump liveness independent of view lock.** Channel drain happens outside the lock.
5. **Dirty tracking is conservative.** False positives (re-highlight clean lines) cost performance, not correctness.

---

## 7. Functions to Change

### `main.rs`

| Function | Change |
|----------|--------|
| `start_server_pump()` | Extract receiver before spawn; drain outside lock; single notify per cycle |
| ACP pump (`cx.spawn` in session start) | Same: extract receivers, drain outside lock |
| `pump_session()` | Split into `drain_session_events()` (outside lock) and `apply_session_events()` (inside lock) |
| Agent tile render method | Use `HighlightCache`; only re-highlight dirty lines |
| `apply_server_batch()` (new) | Notification routing, called inside lock with pre-drained batch |
| `apply_acp_batch()` (new) | ACP event application, called inside lock with pre-drained batch |

### `session_server.rs`

| Function | Change |
|----------|--------|
| `SessionServer::new()` | Return `(Self, UnboundedReceiver<()>, Receiver<ServerNotification>)` |
| `SessionServer::send()` | Also send wake signal |
| `SessionServer::take_receiver()` (new) | Move receiver out for pump ownership |

### `session_client.rs`

| Function | Change |
|----------|--------|
| ACP session struct | Add `take_event_receiver()`, `take_wake_receiver()` |

### New types

| Type | Description |
|------|-------------|
| `HighlightCache` | Per-line cached highlight spans with dirty range tracking |

---

## 8. What NOT to Do

1. **Do not add more `sleep()` / `timer()` calls to "throttle" the pump.** Trades latency for reduced contention; doesn't fix root cause.
2. **Do not debounce `cx.notify()` with a timer.** Adds latency to all renders including keystrokes.
3. **Do not move rendering to a background thread.** GPUI render is main-thread-only; complexity for marginal gain once cache exists.
4. **Do not replace `std::sync::mpsc` with async channel for event data.** Producer is synchronous. `std::sync::mpsc` for data + `futures::channel::mpsc` for wake is the correct split.
5. **Do not increase event budget beyond 256.** Larger batches = longer lock hold. Budget should *decrease* to 64-128 now that receive is outside the lock.
