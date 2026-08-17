# Worklog: Image paste dropped on macOS (bug-0039)

**Date:** 2026-08-16
**Branches touched:** `fix-image-paste` (`1478fad`), fast-forwarded to `main`
(`1478fad`); release binary rebuilt.

## Cog execution evidence

- Graph id: `80l`
- Graph name: `fix-image-paste-mac-clipboard`

### Initial render

```text
graph fix-image-paste-mac-clipboard (frontiers)
frontier 0: implement-mac-pasteboard-read [open]
frontier 1: guard-and-verify [open]
frontier 2: omega [open] (omega)
```

### Node execution

Each node was claimed, its work verified, then closed with output JSON:

- `implement-mac-pasteboard-read` (`qcnp`) — claimed, closed `done`, output:
  `read_clipboard_image_png` + pure `select_clipboard_png_bytes` added,
  `paste_into_compose` reads it before GPUI entries, `objc` dep + cfg(test) seam;
  build green.
- `guard-and-verify` (`indg`) — claimed, closed `done`, output: pure + headless +
  `#[ignore]` real-pasteboard guards, all NC RED; 568 pass; merged ff main
  `1478fad`.
- `omega` (`6yn7`) — claimed, closed `done` with the summary output.

### Final render

```text
graph fix-image-paste-mac-clipboard (frontiers)
frontier 0: implement-mac-pasteboard-read [done]
frontier 1: guard-and-verify [done]
frontier 2: omega [done] (omega)
```

### Notes

- No graph notes were needed; the single root cause is recorded in
  `bug-0039` and INV-UX-21. Node-local outputs captured above.

### Final status

- Status: `complete`

## What happened

The user reported still being unable to paste images into an agent session.
INV-UX-21 shipped the whole path (stage → chip → ACP `ContentBlock::Image` → wire)
on main d9b56b7 but was runtime-unverified. Its single untested assumption — that
GPUI's `read_from_clipboard` yields a `ClipboardEntry::Image` — is false on macOS.

**Root cause.** GPUI 0.2.2's mac `read_from_clipboard`
(`platform/mac/platform.rs:1102-1137`) checks for a string type FIRST and returns
a string-only `ClipboardItem` whenever the board advertises any
`public.utf8-plain-text` — it never reaches its image branch. macOS image copies
from browsers, Finder (file copy), and most apps put a URL/filename text rep on the
board alongside the image. Confirmed empirically: a PNG + URL write leaves the board
advertising `public.png`, `public.tiff`, AND `public.utf8-plain-text` together, so
GPUI hands back the string and the image is dropped. Only a pure screenshot (no
string) worked — hence the "intermittent" feel.

**Fix.** `system_console::read_clipboard_image_png` reads the general
`NSPasteboard` directly (cocoa/objc): prefer `public.png`, else transcode
`public.tiff` to PNG via `NSBitmapImageRep`. `paste_into_compose` calls it before
GPUI's entries and stages the result as `image/png`; GPUI's `ClipboardEntry::Image`
stays the non-mac fallback, then text paste. Added `objc = "0.2"` (mac target). A
`cfg(test)` thread-local override keeps headless tests off the real OS pasteboard.

## Verification

- Pure `select_clipboard_png_prefers_png_and_rejects_empty` — NC RED.
- Headless real-path `image_paste_direct_read_stages_even_with_text_on_clipboard`
  (real `handle_claude_key`→`paste_into_compose`, GPUI clipboard text-only + direct
  read injected) — NC RED observed (disabled the direct-read step).
- `#[ignore]` `read_clipboard_image_png_os_recovers_png_beside_text` — real
  `NSPasteboard` round-trip, recovers the PNG beside a URL string (gap-2 remedy);
  NC RED observed (emulated GPUI short-circuit).
- Full suite: 568 pass / 0 fail / 2 ignored. Release build green.

## Open / NEEDS-RUNTIME

- User must restart the running dev-gui RELEASE binary to get the fix (anti-circling
  rule 5).
- Whether the live `claude-agent-acp` advertises the ACP `image` prompt capability
  is still the separate NEEDS-RUNTIME question from the original INV-UX-21 work — not
  gated on it, may error if unsupported.

## Artifacts

- `docs/bugs/bug-0039-image-paste-dropped-on-macos.md` (+ manifest row)
- `docs/ux-invariants.md` — INV-UX-21 updated (mac direct-read) + changelog entry
