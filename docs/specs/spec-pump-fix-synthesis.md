# Spec: Pump & Render Fix — Synthesis

**Status:** Ready for implementation
**Date:** 2026-05-28
**Source specs:**
- `spec-pump-fix-fp.md` — FP/reactive perspective
- `spec-pump-fix-distributed.md` — distributed systems perspective

---

## Background

Two bugs compound into an unusable editor:

1. **Typing latency:** Multi-second delay between keystroke and character appearing on screen.
2. **Server pump drops/delays agent content:** Fast-streaming agents queue thousands of events that sit unprocessed.

Two independent specs were written by agents with different expertise. This document synthesizes their shared conclusions into a single implementation plan.

---

## Root Causes (unanimous agreement)

### RC1: O(n) render on every frame (PRIMARY CAUSE of typing latency)

The agent pane's render method does this on **every `cx.notify()`**:

1. Extracts ALL lines from the editor as `Vec<String>` — O(n) copy
2. `highlight_markdown_lines()` on ALL lines — O(n) tokenization
3. `highlight_markdown_lines_stripped()` on ALL lines — second O(n) pass
4. Per-line gutter tags for ALL lines — O(n)
5. Frozen range block detection — O(n)
6. Builds the virtualised list — O(visible) ← only this is efficient

**Cost estimate:** ~200ms per frame at 1000 lines. At 30-60 notifies/sec during streaming, the main thread is 100% saturated with render work. Zero budget left for keystroke processing.

**Location:** `main.rs` lines ~9474-9529, the agent pane render path.

### RC2: Lock convoy on `this.update(cx, ...)`

Both pumps call `this.update(cx, |this, cx| { ... })` which takes a mutable borrow on the entire `SketchGpuiView`. While held, keystroke handlers and render cannot proceed.

The **server pump** is worst: it calls `this.update()` twice per batch — once to drain from the channel (unnecessary, channel is thread-safe), once to route/apply. Processing 256 events inside the lock blocks keystrokes for 2-10ms per batch.

**Key insight:** `server.try_recv()` does not need the model lock. The `SessionServer` handle is `Arc`-wrapped and `try_recv()` only needs `&self`.

### RC3: Server pump polls without wake signal

Pure polling at 16ms intervals. Minimum 16ms latency on every notification. Throughput ceiling of ~16k events/sec. Compare: the ACP pump has a `wake_rx` signal and uses `select_biased!` — it works fine.

### RC4: Excessive `cx.notify()` calls

Both pumps call `cx.notify()` per batch. During a drain cycle with multiple batches, this schedules multiple re-renders, each triggering the O(n) render path.

---

## Why Previous Fixes Failed

| Fix attempted | Why it didn't work |
|---|---|
| **Drain loop (batch 256, inner loop)** | Larger batches = longer lock hold time. Lock held for entire batch, blocking keystrokes. Throughput improves but latency gets worse. |
| **Conditional notify (`did_work` check)** | During streaming `did_work` is always true. Reduces notifies from 60/sec to maybe 40/sec. But 40 × 200ms = 8000ms/sec of render work. Still saturated. |
| **Both combined** | Neither addresses the core problem: each notify costs 200ms of main-thread work. You can't fix O(n) render by tweaking the pump. |

The fundamental invariant being violated: **render cost must be bounded by the frame budget (16ms), not proportional to total document size.**

---

## Fix: Three Phases

Each phase is independently shippable and testable. Ordered by impact.

### Phase 1: Render Cache (HIGHEST IMPACT)

**Goal:** Make render cost O(visible + changed) instead of O(total_lines).

**What:** Introduce a `HighlightCache` that stores per-line highlight results. Only re-highlight lines that actually changed.

```rust
struct HighlightCache {
    /// Per-line cached highlight spans.
    lines: Vec<Option<CachedLine>>,
    /// Per-line stripped highlight spans.
    stripped: Vec<Option<CachedLine>>,
    /// Dirty line ranges needing re-highlight.
    dirty: Vec<Range<usize>>,
    /// Editor generation for full-invalidation detection.
    generation: u64,
}
```

**Dirty marking hooks:**
- Keystroke insertion → `mark_dirty(cursor_line..cursor_line+1)`
- Agent streaming append → `mark_dirty(old_line_count..new_line_count)`
- Paste / bulk edit → mark affected range
- Theme change → `invalidate_all()`
- Undo/redo (generation mismatch) → `invalidate_all()`

**Render path change:**
```rust
// Before (current):
let highlighted = highlight_markdown_lines(&all_lines, &theme);          // O(n)
let stripped = highlight_markdown_lines_stripped(&all_lines, &theme);     // O(n)

// After:
cache.update_dirty_lines(&editor, &theme);   // O(changed_lines)
// Then use cache.lines[i] / cache.stripped[i] in the virtualised list builder
```

**Context window:** Markdown fenced code blocks are stateful (in_fence flag). When marking dirty, expand the range by 3 lines in each direction to handle block-level construct boundaries. The `highlight_markdown_lines` function tracks `in_fence` state, so partial re-highlighting needs to know the fence state at the start of the dirty range. Two approaches:
1. Store per-line `in_fence` state in the cache (preferred — O(1) lookup).
2. Scan backwards from the dirty start to find the last fence toggle.

**Impact estimate:**

| Scenario | Before | After |
|---|---|---|
| Type 1 char (1000-line doc) | ~200ms | ~0.2ms |
| Agent streams 10 lines | ~200ms | ~2ms |
| Scroll (no edit) | ~200ms | ~0ms (cache hit) |

**Files:** `main.rs` (agent pane render method, ~line 9474+). Possibly a new `highlight_cache.rs` module.

**GPUI constraint note:** `render()` takes `&mut self`, so the cache can live directly on `SketchGpuiView` — no `RefCell` needed.

### Phase 2: Extract-Then-Apply (Lock Decoupling)

**Goal:** Minimize lock hold time. Channel reads happen outside the model lock.

**Server pump — new structure:**
```
loop {
    // 1. WAIT (no lock)
    sleep or wake_rx

    // 2. EXTRACT (no lock) — channel is Arc, try_recv is &self
    let batch = server.try_recv() in a loop, cap at 4096

    // 3. APPLY (lock held, minimal work)
    this.update(cx, |this, cx| {
        this.apply_server_batch(batch);
    });

    // 4. NOTIFY (after lock released) — exactly ONE per cycle
    cx.notify();
}
```

**Key change:** Clone `Arc<SessionServer>` once at pump creation. Call `try_recv()` outside `this.update()`. One `this.update()` per cycle (not two per batch). One `cx.notify()` per cycle (not per batch).

**ACP pump — same pattern:** Split `pump_session()` into:
- `drain_events(receiver, budget)` → pure channel read, outside lock
- `apply_events(&mut self, events)` → state mutation, inside lock

**Implementation note:** The ACP session's channel receiver needs to be extractable from the model. Either wrap in `Arc` or `take()` it into the pump task at creation time, same as the server pump pattern.

**Files:** `main.rs` (`start_server_pump` ~line 7637, ACP pump ~line 7439, `pump_session` ~line 7827). Possibly `session_client.rs` for receiver extraction.

### Phase 3: Wake Signal for Server Pump

**Goal:** Replace 16ms polling with event-driven wake.

**What:** Add a `futures::channel::mpsc::unbounded()` wake channel to `SessionServer`. Producer sends `()` on every notification. Pump uses `select_biased!` on wake_rx + 100ms heartbeat timeout.

```rust
// In SessionServer:
fn send_notification(&self, note: ServerNotification) {
    self.notification_tx.send(note).ok();
    let _ = self.wake_tx.unbounded_send(()); // wake the pump
}

// In pump:
futures::select_biased! {
    _ = wake_rx.next().fuse() => {
        while wake_rx.next().now_or_never().flatten().is_some() {} // drain coalesced
    }
    _ = timer(100ms).fuse() => {} // heartbeat
}
```

**Files:** `session_client.rs` or wherever `SessionServer` is defined, `main.rs` (`start_server_pump`).

---

## Implementation Order

```
Phase 1 (render cache)  →  fixes typing latency
Phase 2 (lock decouple) →  fixes pump blocking keystrokes during streaming
Phase 3 (wake signal)   →  fixes server pump latency/throughput
```

Phase 1 alone should make typing responsive. Phases 2 and 3 improve streaming quality and reduce contention.

---

## Invariants

1. **Render cost < 4ms per frame.** No O(total_lines) work in render.
2. **Lock hold < 2ms.** No channel I/O inside `this.update()`.
3. **At most 1 `cx.notify()` per pump cycle.**
4. **Highlight cache conservative invalidation.** Generation counter catches any missed dirty marks. Worst case: one O(n) frame, then back to incremental.
5. **Event ordering preserved.** FIFO from channel, applied in order.

---

## What NOT to Do

1. Don't add more `sleep()` / `timer()` calls to "throttle" the pump.
2. Don't debounce `cx.notify()` with a timer (adds latency to keystrokes).
3. Don't increase event budget beyond 256 (longer lock hold).
4. Don't move rendering to a background thread (GPUI is main-thread-only).
5. Don't replace `std::sync::mpsc` with async channel for data (producer is sync).

---

## Validation

- `cargo check --bin sketch-gpui` clean after each phase
- Type rapidly in agent chatbox during active streaming → characters appear within 1 frame
- Stream a 2000-line agent response → content appears smoothly, no multi-second gaps
- Open a 5000-line agent conversation, type a character → frame time < 4ms
