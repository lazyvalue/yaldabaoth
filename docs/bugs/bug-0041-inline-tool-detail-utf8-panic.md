# bug-0041: inline-tool-detail-utf8-panic

**Status:** FIXED
**First seen:** 2026-08-17
**Component:** Agent Tile transcript tool-group headers

## Symptom

Rendering a Bash tool call whose command included an emoji crashed the main GPUI
thread with `byte index 60 is not a char boundary`. The reported command placed
`🖼` across bytes 57–61, exactly where the inline command detail was truncated.
The preceding macOS IMK run-loop message was incidental; the Rust panic was the
fatal event.

## Root cause

`tool_inline_detail` used `str::len()` to decide whether a command or search
pattern was long, then sliced it with `&value[..60]` or `&value[..40]`.
`str::len()` and range indices are UTF-8 bytes, and Rust requires both slice
endpoints to be character boundaries. Arbitrary ACP tool input can place a
multi-byte character across either display limit, so both branches could panic.

## Fix

Both branches now use `truncate_inline_detail`, which finds the byte offset of
the Nth Unicode scalar value with `char_indices()` and slices only at that known
boundary. ASCII behavior is unchanged: command details retain their 60-character
limit, patterns retain 40, and truncated values receive one ellipsis.

## Verification

- `tool_inline_detail_truncates_unicode_without_splitting_a_character` calls the
  production helper with the exact reported command and a second search-pattern
  boundary case.
- Negative control: restoring byte slicing made that guard panic with the exact
  reported `byte index 60` / `🖼` bytes 57–61 failure; restoring the fix made it
  pass.
- Targeted `cargo mutants` caught both generated replacement mutants for
  `truncate_inline_detail`. The initial sandboxed run was unviable; the permitted
  scratch-directory rerun was viable.

## Log

### 2026-08-17 — localized and fixed

The console's source line directly identified the unsafe slice. Inspection found
the same construction in the adjacent search-pattern branch, so the correction
was shared rather than command-only. Cog graph `8zc` tracked localization,
implementation, negative control, mutation testing, records, and finalization.
