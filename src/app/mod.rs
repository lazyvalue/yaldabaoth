use sketch::menu::MenuNode;

mod claude;
mod dispatch;
mod handlers;
mod runtime;
mod screen;
mod state;
pub use state::App;
use state::{AppMode, AppScreen};
#[cfg(test)]
use claude::CLAUDE_BUFFER_NAME;

// Re-imports for tests (picked up via `use super::*`).
#[cfg(test)]
use sketch::buffer::{Buffer, NavMode};
#[cfg(test)]
use sketch::keybind::Action;
#[cfg(test)]
use sketch::view::ViewMode;


/// Convert a rope char index to (line, col).
fn char_to_line_col(doc: &sketch::document::Document, char_idx: usize) -> (usize, usize) {
    let rope = doc.rope();
    let len = rope.len_chars();
    let i = char_idx.min(len);
    let line = rope.char_to_line(i);
    let line_start = rope.line_to_char(line);
    (line, i - line_start)
}

fn fuzzy_match(text: &str, query: &str) -> bool {
    let mut text_chars = text.chars();
    for qc in query.chars() {
        loop {
            match text_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

/// Merge user menu nodes on top of defaults.
/// User entries with the same key at the same level replace the default entry.
/// New entries are appended.
fn merge_menu(mut defaults: Vec<MenuNode>, user_nodes: &[MenuNode]) -> Vec<MenuNode> {
    for user_node in user_nodes {
        if user_node.key.is_empty() {
            // Separator or label — just append
            defaults.push(user_node.clone());
            continue;
        }
        if let Some(pos) = defaults.iter().position(|d| d.key == user_node.key) {
            defaults[pos] = user_node.clone();
        } else {
            defaults.push(user_node.clone());
        }
    }
    defaults
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketch::config::Config;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    fn fresh_socket() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "sketch-attach-test-{}-{}.sock",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn attach_creates_and_switches_to_claude_buffer() {
        let sock = fresh_socket();
        let listener = UnixListener::bind(&sock).expect("bind");
        // Accept-and-park so the client connect succeeds without immediate EOF.
        let sock_for_thread = sock.clone();
        let _accept = thread::spawn(move || {
            let _ = listener.accept();
            // Hold the connection open until the test ends.
            thread::sleep(Duration::from_secs(2));
            let _ = std::fs::remove_file(&sock_for_thread);
        });
        thread::sleep(Duration::from_millis(50));

        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        assert_eq!(app.buffers.len(), 1, "starts with one buffer");
        assert_eq!(app.active_buffer, 0);

        app.attach_claude_channel(sock.to_str().unwrap());

        assert!(
            app.command_error.starts_with("Attached to Claude channel:"),
            "expected success status, got: {}",
            app.command_error
        );
        assert_eq!(app.buffers.len(), 2, "claude buffer should be created");
        let claude_idx = app
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
            .expect("claude buffer must exist");
        assert_eq!(
            app.active_buffer, claude_idx,
            "active_buffer must switch to claude buffer"
        );
    }

    /// Build an app with enough rendered content to exercise scrolling.
    fn rendered_app() -> App {
        let md = (0..200)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = Config::default();
        let mut app = App::new("/tmp/scroll.md".into(), md, &cfg);
        app.last_viewport_height = 24;
        app.last_wrap_width = 80;
        app.buffers[app.active_buffer].rebuild_render_cache(&app.theme);
        app.buffers[app.active_buffer].update_total_lines(80);
        assert_eq!(app.buffers[app.active_buffer].view_mode, ViewMode::Rendered);
        app
    }

    /// Regression: in Rendered mode, the post-keystroke auto-pin used to
    /// translate the (stale) doc cursor into a rendered y, snapping
    /// `scroll_offset` back to 0 on every j/k and pushing the visible
    /// cursor off the bottom. It must follow `rendered_cursor_row`.
    #[test]
    fn ensure_cursor_visible_in_rendered_mode_follows_rendered_cursor_row() {
        let mut app = rendered_app();
        // Walk the rendered cursor below the initial viewport.
        app.buffers[app.active_buffer].rendered_cursor_row = 100;
        // Doc cursor untouched — exactly the divergence that triggered the bug.
        assert_eq!(app.buffers[app.active_buffer].editor.cursor().line, 0);

        let vh = 24;
        app.ensure_cursor_visible(vh);

        let off = app.buffers[app.active_buffer].viewport.scroll_offset;
        assert!(
            100 >= off && 100 < off + vh,
            "rendered cursor at row 100 must sit inside viewport [{off}, {}); got scroll_offset {off}",
            off + vh
        );
    }

    /// Regression: ctrl-d / ctrl-u in Rendered mode used to move only
    /// `editor.cursor().line` (the raw-mode doc cursor), which isn't
    /// displayed there — the visible cursor stayed put and the action
    /// looked dead. Must move `rendered_cursor_row` instead.
    #[test]
    fn page_move_cursor_in_rendered_mode_moves_rendered_cursor_row() {
        let mut app = rendered_app();
        let pre_doc = app.buffers[app.active_buffer].editor.cursor().line;
        let pre_row = app.buffers[app.active_buffer].rendered_cursor_row;

        app.page_move_cursor(12, true);

        let post_doc = app.buffers[app.active_buffer].editor.cursor().line;
        let post_row = app.buffers[app.active_buffer].rendered_cursor_row;
        assert_eq!(
            post_doc, pre_doc,
            "doc cursor must stay put in Rendered mode (was {pre_doc}, now {post_doc})"
        );
        assert_eq!(
            post_row,
            pre_row + 12,
            "rendered_cursor_row must advance by N on ctrl-d"
        );

        // ctrl-u walks back.
        app.page_move_cursor(12, false);
        assert_eq!(
            app.buffers[app.active_buffer].rendered_cursor_row, pre_row,
            "ctrl-u must reverse the move"
        );
    }

    /// Raw mode keeps moving the doc cursor — page motions there must NOT
    /// regress to touching `rendered_cursor_row`.
    #[test]
    fn page_move_cursor_in_raw_mode_moves_doc_cursor() {
        let mut app = rendered_app();
        app.buffers[app.active_buffer].view_mode = ViewMode::Raw;
        app.buffers[app.active_buffer].update_total_lines(80);
        let pre_row = app.buffers[app.active_buffer].rendered_cursor_row;

        app.page_move_cursor(12, true);

        assert_eq!(
            app.buffers[app.active_buffer].editor.cursor().line,
            12,
            "doc cursor must advance by N in Raw mode"
        );
        assert_eq!(
            app.buffers[app.active_buffer].rendered_cursor_row, pre_row,
            "rendered_cursor_row must stay put in Raw mode"
        );
    }

    /// Build an App with a *claude* buffer that already contains:
    ///   prior locked turn -> "\n\n---\n\n" -> caret position (lockable_through here)
    /// then drops a user draft into the editable region. Returns (app, buf_idx,
    /// draft_start_char). After this the layout looks like:
    ///   "old turn\n\n---\n\n[draft]"
    /// with everything up through the HR locked.
    fn claude_app_with_draft(draft: &str) -> (App, usize, usize) {
        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        let buf_idx = app.or_create_claude_buffer();
        app.active_buffer = buf_idx;

        // Seed prior turn + HR + lock through it.
        let pre = "old turn\n\n---\n\n";
        {
            let editor = &mut app.buffers[buf_idx].editor;
            editor.programmatic_insert(0, pre);
            let eof = editor.document().rope().len_chars();
            let (cl, _) = char_to_line_col(editor.document(), eof);
            editor.set_lockable_through_line(cl);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = 0;
        }
        // Now type a draft.
        let draft_start = {
            let editor = &mut app.buffers[buf_idx].editor;
            let s = editor.document().rope().len_chars();
            editor.programmatic_insert(s, draft);
            s
        };
        // Cursor at end of draft (most realistic mid-typing position).
        {
            let editor = &mut app.buffers[buf_idx].editor;
            let eof = editor.document().rope().len_chars();
            let (cl, cc) = char_to_line_col(editor.document(), eof);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = cc;
        }
        (app, buf_idx, draft_start)
    }

    /// Regression: a Claude reply landing while the user has a draft typed
    /// must splice ABOVE the draft — not append at EOF below it. This is the
    /// "interleaving" behavior promised by the doc-comment on
    /// `append_to_claude_buffer`.
    #[test]
    fn claude_reply_splices_above_pending_draft() {
        let (mut app, buf_idx, _) = claude_app_with_draft("my draft text");
        app.append_to_claude_buffer("REPLY LINE 1\nREPLY LINE 2");

        let text = app.buffers[buf_idx].editor.document().full_text();
        let reply_pos = text.find("REPLY LINE 1").expect("reply must be present");
        let draft_pos = text.find("my draft text").expect("draft must be preserved");
        assert!(
            reply_pos < draft_pos,
            "reply must land ABOVE the draft\n--- buffer ---\n{text}\n--------------"
        );
    }

    /// Regression: after splicing a reply above the draft, the cursor must
    /// stay at the same character offset within the draft — so the user's
    /// in-progress sentence "follows" the text down rather than getting
    /// stranded inside the new frozen reply.
    #[test]
    fn claude_reply_keeps_cursor_on_same_draft_character() {
        let (mut app, buf_idx, _) = claude_app_with_draft("my draft text");

        // Cursor was placed at end of draft; capture the character it sits on
        // (well, the one just before — end-of-text).
        let before_text = app.buffers[buf_idx].editor.document().full_text();
        let before_cursor_char = {
            let e = &app.buffers[buf_idx].editor;
            e.document().line_col_to_char(e.cursor().line, e.cursor().col)
        };
        assert_eq!(&before_text[before_cursor_char.saturating_sub(4)..before_cursor_char], "text");

        app.append_to_claude_buffer("REPLY");

        let after_text = app.buffers[buf_idx].editor.document().full_text();
        let after_cursor_char = {
            let e = &app.buffers[buf_idx].editor;
            e.document().line_col_to_char(e.cursor().line, e.cursor().col)
        };
        assert_eq!(
            &after_text[after_cursor_char.saturating_sub(4)..after_cursor_char],
            "text",
            "cursor must still be sitting just past 'text' in the draft\n--- buffer ---\n{after_text}\n--------------"
        );
    }

    /// Pre-existing behavior must still work: when there's no draft, the
    /// reply just lands at EOF and the cursor follows.
    #[test]
    fn claude_reply_with_no_draft_lands_at_eof() {
        let (mut app, buf_idx, _) = claude_app_with_draft("");
        app.append_to_claude_buffer("REPLY");
        let text = app.buffers[buf_idx].editor.document().full_text();
        assert!(text.contains("REPLY"));
        assert!(!text.contains("my draft"));
    }

    // ====================================================================
    // Action-dispatch safety net (precondition for the app.rs split — see
    // docs/refactor-roadmap.md §1). The split is mechanical but big; these
    // fixtures pin down current behavior of `execute_action` per family so
    // a regression during the move shows up as a failing test.
    // ====================================================================

    /// Build a Raw-mode app with N short plain-text lines.
    fn raw_app(n: usize) -> App {
        let md = (0..n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = Config::default();
        let mut app = App::new("/tmp/dispatch.md".into(), md, &cfg);
        app.last_viewport_height = 24;
        app.last_wrap_width = 80;
        app.buffers[app.active_buffer].view_mode = ViewMode::Raw;
        app.buffers[app.active_buffer].update_total_lines(80);
        app
    }

    /// Fire `action` against the cached vh/wrap-width — the same path
    /// the real input loop takes after `compute_viewport_height`.
    fn fire(app: &mut App, action: Action) {
        let vh = app.last_viewport_height;
        let cw = app.last_wrap_width;
        app.execute_action(action, vh, cw);
    }

    fn cur_line(app: &App) -> usize {
        app.buffers[app.active_buffer].editor.cursor().line
    }
    fn cur_col(app: &App) -> usize {
        app.buffers[app.active_buffer].editor.cursor().col
    }
    fn line_text(app: &App, n: usize) -> String {
        app.buffers[app.active_buffer]
            .editor
            .document()
            .line_text(n)
            .trim_end_matches('\n')
            .to_string()
    }
    fn full_text(app: &App) -> String {
        app.buffers[app.active_buffer].editor.document().full_text()
    }

    // ----- Motion (Raw / Edit Mode) ------------------------------------

    #[test]
    fn move_down_in_raw_advances_cursor_line() {
        let mut app = raw_app(5);
        assert_eq!(cur_line(&app), 0);
        fire(&mut app, Action::MoveDown);
        assert_eq!(cur_line(&app), 1);
    }

    #[test]
    fn move_up_in_raw_retreats_cursor_line() {
        let mut app = raw_app(5);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 3;
        fire(&mut app, Action::MoveUp);
        assert_eq!(cur_line(&app), 2);
    }

    #[test]
    fn move_up_at_top_clamps_to_zero() {
        let mut app = raw_app(5);
        fire(&mut app, Action::MoveUp);
        assert_eq!(cur_line(&app), 0);
    }

    #[test]
    fn move_right_in_raw_advances_col() {
        let mut app = raw_app(2);
        fire(&mut app, Action::MoveRight);
        assert_eq!(cur_col(&app), 1);
    }

    #[test]
    fn move_left_at_col_0_stays_put() {
        let mut app = raw_app(2);
        fire(&mut app, Action::MoveLeft);
        assert_eq!(cur_col(&app), 0);
    }

    #[test]
    fn move_word_forward_jumps_past_word() {
        // "line 0" → from col 0 (`l`), word-forward should land on `0`.
        let mut app = raw_app(2);
        fire(&mut app, Action::MoveWordForward);
        assert!(
            cur_col(&app) > 0,
            "word-forward must advance past 'line', got col {}",
            cur_col(&app)
        );
    }

    #[test]
    fn move_line_end_jumps_to_eol() {
        let mut app = raw_app(2);
        fire(&mut app, Action::MoveLineEnd);
        // "line 0" is 6 chars; cursor lands at col 5 (last char) per
        // normal-mode clamping (insert_mode=false in execute_action).
        assert!(
            cur_col(&app) >= 5,
            "line-end must reach eol of 'line 0' (col >= 5), got {}",
            cur_col(&app)
        );
    }

    #[test]
    fn move_line_start_zeros_col() {
        let mut app = raw_app(2);
        app.buffers[app.active_buffer].editor.cursor_mut().col = 4;
        fire(&mut app, Action::MoveLineStart);
        assert_eq!(cur_col(&app), 0);
    }

    #[test]
    fn jump_top_returns_to_line_0() {
        let mut app = raw_app(20);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 10;
        fire(&mut app, Action::JumpTop);
        assert_eq!(cur_line(&app), 0);
    }

    #[test]
    fn jump_bottom_advances_to_last_line() {
        let mut app = raw_app(20);
        fire(&mut app, Action::JumpBottom);
        assert_eq!(cur_line(&app), 19);
    }

    // ----- Edit (Raw mode) --------------------------------------------

    #[test]
    fn insert_mode_enters_insert_app_mode() {
        let mut app = raw_app(2);
        assert_eq!(app.mode, AppMode::Normal);
        fire(&mut app, Action::InsertMode);
        assert_eq!(app.mode, AppMode::Insert);
    }

    #[test]
    fn insert_mode_in_rendered_view_flips_to_raw_then_inserts() {
        let mut app = raw_app(2);
        app.buffers[app.active_buffer].view_mode = ViewMode::Rendered;
        fire(&mut app, Action::InsertMode);
        assert_eq!(
            app.buffers[app.active_buffer].view_mode,
            ViewMode::Raw,
            "ensure_raw_for_editing must flip view to Raw before insert"
        );
        assert_eq!(app.mode, AppMode::Insert);
    }

    #[test]
    fn insert_after_advances_col_then_enters_insert() {
        let mut app = raw_app(2);
        fire(&mut app, Action::InsertAfter);
        assert_eq!(app.mode, AppMode::Insert);
        assert_eq!(cur_col(&app), 1, "InsertAfter (`a`) must shift col by 1");
    }

    #[test]
    fn open_line_below_inserts_blank_line_and_enters_insert() {
        let mut app = raw_app(2);
        fire(&mut app, Action::OpenLineBelow);
        assert_eq!(app.mode, AppMode::Insert);
        assert_eq!(
            line_text(&app, 0),
            "line 0",
            "original line 0 must be preserved"
        );
        assert_eq!(
            line_text(&app, 1),
            "",
            "new blank line must be spliced after line 0"
        );
        assert_eq!(line_text(&app, 2), "line 1");
        assert_eq!(cur_line(&app), 1, "cursor must land on the new blank line");
    }

    #[test]
    fn open_line_above_inserts_blank_line_and_enters_insert() {
        let mut app = raw_app(2);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 1;
        fire(&mut app, Action::OpenLineAbove);
        assert_eq!(app.mode, AppMode::Insert);
        assert_eq!(line_text(&app, 0), "line 0");
        assert_eq!(line_text(&app, 1), "", "new blank line above line 1");
        assert_eq!(line_text(&app, 2), "line 1");
        assert_eq!(cur_line(&app), 1);
    }

    #[test]
    fn delete_char_removes_char_at_cursor() {
        let mut app = raw_app(1);
        // "line 0" → delete at col 0 removes 'l' → "ine 0".
        fire(&mut app, Action::DeleteChar);
        assert_eq!(line_text(&app, 0), "ine 0");
    }

    #[test]
    fn delete_line_drops_current_line() {
        let mut app = raw_app(3);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 1;
        fire(&mut app, Action::DeleteLine);
        assert_eq!(
            app.buffers[app.active_buffer]
                .editor
                .document()
                .line_count(),
            2
        );
        assert_eq!(line_text(&app, 0), "line 0");
        assert_eq!(line_text(&app, 1), "line 2");
    }

    #[test]
    fn undo_restores_pre_delete_state() {
        let mut app = raw_app(3);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 1;
        fire(&mut app, Action::DeleteLine);
        assert_eq!(line_text(&app, 1), "line 2");
        fire(&mut app, Action::Undo);
        assert_eq!(line_text(&app, 1), "line 1");
        assert_eq!(line_text(&app, 2), "line 2");
    }

    #[test]
    fn redo_reapplies_undone_change() {
        let mut app = raw_app(3);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 1;
        fire(&mut app, Action::DeleteLine);
        fire(&mut app, Action::Undo);
        fire(&mut app, Action::Redo);
        assert_eq!(line_text(&app, 1), "line 2");
    }

    // ----- Mode / screen transitions ----------------------------------

    #[test]
    fn enter_command_clears_buffers_and_enters_command_mode() {
        let mut app = raw_app(1);
        app.command_buffer = "stale".into();
        app.command_error = "old error".into();
        fire(&mut app, Action::EnterCommand);
        assert_eq!(app.mode, AppMode::Command);
        assert!(app.command_buffer.is_empty());
        assert!(app.command_error.is_empty());
    }

    #[test]
    fn open_menu_enters_menu_mode() {
        let mut app = raw_app(1);
        fire(&mut app, Action::OpenMenu);
        assert_eq!(app.mode, AppMode::Menu);
    }

    #[test]
    fn outline_enters_outline_mode_and_resets_filter() {
        let mut app = raw_app(1);
        app.outline_filter_text = "stale".into();
        app.outline_filter_mode = true;
        fire(&mut app, Action::Outline);
        assert_eq!(app.mode, AppMode::Outline);
        assert!(app.outline_filter_text.is_empty());
        assert!(!app.outline_filter_mode);
    }

    #[test]
    fn buffer_list_routes_to_buffer_list_screen() {
        let mut app = raw_app(1);
        assert_eq!(app.screen, AppScreen::Editor);
        fire(&mut app, Action::BufferList);
        assert_eq!(app.screen, AppScreen::BufferList);
    }

    #[test]
    fn open_file_browser_full_routes_to_file_browser_screen() {
        let mut app = raw_app(1);
        fire(&mut app, Action::OpenFileBrowserFull);
        assert!(matches!(app.screen, AppScreen::FileBrowser { .. }));
        assert!(app.file_browser.is_some());
    }

    #[test]
    fn open_file_browser_inline_enters_file_browser_mode() {
        let mut app = raw_app(1);
        fire(&mut app, Action::OpenFileBrowser);
        assert_eq!(app.mode, AppMode::FileBrowser);
        assert!(app.file_browser.is_some());
    }

    #[test]
    fn toggle_view_flips_view_mode_both_directions() {
        let mut app = raw_app(1);
        assert_eq!(app.buffers[app.active_buffer].view_mode, ViewMode::Raw);
        fire(&mut app, Action::ToggleView);
        assert_eq!(app.buffers[app.active_buffer].view_mode, ViewMode::Rendered);
        fire(&mut app, Action::ToggleView);
        assert_eq!(app.buffers[app.active_buffer].view_mode, ViewMode::Raw);
    }

    #[test]
    fn force_quit_sets_should_quit_unconditionally() {
        let mut app = raw_app(1);
        // Modify the document so a plain Quit would balk.
        app.buffers[app.active_buffer].editor.programmatic_insert(0, "x");
        assert!(!app.should_quit);
        fire(&mut app, Action::ForceQuit);
        assert!(app.should_quit);
    }

    #[test]
    fn quit_with_modified_buffer_blocks_and_sets_error() {
        let mut app = raw_app(1);
        app.buffers[app.active_buffer].editor.programmatic_insert(0, "x");
        fire(&mut app, Action::Quit);
        assert!(!app.should_quit);
        assert!(
            app.command_error.contains("No write since last change"),
            "expected vim-style 'no write' error, got: {}",
            app.command_error
        );
    }

    // ----- Page motion -------------------------------------------------

    #[test]
    fn full_page_down_in_raw_advances_doc_cursor_by_vh() {
        let mut app = raw_app(100);
        let pre = cur_line(&app);
        fire(&mut app, Action::FullPageDown);
        assert_eq!(cur_line(&app), pre + app.last_viewport_height);
    }

    #[test]
    fn half_page_down_in_raw_advances_by_half_vh() {
        let mut app = raw_app(100);
        let pre = cur_line(&app);
        fire(&mut app, Action::HalfPageDown);
        assert_eq!(cur_line(&app), pre + app.last_viewport_height / 2);
    }

    #[test]
    fn full_page_up_in_raw_retreats_by_vh() {
        let mut app = raw_app(100);
        app.buffers[app.active_buffer].editor.cursor_mut().line = 50;
        fire(&mut app, Action::FullPageUp);
        assert_eq!(cur_line(&app), 50 - app.last_viewport_height);
    }

    #[test]
    fn scroll_down_advances_scroll_offset_only() {
        let mut app = raw_app(100);
        let pre_line = cur_line(&app);
        let pre_off = app.buffers[app.active_buffer].viewport.scroll_offset;
        fire(&mut app, Action::ScrollDown);
        assert_eq!(cur_line(&app), pre_line, "ScrollDown must not move cursor");
        assert!(
            app.buffers[app.active_buffer].viewport.scroll_offset > pre_off,
            "ScrollDown must advance scroll_offset"
        );
    }

    // ----- Headings ---------------------------------------------------

    #[test]
    fn set_heading_1_prefixes_line_with_one_hash() {
        let mut app = raw_app(2);
        fire(&mut app, Action::SetHeading1);
        assert_eq!(line_text(&app, 0), "# line 0");
    }

    #[test]
    fn set_heading_3_replaces_existing_heading_marker() {
        let mut app = raw_app(2);
        fire(&mut app, Action::SetHeading1);
        assert_eq!(line_text(&app, 0), "# line 0");
        fire(&mut app, Action::SetHeading3);
        assert_eq!(
            line_text(&app, 0),
            "### line 0",
            "set-heading must replace existing #s, not stack"
        );
    }

    #[test]
    fn clear_heading_strips_hashes() {
        let mut app = raw_app(2);
        fire(&mut app, Action::SetHeading2);
        assert_eq!(line_text(&app, 0), "## line 0");
        fire(&mut app, Action::ClearHeading);
        assert_eq!(line_text(&app, 0), "line 0");
    }

    // ----- Selection (Helix-style) ------------------------------------

    #[test]
    fn toggle_extend_mode_anchors_at_cursor() {
        let mut app = raw_app(2);
        assert!(
            app.buffers[app.active_buffer]
                .editor
                .selection_anchor()
                .is_none()
        );
        fire(&mut app, Action::ToggleExtendMode);
        assert!(app.buffers[app.active_buffer].editor.extend_mode());
        assert!(
            app.buffers[app.active_buffer]
                .editor
                .selection_anchor()
                .is_some(),
            "ToggleExtendMode must anchor when no selection exists"
        );
    }

    #[test]
    fn select_all_selects_entire_buffer() {
        let mut app = raw_app(3);
        fire(&mut app, Action::SelectAll);
        let text = full_text(&app);
        let sel = app.buffers[app.active_buffer]
            .editor
            .selection_text()
            .expect("SelectAll must produce a selection");
        assert_eq!(sel, text);
    }

    #[test]
    fn extend_by_line_grows_selection() {
        let mut app = raw_app(3);
        fire(&mut app, Action::ExtendByLine);
        let sel = app.buffers[app.active_buffer]
            .editor
            .selection_text()
            .expect("ExtendByLine must produce a selection");
        assert!(
            sel.contains("line 0"),
            "extend-by-line must include the cursor's line, got {sel:?}"
        );
    }

    #[test]
    fn delete_selection_with_no_anchor_falls_back_to_delete_char() {
        let mut app = raw_app(1);
        // No anchor — must behave like DeleteChar.
        fire(&mut app, Action::DeleteSelection);
        assert_eq!(line_text(&app, 0), "ine 0");
    }

    #[test]
    fn collapse_selection_clears_anchor() {
        let mut app = raw_app(3);
        fire(&mut app, Action::SelectAll);
        assert!(
            app.buffers[app.active_buffer]
                .editor
                .selection_anchor()
                .is_some()
        );
        fire(&mut app, Action::CollapseSelection);
        assert!(
            app.buffers[app.active_buffer]
                .editor
                .selection_anchor()
                .is_none()
        );
    }

    #[test]
    fn change_selection_deletes_then_enters_insert_mode() {
        let mut app = raw_app(2);
        fire(&mut app, Action::ExtendByLine);
        fire(&mut app, Action::ChangeSelection);
        assert_eq!(app.mode, AppMode::Insert);
        // The line that was selected should be gone (or partially gone).
        assert!(
            !full_text(&app).starts_with("line 0\nline 1"),
            "ChangeSelection must delete the selected text first; full_text={:?}",
            full_text(&app)
        );
    }

    // ----- Buffer management ------------------------------------------

    #[test]
    fn next_buffer_with_one_buffer_is_no_op() {
        let mut app = raw_app(1);
        assert_eq!(app.buffers.len(), 1);
        fire(&mut app, Action::NextBuffer);
        assert_eq!(app.active_buffer, 0);
    }

    #[test]
    fn next_prev_buffer_cycles_with_two_buffers() {
        let mut app = raw_app(1);
        // Add a synthetic second buffer.
        let cfg = Config::default();
        let buf = Buffer::new(
            "/tmp/other.md".into(),
            "hello".into(),
            cfg.max_line_width,
            &app.theme,
        );
        app.buffers.push(buf);
        assert_eq!(app.active_buffer, 0);
        fire(&mut app, Action::NextBuffer);
        assert_eq!(app.active_buffer, 1);
        fire(&mut app, Action::NextBuffer);
        assert_eq!(app.active_buffer, 0, "NextBuffer wraps");
        fire(&mut app, Action::PrevBuffer);
        assert_eq!(app.active_buffer, 1, "PrevBuffer wraps backward");
    }

    // ----- Rendered-mode navigation -----------------------------------

    #[test]
    fn nav_character_sets_character_nav_mode() {
        let mut app = raw_app(1);
        app.buffers[app.active_buffer].view_mode = ViewMode::Rendered;
        fire(&mut app, Action::NavCharacter);
        assert_eq!(
            app.buffers[app.active_buffer].nav_mode,
            NavMode::Character
        );
    }

    #[test]
    fn nav_cycle_advances_nav_mode() {
        let mut app = raw_app(1);
        app.buffers[app.active_buffer].view_mode = ViewMode::Rendered;
        let before = app.buffers[app.active_buffer].nav_mode;
        fire(&mut app, Action::NavCycle);
        let after = app.buffers[app.active_buffer].nav_mode;
        assert_ne!(before, after, "NavCycle must advance to a different mode");
    }

    // ----- Command-registry path (`dispatch_command`) -----------------

    #[test]
    fn dispatch_command_unknown_sets_command_error() {
        let mut app = raw_app(1);
        app.dispatch_command("nonsense-cmd-xyz", 24, 80);
        assert!(
            app.command_error.starts_with("Unknown command:"),
            "unknown command must set 'Unknown command:' error, got: {}",
            app.command_error
        );
    }

    #[test]
    fn dispatch_command_force_quit_alias_sets_should_quit() {
        let mut app = raw_app(1);
        app.buffers[app.active_buffer].editor.programmatic_insert(0, "dirty");
        app.dispatch_command("q!", 24, 80);
        assert!(app.should_quit, "q! must force-quit even when modified");
    }

    #[test]
    fn dispatch_command_set_width_with_arg_updates_max_line_width() {
        let mut app = raw_app(1);
        app.dispatch_command("set-width 42", 24, 80);
        assert_eq!(app.max_line_width, 42);
    }

    #[test]
    fn dispatch_command_set_width_without_arg_reports_parse_error() {
        // `:set-width` (no arg) goes through the SetMaxLineWidth fast-path in
        // dispatch_command, which tries to parse "" as a usize and reports a
        // parse error. (The "Usage:" message in execute_action is only reached
        // via a keybinding bound to Action::SetMaxLineWidth without a colon-
        // command wrapper, which isn't a configured path today.)
        let mut app = raw_app(1);
        app.dispatch_command("set-width", 24, 80);
        assert!(
            app.command_error.contains("expected number"),
            "set-width with no arg must surface a parse error, got: {}",
            app.command_error
        );
    }

    // ---- Compose textbox (spec-textbox-compose.md) ------------------------

    /// Ready the app on a *claude* buffer with optional pre-existing draft.
    fn app_with_claude(draft: &str) -> App {
        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        let idx = app.or_create_claude_buffer();
        app.active_buffer = idx;
        if !draft.is_empty() {
            let buf = &mut app.buffers[idx];
            let eof = buf.editor.document().rope().len_chars();
            buf.editor.programmatic_insert(eof, draft);
        }
        app
    }

    #[test]
    fn compose_toggle_outside_claude_buffer_is_a_noop() {
        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), "hi".into(), &cfg);
        // Active buffer is a regular file buffer, NOT *claude*.
        app.compose_toggle();
        assert!(
            app.compose_textbox.is_none(),
            "toggle outside *claude* must not open a textbox"
        );
        assert!(
            app.command_error.contains("only available in the *claude*"),
            "expected status hint, got: {}",
            app.command_error
        );
    }

    #[test]
    fn compose_toggle_open_in_claude_buffer_starts_in_insert_mode() {
        let mut app = app_with_claude("");
        app.compose_toggle();
        let tb = app
            .compose_textbox
            .as_ref()
            .expect("textbox must be open after toggle");
        assert_eq!(
            tb.mode,
            AppMode::Insert,
            "spec §2: open lands in Insert so user can type immediately"
        );
        assert_eq!(tb.text(), "", "fresh textbox must be empty");
    }

    #[test]
    fn compose_toggle_close_inserts_at_eof_and_clears_textbox() {
        // Per spec §11 (revised): compose-on-close inserts at EOF, *after* any
        // existing draft text — so the user's new compose lands where they
        // expect to see it.
        let mut app = app_with_claude("DRAFT");
        app.compose_toggle();
        // Type into the textbox.
        let tb = app.compose_textbox.as_mut().unwrap();
        tb.editor.insert_char('C');
        tb.editor.insert_char('O');
        tb.editor.insert_char('M');
        // Close.
        app.compose_toggle();
        assert!(
            app.compose_textbox.is_none(),
            "second toggle must close the textbox"
        );
        let body = app.buffers[app.active_buffer].editor.document().full_text();
        assert!(
            body.ends_with("DRAFTCOM"),
            "compose text must land at EOF (after draft), got: {body:?}"
        );
    }

    #[test]
    fn compose_toggle_close_with_empty_textbox_inserts_nothing() {
        let mut app = app_with_claude("DRAFT");
        app.compose_toggle();
        // Don't type anything.
        app.compose_toggle();
        assert!(app.compose_textbox.is_none());
        let body = app.buffers[app.active_buffer].editor.document().full_text();
        assert_eq!(body, "DRAFT", "empty close must not modify the buffer");
    }

    #[test]
    fn compose_send_empty_warns_and_keeps_textbox_open() {
        let mut app = app_with_claude("");
        app.compose_toggle();
        // Empty textbox.
        app.compose_send();
        assert!(
            app.compose_textbox.is_some(),
            "spec §16: empty send must NOT close the textbox"
        );
        assert!(
            app.command_error.to_lowercase().contains("empty")
                || app.command_error.to_lowercase().contains("nothing"),
            "expected empty-send hint, got: {}",
            app.command_error
        );
    }

    #[test]
    fn compose_send_without_channel_attaches_text_then_warns() {
        let mut app = app_with_claude("");
        app.compose_toggle();
        let tb = app.compose_textbox.as_mut().unwrap();
        tb.editor.insert_char('h');
        tb.editor.insert_char('i');
        app.compose_send();
        assert!(
            app.compose_textbox.is_none(),
            "non-empty send must close the textbox even when no channel is attached"
        );
        let body = app.buffers[app.active_buffer].editor.document().full_text();
        assert!(body.contains("hi"), "compose text must be in main buffer");
        assert!(
            app.command_error.to_lowercase().contains("no channel"),
            "expected no-channel hint, got: {}",
            app.command_error
        );
    }

    #[test]
    fn next_buffer_with_active_compose_closes_textbox_first() {
        // Two buffers: a regular file + *claude*.
        let cfg = Config::default();
        let mut app = App::new("untitled.md".into(), String::new(), &cfg);
        let claude_idx = app.or_create_claude_buffer();
        app.active_buffer = claude_idx;
        app.compose_toggle();
        let tb = app.compose_textbox.as_mut().unwrap();
        tb.editor.insert_char('z');
        // Switch buffers — spec §17 says compose closes first, contents flushed.
        app.execute_action(Action::NextBuffer, 24, 80);
        assert!(
            app.compose_textbox.is_none(),
            "spec §17: compose must close on buffer switch"
        );
        let claude_body = app.buffers[claude_idx].editor.document().full_text();
        assert!(
            claude_body.contains("z"),
            "compose contents must be preserved in the *claude* buffer, got: {claude_body:?}"
        );
    }

    #[test]
    fn force_quit_with_active_compose_discards_textbox() {
        // Spec §18-adjacent: force-quit treats the textbox as ephemeral state
        // and tears it down without flushing into the main buffer.
        let mut app = app_with_claude("");
        app.compose_toggle();
        let tb = app.compose_textbox.as_mut().unwrap();
        tb.editor.insert_char('x');
        app.execute_action(Action::ForceQuit, 24, 80);
        assert!(app.should_quit);
        assert!(
            app.compose_textbox.is_none(),
            "force-quit must drop the textbox without flushing"
        );
    }
}
