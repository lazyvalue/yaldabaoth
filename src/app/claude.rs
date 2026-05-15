use sketch::acp_channel::AcpChannelClient;
use sketch::buffer::Buffer;
use sketch::claude_channel::ChannelClient;
use sketch::view::ViewMode;

use super::{App, AppMode, char_to_line_col};
use super::state::ComposeTextbox;

pub(crate) const CLAUDE_BUFFER_NAME: &str = "*claude*";

impl App {
    pub(crate) fn attach_claude_channel(&mut self, path_str: &str) {
        // Always drop any existing connection so re-running :claude-attach is
        // a clean recovery from staleness (e.g. after Claude restart, sketch
        // is talking to a now-dead sketch-channel that the kernel hasn't
        // bubbled up as broken yet).
        let had_existing = self.claude_channel.take().is_some();

        // Empty string → use the default socket path.
        let path = if path_str.is_empty() {
            ChannelClient::default_socket_path()
        } else {
            self.resolve_user_path(path_str)
        };
        match ChannelClient::connect(&path) {
            Ok(client) => {
                self.claude_channel = Some(client);
                let idx = self.or_create_claude_buffer();
                self.active_buffer = idx;
                self.command_error = if had_existing {
                    format!(
                        "Re-attached to Claude channel: {} (replaced previous connection)",
                        path.display()
                    )
                } else {
                    format!("Attached to Claude channel: {}", path.display())
                };
            }
            Err(e) => {
                self.command_error = format!(
                    "claude-attach: connect failed for {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    pub(crate) fn resolve_user_path(&self, path_str: &str) -> std::path::PathBuf {
        let expanded = if let Some(rest) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(rest)
            } else {
                std::path::PathBuf::from(path_str)
            }
        } else {
            std::path::PathBuf::from(path_str)
        };
        if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded))
                .unwrap_or(expanded)
        }
    }

    /// Returns `true` iff the payload reached the channel server.
    pub(crate) fn claude_send_text(&mut self, text: &str, label: &str) -> bool {
        let buffer_path = self.buffers[self.active_buffer]
            .file_path()
            .display()
            .to_string();
        let mut meta = std::collections::HashMap::new();
        // meta keys must be alphanumeric/underscore (Claude Code constraint).
        meta.insert("label".to_string(), label.to_string());
        meta.insert("file".to_string(), buffer_path.clone());
        let len = text.len();
        let outcome = match &mut self.claude_channel {
            Some(client) => match client.send(text, meta) {
                Ok(()) => Ok(len),
                Err(e) => Err(format!("send error: {}", e)),
            },
            None => Err("No Claude channel attached. Use :claude-attach first.".to_string()),
        };
        match outcome {
            Ok(n) => {
                self.command_error =
                    format!("Sent {} ({} chars) to Claude channel", label, n);
                true
            }
            Err(msg) => {
                if msg.contains("send error") {
                    // Drop the broken connection so the user can re-attach.
                    self.claude_channel = None;
                }
                self.command_error = msg;
                false
            }
        }
    }

    pub(crate) fn claude_send_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let is_claude_buffer = buf.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME;

        let payload = if is_claude_buffer {
            // Walk the active region (>= lockable) and collect contiguous runs
            // of editable chars (those NOT inside any frozen range). These are
            // the user's inline insertions — Claude's text is excluded.
            buf.editor.extract_editable_inserts()
        } else {
            buf.editor.document().full_text().trim().to_string()
        };

        if payload.is_empty() {
            self.command_error = if is_claude_buffer {
                "Nothing to send (no inline edits in this turn).".into()
            } else {
                "Nothing to send (buffer is empty).".into()
            };
            return;
        }

        let sent = self.claude_send_text(&payload, "buffer");

        if is_claude_buffer && sent {
            // Lock the turn: append HR and bump lockable to past it.
            self.lock_active_turn();
            self.ensure_cursor_visible(self.last_viewport_height);
        }
    }

    pub(crate) fn claude_send_selection(&mut self) {
        let sel = self.buffers[self.active_buffer]
            .editor
            .selection_text();
        match sel {
            Some(t) if !t.is_empty() => {
                self.claude_send_text(&t, "selection");
            }
            _ => {
                self.command_error =
                    "No selection. Make one first (e.g. `v` then a motion).".to_string();
            }
        }
    }

    /// Drain any pending replies from the Claude channel into the *claude* buffer.
    pub(crate) fn pump_claude_replies(&mut self, viewport_height: usize) {
        // If the reader thread saw EOF since last tick, the server is gone.
        // Drop the handle and surface a one-shot status message so the user
        // knows to `:claude-attach` again.
        let stale = self
            .claude_channel
            .as_ref()
            .map(|c| !c.is_connected())
            .unwrap_or(false);
        if stale {
            self.claude_channel = None;
            self.command_error =
                "Claude channel went stale (server gone). Run :claude-attach to recover."
                    .into();
        }

        let mut received: Vec<String> = Vec::new();
        if let Some(client) = &self.claude_channel {
            while let Some(text) = client.try_recv() {
                received.push(text);
            }
        }
        if received.is_empty() {
            return;
        }
        for text in received {
            self.append_to_claude_buffer(&text);
        }
        // Scroll the *claude* buffer's viewport to follow the new content,
        // even if the user is currently editing a different buffer.
        // Suppress when compose textbox is open — the user is typing and
        // doesn't want the viewport jumping under them.
        if self.compose_textbox.is_none() {
            if let Some(idx) = self
                .buffers
                .iter()
                .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
            {
                self.ensure_buffer_cursor_visible(idx, viewport_height);
            }
        }
    }

    /// Spawn an ACP agent subprocess and complete the handshake. Mirrors
    /// `attach_claude_channel` for the UNIX-socket path: any existing
    /// connection is dropped first, the *claude* buffer is created/focused
    /// on success, and `command_error` is used as a status line.
    ///
    /// `command_str` is shell-parsed; empty means "use the default agent
    /// (`claude-agent-acp`)". The `SKETCH_ACP_AGENT` env var is honoured if
    /// no command is given.
    pub(crate) fn attach_acp_channel(&mut self, command_str: &str) {
        let had_existing = self.acp_channel.take().is_some();
        let resolved = if command_str.is_empty() {
            std::env::var("SKETCH_ACP_AGENT").unwrap_or_default()
        } else {
            command_str.to_string()
        };
        let cwd = std::env::current_dir().ok();
        match AcpChannelClient::spawn(&resolved, cwd) {
            Ok(client) => {
                let label = client.description();
                self.acp_channel = Some(client);
                let idx = self.or_create_claude_buffer();
                self.active_buffer = idx;
                self.command_error = if had_existing {
                    format!("Re-attached to {label} (replaced previous)")
                } else {
                    format!("Attached to {label}")
                };
            }
            Err(e) => {
                self.command_error = format!("claude-acp-attach failed: {e}");
            }
        }
    }

    /// Send `text` as an ACP prompt. Returns true on success.
    pub(crate) fn acp_send_text(&mut self, text: &str, label: &str) -> bool {
        let outcome = match &mut self.acp_channel {
            Some(client) => match client.send(text) {
                Ok(()) => Ok(text.len()),
                Err(e) => Err(format!("ACP send error: {e}")),
            },
            None => Err(
                "No ACP agent attached. Use :claude-acp-attach first.".to_string(),
            ),
        };
        match outcome {
            Ok(n) => {
                self.command_error =
                    format!("Sent {label} ({n} chars) to ACP agent");
                true
            }
            Err(msg) => {
                if msg.contains("ACP send error") {
                    self.acp_channel = None;
                }
                self.command_error = msg;
                false
            }
        }
    }

    pub(crate) fn acp_send_buffer(&mut self) {
        let buf = &self.buffers[self.active_buffer];
        let is_claude_buffer = buf.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME;

        let payload = if is_claude_buffer {
            buf.editor.extract_editable_inserts()
        } else {
            buf.editor.document().full_text().trim().to_string()
        };

        if payload.is_empty() {
            self.command_error = if is_claude_buffer {
                "Nothing to send (no inline edits in this turn).".into()
            } else {
                "Nothing to send (buffer is empty).".into()
            };
            return;
        }

        let sent = self.acp_send_text(&payload, "buffer");

        if is_claude_buffer && sent {
            // Same lock-and-advance behaviour as the UNIX-socket path so
            // the user can keep typing while Claude works on the prior turn.
            self.lock_active_turn();
            self.ensure_cursor_visible(self.last_viewport_height);
        }
    }

    pub(crate) fn acp_send_selection(&mut self) {
        let sel = self.buffers[self.active_buffer]
            .editor
            .selection_text();
        match sel {
            Some(t) if !t.is_empty() => {
                self.acp_send_text(&t, "selection");
            }
            _ => {
                self.command_error =
                    "No selection. Make one first (e.g. `v` then a motion).".to_string();
            }
        }
    }

    /// Drain any streamed reply chunks from the ACP worker into the
    /// *claude* buffer. Identical splice behaviour to `pump_claude_replies`.
    pub(crate) fn pump_acp_replies(&mut self, viewport_height: usize) {
        let stale = self
            .acp_channel
            .as_ref()
            .map(|c| !c.is_connected())
            .unwrap_or(false);
        if stale {
            self.acp_channel = None;
            self.command_error =
                "ACP agent went away. Run :claude-acp-attach to recover.".into();
        }

        // The TUI only renders text chunks for now; tool-call notifications
        // are dropped (the GPUI frontend renders them inline as collapsible
        // blocks — adding the same to the TUI would mean teaching the raw
        // viewer about non-text "rows", which is more work than fits here).
        let mut received: Vec<String> = Vec::new();
        let mut current_turns = self.acp_last_seen_turns;
        if let Some(client) = &self.acp_channel {
            while let Some(ev) = client.try_recv() {
                if let sketch::acp_channel::ReplyEvent::Chunk(text) = ev {
                    received.push(text);
                }
            }
            current_turns = client.turn_count();
        }
        let turn_ended = current_turns > self.acp_last_seen_turns;
        if received.is_empty() && !turn_ended {
            return;
        }
        for text in received {
            self.append_to_claude_buffer(&text);
        }
        if turn_ended {
            // Drain any events queued between the try_recv loop and the
            // turn-count read so finalize sees a complete reply.
            let mut tail: Vec<String> = Vec::new();
            if let Some(client) = &self.acp_channel {
                while let Some(ev) = client.try_recv() {
                    if let sketch::acp_channel::ReplyEvent::Chunk(text) = ev {
                        tail.push(text);
                    }
                }
            }
            for text in tail {
                self.append_to_claude_buffer(&text);
            }
            self.finalize_claude_turn();
            self.acp_last_seen_turns = current_turns;
        }
        if self.compose_textbox.is_none() {
            if let Some(idx) = self
                .buffers
                .iter()
                .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
            {
                self.ensure_buffer_cursor_visible(idx, viewport_height);
            }
        }
    }

    /// Locate (or lazily create) the special *claude* buffer.
    pub(crate) fn or_create_claude_buffer(&mut self) -> usize {
        if let Some(i) = self
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
        {
            return i;
        }
        let mut buf = Buffer::new(
            CLAUDE_BUFFER_NAME.to_string(),
            String::new(),
            self.max_line_width,
            &self.theme,
        );
        // Default new buffers to Rendered, but the *claude* transcript is a
        // chat-style raw editor — viewport scrolling tracks doc lines there.
        buf.view_mode = ViewMode::Raw;
        self.buffers.push(buf);
        self.buffers.len() - 1
    }

    /// Ensure the cursor of buffer `buf_idx` is visible in its own viewport.
    /// Works for non-active buffers too, so a Claude reply landing in *claude*
    /// while the user is in another buffer still scrolls *claude* into place.
    pub(crate) fn ensure_buffer_cursor_visible(&mut self, buf_idx: usize, viewport_height: usize) {
        let wrap_width = self.last_wrap_width.max(1);
        let buf = &mut self.buffers[buf_idx];
        let rendered_y = match buf.view_mode {
            ViewMode::Raw => sketch::buffer::raw_cursor_visual_row(&buf.editor, wrap_width),
            // Skip for Rendered mode — it uses rendered_cursor_row which we
            // don't update for programmatic edits.
            ViewMode::Rendered => return,
        };
        // Also keep the buffer's own total_lines fresh so the scroll math is
        // correct for buffers that aren't currently active (we only update the
        // active one in the main loop).
        buf.update_total_lines(wrap_width);
        buf.viewport
            .ensure_cursor_visible(rendered_y, viewport_height);
    }

    /// Splice a Claude reply into the *claude* buffer above any pending draft.
    ///
    /// Buffer layout we maintain:
    /// ```text
    /// [turn 1]              ← frozen
    /// \n---\n               ← frozen (the HR line)
    /// [turn 2]              ← frozen
    /// \n---\n               ← frozen
    /// [draft]               ← editable
    /// ```
    /// On reply: take the current draft (everything past the frozen HR), and
    /// rewrite the editable region to `claude_text + HR + draft`. Advance the
    /// frozen line to the new HR. The user's pending draft is preserved.
    /// A new Claude reply landed. Splice it into the *claude* buffer ABOVE
    /// any pending user draft, so the user can keep typing while Claude is
    /// working and have the reply slot in above without disrupting them.
    ///
    /// Layout maintained:
    /// ```text
    /// [prior turns]      ← locked (lockable_through_line + frozen ranges)
    /// [new reply]        ← inserted here, marked frozen
    /// [user draft]       ← preserved verbatim, cursor stays on same char
    /// ```
    /// Streaming-safe: each chunk re-finds the splice point so subsequent
    /// chunks slot in just after the prior chunk, not below the draft.
    pub(crate) fn append_to_claude_buffer(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let buf_idx = self.or_create_claude_buffer();
        let buf = &mut self.buffers[buf_idx];
        let editor = &mut buf.editor;

        let total_len = editor.document().rope().len_chars();

        // Splice point: end of all locked content. Below this is the user's
        // editable draft (possibly empty). Take the max of lockable_through
        // and the end of the last frozen range — either can be further down.
        let lockable = editor.lockable_through_char();
        let frozen_end_line = editor
            .frozen_lines()
            .iter()
            .map(|&(_, e)| e)
            .max()
            .unwrap_or(0);
        let frozen_end_char = if frozen_end_line == 0 {
            0
        } else if frozen_end_line >= editor.document().line_count() {
            total_len
        } else {
            editor.document().line_col_to_char(frozen_end_line, 0)
        };
        let splice_at = lockable.max(frozen_end_char).min(total_len);

        // Capture the draft and where the cursor sits relative to it.
        let draft_text: String = editor
            .document()
            .rope()
            .slice(splice_at..total_len)
            .to_string();
        let cursor_char = editor
            .document()
            .line_col_to_char(editor.cursor().line, editor.cursor().col);
        let cursor_in_draft = cursor_char.saturating_sub(splice_at);
        let cursor_was_in_draft = cursor_char >= splice_at;

        // Strip the draft so the reply can append at end-of-locked-region;
        // we re-attach it after the reply is in place and frozen.
        if !draft_text.is_empty() {
            editor.programmatic_delete(splice_at, total_len);
        }

        // Append the chunk verbatim. ACP streams arbitrary slices of one
        // logical message; any extra padding here breaks sentences and
        // inserts blank lines between every chunk.
        let pre_len = editor.document().rope().len_chars();
        editor.programmatic_insert(pre_len, text);

        // Freeze the lines that the chunk now occupies. add_frozen_lines uses
        // a half-open [start, end) range, so when the chunk ends mid-line we
        // have to bump end_line past it. If the chunk ended on \n, the line
        // of claude_end_char is already the next line and serves directly.
        let claude_end_char = pre_len + text.chars().count();
        let start_line = char_to_line_col(editor.document(), pre_len).0;
        let mut end_line = char_to_line_col(editor.document(), claude_end_char).0;
        if !text.ends_with('\n') {
            end_line += 1;
        }
        editor.add_frozen_lines(start_line, end_line);

        // Re-attach the draft below the freshly-frozen reply. Insert a `\n`
        // separator only if the chunk didn't end on a newline — otherwise the
        // user's draft would run onto Claude's last line.
        let needs_separator = !draft_text.is_empty() && !text.ends_with('\n');
        if needs_separator {
            let here = editor.document().rope().len_chars();
            editor.programmatic_insert(here, "\n");
        }
        let draft_reattach_at = editor.document().rope().len_chars();
        if !draft_text.is_empty() {
            editor.programmatic_insert(draft_reattach_at, &draft_text);
        }

        if cursor_was_in_draft {
            let new_cursor_char = if draft_text.is_empty() {
                editor.document().rope().len_chars()
            } else {
                draft_reattach_at + cursor_in_draft
            };
            let (cl, cc) = char_to_line_col(editor.document(), new_cursor_char);
            editor.cursor_mut().line = cl;
            editor.cursor_mut().col = cc;
        }
        // else: cursor was inside locked content (e.g. user navigated up to
        // inline-edit a prior turn). Leave it where it is — programmatic
        // splice happened below the cursor's line.
        editor.clear_selection();
        buf.view_cache_dirty = true;
    }

    /// Called when the ACP turn ends. Streaming chunks splice in verbatim
    /// (no padding) — by the time the agent's prompt response lands, the
    /// cursor may sit on the last frozen line with no editable space below.
    /// Append a trailing newline (if needed) so the user can keep typing.
    pub(crate) fn finalize_claude_turn(&mut self) {
        let Some(buf_idx) = self
            .buffers
            .iter()
            .position(|b| b.file_path().to_string_lossy() == CLAUDE_BUFFER_NAME)
        else {
            return;
        };
        let buf = &mut self.buffers[buf_idx];
        let editor = &mut buf.editor;
        let total_len = editor.document().rope().len_chars();
        let needs_newline = total_len == 0
            || editor
                .document()
                .full_text()
                .chars()
                .last()
                .map(|c| c != '\n')
                .unwrap_or(true);
        if needs_newline {
            editor.programmatic_insert(total_len, "\n");
        }
        let eof = editor.document().rope().len_chars();
        let (cl, cc) = char_to_line_col(editor.document(), eof);
        editor.cursor_mut().line = cl;
        editor.cursor_mut().col = cc;
        editor.clear_selection();
        buf.view_cache_dirty = true;
    }

    /// Toggle the compose textbox. If closed, open it (must be in *claude* buffer).
    /// If open, extract text, insert it at the splice point in the main buffer, and close.
    pub(crate) fn compose_toggle(&mut self) {
        let is_claude = self.buffers[self.active_buffer]
            .file_path()
            .to_string_lossy()
            == CLAUDE_BUFFER_NAME;

        if self.compose_textbox.is_some() {
            // Close: extract text and insert into main buffer.
            let text = self.compose_textbox.as_ref().unwrap().text();
            self.compose_textbox = None;
            if !text.is_empty() {
                // Insert at the end of the buffer (where the user would type).
                let buf = &mut self.buffers[self.active_buffer];
                let eof = buf.editor.document().rope().len_chars();
                buf.editor.programmatic_insert(eof, &text);
                // Move cursor to end of inserted text.
                let new_eof = buf.editor.document().rope().len_chars();
                let (cl, cc) = char_to_line_col(buf.editor.document(), new_eof);
                buf.editor.cursor_mut().line = cl;
                buf.editor.cursor_mut().col = cc;
                buf.view_cache_dirty = true;
            }
            self.mode = AppMode::Normal;
        } else {
            if !is_claude {
                self.command_error =
                    "Compose textbox is only available in the *claude* buffer.".into();
                return;
            }
            self.compose_textbox = Some(ComposeTextbox::new());
            // The textbox starts in Insert mode; App mode stays Normal
            // (compose handler intercepts keys before the main dispatch).
        }
    }

    /// Send the compose textbox contents: close the textbox, insert text into
    /// the main buffer, then send via whichever channel is active.
    pub(crate) fn compose_send(&mut self) {
        let text = match &self.compose_textbox {
            Some(tb) => tb.text(),
            None => return,
        };
        if text.trim().is_empty() {
            self.command_error = "Nothing to send (compose box is empty).".into();
            return;
        }
        // Close the textbox and insert text into main buffer (same as toggle-off).
        self.compose_textbox = None;
        let buf = &mut self.buffers[self.active_buffer];
        let eof = buf.editor.document().rope().len_chars();
        buf.editor.programmatic_insert(eof, &text);
        let new_eof = buf.editor.document().rope().len_chars();
        let (cl, cc) = char_to_line_col(buf.editor.document(), new_eof);
        buf.editor.cursor_mut().line = cl;
        buf.editor.cursor_mut().col = cc;
        buf.view_cache_dirty = true;
        self.mode = AppMode::Normal;

        // Now send — prefer ACP if attached, fall back to UNIX socket channel.
        if self.acp_channel.is_some() {
            self.acp_send_buffer();
        } else if self.claude_channel.is_some() {
            self.claude_send_buffer();
        } else {
            self.command_error =
                "No channel attached. Use :claude-acp-attach or :claude-attach first.".into();
        }
    }

    /// Lock the active turn: append `\n\n---\n\n` and bump `lockable_through_char`
    /// to the new EOF. After this, the user can't edit the just-sent content.
    pub(crate) fn lock_active_turn(&mut self) {
        let buf_idx = self.active_buffer;
        let buf = &mut self.buffers[buf_idx];
        let editor = &mut buf.editor;

        let pre_len = editor.document().rope().len_chars();
        // Always end with exactly two newlines after the HR for clean spacing.
        let s = editor.document().full_text();
        let trailing_nl = s.chars().rev().take_while(|c| *c == '\n').count();
        let lead = "\n".repeat(2usize.saturating_sub(trailing_nl));
        let separator = format!("{}---\n\n", lead);
        editor.programmatic_insert(pre_len, &separator);

        // Cursor goes to EOF (start of next active turn — empty until reply).
        // Lock everything ABOVE the cursor's line; the cursor's own line stays
        // editable so the user can immediately keep typing.
        let eof = editor.document().rope().len_chars();
        let (cl, cc) = char_to_line_col(editor.document(), eof);
        editor.set_lockable_through_line(cl);
        editor.cursor_mut().line = cl;
        editor.cursor_mut().col = cc;
        editor.clear_selection();
        buf.view_cache_dirty = true;
    }
}
