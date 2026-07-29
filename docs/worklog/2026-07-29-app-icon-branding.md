# App icon branding — 2026-07-29

## Outcome

The running macOS application now uses the embedded `yaldabaoth-logo.png` as
its Dock and app-switcher icon. This is the same source image used by the system
console watermark and startup splash.

## Implementation

- Added a macOS-only AppKit bridge that creates an `NSImage` directly from the
  already embedded PNG bytes.
- Installed that image at the beginning of GPUI's application callback, after
  AppKit initialization and before Yalda opens its first window.
- Kept a no-op implementation for other platforms so the shared startup path
  remains portable.

## Verification

- `cargo check --bin yalda-gpui`
- Existing system-console focused unit tests
