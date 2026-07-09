# 001 — Remaining editing bug backlog

Bugs surfaced by the Fable comb (2026-07-08) not yet fixed. Each needs a headless
guard + negative control per the anti-circling rules. Ordered by confidence×impact.
File:line references are from the 2026-07-08 tree — reconfirm before fixing.

- [ ] **Undo silently drops text typed in the compose.** `insert_char` opens no
      undo group; it depends on `begin_insert` (`editor.rs:1051`) having run.
      Compose Insert entries that set `mode = Insert` directly (fresh session /
      `/clear` / `open_you_block_at_cursor` / `settle_input_focus`) never call
      `begin_insert`, so `record_splice` drops every splice (`document.rs:143`).
      RISK: `begin_undo_group` *overwrites* `pending_undo`, so a naive "always
      begin_insert" can double-open and lose a group — fix carefully. Verify the
      real repro first (does the message box actually lose undo?).
- [ ] **First-Esc in a You-block skips `end_insert()`** (`agent_ui.rs` ws_esc_mode
      branch) — leaks the undo record + leaves the Normal caret one past EOL.
- [ ] **Tab-expanded lines misalign caret + selection.** Rendered text expands
      `\t`→4 spaces (`main.rs` highlight_snapshot, `transcript_view.rs`,
      `screens.rs`) but cursor/selection columns stay raw char columns, so the
      painted caret sits left of where edits land. Map doc col → display col
      before handing `cursor_col`/selection to the render helpers. Cross-surface.
- [ ] **Count prefixes eaten** (`edit_ui.rs:617`): `10j` moves one line. Loop the
      motion arms `count.unwrap_or(1)` times. (Shared dispatch → both surfaces.)
- [ ] **`e` (word-end) strands on the newline; `x`/`d` there joins lines**
      (`cursor.rs:177` move_word_end includes trailing `\n`).
- [ ] **Undo/redo + `cursor_set` restore a stale `desired_col`** (`editor.rs:1246`,
      `cursor.rs:60`) — caret lands at the old sticky column. Clear `desired_col`
      on direct cursor placement.
- [ ] **Cmd-V pastes only in Insert; Normal-mode Cmd-V is a silent no-op**
      (`main.rs:3329`). Route Normal-mode Cmd-V through the `p` path.
- [ ] **Wide glyphs (emoji/CJK) break the 8px-per-char compose wrap grid** — text
      + caret clip off the right edge (`agent.rs` wrap_line_cols counts chars).
      Largely subsumed by ticket 003 (renderer merge).

## Verification

Each fix: drive the REAL entry point (`handle_edit_key` / `handle_claude_key`),
add a `verify_harness.rs` guard, observe it RED with the fix reverted. Tab-align
and wide-glyph bugs are paint-truth — assert via `caret_token_split`-style pure
helpers or the layout probe, not just buffer state.
</content>
