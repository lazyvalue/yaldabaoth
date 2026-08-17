# bug-0039: image-paste-dropped-on-macos

**Status:** FIXED
**First seen:** 2026-08-16
**Component:** Agent tile compose paste (`paste_into_compose`, INV-UX-21)

## Symptom

Cmd+V of an image into an agent session pastes nothing usable — no image chip
appears, and the compose either stays empty or receives a URL/filename instead of
the picture. Reported as "I still can't paste images into a yalda session."
INV-UX-21 (image paste) shipped on main d9b56b7 but was runtime-unverified.

## Context / root cause

The paste path was correct end to end (staging → chip → ACP `ContentBlock::Image`
→ wire). The single untested assumption was the very first step: the memory note
for INV-UX-21 states "GPUI clipboard carries images (`read_from_clipboard()` →
`ClipboardEntry::Image`)." On macOS that is false for the common case.

GPUI 0.2.2's mac `read_from_clipboard` (`platform/mac/platform.rs:1102-1137`)
checks for a string type **first** and returns a **string-only** `ClipboardItem`
whenever the pasteboard advertises any `public.utf8-plain-text` type — it never
reaches its image branch:

```
if types contains "public.utf8-plain-text" { return string_item }   // <- always wins
for format in ImageFormat::iter() { try image }                     // <- never reached
```

macOS image copies from browsers, Finder (file copy), and most apps put a
text/URL/filename representation on the board **alongside** the image. Empirically
confirmed: writing a PNG + a URL string to the general pasteboard leaves it
advertising `public.png`, `public.tiff`, AND `public.utf8-plain-text` together, so
GPUI hands back the string and `paste_into_compose`'s
`ClipboardEntry::Image` loop stages nothing. A pure screenshot-to-clipboard
(TIFF/PNG only, no string) was the one case that worked — which is why it looked
intermittent.

## Fix

Bypass GPUI for the image read on macOS. New
`system_console::read_clipboard_image_png` reads the general `NSPasteboard`
directly via `cocoa`/`objc`: prefer `public.png`, else transcode a `public.tiff`
rep to PNG via `NSBitmapImageRep`. `paste_into_compose` calls this **first** and
stages the result as `image/png`; only if it finds nothing does it fall back to
GPUI's `ClipboardEntry::Image` entries (non-mac platforms, where GPUI has no
short-circuit) and then to a text paste. Added `objc = "0.2"` as a mac-target dep.

A `#[cfg(test)]` thread-local override (`set_clipboard_image_test_override`) keeps
headless tests off the real OS pasteboard; the production entry reads the OS, the
test entry returns the injected value.

## Verification

- **Pure selector** `select_clipboard_png_prefers_png_and_rejects_empty` —
  PNG-over-TIFF preference + empty rejection. NC: reduce the selector to
  `tiff_as_png` only → RED.
- **Headless real path** `image_paste_direct_read_stages_even_with_text_on_clipboard`
  (verify_harness) — drives the real `handle_claude_key`→`paste_into_compose`
  with GPUI's clipboard holding ONLY text (the mac short-circuit scenario) and the
  direct read injected; asserts one `image/png` staged, bytes round-trip, URL text
  not typed. NC: disable the direct-read step in `paste_into_compose` → RED
  (observed).
- **`#[ignore]` real-pasteboard round-trip**
  `read_clipboard_image_png_os_recovers_png_beside_text` — writes a real PNG + a
  URL string to the live `NSPasteboard`, asserts `read_clipboard_image_png_os`
  recovers a valid PNG. This is the documented gap-2 (live OS integration) remedy;
  run with `-- --ignored`. NC: emulate GPUI's string short-circuit in the reader →
  RED (observed).
- Full suite: 568 pass / 0 fail; the 2 ignored are this test + one pre-existing.

## Notes

- The pre-existing headless `image_paste_stages_pending_attachment` (GPUI-entry
  path) still passes — that fallback is intact for non-mac.
- Images remain ephemeral (not in the WAL); a resumed transcript shows the `🖼`
  marker, not the image. Unchanged by this fix.
- Whether the live `claude-agent-acp` advertises the ACP `image` prompt capability
  is still the separate NEEDS-RUNTIME question from the original INV-UX-21 work.

## 2026-08-16 (2) — second root cause: Cmd+V never reached the fix

The direct-pasteboard read landed, but the user reported "when I paste nothing
happens." The first fix was in **dead code**.

`cmd-v` is bound globally to the `PasteFromClipboard` **action**
(`keymap_registry.rs:144`), and GPUI 0.2.2 dispatches bound actions BEFORE
`on_key_down` key listeners (the same dispatch-order fact as bug-0038). So a real
Cmd+V runs `paste_from_clipboard` (`main.rs`) — NOT the `handle_claude_key` branch
that called `paste_into_compose`. `paste_from_clipboard` did
`read_from_clipboard().and_then(|i| i.text())` and returned early when there was no
text, so a pure-image clipboard produced **nothing**.

The first headless guard drove `handle_claude_key` directly, so it passed while
the real path stayed broken — exactly anti-circling rules 1 (drive the REAL entry
point) and 4 (`simulate_keystrokes` action dispatch vs a hand-called handler).

**Fix (2).** Extracted `stage_clipboard_images_onto_compose` (agent_ui.rs) and
called it from `paste_from_clipboard` BEFORE the text early-return: an agent tile
with a clipboard image stages it and returns; otherwise the text paste runs
unchanged. `paste_into_compose` now delegates to the same helper.

**Guards (2).** Both `image_paste_*` tests now `register_keymap` +
`simulate_keystrokes("cmd-v")` — the REAL action dispatch — instead of calling
`handle_claude_key`. Negative control: remove the image block from
`paste_from_clipboard` → BOTH tests RED (observed). 568 pass.

## 2026-08-16 (3) — no pre-send indication in worksheet-idle mode

User: "when pasting an image in worksheet or chatbox mode I want indication it's
in the buffer; indication only appears after sent." Probed both modes:

- **Chatbox / mid-turn** — the `🖼` chip DID paint before send (probe
  `compose-image-chips` returned real area). Works.
- **Worksheet-idle** — `show_compose` (`screens.rs`) is
  `is_chatbox() || turn_phase.is_awaiting()`, so an idle worksheet renders NO
  compose panel — and the chip row lived only INSIDE that panel. So a paste had
  zero on-screen indication until send (which then shows the transcript `🖼`
  marker). Probe confirmed: `compose-box` = None, `compose-image-chips` = None
  after paste, while the image was staged in state.

**Fix (3).** Extracted `render_agent::pending_image_chip_strip` and render it in
BOTH places: inside the compose panel (chatbox/mid-turn, unchanged) and as a
standalone strip pinned to the bottom of the agent tile's main column when
`show_compose == false` and images are staged (worksheet-idle). INV-UX-21
property 2 updated.

**Guards (3).** `image_paste_chip_paints_before_send_chatbox` and
`…_worksheet_idle` — paint probes asserting the chip has real area. The worksheet
one is non-vacuous (asserts `compose-box` is None, so the strip is the only
indication). Negative control: delete the standalone-strip arm in `render_agent`
→ worksheet test RED, chatbox test green (observed). 570 pass.
