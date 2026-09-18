# bug-0071: nvidia-wayland-freezes-on-splash

**Status:** OPEN (localized; launch workaround verified; code fix awaits a policy call)
**First seen:** 2026-09-18
**Component:** Workspace (GUI boot / Linux GPU selection)

## Symptom

"Booting up the gui just hangs." `./dev-gui.sh` at 10:30 on 2026-09-18 built and
launched `0bfe571`; the window maps, paints the splash, and never changes again.
Keys/clicks appear dead. The session-server log shows the GUI connected, attached
34 sessions, replayed their durable prefixes, then `conn 1 closed after 104.7s`
when Scott killed it.

## Context / root cause

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

Undecided — needs Scott's call, because every in-repo option moves rendering off
the 5070 Ti:

1. **Env workaround only (verified):** launch with `ZED_DEVICE_ID=0x13c0`. Zero
   code. Machine-specific id; must be set wherever the GUI is launched from
   (shell profile / niri `environment {}` / `dev-gui.sh`), including the in-app
   Rebuild & Restart relaunch, which inherits the running GUI's env.
2. **Code: prefer a non-NVIDIA adapter on native Wayland** when one exists and
   `ZED_DEVICE_ID` is unset — set `ZED_DEVICE_ID` in `main()` next to
   `should_prefer_x11`, behind a pure predicate with a guard test and a
   `YALDA_GPU=` override. Survives driver updates; costs iGPU rendering even
   once NVIDIA fixes the driver.
3. **Driver side:** pin/downgrade `libnvidia-gl-580` to 580.173.02, or try a
   newer driver. Out of repo.

A headless guard cannot reproduce this (genuine gap 1/2: real GPU presentation);
any code fix is guarded at the predicate level and verified on the real path by
a window screenshot past the splash deadline, as done here.

## Approaches already tried (do NOT repeat)

- <none yet>

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
