# bug-0071: nvidia-wayland-freezes-on-splash

**Status:** FIXED
**First seen:** 2026-09-18
**Component:** Workspace (GUI boot / Linux GPU selection)

## Symptom

"Booting up the gui just hangs." `./dev-gui.sh` at 10:30 on 2026-09-18 built and
launched `0bfe571`; the window maps, paints the splash, and never changes again.
Keys/clicks appear dead. The session-server log shows the GUI connected, attached
34 sessions, replayed their durable prefixes, then `conn 1 closed after 104.7s`
when Scott killed it.

## Context / root cause

> **Corrected 2026-09-18 11:10.** The headline below ("not a Yalda code
> regression — the NVIDIA WSI stops presenting") was the first-pass conclusion and
> is WRONG about the layer. The driver legally returns `VK_ERROR_OUT_OF_DATE_KHR`;
> blade-graphics 0.7.1 (gpui 0.2.2's renderer) never recreates the swapchain. The
> evidence listed here stands; its interpretation is superseded by the second log
> entry. Kept verbatim as the record of what was believed and why.

**Not a Yalda code regression — the NVIDIA 580.178.04 Vulkan WSI stops presenting
on native Wayland (niri 26.04) after the swapchain's first pass.** The process is
healthy; only the picture is frozen.

Evidence, all on the real release binary under the real niri session:

- gdb, 3 samples over 50 s: main thread idle in `calloop … Poller::wait`, process
  ~7% CPU and falling. No deadlock, no busy loop, no panic. stderr shows
  `connected to session server` + all 34 `restore leaf … BOUND+resume`.
- niri: the window is mapped, focused and on-screen; `grim` full-output captures
  at 5 s and 10 s both show the splash (deadline is 1.5 s), and
  `niri msg action screenshot-window` at 6 s and 31 s are byte-identical.
- `WAYLAND_DEBUG=1`, 8 s: 947 `wl_surface.commit`, 943 `frame` requests, 957
  `wl_callback.done` — the frame-callback loop is alive at refresh rate — but
  only **3 `wl_surface.attach`** ever (one per swapchain image: `wl_buffer#42`,
  `#48`, `#50`) and only 2 `wl_buffer.release`. After each of the 3 images has
  gone out once, no new buffer is ever attached, so the compositor keeps showing
  an early frame (the splash). The 1.5 s splash timer fires and notifies; the
  resulting frame never reaches the screen.
- Same binary pinned to the AMD iGPU — either
  `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json` or
  `ZED_DEVICE_ID=0x13c0` (all ICDs still loaded) — boots past the splash and
  renders the full workspace with all sessions restored.
- apt history: the only relevant package change is `libnvidia-gl-580`
  580.173.02 → 580.178.04 (2026-09-11). niri (26.04ppa3) and Mesa (26.0.8) are
  unchanged since 2026-08-27. The GUI ran on NVIDIA/580.173 native Wayland
  through Sep 10–16.

**Why it surfaced now, and the bug-0070 verification gap.** Between Sep 11 and
the reboot on 2026-09-17 14:13 the NVIDIA userspace/module mismatch meant the
NVIDIA device did not enumerate, so blade silently rendered on radv — which is
what bug-0070's "launches under niri" probe actually exercised. That probe
asserted the process stayed up for 15 s; it never checked that anything past the
first frame painted. The reboot loaded the matching 580.178.04 module, blade's
default (`device_id: 0` ⇒ first suitable adapter ⇒ the discrete RTX 5070 Ti)
went back to NVIDIA, and the first launch on it froze.

Not localized further than "the driver stops attaching buffers": whether
`vkQueuePresentKHR` is silently queuing inside the driver or blade is getting
`acquire_next_image` results it mishandles was not determined (no debug symbols
in the release binary; main thread is not blocked in acquire at sample time).

## Planned solution

Superseded 2026-09-18 11:00 — the first localization stopped one layer too high
(see the second log entry). Scott's requirement: no code that pins Yalda to a
driver/GPU; it must work on whatever the default is, like every other app on the
box. The real defect is app-side: blade-graphics 0.7.1 (gpui 0.2.2's renderer)
never recreates a swapchain that the driver reports `VK_ERROR_OUT_OF_DATE_KHR`.
Fix that, driver-agnostically, in a vendored blade-graphics under
`[patch.crates-io]` (gpui 0.2.2 is the newest release, so no version bump
carries a fix): `acquire_frame` rebuilds the swapchain from the last
`reconfigure_surface` inputs and retries once.

## Approaches already tried (do NOT repeat)

- **"It's the NVIDIA driver; pin the GPU" (`ZED_DEVICE_ID=0x13c0` / prefer a
  non-NVIDIA adapter).** Works as a launch workaround but is the wrong layer:
  the driver's OUT_OF_DATE is legal Vulkan and vkcube presents fine on the same
  GPU. Rejected by Scott; do not reintroduce GPU/driver special-casing.

---

## Log

### 2026-09-18 10:50 — localized to NVIDIA 580.178.04 WSI; workaround verified; no code change

Reproduced on the real path (release binary, real niri session, no GUI was
running beforehand; every probe instance was started and killed by PID by the
investigating agent; the session server was never touched). Evidence as listed
under Context. `ZED_DEVICE_ID=0x13c0 target/release/yalda-gpui` verified by
window screenshot at 7 s: full workspace painted (470 distinct colours in a
64×64 downsample vs. 159 for the frozen splash). No fix committed — the choice
between options 1–3 is Scott's.

### 2026-09-18 11:10 — real root cause: blade never recreates an OUT_OF_DATE swapchain; fixed in a vendored blade (Cog graph `m1w`)

**Why the first entry was incomplete.** "Firefox, Chrome and Factorio just work"
— they present through GL/EGL, where the driver absorbs this. The relevant
comparison is another Vulkan-WSI client: `vkcube --wsi wayland --gpu_number 0`
presented 400/400 frames on the same NVIDIA GPU (400 attaches, 397 releases). So
the driver's WSI works; Yalda's renderer stack was the variable.

**Localization (real release binary, real niri session).**
- The `WAYLAND_DEBUG` trace shows niri sending a fresh
  `zwp_linux_dmabuf_feedback_v1` (scanout tranche) right after the 3rd attach —
  the compositor telling the client its buffers could be scanned out directly.
- gdb Python hook on `<blade_graphics::hal::Surface>::acquire_frame`, dumping the
  sret `Frame`: calls 1–3 return `image_index = Some(0|1|2)`; call 4 onward
  return `image_index = None` with `internal == frames[0]` — byte-for-byte the
  `Err(vk::Result::ERROR_OUT_OF_DATE_KHR)` arm. 98 acquires in 12 s, 95 of them
  image-less; `present` drops each silently (`command.rs:540`).
- blade 0.7.1's arm only `log::warn!`s (Yalda installs no logger, so nothing was
  ever printed) and gpui 0.2.2 calls `reconfigure_surface` only from
  `update_drawable_size*` (window resize / transparency change). Nothing
  recreates the swapchain, so the window is frozen forever on the last presented
  image. radv never returns OUT_OF_DATE for a feedback change, which is the
  whole driver-dependence.

**Change.** `vendor/blade-graphics/` (0.7.1 + patch, documented in
`vendor/blade-graphics/YALDA-PATCH.md`), wired by `[patch.crates-io]` in
`Cargo.toml`: `Surface` remembers the last `reconfigure_surface` inputs; the
swapchain/frame construction is shared (`Surface::build_swapchain`);
`acquire_frame` recreates + retries once on OUT_OF_DATE, else falls back to
upstream behaviour. No GPU/driver/compositor special-casing; no Yalda source
change.

**Verification.**
- Real path, default GPU (NVIDIA RTX 5070 Ti / 580.178.04), env scrubbed of
  `ZED_DEVICE_ID` / `VK_DRIVER_FILES`, launched from the repo root so the real
  workspace restores; `niri msg action screenshot-window` at 10 s:
  **patched** = full workspace + agent transcript painted (479 distinct colours
  in a 64x64 downsample), 85 `wl_surface.attach` in 10 s, 9 swapchain dmabufs
  created (6 initial + 3 from the recreate).
- Negative control, same probe, unpatched `main` binary: splash frozen (159
  colours), **3** attaches, 6 dmabufs. RED for the right reason.
- `cargo test --bin yalda-gpui` with the patched dependency: `836 passed; 0 failed;
  1 ignored`.
- Rebuilt `target/release/yalda-gpui` on `main` (`f02ff63`) re-probed the same
  way: 80 attaches in 9 s, full workspace painted.
- **NEEDS-RUNTIME (genuine gaps 1+2):** no headless guard is possible — the
  defect only exists with a real driver WSI returning OUT_OF_DATE; the harness
  has no GPU presentation path. The guard is the real-path probe above; repeat
  it when dropping the patch or bumping gpui.

**Probe side effect, cleaned up.** One probe was launched with the worktree as
cwd; `workspace.json` / `projects.json` / `acp_sessions.json` are keyed by launch
cwd, so it registered a stray `Bug-0071` project. The main entry was verified
untouched and the stray keys removed (`workspace.json` back to its exact
pre-probe 20 687 bytes). Lesson: run real-path probes with cwd = the repo root.

**Activation.** Build only. No running GUI existed during any probe; every probe
process was started and killed by PID by the agent; the session server was never
touched.
