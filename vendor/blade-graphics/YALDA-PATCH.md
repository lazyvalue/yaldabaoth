# Yalda patch over blade-graphics 0.7.1

Upstream: crates.io `blade-graphics` 0.7.1 (git `88b6c64cd7a32dd933acc4e5266808a07143ce84`,
`blade-graphics/`). Wired in through `[patch.crates-io]` in the root `Cargo.toml`.
Dropped from the vendored copy: `etc/` (README images, 2.5 MB), `Cargo.lock`,
`Cargo.toml.orig`, cargo metadata dotfiles. Everything else is byte-identical
except the change below. Bug record: `docs/bugs/bug-0071-*.md`.

## Why

gpui 0.2.2 (the newest release) pins blade-graphics 0.7.1. Its Vulkan
`Surface::acquire_frame` answers `VK_ERROR_OUT_OF_DATE_KHR` by logging a warning
and returning a frame with no image; `present` then silently drops it. Nothing
recreates the swapchain — gpui only calls `reconfigure_surface` on a window
resize — so once a driver invalidates the swapchain for any other reason, every
later frame is discarded and the window stays frozen on its last presented
image. NVIDIA 580.178.04 does exactly that on native Wayland when the compositor
(niri) sends updated `zwp_linux_dmabuf_feedback` a few frames after map: acquires
1–3 succeed, every acquire after returns OUT_OF_DATE. Mesa/radv never returns it
there, which is why the bug is driver-dependent. Recreating on OUT_OF_DATE is
what the Vulkan spec asks of the application (vkcube does; it runs fine on the
same GPU).

## What changed (`src/vulkan/mod.rs`, `src/vulkan/surface.rs`)

- `Surface` gains `recreate: Option<SwapchainRecreate>` — the inputs of the last
  successful `reconfigure_surface` (a clone of the core `ash::Device`, image
  count, surface format, extent, usage, composite alpha, present mode, queue
  family, exclusive-fullscreen flag).
- The swapchain + per-image frame construction at the tail of
  `Context::reconfigure_surface` moved verbatim into
  `Surface::build_swapchain(&SwapchainRecreate)`; `reconfigure_surface` computes
  the params exactly as before, builds through it, and stores the params.
- `Surface::acquire_frame` = `try_acquire_frame`; on OUT_OF_DATE it calls
  `recreate_swapchain` (same build path, `old_swapchain` set, `device_wait_idle`
  via the existing `deinit_swapchain`) and retries once. If the retry is also
  out of date it falls back to upstream's image-less frame, so a genuine
  resize race still behaves as before until gpui reconfigures.

No GPU, driver, or compositor is special-cased.

## Dropping this patch

Remove the `[patch.crates-io]` entry and `vendor/blade-graphics/` once Yalda moves
to a gpui release whose renderer recreates an out-of-date swapchain. Re-verify
with the bug-0071 real-path probe (window screenshot past the splash deadline on
an NVIDIA GPU under native Wayland).
