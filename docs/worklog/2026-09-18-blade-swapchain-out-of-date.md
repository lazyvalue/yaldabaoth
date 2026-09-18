# Worklog: GUI boot freeze — blade never recreated an OUT_OF_DATE swapchain (bug-0071)

**Date:** 2026-09-18

## Cog execution evidence

- Graph id: `m1w`

### Initial render

```text
graph bug-0071-blade-swapchain-out-of-date (frontiers)
frontier 0: vendor-patch-blade [open]
frontier 1: verify-real-path [open]
frontier 2: record-and-merge [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `vajy` `vendor-patch-blade`: claimed → closed; output: blade-graphics 0.7.1
  vendored under `vendor/blade-graphics`, wired via `[patch.crates-io]`,
  `acquire_frame` recreates the swapchain on OUT_OF_DATE; release build OK.
- `trri` `verify-real-path`: claimed → closed; output: real-path GREEN (patched,
  default NVIDIA GPU: full workspace, 85 attaches) and RED (unpatched: frozen
  splash, 3 attaches); `cargo test --bin yalda-gpui` 836 passed / 0 failed.
- `s9um` `record-and-merge`: claimed → closed; output: bug-0071 + manifest
  rewritten, fix fast-forwarded to `main` (`f02ff63`), release binary rebuilt on
  `main` and re-probed GREEN, this worklog validated.
- `vvg4` `omega`: claimed → closed; output: graph complete; activation boundary
  is build-only.

### Notes

- Graph note `deviation`: one probe was launched with the worktree as cwd, which
  registered a stray `Bug-0071` project/workspace key in `~/.yalda` (state is
  keyed by launch cwd). Main entry verified untouched; stray keys removed;
  `workspace.json` back to its exact pre-probe size. Probes now run from the
  repo root.
- The graph was created after localization and before any tracked source edit.
  The earlier record-only commit `dac2ede` (first-pass bug file) predates it.
- Repository-wide `cargo fmt --all --check` is RED on unrelated pre-existing
  files; the vendored crate and the patch are fmt-clean.

## What happened

Scott: "Booting up the gui just hangs." First pass localized the freeze to "the
NVIDIA 580.178 driver stops presenting" and offered a GPU-pin workaround. Scott
rejected that framing — no driver-specific code; every other app works. Correct:
vkcube (also Vulkan WSI) presents fine on the same GPU. A gdb hook on blade's
`acquire_frame` showed 3 real images and then image-less frames forever — the
`ERROR_OUT_OF_DATE_KHR` arm, which blade 0.7.1 handles by warning and dropping
the frame. gpui 0.2.2 only reconfigures on resize, so nothing ever recovered.
niri's post-map dmabuf-feedback update is what makes NVIDIA invalidate the
swapchain; radv never does, which hid the bug while the NVIDIA module was
mismatched (Sep 11–17) and during bug-0070's verification.

## Decisions

- Fix at the defect (blade's OUT_OF_DATE handling), not at GPU selection. gpui
  0.2.2 is the newest crates.io release and pins blade 0.7.1, so the fix is a
  minimal vendored patch (`vendor/blade-graphics/YALDA-PATCH.md` documents the
  diff and when to drop it).
- No headless guard: the defect needs a real driver WSI (genuine harness gap).
  The real-path probe (window screenshot past the splash deadline + attach
  count) is the guard, with an observed-RED control.

## Open / follow-ups

- Upstream: the same defect exists in blade-graphics 0.7.1 / gpui 0.2.2 for any
  Vulkan driver that returns OUT_OF_DATE without a resize. Worth reporting.
- Yalda installs no `log` backend, so blade's warning was invisible. A logger
  behind an env var would have made this a five-minute bug.
- bug-0070's probe asserted "process stays up", not "paints past first frame".
  Real-path boot probes should assert a post-splash screenshot.

## Runtime status

Neither Yalda nor its session server was restarted; no GUI was running at any
point. `target/release/yalda-gpui` is rebuilt from `main` — `./dev-gui.sh`
launches the fixed binary.

### Final status

- Status: `complete`

```text
graph bug-0071-blade-swapchain-out-of-date (frontiers)
frontier 0: vendor-patch-blade [done]
frontier 1: verify-real-path [done]
frontier 2: record-and-merge [done]
frontier 3: omega [done] (omega)
```
