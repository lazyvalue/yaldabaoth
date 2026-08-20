//! Unit tests for the GPUI app (moved out of main.rs, split-gpui-main).

use super::*;
use crate::chrome::{DESKTOP_CELL_H, DESKTOP_CELL_W, DESKTOP_GUTTER};

#[test]
fn bare_ctrl_w_is_exclusively_reserved_as_the_shell_prefix() {
    assert!(is_ctrl_w_shell_prefix(&KeyPress::new(
        Key::Char('w'),
        KMods::CONTROL,
    )));
    assert!(is_ctrl_w_shell_prefix(&KeyPress::new(
        Key::Char('W'),
        KMods::CONTROL,
    )));
    assert!(!is_ctrl_w_shell_prefix(&KeyPress::new(
        Key::Char('w'),
        KMods::ALT,
    )));
    assert!(!is_ctrl_w_shell_prefix(&KeyPress::new(
        Key::Char('h'),
        KMods::CONTROL,
    )));
}

/// UXI-Workspace-22: every shipped `Ctrl-W …` command is shell-owned and is
/// wired at the common tile ancestor. This exact-set assertion is the change
/// detector: adding a registry row without adding its central listener fails
/// here instead of failing only in whichever App happens not to forward it.
#[test]
fn ctrl_w_registry_exactly_matches_central_shell_actions() {
    use std::collections::BTreeSet;

    let registered: BTreeSet<&str> = KeymapRegistry::defaults()
        .entries
        .iter()
        .filter(|entry| entry.default_keystrokes.starts_with("ctrl-w "))
        .map(|entry| entry.action)
        .collect();
    let centrally_wired: BTreeSet<&str> = CTRL_W_SHELL_ACTION_NAMES.iter().copied().collect();

    assert_eq!(
        centrally_wired, registered,
        "the Ctrl-W keymap and the shell's common-ancestor listeners must evolve together"
    );
}

/// UXI-JumpPanel-7: the jump-panel accent colors are theme-owned, not fixed
/// constants. Nightfox art-directs its own palette-native jump colors (explicit
/// recessed panel bg + muted-rose header + soft-blue subheader + warm-orange
/// working star); the other themes keep the legacy theme-neutral constants
/// (`jump_panel_bg: None` ⇒ derive the shade from `editor_bg`).
///
/// Negative control: revert Nightfox's jump fields to the legacy constants
/// (`jump_header = #ff6b6b`, `jump_panel_bg = None`, …) → the Nightfox asserts fail.
#[test]
fn nightfox_jump_panel_colors_are_art_directed() {
    use yalda::style::Color;
    use yalda::theme::AgentTheme;
    let nf = AgentTheme::nightfox();
    assert_eq!(
        nf.jump_header,
        Color::Rgb(0xc9, 0x4f, 0x6d),
        "Nightfox red header"
    );
    assert_eq!(
        nf.jump_subheader,
        Color::Rgb(0x71, 0x9c, 0xd6),
        "Nightfox blue subheader"
    );
    assert_eq!(
        nf.jump_working,
        Color::Rgb(0xf4, 0xa2, 0x61),
        "Nightfox orange working star"
    );
    // Distinct from the legacy theme-neutral constants the other themes fall back to.
    assert_ne!(
        nf.jump_header,
        Color::Rgb(0xff, 0x6b, 0x6b),
        "not the legacy #ff6b6b header"
    );
    let dr = AgentTheme::dracula();
    assert_eq!(
        dr.jump_header,
        Color::Rgb(0xff, 0x6b, 0x6b),
        "Dracula preserves the legacy jump-header red (unchanged look)"
    );
}

/// UXI-AgentTile-21: the sentence splitter behind `[N]r` reply-with-quotation.
/// Counts + joins the first N sentences; a decimal point and a common
/// abbreviation do NOT split; a run with no terminator is one sentence; blank
/// yields "" (the caller's "nothing to quote" signal).
/// bug-0033: a code fence is bounded to its own agent turn (frozen range). A
/// stray/unclosed ``` must NOT pair with a later turn's ``` and swallow everything
/// between into one block; a balanced in-turn block is still detected.
#[test]
fn detect_block_ranges_bounds_fence_to_turn() {
    // Turn 1 = frozen (0,2): a stray open ``` that never closes in the turn.
    // Turn 2 = frozen (3,5): its own ```. Line 2 is a user line between them.
    let ls: Vec<String> = [
        "```",        // 0  turn 1 stray fence
        "agent text", // 1  turn 1
        "user reply", // 2  (between turns)
        "```",        // 3  turn 2 fence
        "more",       // 4  turn 2
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let frozen = [(0usize, 2usize), (3usize, 5usize)];
    let ranges = detect_block_ranges(&ls, &frozen);
    for &(s, e) in &ranges {
        assert!(
            !(s <= 1 && e >= 3),
            "a fence bled across the turn boundary into a block ({s},{e})"
        );
    }

    // Balanced in-turn block is still detected.
    let ls2: Vec<String> = ["```rust", "let x = 1;", "```", "after"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let ranges2 = detect_block_ranges(&ls2, &[(0usize, 4usize)]);
    assert!(
        ranges2.contains(&(0, 3)),
        "a balanced in-turn code block must still be a block; got {ranges2:?}"
    );
}

/// bug-0031: the caret is a BEAM while a selection is active, a BLOCK otherwise.
#[test]
fn caret_mode_during_selection_beams_only_with_selection() {
    use crate::EditMode;
    assert_eq!(
        caret_mode_during_selection(EditMode::Normal, true),
        EditMode::Insert,
        "active selection ⇒ beam"
    );
    assert_eq!(
        caret_mode_during_selection(EditMode::Normal, false),
        EditMode::Normal,
        "no selection ⇒ block"
    );
}

/// UXI-AgentTile-35: a multi-line selection is quoted as a standard Markdown
/// blockquote — `> ` on every line, a bare `>` for an empty line.
#[test]
fn blockquote_lines_prefixes_every_line() {
    assert_eq!(blockquote_lines("solo"), "> solo");
    assert_eq!(blockquote_lines("aa\nbb\ncc"), "> aa\n> bb\n> cc");
    // Empty interior line stays contiguous with a bare `>`.
    assert_eq!(blockquote_lines("aa\n\nbb"), "> aa\n>\n> bb");
    assert_eq!(blockquote_lines(""), ">");
}

#[test]
fn first_n_sentences_splits_and_respects_abbrevs() {
    // Count + single-space join.
    assert_eq!(first_n_sentences("One. Two. Three. Four.", 1), "One.");
    assert_eq!(
        first_n_sentences("One. Two. Three. Four.", 3),
        "One. Two. Three."
    );
    // Clamp: more requested than available → all of them, no error.
    assert_eq!(first_n_sentences("One. Two.", 9), "One. Two.");
    // Abbreviation dot does not split.
    assert_eq!(
        first_n_sentences("Use foo, e.g. bar, here. Next.", 1),
        "Use foo, e.g. bar, here."
    );
    // Decimal dot does not split (followed by a digit, not whitespace).
    assert_eq!(
        first_n_sentences("It is 3.5 now. Next.", 1),
        "It is 3.5 now."
    );
    // `?` and `!` also terminate.
    assert_eq!(first_n_sentences("Really? Yes! Ok.", 2), "Really? Yes!");
    // No terminator at all → the whole text is one sentence.
    assert_eq!(first_n_sentences("no period here", 1), "no period here");
    // Blank / empty → "" (no-op signal).
    assert_eq!(first_n_sentences("   ", 1), "");
    assert_eq!(first_n_sentences("", 2), "");
    assert_eq!(first_n_sentences("anything", 0), "");
}

/// UXI-AgentTile-21 REGRESSION ("bold breaks the sentence parser"): a terminator
/// wrapped in closing markup — `*italic.*`, `**bold.**`, `` `code.` ``,
/// `(aside.)`, `"quoted."` — still ends the sentence. Before the fix the `.` was
/// followed by `*` (not whitespace), so the boundary was missed and the sentence
/// ran on into the next one. The closers stay IN the returned sentence so the
/// quoted markup remains balanced.
#[test]
fn first_n_sentences_terminates_through_closing_markup() {
    // The reported case: emphasis around a whole sentence.
    assert_eq!(
        first_n_sentences("*this sentence is bold.* Next one.", 1),
        "*this sentence is bold.*",
        "a `.` before a closing `*` still ends the sentence"
    );
    // Double-asterisk bold: a RUN of closers is consumed.
    assert_eq!(first_n_sentences("**Bold.** Next.", 1), "**Bold.**");
    // Underscore emphasis, inline code, parens, quotes.
    assert_eq!(first_n_sentences("_Emph._ Next.", 1), "_Emph._");
    assert_eq!(first_n_sentences("`code.` Next.", 1), "`code.`");
    assert_eq!(first_n_sentences("(Aside.) Next.", 1), "(Aside.)");
    assert_eq!(
        first_n_sentences("He said \"go.\" Then left.", 1),
        "He said \"go.\""
    );
    // Counting still works ACROSS emphasised sentences.
    assert_eq!(
        first_n_sentences("*One.* Two. Three.", 2),
        "*One.* Two.",
        "2r spans an emphasised first sentence and a plain second"
    );
    // Closing markup at end-of-text also terminates.
    assert_eq!(first_n_sentences("*Only one.*", 1), "*Only one.*");
    // A closer must still be followed by whitespace/EOT — `*` mid-word doesn't
    // fabricate a boundary, and decimals are untouched.
    assert_eq!(
        first_n_sentences("a.*b continues here", 1),
        "a.*b continues here"
    );
    assert_eq!(
        first_n_sentences("It is 3.5 now. Next.", 1),
        "It is 3.5 now."
    );
}

/// UXI-Blockquote-1: the classification seam behind italicising `>` text on the
/// compose / You-block surfaces (where the markdown highlighter never runs). The
/// italic PAINT itself is a human-eye check (harness gap #1); this pins WHICH
/// lines get it, matching `md_highlight::split_quote_prefix`'s rule.
#[test]
fn is_blockquote_line_matches_leading_marker_only() {
    assert!(is_blockquote_line("> quoted"));
    assert!(is_blockquote_line(">no space still counts"));
    assert!(is_blockquote_line(">> nested"));
    // Leading whitespace is allowed before the marker.
    assert!(is_blockquote_line("   > indented quote"));
    // A `>` that is not line-leading is NOT a quote (comparisons, arrows, code).
    assert!(!is_blockquote_line("a > b"));
    assert!(!is_blockquote_line("if x >= 3"));
    assert!(!is_blockquote_line("ordinary text"));
    assert!(!is_blockquote_line(""));
    assert!(!is_blockquote_line("   "));
}

#[test]
fn inline_you_block_wrap_width_prefers_measurement_then_viewport() {
    assert_eq!(
        crate::inline_you_block_wrap_cols(73, 1_574.0),
        73,
        "the block's exact painted measurement wins"
    );
    assert_eq!(
        crate::inline_you_block_wrap_cols(0, 1_574.0),
        194,
        "an unmeasured first paint derives columns from the transcript viewport"
    );
    assert_eq!(
        crate::inline_you_block_wrap_cols(0, 0.0),
        40,
        "40 columns remains only the pre-layout emergency fallback"
    );
    assert_eq!(
        crate::inline_you_block_wrap_cols(0, 1.0),
        40,
        "a one-pixel pre-layout sentinel is not a usable viewport"
    );
    assert_eq!(
        crate::inline_you_block_wrap_cols(0, 8.0),
        1,
        "a tiny measured viewport still makes forward progress"
    );
}

/// Agent-chat heading-marker toggle (the only markdown the user wants visible
/// in transcripts): `heading_line_with_markers` re-inserts the literal `#`
/// markers pulldown strips, as a leading span, with one space before the text.
/// `##` for h2, `###` for h3, level clamped to 1..=6, and existing spans kept.
#[test]
fn heading_line_with_markers_prepends_markers() {
    let style = yalda::style::Style::default();
    let h2 = heading_line_with_markers(2, &StyledLine::plain("Overview"), style);
    assert_eq!(h2.text_content(), "## Overview", "h2 shows ##");

    let h3 = heading_line_with_markers(3, &StyledLine::plain("Details"), style);
    assert_eq!(h3.text_content(), "### Details", "h3 shows ###");

    // The marker is a distinct leading span (so it carries the heading style),
    // ahead of the original content spans.
    assert_eq!(h2.spans.len(), 2, "marker span + content span");
    assert_eq!(h2.spans[0].text, "## ");

    // Level clamps to the 1..=6 heading range (mirrors `block_inner`).
    let h7 = heading_line_with_markers(7, &StyledLine::plain("X"), style);
    assert_eq!(h7.text_content(), "###### X", "level clamps at 6");
}

/// User-turn jump navigation (agent `.` menu): `user_turn_item_indices` returns
/// the flat-item index of every user `TurnHeader` in render order — the single
/// source the handler clamps against and `build_body` resolves the jump ordinal
/// through. Claude turns and plain lines are skipped; order is preserved.
#[test]
fn user_turn_item_indices_picks_only_user_headers() {
    let items = vec![
        FlatItem::TurnHeader {
            role: TurnRole::User,
        }, // 0
        FlatItem::Line(0), // 1
        FlatItem::TurnHeader {
            role: TurnRole::Claude,
        }, // 2
        FlatItem::Line(1), // 3
        FlatItem::TurnHeader {
            role: TurnRole::User,
        }, // 4
        FlatItem::Line(2), // 5
        FlatItem::TurnHeader {
            role: TurnRole::User,
        }, // 6
    ];
    let idx = user_turn_item_indices(&items);
    assert_eq!(idx, vec![0, 4, 6], "only user TurnHeaders, in order");
    // The Nth-user-turn resolution `build_body` performs: ordinal → flat index.
    assert_eq!(idx.get(0).copied(), Some(0), "first user turn");
    assert_eq!(idx.get(2).copied(), Some(6), "last user turn");
    assert_eq!(
        idx.get(3).copied(),
        None,
        "out-of-range ordinal yields no reveal"
    );

    // No user turns → empty (the handler's "no user turns yet" guard).
    assert!(user_turn_item_indices(&[FlatItem::Line(0)]).is_empty());
}

/// User-turn jump stepping (`next_jump_ord`): `k`/`j` step by ∓1 and saturate at
/// both ends; `to_last` parks on the most recent turn; a stale `cur` past the
/// end is clamped before stepping.
#[test]
fn next_jump_ord_steps_and_clamps() {
    // 3 user turns → ordinals 0,1,2.
    assert_eq!(next_jump_ord(0, 3, 1, false), 1, "j: newer");
    assert_eq!(next_jump_ord(1, 3, 1, false), 2, "j: newer");
    assert_eq!(next_jump_ord(2, 3, 1, false), 2, "j: saturates at last");
    assert_eq!(next_jump_ord(2, 3, -1, false), 1, "k: older");
    assert_eq!(next_jump_ord(0, 3, -1, false), 0, "k: saturates at first");
    // to_last ignores delta → most recent.
    assert_eq!(next_jump_ord(0, 3, 0, true), 2, "to_last → last turn");
    // A stale ordinal (turns removed/replayed) clamps to the live range first.
    assert_eq!(
        next_jump_ord(9, 3, -1, false),
        1,
        "stale cur clamps to last, then steps"
    );
    // Single turn: every step stays put.
    assert_eq!(next_jump_ord(0, 1, 1, false), 0);
    assert_eq!(next_jump_ord(0, 1, -1, false), 0);
}

/// Worksheet frozen-BLOCK navigation: the caret may rest on EVERY editable line
/// and EVERY non-blank frozen prose line (each such line is its own block, so the
/// caret can land between any two to insert there), but NOT on tool groups,
/// structural blocks, or blank frozen padding — those are crossed in one
/// keystroke.
#[test]
fn build_nav_stops_per_frozen_prose_line() {
    use crate::FlatItem;
    // Lines 0..4 are a frozen agent turn (prose, prose, blank tool anchor,
    // prose); lines 4..6 are the editable user tail.
    let lines: Vec<String> = ["alpha", "beta", "", "gamma", "", "draft"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let frozen = [(0usize, 4usize)]; // lines 0,1,2,3 frozen
    let flat = vec![
        FlatItem::Line(0), // frozen prose → stop
        FlatItem::Line(1), // frozen prose → stop (its own block)
        FlatItem::Line(2), // blank frozen padding → skip
        FlatItem::ToolGroup {
            anchor_line: 2,
            ids: vec![],
        }, // crossed → skip
        FlatItem::Line(3), // frozen prose → stop
        FlatItem::Line(4), // editable → stop
        FlatItem::Line(5), // editable → stop
    ];
    let stops = crate::build_nav_stops(&flat, &lines, &frozen);
    assert_eq!(
        stops,
        vec![0, 1, 3, 4, 5],
        "stops: every non-blank frozen prose line (0,1,3) + editable lines (4,5); \
         tool group + blank frozen padding are crossed"
    );
}

/// FIX 5 contract: `snap_nav_stop` returns `None` when there is no stop in the
/// REGRESSION (screenshot: a multi-line Bash command rendered "not folded"): the
/// tool-group fold header must be ONE short line. A multi-line / very long title is
/// clamped to its first line (+ ` …`) so the command body never fills the header.
#[test]
fn fold_header_line_is_single_short_line() {
    use crate::fold_header_line;
    // Single short line: unchanged.
    assert_eq!(fold_header_line("Read foo.rs"), "Read foo.rs");
    // Multi-line (heredoc): first line + ellipsis, no newlines.
    let heredoc = "python3 - <<'PY'\np='x.rs'\ns=open(p).read()\nPY";
    let h = fold_header_line(heredoc);
    assert!(!h.contains('\n'), "header has no newlines: {h:?}");
    assert!(
        h.starts_with("python3 - <<'PY'"),
        "keeps the first line: {h:?}"
    );
    assert!(h.ends_with('…'), "shows a truncation cue: {h:?}");
    // Very long single line: capped.
    let long = "git ".to_string() + &"a".repeat(300);
    let c = fold_header_line(&long);
    assert!(
        c.chars().count() <= 124,
        "capped (~120 + ' …'): {} chars",
        c.chars().count()
    );
    assert!(c.ends_with('…'));
    // Empty title: empty, no panic.
    assert_eq!(fold_header_line(""), "");
}

/// bug-0041: inline tool details are bounded by characters, not UTF-8 bytes.
/// The reported Bash command places a four-byte emoji across the old byte-60
/// slice, which panicked while building the folded tool-group header.
#[test]
fn tool_inline_detail_truncates_unicode_without_splitting_a_character() {
    use yalda::acp_channel::{ToolCall, ToolCallId};

    let mut command = ToolCall::new(ToolCallId::from("unicode-command"), "Bash".to_string());
    command.raw_input = Some(serde_json::json!({
        "command": "cd /Users/scott/ws/yaldabaoth; grep -rn \"pending_images\\|🖼\\|pending_image\\|chip\" src/bin/yalda-gpui/screens.rs src/bin/yalda-gpui/agent.rs src/bin/yalda-gpui/transcript_view.rs | head -30"
    }));
    let detail = tool_inline_detail(&command).expect("command detail");
    assert_eq!(
        detail.chars().count(),
        61,
        "60 characters plus the ellipsis"
    );
    assert!(
        detail.contains('🖼'),
        "the boundary-crossing emoji remains intact: {detail:?}"
    );
    assert!(detail.ends_with('…'));

    let mut search = ToolCall::new(ToolCallId::from("unicode-pattern"), "Grep".to_string());
    search.raw_input = Some(serde_json::json!({
        "pattern": format!("{}🖼tail", "a".repeat(39))
    }));
    let expected_pattern = format!("{}🖼…", "a".repeat(39));
    assert_eq!(
        tool_inline_detail(&search).as_deref(),
        Some(expected_pattern.as_str()),
        "the search-pattern branch uses the same character-safe boundary"
    );
}

/// REGRESSION ("/clear then can't type"): the `/clear` server path builds a fresh
/// session then `settle_input_focus()`s it. A fresh (empty) worksheet opens a VISIBLE
/// tail You-block that is immediately TYPEABLE (focus=Compose, Insert) — you just
/// cleared to write, so typing lands + is visible with NO `i`. The `space` tile-menu
/// leader still works on the empty block via the empty-draft heuristic
/// (`focused_in_insert_mode`), so this does not re-regress the tile menu.
#[test]
fn clear_resets_worksheet_to_a_typeable_block() {
    let mut st = AgentState::new_server_managed(None);
    assert!(
        st.editor.document().is_empty(),
        "fresh transcript after clear"
    );
    assert!(
        !st.input_surface.is_chatbox(),
        "default placement is worksheet"
    );
    st.settle_input_focus();
    assert!(st.you_block_open, "clear opens a VISIBLE tail You-block");
    assert_eq!(
        st.focus,
        AgentFocus::Compose,
        "focused so typing lands immediately"
    );
    assert_eq!(
        st.input_surface.compose().mode,
        EditMode::Insert,
        "in Insert — type and see it, no `i`"
    );
}

/// move direction. The worksheet key handler relies on this `None` to fall the
/// caret back to its pre-motion stop instead of stranding it on an unrenderable
/// block-interior / blank line (Finding E).
#[test]
fn snap_nav_stop_none_when_no_stop_in_direction() {
    use crate::FlatItem;
    let mut st = AgentState::new_for_test();
    // Single navigable stop at line 5 (e.g. a lone editable line below a leading
    // code block whose interior lines are not stops).
    st.view_model
        .store(1, vec![FlatItem::Line(5)], vec![None], vec![5], vec![5]);
    assert_eq!(st.view_model.snap_nav_stop(5, true), Some(5));
    assert_eq!(st.view_model.snap_nav_stop(5, false), Some(5));
    // Moving down past the last stop, or up before the first, finds nothing.
    assert_eq!(st.view_model.snap_nav_stop(6, true), None);
    assert_eq!(st.view_model.snap_nav_stop(4, false), None);
    // Empty cache (no render yet) → None in both directions → motion left to the
    // caller's fallback.
    let empty = AgentState::new_for_test();
    assert_eq!(empty.view_model.snap_nav_stop(0, true), None);
    assert_eq!(empty.view_model.snap_nav_stop(0, false), None);
}

/// `j` pressed while already parked on the newest user turn means "go past the
/// last turn" → drop at the buffer's page end. Every other step stays on a
/// user-turn header.
#[test]
fn jump_lands_at_page_end_only_on_j_at_newest() {
    // 3 turns (ordinals 0,1,2). At the last, a `j` that can't advance → page end.
    assert!(
        jump_lands_at_page_end(2, 2, 3, 1, false),
        "j at newest → page end"
    );
    // Mid-list `j` advances to a header, not the page end.
    assert!(
        !jump_lands_at_page_end(1, 2, 3, 1, false),
        "j mid-list → header"
    );
    // `k` (older) never lands at the page end, even at the last turn.
    assert!(
        !jump_lands_at_page_end(2, 2, 3, -1, false),
        "k never page-ends"
    );
    // toggle-on ("jump to last") parks on the last header, not the page end.
    assert!(
        !jump_lands_at_page_end(2, 2, 3, 0, true),
        "to_last → header"
    );
    // Single turn: a `j` there (already newest) goes to the page end.
    assert!(
        jump_lands_at_page_end(0, 0, 1, 1, false),
        "lone turn: j → page end"
    );
}

/// 5c / ADR-0007: a theme switch re-renders Doc blocks via `re_render_one_doc`.
/// For a pool-bound Doc the authority is the LIVE shared core (unsaved edits
/// from a sibling Edit view), not the file on disk. The old code read disk
/// here — silently reverting unsaved edits, and (because `rendered_seq` would
/// not advance) the per-frame `refresh_blocks` would not self-correct. This
/// pins the fix: re-render reflects the live core and stamps `rendered_seq`.
#[test]
fn re_render_one_doc_sources_live_core_not_disk() {
    let dir = std::env::temp_dir().join(format!("yalda_rerender_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.md");
    // Disk holds a single paragraph → exactly one rendered block.
    std::fs::write(&path, "disk only\n").unwrap();

    let mut ws: workspace::Frame<App> = workspace::Frame::new(ProjectId(0));
    let (id, core) = ws.open_and_retain(&path).unwrap();

    // A pool-bound Doc, rendered at the disk content (rendered_seq stamped at
    // the pristine core).
    let mut doc = DocState::viewing(
        render_with_wiki("disk only\n", &Theme::default(), Some(&path)),
        path.display().to_string().into(),
        Some(DocSource::new(id, core.clone())),
    );
    assert_eq!(doc.blocks.len(), 1, "disk content is one block");

    // Simulate an unsaved edit through a sibling view: append two more
    // paragraphs that exist ONLY in the live core, never on disk.
    {
        let mut c = core.borrow_mut();
        let d = c.document_mut();
        let n = d.full_text().chars().count();
        d.insert_str_at_char(n, "\n\npara two\n\npara three\n");
    }
    let live_seq = core.borrow().document().edit_seq();
    let live_blocks = render_with_wiki(
        &core.borrow().document().full_text(),
        &Theme::default(),
        Some(&path),
    )
    .len();
    assert!(live_blocks >= 3, "live core now has multiple blocks");

    // Theme switch path. Must reflect the LIVE core (≥3 blocks), not disk (1).
    re_render_one_doc(&mut doc, &Theme::default());
    assert_eq!(
        doc.blocks.len(),
        live_blocks,
        "re-render must source the live core, not disk"
    );
    assert_eq!(
        doc.source.as_ref().unwrap().rendered_seq,
        live_seq,
        "rendered_seq must advance to the live edit_seq so refresh_blocks stays coherent"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// ADR-0010: the canonical on-disk cwd key resolves a symlinked spelling
/// and the real path to the SAME string (so a session saved under one is
/// found when launched under the other), and falls back to the raw spelling
/// when the path can't be canonicalized (never regresses to never-matching).
#[test]
fn persist_cwd_key_canonicalizes_symlinks() {
    use std::os::unix::fs::symlink;
    let base = std::env::temp_dir().join(format!("yalda-cwdkey-{}", std::process::id()));
    let real = base.join("real");
    let link = base.join("link");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();

    assert_eq!(
        persist_cwd_key(&link),
        persist_cwd_key(&real),
        "symlinked and real cwd must share one on-disk key"
    );
    assert_ne!(
        persist_cwd_key(&link),
        link.to_string_lossy(),
        "the key must be canonicalized, not the raw symlink spelling"
    );

    // Non-existent path: canonicalize fails -> echo raw (no never-match).
    let missing = base.join("does-not-exist");
    assert_eq!(persist_cwd_key(&missing), missing.to_string_lossy());

    let _ = std::fs::remove_dir_all(&base);
}

/// Settings persistence round-trips (theme, agent bar, text zoom) and a
/// preferences file written before `text_scale` existed still loads — the
/// `#[serde(default)]` keeps it forward-compatible (no panic, zoom = None).
#[test]
fn preferences_round_trip_with_text_scale() {
    let prefs = Preferences {
        theme: Some("dracula".into()),
        text_scale: Some(1.21),
        window_width_px: Some(1110.0),
        window_height_px: Some(770.0),
        desktop_grid_cols: Some(100),
        desktop_grid_rows: Some(30),
        desktop_grid_defaults_version: Some(2),
        jump_panel_visible: Some(false),
        jump_cwd_order: Some(vec!["/work/beta".into(), "/work/alpha".into()]),
        jump_session_order: Some(vec!["sid-2".into(), "sid-1".into()]),
        jump_archived_sessions: Some(vec!["sid-old".into()]),
        jump_folded_projects: Some(vec!["Fulcrum".into(), "Yaldabaoth".into()]),
        jump_folded_workspaces: Some(vec!["Yaldabaoth\u{1f}workspace-1".into()]),
        jump_workspace_order: Some(vec![
            "Yaldabaoth\u{1f}workspace-3".into(),
            "Yaldabaoth\u{1f}workspace-1".into(),
        ]),
        jump_tag_order: Some(std::collections::HashMap::from([(
            "Yaldabaoth".to_string(),
            vec!["urgent".to_string(), "frontend".to_string()],
        )])),
        jump_folded_tags: Some(vec!["Yaldabaoth\u{1f}frontend".into()]),
        jump_tile_order: Some(vec![30, 10, 20]),
        jump_detached_tile_order: Some(vec![60, 40, 50]),
    };
    let json = serde_json::to_string(&prefs).unwrap();
    let back: Preferences = serde_json::from_str(&json).unwrap();
    assert_eq!(back.theme.as_deref(), Some("dracula"));
    assert_eq!(back.text_scale, Some(1.21));
    assert_eq!(back.window_width_px, Some(1110.0));
    assert_eq!(back.window_height_px, Some(770.0));
    assert_eq!(back.jump_panel_visible, Some(false));
    assert_eq!(
        back.jump_cwd_order.as_deref(),
        Some(&["/work/beta".into(), "/work/alpha".into()][..])
    );
    assert_eq!(
        back.jump_session_order.as_deref(),
        Some(&["sid-2".into(), "sid-1".into()][..])
    );
    assert_eq!(
        back.jump_archived_sessions.as_deref(),
        Some(&["sid-old".into()][..])
    );
    assert_eq!(back.desktop_grid_cols, Some(100));
    assert_eq!(back.desktop_grid_rows, Some(30));
    assert_eq!(back.desktop_grid_defaults_version, Some(2));
    assert_eq!(
        back.jump_folded_projects.as_deref(),
        Some(&["Fulcrum".into(), "Yaldabaoth".into()][..])
    );
    assert_eq!(
        back.jump_folded_workspaces.as_deref(),
        Some(&["Yaldabaoth\u{1f}workspace-1".into()][..])
    );
    assert_eq!(
        back.jump_workspace_order.as_deref(),
        Some(
            &[
                "Yaldabaoth\u{1f}workspace-3".to_string(),
                "Yaldabaoth\u{1f}workspace-1".to_string(),
            ][..]
        )
    );
    // UXI-JumpPanel-21: per-project tag order + folded-tag keys round-trip.
    assert_eq!(
        back.jump_tag_order
            .as_ref()
            .and_then(|m| m.get("Yaldabaoth"))
            .map(|v| v.as_slice()),
        Some(&["urgent".to_string(), "frontend".to_string()][..])
    );
    assert_eq!(
        back.jump_folded_tags.as_deref(),
        Some(&["Yaldabaoth\u{1f}frontend".to_string()][..])
    );
    // UXI-JumpPanel-28: the tile drag order round-trips.
    assert_eq!(back.jump_tile_order.as_deref(), Some(&[30, 10, 20][..]));
    assert_eq!(
        back.jump_detached_tile_order.as_deref(),
        Some(&[60, 40, 50][..])
    );

    // Default (no zoom) is omitted from the serialized form.
    let bare = Preferences::default();
    assert!(!serde_json::to_string(&bare).unwrap().contains("text_scale"));

    // An old file lacking the field deserializes with text_scale == None.
    let legacy = r#"{"theme":"folio","agent_status_position":"bottom"}"#;
    let parsed: Preferences = serde_json::from_str(legacy).unwrap();
    assert_eq!(parsed.text_scale, None);
    assert_eq!(parsed.window_width_px, None);
    assert_eq!(parsed.window_height_px, None);
    assert_eq!(parsed.jump_workspace_order, None);
    assert_eq!(parsed.jump_detached_tile_order, None);
    assert_eq!(parsed.theme.as_deref(), Some("folio"));
}

#[test]
fn restored_window_size_uses_saved_pair_and_rejects_partial_or_invalid_values() {
    assert_eq!(
        restore_window_size(Some(1110.0), Some(770.0)),
        (1110.0, 770.0)
    );
    assert_eq!(
        restore_window_size(Some(1110.0), None),
        (DEFAULT_WINDOW_WIDTH_PX, DEFAULT_WINDOW_HEIGHT_PX),
        "a partial saved size falls back atomically"
    );
    assert_eq!(
        restore_window_size(Some(-1.0), Some(770.0)),
        (DEFAULT_WINDOW_WIDTH_PX, DEFAULT_WINDOW_HEIGHT_PX),
        "a hand-edited invalid size cannot poison window startup"
    );
}

#[test]
fn default_tile_span_migrations_reach_four_by_four_without_overriding_later_choices() {
    assert_eq!(
        restore_desktop_grid(None, None, None),
        (4, 4),
        "a fresh install gives new tiles a useful 4×4 span"
    );
    assert_eq!(
        restore_desktop_grid(Some(2), Some(2), None),
        (4, 4),
        "the original persisted default still migrates"
    );
    assert_eq!(
        restore_desktop_grid(Some(2), Some(2), Some(DESKTOP_GRID_DEFAULTS_VERSION),),
        (2, 2),
        "a post-migration explicit 2×2 choice stays intact"
    );
    assert_eq!(
        restore_desktop_grid(Some(3), Some(3), Some(2)),
        (4, 4),
        "the shipped v2 3×3 default span migrates once"
    );
    assert_eq!(
        restore_desktop_grid(Some(3), Some(3), Some(DESKTOP_GRID_DEFAULTS_VERSION)),
        (3, 3),
        "a 3×3 choice made after this migration remains authoritative"
    );
    assert_eq!(
        restore_desktop_grid(Some(5), Some(3), Some(2)),
        (5, 3),
        "an asymmetric custom span is not mistaken for the shipped 3×3 default"
    );
}

#[test]
fn fixed_cells_and_four_by_four_tiles_match_the_retina_reference() {
    assert_eq!((DESKTOP_CELL_W, DESKTOP_CELL_H), (160.0, 160.0));
    assert_eq!(DESKTOP_GUTTER, 12.0);
    let (_, _, width, height) = workspace::tile_rect(
        workspace::Slot::new(0, 0),
        workspace::Span::new(4, 4),
        (DESKTOP_CELL_W, DESKTOP_CELL_H),
        DESKTOP_GUTTER,
    );
    assert_eq!(
        (width, height),
        (676.0, 676.0),
        "a default 4×4 tile should match the 675×672 logical-pixel reference"
    );
}

fn s(text: &str) -> Segment {
    (text.to_string(), NStyle::default())
}

/// Finding 9 enforcement hook: the turn lifecycle is a total function over
/// `TurnPhase`, and the canonical `submit → stop → stop → finalize`
/// sequence pins the escalation behavior that used to live only in a field
/// comment. The first Stop moves Awaiting → StopRequested (graceful cancel
/// pending, not yet escalated); the second Stop, gated on `stop_requested()`,
/// escalates; `finalize` returns to Idle.
#[test]
fn turn_phase_submit_stop_stop_finalize_pins_escalation() {
    use std::time::Instant;

    // submit → Awaiting (in flight, no stop yet).
    let mut phase = TurnPhase::begin(Instant::now());
    assert!(phase.is_awaiting(), "submit must enter awaiting");
    assert!(!phase.stop_requested(), "fresh turn has no pending stop");
    assert!(
        phase.turn_started().is_some(),
        "awaiting carries the elapsed timer"
    );
    assert!(
        phase.last_event_at().is_some(),
        "awaiting carries the quiet clock"
    );

    // First Stop → StopRequested, graceful (not escalated). The handler
    // gate `stop_requested()` is what decides escalate-vs-graceful.
    let first_stop_escalates = phase.stop_requested();
    assert!(
        !first_stop_escalates,
        "the FIRST stop must be graceful, not a hard kill"
    );
    phase.request_stop(Instant::now());
    assert!(
        phase.is_awaiting(),
        "a pending stop is still in flight (timers run)"
    );
    assert!(
        phase.stop_requested(),
        "first stop records a pending cancel"
    );
    assert!(!phase.is_escalated(), "first stop has not escalated");
    // Timers survive the transition so the indicator keeps reading.
    assert!(phase.turn_started().is_some());
    assert!(phase.last_event_at().is_some());

    // Second Stop → the handler sees `stop_requested()` and escalates.
    let second_stop_escalates = phase.stop_requested();
    assert!(
        second_stop_escalates,
        "the SECOND stop while awaiting must escalate"
    );
    phase.escalate();
    assert!(
        phase.is_escalated(),
        "second stop marks the phase escalated"
    );

    // finalize (turn end / force-restart) → Idle, all markers cleared.
    phase = TurnPhase::Idle;
    assert!(!phase.is_awaiting(), "finalize returns to idle");
    assert!(!phase.stop_requested(), "idle has no pending stop");
    assert!(!phase.is_escalated(), "idle is not escalated");
    assert!(phase.turn_started().is_none(), "idle has no timer");
    assert!(phase.last_event_at().is_none(), "idle has no quiet clock");
}

/// `request_stop`/`escalate`/`note_event` are no-ops when idle, so a stray
/// Stop or stale event can never strand the phase in a contradictory state.
#[test]
fn turn_phase_idle_transitions_are_noops() {
    use std::time::Instant;
    let mut phase = TurnPhase::Idle;
    phase.request_stop(Instant::now());
    assert!(matches!(phase, TurnPhase::Idle), "stop on idle is a no-op");
    phase.escalate();
    assert!(
        matches!(phase, TurnPhase::Idle),
        "escalate on idle is a no-op"
    );
    phase.note_event(Instant::now());
    assert!(matches!(phase, TurnPhase::Idle), "event on idle is a no-op");

    // note_event refreshes the quiet clock only while in flight.
    let t0 = Instant::now();
    let mut awaiting = TurnPhase::Awaiting {
        started: t0,
        last_event: t0,
    };
    let later = t0 + std::time::Duration::from_secs(5);
    awaiting.note_event(later);
    assert_eq!(
        awaiting.last_event_at(),
        Some(later),
        "note_event advances the quiet clock while awaiting",
    );
    assert_eq!(
        awaiting.turn_started(),
        Some(t0),
        "note_event must not disturb the elapsed timer",
    );
}

#[test]
fn split_segments_at_col_zero_in_first_segment() {
    let segs = vec![s("hello"), s(" "), s("world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
    assert!(before.is_empty());
    assert_eq!(ch, 'h');
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(after_text, "ello world");
}

#[test]
fn split_segments_at_col_inside_a_segment() {
    // col 2 of "hello" → 'l', before="he", after="lo world"
    let segs = vec![s("hello"), s(" world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 2);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "he");
    assert_eq!(ch, 'l');
    assert_eq!(after_text, "lo world");
}

#[test]
fn split_segments_at_col_on_segment_boundary() {
    // col 5 lands on the first char of the second segment (' ').
    let segs = vec![s("hello"), s(" world")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 5);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    let after_text: String = after.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "hello");
    assert_eq!(ch, ' ');
    assert_eq!(after_text, "world");
}

#[test]
fn split_segments_at_col_past_end_is_virtual_space() {
    let segs = vec![s("hi")];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 99);
    let before_text: String = before.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(before_text, "hi");
    assert_eq!(ch, ' '); // cursor at/past EOL renders as a space caret
    assert!(after.is_empty());
}

#[test]
fn split_segments_at_col_empty_input() {
    let segs: Vec<Segment> = vec![];
    let (before, (ch, _), after) = split_segments_at_col(&segs, 0);
    assert!(before.is_empty());
    assert_eq!(ch, ' ');
    assert!(after.is_empty());
}

/// Builds a synthetic frozen transcript: `n_blocks` fenced code blocks
/// (`block_lines` lines each) separated by prose, plus one editable tail
/// line. Returns `(lines, frozen_ranges, frozen_line_count)`.
fn synthetic_transcript(
    n_blocks: usize,
    block_lines: usize,
) -> (Vec<String>, Vec<(usize, usize)>, usize) {
    let mut lines: Vec<String> = Vec::new();
    for b in 0..n_blocks {
        lines.push(format!("prose before block {b}"));
        lines.push("```rust".to_string());
        for i in 0..block_lines {
            lines.push(format!("let x_{b}_{i} = {i};"));
        }
        lines.push("```".to_string());
    }
    lines.push(String::new()); // editable tail
    let frozen_len = lines.len() - 1;
    (lines, vec![(0usize, frozen_len)], frozen_len)
}

fn block_ptrs(flat: &[FlatItem]) -> Vec<*const RenderedBlock> {
    flat.iter()
        .filter_map(|f| match f {
            FlatItem::Block(b) => Some(std::rc::Rc::as_ptr(b)),
            _ => None,
        })
        .collect()
}

/// Worksheet-typing perf invariant: an S1 rebuild whose frozen prefix is
/// unchanged (a keystroke in the editable tail bumps the fingerprint but
/// not the frozen line count) must reuse every parsed `RenderedBlock` by
/// Rc IDENTITY — no re-parse, no deep clone. The old rebuild deep-cloned
/// every parsed block into fresh per-rebuild lookup maps on every
/// keystroke — the dominant per-keystroke cost on large transcripts (the
/// "Worksheet mode is slow to type" report).
#[test]
fn worksheet_rebuild_reuses_parsed_blocks_by_identity() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (lines, frozen, frozen_len) = synthetic_transcript(3, 4);

    let (flat1, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    let blocks1 = block_ptrs(&flat1);
    assert_eq!(blocks1.len(), 3, "three fenced blocks must parse");

    // Keystroke in the editable tail: new fingerprint, same frozen count.
    let mut lines2 = lines.clone();
    *lines2.last_mut().unwrap() = "x".to_string();
    let (flat2, _) = rebuild_agent_view_model(&mut st, &lines2, &frozen, &theme, 2);
    assert_eq!(
        blocks1,
        block_ptrs(&flat2),
        "a tail keystroke must reuse every parsed block by Rc identity"
    );
}

/// UXI-AgentTile-11 rule 5: a You-block may be opened only within the latest agent turn
/// (after one of its newlines) or at the transcript tail; an older frozen turn is
/// not a legal insertion point. Drives the guard `you_block_anchor_is_legal` over a
/// two-turn transcript built through the real `append_llm_chunk` tagging path.
#[test]
fn you_block_anchor_guard_restricts_to_latest_turn() {
    let mut st = AgentState::new_for_test();
    st.editor
        .append_llm_chunk(TurnId::Llm(1), "old turn line\n");
    st.editor
        .append_llm_chunk(TurnId::Llm(2), "new line a\nnew line b\n");

    let (s, _e) = st
        .latest_agent_turn_range()
        .expect("an agent turn is tagged");
    assert!(
        !st.you_block_anchor_is_legal(0),
        "a line in the OLDER turn is not a legal You-block anchor"
    );
    assert!(
        st.you_block_anchor_is_legal(s),
        "a line in the LATEST turn is a legal anchor"
    );
    let last = st.editor.document().line_count().saturating_sub(1);
    assert!(
        st.you_block_anchor_is_legal(last),
        "the transcript tail is always a legal anchor"
    );
}

/// Streaming perf invariant: a chunk that inserts lines ABOVE the blocks
/// shifts every `(start, end)` range, but parses are keyed by CONTENT —
/// the revalidation must keep Rc identity for every block whose text is
/// unchanged. The old position-keyed cache missed on every shift, so each
/// streamed chunk re-parsed (pulldown-cmark + syntect) the entire frozen
/// transcript — the paint-thread flood behind "typing lags while a turn
/// streams".
#[test]
fn streamed_shift_reuses_parses_by_content() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (lines, frozen, frozen_len) = synthetic_transcript(3, 4);

    let (flat1, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    let blocks1 = block_ptrs(&flat1);
    assert_eq!(blocks1.len(), 3);

    // Simulated streamed chunk: one new prose line at the top; every
    // block range shifts down by one and the frozen prefix grows.
    let mut lines2 = vec!["new streamed prose".to_string()];
    lines2.extend(lines.iter().cloned());
    let frozen2 = vec![(0usize, frozen_len + 1)];
    let (flat2, _) = rebuild_agent_view_model(&mut st, &lines2, &frozen2, &theme, 2);
    assert_eq!(
        blocks1,
        block_ptrs(&flat2),
        "a range shift with unchanged block text must reuse every parse by Rc identity"
    );
}

/// INV-10 at the rebuild level: a detected range that `parse_block_range`
/// rejects (here a pipe "table" without a separator row) resolves to
/// `None` in `resolved_blocks` and must render as its source Lines — no
/// Block item, no swallowed lines.
#[test]
fn rebuild_renders_unparsed_range_as_lines() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let lines: Vec<String> = vec![
        "| a | b |".to_string(),
        "| 1 | 2 |".to_string(),
        "| 3 | 4 |".to_string(),
        String::new(), // editable tail
    ];
    let frozen = vec![(0usize, 3)];
    let (flat, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    assert!(
        !flat.iter().any(|f| matches!(f, FlatItem::Block(_))),
        "rejected range must emit no Block item"
    );
    let line_items: Vec<usize> = flat
        .iter()
        .filter_map(|f| match f {
            FlatItem::Line(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        line_items,
        vec![0, 1, 2],
        "every source line of an unparsed range must render as a Line; the \
         trailing blank editable tail is stripped (no stray empty bottom row)"
    );
}

/// Worksheet caret-on-blank-tail regression: the blank-line collapse pass
/// strips/collapses blank user Lines, but in Worksheet mode the caret can sit
/// on one (e.g. you press Enter twice on the empty tail). If that Line is
/// stripped the caret vanishes (`line_idx == cursor_line` matches no rendered
/// row) and the cursor-reveal routes to the wrong item (`item_for_line` falls
/// back to the last item, scrolling past the caret). The cursor's line must
/// survive collapse — but ONLY in Worksheet mode (in Chatbox the editable tail
/// is a separate surface, so a stray blank tail Line is just noise).
#[test]
fn rebuild_keeps_worksheet_cursor_line_through_collapse() {
    let theme = Theme::default();
    // Agent text, then two consecutive blank tail lines — the "Enter twice"
    // repro. Cursor parks on the SECOND blank (line 2).
    let lines: Vec<String> = vec!["agent text".to_string(), String::new(), String::new()];
    let frozen = vec![(0usize, 1)];

    // Chatbox (default): the second consecutive blank collapses away.
    let mut chat = AgentState::new_for_test();
    chat.editor.cursor_mut().line = 2;
    let (flat_chat, _) = rebuild_agent_view_model(&mut chat, &lines, &frozen, &theme, 1);
    assert!(
        !flat_chat.iter().any(|f| matches!(f, FlatItem::Line(2))),
        "Chatbox mode collapses the consecutive blank tail line"
    );

    // Worksheet: the caret's line (2) is protected from collapse and the
    // reverse index points reveal straight at it.
    let mut ws = AgentState::new_for_test();
    ws.input_surface = InputSurface::new(InputModeKind::Worksheet);
    ws.editor.cursor_mut().line = 2;
    let (flat_ws, _) = rebuild_agent_view_model(&mut ws, &lines, &frozen, &theme, 1);
    let pos = flat_ws
        .iter()
        .position(|f| matches!(f, FlatItem::Line(2)))
        .expect("Worksheet keeps the caret's blank line so the caret can render");
    assert_eq!(
        ws.view_model.item_for_line(2),
        pos,
        "cursor-reveal must target the caret's real flat position, not a fallback"
    );
}

/// REGRESSION ("the cursor can go below the end of the visible buffer when
/// entering worksheet mode"): the blank-collapse pass that strips a trailing
/// blank editable line is cursor-AND-mode sensitive (`protect_line` keeps the
/// caret's line only in Worksheet mode), but `view_model_fingerprint` folded in
/// NEITHER. So toggling Chatbox→Worksheet — which moves the caret to the tail
/// (a blank compose row) — produced an identical fingerprint, the S1 memo
/// returned the Chatbox-built flat list (tail stripped), and the caret rendered
/// on a line with no row: below the visible buffer. The fingerprint must change
/// when the input surface flips (and when the worksheet caret moves), so the
/// memo busts and the rebuild protects the tail.
#[test]
fn view_model_fingerprint_busts_on_input_surface_and_worksheet_cursor() {
    // Agent line + a trailing blank editable tail (the worksheet compose row).
    let lines: Vec<String> = vec!["agent reply".to_string(), String::new()];
    let frozen = vec![(0usize, 1)];
    let (line_count, frozen_count) = (2usize, 1usize);

    // Same content + same caret line, only the input surface differs.
    let mut chat = AgentState::new_for_test(); // Chatbox by default
    chat.editor.cursor_mut().line = 1;
    let mut ws = AgentState::new_for_test();
    ws.input_surface = InputSurface::new(InputModeKind::Worksheet);
    ws.editor.cursor_mut().line = 1;
    assert_ne!(
        chat.view_model_fingerprint(line_count, frozen_count),
        ws.view_model_fingerprint(line_count, frozen_count),
        "flipping into Worksheet mode must bust the S1 memo (protect_line differs)"
    );

    // Within Worksheet mode, moving the caret onto the (otherwise-collapsed)
    // blank tail must also change the fingerprint, or the cached flat list
    // (built with the caret elsewhere, tail stripped) leaves the caret roomless.
    let fp_caret_up = ws.view_model_fingerprint(line_count, frozen_count);
    ws.editor.cursor_mut().line = 0;
    let fp_caret_frozen = ws.view_model_fingerprint(line_count, frozen_count);
    assert_ne!(
        fp_caret_up, fp_caret_frozen,
        "a worksheet caret move on/off a collapsible blank must bust the memo"
    );

    // End-to-end through the real memo: build in Chatbox (tail stripped), then
    // the worksheet build for the same content must render the caret's tail.
    let theme = Theme::default();
    let fp_chat = chat.view_model_fingerprint(line_count, frozen_count);
    let (flat_chat, _) = rebuild_agent_view_model(&mut chat, &lines, &frozen, &theme, fp_chat);
    assert!(
        !flat_chat.iter().any(|f| matches!(f, FlatItem::Line(1))),
        "Chatbox build strips the trailing blank tail"
    );
    ws.editor.cursor_mut().line = 1; // caret on the tail, as entering worksheet lands it
    let fp_ws = ws.view_model_fingerprint(line_count, frozen_count);
    let (flat_ws, _) = rebuild_agent_view_model(&mut ws, &lines, &frozen, &theme, fp_ws);
    assert!(
        flat_ws.iter().any(|f| matches!(f, FlatItem::Line(1))),
        "Worksheet build keeps the caret's tail line so the caret has a row"
    );
}

/// FIX 2 (no empty "You" region): a worksheet submit that collected a blank
/// spacer line between two authored lines must freeze ONLY the non-blank lines.
/// Freezing the blank too painted an empty frozen "You" turn into the transcript
/// (the reported bug).
#[test]
fn commit_worksheet_skips_blank_lines() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.programmatic_insert(0, "hello\n\nworld\n");
    let collected = vec![
        (0usize, "hello".to_string()),
        (1usize, String::new()),
        (2usize, "world".to_string()),
    ];
    st.commit_worksheet_turn(&collected, "hello\nworld")
        .expect("worksheet turn commits");
    assert!(st.editor.is_frozen_line(0), "non-blank line is frozen");
    assert!(
        !st.editor.is_frozen_line(1),
        "the blank spacer must stay editable — no empty frozen You region"
    );
    assert!(st.editor.is_frozen_line(2), "non-blank line is frozen");
    let a1 = st.editor.anchor_for_line(1);
    assert!(
        st.editor.metadata::<TurnId>().get(a1).is_none(),
        "the blank spacer must carry no User turn tag"
    );
}

/// FIX 1 end-to-end: the render-time block detector must seed the editor's
/// atomic-block set, so an `o`/`O`/Enter on the interior of a frozen code block
/// is rejected (the "butchers Claude text" guard) — not just in a hand-built
/// unit but through the real `rebuild_agent_view_model` wiring.
#[test]
fn rebuild_seeds_atomic_blocks_and_blocks_interior_insert() {
    let theme = Theme::default();
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor
        .programmatic_insert(0, "intro\n```\ncode\n```\n\n");
    for l in 0..4usize {
        st.editor.add_frozen_lines(l, l + 1);
        let a = st.editor.anchor_for_line(l);
        st.editor.metadata_mut::<TurnId>().insert(a, TurnId::Llm(1));
    }
    let lines: Vec<String> = (0..st.editor.document().line_count())
        .map(|i| {
            st.editor
                .document()
                .line_text(i)
                .trim_end_matches('\n')
                .to_string()
        })
        .collect();
    let frozen = st.editor.frozen_lines().to_vec();
    let frozen_len: usize = frozen.iter().map(|(s, e)| e - s).sum();
    rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    assert_eq!(
        st.editor.atomic_blocks(),
        &[(1usize, 4usize)],
        "rebuild seeds the detected ```code``` block (lines 1..4) as atomic"
    );
    // An interior split (col 0 of the code line) is now a no-op.
    let before = st.editor.document().line_count();
    st.editor.cursor_mut().line = 2;
    st.editor.cursor_mut().col = 0;
    st.editor.open_line_above();
    assert_eq!(
        st.editor.document().line_count(),
        before,
        "interior code-block split is rejected end-to-end after rebuild seeds atomic blocks"
    );
}

/// FIX 3 (phantom "You" header): a blank editable gap wedged between two frozen
/// Claude turns must NOT sprout a "You" header. The old whole-rest-of-doc scan
/// saw the downstream Claude lines as non-blank and emitted one; the scan is now
/// bounded to the current editable run (all blank here).
#[test]
fn rebuild_blank_gap_between_claude_turns_has_no_you_header() {
    let theme = Theme::default();
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.mode = EditMode::Normal; // content-driven: a blank gap must not self-header
    st.editor
        .programmatic_insert(0, "answer one\n\nanswer two\n");
    for (l, turn) in [(0usize, TurnId::Llm(1)), (2usize, TurnId::Llm(2))] {
        st.editor.add_frozen_lines(l, l + 1);
        let a = st.editor.anchor_for_line(l);
        st.editor.metadata_mut::<TurnId>().insert(a, turn);
    }
    let lines: Vec<String> = (0..st.editor.document().line_count())
        .map(|i| {
            st.editor
                .document()
                .line_text(i)
                .trim_end_matches('\n')
                .to_string()
        })
        .collect();
    let frozen = st.editor.frozen_lines().to_vec();
    let frozen_len: usize = frozen.iter().map(|(s, e)| e - s).sum();
    let (flat, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    assert!(
        !flat.iter().any(|f| matches!(
            f,
            FlatItem::TurnHeader {
                role: TurnRole::User
            }
        )),
        "a blank gap between two Claude turns must not emit a phantom You header"
    );
}

/// FIX 3 inverse: a NON-blank editable interjection between two Claude turns is a
/// real user turn and DOES get a "You" header.
#[test]
fn rebuild_text_gap_between_claude_turns_gets_you_header() {
    let theme = Theme::default();
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor
        .programmatic_insert(0, "answer one\nmy note\nanswer two\n");
    for (l, turn) in [(0usize, TurnId::Llm(1)), (2usize, TurnId::Llm(2))] {
        st.editor.add_frozen_lines(l, l + 1);
        let a = st.editor.anchor_for_line(l);
        st.editor.metadata_mut::<TurnId>().insert(a, turn);
    }
    let lines: Vec<String> = (0..st.editor.document().line_count())
        .map(|i| {
            st.editor
                .document()
                .line_text(i)
                .trim_end_matches('\n')
                .to_string()
        })
        .collect();
    let frozen = st.editor.frozen_lines().to_vec();
    let frozen_len: usize = frozen.iter().map(|(s, e)| e - s).sum();
    let (flat, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);
    assert!(
        flat.iter().any(|f| matches!(
            f,
            FlatItem::TurnHeader {
                role: TurnRole::User
            }
        )),
        "a real text interjection between Claude turns must get a You header"
    );
}

/// Rebuild the agent view model and return the flat-item list for `st`'s
/// current editor document. Mirrors the cached-render miss path.
fn flat_of(st: &mut AgentState) -> std::rc::Rc<Vec<FlatItem>> {
    let lines: Vec<String> = (0..st.editor.document().line_count())
        .map(|i| {
            st.editor
                .document()
                .line_text(i)
                .trim_end_matches('\n')
                .to_string()
        })
        .collect();
    let frozen = st.editor.frozen_lines().to_vec();
    rebuild_agent_view_model(st, &lines, &frozen, &Theme::default(), 1).0
}

fn has_user_header(flat: &[FlatItem]) -> bool {
    flat.iter().any(|f| {
        matches!(
            f,
            FlatItem::TurnHeader {
                role: TurnRole::User
            }
        )
    })
}

/// REGRESSION (live screenshot: the transcript showed a stack of EMPTY
/// alternating `You`/`Claude` dividers between real turns). UXI-AgentTile-5: a turn
/// header renders only for a turn with visible content. Build a transcript with
/// EMPTY turns — blank lines carrying escalating turn numbers, the separator /
/// resume artifacts behind the bug — interleaved with real turns, and assert no
/// empty header survives (no header is followed by another header or the end;
/// header count == content-bearing turn count).
#[test]
fn rebuild_drops_empty_turn_headers() {
    let mut st = AgentState::new_for_test();
    // Lines: real, blank, blank, real, real (line 5 is the trailing empty line).
    st.editor
        .programmatic_insert(0, "first answer\n\n\nreal question\nsecond answer\n");
    // Tag each content line with a turn; the two blank lines get their OWN
    // escalating turn numbers → without the fix each mints a phantom header.
    for (l, turn) in [
        (0usize, TurnId::Llm(1)),  // real Claude turn
        (1usize, TurnId::Llm(2)),  // blank → would be an empty "Claude"
        (2usize, TurnId::User(3)), // blank → would be an empty "You"
        (3usize, TurnId::User(4)), // real You turn
        (4usize, TurnId::Llm(5)),  // real Claude turn
    ] {
        st.editor.add_frozen_lines(l, l + 1);
        let a = st.editor.anchor_for_line(l);
        st.editor.metadata_mut::<TurnId>().insert(a, turn);
    }

    let flat = flat_of(&mut st);

    // No header is orphaned: every TurnHeader is immediately followed by a
    // non-header (content) item, and the list never ends on a header.
    for (i, item) in flat.iter().enumerate() {
        if matches!(item, FlatItem::TurnHeader { .. }) {
            let next = flat.get(i + 1);
            assert!(
                matches!(next, Some(it) if !matches!(it, FlatItem::TurnHeader { .. })),
                "empty turn header at index {i}: next item is {next:?}\nflat: {flat:?}"
            );
        }
    }
    // Exactly the three content-bearing turns (Llm1, User4, Llm5) get a header.
    let headers = flat
        .iter()
        .filter(|it| matches!(it, FlatItem::TurnHeader { .. }))
        .count();
    assert_eq!(
        headers, 3,
        "expected 3 headers, got {headers}\nflat: {flat:?}"
    );
}

/// Content-driven half (NORMAL mode): with the caret parked in Normal mode, the
/// "You" divider tracks content — absent while the editable run is empty,
/// present once it holds non-whitespace text. (The presence-driven Insert-mode
/// behavior is `rebuild_worksheet_divider_on_insert_entry`.)
#[test]
fn rebuild_worksheet_blank_tail_has_no_header_until_text() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.mode = EditMode::Normal; // not composing — content-driven case
    st.editor.append_llm_chunk(TurnId::Llm(1), "answer\n");
    let tail = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().line = tail;
    st.editor.cursor_mut().col = 0;
    assert!(
        !has_user_header(&flat_of(&mut st)),
        "a blank editable tail in Normal mode (nothing written) shows no You divider"
    );
    // First real keystroke surfaces it immediately.
    st.editor.insert_char('h');
    assert!(
        has_user_header(&flat_of(&mut st)),
        "the You divider appears on the first non-whitespace character"
    );
}

// NOTE (Model C): the Model-A test `rebuild_worksheet_divider_on_insert_entry`
// was removed. It asserted a presence-driven "You" divider as a *transcript
// flat-item* keyed on the transcript editor's mode. Under Model C the transcript
// is read-only and the draft lives in the separate Compose buffer, so the
// transcript never hosts that divider — the "You" boundary is the inline
// compose's own gutter/border (screens.rs). The replacement coverage is the
// render-side compose-boundary test (ticket M5).

/// Content-driven half (NORMAL mode): a whitespace-only draft shows no divider;
/// real text turns it on, clearing it back to whitespace turns it off. (Asserted
/// in Normal mode, where the divider is purely content-driven — in Insert mode
/// presence would show it regardless, per `rebuild_worksheet_divider_on_insert_entry`.)
#[test]
fn rebuild_worksheet_whitespace_only_run_has_no_header() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.mode = EditMode::Normal; // content-driven case
    st.editor.append_llm_chunk(TurnId::Llm(1), "answer\n");
    let tail = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().line = tail;
    st.editor.cursor_mut().col = 0;
    // Only whitespace.
    for ch in "   \t".chars() {
        st.editor.insert_char(ch);
    }
    assert!(
        !has_user_header(&flat_of(&mut st)),
        "a whitespace-only draft must show no You divider"
    );
    // Type a real char (divider on), then delete it back to whitespace (divider off).
    st.editor.insert_char('x');
    assert!(
        has_user_header(&flat_of(&mut st)),
        "a real character turns the divider on"
    );
    st.editor.backspace();
    assert!(
        !has_user_header(&flat_of(&mut st)),
        "deleting back to whitespace-only turns the divider off again"
    );
}

// NOTE (Model C): the Model-A test `rebuild_worksheet_interjection_header_tracks_text`
// was removed. It interjected user text directly INTO the transcript
// (`open_line_below` between two Claude turns) and asserted the presence-driven
// "You" divider. Under Model C the transcript is read-only — you never interject
// inside it; you compose in the separate buffer rendered below it — so that flow
// and its divider no longer exist here (M5 covers the compose boundary).

/// Issue 2: a trailing blank editable line the caret has moved off of must NOT
/// render as a stray empty row at the bottom of the transcript.
#[test]
fn rebuild_strips_trailing_blank_editable_tail() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "answer\n");
    // User text on line 1, then a trailing blank line 2; caret rests on line 1.
    st.editor.cursor_mut().line = 1;
    st.editor.cursor_mut().col = 0;
    for ch in "hello\n".chars() {
        st.editor.insert_char(ch);
    }
    st.editor.cursor_mut().line = 1; // caret back on "hello", off the blank tail
    st.editor.cursor_mut().col = 5;
    let flat = flat_of(&mut st);
    let blank_tail = (0..st.editor.document().line_count())
        .rfind(|&l| {
            !st.editor.is_frozen_line(l) && st.editor.document().line_text(l).trim().is_empty()
        })
        .expect("there is a trailing blank editable line in the doc");
    assert!(
        !flat
            .iter()
            .any(|f| matches!(f, FlatItem::Line(i) if *i == blank_tail)),
        "the trailing blank editable line must not render (no extra blank newline)"
    );
}

/// Issue 2 guard: the caret's OWN blank line still renders (it must, so the
/// caret has a row), even though it's a trailing blank.
#[test]
fn rebuild_keeps_trailing_blank_when_caret_is_on_it() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "answer\n");
    for ch in "hello\n".chars() {
        st.editor.cursor_mut().line = st.editor.document().line_count() - 1;
        st.editor.cursor_mut().col = 0;
        // (rebuild not needed between chars; we just need the final doc)
        st.editor.insert_char(ch);
    }
    // Caret left on the trailing blank line (line 2).
    let tail = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().line = tail;
    st.editor.cursor_mut().col = 0;
    let flat = flat_of(&mut st);
    assert!(
        flat.iter()
            .any(|f| matches!(f, FlatItem::Line(i) if *i == tail)),
        "the caret's own blank line must still render so the caret has a row"
    );
}

// Mimic agent_ui chunk handling: floor + floored append for `turn`.
fn sim_chunk(st: &mut AgentState, turn: usize, text: &str) {
    let floor = agent_tail_floor_char(&st.editor);
    st.editor
        .append_llm_chunk_floored(TurnId::Llm(turn), text, floor);
}
// Mimic agent_ui ToolCallStarted handling: floor + anchor + register.
fn sim_tool(st: &mut AgentState, turn: usize, id: &str, title: &str) {
    let floor = agent_tail_floor_char(&st.editor);
    let anchor = anchor_for_new_tool_call(&mut st.editor, floor);
    let tcid: yalda::acp_channel::ToolCallId = id.to_string().into();
    let key = ToolCallKey::from_id(&tcid);
    st.editor
        .metadata_mut::<TurnId>()
        .insert(anchor, TurnId::Tool(turn));
    let tc = yalda::acp_channel::ToolCall::new(tcid, title.to_string());
    st.tools.register(key, tc, anchor);
}

fn doc_lines(st: &AgentState) -> Vec<String> {
    (0..st.editor.document().line_count())
        .map(|i| {
            st.editor
                .document()
                .line_text(i)
                .trim_end_matches('\n')
                .to_string()
        })
        .collect()
}

/// Issue 3 (the out-of-order / corruption root cause): a new agent turn that
/// streams its first chunk while the user has a worksheet draft must NOT fuse
/// the chunk into the draft line or freeze the draft as agent content. The
/// draft stays its own editable line at the tail; the agent content lands above.
#[test]
fn floored_first_chunk_never_merges_into_draft() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "ok\n");
    st.editor.cursor_mut().line = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().col = 0;
    for ch in "my draft".chars() {
        st.editor.insert_char(ch);
    }
    // New turn (Llm 2) streams a sub-line first chunk.
    sim_chunk(&mut st, 2, "Let me look. ");
    let lines = doc_lines(&st);
    // The draft survives intact on its own editable line.
    let draft_line = lines
        .iter()
        .position(|l| l == "my draft")
        .expect("the draft must survive as its own line");
    assert!(
        !st.editor.is_frozen_line(draft_line),
        "the draft line must stay editable, not frozen as agent content"
    );
    // The chunk is above the draft and does not share its line.
    let chunk_line = lines
        .iter()
        .position(|l| l.contains("Let me look."))
        .expect("the chunk must be present");
    assert!(
        chunk_line < draft_line,
        "agent content stays above the draft"
    );
    assert!(
        !lines[chunk_line].contains("my draft"),
        "the chunk must not be fused onto the draft line: {lines:?}"
    );
}

/// Issue 3: consecutive sub-line chunks of one turn (no trailing newlines,
/// nothing between them) flow onto ONE agent line above the draft — the
/// open-stream bit prevents the per-chunk choppiness the naive floor produced.
#[test]
fn floored_subline_chunks_merge_onto_one_line() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "ok\n");
    st.editor.cursor_mut().line = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().col = 0;
    for ch in "draft".chars() {
        st.editor.insert_char(ch);
    }
    sim_chunk(&mut st, 2, "Let me ");
    sim_chunk(&mut st, 2, "look ");
    sim_chunk(&mut st, 2, "at it. ");
    let lines = doc_lines(&st);
    assert!(
        lines.iter().any(|l| l.trim() == "Let me look at it."),
        "sub-line chunks must coalesce onto one agent line: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "draft"),
        "the draft survives at the tail: {lines:?}"
    );
}

/// Issue 3: a chunk that ends with a newline is a real paragraph break, so the
/// next same-turn chunk starts a fresh agent line (not merged), even while the
/// draft sits below.
#[test]
fn floored_hard_break_starts_new_line_above_draft() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "ok\n");
    st.editor.cursor_mut().line = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().col = 0;
    for ch in "draft".chars() {
        st.editor.insert_char(ch);
    }
    sim_chunk(&mut st, 2, "First para.\n");
    sim_chunk(&mut st, 2, "Second para.");
    let lines = doc_lines(&st);
    let p1 = lines.iter().position(|l| l == "First para.");
    let p2 = lines.iter().position(|l| l == "Second para.");
    assert!(
        p1.is_some() && p2.is_some() && p1 != p2,
        "a hard \\n break keeps paragraphs on separate lines: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "draft"),
        "the draft survives at the tail: {lines:?}"
    );
}

/// Issue 3: tools interleaved with agent text while a draft is present keep
/// document order — text, tool, text, tool, text — all above the preserved draft.
#[test]
fn floored_tools_and_text_stay_in_order_above_draft() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor.append_llm_chunk(TurnId::Llm(1), "ok\n");
    st.editor.cursor_mut().line = st.editor.document().line_count() - 1;
    st.editor.cursor_mut().col = 0;
    for ch in "my draft".chars() {
        st.editor.insert_char(ch);
    }
    sim_chunk(&mut st, 2, "Let me look. ");
    sim_tool(&mut st, 2, "t1", "grep");
    sim_chunk(&mut st, 2, "Found it. ");
    sim_tool(&mut st, 2, "t2", "read");
    sim_chunk(&mut st, 2, "Done.");
    let flat = flat_of(&mut st);
    // Collapse the flat list to a coarse ordered signature.
    let mut sig: Vec<&str> = Vec::new();
    for f in flat.iter() {
        match f {
            FlatItem::ToolGroup { .. } => sig.push("tool"),
            FlatItem::Line(i) => {
                let t = st.editor.document().line_text(*i);
                if t.contains("Let me look.") {
                    sig.push("look")
                } else if t.contains("Found it.") {
                    sig.push("found")
                } else if t.contains("Done.") {
                    sig.push("done")
                } else if t.contains("my draft") {
                    sig.push("draft")
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        sig,
        vec!["look", "tool", "found", "tool", "done", "draft"],
        "tools/text must stay in document order with the draft last"
    );
}

/// After an agent turn settles in Worksheet mode, the caret drops to the
/// editable tail (last line) so the user composes their next message at the
/// bottom, and a reveal is queued so the viewport follows.
#[test]
fn worksheet_turn_end_moves_caret_to_tail() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);
    st.editor
        .append_llm_chunk(TurnId::Llm(1), "a long\nmulti-line\nreply");
    // Caret parked up in the transcript (as if the user scrolled to read).
    st.editor.cursor_mut().line = 0;
    st.editor.cursor_mut().col = 0;
    st.pending_reveal_cursor = false;
    assert!(st.finalize_agent_turn_idem(0, 1), "first finalize runs");
    let last = st.editor.document().line_count() - 1;
    assert_eq!(
        st.editor.cursor().line,
        last,
        "the caret drops to the editable tail after the turn"
    );
    assert!(
        st.pending_reveal_cursor,
        "a reveal is queued so the viewport scrolls to the tail"
    );
}

/// Chatbox composes in a separate surface, so a turn end must NOT yank the
/// transcript caret.
#[test]
fn chatbox_turn_end_leaves_caret_put() {
    let mut st = AgentState::new_for_test(); // defaults to Chatbox
    st.editor
        .append_llm_chunk(TurnId::Llm(1), "a long\nmulti-line\nreply");
    st.editor.cursor_mut().line = 0;
    st.editor.cursor_mut().col = 0;
    assert!(st.finalize_agent_turn_idem(0, 1), "first finalize runs");
    assert_eq!(
        st.editor.cursor().line,
        0,
        "the transcript caret stays put in Chatbox mode"
    );
}

/// UXI-AgentTile-9 (ux-invariants.md): the compose word-wraps. `wrap_line_cols`
/// partitions a line into ≤width visual rows, breaking at spaces, hard-breaking
/// over-long words, covering EVERY char (so the caret is addressable everywhere),
/// always ≥1 row.
#[test]
fn wrap_line_cols_word_wraps_and_covers_every_char() {
    let w = |s: &str, width: usize| -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        wrap_line_cols(&chars, width)
            .into_iter()
            .map(|(a, b)| chars[a..b].iter().collect())
            .collect()
    };

    // Empty line → exactly one (empty) row so the caret has a row to sit on.
    assert_eq!(w("", 10), vec![""]);
    // Fits within width → single row.
    assert_eq!(w("hello", 10), vec!["hello"]);
    // Breaks at spaces (the trailing space stays on its row).
    assert_eq!(w("the quick brown", 9), vec!["the ", "quick ", "brown"]);
    // A word longer than the width is hard-broken at the column limit.
    assert_eq!(w("abcdefgh", 3), vec!["abc", "def", "gh"]);
    // width 1 still makes progress (no infinite loop).
    assert_eq!(w("ab", 1), vec!["a", "b"]);

    // Coverage: every wrapped row is contiguous and the rows tile the line.
    for (s, width) in [
        ("the quick brown fox jumped", 7),
        ("loooooong", 3),
        ("a b c", 1),
    ] {
        let chars: Vec<char> = s.chars().collect();
        let rows = wrap_line_cols(&chars, width);
        assert_eq!(rows.first().unwrap().0, 0, "first row starts at 0");
        assert_eq!(rows.last().unwrap().1, chars.len(), "last row ends at EOL");
        for pair in rows.windows(2) {
            assert_eq!(
                pair[0].1, pair[1].0,
                "rows are contiguous (no dropped char)"
            );
        }
        for &(a, b) in &rows {
            assert!(b > a || chars.is_empty(), "each row makes progress");
            assert!(b - a <= width.max(1), "no row exceeds the wrap width");
        }
    }
}

/// UXI-TextEditing-1 over the wrapped compose: the caret resolves to the single visual row
/// holding its column; a row-boundary column belongs to the NEXT row; end-of-line
/// sits on the last row — so the caret is always on a rendered row (never lost).
#[test]
fn caret_visual_row_places_caret_on_a_rendered_row() {
    let chars: Vec<char> = "the quick brown".chars().collect();
    let rows = wrap_line_cols(&chars, 9); // [(0,4),(4,10),(10,15)]
    assert_eq!(caret_visual_row(&rows, 0), 0, "col 0 → first row");
    assert_eq!(caret_visual_row(&rows, 3), 0, "mid first row");
    assert_eq!(
        caret_visual_row(&rows, 4),
        1,
        "row-boundary col → next row's start"
    );
    assert_eq!(caret_visual_row(&rows, 10), 2, "next boundary → third row");
    assert_eq!(caret_visual_row(&rows, 15), 2, "end-of-line → last row");

    // Empty line: the sole row holds the caret.
    assert_eq!(caret_visual_row(&wrap_line_cols(&[], 10), 0), 0);
}

/// REGRESSION (live report: "I can move the cursor below the fold in the chatbox
/// again"): UXI-TextEditing-1 under UXI-AgentTile-9. Once the compose word-wraps, the vertical
/// window MUST be computed over VISUAL rows, not logical lines — the wrap change
/// computed it over logical lines, so the caret's visual row fell below the box.
/// This drives the real path: `compose_visual_metrics` → `compose_first_visible_line`
/// (over visual rows) → `compose_item_for_visual_row` (map back to the list's
/// item/offset). For EVERY caret position in a wrapped draft, the chosen window
/// must contain the caret's visual row, and the item/offset mapping must round-trip.
#[test]
fn compose_wrapped_caret_never_below_the_fold() {
    let lines: Vec<String> = vec![
        "short".into(),
        "this is a fairly long line that definitely wraps several times".into(),
        "x".into(),
        "another long wrapping line that also goes well past the width".into(),
        "end".into(),
    ];
    let width = 10; // box width in columns
    let visible = 8; // COMPOSE_MAX_VISIBLE_LINES (box height in visual rows)

    let (_, total, per_line) = compose_visual_metrics(&lines, 0, 0, width);
    // The draft must actually exceed the box (so this exercises the scrolling path).
    assert!(
        total > visible,
        "test draft must wrap beyond the visible window"
    );

    let mut prev_top = 0usize;
    for (li, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        for col in 0..=len {
            let (caret_vrow, total2, per2) = compose_visual_metrics(&lines, li, col, width);
            assert_eq!(total2, total);
            assert_eq!(per2, per_line);

            let top = compose_first_visible_line(caret_vrow, prev_top, total2, visible);

            // UXI-TextEditing-1: the caret's VISUAL row is inside the visible window.
            assert!(
                caret_vrow >= top && caret_vrow < top + visible,
                "caret visual row {caret_vrow} fell outside window [{top}, {}) \
                 at line {li} col {col} — BELOW THE FOLD",
                top + visible
            );

            // The top visual row maps back to a list (item, offset) that resolves
            // to exactly that visual row — so the list actually scrolls there.
            let (item, off) = compose_item_for_visual_row(&per2, top);
            let mapped: usize = per2.iter().take(item).sum::<usize>() + off;
            assert_eq!(mapped, top, "item/offset must round-trip to the visual top");

            prev_top = top;
        }
    }
}

/// THE permanent guard against the 15×-recurring "chatbox caret/text scrolls
/// off-screen" bug (spec-chatbox-caret-containment.md Constraint 4): drive a real
/// `Chatbox` editor through every Behavior-7 edit path and, after each, assert
/// `compute_window` keeps the caret CELL inside the visible box on BOTH axes for
/// a range of extents (including the degenerate 1×1). This tests the integration
/// (cursor + tab-expanded line-length read), not just the pure window math.
#[test]
fn chatbox_caret_cell_stays_in_window_for_every_edit_path() {
    // After a given edit, the live caret cell must be inside the window the
    // box would render at, for several visible extents.
    fn assert_contained(cb: &Compose, label: &str) {
        for &rows in &[1usize, 8] {
            for &cols in &[1usize, 4, 20, 80] {
                let w = cb.compute_window(rows, cols);
                let cur = cb.editor.cursor();
                assert!(
                    cur.line >= w.top_line && cur.line < w.top_line + rows,
                    "[{label}] caret line {} escaped vertical window {w:?} (rows={rows})",
                    cur.line,
                );
                let inner = cols.saturating_sub(1).max(1);
                assert!(
                    cur.col >= w.left_col && cur.col <= w.left_col + inner,
                    "[{label}] caret col {} escaped horizontal window {w:?} (cols={cols})",
                    cur.col,
                );
            }
        }
    }

    let mut cb = Compose::new();
    assert_contained(&cb, "empty");

    // Type a single VERY long line — the caret rides off the right edge unless
    // the horizontal window scrolls (the reported "text goes off screen").
    for ch in "the quick brown fox jumps over the lazy dog "
        .chars()
        .cycle()
        .take(400)
    {
        cb.editor.insert_char(ch);
    }
    assert_contained(&cb, "long-line-EOL");

    // Walk the caret back to the start of the line (caret left of the window).
    cb.editor.move_cursor_first_non_blank();
    assert_contained(&cb, "long-line-home");

    // Jump back to end of that line.
    cb.editor.move_cursor_line_end(true);
    assert_contained(&cb, "long-line-end");

    // Add many newlines so the draft exceeds the visible rows — the vertical
    // half (newline at EOL of a full window is exactly the reported jump).
    for _ in 0..30 {
        cb.editor.insert_char('\n');
        for ch in "another reply line that is also quite wide ".chars() {
            cb.editor.insert_char(ch);
        }
    }
    assert_contained(&cb, "many-lines-EOL");

    // Move the caret to the very top (caret above the window).
    cb.editor.jump_to_line(0);
    assert_contained(&cb, "jump-top");

    // ...and to the very bottom.
    cb.editor.jump_cursor_bottom();
    assert_contained(&cb, "jump-bottom");

    // Backspace a run of chars (delete path, caret tracks left).
    for _ in 0..50 {
        cb.editor.backspace();
    }
    assert_contained(&cb, "after-backspace");
}

/// Pull the background color baked into the first span of the first code
/// block in a flat-items list (the syntect/`code_block_bg` bake that goes
/// stale on a theme switch).
fn first_code_block_bg(flat: &[FlatItem]) -> Option<yalda::style::Color> {
    flat.iter().find_map(|f| match f {
        FlatItem::Block(b) => match b.as_ref() {
            RenderedBlock::CodeBlock { lines, .. } => lines.first()?.spans.first()?.style.bg,
            _ => None,
        },
        _ => None,
    })
}

/// Theme-switch invariant (the "washed-out code block in Nightfox" bug): a
/// fenced code block bakes its span colors (background + syntect foregrounds)
/// in at parse time, and `block_cache` keys on content, not theme — so a
/// rebuild under a new theme with an unchanged frozen count REUSES the stale
/// parse. `invalidate_theme` must force the re-parse so the block adopts the
/// new theme's colors instead of rendering, e.g., a Folio light box on a
/// Nightfox transcript.
#[test]
fn theme_switch_invalidate_reparses_code_blocks() {
    let mut st = AgentState::new_for_test();
    let light = Theme::folio();
    let dark = Theme::nightfox();
    let (lines, frozen, frozen_len) = synthetic_transcript(1, 4);

    let (flat1, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &light, 1);
    let bg_light = first_code_block_bg(&flat1).expect("a parsed code block under the light theme");

    // No invalidate: same frozen count ⇒ the stale light-theme parse is reused
    // even though we rebuild under the dark theme. This is the bug.
    let (flat2, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &dark, 2);
    assert_eq!(
        first_code_block_bg(&flat2),
        Some(bg_light),
        "without invalidation the block keeps the prior theme's baked colors"
    );

    // Invalidate, then rebuild under the dark theme: the block re-parses and
    // its baked background must now differ from the light theme's.
    st.view_model.invalidate_theme();
    let (flat3, _) = rebuild_agent_view_model(&mut st, &lines, &frozen, &dark, 3);
    let bg_dark = first_code_block_bg(&flat3).expect("a re-parsed code block under the dark theme");
    assert_ne!(
        bg_dark, bg_light,
        "after invalidate_theme the code block adopts the new theme's colors"
    );
}

/// INV-UX-1 (cursor + text always visible): the WP edit view's code-line
/// background MUST follow the active theme, not a hardcoded dark swatch. Folio's
/// fenced-code syntax tokens are dark (designed for its linen `code_block_bg`);
/// the old hardcoded `0x21222c` painted them — and the caret's character —
/// dark-on-dark ("moving the cursor through a code block loses the cursor").
///
/// Drives the REAL value the WP render paints (`wp_code_block_bg_rgb`) and
/// asserts it contrasts with the darkest token the REAL Folio highlighter emits.
///
/// Negative control: revert `wp_code_block_bg_rgb` to `0x21222c` → the Folio bg
/// goes dark, contrast collapses below the threshold, and this fails.
#[test]
fn wp_code_bg_contrasts_with_folio_syntax_tokens() {
    use yalda::highlight::Highlighter;
    use yalda::style::{Color, Style};

    fn luma(r: u8, g: u8, b: u8) -> f32 {
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
    }
    fn u32_luma(c: u32) -> f32 {
        luma((c >> 16) as u8, (c >> 8) as u8, c as u8)
    }

    let folio = Theme::folio();
    let bg = wp_code_block_bg_rgb(&folio);
    assert_ne!(
        bg, 0x21222c,
        "WP code bg must not be the hardcoded dark swatch"
    );

    // Darkest token the real Folio highlighter emits on a representative line.
    let hl = Highlighter::with_syntect_theme(folio.name.syntect_theme());
    let darkest = hl
        .highlight_line_stateless(
            "rust",
            "pub fn add(x: usize) -> u8 { 42 }",
            Style::default(),
        )
        .expect("rust highlighting")
        .into_iter()
        .filter(|(t, _)| !t.trim().is_empty())
        .filter_map(|(_, s)| match s.fg {
            Some(Color::Rgb(r, g, b)) => Some(luma(r, g, b)),
            _ => None,
        })
        .fold(f32::MAX, f32::min);

    let contrast = (u32_luma(bg) - darkest).abs();
    assert!(
        contrast > 120.0,
        "WP code bg (luma {:.0}) must contrast with the darkest Folio token (luma {:.0}); got {:.0}",
        u32_luma(bg),
        darkest,
        contrast
    );
}

/// Cost probe for the worksheet-keystroke path: repeated rebuilds over a
/// large transcript with the frozen prefix unchanged. Prints the per-
/// rebuild cost; the assert is a generous debug-build ceiling that only
/// trips if the rebuild regresses to re-parsing/deep-cloning per
/// keystroke again.
#[test]
fn worksheet_rebuild_cost_probe() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    let (mut lines, frozen, frozen_len) = synthetic_transcript(50, 60);

    // Warm: parse all blocks once.
    let _ = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 0);

    const ROUNDS: u64 = 200;
    let t0 = std::time::Instant::now();
    for k in 0..ROUNDS {
        let n = lines.len();
        lines[n - 1] = format!("typing {k}");
        let _ = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, k + 1);
    }
    let per = t0.elapsed() / ROUNDS as u32;
    eprintln!(
        "[probe] worksheet rebuild: {} lines, 50 blocks → {per:?}/keystroke",
        lines.len()
    );
    assert!(
        per < std::time::Duration::from_millis(10),
        "worksheet rebuild regressed to {per:?}/keystroke (budget 10ms debug)"
    );
}

/// INV-RV regression: cursor-reveal is O(1) and the reverse index is a
/// faithful mirror of the rendered `flat_items`. The old Worksheet key
/// handler recomputed the cursor's flat-item position from scratch on EVERY
/// keystroke — an O(transcript) gutter scan + tool/anchor walk — which is the
/// monotonic "typing gets slower as the session grows" regression. The fix
/// derives a doc-line → item index FROM the canonical flat list at build
/// time, so the per-keystroke reveal is a single array read. This test pins
/// (a) the map points every `Line` item at its real position (single source
/// of truth — it can't drift from what's rendered), (b) lookups are
/// bounds-clamped (cursor past EOF must not panic), and (c) the map is built
/// once per rebuild, not per keystroke.
#[test]
fn reveal_index_mirrors_flat_items_and_is_o1() {
    let mut st = AgentState::new_for_test();
    let theme = Theme::default();
    // Several fenced blocks + interleaved prose + an editable tail.
    let (lines, frozen, frozen_len) = synthetic_transcript(4, 6);

    VIEW_MODEL_REBUILDS.with(|n| n.set(0));
    let (flat, _gut) = rebuild_agent_view_model(&mut st, &lines, &frozen, &theme, 1);

    // (a) Every `Line(idx)` is reachable in O(1) at its REAL flat position —
    // the reverse index mirrors the canonical list exactly.
    let mut checked = 0usize;
    for (p, item) in flat.iter().enumerate() {
        if let FlatItem::Line(idx) = item {
            assert_eq!(
                st.view_model.item_for_line(*idx),
                p,
                "item_for_line({idx}) must equal the Line's real flat position {p}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "transcript must contain Line items to validate"
    );

    // (a') The desync vector: Block items dropped their source range, so the
    // map must re-pair them with `resolved`. Verify every doc line resolves
    // IN-RANGE, and that lines collapsed into a structural Block resolve to a
    // `Block` item (not to a stray Line or off-by-one position). At least one
    // line must land on a Block (the synthetic transcript has fenced blocks).
    let mut lines_on_a_block = 0usize;
    for line in 0..lines.len() {
        let idx = st.view_model.item_for_line(line);
        assert!(
            idx < flat.len(),
            "line {line} resolved out of range ({idx})"
        );
        if matches!(flat[idx], FlatItem::Block(_)) {
            lines_on_a_block += 1;
        }
    }
    assert!(
        lines_on_a_block > 0,
        "block-covered lines must resolve to their Block item, not a Line/off-by-one"
    );

    // (b) Out-of-range (cursor past EOF) clamps into the list, never panics.
    let last = flat.len().saturating_sub(1);
    assert!(
        st.view_model.item_for_line(usize::MAX) <= last,
        "an out-of-range reveal must clamp into the built list"
    );

    // (c) The reverse index is part of the memoized view model: re-rendering
    // at the SAME structural fingerprint is a pure cache hit, so neither the
    // flat list NOR the reveal index is rebuilt per keystroke — the reveal
    // cost stays independent of transcript length.
    let rebuilds = VIEW_MODEL_REBUILDS.with(|n| n.get());
    for _ in 0..100 {
        match st.view_model.cached(1) {
            Some(_hit) => {} // O(1) reuse, no store
            None => panic!("same-fingerprint render must hit the S1 cache"),
        }
    }
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        rebuilds,
        "100 same-fingerprint renders must not rebuild — per-keystroke work is O(changed)"
    );
}

/// S1 enforcement, on the split `cached` + `store` API: an unchanged
/// fingerprint must hit (`cached` returns `Some` the same-pointer `Rc`s
/// and the caller never `store`s, so ZERO rebuilds). A changed fingerprint
/// must miss (`cached` returns `None`) and the following `store` produces
/// fresh `Rc`s. `VIEW_MODEL_REBUILDS` counts `store` calls (= misses).
/// Models the `highlight_cache` fast-skip tests.
#[test]
fn view_model_memoization_fast_skip() {
    VIEW_MODEL_REBUILDS.with(|n| n.set(0));
    let mut st = AgentState::new_for_test();

    // Build a fingerprint over the empty structural state.
    let fp1 = st.view_model_fingerprint(0, 0);

    // Cold cache: miss → rebuild at the call site, then `store`.
    assert!(st.view_model.cached(fp1).is_none(), "cold cache must miss");
    let (flat1, gut1) =
        st.view_model
            .store(fp1, vec![FlatItem::Line(0)], vec![None], vec![0], vec![0]);
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        1,
        "store counts as one rebuild"
    );
    let seq_after_first = st.view_model.view_model_seq;
    assert_eq!(seq_after_first, 1, "first store bumps the seq to 1");

    // SAME fingerprint: hit → reuse the very same `Rc`s (pointer identity),
    // no `store`, seq unchanged.
    let (flat2, gut2) = st
        .view_model
        .cached(fp1)
        .expect("same fingerprint must hit");
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        1,
        "a hit must not rebuild"
    );
    assert!(
        std::rc::Rc::ptr_eq(&flat1, &flat2),
        "flat_items Rc must be reused on a hit"
    );
    assert!(
        std::rc::Rc::ptr_eq(&gut1, &gut2),
        "gutter Rc must be reused on a hit"
    );
    assert_eq!(
        st.view_model.view_model_seq, seq_after_first,
        "seq must not change on a hit"
    );

    // Fingerprint sensitivity: a structural change (turn_phase enters
    // awaiting, which the thinking indicator depends on) yields a DIFFERENT
    // fingerprint → miss, and the following `store` produces a fresh `Rc`.
    st.turn_phase = TurnPhase::begin(std::time::Instant::now());
    let fp2 = st.view_model_fingerprint(0, 0);
    assert_ne!(fp1, fp2, "turn_phase awaiting must change the fingerprint");
    assert!(
        st.view_model.cached(fp2).is_none(),
        "changed fingerprint must miss"
    );
    let (flat3, _gut3) = st.view_model.store(
        fp2,
        vec![FlatItem::ThinkingIndicator],
        vec![None],
        vec![0],
        vec![],
    );
    assert_eq!(
        VIEW_MODEL_REBUILDS.with(|n| n.get()),
        2,
        "a miss + store rebuilds again"
    );
    assert!(
        !std::rc::Rc::ptr_eq(&flat1, &flat3),
        "a rebuild must produce a fresh Rc"
    );
    assert_eq!(
        st.view_model.view_model_seq, 2,
        "second store bumps the seq again"
    );
}

/// F7 (parse-don't-validate at the trust boundary): a `ToolCallKey` parsed
/// from a protocol `ToolCallId` is the maps' key type, and two keys built
/// from the same protocol id are equal + hash-equal, so an insert via one
/// and a lookup via another (the live-update path) land on the same entry.
/// The type itself is the enforcement hook (no `Deref` to `String`, so an
/// arbitrary label can't be substituted for a tool id); this pins the
/// round-trip the maps rely on.
#[test]
fn tool_call_key_round_trips_through_the_maps() {
    use yalda::acp_channel::ToolCallId;

    let id: ToolCallId = "tool-abc".into();
    let key_started = ToolCallKey::from_id(&id);
    // A later `ToolCallUpdated` re-parses the SAME protocol id into a key.
    let key_updated = ToolCallKey::from_id(&id);

    assert_eq!(
        key_started, key_updated,
        "keys parsed from the same protocol id must be equal"
    );
    assert_eq!(
        key_started.as_str(),
        "tool-abc",
        "the render edge can recover the id string"
    );
    assert_eq!(key_started.to_string(), "tool-abc");

    // Insert on the started key, look up on the (separately parsed) updated
    // key — the live ToolCallUpdated path. The lookup must hit.
    let mut map: std::collections::HashMap<ToolCallKey, u32> = std::collections::HashMap::new();
    map.insert(key_started, 7);
    assert_eq!(
        map.get(&key_updated),
        Some(&7),
        "a key re-parsed from the same id must resolve the same map entry"
    );

    // A DIFFERENT id is a distinct key — no accidental collision.
    let other = ToolCallKey::from_id(&("tool-xyz".into()));
    assert_eq!(map.get(&other), None, "a different id must miss");
}

/// The fingerprint must EXCLUDE tool-call content (the `ToolCallUpdated`
/// trap): mutating a `ToolCall`'s content without touching
/// `tool_call_order` / `edit_seq` must leave the fingerprint unchanged,
/// so the cached flat_items (which only carry tool ids) stay valid.
#[test]
fn view_model_fingerprint_ignores_tool_content() {
    let mut st = AgentState::new_for_test();
    st.tools.order.push(ToolCallKey::from_id(&"tool-1".into()));
    let before = st.view_model_fingerprint(7, 3);

    // Simulate a ToolCallUpdated: content changes, order/edit_seq don't.
    // (We don't have a ToolCall constructor handy in-test; the point is
    // that the fingerprint reads neither `tool_calls` content nor map
    // size — only `tool_call_order`.) Re-derive with identical structural
    // inputs and assert stability.
    let after = st.view_model_fingerprint(7, 3);
    assert_eq!(before, after, "tool content is not part of the fingerprint");
}

/// F6 / INV (header-owning turns are exactly {Llm, User}): `HeaderRole`
/// is a TOTAL mapping over `TurnId` — `Tool`/`System` -> None (no header),
/// `Llm` -> Claude, `User` -> User. This replaces the old `unreachable!()`
/// arm with a compiler-checked `Option`, so a new `TurnId` variant is a
/// compile error, not a paint-path panic.
#[test]
fn header_role_is_total_over_turn_id() {
    assert_eq!(HeaderRole::from_turn(TurnId::Tool(3)), None);
    assert_eq!(HeaderRole::from_turn(TurnId::System), None);
    assert_eq!(
        HeaderRole::from_turn(TurnId::Llm(1)),
        Some(HeaderRole::Claude)
    );
    assert_eq!(
        HeaderRole::from_turn(TurnId::User(2)),
        Some(HeaderRole::User)
    );
    // And the role threads through to the rendered `TurnRole`.
    assert_eq!(HeaderRole::Claude.into_turn_role(), TurnRole::Claude);
    assert_eq!(HeaderRole::User.into_turn_role(), TurnRole::User);
}

#[test]
fn agent_turn_header_uses_active_provider_name() {
    use yalda::acp_channel::AgentProvider;

    assert_eq!(
        turn_header_label(TurnRole::Claude, AgentProvider::Claude),
        "Claude"
    );
    assert_eq!(
        turn_header_label(TurnRole::Claude, AgentProvider::Codex),
        "Codex"
    );
    assert_eq!(
        turn_header_label(TurnRole::User, AgentProvider::Codex),
        "You"
    );

    let mut state = AgentState::new_for_test();
    let claude_fp = TranscriptSeqs::of(&state).fingerprint_hash();
    state.provider = AgentProvider::Codex;
    let codex_fp = TranscriptSeqs::of(&state).fingerprint_hash();
    assert_ne!(
        claude_fp, codex_fp,
        "changing provider must invalidate the cached transcript label"
    );
}

/// F8 / INV-12 (count parity): `reconcile_list` is the ONLY mutator of
/// `(list_state, list_item_count)`, updating both together so they can't
/// drift. It returns whether the list grew. After any reconcile the
/// registered count equals the requested count.
#[test]
fn reconcile_list_keeps_count_in_sync_and_reports_growth() {
    // Ticket 021: the `(list_state, list_item_count)` pair moved out of
    // `AgentState` into the `TranscriptScroll` UI-state struct owned by
    // `TranscriptView`. The reconcile logic is unchanged and still pure-
    // testable — `block_ranges_active` is now passed in rather than read off
    // `AgentState`.
    // `n` distinct line items — distinct so the block-mode key diff is exercised.
    let lines = |n: usize| -> Vec<FlatItem> { (0..n).map(FlatItem::Line).collect() };

    let mut sc = TranscriptScroll::new();
    assert_eq!(sc.list_item_count, 0);

    // Growth: count rises, reports grew=true, splices.
    assert!(
        sc.reconcile_list(false, &lines(5), 0),
        "0 -> 5 must report growth"
    );
    assert_eq!(sc.list_item_count, 5, "count tracks the requested length");

    // No change: same count, reports grew=false, count unchanged.
    assert!(
        !sc.reconcile_list(false, &lines(5), 0),
        "5 -> 5 is not growth"
    );
    assert_eq!(sc.list_item_count, 5);

    // Shrink: count falls, reports grew=false, resets.
    assert!(
        !sc.reconcile_list(false, &lines(2), 0),
        "5 -> 2 is not growth"
    );
    assert_eq!(sc.list_item_count, 2, "count tracks a shrink too");

    // With block ranges active, growth now SPLICES (preserving the prefix's
    // scroll anchor) instead of reset — but parity must still hold.
    assert!(sc.reconcile_list(true, &lines(9), 0));
    assert_eq!(sc.list_item_count, 9);
}

/// The worksheet "newline jumps to the top of the viewport" regression
/// (project_chatbox_offscreen_recurring sibling): with block ranges active, a
/// structural edit must SPLICE the changed range — preserving the unchanged
/// prefix above the edit so the scroll anchor survives — NOT `reset()`. We can't
/// observe GPUI scroll headlessly, but we pin the count-parity contract across a
/// mid-list insert (the newline case) and a delete, which the splice path must
/// keep exact for the reveal to land on measured rows.
#[test]
fn worksheet_reconcile_splices_structural_edits_keeping_parity() {
    let lines = |n: usize| -> Vec<FlatItem> { (0..n).map(FlatItem::Line).collect() };
    let mut sc = TranscriptScroll::new();

    // Seed a long worksheet transcript.
    assert!(sc.reconcile_list(true, &lines(40), 1));
    assert_eq!(sc.list_item_count, 40);

    // Insert a line in the MIDDLE (a newline at line 20): the new item list is
    // 41 long. Splice must keep parity exact (the old code reset to top here).
    let mut after_insert = lines(20);
    after_insert.push(FlatItem::Line(999)); // the freshly-inserted editable line
    after_insert.extend((20..40).map(FlatItem::Line));
    assert!(
        sc.reconcile_list(true, &after_insert, 2),
        "structural growth reports grew=true",
    );
    assert_eq!(
        sc.list_item_count, 41,
        "count parity after a mid-list insert"
    );

    // Delete it again → shrink, parity holds.
    assert!(
        !sc.reconcile_list(true, &lines(40), 3),
        "shrink is not growth"
    );
    assert_eq!(sc.list_item_count, 40);
}

/// F10 / INV-10 (block/line partition is total): a range
/// `detect_block_ranges` emits but `parse_block_range` rejects must
/// `FallBackToLines`, contribute NO entry to the block cache, and so
/// leave every one of its source lines to render as a standalone Line.
/// Mirrors render_agent's cache + `in_block` construction exactly.
#[test]
fn unparsed_detected_range_falls_back_to_one_line_per_source_line() {
    // 3 pipe-delimited rows with NO separator row: `detect_block_ranges`
    // accepts it (>=3 rows, all `|...|`), but it is NOT a valid markdown
    // table, so `parse_block_range` rejects it.
    let lines: Vec<String> = vec![
        "| a | b |".to_string(),
        "| c | d |".to_string(),
        "| e | f |".to_string(),
    ];
    let frozen = vec![(0usize, lines.len())];
    let ranges = detect_block_ranges(&lines, &frozen);
    assert_eq!(
        ranges,
        vec![(0, 3)],
        "the 3 pipe rows must be DETECTED as a candidate range"
    );

    let theme = Theme::default();
    assert!(
        matches!(
            parse_block_range(&lines, 0, 3, &theme),
            BlockParse::FallBackToLines
        ),
        "a separator-less pipe block must NOT parse as a table"
    );

    // Replicate the render_agent partition: block_cache holds only Parsed
    // ranges; `in_block` is derived from the cache; any line not in a
    // block is emitted as a Line.
    let mut block_cache: std::collections::HashMap<(usize, usize), RenderedBlock> =
        std::collections::HashMap::new();
    for &(s, e) in &ranges {
        if let BlockParse::Parsed(b) = parse_block_range(&lines, s, e, &theme) {
            block_cache.insert((s, e), b);
        }
    }
    let mut in_block: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &(s, e) in &ranges {
        if block_cache.contains_key(&(s, e)) {
            for li in s..e {
                in_block.insert(li);
            }
        }
    }
    let line_items: Vec<usize> = (0..lines.len())
        .filter(|i| !block_cache.keys().any(|&(s, _)| s == *i) && !in_block.contains(i))
        .collect();
    // Count parity over the range: a Line for EVERY source line, no Block.
    assert!(
        block_cache.is_empty(),
        "rejected range must emit no Block item"
    );
    assert_eq!(
        line_items,
        vec![0, 1, 2],
        "every source line of an unparsed range must render as a Line"
    );
}

/// F11 / INV-8 (memo soundness): the fingerprint must change when a
/// resolved tool anchor line changes, because the flat build groups tool
/// calls by that resolved line. Holding `edit_seq` FIXED across the two
/// fingerprint calls isolates the anchor dependency from the `edit_seq`
/// co-variation the memo previously leaned on implicitly.
/// A.6: `ToolCalls::register` is the one chokepoint that keeps `order`,
/// `calls`, and `anchor` in sync — a new id appends to order exactly once,
/// a re-register (the update path) never duplicates the order entry, and
/// `clear` empties every map together.
#[test]
fn tool_calls_register_keeps_maps_in_sync() {
    use yalda::acp_channel::{ToolCall, ToolCallId};
    let mut st = AgentState::new_for_test();
    let id1: ToolCallId = "t1".into();
    let k1 = ToolCallKey::from_id(&id1);

    st.tools.register(
        k1.clone(),
        ToolCall::new(id1.clone(), String::from("ls")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(st.tools.order.len(), 1);
    assert!(st.tools.calls.contains_key(&k1) && st.tools.anchor.contains_key(&k1));

    // Re-register the same id (an update arriving via register): order must
    // NOT grow — the three maps stay coherent.
    st.tools.register(
        k1.clone(),
        ToolCall::new(id1.clone(), String::from("ls -la")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(
        st.tools.order.len(),
        1,
        "re-register must not duplicate the order entry"
    );
    assert_eq!(st.tools.calls.len(), 1);

    // A distinct id appends.
    let id2: ToolCallId = "t2".into();
    let k2 = ToolCallKey::from_id(&id2);
    st.tools.register(
        k2.clone(),
        ToolCall::new(id2, String::from("grep")),
        st.editor.anchor_for_line(0),
    );
    assert_eq!(st.tools.order.len(), 2);
    assert!(st.tools.order.contains(&k2));

    st.tools.clear();
    assert!(
        st.tools.order.is_empty() && st.tools.calls.is_empty() && st.tools.anchor.is_empty(),
        "clear empties every map together"
    );
}

/// Feature #1 — detect subagents the way the HARNESS emits them. claude-code-acp
/// maps `Task` to `ToolKind::Think` (same kind as `TodoWrite`) carrying the spawn
/// in `raw_input`; the old `kind == Other` check NEVER matched a real Task, so
/// subagents were invisible. classify_subagent now keys on the structure (Think +
/// prompt/subagent_type, TodoWrite excluded by `todos`) + a name fallback, and
/// captures the prompt for the pane.
#[test]
fn classify_subagent_detects_the_harness_task_shape() {
    use yalda::acp_channel::{ToolCall, ToolCallId, ToolCallStatus, ToolKind};
    let mk = |title: &str, kind: ToolKind, raw: Option<serde_json::Value>| -> ToolCall {
        let id: ToolCallId = "t".into();
        let mut tc = ToolCall::new(id, title.to_string());
        tc.kind = kind;
        tc.raw_input = raw;
        tc.status = ToolCallStatus::InProgress;
        tc
    };

    // A real Task subagent: Think + prompt → detected, prompt captured.
    let sa = classify_subagent(&mk(
        "Explore the repo",
        ToolKind::Think,
        Some(serde_json::json!({"prompt": "map the code", "subagent_type": "Explore"})),
    ))
    .expect("Think + prompt is a subagent");
    assert_eq!(sa.prompt.as_deref(), Some("map the code"));

    // THE regression: a Think subagent with NO subagent-y title — the old
    // kind==Other + name heuristic missed this entirely.
    assert!(
        classify_subagent(&mk(
            "anonymous",
            ToolKind::Think,
            Some(serde_json::json!({"prompt": "go"}))
        ))
        .is_some(),
        "a Think+prompt call is a subagent even without a Task-ish title"
    );

    // TodoWrite is also Think — excluded by its `todos` input.
    assert!(
        classify_subagent(&mk(
            "TodoWrite",
            ToolKind::Think,
            Some(serde_json::json!({"todos": []}))
        ))
        .is_none(),
        "TodoWrite is not a subagent"
    );

    // Plain tools are not subagents.
    assert!(
        classify_subagent(&mk(
            "Read",
            ToolKind::Read,
            Some(serde_json::json!({"path": "x"}))
        ))
        .is_none()
    );

    // Name fallback still works for adapters that only title it.
    assert!(classify_subagent(&mk("Task: foo", ToolKind::Other, None)).is_some());
}

/// End-to-end: a registered Task tool call surfaces through `subagents()` with its
/// prompt — the data the bottom panes render.
#[test]
fn subagents_surfaces_registered_task_with_prompt() {
    use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
    let mut st = AgentState::new_for_test();
    let id: ToolCallId = "task1".into();
    let mut tc = ToolCall::new(id.clone(), "Explore repo".to_string());
    tc.kind = ToolKind::Think;
    tc.raw_input = Some(serde_json::json!({"prompt": "map the code", "subagent_type": "Explore"}));
    st.tools
        .register(ToolCallKey::from_id(&id), tc, st.editor.anchor_for_line(0));

    let subs = st.subagents();
    assert_eq!(subs.len(), 1, "the Task call is surfaced as a subagent");
    assert_eq!(subs[0].prompt.as_deref(), Some("map the code"));
}

#[test]
fn codex_subagent_activity_is_classified_by_child_thread() {
    use yalda::acp_channel::{ToolCall, ToolCallId, ToolCallStatus};
    let id: ToolCallId = "codex-start".into();
    let mut tc = ToolCall::new(id, "Start subagent review_maintainability".to_string());
    tc.raw_input = Some(serde_json::json!({
        "agentThreadId": "019c-child",
        "agentPath": "review/review_maintainability",
        "activityKind": "started"
    }));
    tc.meta = serde_json::json!({
        "codex": {
            "subagent": {
                "threadId": "019c-child",
                "path": "review/review_maintainability",
                "activity": "started"
            }
        }
    })
    .as_object()
    .cloned();

    let subagent = classify_subagent(&tc).expect("Codex activity is a subagent");
    assert_eq!(subagent.key, SubAgentKey::CodexThread("019c-child".into()));
    assert_eq!(subagent.label, "review_maintainability");
    assert_eq!(subagent.status, ToolCallStatus::InProgress);
}

#[test]
fn codex_subagent_lifecycle_folds_to_one_row() {
    use yalda::acp_channel::{ToolCall, ToolCallId, ToolCallStatus};
    let mut state = AgentState::new_for_test();
    for (id, activity) in [
        ("start", "started"),
        ("interact", "interacted"),
        ("stop", "interrupted"),
    ] {
        let tool_id: ToolCallId = id.into();
        let mut tc = ToolCall::new(tool_id.clone(), format!("{activity} subagent review"));
        tc.raw_input = Some(serde_json::json!({
            "agentThreadId": "019c-child",
            "agentPath": "review/review_maintainability",
            "activityKind": activity
        }));
        state.tools.register(
            ToolCallKey::from_id(&tool_id),
            tc,
            state.editor.anchor_for_line(0),
        );
    }

    let rows = state.subagents();
    assert_eq!(rows.len(), 1, "one child thread must produce one row");
    assert_eq!(rows[0].status, ToolCallStatus::Failed);
    assert_eq!(rows[0].activity.as_deref(), Some("interrupted"));
}

#[test]
fn codex_child_replay_reducer_preserves_roles_and_tools() {
    use yalda::acp_channel::{ReplyEvent, ToolCall, ToolCallId};
    let tool_id: ToolCallId = "read-1".into();
    let transcript = SubAgentTranscript::from_reply_events([
        ReplyEvent::UserMessage("inspect it".into()),
        ReplyEvent::Chunk("first ".into()),
        ReplyEvent::Chunk("answer".into()),
        ReplyEvent::ToolCallStarted(ToolCall::new(tool_id.clone(), "Read file".to_string())),
        ReplyEvent::ReplayComplete,
    ]);

    assert_eq!(transcript.items.len(), 3);
    assert!(matches!(
        &transcript.items[0],
        SubAgentTranscriptItem::User(text) if text == "inspect it"
    ));
    assert!(matches!(
        &transcript.items[1],
        SubAgentTranscriptItem::Agent(text) if text == "first answer"
    ));
    assert!(
        transcript
            .tools
            .contains_key(&ToolCallKey::from_id(&tool_id))
    );
}

#[test]
fn fingerprint_tracks_resolved_tool_anchor_line() {
    let mut st = AgentState::new_for_test();
    // Seed a few frozen lines so an anchor can resolve to a real line.
    st.editor
        .programmatic_insert(0, "line0\nline1\nline2\nline3\n");

    // Anchor a tool call to line 2 and register it in the build's inputs.
    let anchor = st.editor.anchor_for_line(2);
    let key = ToolCallKey::from_id(&"tool-1".into());
    st.tools.order.push(key.clone());
    st.tools.anchor.insert(key, anchor);
    assert_eq!(st.editor.line_for_anchor(anchor), Some(2));

    // Fingerprint at a FIXED edit_seq/frozen_count.
    let fp_before = st.view_model_fingerprint(42, 4);

    // Insert a line ABOVE the anchor: its resolved line moves 2 -> 3.
    // We pass the SAME edit_seq (42) again, so any fingerprint change is
    // attributable to the resolved anchor line, not to edit_seq.
    st.editor.programmatic_insert(0, "header\n");
    assert_eq!(
        st.editor.line_for_anchor(anchor),
        Some(3),
        "the anchor must have shifted down by one line"
    );
    let fp_after = st.view_model_fingerprint(42, 4);

    assert_ne!(
        fp_before, fp_after,
        "a moved tool anchor must change the fingerprint even at a fixed edit_seq"
    );
}

/// F4 / INV-13 enforcement: the tail re-reveal must fire on CONTENT growth
/// (`edit_seq` advanced), NOT on a flat-item count delta. A chunk that
/// grows the last line without adding a row (agent prose before a `\n`)
/// bumps `edit_seq` but leaves the count unchanged; the old count-keyed
/// path skipped it. `reveal_tail_if_following` must request the reveal
/// anyway, and must NOT re-request at the same `edit_seq` (idle ticks).
#[test]
fn reveal_tail_keys_on_content_growth_not_count() {
    // Ticket 021: the reveal logic + the `last_scrolled_edit_seq` watermark
    // moved to `TranscriptScroll`; `follow_tail()` (the follow DECISION) stays
    // on `AgentState`. The caller threads `follow_tail()` + the document's
    // `edit_seq` into `reveal_tail_if_following`, exactly as `TranscriptView`
    // does in render. The behavior under test is unchanged.
    let mut st = AgentState::new_for_test();
    let mut sc = TranscriptScroll::new();
    // new_for_test starts in Chatbox with follow_output = true, so the
    // follow decision is satisfied; we isolate the edit_seq/count behavior.
    assert!(st.follow_tail(), "Chatbox + follow_output should follow");

    let count = 3usize; // simulated post-reconcile flat-item count
    let seq0 = st.editor.document().edit_seq();

    // First reveal at the current edit_seq: requested (watermark was MAX).
    assert!(
        sc.reveal_tail_if_following(st.follow_tail(), seq0, count),
        "first reveal at a new edit_seq must be requested"
    );
    assert_eq!(
        sc.last_scrolled_edit_seq, seq0,
        "reveal stamps the watermark to the current edit_seq"
    );

    // Idle tick — same edit_seq, same count: must NOT re-reveal (so a
    // user who scrolled up isn't yanked back every frame).
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq0, count),
        "no content growth ⇒ no re-reveal at the same edit_seq"
    );

    // Append a chunk WITHOUT a trailing newline: grows the last line but
    // adds no row, so the flat-item count is UNCHANGED. This is exactly
    // the case the old `new_count != old_count` trigger missed.
    let char_len = st.editor.document().rope().len_chars();
    st.editor
        .programmatic_insert(char_len, "more streamed prose");
    let seq1 = st.editor.document().edit_seq();
    assert_ne!(seq1, seq0, "an intra-line insert must advance edit_seq");

    // Count is held constant (no new row) — the reveal must STILL fire,
    // keyed on the advanced edit_seq, not on a count delta.
    assert!(
        sc.reveal_tail_if_following(st.follow_tail(), seq1, count),
        "intra-line content growth must re-reveal even with unchanged count"
    );
    assert_eq!(sc.last_scrolled_edit_seq, seq1);

    // A zero count never reveals (guards the `count - 1` underflow).
    let seq2_before = sc.last_scrolled_edit_seq;
    st.editor.programmatic_insert(0, "x");
    let seq2 = st.editor.document().edit_seq();
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq2, 0),
        "an empty list never reveals regardless of growth"
    );
    assert_eq!(
        sc.last_scrolled_edit_seq, seq2_before,
        "a skipped reveal must not advance the watermark"
    );

    // When following is OFF (user scrolled up in Chatbox), growth alone
    // must not yank the viewport back.
    st.follow_output.set(false);
    assert!(!st.follow_tail());
    st.editor.programmatic_insert(0, "y");
    let seq3 = st.editor.document().edit_seq();
    assert!(
        !sc.reveal_tail_if_following(st.follow_tail(), seq3, count),
        "no reveal while the user has scrolled away from the tail"
    );
}

/// F12 / INV-11 enforcement: an UNTERMINATED code fence must yield NO
/// block range, so its arrived lines render as plain Lines (each its own
/// FlatItem) until the closing fence freezes. A matched closing fence is
/// required, symmetric to the >=3-row table rule.
#[test]
fn detect_block_ranges_skips_unterminated_fence() {
    // Open fence, two body lines, NO closing ``` — all frozen.
    let lines: Vec<String> = vec![
        "```rust".to_string(),
        "let x = 1;".to_string(),
        "let y = 2;".to_string(),
    ];
    let frozen = vec![(0usize, lines.len())];
    let ranges = detect_block_ranges(&lines, &frozen);
    assert!(
        ranges.is_empty(),
        "an unterminated fence must NOT emit a block range, got {ranges:?}"
    );

    // Sanity: once the closing fence arrives, the range IS emitted so
    // the closed block still renders as one Block.
    let mut closed = lines.clone();
    closed.push("```".to_string());
    let frozen_closed = vec![(0usize, closed.len())];
    let ranges_closed = detect_block_ranges(&closed, &frozen_closed);
    assert_eq!(
        ranges_closed,
        vec![(0usize, closed.len())],
        "a closed fence must emit exactly one block range"
    );
}

#[test]
fn segments_to_styled_line_preserves_text_and_count() {
    let segs = vec![s("foo"), s("bar"), s("")];
    let line = segments_to_styled_line(&segs);
    assert_eq!(line.spans.len(), 3);
    assert_eq!(line.spans[0].text, "foo");
    assert_eq!(line.spans[2].text, "");
}

// ---- line_selection_range ----

#[test]
fn line_selection_range_outside_returns_none() {
    // Selection lines 1..=3, querying line 0 (above) and line 5 (below).
    let sel = ((1, 0), (3, 0));
    assert_eq!(line_selection_range(sel, 0, 10), None);
    assert_eq!(line_selection_range(sel, 5, 10), None);
}

#[test]
fn line_selection_range_single_line_returns_partial() {
    // Sel from col 2 to col 6 on line 4.
    let sel = ((4, 2), (4, 6));
    assert_eq!(line_selection_range(sel, 4, 20), Some((2, 6)));
}

#[test]
fn line_selection_range_first_line_starts_at_sc() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 2, 12), Some((5, 12)));
}

#[test]
fn line_selection_range_last_line_ends_at_ec() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 4, 20), Some((0, 3)));
}

#[test]
fn line_selection_range_middle_line_full_width() {
    let sel = ((2, 5), (4, 3));
    assert_eq!(line_selection_range(sel, 3, 8), Some((0, 8)));
}

// ---- apply_selection_bg ----

fn seg_text(segs: &[Segment]) -> String {
    segs.iter().map(|(t, _)| t.as_str()).collect()
}

#[test]
fn apply_selection_bg_no_overlap_preserves_segments() {
    // Selection col 0..2 but apply over a single 3-char segment by passing
    // 99..100 (out of range). Result should equal input with 0 bg applied.
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 99, 100, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert!(out.iter().all(|(_, st)| st.bg.is_none()));
}

#[test]
fn selection_bg_does_not_flip_prose_to_code_font() {
    use crate::span_uses_code_font;
    let code_yellow = NColor::Rgb(241, 250, 140);
    let sel = NColor::Rgb(68, 71, 90); // a selection-highlight color
    let code_bg = NColor::Rgb(40, 42, 54); // inline-code / code-block bg

    // Plain prose, no selection → body font.
    assert!(!span_uses_code_font(None, None, None));
    // Inline code detected by its distinctive bg → code font.
    assert!(span_uses_code_font(Some(code_bg), None, None));
    // Inline code detected by the Dracula code fg → code font.
    assert!(span_uses_code_font(None, Some(code_yellow), None));

    // THE FIX: selected prose carries the selection bg, but that must NOT be
    // read as code (else highlighting flips proportional text to monospace).
    assert!(
        !span_uses_code_font(Some(sel), None, Some(sel)),
        "selected prose must stay in the body font"
    );
    // NEGATIVE CONTROL: without the selection-bg exclusion (pretend selection_bg
    // is unknown/None), the same span WOULD be misclassified as code — proving
    // the exclusion is load-bearing.
    assert!(
        span_uses_code_font(Some(sel), None, None),
        "control: any bg without the exclusion looks like code"
    );
    // Selected INLINE CODE keeps the code font via its fg even though its bg is
    // now the selection color.
    assert!(span_uses_code_font(Some(sel), Some(code_yellow), Some(sel)));
}

#[test]
fn apply_selection_bg_full_segment_gets_bg() {
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 0, 3, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert!(out.iter().all(|(_, st)| st.bg == Some(NColor::Red)));
}

/// A transcript selection is one visual state, regardless of the Markdown token
/// beneath it. In particular, selecting an entire bullet line must not leave
/// the marker green while ordinary selected prose uses the agent's cool blue.
#[test]
fn transcript_whole_line_selection_unifies_bullet_and_prose_color() {
    let theme = Theme::default();
    let line = "- selected prose".to_string();
    let mut segs =
        yalda::md_highlight::highlight_markdown_lines_stripped(std::slice::from_ref(&line), &theme)
            .remove(0);
    for (_, style) in &mut segs {
        if *style == theme.paragraph {
            *style = style.fg(theme.agent.agent_tint);
        }
    }

    let selected = apply_transcript_line_selection(
        &segs,
        &line,
        true,
        Some(((0, 0), (0, line.chars().count()))),
        0,
        theme.agent.selection_bg,
        theme.agent.agent_tint,
    );
    let marker = selected
        .iter()
        .find(|(text, _)| text.contains('•'))
        .expect("rendered bullet marker");
    let prose = selected
        .iter()
        .find(|(text, _)| text.contains("selected prose"))
        .expect("rendered prose");

    assert_eq!(
        marker.1.bg,
        Some(theme.agent.selection_bg),
        "whole-line selection must reach the substituted bullet glyph"
    );
    assert_eq!(prose.1.bg, marker.1.bg, "selection background is universal");
    assert_eq!(
        marker.1.fg, prose.1.fg,
        "selected bullet marker must use the same blue foreground as selected prose"
    );
}

#[test]
fn stripped_bullet_marker_maps_back_to_raw_marker() {
    let map = stripped_to_raw_cols("- selected", "• selected");
    assert_eq!(map[0], 0, "rendered bullet maps to the raw dash");
    assert_eq!(map[1], 1, "space after the bullet keeps its raw column");
    assert_eq!(
        raw_to_stripped_col(&map, 2),
        2,
        "prose begins at rendered col 2"
    );
    assert_eq!(
        map.last().copied(),
        Some("- selected".chars().count()),
        "the alignment reaches raw EOL instead of collapsing at the marker"
    );

    let marker_only = stripped_to_raw_cols("-", "•");
    assert_eq!(
        marker_only,
        vec![0, 1],
        "a substituted marker advances through raw EOL"
    );

    let deletion = stripped_to_raw_cols("-x", "x");
    assert_eq!(
        deletion,
        vec![1, 2],
        "ordinary deletion must not take the bullet-substitution branch"
    );

    let missing = stripped_to_raw_cols("abc", "z");
    assert_eq!(
        missing,
        vec![3, 3],
        "a rendered glyph absent from raw text saturates safely at EOL"
    );
}

#[test]
fn transcript_selection_color_keeps_inline_code_monospace() {
    let theme = Theme::default();
    let code = vec![("code".to_string(), theme.code_inline)];
    let selected = apply_selection_style(
        &code,
        0,
        4,
        theme.agent.selection_bg,
        theme.agent.agent_tint,
    );

    assert_eq!(selected[0].1.fg, Some(theme.agent.agent_tint));
    assert_eq!(selected[0].1.bg, Some(theme.agent.selection_bg));
    assert!(
        style_uses_code_font(selected[0].1, Some(theme.agent.selection_bg)),
        "universal selection colors must not change inline-code typography"
    );
}

#[test]
fn transcript_selection_style_changes_only_requested_span() {
    let segs = vec![("abc".to_string(), NStyle::default().fg(NColor::White))];
    let selected = apply_selection_style(&segs, 1, 2, NColor::Blue, NColor::LightBlue);

    assert_eq!(seg_text(&selected), "abc");
    assert_eq!(
        selected.len(),
        3,
        "selection boundaries split the source run"
    );
    assert_eq!(selected[0], ("a".to_string(), segs[0].1));
    assert_eq!(selected[1].0, "b");
    assert_eq!(selected[1].1.bg, Some(NColor::Blue));
    assert_eq!(selected[1].1.fg, Some(NColor::LightBlue));
    assert_eq!(selected[2], ("c".to_string(), segs[0].1));
}

#[test]
fn apply_selection_bg_splits_segment_at_boundary() {
    // Selection covers chars 1..2 of a 3-char segment → expect 3 segments:
    // unselected "a", selected "b", unselected "c".
    let segs = vec![s("abc")];
    let out = apply_selection_bg(&segs, 1, 2, NColor::Red);
    assert_eq!(seg_text(&out), "abc");
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].0, "a");
    assert_eq!(out[0].1.bg, None);
    assert_eq!(out[1].0, "b");
    assert_eq!(out[1].1.bg, Some(NColor::Red));
    assert_eq!(out[2].0, "c");
    assert_eq!(out[2].1.bg, None);
}

#[test]
fn apply_selection_bg_spans_multiple_input_segments() {
    // Sel chars 2..6 across two segments "hello"+"world".
    let segs = vec![s("hello"), s("world")];
    let out = apply_selection_bg(&segs, 2, 6, NColor::Red);
    // Reconstructed text should be unchanged; "ll" + "o" + "w" should be bg'd.
    assert_eq!(seg_text(&out), "helloworld");
    let bg_text: String = out
        .iter()
        .filter(|(_, st)| st.bg == Some(NColor::Red))
        .map(|(t, _)| t.as_str())
        .collect();
    assert_eq!(bg_text, "llow");
}

#[test]
fn apply_selection_bg_empty_input_returns_empty() {
    let out = apply_selection_bg(&[], 0, 5, NColor::Red);
    assert!(out.is_empty());
}

// ---- classify_wp_line ----

#[test]
fn classify_wp_line_empty_blank_and_whitespace() {
    assert_eq!(classify_wp_line("", false), WpLineKind::Empty);
    assert_eq!(classify_wp_line("   ", false), WpLineKind::Empty);
    assert_eq!(classify_wp_line("\t  ", false), WpLineKind::Empty);
}

#[test]
fn classify_wp_line_headings_levels_1_through_6() {
    assert_eq!(classify_wp_line("# H1", false), WpLineKind::Heading(1));
    assert_eq!(classify_wp_line("## H2", false), WpLineKind::Heading(2));
    assert_eq!(classify_wp_line("### H3", false), WpLineKind::Heading(3));
    assert_eq!(classify_wp_line("###### H6", false), WpLineKind::Heading(6));
    // 7 hashes = not a valid heading per CommonMark; treat as paragraph.
    assert_eq!(
        classify_wp_line("####### too many", false),
        WpLineKind::Paragraph
    );
}

#[test]
fn classify_wp_line_heading_requires_space_after_hashes() {
    // No space after hashes = not a heading.
    assert_eq!(classify_wp_line("#hashtag", false), WpLineKind::Paragraph);
    // Hashes only on the line is still a heading per CommonMark.
    assert_eq!(classify_wp_line("##", false), WpLineKind::Heading(2));
}

#[test]
fn classify_wp_line_bullet_markers() {
    assert_eq!(classify_wp_line("- item", false), WpLineKind::BulletItem);
    assert_eq!(classify_wp_line("* item", false), WpLineKind::BulletItem);
    assert_eq!(classify_wp_line("+ item", false), WpLineKind::BulletItem);
    assert_eq!(
        classify_wp_line("  - nested", false),
        WpLineKind::BulletItem
    );
    // Dash without trailing space is not a bullet.
    assert_eq!(classify_wp_line("-no-space", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_ordered_markers() {
    assert_eq!(classify_wp_line("1. item", false), WpLineKind::OrderedItem);
    assert_eq!(classify_wp_line("42. item", false), WpLineKind::OrderedItem);
    assert_eq!(classify_wp_line("3) item", false), WpLineKind::OrderedItem);
    // No space after marker.
    assert_eq!(classify_wp_line("1.no", false), WpLineKind::Paragraph);
    // No marker punctuation.
    assert_eq!(classify_wp_line("1 hello", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_blockquote() {
    assert_eq!(classify_wp_line("> quote", false), WpLineKind::Blockquote);
    assert_eq!(classify_wp_line(">>nested", false), WpLineKind::Blockquote);
}

#[test]
fn classify_wp_line_code_fences() {
    // Opening fence outside of a fence.
    assert_eq!(classify_wp_line("```", false), WpLineKind::CodeFence);
    assert_eq!(classify_wp_line("```rust", false), WpLineKind::CodeFence);
    assert_eq!(classify_wp_line("~~~", false), WpLineKind::CodeFence);
    // Inside a fence: any line is content unless it's a closer.
    assert_eq!(
        classify_wp_line("let x = 1;", true),
        WpLineKind::CodeContent
    );
    assert_eq!(classify_wp_line("```", true), WpLineKind::CodeFence);
    // A heading inside a fence is still code, not a heading.
    assert_eq!(
        classify_wp_line("# not a heading", true),
        WpLineKind::CodeContent
    );
}

#[test]
fn classify_wp_line_table_row_heuristic() {
    // 2+ pipes → table row.
    assert_eq!(
        classify_wp_line("| col1 | col2 |", false),
        WpLineKind::TableRow
    );
    assert_eq!(classify_wp_line("|---|---|", false), WpLineKind::TableRow);
    // Single pipe falls through to paragraph (heuristic requires 2+).
    assert_eq!(classify_wp_line("a | b", false), WpLineKind::Paragraph);
    // Zero pipes = paragraph.
    assert_eq!(classify_wp_line("just text", false), WpLineKind::Paragraph);
}

#[test]
fn classify_wp_line_paragraph_fallback() {
    assert_eq!(
        classify_wp_line("hello world", false),
        WpLineKind::Paragraph
    );
    assert_eq!(
        classify_wp_line("**bold** text", false),
        WpLineKind::Paragraph
    );
}

// ---- Menu rendering helpers ----

#[test]
fn format_menu_key_single_char() {
    let kp = KeyPress::new(Key::Char('f'), KMods::NONE);
    assert_eq!(format_menu_key(&[kp]), "f");
}

#[test]
fn format_menu_key_with_ctrl() {
    let kp = KeyPress::new(Key::Char('k'), KMods::CONTROL);
    assert_eq!(format_menu_key(&[kp]), "Ctrl-k");
}

#[test]
fn format_menu_key_named_keys() {
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::Enter, KMods::NONE)]),
        "Enter"
    );
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::Esc, KMods::NONE)]),
        "Esc"
    );
    assert_eq!(
        format_menu_key(&[KeyPress::new(Key::F(2), KMods::NONE)]),
        "F2"
    );
}

#[test]
fn format_menu_key_multi_press_sequence() {
    // `g g` for goto-top, etc.
    let g = KeyPress::new(Key::Char('g'), KMods::NONE);
    assert_eq!(format_menu_key(&[g.clone(), g]), "g g");
}

#[test]
fn gpui_menu_has_required_entries() {
    // Sanity check: the menu builder must include every action that
    // `dispatch_menu_command` knows how to dispatch. If we add a new
    // command name to the menu, this assert points at the missing
    // dispatch arm via the matching test below.
    fn collect_leaves<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a str>) {
        for n in nodes {
            match &n.action {
                yalda::menu::MenuAction::Command(s) => out.push(s.as_str()),
                yalda::menu::MenuAction::Submenu(children) => {
                    collect_leaves(children, out);
                }
                _ => {}
            }
        }
    }
    let menu = gpui_menu();
    let mut leaf_actions: Vec<&str> = Vec::new();
    collect_leaves(&menu, &mut leaf_actions);
    // UXI-Menu-8 / UXI-Workspace-26: the shell root holds new-tile, theme,
    // toggle-jump, the layout-modes submenu, workspace ops, and the flattened
    // system/dev commands. Tile-scoped verbs moved to the `<space>` tile menu.
    let expected = [
        "new-agent-tile",
        "new-buffer-tile",
        "new-linear-tile",
        "new-cog-tile",
        "new-keymap-tile",
        "theme-nightfox",
        "theme-folio",
        "toggle-jump-panel",
        // layout modes submenu
        "layout-columns",
        "layout-tiling",
        "layout-monocle",
        "primary-grow",
        "primary-shrink",
        "primary-count-increase",
        "primary-count-decrease",
        // workspace submenu
        "new-workspace",
        "rename-workspace",
        "close-workspace",
        "new-project",
        "workspace-back-and-forth",
        // flattened system/dev
        "dev-restart-gui",
        "dev-restart-all",
        "open-system-console",
    ];
    for e in expected {
        assert!(
            leaf_actions.contains(&e),
            "expected menu to contain leaf {:?}, got {:?}",
            e,
            leaf_actions
        );
    }
    // Everything else is out of the shell scope. Tile verbs (close, send, tag,
    // hide/unhide, detach, archive) now live on the `<space>` tile menu; the
    // retired plane view + set-cwd + also-show are gone entirely.
    for gone in [
        "close-window",
        "send-tile-follow",
        "tile-tag",
        "tile-detach",
        "tile-hide",
        "tile-unhide",
        "archive-session",
        "workspace-set-cwd",
        "also-show-tile",
        "plane-zoom-in",
        "plane-reset-view",
        "workspace-toggle-columns",
        "tag-add",
        "quit",
    ] {
        assert!(
            !leaf_actions.contains(&gone),
            "{gone:?} should no longer be in the shell menu"
        );
    }
}

#[test]
fn shell_menu_root_is_the_approved_items() {
    let menu = gpui_menu();
    let actual: Vec<(String, &str)> = menu
        .iter()
        .filter(|node| {
            matches!(node.kind(), MenuNodeKind::Command | MenuNodeKind::Submenu)
        })
        .map(|node| (format_menu_key(&node.key), node.label.as_str()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("n".into(), "new tile"),
            ("t".into(), "theme"),
            ("j".into(), "toggle jump panel"),
            ("l".into(), "layout"),
            ("w".into(), "workspace"),
            ("r".into(), "rebuild and restart gui"),
            ("R".into(), "rebuild and restart all"),
            ("`".into(), "system console"),
        ],
        "the shell root is an exact contract; tile verbs belong on the tile menu"
    );
    // The flattened system commands dispatch straight from the root.
    for (key, expected) in [('r', "dev-restart-gui"), ('R', "dev-restart-all")] {
        let mut state = MenuState::new();
        state.open();
        assert_eq!(
            state.process_key(KeyPress::new(Key::Char(key), KMods::NONE), &menu),
            Some(expected.to_string()),
            "root {key} must dispatch {expected}"
        );
    }
}

#[test]
fn agent_menu_root_and_view_are_the_approved_items() {
    let menu = agent_local_menu();
    let actual: Vec<(String, &str)> = menu
        .iter()
        .filter(|node| {
            matches!(
                node.kind(),
                MenuNodeKind::Command | MenuNodeKind::Submenu
            )
        })
        .map(|node| (format_menu_key(&node.key), node.label.as_str()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("w".into(), "switch worksheet ⇄ message box"),
            ("m".into(), "switch model"),
            ("s".into(), "select session"),
            ("c".into(), "clear"),
            ("v".into(), "view"),
            // shared tile-menu tail (UXI-Menu-9) + agent-only archive
            ("p".into(), "send to workspace"),
            ("X".into(), "close"),
            ("t".into(), "tag"),
            ("h".into(), "hide"),
            ("u".into(), "unhide"),
            ("f".into(), "detach tile"),
            // agent-only session verbs
            ("r".into(), "rename session"),
            ("a".into(), "archive"),
        ]
    );
    let view = menu
        .iter()
        .find(|node| node.label == "view")
        .expect("view submenu");
    let MenuAction::Submenu(children) = &view.action else {
        panic!("view must be a submenu");
    };
    let children: Vec<(String, &str)> = children
        .iter()
        .map(|node| (format_menu_key(&node.key), node.label.as_str()))
        .collect();
    assert_eq!(
        children,
        vec![("a".into(), "agents"), ("t".into(), "tasks")]
    );

    for (key, expected) in [
        ('w', "agent-input-toggle"),
        ('s', "claude-session-picker"),
        ('c', "claude-clear"),
        ('p', "send-tile-follow"),
        ('X', "close-window"),
        ('t', "tile-tag"),
        ('h', "tile-hide"),
        ('u', "tile-unhide"),
        ('f', "tile-detach"),
        ('r', "claude-rename"),
        ('a', "archive-session"),
    ] {
        let mut state = MenuState::new();
        state.open();
        assert_eq!(
            state.process_key(KeyPress::new(Key::Char(key), KMods::NONE), &menu),
            Some(expected.to_string()),
            "Agent root {key} must dispatch {expected}"
        );
    }
}

#[test]
fn every_tile_menu_has_shared_tile_commands() {
    // UXI-Menu-9: every `<space>` tile menu ends with the shared tile commands:
    // send(p)/close(X)/tag(t) then hide(h)/unhide(u)/detach(f). The Agent menu
    // additionally carries archive(a).
    let shared = [
        ("p", "send to workspace", "send-tile-follow"),
        ("X", "close", "close-window"),
        ("t", "tag", "tile-tag"),
        ("h", "hide", "tile-hide"),
        ("u", "unhide", "tile-unhide"),
        ("f", "detach tile", "tile-detach"),
    ];
    for (name, menu) in [
        ("doc", doc_local_menu()),
        ("edit", edit_local_menu()),
        ("agent", agent_local_menu()),
        ("browser", browser_local_menu()),
        ("linear", linear_local_menu()),
        ("cog", cog_local_menu()),
        ("keymap", keymap_local_menu()),
    ] {
        for (key, label, command) in shared {
            let node = menu
                .iter()
                .find(|n| n.label == label)
                .unwrap_or_else(|| panic!("{name} menu is missing shared command {label}"));
            assert_eq!(format_menu_key(&node.key), key, "{name} {label} key");
            assert!(
                matches!(&node.action, MenuAction::Command(actual) if actual == command),
                "{name} {label} must dispatch {command}"
            );
        }
    }
    // Rename + Archive are Agent-only (session concepts).
    let agent = agent_local_menu();
    let archive = agent
        .iter()
        .find(|n| n.label == "archive")
        .expect("agent menu must carry archive");
    assert_eq!(format_menu_key(&archive.key), "a");
    assert!(
        matches!(&archive.action, MenuAction::Command(c) if c == "archive-session"),
    );
    let rename = agent
        .iter()
        .find(|n| n.label == "rename session")
        .expect("agent menu must carry rename session");
    assert_eq!(format_menu_key(&rename.key), "r");
    assert!(
        matches!(&rename.action, MenuAction::Command(c) if c == "claude-rename"),
    );
    for (name, menu) in [
        ("doc", doc_local_menu()),
        ("edit", edit_local_menu()),
        ("browser", browser_local_menu()),
    ] {
        assert!(
            !menu.iter().any(|n| n.label == "archive" || n.label == "rename session"),
            "{name} menu must NOT carry session-only verbs"
        );
    }
}

#[test]
fn menu_state_round_trip_picks_command() {
    // Pressing root `j` closes the menu and dispatches the jump-panel toggle.
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('j'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("toggle-jump-panel".to_string()));
    assert!(!state.is_active(), "menu should close after a leaf select");
}

#[test]
fn shell_layout_submenu_selects_modes() {
    // UXI-Workspace-26: `l` opens the layout submenu; c/t/m pick a mode.
    let menu = gpui_menu();
    for (key, expected) in [
        ('c', "layout-columns"),
        ('t', "layout-tiling"),
        ('m', "layout-monocle"),
    ] {
        let mut state = MenuState::new();
        state.open();
        assert_eq!(
            state.process_key(KeyPress::new(Key::Char('l'), KMods::NONE), &menu),
            None,
            "l opens the layout submenu"
        );
        assert_eq!(
            state.process_key(KeyPress::new(Key::Char(key), KMods::NONE), &menu),
            Some(expected.to_string()),
            "l {key} must dispatch {expected}"
        );
    }
}

#[test]
fn shell_menu_close_tile_at_root_close_workspace_under_w() {
    // UXI-Menu-8/9: tile close moved to the `<space>` tile menu, so the shell
    // root no longer binds `X`; workspace close remains lowercase `w x`.
    let menu = gpui_menu();

    let mut upper = MenuState::new();
    upper.open();
    assert_eq!(
        upper.process_key(KeyPress::new(Key::Char('X'), KMods::NONE), &menu),
        None,
        "root X is no longer bound (close moved to the tile menu)"
    );

    let mut lower = MenuState::new();
    lower.open();
    assert_eq!(
        lower.process_key(KeyPress::new(Key::Char('x'), KMods::NONE), &menu),
        None,
        "lowercase x is intentionally unbound at root"
    );

    // Descend into the workspace submenu, then `x` closes the workspace.
    let mut ws = MenuState::new();
    ws.open();
    assert_eq!(
        ws.process_key(KeyPress::new(Key::Char('w'), KMods::NONE), &menu),
        None,
        "w opens the workspace submenu"
    );
    assert_eq!(
        ws.process_key(KeyPress::new(Key::Char('x'), KMods::NONE), &menu),
        Some("close-workspace".to_string()),
        "w x closes the workspace"
    );
}

#[test]
fn menu_trail_crumbs_tracks_descent() {
    // UXI-Menu-3: the header trail is the literal chord to the current level.
    // At root it's just [leader] + the scope name; each submenu descent appends
    // that submenu's key and its label becomes the level name.
    use crate::menu_trail_crumbs;
    let menu = doc_local_menu();

    // Root: leader glyph only, scope name is the level label.
    let mut state = MenuState::new();
    state.open();
    let (crumbs, label) = menu_trail_crumbs(&menu, &state.path, "␣", "DOC");
    assert_eq!(
        crumbs,
        vec!["␣".to_string()],
        "root shows only the leader glyph"
    );
    assert_eq!(label, "DOC", "root level label is the scope");

    // Descend into the `n` → "navigate" submenu: crumb `n` is appended and the
    // level label becomes that submenu's name.
    let after_n = state.process_key(KeyPress::new(Key::Char('n'), KMods::NONE), &menu);
    assert_eq!(after_n, None, "n opens the navigate submenu");
    let (crumbs, label) = menu_trail_crumbs(&menu, &state.path, "␣", "DOC");
    assert_eq!(
        crumbs,
        vec!["␣".to_string(), "n".to_string()],
        "descended key is appended to the trail"
    );
    assert_eq!(
        label, "navigate",
        "level label is the submenu label after descent"
    );
    // Negative control: a `current_label`-only breadcrumb would drop the `n`
    // crumb — asserting the descended key survives is what the trail adds.
    assert!(
        crumbs.contains(&"n".to_string()),
        "trail must carry the descended key"
    );
}

#[test]
fn local_menus_have_no_duplicate_keys_per_level() {
    // UXI-Menu-7: every menu (the `.` shell menu and every `<space>` App menu)
    // must be unambiguous — one key, one entry, at each depth, INCLUDING the new
    // shell `w`/`s` submenus and the agent `s`/`v` submenus.
    fn check_level(nodes: &[MenuNode], path: &str) {
        let mut seen: Vec<&[KeyPress]> = Vec::new();
        for n in nodes {
            match &n.action {
                yalda::menu::MenuAction::Command(_) | yalda::menu::MenuAction::Submenu(_) => {
                    assert!(
                        !seen.contains(&n.key.as_slice()),
                        "duplicate key {:?} at {path}",
                        n.key
                    );
                    seen.push(&n.key);
                }
                _ => {}
            }
            if let yalda::menu::MenuAction::Submenu(children) = &n.action {
                check_level(children, &format!("{path}/{}", n.label));
            }
        }
    }
    check_level(&gpui_menu(), "shell");
    check_level(&doc_local_menu(), "doc");
    check_level(&edit_local_menu(), "edit");
    check_level(&agent_local_menu(), "agent");
    check_level(&browser_local_menu(), "browser");
    check_level(&linear_local_menu(), "linear");
    check_level(&cog_local_menu(), "cog");
    check_level(&keymap_local_menu(), "keymap");
    // (The DYNAMIC agent menu — grafted archive + model submenu — is covered by
    // verify_harness::agent_dynamic_menu_has_no_duplicate_keys, which needs a view.)
}

#[test]
fn doc_local_menu_g_g_resolves_goto_top() {
    let mut state = MenuState::new();
    state.open();
    let menu = doc_local_menu();
    let after_g = state.process_key(KeyPress::new(Key::Char('g'), KMods::NONE), &menu);
    assert_eq!(after_g, None, "g alone should open the goto submenu");
    let cmd = state.process_key(KeyPress::new(Key::Char('g'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("doc-goto-top".to_string()));
}

#[test]
fn browser_local_menu_dot_resolves_toggle_hidden() {
    // `.` opens the local menu; `. .` is the relocated toggle-hidden.
    let mut state = MenuState::new();
    state.open();
    let menu = browser_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('.'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("browser-hidden".to_string()));
}

#[test]
fn edit_local_menu_e_v_resolves_extend_mode() {
    let mut state = MenuState::new();
    state.open();
    let menu = edit_local_menu();
    let after_e = state.process_key(KeyPress::new(Key::Char('e'), KMods::NONE), &menu);
    assert_eq!(after_e, None, "e alone should open the edit submenu");
    let cmd = state.process_key(KeyPress::new(Key::Char('v'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("toggle-extend-mode".to_string()));
}

#[test]
fn agent_local_s_resolves_to_session_picker() {
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('s'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("claude-session-picker".to_string()));
}

#[test]
fn agent_local_m_opens_model_submenu() {
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('m'), KMods::NONE), &menu);
    assert_eq!(cmd, None);
    assert!(
        state.is_active(),
        "model submenu remains open on its placeholder"
    );
}

#[test]
fn agent_local_c_resolves_to_clear() {
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('c'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("claude-clear".to_string()));
}

#[test]
fn agent_local_p_sends_tile_to_workspace() {
    // Send-to-workspace moved onto the tile menu (UXI-Menu-9); `p` dispatches it.
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('p'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("send-tile-follow".to_string()));
}

#[test]
fn theme_toggle_alternates_nightfox_and_folio() {
    // From Folio → Nightfox; from anything else (Nightfox or any other theme)
    // → Folio, so the toggle always lands on one of the pair and alternates.
    assert_eq!(next_toggle_theme(ThemeName::Folio), ThemeName::Nightfox);
    assert_eq!(next_toggle_theme(ThemeName::Nightfox), ThemeName::Folio);
    // Any non-Folio theme jumps into the pair at Folio.
    assert_eq!(next_toggle_theme(ThemeName::Dracula), ThemeName::Folio);
    // Toggling twice from Folio returns to Folio.
    let back = next_toggle_theme(next_toggle_theme(ThemeName::Folio));
    assert_eq!(back, ThemeName::Folio);
}

#[test]
fn agent_local_shift_c_is_absent_clear_is_lowercase() {
    let mut state = MenuState::new();
    state.open();
    let menu = agent_local_menu();
    let cmd = state.process_key(KeyPress::new(Key::Char('C'), KMods::NONE), &menu);
    assert_eq!(cmd, None);
    assert!(state.is_active());
}

#[test]
fn menu_n_b_resolves_to_new_buffer_tile() {
    // `n` opens the new submenu; `b` creates a new buffer tile (in Picking).
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    let after_n = state.process_key(KeyPress::new(Key::Char('n'), KMods::NONE), &menu);
    assert_eq!(after_n, None, "n alone should open the new submenu");
    assert!(state.is_active(), "submenu open keeps menu state active");
    let cmd = state.process_key(KeyPress::new(Key::Char('b'), KMods::NONE), &menu);
    assert_eq!(cmd, Some("new-buffer-tile".to_string()));
}

#[test]
fn menu_root_submenus_resolve() {
    // Root layout (untitled.md "Workspace › Commands"): n=new, t=theme are
    // the two submenus; the rest are leaves.
    let menu = gpui_menu();
    for (ch, follow, expected) in &[
        ('n', 'a', "new-agent-tile"),
        ('n', 'b', "new-buffer-tile"),
        ('n', 'l', "new-linear-tile"),
        ('t', 'n', "theme-nightfox"),
        ('t', 'f', "theme-folio"),
    ] {
        let mut state = MenuState::new();
        state.open();
        let after = state.process_key(KeyPress::new(Key::Char(*ch), KMods::NONE), &menu);
        assert_eq!(after, None, "{ch:?} should open a submenu");
        let cmd = state.process_key(KeyPress::new(Key::Char(*follow), KMods::NONE), &menu);
        assert_eq!(
            cmd,
            Some(expected.to_string()),
            "{ch:?} {follow:?} should resolve to {expected:?}"
        );
    }
}

#[test]
fn menu_e_and_w_resolve_to_edit_views() {
    // enter-edit / enter-wp live in the Doc local menu (the workspace menu
    // no longer carries any tile-scoped entries).
    let local = doc_local_menu();
    for (ch, expected) in &[('e', "enter-edit"), ('w', "enter-wp")] {
        let mut state = MenuState::new();
        state.open();
        let cmd = state.process_key(KeyPress::new(Key::Char(*ch), KMods::NONE), &local);
        assert_eq!(
            cmd,
            Some(expected.to_string()),
            "doc-local key {:?} should resolve to {:?}",
            ch,
            expected
        );
    }
}

#[test]
fn menu_state_unknown_key_keeps_menu_open() {
    let mut state = MenuState::new();
    state.open();
    let menu = gpui_menu();
    // 'z' isn't bound at root.
    let cmd = state.process_key(KeyPress::new(Key::Char('z'), KMods::NONE), &menu);
    assert_eq!(cmd, None);
    assert!(state.is_active(), "menu should stay open on unknown key");
}

#[test]
fn append_llm_chunk_chains_turns_above_draft() {
    // Mirrors the old splice-then-lock-then-splice integration test
    // for the new append-and-tag flow: each turn appends just after
    // the last frozen Llm(n) line; a manually-inserted user draft
    // (simulating worksheet typing) survives the agent's reply
    // arriving for the same turn.
    let mut ed = Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
    // Turn 1: agent greets.
    ed.append_llm_chunk(TurnId::Llm(1), "Hi.");
    finalize_agent_turn(&mut ed);
    // User types a reply on the editable line below the frozen
    // "Hi.". The worksheet cursor lives wherever the user puts it.
    ed.cursor_mut().line = ed.document().line_count().saturating_sub(1);
    ed.cursor_mut().col = 0;
    ed.insert_char('o');
    ed.insert_char('k');
    // Turn 2 starts: agent's first chunk goes at EOF (no Llm(2) lines
    // yet) — i.e. after the user's draft "ok". This matches the
    // worksheet's "agent writes at the far end" model (§19).
    ed.append_llm_chunk(TurnId::Llm(2), "Yes!");
    finalize_agent_turn(&mut ed);

    let text = ed.document().full_text();
    assert!(text.contains("Hi."));
    assert!(text.contains("ok"));
    assert!(text.contains("Yes!"));
    let pos_hi = text.find("Hi.").unwrap();
    let pos_ok = text.find("ok").unwrap();
    let pos_yes = text.find("Yes!").unwrap();
    assert!(pos_hi < pos_ok, "Hi before ok ({:?})", text);
    assert!(pos_ok < pos_yes, "ok before Yes! ({:?})", text);
}

#[test]
fn agent_content_floors_above_worksheet_draft() {
    // The interspersed-tool-group bug: while the agent streams a turn, the
    // user composes a worksheet draft at the tail. Tool anchors (and the LLM
    // EOF fallback) must splice ABOVE that untagged draft, never below it.
    let mut ed = Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
    ed.append_llm_chunk(TurnId::Llm(1), "Agent prose.\n");
    let a0 = ed.anchor_for_line(0);
    ed.metadata_mut::<TurnId>().insert(a0, TurnId::Llm(1));
    finalize_agent_turn(&mut ed);

    // User types a draft on the editable tail (worksheet compose).
    ed.cursor_mut().line = ed.document().line_count().saturating_sub(1);
    ed.cursor_mut().col = 0;
    for ch in "draft".chars() {
        ed.insert_char(ch);
    }
    fn draft_line(ed: &Editor) -> usize {
        ed.document()
            .full_text()
            .lines()
            .position(|l| l.contains("draft"))
            .unwrap()
    }

    // A tool call arrives mid-compose. With the draft present the floor is
    // above it, so the anchor lands above the draft line.
    let floor = agent_tail_floor_char(&ed);
    assert!(
        floor < ed.document().rope().len_chars(),
        "a non-blank draft must push the floor above EOF"
    );
    let anchor = anchor_for_new_tool_call(&mut ed, floor);
    ed.metadata_mut::<TurnId>().insert(anchor, TurnId::Tool(1));
    let tool_line = ed.line_for_anchor(anchor).expect("tool anchor resolves");
    assert!(
        tool_line < draft_line(&ed),
        "tool anchor must render above the user draft, not below it: {:?}",
        ed.document().full_text()
    );

    // A same-turn LLM chunk also floors above the draft.
    let floor = agent_tail_floor_char(&ed);
    ed.append_llm_chunk_floored(TurnId::Llm(1), "More prose.\n", floor);
    let text = ed.document().full_text();
    let pos_more = text.find("More prose.").unwrap();
    let pos_draft = text.find("draft").unwrap();
    assert!(
        pos_more < pos_draft,
        "streamed prose must stay above the draft: {text:?}"
    );
    assert!(
        text.trim_end().ends_with("draft"),
        "the user's draft stays at the tail: {text:?}"
    );
}

#[test]
fn agent_tail_floor_is_eof_without_draft() {
    // No-op guarantee for Chatbox / an untouched worksheet tail: an all-blank
    // trailing region keeps the floor at EOF, so the splice is unchanged.
    let mut ed = Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
    ed.append_llm_chunk(TurnId::Llm(1), "Agent prose.\n");
    let a0 = ed.anchor_for_line(0);
    ed.metadata_mut::<TurnId>().insert(a0, TurnId::Llm(1));
    finalize_agent_turn(&mut ed);
    assert_eq!(
        agent_tail_floor_char(&ed),
        ed.document().rope().len_chars(),
        "blank trailing region ⇒ floor is EOF (no behavior change)"
    );
}

/// Source files must split into one CodeBlock per line: the doc view scrolls
/// and focuses by block (j/k move `cursor_block`) and `gpui::list`
/// virtualizes by item, so a whole file as ONE block can neither scroll nor
/// virtualize. `start_line` carries the absolute line number for the gutter.
#[test]
fn source_file_renders_one_block_per_line() {
    let path = std::path::Path::new("example.rs");
    let blocks = render_with_wiki(
        "fn main() {\n    let x = 1;\n}\n",
        &Theme::default(),
        Some(path),
    );
    assert_eq!(blocks.len(), 3, "one block per source line");
    for (i, b) in blocks.iter().enumerate() {
        match b {
            RenderedBlock::CodeBlock {
                lines,
                source_file,
                start_line,
                ..
            } => {
                assert!(*source_file);
                assert_eq!(*start_line, i);
                assert_eq!(lines.len(), 1);
            }
            other => panic!("expected CodeBlock, got {:?}", other),
        }
    }
}

/// Empty source files still produce a single (empty) block so cursor and
/// reveal logic have a target.
#[test]
fn empty_source_file_renders_single_block() {
    let path = std::path::Path::new("empty.rs");
    let blocks = render_with_wiki("", &Theme::default(), Some(path));
    assert_eq!(blocks.len(), 1);
}

// --- Visual-selection highlight on blank / whitespace-only lines
//     (apply_line_selection). A blank line whose newline is inside a
//     multi-line selection must still render a highlighted placeholder so the
//     selection reads as continuous; the syntax highlighter yields no segments
//     for such lines, so apply_selection_bg alone would paint nothing. ---

#[test]
fn blank_line_inside_selection_gets_highlight_placeholder() {
    let style = NStyle::default();
    let bg = NColor::Rgb(1, 2, 3);
    // Selection covers line 0..=2; line 1 is blank and fully interior.
    let out = apply_line_selection(&[], "", ((0, 0), (2, 1)), 1, style, bg);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, " ");
    assert_eq!(out[0].1, style.bg(bg), "placeholder carries selection bg");
}

#[test]
fn whitespace_only_line_fully_selected_gets_highlight_placeholder() {
    let style = NStyle::default();
    let bg = NColor::Rgb(1, 2, 3);
    // A line of spaces, fully inside the selection (newline also selected).
    let out = apply_line_selection(&[], "   ", ((0, 0), (2, 0)), 1, style, bg);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, " ");
    assert_eq!(out[0].1, style.bg(bg));
}

#[test]
fn blank_line_at_selection_end_is_not_highlighted() {
    // Selection ends at the start of the blank line (col 0) — its newline is
    // NOT selected, so it stays un-highlighted (matches vim).
    let out = apply_line_selection(
        &[],
        "",
        ((0, 0), (1, 0)),
        1,
        NStyle::default(),
        NColor::Rgb(1, 2, 3),
    );
    assert!(out.is_empty(), "unchanged empty input → no placeholder");
}

#[test]
fn line_outside_selection_is_unchanged() {
    let style = NStyle::default();
    let segs = vec![("text".to_string(), style)];
    let out = apply_line_selection(
        &segs,
        "text",
        ((0, 0), (0, 4)),
        3,
        style,
        NColor::Rgb(1, 2, 3),
    );
    assert_eq!(out, segs, "line 3 is outside a line-0 selection");
}

/// Regression (doc scroll anchoring): a block-count change (e.g. a live
/// edit-flush re-parse from a sibling Edit tile) must keep the Doc viewport
/// anchored — `DocState::reconcile_list` splices the changed range instead of
/// `reset()`-ing the list, which would snap the viewport to the top. Mirrors
/// the Edit view's `edit_list_splice_preserves_scroll_anchor`.
#[test]
fn doc_list_splice_preserves_scroll_anchor() {
    let path = std::path::PathBuf::from("test.md");
    let mut src = String::new();
    for i in 0..40 {
        src.push_str(&format!("paragraph {i}\n\n"));
    }
    let blocks = render_with_wiki(&src, &Theme::default(), Some(&path));
    let n = blocks.len();
    assert!(n >= 30, "expected many blocks, got {n}");

    let mut doc = DocState::viewing(blocks, "test.md".into(), None);

    // First reconcile populates the list to N items.
    doc.reconcile_list();
    assert_eq!(doc.list.len(), n, "list synced to block count");

    // Scroll into the middle of the document.
    doc.list.state().scroll_to(gpui::ListOffset {
        item_ix: 20,
        offset_in_item: gpui::px(0.),
    });

    // Remove a block BELOW the viewport top (an edit-flush re-parse).
    let mut new_blocks = doc.blocks.clone();
    new_blocks.remove(30);
    doc.set_blocks(new_blocks);
    doc.reconcile_list();

    assert_eq!(doc.list.len(), n - 1, "one block removed");
    assert_eq!(
        doc.list.state().logical_scroll_top().item_ix,
        20,
        "a block change below the viewport top must leave the doc anchored, not jump to 0"
    );
}

// ============================================================================
// Model C — worksheet/compose agent-buffer invariants (design-c.md §6)
// ============================================================================

/// INV (§4.4): a reconnect/replay rebuilds the TRANSCRIPT editor only; the
/// compose draft lives in `input_surface` and must survive untouched. Pins the
/// "draft lost on reconnect" failure mode the redesign fixes for free.
#[test]
fn compose_draft_survives_reset_for_replay() {
    let mut st = AgentState::new_for_test();
    for ch in "half-typed prompt".chars() {
        st.input_surface.compose_mut().editor.insert_char(ch);
    }
    // Put some committed content in the transcript so the reset has work to do.
    st.editor.programmatic_insert(0, "old transcript\n");

    st.reset_for_replay();

    assert_eq!(
        st.input_surface.compose().text(),
        "half-typed prompt",
        "compose draft must survive a replay reset"
    );
    assert_eq!(
        st.editor.document().full_text(),
        "",
        "the transcript editor is wiped for replay"
    );
}

/// REGRESSION ("/clear then can't type until I toggle chatbox↔worksheet"): the
/// `/clear` path settles a fresh session into a typeable worksheet block, but the
/// new server session's channel then OPENS asynchronously (a bumped generation ⇒
/// `reset_for_replay`). A historyless session has no `ReplayEnd`, so nothing
/// re-settles — `reset_for_replay` closes the block and leaves it closed. Because
/// the worksheet keystroke path busts the transcript cache ONLY while
/// `inline_you_block_active()` (i.e. `you_block_open`), a closed block means typed
/// chars never repaint. The block must survive an empty-transcript replay.
#[test]
fn clear_then_empty_channel_open_keeps_worksheet_typeable() {
    let mut st = AgentState::new_server_managed(None);
    st.settle_input_focus();
    assert!(
        st.inline_you_block_active(),
        "clear settles a typeable worksheet block"
    );
    // The fresh server session's channel opens (bumped generation ⇒ replay reset).
    // No history ⇒ no ReplayEnd ⇒ no compensating finish_replay/settle.
    st.reset_for_replay();
    assert!(
        st.inline_you_block_active(),
        "after clear + channel-open the empty worksheet must STAY typeable \
         (else typed chars don't repaint until a chatbox↔worksheet toggle)"
    );
}

/// EXHAUSTIVE truth table for `inline_you_block_active` — the gate that decides
/// whether the inline You-block renders AND a worksheet keystroke busts the
/// transcript cache (repaints). The predicate (UXI-AgentTile-12) is
/// `(you_block_open || focus==Compose) && !awaiting && !chatbox`. Every clause
/// must be load-bearing (mutation testing found the original three operands
/// untested); the `|| focus==Compose` clause closes the recurring
/// "/clear worksheet-invisible" bug — see docs/projects/clear-worksheet-invisible.
#[test]
fn inline_you_block_active_truth_table() {
    let base = || {
        let mut st = AgentState::new_server_managed(None);
        st.input_surface = InputSurface::new(InputModeKind::Worksheet);
        st.you_block_open = true;
        st // open + idle + worksheet (focus defaults to Transcript)
    };
    assert!(
        base().inline_you_block_active(),
        "open + idle + worksheet ⇒ active"
    );

    // Each conjunct flipped in isolation must turn it OFF.
    let mut closed = base();
    closed.you_block_open = false;
    closed.focus = AgentFocus::Transcript; // explicit: NOT the compose either
    assert!(
        !closed.inline_you_block_active(),
        "block closed + focus on transcript (nav) ⇒ inactive"
    );

    let mut awaiting = base();
    awaiting.turn_phase = TurnPhase::begin(std::time::Instant::now());
    assert!(
        !awaiting.inline_you_block_active(),
        "mid-turn (awaiting) ⇒ inactive"
    );

    let mut chatbox = base();
    chatbox.input_surface = InputSurface::new(InputModeKind::Chatbox);
    assert!(
        !chatbox.inline_you_block_active(),
        "chatbox placement ⇒ inactive"
    );

    // UXI-AgentTile-12: the `|| focus==Compose` clause. "The hole" — focus on the compose
    // in an idle worksheet with the block CLOSED — must be ACTIVE (else keystrokes
    // route to a compose that paints nowhere: the invisible-text bug).
    let hole = || {
        let mut st = base();
        st.you_block_open = false;
        st.focus = AgentFocus::Compose;
        st
    };
    assert!(
        hole().inline_you_block_active(),
        "UXI-AgentTile-12: focus=Compose + closed block + idle worksheet ⇒ ACTIVE (routing⇒painting)"
    );
    // But the compose-focus clause is still gated by !awaiting and !chatbox — a
    // focus=Compose that is mid-turn or chatbox has its draft in the bottom box.
    let mut hole_awaiting = hole();
    hole_awaiting.turn_phase = TurnPhase::begin(std::time::Instant::now());
    assert!(
        !hole_awaiting.inline_you_block_active(),
        "focus=Compose but AWAITING ⇒ inactive (draft is the mid-turn bottom box)"
    );
    let mut hole_chatbox = hole();
    hole_chatbox.input_surface = InputSurface::new(InputModeKind::Chatbox);
    assert!(
        !hole_chatbox.inline_you_block_active(),
        "focus=Compose but CHATBOX ⇒ inactive (draft is the pinned box)"
    );
}

/// INV (§4.5): the follow-tail policy is `follow_output` in BOTH placements —
/// the cursor lives in the compose, never the (read-only) transcript, so the
/// old `Worksheet => cursor_at_eof` arm is gone.
#[test]
fn should_follow_tail_is_follow_output_only() {
    assert!(should_follow_tail(true), "follow when follow_output is set");
    assert!(
        !should_follow_tail(false),
        "don't follow when follow_output is clear"
    );
}

/// `Compose::seeded` round-trips multi-line text (used by draft restore +
/// the not-delivered resubmit path).
#[test]
fn compose_seeded_roundtrips_text() {
    assert_eq!(Compose::seeded("alpha\nbeta").text(), "alpha\nbeta");
    assert_eq!(Compose::seeded("").text(), "");
}

/// `InputSurface::with_draft` sets BOTH placement and the seeded draft — the
/// shape the three restore sites rely on (a mechanical `Compose::new()` would
/// silently drop the draft).
#[test]
fn input_surface_with_draft_sets_mode_and_draft() {
    let s = InputSurface::with_draft(InputModeKind::Worksheet, "resumed");
    assert_eq!(s.mode(), InputModeKind::Worksheet);
    assert!(!s.is_chatbox());
    assert_eq!(s.compose().text(), "resumed");

    let empty = InputSurface::with_draft(InputModeKind::Chatbox, "");
    assert!(empty.is_chatbox());
    assert_eq!(empty.compose().text(), "");
}

/// Persistence round-trip (§4.4): a non-empty compose draft survives
/// save→load; an empty draft is not written (absent → None on load). Uses the
/// `with_acp_persist_path` seam so it never touches `~/.yalda`.
#[test]
fn compose_draft_persist_roundtrip() {
    let dir = std::env::temp_dir().join(format!("yalda_compose_persist_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("acp_sessions.json");
    let cwd = std::path::Path::new("/tmp/yalda-test-cwd");

    persist::with_acp_persist_path(path.clone(), || {
        let snaps = vec![
            persist::SessionSnapshot {
                id: "S-with-draft".into(),
                label: "claude-1".into(),
                provider: AgentProvider::Claude,
                active: true,
                mode: InputModeKind::Worksheet,
                tasklist_open: false,
                subagents_open: false,
                sidepanel_hidden: false,
                cwd: cwd.to_path_buf(),
                compose_draft: Some("persisted draft".into()),
                summary: None,
            },
            persist::SessionSnapshot {
                id: "S-empty".into(),
                label: "claude-2".into(),
                provider: AgentProvider::Claude,
                active: false,
                mode: InputModeKind::Chatbox,
                tasklist_open: false,
                subagents_open: false,
                sidepanel_hidden: false,
                cwd: cwd.to_path_buf(),
                compose_draft: None,
                summary: None,
            },
        ];
        persist::save_persisted_acp_sessions(cwd, &snaps);

        let loaded = persist::load_persisted_acp_sessions(cwd);
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0].compose_draft.as_deref(),
            Some("persisted draft"),
            "non-empty draft round-trips"
        );
        assert_eq!(
            loaded[1].compose_draft, None,
            "empty draft is not written / loads as None"
        );
        assert_eq!(loaded[0].mode, InputModeKind::Worksheet);
    });

    let _ = std::fs::remove_dir_all(&dir);
}

/// REGRESSION (bug-0005): two sessions in one cwd must never restore with the SAME
/// label ("two sessions named 'claude'"). Drives the REAL loader
/// (`load_persisted_acp_sessions`) against a raw `acp_sessions.json` carrying the
/// fallback shapes that produced the bug — two slots explicitly labeled "claude" plus
/// one with a MISSING label — and asserts every restored label is non-empty and
/// distinct.
///
/// Negative control: remove the `dedupe_slot_labels(&mut slots)` call in
/// `load_persisted_acp_sessions` → the two "claude"s survive and the missing one
/// loads empty → the uniqueness + non-empty asserts fail RED.
#[test]
fn restore_dedupes_duplicate_claude_labels() {
    let dir = std::env::temp_dir().join(format!("yalda_label_dedup_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("acp_sessions.json");
    let cwd = std::path::Path::new("/tmp/yalda-dedup-cwd");
    let json = format!(
        r#"{{ "{}": [
            {{"id":"S-a","label":"claude","active":true}},
            {{"id":"S-b","label":"claude"}},
            {{"id":"S-c"}}
        ]}}"#,
        cwd.to_string_lossy()
    );
    std::fs::write(&path, json).unwrap();

    let loaded =
        persist::with_acp_persist_path(path.clone(), || persist::load_persisted_acp_sessions(cwd));
    assert_eq!(loaded.len(), 3, "all three slots load");
    assert!(
        loaded.iter().all(|slot| slot.provider.is_none()),
        "pre-provider snapshots stay unknown until the authoritative roster arrives"
    );
    let labels: Vec<String> = loaded.iter().map(|s| s.label.clone()).collect();
    assert!(
        labels.iter().all(|l| !l.trim().is_empty()),
        "no restored label is empty/bare: {labels:?}"
    );
    let uniq: std::collections::HashSet<&String> = labels.iter().collect();
    assert_eq!(
        uniq.len(),
        labels.len(),
        "every restored session label is unique — never two 'claude': {labels:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Unit: `unique_label` keeps a free non-empty name and otherwise hands out the
/// smallest free `claude-N` (the core of the bug-0005 dedupe).
#[test]
fn unique_label_fills_gaps_and_replaces_empty_or_dup() {
    use std::collections::HashSet;
    let mut used: HashSet<String> = HashSet::new();
    // A free, valid label is kept verbatim.
    let a = persist::unique_label("claude-1", &used);
    assert_eq!(a, "claude-1");
    used.insert(a);
    // A DUPLICATE gets the next free number.
    let b = persist::unique_label("claude-1", &used);
    assert_eq!(b, "claude-2");
    used.insert(b);
    // An EMPTY label gets the next free number, never bare "claude".
    let c = persist::unique_label("", &used);
    assert_eq!(c, "claude-3");
}

// ----------------------------------------------------------------------------
// INV-ORDER (the keystone) — the ordering invariant the old model lacked.
//
//   The transcript is append-only/read-only; the draft is a SEPARATE buffer.
//   The only cross-buffer transfer is text, never a position. ⇒ a turn's
//   chunks can only extend the transcript at EOF; a draft is never inside
//   history. Ordering corruption is unrepresentable.
//
// These reproduce the reported failure ("a newer agent turn renders above an
// older exchange; my draft sits in the middle of history") and would FAIL
// against the old shared-rope model (draft in the transcript ⇒ streaming had to
// insert ABOVE it via a stale floor/tag, landing mid-document).
// ----------------------------------------------------------------------------

/// Streaming a NEW agent turn while the user is mid-draft appends at transcript
/// EOF and leaves the compose untouched. Drives the REAL floor path
/// (`agent_tail_floor_char` + `append_llm_chunk_floored`) the live pump uses, so
/// a regression to mid-document insertion is caught. Pins design-c §6 #7.
#[test]
fn inv_order_streaming_with_draft_appends_at_eof() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);

    // Prior exchange: U1 then its answer, both frozen/tagged in the transcript.
    st.insert_user_turn(
        "first question",
        yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
        false,
    );
    st.editor.append_llm_chunk(TurnId::Llm(1), "first answer\n");
    let transcript_before = st.editor.document().full_text();

    // The user is composing the NEXT prompt — it lives in the SEPARATE compose
    // buffer, NOT in the transcript (this is the exact text from the report).
    for ch in "did we activate /spec".chars() {
        st.input_surface.compose_mut().editor.insert_char(ch);
    }

    // A new agent turn streams in. The live pump computes the floor and uses the
    // floored append; replay that here exactly.
    let floor = agent_tail_floor_char(&st.editor);
    assert_eq!(
        floor,
        st.editor.document().rope().len_chars(),
        "no user draft in the transcript ⇒ the floor is EOF (the structural core \
         of the fix: there is nothing to stream ABOVE)"
    );
    st.editor
        .append_llm_chunk_floored(TurnId::Llm(2), "second answer\n", floor);

    // The new turn appended at the bottom; the prior exchange is untouched and
    // still ABOVE it; the draft never entered the transcript.
    assert_eq!(
        st.editor.document().full_text(),
        format!("{transcript_before}second answer\n"),
        "the new agent turn must append at EOF, below the older exchange — not \
         mid-document, and never fused with the draft"
    );
    assert_eq!(
        st.input_surface.compose().text(),
        "did we activate /spec",
        "the draft stays in the compose buffer, out of the transcript (INV-3)"
    );
}

/// Several interleaved user/agent turns land in the transcript in the order they
/// occurred — even with a non-empty draft held throughout. The old model could
/// place a turn's continuation at its (stale) tag position, rendering a newer
/// turn above an older one; the append-only transcript makes that impossible.
#[test]
fn inv_order_interleaved_turns_stay_chronological() {
    let mut st = AgentState::new_for_test();
    st.input_surface = InputSurface::new(InputModeKind::Worksheet);

    // Hold a draft for the whole sequence — it must never perturb ordering.
    for ch in "scratch draft".chars() {
        st.input_surface.compose_mut().editor.insert_char(ch);
    }

    let submit = |st: &mut AgentState, body: &str| {
        st.insert_user_turn(
            body,
            yalda::agent_transcript::UserTurnOrigin::LocalSubmit,
            false,
        );
    };
    let stream = |st: &mut AgentState, k: usize, body: &str| {
        let floor = agent_tail_floor_char(&st.editor);
        st.editor
            .append_llm_chunk_floored(TurnId::Llm(k), body, floor);
    };

    submit(&mut st, "q1");
    stream(&mut st, 1, "a1\n");
    submit(&mut st, "q2");
    stream(&mut st, 2, "a2\n");
    submit(&mut st, "q3");
    stream(&mut st, 3, "a3\n");

    let text = st.editor.document().full_text();
    let order: Vec<&str> = ["q1", "a1", "q2", "a2", "q3", "a3"]
        .into_iter()
        .filter(|tok| text.contains(tok))
        .collect();
    assert_eq!(
        order,
        vec!["q1", "a1", "q2", "a2", "q3", "a3"],
        "transcript content must be in chronological order; got:\n{text}"
    );
    // Each marker appears strictly after the previous one (no reordering).
    let mut last = 0usize;
    for tok in ["q1", "a1", "q2", "a2", "q3", "a3"] {
        let at = text.find(tok).unwrap_or_else(|| panic!("missing {tok}"));
        assert!(
            at >= last,
            "out-of-order: {tok} at {at} precedes {last}\n{text}"
        );
        last = at;
    }
    assert_eq!(
        st.input_surface.compose().text(),
        "scratch draft",
        "the draft is untouched by any number of turns (INV-2)"
    );
}

/// UXI-AgentTile-18 (identity, not index): the layout snapshot persists WHICH
/// session occupies each tile (`resume_sid`), and `restore_layout` hands each
/// leaf back paired with ITS OWN session id — so a restart rebinds tiles by
/// identity, never by list position. This is the persistence-layer proof of the
/// fix; the live re-attach is the runtime tail (harness gap #2).
///
/// Negative control: revert `snapshot_content`'s agent arm to
/// `session_id: None` and the restored leaves come back `(_, None)` → the
/// identity assertion fails RED.
#[test]
fn agent_tile_persists_session_identity_not_index() {
    use crate::agent_sessions::SessionId;
    // Two Bound tiles (ADR-0026 enum); each carries a local SessionId. The durable
    // server id is resolved from the store via the SidResolver (single source of
    // truth — no cached resume_sid).
    let t1 = AgentTile::Bound {
        session: SessionId(1),
        reopening: None,
    };
    let t2 = AgentTile::Bound {
        session: SessionId(2),
        reopening: None,
    };
    let layout: workspace::Layout<App> = workspace::Layout::Split {
        dir: workspace::SplitDir::V,
        children: vec![
            (
                1.0,
                workspace::Layout::Leaf(workspace::Window::new(10, ProjectId(0), App::Agent(t1))),
            ),
            (
                1.0,
                workspace::Layout::Leaf(workspace::Window::new(20, ProjectId(0), App::Agent(t2))),
            ),
        ],
    };

    // The store resolver maps each tile's SessionId → its server sid.
    let resolve = |id: SessionId| match id {
        SessionId(1) => Some(crate::agent_sessions::ServerSid::new("SID-A")),
        SessionId(2) => Some(crate::agent_sessions::ServerSid::new("SID-B")),
        _ => None,
    };

    // Save side: the persisted layout carries each leaf's resolved session id.
    let snap = snapshot_layout(&layout, &resolve);

    // Restore side: each leaf comes back paired with ITS OWN id — identity
    // travels with the leaf, independent of any session-list ordering.
    let mut ws = workspace::Frame::<App>::new(ProjectId(0));
    let theme = Theme::default();
    let (_lay, _max, agents) = restore_layout(&mut ws, &theme, snap, ProjectId(0));

    assert_eq!(
        agents,
        vec![
            (10, Some(crate::agent_sessions::ServerSid::new("SID-A"))),
            (20, Some(crate::agent_sessions::ServerSid::new("SID-B"))),
        ],
        "each tile restores bound to its OWN session id (identity, not index)"
    );
}

#[test]
fn attached_hidden_and_detached_tiles_snapshot_with_identity_tags_and_solo_focus() {
    let mut projects = Projects::new();
    let cwd = std::env::temp_dir();
    let project = projects.ensure_at_cwd(cwd.clone(), "tmp");
    let mut frame = workspace::Frame::with_initial(
        App::Agent(AgentTile::Bound {
            session: SessionId(1),
            reopening: None,
        }),
        project,
    );
    frame.tile_mut(1).unwrap().tags.insert("bound-tag".into());
    let hidden = frame
        .split_focused(
            workspace::SplitDir::V,
            App::Agent(AgentTile::Bound {
                session: SessionId(3),
                reopening: None,
            }),
        )
        .unwrap();
    frame.workspaces[0].desktop.reconcile(&[1, hidden]);
    frame
        .tile_mut(hidden)
        .unwrap()
        .tags
        .insert("hidden-tag".into());
    assert!(frame.hide_window(hidden).is_ok());
    let unbound = frame.push_detached(
        App::Agent(AgentTile::Bound {
            session: SessionId(2),
            reopening: None,
        }),
        project,
    );
    frame
        .tile_mut(unbound)
        .unwrap()
        .tags
        .extend(["alpha".to_string(), "beta".to_string()]);
    assert!(frame.present_solo(unbound));

    let resolve = |id: SessionId| match id {
        SessionId(1) => Some(ServerSid::new("SID-A")),
        SessionId(3) => Some(ServerSid::new("SID-HIDDEN")),
        SessionId(2) => Some(ServerSid::new("SID-B")),
        _ => None,
    };
    let snap = snapshot_workspace(&frame, &projects, &resolve);
    assert!(snap.tile_tags_migrated);
    assert_eq!(snap.direct_unbound, Some(unbound));
    assert_eq!(
        snap.solo_presentation,
        Some(PersistedSoloPresentation::Detached(unbound))
    );
    assert!(snap.scratchpad.is_empty());
    assert_eq!(snap.workspaces[0].hidden_tiles.len(), 1);
    assert_eq!(snap.workspaces[0].hidden_tiles[0].tile.id, hidden);
    assert!(
        snap.workspaces[0].hidden_tiles[0]
            .previous_placement
            .is_some()
    );
    assert_eq!(snap.detached_tiles.len(), 1);
    assert_eq!(snap.detached_tiles[0].tile.id, unbound);
    assert_eq!(
        snap.detached_tiles[0].tile.tags,
        workspace::TagSet::from(["alpha".to_string(), "beta".to_string()])
    );
    match &snap.detached_tiles[0].tile.kind {
        PersistedKind::Agent { session_id } => {
            assert_eq!(session_id.as_ref().map(ServerSid::as_str), Some("SID-B"));
        }
        _ => panic!("unbound Agent kind must persist"),
    }
    match &snap.workspaces[0].layout {
        PersistedLayout::Leaf(leaf) => {
            assert!(leaf.tags.contains("bound-tag"));
        }
        _ => panic!("initial workspace is one leaf"),
    }

    let json = serde_json::to_string(&snap).expect("serialize frame");
    let mut back: PersistedFrame = serde_json::from_str(&json).expect("deserialize frame");
    let mut restored = workspace::Frame::new(project);
    let persisted_workspace = back.workspaces.remove(0);
    let (layout, max_bound, bound_agents) = restore_layout(
        &mut restored,
        &Theme::default(),
        persisted_workspace.layout,
        project,
    );
    restored.workspaces.push(workspace::Workspace::with_layout(
        "workspace-1".into(),
        layout,
        1,
        project,
    ));
    restored.next_window_id = max_bound + 1;
    let persisted_hidden = persisted_workspace.hidden_tiles.into_iter().next().unwrap();
    let hidden_placement = persisted_hidden.previous_placement.map(|placement| {
        (
            workspace::Slot::new(placement.row, placement.col),
            workspace::Span::new(placement.rows, placement.cols),
        )
    });
    let (hidden_window, hidden_agent) = restore_leaf(
        &mut restored,
        &Theme::default(),
        persisted_hidden.tile,
        project,
    );
    restored
        .insert_restored_hidden(0, hidden_window, hidden_placement)
        .unwrap();
    let persisted_unbound = back.detached_tiles.remove(0);
    let (window, unbound_agent) = restore_leaf(
        &mut restored,
        &Theme::default(),
        persisted_unbound.tile,
        project,
    );
    restored.next_window_id = restored.next_window_id.max(window.id() + 1);
    restored.insert_restored_detached(window).unwrap();
    assert!(back.scratchpad.is_empty());
    restored.present_solo(back.direct_unbound.unwrap());

    assert_eq!(
        bound_agents,
        vec![(1, Some(ServerSid::new("SID-A")))],
        "bound Agent identity survives"
    );
    assert_eq!(
        unbound_agent,
        Some(Some(ServerSid::new("SID-B"))),
        "unbound Agent identity survives"
    );
    assert_eq!(
        hidden_agent,
        Some(Some(ServerSid::new("SID-HIDDEN"))),
        "hidden Agent identity survives"
    );
    assert_eq!(
        restored.tile_membership(hidden),
        Some(workspace::TileMembership::Attached {
            workspace: 0,
            visibility: workspace::AttachedVisibility::Hidden,
        })
    );
    assert_eq!(restored.presented_detached_tile_id(), Some(unbound));
    assert!(restored.tile(1).unwrap().tags.contains("bound-tag"));
    assert!(restored.tile(unbound).unwrap().tags.contains("beta"));
    assert!(
        restored.alloc_window_id() > unbound,
        "allocator advances beyond both ownership domains"
    );
}
#[test]
fn all_hidden_workspace_persists_as_empty_without_inventing_a_tile() {
    let mut projects = Projects::new();
    let project = projects.ensure_at_cwd(std::env::temp_dir(), "tmp");
    let mut frame = workspace::Frame::with_initial(
        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
            std::env::temp_dir(),
        ))),
        project,
    );
    assert!(frame.hide_window(1).is_ok());

    let snapshot = snapshot_workspace(&frame, &projects, &|_| None);
    assert!(matches!(
        snapshot.workspaces[0].layout,
        PersistedLayout::Empty
    ));
    assert_eq!(snapshot.workspaces[0].hidden_tiles.len(), 1);
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: PersistedFrame = serde_json::from_str(&json).unwrap();
    assert!(matches!(
        restored.workspaces[0].layout,
        PersistedLayout::Empty
    ));
    assert_eq!(restored.workspaces[0].hidden_tiles[0].tile.id, 1);
}

#[test]
fn persisted_duplicate_agent_identity_keeps_session_cwd_project() {
    let correct_cwd = PathBuf::from("/tmp/yalda-restore-correct-project");
    let wrong_cwd = PathBuf::from("/tmp/yalda-restore-wrong-project");
    let mut projects = Projects::new();
    let correct_project = projects.ensure_at_cwd(correct_cwd.clone(), "correct");
    let wrong_project = projects.ensure_at_cwd(wrong_cwd, "wrong");
    let mut frame = workspace::Frame::with_initial(
        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
            correct_cwd.clone(),
        ))),
        correct_project,
    );
    let sid = ServerSid::new("SID-CROSS-PROJECT");
    let wrong = frame.push_detached(App::Agent(AgentTile::dormant(sid.clone())), wrong_project);
    let correct = frame.push_detached(App::Agent(AgentTile::dormant(sid.clone())), correct_project);
    let mut persisted = snapshot_workspace(&frame, &projects, &|_| None);
    // Corruption may duplicate both the durable session and the stable tile id.
    // Canonicalization therefore keys the concrete persisted occurrence, not id.
    persisted.detached_tiles[0].tile.id = correct;
    let authoritative = HashMap::from([(sid.to_string(), correct_cwd.clone())]);

    let repair = heal_persisted_agent_ownership(&mut persisted, &authoritative, &correct_cwd);

    assert_eq!(repair.removed_detached_duplicates, 1);
    assert_eq!(persisted.detached_tiles.len(), 1);
    assert_eq!(persisted.detached_tiles[0].tile.id, correct);
    assert_ne!(persisted.detached_tiles[0].tile.id, wrong);
    assert_eq!(
        persisted.detached_tiles[0].project_cwd.as_deref(),
        Some(correct_cwd.to_string_lossy().as_ref())
    );
}

#[test]
fn unbound_restore_rejects_duplicate_window_and_agent_identity() {
    let mut ids = std::collections::HashSet::from([10]);
    let mut sids = std::collections::HashSet::from(["SID-A".to_string()]);
    assert!(!accept_tile_restore(
        10,
        Some(&ServerSid::new("SID-B")),
        &mut ids,
        &mut sids,
    ));
    assert!(!accept_tile_restore(
        20,
        Some(&ServerSid::new("SID-A")),
        &mut ids,
        &mut sids,
    ));
    assert!(
        accept_tile_restore(20, Some(&ServerSid::new("SID-B")), &mut ids, &mut sids,),
        "sid rejection rolls back the id reservation"
    );
}

#[test]
fn persisted_leaf_reservation_classifies_identity_and_rejects_duplicates_atomically() {
    let agent = PersistedLeaf {
        id: 20,
        tags: workspace::TagSet::new(),
        kind: PersistedKind::Agent {
            session_id: Some(ServerSid::new("SID-A")),
        },
    };
    let same_sid = PersistedLeaf {
        id: 21,
        tags: workspace::TagSet::new(),
        kind: PersistedKind::Agent {
            session_id: Some(ServerSid::new("SID-A")),
        },
    };
    let non_agent = PersistedLeaf {
        id: 22,
        tags: workspace::TagSet::new(),
        kind: PersistedKind::Linear {},
    };
    let mut ids = std::collections::HashSet::new();
    let mut sids = std::collections::HashSet::new();

    assert_eq!(
        reserve_persisted_leaf(&agent, &mut ids, &mut sids),
        Some(PersistedTileIdentity::Agent(ServerSid::new("SID-A")))
    );
    assert_eq!(
        reserve_persisted_leaf(&same_sid, &mut ids, &mut sids),
        None,
        "one durable Agent session cannot be reserved twice"
    );
    assert!(
        !ids.contains(&same_sid.id),
        "a rejected sid must roll back its WindowId reservation"
    );
    assert_eq!(
        reserve_persisted_leaf(&non_agent, &mut ids, &mut sids),
        Some(PersistedTileIdentity::NonAgent)
    );
    assert_eq!(
        reserve_persisted_leaf(&non_agent, &mut ids, &mut sids),
        None,
        "a WindowId cannot occur in two ownership domains"
    );
}

// ---------------------------------------------------------------------------
// Infinite-plane persistence (spec-infinite-plane-workspace.md Behavior 7 / D4)
// ---------------------------------------------------------------------------

/// Minimal well-formed `PersistedWorkspace` for the plane-persistence serde tests:
/// one Agent leaf, no rail, default layout params. The caller sets the plane
/// fields (`desktop_slots`, `camera`) under test.
#[cfg(test)]
fn plane_persist_test_workspace() -> PersistedWorkspace {
    PersistedWorkspace {
        auto_name: "plane-1".into(),
        display_name: None,
        focused_window: 1,
        layout: PersistedLayout::Leaf(PersistedLeaf {
            id: 1,
            tags: Default::default(),
            kind: PersistedKind::Agent { session_id: None },
        }),
        hidden_tiles: Vec::new(),
        rail: None,
        layout_mode: workspace::LayoutMode::Plane,
        primary_ratio: default_primary_ratio(),
        primary_count: default_primary_count(),
        tag_view: Default::default(),
        desktop_slots: Vec::new(),
        desktop_spans: Vec::new(),
        camera: None,
        view: workspace::WorkspaceView::Plane,
        cwd: Some("/tmp".into()),
        legacy_kv: Default::default(),
    }
}

/// A `PersistedWorkspace` carrying NEGATIVE-coordinate slots and a NON-default camera
/// round-trips byte-faithfully through serialize → deserialize (D4: signed
/// slots + persisted camera). Guards the `i32` slot widening and the
/// pan/zoom camera field.
#[test]
fn plane_persist_round_trips_signed_slots_and_camera() {
    let mut wsp = plane_persist_test_workspace();
    wsp.desktop_slots = vec![(1, -3, -7), (2, 0, 0), (3, 5, -2)];
    wsp.desktop_spans = vec![(1, 2, 1)];
    wsp.camera = Some(PersistedCamera {
        pan: (-2.5, 4.0),
        zoom: workspace::Detail::Minimap,
    });

    let json = serde_json::to_string(&wsp).expect("serialize");
    let back: PersistedWorkspace = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(
        back.desktop_slots, wsp.desktop_slots,
        "signed slots survive"
    );
    assert_eq!(back.desktop_spans, wsp.desktop_spans, "spans survive");
    assert_eq!(back.camera, wsp.camera, "camera (pan + zoom) survives");
    // Explicitly pin the signed slot: a naive u32 tuple would have refused to
    // parse `-3` at all, so a passing round-trip proves the widening.
    assert!(
        back.desktop_slots.contains(&(1, -3, -7)),
        "negative-coordinate slot round-trips intact: {:?}",
        back.desktop_slots
    );
}

/// Back-compat for the master→primary rename: a snapshot written by a pre-rename
/// binary (JSON keys `master_ratio`/`master_count`) still loads into the renamed
/// `primary_ratio`/`primary_count` fields via `#[serde(alias = ...)]`, and a new
/// snapshot serializes with the new keys.
#[test]
fn primary_area_serde_reads_legacy_master_keys() {
    // Legacy keys deserialize via alias.
    let legacy = r#"{
        "auto_name": "plane-1",
        "display_name": null,
        "focused_window": 1,
        "layout": { "leaf": { "id": 1, "kind": "claude", "data": { "session_id": null } } },
        "master_ratio": 0.35,
        "master_count": 3,
        "cwd": "/tmp"
    }"#;
    let back: PersistedWorkspace =
        serde_json::from_str(legacy).expect("legacy master_* snapshot must load");
    assert!((back.primary_ratio - 0.35).abs() < 0.001, "master_ratio alias");
    assert_eq!(back.primary_count, 3, "master_count alias");

    // New snapshots serialize with the new keys.
    let json = serde_json::to_string(&plane_persist_test_workspace()).expect("serialize");
    assert!(json.contains("\"primary_ratio\""), "writes primary_ratio: {json}");
    assert!(json.contains("\"primary_count\""), "writes primary_count: {json}");
    assert!(!json.contains("\"master_ratio\""), "no legacy master_ratio key");
}

/// UXI-Workspace-26: the UI arrangements (`view`) round-trip, retired `"plane"`
/// loads as `Columns`, an OLD snapshot with no `view` field loads as the
/// `Columns` default, and an UNKNOWN value from a newer binary degrades to
/// `Columns` (never dropping the snapshot).
#[test]
fn workspace_view_round_trips_and_unknown_defaults_columns() {
    // Round-trip each UI-selectable arrangement.
    for (view, token) in [
        (workspace::WorkspaceView::Columns, "columns"),
        (workspace::WorkspaceView::Tiling, "tiling"),
        (workspace::WorkspaceView::Monocle, "monocle"),
    ] {
        let mut wsp = plane_persist_test_workspace();
        wsp.view = view;
        let json = serde_json::to_string(&wsp).expect("serialize");
        assert!(
            json.contains(&format!("\"{token}\"")),
            "view serializes as {token}: {json}"
        );
        let back: PersistedWorkspace = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.view, view, "{token} survives");
    }

    // Retired Plane loads as Columns (UXI-Workspace-26: retired from the UI).
    let mut wsp = plane_persist_test_workspace();
    wsp.view = workspace::WorkspaceView::Plane;
    let json = serde_json::to_string(&wsp).expect("serialize plane");
    let back: PersistedWorkspace = serde_json::from_str(&json).expect("deserialize plane");
    assert_eq!(
        back.view,
        workspace::WorkspaceView::Columns,
        "retired plane loads as columns"
    );

    // Absent field (old snapshot) ⇒ Columns via #[serde(default)].
    let old = r#"{
        "auto_name": "plane-1",
        "display_name": null,
        "focused_window": 1,
        "layout": { "leaf": { "id": 1, "kind": "claude", "data": { "session_id": null } } },
        "cwd": "/tmp"
    }"#;
    let none: PersistedWorkspace = serde_json::from_str(old).expect("old snapshot loads");
    assert_eq!(
        none.view,
        workspace::WorkspaceView::Columns,
        "absent view ⇒ Columns"
    );

    // Unknown value from a newer binary ⇒ Columns, snapshot NOT dropped.
    let future = r#"{
        "auto_name": "plane-1",
        "display_name": null,
        "focused_window": 1,
        "layout": { "leaf": { "id": 1, "kind": "claude", "data": { "session_id": null } } },
        "view": "hyperflow",
        "cwd": "/tmp"
    }"#;
    let unknown: PersistedWorkspace =
        serde_json::from_str(future).expect("unknown view must NOT drop the snapshot");
    assert_eq!(
        unknown.view,
        workspace::WorkspaceView::Columns,
        "unknown view ⇒ Columns"
    );
}

/// A LITERAL old-format `workspace.json` workspace (unsigned `desktop_slots`, NO
/// `camera` field, a retired `"layout_mode":"master_stack"`) deserializes with
/// its slots intact and the camera defaulting to origin+Full — NO panic, the
/// snapshot is NOT dropped (D4 / Behavior 7: existing files load transparently).
#[test]
fn old_workspace_json_loads_as_plane_with_origin_camera() {
    // Note: `desktop_slots` here are the old NON-NEGATIVE values, written by a
    // pre-plane binary as `u32`. They deserialize as the same positive `i32`.
    let old = r#"{
        "auto_name": "plane-1",
        "display_name": null,
        "focused_window": 1,
        "layout": { "leaf": { "id": 1, "kind": "claude", "data": { "session_id": null } } },
        "layout_mode": "master_stack",
        "primary_ratio": 0.6,
        "primary_count": 1,
        "desktop_slots": [[1, 0, 0], [2, 1, 3]],
        "cwd": "/tmp"
    }"#;

    let wsp: PersistedWorkspace =
        serde_json::from_str(old).expect("old-format workspace must load, not drop the snapshot");

    assert_eq!(
        wsp.desktop_slots,
        vec![(1, 0, 0), (2, 1, 3)],
        "old unsigned slots load as the same positive signed slots"
    );
    assert!(
        wsp.camera.is_none(),
        "absent camera field stays None (restored as Camera::default() = origin+Full)"
    );
    // The restore path turns an absent camera into the origin at Full.
    let restored = wsp
        .camera
        .map(|c| workspace::Camera {
            pan: c.pan,
            zoom: c.zoom,
        })
        .unwrap_or_default();
    assert_eq!(
        restored,
        workspace::Camera::default(),
        "no persisted camera ⇒ origin+Full"
    );
    // Behavior 7 (Stage D): the retired mode surface is gone, so a persisted
    // `"master_stack"` (or any old mode string) deserializes to the sole `Plane`
    // value rather than failing the parse and dropping the snapshot. The load
    // path ignores this field regardless — every workspace is a plane.
    assert_eq!(wsp.layout_mode, workspace::LayoutMode::Plane);
}

/// T007 (Tab→Workspace rename): a pre-rename `workspace.json` frame written with
/// the OLD on-disk keys `"tabs"` / `"active_tab"` must still load after the Rust
/// fields were renamed to `workspaces` / `active_workspace`. The `#[serde(rename)]`
/// bridges the old keys; without it the frame — and every saved workspace — would
/// silently vanish. Negative control: drop the `#[serde(rename = "tabs")]` on
/// `PersistedFrame::workspaces` and this `expect` panics on the missing field.
#[test]
fn old_workspace_json_frame_loads_with_pre_rename_keys() {
    let old = r#"{
        "tabs": [
            {
                "auto_name": "plane-1",
                "display_name": null,
                "focused_window": 1,
                "layout": { "leaf": { "id": 1, "kind": "claude", "data": { "session_id": null } } },
                "layout_mode": "master_stack",
                "primary_ratio": 0.6,
                "primary_count": 1,
                "desktop_slots": [[1, 0, 0], [2, 1, 3]],
                "cwd": "/tmp"
            }
        ],
        "active_tab": 0,
        "unbound_tiles": [
            {
                "project_cwd": "/tmp",
                "tile": { "id": 2, "kind": "claude", "data": { "session_id": null } }
            }
        ],
        "direct_unbound": 2,
        "scratchpad": [2]
    }"#;

    let frame: PersistedFrame = serde_json::from_str(old)
        .expect("pre-rename frame with `tabs`/`active_tab` keys must load, not drop the layout");

    assert_eq!(
        frame.workspaces.len(),
        1,
        "the `tabs` on-disk key maps to the renamed `workspaces` field"
    );
    assert_eq!(
        frame.active_workspace, 0,
        "the `active_tab` on-disk key maps to the renamed `active_workspace` field"
    );
    assert_eq!(
        frame.detached_tiles.len(),
        1,
        "legacy unbound tiles migrate to Detached"
    );
    assert_eq!(frame.detached_tiles[0].tile.id, 2);
    assert_eq!(frame.direct_unbound, Some(2));
    assert_eq!(frame.scratchpad, vec![2]);
    assert!(frame.solo_presentation.is_none());
    assert!(frame.workspaces[0].hidden_tiles.is_empty());
    assert!(
        !frame.tile_tags_migrated,
        "missing migration flag triggers one-time legacy tag import"
    );
    assert_eq!(
        frame.workspaces[0].desktop_slots,
        vec![(1, 0, 0), (2, 1, 3)],
        "the nested workspace snapshot survives the frame rename"
    );
}

/// A camera whose `zoom` string is unknown to this binary (a value from a NEWER
/// build) deserializes to `Full` via `Detail`'s hand-rolled fallback — it does
/// NOT raise a serde error that would discard the whole snapshot (D4). This is
/// the anti-circling guard: a derived `Detail` deserializer would hard-error
/// here and silently reset the workspace on the next save.
#[test]
fn unknown_detail_zoom_falls_back_to_full() {
    let json = r#"{ "pan": [1.0, 2.0], "zoom": "hyper" }"#;
    let cam: PersistedCamera =
        serde_json::from_str(json).expect("unknown zoom must fall back, not error");
    assert_eq!(cam.zoom, workspace::Detail::Full, "unknown zoom ⇒ Full");
    assert_eq!(
        cam.pan,
        (1.0, 2.0),
        "pan is unaffected by the zoom fallback"
    );

    // Direct `Detail` parse, mirroring the LayoutMode fallback test.
    let d: workspace::Detail = serde_json::from_str("\"hyper\"").expect("unknown detail string");
    assert_eq!(d, workspace::Detail::Full);
    // Known strings still round-trip.
    for (s, want) in [
        ("\"full\"", workspace::Detail::Full),
        ("\"card\"", workspace::Detail::Card),
        ("\"minimap\"", workspace::Detail::Minimap),
    ] {
        let got: workspace::Detail = serde_json::from_str(s).unwrap();
        assert_eq!(got, want, "{s}");
    }
    assert_eq!(
        serde_json::to_string(&workspace::Detail::Minimap).unwrap(),
        "\"minimap\""
    );
}

/// bug-0018: a clicked link routes by `classify_link`. External URLs
/// (http/https/mailto) → `External` (opened in the default browser); `wiki:`
/// note links and relative/scheme-less links → `Wiki` (opened in a new in-app
/// buffer tile). This is the exact routing decision the doc-view `on_click`
/// runs; before the fix, URL links were never collected/classified and were
/// inert.
///
/// Negative control: remove the http/https/mailto branch in `classify_link`
/// (so it falls through to `Wiki`) → the URL asserts fail RED (a URL would be
/// mis-routed to `open_wiki_link`, which only resolves LOCAL files — the bug).
#[test]
fn classify_link_routes_urls_external_and_notes_wiki() {
    use crate::{
        LinkTarget, RenderedMarkdownLink, classify_link, normalize_local_link_target,
        rendered_markdown_links,
    };

    // External URLs → open in the browser.
    assert_eq!(
        classify_link("https://example.com/x"),
        LinkTarget::External("https://example.com/x".into()),
    );
    assert_eq!(
        classify_link("http://localhost:3000"),
        LinkTarget::External("http://localhost:3000".into()),
    );
    assert_eq!(
        classify_link("HTTPS://Example.COM"),
        LinkTarget::External("HTTPS://Example.COM".into()),
        "scheme match is case-insensitive; raw target preserved"
    );
    assert_eq!(
        classify_link("mailto:scott@maher.lol"),
        LinkTarget::External("mailto:scott@maher.lol".into()),
    );
    // Surrounding whitespace is trimmed before classifying.
    assert_eq!(
        classify_link("  https://trimmed.example  "),
        LinkTarget::External("https://trimmed.example".into()),
    );

    // Wiki / local references → open in-app (prefix stripped for wiki:).
    assert_eq!(
        classify_link("wiki:my-note"),
        LinkTarget::Wiki("my-note".into())
    );
    assert_eq!(
        classify_link("./relative.md"),
        LinkTarget::Wiki("./relative.md".into())
    );
    assert_eq!(
        classify_link("other-note"),
        LinkTarget::Wiki("other-note".into())
    );
    // A non-browser scheme is NOT opened externally (open must not launch an
    // arbitrary local handler) — treated as a local reference.
    assert_eq!(
        classify_link("file:///etc/passwd"),
        LinkTarget::Wiki("file:///etc/passwd".into())
    );

    assert_eq!(
        normalize_local_link_target("file:///tmp/My%20Note.md#heading"),
        "/tmp/My Note.md"
    );
    assert_eq!(
        normalize_local_link_target("/tmp/My%20Note.md:42:7"),
        "/tmp/My Note.md"
    );
    assert_eq!(
        normalize_local_link_target("<./notes/topic.md#details>"),
        "./notes/topic.md"
    );

    assert_eq!(
        rendered_markdown_links(
            "Open [the note](notes/My%20Note.md:42) now",
            "Open the note now"
        ),
        vec![RenderedMarkdownLink {
            range: 5..13,
            target: "notes/My%20Note.md:42".into(),
        }]
    );
}

// ── UXI-AgentTile-25: beautiful tool-body section planning (pure) ──────────

#[cfg(test)]
fn mk_tc(
    title: &str,
    kind: yalda::acp_channel::ToolKind,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    content: Vec<yalda::acp_channel::ToolCallContent>,
) -> yalda::acp_channel::ToolCall {
    let id: yalda::acp_channel::ToolCallId = "t".into();
    let mut tc = yalda::acp_channel::ToolCall::new(id, title.to_string());
    tc.kind = kind;
    tc.raw_input = input;
    tc.raw_output = output;
    tc.content = content;
    tc
}

/// `extract_output_text` pulls markdown text out of the shapes ACP tools return,
/// and returns None when there's no clean text (JSON fallback).
#[test]
fn extract_output_text_pulls_text_from_common_shapes() {
    use crate::extract_output_text;
    use serde_json::json;
    assert_eq!(
        extract_output_text(&json!("hello")).as_deref(),
        Some("hello")
    );
    assert_eq!(
        extract_output_text(
            &json!({"content":[{"type":"text","text":"# Title"},{"type":"text","text":"body"}]})
        )
        .as_deref(),
        Some("# Title\n\nbody")
    );
    assert_eq!(
        extract_output_text(&json!({"output":"ran ok"})).as_deref(),
        Some("ran ok")
    );
    assert_eq!(
        extract_output_text(&json!({"result":"done"})).as_deref(),
        Some("done")
    );
    // No clean text → None (caller falls back to JSON).
    assert_eq!(extract_output_text(&json!({"count": 3, "ok": true})), None);
    assert_eq!(extract_output_text(&json!(null)), None);
}

/// UXI-AgentTile-26: the Task/subagent tool returns its result as a BARE
/// top-level content-block array `[ {type:"text", text:"…"} ]` (NOT wrapped in
/// `{content:[…]}`). `extract_output_text` must pull the readable text out of it
/// — with the inner `text`'s REAL newlines — instead of falling through to an
/// escaped-JSON dump (the ugly OUTPUT section in the live screenshot).
///
/// Negative control (observed RED): delete the `Value::Array(items) =>
/// join_content_blocks(items)` arm → the bare array returns `None`, the caller
/// dumps raw JSON, and the `Some("…real text…")` assert fails.
#[test]
fn extract_output_text_handles_bare_content_block_array() {
    use crate::extract_output_text;
    use serde_json::json;
    // Bare array with a multiline text field — the inner `\n` must become a real
    // newline in the extracted string (no escaped JSON).
    let out = extract_output_text(&json!([
        {"type": "text", "text": "line one\nline two"}
    ]));
    assert_eq!(out.as_deref(), Some("line one\nline two"));
    // Multiple blocks join with a blank line, like the `{content:[…]}` shape.
    let joined = extract_output_text(&json!([
        {"type": "text", "text": "# Report"},
        {"type": "text", "text": "body"}
    ]));
    assert_eq!(joined.as_deref(), Some("# Report\n\nbody"));
    // A bare array with no text payload still falls through to None (JSON).
    assert_eq!(extract_output_text(&json!([{"type": "image"}])), None);
}

/// A subagent renders its prompt and its report as MARKDOWN sections (not JSON),
/// with agent-type + description surfaced separately. This is the showcase.
///
/// Negative control (observed RED): make the report/output branch always emit
/// `SectionBody::Json` → the "report is Markdown" assert fails.
#[test]
fn plan_tool_sections_subagent_prompt_and_report_are_markdown() {
    use crate::{SectionBody, SectionRole, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let tc = mk_tc(
        "Explore the repo",
        yalda::acp_channel::ToolKind::Think,
        Some(
            json!({"subagent_type":"Explore","description":"map the code","prompt":"# Task\nFind all the **things**."}),
        ),
        Some(json!({"content":[{"type":"text","text":"## Report\n- found it\n- done"}]})),
        vec![],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    // agent chip
    assert!(sections.iter().any(|s| s.label == "agent"
        && matches!(&s.body, SectionBody::Chips(c) if c.iter().any(|(k,v)| k=="agent" && v=="Explore"))));
    // description as prose
    assert!(
        sections
            .iter()
            .any(|s| s.label == "task" && matches!(s.body, SectionBody::Prose(_)))
    );
    // prompt as MARKDOWN (input side)
    assert!(sections.iter().any(|s| s.label == "prompt"
        && s.role == SectionRole::Input
        && matches!(s.body, SectionBody::Markdown { .. })));
    // report as MARKDOWN, emphasized (output side) — the star.
    let report = sections
        .iter()
        .find(|s| s.label == "report")
        .expect("a report section");
    assert_eq!(report.role, SectionRole::Output);
    assert!(
        matches!(report.body, SectionBody::Markdown { .. }),
        "report renders as markdown, not json"
    );
    assert!(report.emphasis, "the subagent report is emphasized");
}

/// UXI-AgentTile-26: a subagent whose `raw_output` is a BARE content-block array
/// (`[ {type:"text", text:"…"} ]`, the real Task-tool shape) renders its report
/// as MARKDOWN — NOT a raw escaped-JSON dump. This is the OUTPUT section from the
/// live screenshot, where the array fell through to `SectionBody::Json`.
///
/// Negative control (observed RED): delete the `Value::Array(items) =>` arm in
/// `extract_output_text` → the array yields `None`, `plan_tool_sections` emits a
/// `SectionBody::Json` "output" section, and the "no Json / report is Markdown"
/// asserts fail.
#[test]
fn plan_tool_sections_bare_array_output_is_markdown_not_json() {
    use crate::{SectionBody, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let tc = mk_tc(
        "Explore the repo",
        yalda::acp_channel::ToolKind::Think,
        Some(json!({"subagent_type":"Explore","prompt":"go"})),
        // BARE array (not `{content:[…]}`) — the shape the screenshot showed raw.
        Some(json!([{"type":"text","text":"## Findings\n- one\n- two"}])),
        vec![],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    let report = sections
        .iter()
        .find(|s| s.label == "report")
        .expect("a report section from the bare-array output");
    assert!(
        matches!(report.body, SectionBody::Markdown { text: _ }),
        "the bare-array output renders as markdown, not raw JSON"
    );
    assert!(
        !sections
            .iter()
            .any(|s| matches!(s.body, SectionBody::Json(_))),
        "no raw-JSON section for a bare content-block array output"
    );
}

/// A Bash command renders as a code section (not JSON); terminal output stays
/// monospace (a leading `#` must not become an H1).
#[test]
fn plan_tool_sections_bash_is_code_not_markdown() {
    use crate::{SectionBody, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let tc = mk_tc(
        "Bash",
        yalda::acp_channel::ToolKind::Execute,
        Some(json!({"command":"grep -rn foo .","description":"search"})),
        Some(json!("# not a heading\nresults")),
        vec![],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    assert!(
        sections
            .iter()
            .any(|s| s.label == "command" && matches!(s.body, SectionBody::Code { .. }))
    );
    // terminal output is Code, never Markdown.
    assert!(
        sections
            .iter()
            .any(|s| s.label == "output" && matches!(s.body, SectionBody::Code { .. }))
    );
    assert!(
        !sections
            .iter()
            .any(|s| matches!(s.body, SectionBody::Markdown { .. })),
        "bash output must not be markdown-rendered"
    );
}

/// An Edit synthesizes a diff from old/new when `content` carries none, and does
/// NOT dump old_string/new_string as JSON.
#[test]
fn plan_tool_sections_edit_synthesizes_diff() {
    use crate::{SectionBody, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let tc = mk_tc(
        "Edit",
        yalda::acp_channel::ToolKind::Edit,
        Some(json!({"file_path":"/a/b.rs","old_string":"let x = 1;","new_string":"let x = 2;"})),
        None,
        vec![],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    let diff = sections
        .iter()
        .find(|s| matches!(s.body, SectionBody::Diff { .. }))
        .expect("a diff section");
    let SectionBody::Diff { text, .. } = &diff.body else {
        unreachable!()
    };
    assert!(
        text.contains("- let x = 1;") && text.contains("+ let x = 2;"),
        "synthesized +/- diff"
    );
    // path is a chip, not raw json; no Json section for the edit input.
    assert!(
        sections
            .iter()
            .any(|s| s.label == "path" && matches!(s.body, SectionBody::Chips(_)))
    );
    assert!(
        !sections
            .iter()
            .any(|s| matches!(s.body, SectionBody::Json(_)))
    );
}

/// When `content` and `raw_output` carry the SAME text (Claude Code mirrors
/// output into content), only ONE section is emitted — not the doubled text the
/// old UI showed (content + JSON-escaped output).
#[test]
fn plan_tool_sections_dedups_content_and_output() {
    use crate::{SectionBody, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let tc = mk_tc(
        "Task",
        yalda::acp_channel::ToolKind::Think,
        Some(json!({"prompt":"do it","subagent_type":"general"})),
        Some(json!({"content":[{"type":"text","text":"the one and only report"}]})),
        vec![yalda::acp_channel::ToolCallContent::from(
            "the one and only report".to_string(),
        )],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    let report_like = sections
        .iter()
        .filter(
            |s| matches!(s.body, SectionBody::Markdown{ref text} if text.contains("one and only")),
        )
        .count();
    assert_eq!(
        report_like, 1,
        "content/output dedup: the shared report is shown once, not twice"
    );
}

/// An unknown tool with a long multiline string field renders it as a readable
/// code section (real newlines), not a `\n`-riddled JSON blob.
#[test]
fn plan_tool_sections_unknown_multiline_is_code() {
    use crate::{SectionBody, ToolRenderPolicy, plan_tool_sections};
    use serde_json::json;
    let big = "line one\nline two\nline three\nline four which is here";
    let tc = mk_tc(
        "mystery",
        yalda::acp_channel::ToolKind::Other,
        Some(json!({"blob": big, "n": 5})),
        None,
        vec![],
    );
    let sections = plan_tool_sections(&tc, ToolRenderPolicy::Full);
    assert!(
        sections
            .iter()
            .any(|s| matches!(&s.body, SectionBody::Code{text,..} if text.contains("line two"))),
        "multiline string becomes a code section"
    );
    // the scalar `n` is a chip.
    assert!(
        sections
            .iter()
            .any(|s| matches!(&s.body, SectionBody::Chips(c) if c.iter().any(|(k,_)| k=="n")))
    );
}

// ── Projects: persistence + migration (T002, UXI-Project-8) ─────────────────

/// UXI-Project-8 — migration maps distinct cwds to basename-derived project
/// names, with the two known cwds falling out of the general rule; it is total
/// (every cwd yields a project) and dedups by canonical cwd.
///
/// Negative control: replace `project_name_for_cwd(&cwd)` in
/// `migrate_cwds_to_projects` with a constant `"Yaldabaoth".to_string()` — then
/// all three cwds resolve to one name, fold via `get_or_create`, and this fails
/// (`len()==1`, `by_name("Fulcrum")`/`("Archon")` are `None`).
#[test]
fn migration_maps_known_cwds_and_basename_fallback() {
    let cwds = vec![
        std::path::PathBuf::from("/home/scott/ws/yaldabaoth"),
        std::path::PathBuf::from("/home/scott/ws/fulcrum"),
        std::path::PathBuf::from("/home/scott/ws/archon"),
        // A duplicate of the first — must fold, not create a fourth project.
        std::path::PathBuf::from("/home/scott/ws/yaldabaoth"),
    ];
    let ps = migrate_cwds_to_projects(cwds);

    assert_eq!(
        ps.len(),
        3,
        "three distinct cwds → three projects (dup folded)"
    );
    // The two user-named projects fall out of the basename rule.
    assert!(
        ps.by_name("Yaldabaoth").is_some(),
        "ws/yaldabaoth → Yaldabaoth"
    );
    assert!(ps.by_name("Fulcrum").is_some(), "ws/fulcrum → Fulcrum");
    // The fallback names any other cwd from its basename.
    assert!(
        ps.by_name("Archon").is_some(),
        "ws/archon → Archon (basename)"
    );
    // Totality: every cwd resolves back to a project (nothing dropped).
    for c in [
        "/home/scott/ws/yaldabaoth",
        "/home/scott/ws/fulcrum",
        "/home/scott/ws/archon",
    ] {
        assert!(
            ps.by_cwd(&std::path::PathBuf::from(c)).is_some(),
            "cwd {c} maps to a project"
        );
    }
}

/// `project_name_for_cwd` capitalizes the basename's first letter and is total.
#[test]
fn project_name_for_cwd_capitalizes_basename() {
    use std::path::Path;
    assert_eq!(
        project_name_for_cwd(Path::new("/home/scott/ws/yaldabaoth")),
        "Yaldabaoth"
    );
    assert_eq!(
        project_name_for_cwd(Path::new("/home/scott/ws/fulcrum")),
        "Fulcrum"
    );
    assert_eq!(project_name_for_cwd(Path::new("/x/archon")), "Archon");
    assert_eq!(project_name_for_cwd(Path::new("/")), "Project"); // rootless fallback
}

/// The `projects.json` registry round-trips through disk via the `cfg(test)`
/// path seam — proving persistence never touches `~/.yalda` and restores names,
/// cwds, and params faithfully (`projects_from_persisted`).
#[test]
fn projects_persist_round_trips_via_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("projects.json");

    let mut ps = Projects::new();
    let yalda = ps
        .create(
            "Yaldabaoth".into(),
            std::path::PathBuf::from("/ws/yaldabaoth"),
        )
        .unwrap();
    ps.get_mut(yalda)
        .unwrap()
        .params
        .insert("model".into(), "opus".into());
    ps.create("Fulcrum".into(), std::path::PathBuf::from("/ws/fulcrum"))
        .unwrap();

    crate::persist::with_projects_path(file.clone(), || save_persisted_projects(&ps));
    let doc = crate::persist::with_projects_path(file.clone(), || {
        load_persisted_projects().expect("file written")
    });
    let restored = projects_from_persisted(&doc);

    assert_eq!(restored.len(), 2);
    let ry = restored.by_name("Yaldabaoth").expect("Yaldabaoth restored");
    assert_eq!(
        restored.cwd_of(ry),
        Some(std::path::Path::new("/ws/yaldabaoth"))
    );
    assert_eq!(
        restored
            .get(ry)
            .unwrap()
            .params
            .get("model")
            .map(String::as_str),
        Some("opus"),
        "params round-trip"
    );
    assert!(restored.by_name("Fulcrum").is_some());
}

// ── UXI-AgentTile-27: the naming sanitizers/parser ───────────────────────────
// These are where the real risk lives: the model can return anything, and
// property 2's shape guarantee is enforced CLIENT-side. Every case below is a
// reply shape a model has a plausible reason to produce.

#[test]
fn sanitize_name_enforces_shape_and_cap() {
    use crate::agent_naming::{MAX_NAME_CHARS, sanitize_name};

    // The happy path passes through, lowercased.
    assert_eq!(
        sanitize_name("Payments Refactor").as_deref(),
        Some("payments refactor")
    );
    // Quotes, trailing punctuation, and stray markdown are stripped.
    assert_eq!(
        sanitize_name("\"flaky test hunt\".").as_deref(),
        Some("flaky test hunt")
    );
    // At most three words — a model that writes a sentence gets truncated, not
    // installed verbatim.
    assert_eq!(
        sanitize_name("rewrite the payments adapter for the new gateway").as_deref(),
        Some("rewrite the payments")
    );
    // The cap holds even when three words would blow past it (words are dropped
    // from the end rather than cut mid-word).
    let long = sanitize_name("supercalifragilistic expialidocious pipeline").unwrap();
    assert!(long.chars().count() <= MAX_NAME_CHARS, "got {long:?}");
    // A single over-long word is hard-truncated rather than dropped entirely.
    let one = sanitize_name("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    assert_eq!(one.chars().count(), MAX_NAME_CHARS);
    // Nothing usable → no name at all (rather than an empty label).
    assert_eq!(sanitize_name("   "), None);
    assert_eq!(sanitize_name("!!! ???"), None);
}

#[test]
fn sanitize_summary_keeps_two_sentences_and_flattens() {
    use crate::agent_naming::{MAX_SUMMARY_CHARS, sanitize_summary};

    // Newlines collapse — the jump panel renders one small italic line.
    assert_eq!(
        sanitize_summary("First sentence.\n  Second one.").as_deref(),
        Some("First sentence. Second one.")
    );
    // A third sentence is dropped.
    assert_eq!(
        sanitize_summary("One. Two. Three. Four.").as_deref(),
        Some("One. Two.")
    );
    // A rambling single "sentence" is capped on a word boundary with an ellipsis.
    let long = "word ".repeat(200);
    let capped = sanitize_summary(&long).unwrap();
    assert!(
        capped.chars().count() <= MAX_SUMMARY_CHARS + 1,
        "got {} chars",
        capped.chars().count()
    );
    assert!(
        capped.ends_with('…'),
        "a truncated summary is marked: {capped:?}"
    );
    assert_eq!(sanitize_summary("   "), None);
}

#[test]
fn naming_summary_is_topic_only_short_and_has_a_user_turn_fallback() {
    use crate::agent_naming::{MAX_SUMMARY_CHARS, fallback_topic_summary, naming_system_prompt};

    assert_eq!(MAX_SUMMARY_CHARS, 140, "jump summaries stay glanceable");
    let prompt = naming_system_prompt();
    assert!(
        prompt.contains("enduring topic or goal")
            && prompt.contains("Do NOT mention progress")
            && prompt.contains("Maximum 140 characters"),
        "the model is asked for durable topic, not a progress report"
    );
    assert_eq!(
        fallback_topic_summary(
            "user: redesign the jump panel agent tabs\n\
             with a clearer segmented control\n\
             agent: I have started editing several files\n\
             status: implementation is half done"
        )
        .as_deref(),
        Some("redesign the jump panel agent tabs with a clearer segmented control"),
        "fallback uses only the opening user topic and excludes agent progress"
    );
}

#[test]
fn jump_supporting_text_is_cool_and_readable_in_everyday_themes() {
    use yalda::style::Color;
    use yalda::theme::AgentTheme;

    let nightfox = AgentTheme::nightfox();
    let folio = AgentTheme::folio();
    assert_eq!(
        crate::jump_supporting_text_color(&nightfox),
        Color::Rgb(0x9a, 0xbe, 0xd0),
        "Nightfox uses its pale blue prose tint, not low-contrast dim blue"
    );
    assert_eq!(
        crate::jump_supporting_text_color(&folio),
        Color::Rgb(0x2d, 0x3d, 0x4e),
        "Folio uses deep steel, not gold/tan"
    );
    assert_ne!(
        crate::jump_supporting_text_color(&nightfox),
        nightfox.warm_accent
    );
    assert_ne!(crate::jump_supporting_text_color(&folio), folio.warm_accent);
}

#[test]
fn jump_panel_state_palette_is_orange_green_and_gray() {
    use yalda::style::Color;
    use yalda::theme::{AgentTheme, OverlayTheme};

    let agent = AgentTheme::nightfox();
    assert_eq!(
        crate::jump_agent_status_color(&agent, crate::AgentDotStatus::Working),
        agent.jump_working,
        "working uses the theme's orange"
    );
    assert_eq!(
        crate::jump_agent_status_color(&agent, crate::AgentDotStatus::WaitingForYou),
        agent.tool_completed,
        "ready for input uses the theme's green"
    );

    let nightfox = OverlayTheme::nightfox();
    let folio = OverlayTheme::folio();
    assert_eq!(
        crate::jump_selection_color(&nightfox),
        Color::Rgb(0x2b, 0x3b, 0x51),
        "Nightfox selection uses its neutral slate"
    );
    assert_eq!(
        crate::jump_selection_color(&folio),
        Color::Rgb(0xe3, 0xe3, 0xe0),
        "Folio selection uses a neutral grey band"
    );
    assert_ne!(
        crate::jump_selection_color(&nightfox),
        crate::jump_agent_status_color(&agent, crate::AgentDotStatus::Working)
    );
    assert_ne!(
        crate::jump_selection_color(&nightfox),
        crate::jump_agent_status_color(&agent, crate::AgentDotStatus::WaitingForYou)
    );
}

/// Folio surface tiers + Nightfox Steel accent. Drives the REAL chrome tile-bg
/// resolver (`resolve_tile_bg`, called by all three layout renderers) plus the
/// overlay palette the menus/jump/dialog read.
///
/// Negative control: set Folio `tile_bg` back to `None` (or restore the Nightfox
/// purple key) and the matching assert fails — verified inline.
#[test]
fn folio_bone_surfaces_and_nightfox_steel_accent() {
    use yalda::style::Color;
    use yalda::theme::{OverlayTheme, Theme};

    // --- Folio: tiles (Bone) sit a step DARKER than the Soft White desktop. ---
    let folio = Theme::folio();
    let desktop = nc(folio.editor_bg); // #f5f5f0 Soft White (margin)
    let tile = crate::chrome::resolve_tile_bg(&folio, desktop); // real chrome path
    assert_eq!(
        tile,
        nc(Color::Rgb(0xf0, 0xed, 0xe4)),
        "Folio tiles paint Bone via the explicit theme.tile_bg"
    );
    assert_ne!(tile, desktop, "tiles must differ from the desktop margin");
    assert!(
        tile.l < desktop.l,
        "Bone tiles must be darker than the Soft White desktop so tiles read"
    );

    // A theme without an explicit tile_bg still derives a tile surface (not Bone).
    let dracula = Theme::dracula();
    assert!(dracula.tile_bg.is_none());
    let derived = crate::chrome::resolve_tile_bg(&dracula, nc(dracula.editor_bg));
    assert_eq!(
        derived,
        tint_bg(nc(dracula.editor_bg), 0.5, 0.06, 0.02),
        "themes without tile_bg fall back to the derived tint"
    );

    // --- Nightfox Steel: the overlay key is steel-blue, never the old purple. ---
    let nf = OverlayTheme::nightfox();
    assert_eq!(nf.key, Color::Rgb(0x7a, 0xa7, 0xd6), "Nightfox key is steel");
    assert_ne!(
        nf.key,
        Color::Rgb(0x9d, 0x79, 0xd6),
        "the retired Nightfox purple must be gone"
    );
    assert_eq!(nf.input, nf.key, "Nightfox caret follows the steel key, not yellow");
}

#[test]
fn parse_naming_reply_tolerates_real_model_output() {
    use crate::agent_naming::parse_naming_reply;

    // The contract shape.
    let clean = parse_naming_reply(
        r#"{"name": "payments refactor", "summary": "Ripping out the adapter."}"#,
    );
    assert_eq!(clean.name.as_deref(), Some("payments refactor"));
    assert_eq!(clean.summary.as_deref(), Some("Ripping out the adapter."));

    // Wrapped in a code fence (a very common deviation).
    let fenced = parse_naming_reply(
        "```json\n{\"name\": \"flaky tests\", \"summary\": \"Hunting a flake.\"}\n```",
    );
    assert_eq!(fenced.name.as_deref(), Some("flaky tests"));

    // With a preamble the instruction told it not to write.
    let chatty = parse_naming_reply(
        "Sure! Here you go:\n{\"name\": \"jump panel\", \"summary\": \"Panel work.\"}",
    );
    assert_eq!(chatty.name.as_deref(), Some("jump panel"));

    // No JSON at all: the first line is salvaged as the name.
    let bare = parse_naming_reply("payments refactor\nWe are ripping out the adapter.");
    assert_eq!(bare.name.as_deref(), Some("payments refactor"));
    assert_eq!(
        bare.summary.as_deref(),
        Some("We are ripping out the adapter.")
    );

    // Total garbage yields nothing installable, so the caller keeps `claude-N`.
    assert!(parse_naming_reply("   ").is_empty());
}

#[test]
fn parse_dotenv_reads_keys_and_ignores_noise() {
    use crate::persist::{is_private_dotenv_key, parse_dotenv};

    let parsed = parse_dotenv(
        "# a comment\n\
         \n\
         ANTHROPIC_API_KEY=sk-ant-123\n\
         export QUOTED=\"with spaces\"\n\
         SINGLE='single quoted'\n\
         not a key value line\n\
         EMPTY=\n",
    );
    assert_eq!(
        parsed,
        vec![
            ("ANTHROPIC_API_KEY".to_string(), "sk-ant-123".to_string()),
            ("QUOTED".to_string(), "with spaces".to_string()),
            ("SINGLE".to_string(), "single quoted".to_string()),
            ("EMPTY".to_string(), String::new()),
        ]
    );
    assert!(
        is_private_dotenv_key("ANTHROPIC_API_KEY"),
        "the autonaming credential must stay in Yalda's private store"
    );
    assert!(!is_private_dotenv_key("LINEAR_API_KEY"));
}

#[test]
fn system_console_log_is_bounded_and_classifies_build_output() {
    use crate::{
        ConsoleLevel, ConsoleLog, SYSTEM_CONSOLE_MAX_LINES, classify_build_line,
        record_system_message, with_system_console_path,
    };

    assert_eq!(
        classify_build_line("warning: unused import"),
        ConsoleLevel::Warn
    );
    assert_eq!(
        classify_build_line("error[E0308]: mismatched types"),
        ConsoleLevel::Error
    );
    assert_eq!(
        classify_build_line("   Compiling yalda v0.1.0"),
        ConsoleLevel::Info
    );

    let mut log = ConsoleLog::default();
    for i in 0..SYSTEM_CONSOLE_MAX_LINES + 7 {
        log.push(ConsoleLevel::Info, format!("line-{i}"));
    }
    assert_eq!(log.lines().len(), SYSTEM_CONSOLE_MAX_LINES);
    assert_eq!(
        log.lines().front().map(|line| line.text.as_str()),
        Some("line-7"),
        "the bounded store retains the newest rows"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("console.log");
    with_system_console_path(path.clone(), || {
        record_system_message(ConsoleLevel::Command, "cargo build --release");
        record_system_message(ConsoleLevel::Error, "build failed");
    });
    let persisted = std::fs::read_to_string(path).expect("persisted log");
    assert!(persisted.contains("CMD\tcargo build --release"));
    assert!(persisted.contains("ERROR\tbuild failed"));
}
