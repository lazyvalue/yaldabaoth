# bug-0029: model-badge-click-opens-nothing

**Status:** FIXED
**First seen:** 2026-08-04
**Component:** `docs/components/agent-tile/model.md` (`UXI-AgentTile-16`)

## Symptom

In an agent tile the status-strip model badge renders with its `▾` affordance
(so the session HAS an advertised picklist — `available_models` is non-empty),
but **clicking it presents nothing at all**. No menu, no overlay, no flicker.
The same picklist is reachable from the keyboard via `space → M → <n>`.

Reported: "There's nothing when I click the dropdown. I see a down arrow. But
nothing is presented."

## Context / root cause

`UXI-AgentTile-16` states the switcher "is reachable two ways that share one
dispatch path: the keyboard `space → M → <n>` submenu and clicking the
status-strip `model ▾` badge." The click half is dead.

The badge's `on_click` (screens.rs, `render_agent`'s status strip) does:

```rust
.on_click(|_ev, window, cx| {
    window.dispatch_action(Box::new(crate::OpenLocalMenu), cx);
})
```

`window.dispatch_action` dispatches along the **focused** node's dispatch path,
so an `OpenLocalMenu` raised from inside an agent tile is only handled if the
`AgentView` root registers a listener for it. It does not: `on_action(Self::open_local_menu)`
appears exactly twice in the whole GUI — on the `YaldaView` (doc) root and on
the `BrowserView` root. The `AgentView` root registers ~50 other actions but not
this one, so the dispatched action walks the tree, finds no handler, and is
dropped silently.

The keyboard path works because it never uses the action: `space` is swallowed by
`leader_intercept` inside `handle_claude_key`, which calls `open_local_menu_inner`
directly.

Ruled out (the two known click-death modes in this codebase):
- bug-0019 (backdrop hit-test/`occlude`) — there is no overlay open at click time.
- bug-0023 (element-id re-keyed between down and up) — the badge lives in the
  status strip, not under the transcript's fingerprint-keyed wrapper.

## Planned solution

Register the missing handler on the agent screen root:
`.on_action(cx.listener(Self::open_local_menu))` on the `AgentView` root in
`render_agent`. This restores the single shared dispatch path the spec describes
(badge click and `space` both land in `open_local_menu_inner`) rather than giving
the badge its own bespoke menu-opening code path.

Guard: a headless test that boots a bound agent tile, feeds a real
`ModelsAvailable` reply through the reducer, probes the badge's REAL painted
rect, `simulate_click`s it, and asserts the local menu overlay is open and its
`switch model` submenu carries the advertised model entries (not the
"(models not available yet)" placeholder). Negative control: revert the
`on_action` line → no overlay opens.

## Approaches already tried (do NOT repeat)

- <none yet>

---

## Log

### 2026-08-04 13:40 — opened

Localized to the missing `on_action(Self::open_local_menu)` on the `AgentView`
root by static inspection (the action is registered only on `YaldaView` and
`BrowserView`). Fix + guard next; entry to be completed once the negative
control has been observed RED.

### 2026-08-04 13:55 — fixed: register the action on the AgentView root

**Changed.** `screens.rs` `render_agent`: added
`.on_action(cx.listener(Self::open_local_menu))` to the `AgentView` root (one
line + comment). Also wrapped the clickable badge in
`probe_bounds("agent-model-badge", …)` so the guard can click its REAL painted
rect.

**Guard.** `verify_harness.rs::agent_model_badge_click_opens_the_model_switcher`
— boots a bound agent tile, feeds a real `ModelsAvailable` through
`apply_server_batch`, probes the badge's painted rect, `simulate_mouse_move` +
`simulate_click`s its center (the window's real mouse dispatch — not a
hand-called `open_local_menu_inner`), then asserts the `AGENT` local menu is
open AND its `switch model` submenu carries the advertised labels with the
current model marked `✓`.

**Negative control — observed RED.** Ran the guard with only the probe wrapper
in place (fix not yet applied):
`panicked … clicking the model ▾ badge presented NOTHING — the dispatched
OpenLocalMenu found no handler on the AgentView root (bug-0029)`. The probe had
already found the badge with real area, so the failure was the dropped action,
not a missing paint. With the `on_action` line added: `ok`. Full suite green
(519 GUI + 162 lib + all integration bins, 0 failed).

**Outcome.** The click path and the `space → M` path now share one dispatch
target exactly as `UXI-AgentTile-16` specifies. Note the badge opens the whole
AGENT menu (models are one level in, under `M`) — that is the spec'd behavior,
not a bespoke model popup.

**Not covered:** the click opens the menu; picking a model from it is a separate
already-guarded path (`set_agent_model_issues_set_config_on_channel`). Runtime
check by a human still worth one click after the release rebuild.
