//! The **session tag editor** (`UXI-AgentTile-33`) — a two-column modal dialog
//! for adding/removing a session's tags (`UXI-JumpPanel-20`), replacing the
//! earlier in-tile add/remove prompt.
//!
//! Left column = **ADD**: a type-to-filter/create list of every tag in use
//! across sessions that this session doesn't already carry, plus a synthetic
//! "＋ create <typed>" row when the typed text is novel. Right column = **ON THIS
//! SESSION**: the session's current tags, each removable. Fully keyboard-driven
//! (type to filter/create, `tab`/`←`/`→` switch columns, `↑↓` move, `enter`
//! toggles, `esc` closes) and mouse-driven (click a row to add/remove).
//!
//! Modeled on the `Cmd-P` jump palette (`jump_palette.rs`): a `capture_key_down`
//! wrapper swallows every key while open, and the card is layered over the
//! still-visible screen behind a click-away backdrop.

use super::*;

/// Which column currently has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TagEditorColumn {
    Available,
    Current,
}

/// Overlay state: the target session, the typed filter/new-tag text, the focused
/// column and the highlighted row within it.
pub(crate) struct TagEditorOverlay {
    #[allow(dead_code)] // reserved: re-resolve the tile if focus moves while open
    pub(crate) session: SessionId,
    pub(crate) sid: String,
    pub(crate) input: String,
    pub(crate) column: TagEditorColumn,
    pub(crate) selected: usize,
}

/// A row in the ADD (left) column: an existing known tag, or the synthetic
/// "create the typed text as a new tag" affordance.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum TagLeftRow {
    New(String),
    Existing(String),
}

impl TagLeftRow {
    pub(crate) fn tag(&self) -> &str {
        match self {
            TagLeftRow::New(s) | TagLeftRow::Existing(s) => s,
        }
    }
}

/// The rows to draw in each column, derived from the overlay + the tag store.
pub(crate) struct TagEditorModel {
    pub(crate) left: Vec<TagLeftRow>,
    pub(crate) current: Vec<String>,
}

impl YaldaGpuiView {
    /// Every tag ever used across all sessions (the union of the sidecar's
    /// values), sorted + unique — the pool the ADD column filters.
    pub(crate) fn all_known_tags(&self) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for tags in self.session_tags.values() {
            for t in tags {
                set.insert(t.clone());
            }
        }
        set.into_iter().collect()
    }

    /// Derive the two columns for the current overlay state. Pure over the tag
    /// store, so the model is headlessly testable.
    pub(crate) fn tag_editor_model(&self, ov: &TagEditorOverlay) -> TagEditorModel {
        let current: Vec<String> = {
            let mut c = self.session_tags.get(&ov.sid).cloned().unwrap_or_default();
            c.sort();
            c.dedup();
            c
        };
        let on_session =
            |t: &str| current.iter().any(|c| c.eq_ignore_ascii_case(t));
        let typed = ov.input.trim();
        let q = typed.to_lowercase();
        let mut left: Vec<TagLeftRow> = Vec::new();
        // A "create" row when the typed text is a genuinely new tag.
        let known_exact =
            self.all_known_tags().iter().any(|k| k.eq_ignore_ascii_case(typed));
        if !typed.is_empty() && !on_session(typed) && !known_exact {
            left.push(TagLeftRow::New(typed.to_string()));
        }
        for t in self.all_known_tags() {
            if on_session(&t) {
                continue; // already on the session — it's in the right column
            }
            if !q.is_empty() && !t.to_lowercase().contains(&q) {
                continue; // doesn't match the filter
            }
            left.push(TagLeftRow::Existing(t));
        }
        TagEditorModel { left, current }
    }

    pub(crate) fn tag_editor_ref(&self) -> Option<&TagEditorOverlay> {
        if let ActiveOverlay::TagEditor(o) = &self.active_overlay {
            Some(o)
        } else {
            None
        }
    }

    pub(crate) fn tag_editor_mut(&mut self) -> Option<&mut TagEditorOverlay> {
        if let ActiveOverlay::TagEditor(o) = &mut self.active_overlay {
            Some(o)
        } else {
            None
        }
    }

    pub(crate) fn overlay_is_tag_editor(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::TagEditor(_))
    }

    /// Open the tag editor for the focused bound session. No-op if any overlay is
    /// already open or no session is focused; a session with no server sid can't
    /// be tagged (tags key by sid), so it sets a transient note instead.
    pub(crate) fn open_tag_editor(&mut self, cx: &mut Context<Self>) {
        if self.has_overlay() {
            return;
        }
        let Some(id) = self.focused_bound_session() else {
            return;
        };
        let Some(sid) = self.sessions.sid_of(id).map(|s| s.as_str().to_string()) else {
            self.transient_status = Some("session not ready to tag".into());
            cx.notify();
            return;
        };
        self.transient_status = None;
        self.open_overlay(ActiveOverlay::TagEditor(TagEditorOverlay {
            session: id,
            sid,
            input: String::new(),
            column: TagEditorColumn::Available,
            selected: 0,
        }));
        cx.notify();
    }

    /// Add `tag` to the editor's session and reset the filter (so the list
    /// refreshes and the just-added tag hops to the right column).
    pub(crate) fn tag_editor_add(&mut self, tag: &str, cx: &mut Context<Self>) {
        let Some(sid) = self.tag_editor_ref().map(|o| o.sid.clone()) else {
            return;
        };
        self.add_session_tag(&sid, tag);
        if let Some(o) = self.tag_editor_mut() {
            o.input.clear();
            o.selected = 0;
        }
        cx.notify();
    }

    /// Remove `tag` from the editor's session, clamping the Current highlight.
    pub(crate) fn tag_editor_remove(&mut self, tag: &str, cx: &mut Context<Self>) {
        let Some(sid) = self.tag_editor_ref().map(|o| o.sid.clone()) else {
            return;
        };
        self.remove_session_tag(&sid, tag);
        if let Some(o) = self.tag_editor_mut() {
            o.selected = o.selected.saturating_sub(1);
        }
        cx.notify();
    }

    /// Enter: toggle the highlighted row (add from the left column, remove from
    /// the right).
    pub(crate) fn activate_tag_editor(&mut self, cx: &mut Context<Self>) {
        let Some(ov) = self.tag_editor_ref() else {
            return;
        };
        let model = self.tag_editor_model(ov);
        let (col, sel) = (ov.column, ov.selected);
        match col {
            TagEditorColumn::Available => {
                if let Some(row) = model.left.get(sel) {
                    let t = row.tag().to_string();
                    self.tag_editor_add(&t, cx);
                }
            }
            TagEditorColumn::Current => {
                if let Some(t) = model.current.get(sel).cloned() {
                    self.tag_editor_remove(&t, cx);
                }
            }
        }
    }

    pub(crate) fn handle_tag_editor_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        // Length of the focused column, as a value (drop the borrow before mut).
        let (col, len) = {
            let Some(ov) = self.tag_editor_ref() else {
                return;
            };
            let m = self.tag_editor_model(ov);
            let len = match ov.column {
                TagEditorColumn::Available => m.left.len(),
                TagEditorColumn::Current => m.current.len(),
            };
            (ov.column, len)
        };
        match press.key {
            Key::Esc => {
                self.clear_overlay();
                cx.notify();
            }
            Key::Enter => self.activate_tag_editor(cx),
            Key::Tab | Key::BackTab | Key::Left | Key::Right => {
                if let Some(o) = self.tag_editor_mut() {
                    o.column = match o.column {
                        TagEditorColumn::Available => TagEditorColumn::Current,
                        TagEditorColumn::Current => TagEditorColumn::Available,
                    };
                    o.selected = 0;
                }
                cx.notify();
            }
            Key::Down => {
                if let Some(o) = self.tag_editor_mut()
                    && len > 0
                {
                    o.selected = (o.selected + 1) % len;
                }
                cx.notify();
            }
            Key::Up => {
                if let Some(o) = self.tag_editor_mut()
                    && len > 0
                {
                    o.selected = (o.selected + len - 1) % len;
                }
                cx.notify();
            }
            // In the Current column, Backspace/Delete removes the highlighted tag;
            // in the Available column it edits the filter text.
            Key::Backspace if col == TagEditorColumn::Current => self.activate_tag_editor(cx),
            Key::Delete if col == TagEditorColumn::Current => self.activate_tag_editor(cx),
            Key::Backspace => {
                if let Some(o) = self.tag_editor_mut() {
                    o.input.pop();
                    o.selected = 0;
                }
                cx.notify();
            }
            // Typing always edits the filter/new-tag text and focuses the ADD
            // column (unmodified chars only — a Cmd/Ctrl/Alt chord dies here).
            Key::Char(c)
                if !press.modifiers.contains(KMods::PLATFORM)
                    && !press.modifiers.contains(KMods::CONTROL)
                    && !press.modifiers.contains(KMods::ALT) =>
            {
                if let Some(o) = self.tag_editor_mut() {
                    o.input.push(c);
                    o.column = TagEditorColumn::Available;
                    o.selected = 0;
                }
                cx.notify();
            }
            _ => {}
        }
    }

    pub(crate) fn render_tag_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let st = DetailStyle {
            fg: self.editor_fg(),
            dim: nc(self.theme.agent.dim),
            accent: nc(self.theme.agent.warm_accent),
            err: nc(self.theme.agent.jump_header),
            mono: self.code_font.clone(),
            prose: self.body_font.clone(),
            base: px(13.0),
            pt: 13.0,
        };
        let ov_theme = &self.theme.overlay;
        let popup_bg: Hsla = nc(ov_theme.bg);
        let popup_border: Hsla = nc(ov_theme.border);
        let label_fg: Hsla = nc(ov_theme.label);
        let input_fg: Hsla = nc(ov_theme.input);
        let electric = nc(self.theme.agent.jump_subheader);
        let ready = nc(self.theme.agent.tool_completed);
        let accent = nc(self.theme.agent.frozen_bar);
        let mut sel_bg = accent;
        sel_bg.a = 0.15;
        let divider = {
            let mut c = st.dim;
            c.a = 0.4;
            c
        };

        let Some(ov) = self.tag_editor_ref() else {
            return div().into_any_element();
        };
        let model = self.tag_editor_model(ov);
        let focus = ov.column;
        let selected = ov.selected;
        let input = ov.input.clone();

        // ── Header + type-to-filter input.
        let header = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .font_weight(FontWeight::BOLD)
            .text_size(px(11.0))
            .child(SharedString::new_static("TAG SESSION"));
        let input_row = div()
            .px_4()
            .py_2()
            .text_color(input_fg)
            .text_size(px(14.0))
            .font_family(st.mono.clone())
            .child(SharedString::from(format!("{input}\u{2588}")));

        // ── Column headers.
        let col_head = |text: &'static str, active: bool| {
            div()
                .px_3()
                .py_1()
                .text_color(if active { electric } else { st.dim })
                .font_weight(FontWeight::BOLD)
                .text_size(px(10.0))
                .child(SharedString::new_static(text))
        };

        // ── ADD (left) column rows.
        let mut left_col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(col_head("ADD", focus == TagEditorColumn::Available));
        if model.left.is_empty() {
            left_col = left_col.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(st.dim)
                    .text_size(px(12.0))
                    .child(SharedString::new_static("type to create a tag")),
            );
        }
        for (n, row) in model.left.iter().enumerate() {
            let is_sel = focus == TagEditorColumn::Available && n == selected;
            let (glyph, name, is_new) = match row {
                TagLeftRow::New(s) => ("＋", s.clone(), true),
                TagLeftRow::Existing(s) => ("🏷", s.clone(), false),
            };
            let tag = row.tag().to_string();
            let mut r = div()
                .id(SharedString::from(format!("tag-editor-left-{n}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w_full()
                .px_3()
                .py_1()
                .hover(|s| s.bg(sel_bg))
                .child(
                    div()
                        .w(px(16.0))
                        .flex_none()
                        .text_color(if is_new { ready } else { electric })
                        .child(SharedString::new_static(glyph)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.0))
                        .text_color(if is_sel { accent } else { st.fg })
                        .child(SharedString::from(if is_new {
                            format!("create \u{201c}{name}\u{201d}")
                        } else {
                            name
                        })),
                );
            if is_sel {
                r = r.bg(sel_bg);
            }
            r = r
                .on_click(cx.listener(move |this, _ev, _w, cx| {
                    this.tag_editor_add(&tag, cx);
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                    if *hovered && let Some(o) = this.tag_editor_mut() {
                        o.column = TagEditorColumn::Available;
                        o.selected = n;
                        cx.notify();
                    }
                }));
            left_col = left_col.child(probe_bounds_dyn(
                format!("tag-editor-left-{n}"),
                r.into_any_element(),
            ));
        }

        // ── ON THIS SESSION (right) column rows.
        let mut right_col = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(col_head("ON THIS SESSION", focus == TagEditorColumn::Current));
        if model.current.is_empty() {
            right_col = right_col.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(st.dim)
                    .text_size(px(12.0))
                    .child(SharedString::new_static("no tags yet")),
            );
        }
        for (n, tag) in model.current.iter().cloned().enumerate() {
            let is_sel = focus == TagEditorColumn::Current && n == selected;
            let tag_for_click = tag.clone();
            let mut r = div()
                .id(SharedString::from(format!("tag-editor-current-{n}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w_full()
                .px_3()
                .py_1()
                .hover(|s| s.bg(sel_bg))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.0))
                        .text_color(if is_sel { accent } else { st.fg })
                        .child(SharedString::from(format!("🏷 {tag}"))),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(st.dim)
                        .text_size(px(12.0))
                        .child(SharedString::new_static("✕")),
                );
            if is_sel {
                r = r.bg(sel_bg);
            }
            r = r
                .on_click(cx.listener(move |this, _ev, _w, cx| {
                    this.tag_editor_remove(&tag_for_click, cx);
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                    if *hovered && let Some(o) = this.tag_editor_mut() {
                        o.column = TagEditorColumn::Current;
                        o.selected = n;
                        cx.notify();
                    }
                }));
            right_col = right_col.child(probe_bounds_dyn(
                format!("tag-editor-current-{n}"),
                r.into_any_element(),
            ));
        }

        let columns = div()
            .flex()
            .flex_row()
            .w_full()
            .child(left_col)
            .child(div().w(px(1.0)).bg(divider))
            .child(right_col);

        let footer = div()
            .px_4()
            .py_1()
            .text_color(label_fg)
            .text_size(px(11.0))
            .child(SharedString::new_static(
                "type:filter/create  tab:switch  ↑↓:move  enter:toggle  esc:done",
            ));

        // Click-away backdrop (closes) under an occluding card.
        let backdrop = div()
            .id("tag-editor-backdrop")
            .absolute()
            .inset_0()
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.25))
            .on_click(cx.listener(|this, _ev, _w, cx| {
                this.clear_overlay();
                cx.notify();
            }));

        let card = div()
            .occlude()
            .w(px(560.0))
            .bg(popup_bg)
            .border_2()
            .border_color(popup_border)
            .flex()
            .flex_col()
            .child(header)
            .child(input_row)
            .child(div().h(px(1.0)).mx_3().bg(divider))
            .child(columns)
            .child(div().h(px(1.0)).mx_3().bg(divider))
            .child(footer);

        probe_bounds(
            "tag-editor",
            div()
                .absolute()
                .inset_0()
                .child(backdrop)
                .child(
                    div()
                        .absolute()
                        .top(px(90.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .flex_row()
                        .justify_center()
                        .child(card),
                )
                .into_any_element(),
        )
    }
}
