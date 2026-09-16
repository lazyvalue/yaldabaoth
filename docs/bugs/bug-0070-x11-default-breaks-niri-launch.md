# bug-0070: x11-default-breaks-niri-launch

**Status:** FIXED
**First seen:** 2026-09-16
**Component:** Workspace (GUI boot / Linux windowing backend)

## Symptom

"Yalda won't run." The in-app Rebuild & Restart at 10:07 on 2026-09-16 built
`78e1427` and relaunched; the console log ends at `Yalda starting (pid 196587)`
and the process is gone. Launching `./target/release/yalda-gpui` by hand:

```
[yalda-gpui] using the X11 backend for working window move/resize (GNOME/Wayland
forces client-side decorations Yalda doesn't draw yet); set YALDA_WAYLAND=1 to
force native Wayland
thread 'main' panicked at blade-graphics-0.7.1/src/vulkan/surface.rs:344:90:
called `Result::unwrap()` on an `Err` value: ERROR_INITIALIZATION_FAILED
   3: <blade_graphics::hal::Context>::reconfigure_surface
   4: BladeRenderer::new::<gpui::platform::linux::x11::window::RawWindow>
   5: X11Window::new
```

exit 101, every time. `YALDA_WAYLAND=1 ./target/release/yalda-gpui` launches,
connects to the session server and restores all 34 agent leaves.

## Context / root cause

Two halves, one code and one environmental:

1. **Code (the regression):** `94f53df` ("prefer X11 backend", authored Aug 26,
   landed on main 2026-09-15 via the origin rebase) unsets `WAYLAND_DISPLAY` at the
   top of `main()` on EVERY Wayland session that has a `DISPLAY`, to route GPUI onto
   Xwayland so GNOME/Mutter's refusal of server-side decorations doesn't leave the
   window un-movable. Its own commit message says wlroots-class compositors should
   keep native Wayland — but the predicate `should_prefer_x11` never looks at which
   compositor is running. Scott runs **niri** (`XDG_CURRENT_DESKTOP=niri`), which
   provides SSD; the GUI built on Sep 10 (pre-rebase) ran native Wayland for six
   days. The first launch of a post-rebase binary flipped it to X11.
2. **Environment (why X11 is fatal here):** on 2026-09-11 apt upgraded the NVIDIA
   userspace to 580.178.04 while the kernel module loaded at boot (7 days ago) is
   still 580.173.02 (`nvidia-smi`: "Driver/library version mismatch"; the NVIDIA
   device no longer enumerates in `vulkaninfo`). The long-lived Xwayland `:0`
   can't provide a working Vulkan surface (fails even with `VK_DRIVER_FILES`
   pinned to radv), so `BladeRenderer::new` on the X11 window panics. Native
   Wayland surfaces via radv still work.

Bug-0058/bug-0064 lineage: a panic before the window exists means the relaunch
path shows nothing at all — no splash, no error.

## Planned solution

Make the X11 preference conditional on the compositor actually needing it: a pure
`desktop_refuses_ssd(XDG_CURRENT_DESKTOP)` helper (true for a `GNOME` entry in the
colon-separated list, case-insensitive) threaded into `should_prefer_x11` as a
fifth input. niri / sway / Hyprland / KDE / COSMIC stay native Wayland (the state
that was working); GNOME keeps the 94f53df behavior. `YALDA_WAYLAND=1` remains
the override. Guard: extend `prefer_x11_only_on_wayland_with_x_fallback_and_no_override`
with the niri and GNOME cases, observed RED with the desktop input ignored.

The driver mismatch is out of scope for the code fix (a reboot reloads the
module); after the fix it no longer affects launching.

## Approaches already tried (do NOT repeat)

- <none yet>

---

## Log

### 2026-09-16 10:15 — gate the X11 preference on GNOME (Cog graph `hdm`)

**Localization (real path).** `./target/release/yalda-gpui` at `78e1427` under
niri: exit 101, panic in `blade_graphics::hal::Context::reconfigure_surface`
from `X11Window::new`, every launch. `YALDA_WAYLAND=1`: launches, connects,
restores 34 leaves. `VK_DRIVER_FILES=radeon_icd.json` on X11 still panics —
the X11/Xwayland surface is the broken piece, not the ICD choice. `nvidia-smi`
reports "Driver/library version mismatch" (userspace 580.178.04 installed
2026-09-11, module 580.173.02 loaded at boot).

**Change.** `src/bin/yalda-gpui/main.rs`: `should_prefer_x11` gains a fifth
input `desktop_refuses_ssd`, ANDed into the decision; new pure
`desktop_refuses_ssd(Option<&str>)` matches a `GNOME` entry (case-insensitive)
in the colon-separated `XDG_CURRENT_DESKTOP`; the wrapper reads the env var and
passes it. Unset/unknown desktops ⇒ native Wayland.

**Verification.**
- Guards (`src/bin/yalda-gpui/tests.rs`):
  `prefer_x11_only_on_wayland_with_x_fallback_and_no_override` (adds the niri
  case + GNOME on every existing case) and `desktop_refuses_ssd_matches_only_gnome`.
- Negative control: with the predicate wired but ignoring the new input, the
  guard failed at `tests.rs:18` — `assertion failed:
  !should_prefer_x11(true, true, false, false, false)` — exactly the niri case.
  Green after the one-line predicate change.
- Full suite: `cargo test --bin yalda-gpui` → 836 passed, 0 failed, 1 ignored.
- Real path: rebuilt `target/release/yalda-gpui` (Sep 16 10:12), launched under
  the real niri env with NO overrides as a 15 s bounded probe: no X11 banner, no
  panic, `connected to session server`, `restore(server): 34 agent leaves`,
  stayed up until the timeout (exit 124). Before the fix the same command was
  exit 101. No running GUI or server was touched (none was running).

**Out of scope / still true.** The NVIDIA userspace/module mismatch persists
until a reboot; it only matters for the X11 path now, and for anything else on
the box that wants the discrete GPU's Vulkan. GNOME users keep the 94f53df
behavior; if GNOME's Xwayland Vulkan is ever broken the same way, this fix does
not help them (a runtime fallback from a blade panic is not possible without a
gpui change).
