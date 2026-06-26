# Project: Close the verification-harness gaps (headless e2e)

**Status:** in progress — gap #3 (perf gate) DONE; gaps #1 (pixels) + #2
(in-process loop) scoped & ready to build.
**Started:** 2026-06-25
**Source:** `docs/dev-system.md` § "Verification harness" — the three remaining
gaps that force the `NEEDS-RUNTIME` human-oracle tax.

## Why

State-level behavior is already drivable headlessly (`verify_harness.rs`,
~40 `#[gpui::test]`s). Three gaps remain, each producing `NEEDS-RUNTIME` flags a
human must clear by hand. Closing them retires most of that tax. Recommended
order (dev-system): in-process loop → element snapshots → perf gate. We did the
perf gate first (smallest, self-contained); the big two are below.

## The three gaps + status

| # | Gap | Status |
|---|-----|--------|
| #3.3 | `--release` perf gate | ✅ DONE — `benches/render_bench.rs` (criterion) |
| #3.2 | painted-bounds / layout snapshots | ✅ DONE (v1) — `probe_bounds`/`layout_probe_*` (render_blocks.rs) + `compose_caret_row_painted_inside_box_when_wrapped`; proves painted geometry headlessly, validated by injecting the regression |
| #3.1 | full GUI↔server↔agent loop in one process | ticket 001 (scoped) |

**#3.2 (DONE v1):** the layout probe is a `CaptureBounds`-style element that
records a tagged element's PAINTED bounds into a thread-local a `#[gpui::test]`
reads after `run_until_parked`. `compose_caret_row_painted_inside_box_when_wrapped`
uses it to assert the caret row is painted inside the compose box even when the
draft wraps far past the box — the real fix for the recurring caret-below-fold
class (a model test proves the math; this proves the paint). Reusable for any
"panel didn't collapse / element X is where it should be" assertion. Remaining:
extend probes to the other surfaces in the coverage matrix (`001-build-plan.md`).

## #3.3 — perf gate (DONE)

`benches/render_bench.rs` benchmarks the lib hot paths a realistic transcript /
doc runs — `render::render_with_highlighter` + `md_highlight::
highlight_markdown_lines_syn` — over 200- and 1000-paragraph synthetic docs,
optimized. `cargo bench --bench render_bench`. Baseline numbers (this machine):
render 200 ≈ 3.9ms / 1000 ≈ 19ms; highlight 200 ≈ 4.1ms / 1000 ≈ 20ms. Provides
the measurement + criterion's regression report; wiring a CI baseline-compare
threshold (`--save-baseline` / `--baseline`) is the remaining step to make it an
automatic fail-gate. NOTE: the agent transcript's `rebuild_agent_view_model`
lives in the binary (not benchable from `benches/`); its per-keystroke cost is
covered by the existing binary perf-probe test (`tests.rs` ~1349).

## #3.1 — in-process GUI↔server↔agent loop (the keystone, ~900 LOC)

**Verified seam (scout, 2026-06-25):** `SessionServerClient` (`src/session_client.rs`)
is hard-wired to a Unix socket (`socket_path()`, `UnixStream`); there is NO trait
boundary, so a test can't inject an in-process server. The GUI stores it on
`YaldaGpuiView.session_server` and drains it via `start_server_pump` →
`apply_server_batch`. Server-side fakes ALREADY exist (`FakeTransport` /
`FakeAgentControls` / `FakeAgentSpawner`, `acp_channel.rs`, `feature=test-support`)
and `tests/*` drive the REAL server binary in-process over a real socket.
`verify_harness.rs` notes seam tests can't make `sent` true headlessly because
there's no daemon/channel; `install_agent_slot` sidesteps the server entirely.

**Plan (smallest seam):**
1. Extract `trait ServerTransport` (send_frame / recv_line / close) from
   `SessionServerClient`; the production path is a `UnixStream` impl — behavior-
   preserving (existing socket + `tests/session_resilience_test.rs` stay green).
2. `InProcessServerTransport` (channel pair) — new.
3. Run the server actor (`run_manager`) in-process on a tokio task driven by the
   in-process transport, with `AgentSpawner = FakeAgentSpawner` vending
   `FakeTransport`.
4. `SessionServerClient::with_transport(Box<dyn ServerTransport>)` constructor +
   `YaldaGpuiView::new_browser_with_server(...)` overload.
5. `verify_harness.rs::boot_with_in_process_server()` + one e2e test:
   open agent → submit → fake agent streams → pump → render → assert transcript.

**Risk:** step 1 touches the live client lib — must keep the real socket path
byte-identical (the resilience harness is the guard). Do it behind a build check;
it's the only behavior-sensitive step.

**Payoff:** retires the largest batch of `NEEDS-RUNTIME` flags (submit→stream→
reduce→render becomes headlessly assertable).

## #3.2 — element-tree / layout snapshots (pixels)

`run_until_parked()` runs a real layout/paint pass but the harness asserts state,
not what's painted ("spinner cleared", "panel didn't collapse", "right color
after theme switch"). Plan: snapshot the computed element tree / layout bounds
after `run_until_parked` (the high-leverage 80%) via `insta` (already a dev-dep),
or offscreen-render + hash regions. Smaller than #3.1 but needs a GPUI
introspection entry point; scope before building.

## Definition of done (per gap)

Builds + tests green; the new headless coverage replaces specific `NEEDS-RUNTIME`
flags in `docs/backlog.md` (name which). #3.1 additionally: real socket path
unchanged (resilience harness green).
