//! `KeymapView` — the cached body of the keybindings reference tile
//! (`App::Keymap`). A yux component (see `yux/CLAUDE.md`): it READS the live
//! [`KeymapRegistry`] and global theme off the root view, OWNS only its UI state
//! (scroll, browse cursor, filter text, in-progress rebind capture), and
//! self-invalidates when that UI state changes. It never owns the bindings —
//! those live on the root, so a rebind mutates the one source of truth and the
//! reference always shows the real, live keys.
//!
//! The whole surface (header + filter line + grouped list) is rendered here so
//! everything the tile shows is in one cached entity; `screens.rs::render_keymap`
//! just embeds it via `cached_child`. Interactions are handled on the root
//! (`keymap_ui.rs`), which drives this view through `Entity::update`.

use super::*;

/// Modal state of the tile: `Browse` = vim navigation over the rows; `Filter` =
/// typing into the search box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeymapMode {
    Browse,
    Filter,
}

/// An in-progress rebind. `idx` is the registry entry being rebound; `chords`
/// accumulates the captured keystrokes (space-joined into the new binding on
/// commit). While a capture is active the app's keymap is CLEARED (see
/// `keymap_ui.rs`) so the pressed chord is recorded here instead of firing an
/// action.
#[derive(Clone)]
pub(crate) struct CaptureState {
    pub idx: usize,
    pub chords: Vec<String>,
    /// Set when the last commit attempt failed (e.g. it collided / didn't
    /// parse) — shown inline so the user can retry.
    pub error: Option<String>,
}

pub(crate) struct KeymapView {
    root: WeakEntity<YaldaGpuiView>,
    scroll: ScrollHandle,
    /// Browse cursor: index into the flat list of *rebindable* rows currently
    /// visible (menu-reference rows are skipped). Clamped on every rebuild.
    cursor: usize,
    mode: KeymapMode,
    filter: String,
    capture: Option<CaptureState>,
    perf_label: &'static str,
}

/// One rendered row.
struct RowVM {
    /// The registry entry index (rebindable), or `None` for a read-only
    /// menu-command reference row.
    entry_idx: Option<usize>,
    keys: String,
    desc: String,
    /// Dim trailing detail (the action name, or the menu command name).
    detail: String,
    changed: bool,
    default_keys: String,
}

struct GroupVM {
    title: String,
    rows: Vec<RowVM>,
}

struct SectionVM {
    title: String,
    /// A one-line subtitle under the section heading (context only).
    subtitle: Option<String>,
    groups: Vec<GroupVM>,
}

impl KeymapView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        KeymapView {
            root,
            scroll: ScrollHandle::new(),
            cursor: 0,
            mode: KeymapMode::Browse,
            filter: String::new(),
            capture: None,
            perf_label: "keymap",
        }
    }

    pub(crate) fn mode(&self) -> KeymapMode {
        self.mode
    }

    pub(crate) fn set_mode(&mut self, mode: KeymapMode) {
        self.mode = mode;
    }

    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    pub(crate) fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.cursor = 0;
    }

    pub(crate) fn backspace_filter(&mut self) {
        self.filter.pop();
        self.cursor = 0;
    }

    pub(crate) fn clear_filter(&mut self) {
        self.filter.clear();
        self.cursor = 0;
    }

    pub(crate) fn capture(&self) -> Option<&CaptureState> {
        self.capture.as_ref()
    }

    pub(crate) fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Begin capturing a new binding for the entry the cursor is on.
    pub(crate) fn begin_capture(&mut self, idx: usize) {
        self.capture = Some(CaptureState {
            idx,
            chords: Vec::new(),
            error: None,
        });
    }

    pub(crate) fn push_chord(&mut self, chord: String) {
        if let Some(c) = &mut self.capture {
            c.chords.push(chord);
            c.error = None;
        }
    }

    pub(crate) fn pop_chord(&mut self) {
        if let Some(c) = &mut self.capture {
            c.chords.pop();
        }
    }

    pub(crate) fn cancel_capture(&mut self) {
        self.capture = None;
    }

    pub(crate) fn capture_failed(&mut self, msg: &str) {
        if let Some(c) = &mut self.capture {
            c.error = Some(msg.to_string());
            c.chords.clear();
        }
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Move the browse cursor by `delta`, clamped to `count` visible rows. The
    /// caller computes `count` from the registry (which it holds directly),
    /// keeping this method free of any root-entity read (which would alias the
    /// root while it's leased).
    pub(crate) fn move_cursor(&mut self, delta: i32, count: usize) {
        if count == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, count as i32 - 1) as usize;
    }

    pub(crate) fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn cursor_end(&mut self, count: usize) {
        self.cursor = count.saturating_sub(1);
    }

    pub(crate) fn scroll_by(&mut self, down: f32) {
        let cur = self.scroll.offset();
        let y = (cur.y - px(down)).min(px(0.0));
        self.scroll.set_offset(gpui::point(cur.x, y));
    }
}

/// The flat, filtered order of rebindable registry entry indices — the authority
/// the browse cursor indexes into. Free function so both the render path (which
/// holds a borrowed `&KeymapRegistry`) and the root's key handlers (which own the
/// registry on `self`) compute the SAME order without any root-entity read.
pub(crate) fn keymap_visible_order(reg: &KeymapRegistry, filter: &str) -> Vec<usize> {
    let filter = filter.to_lowercase();
    let mut out = Vec::new();
    for ctx in CONTEXT_ORDER {
        for e in reg.entries.iter().filter(|e| &e.context == ctx) {
            if binding_matches(e, &filter) {
                out.push(e.idx);
            }
        }
    }
    out
}

/// Does a registry entry match the (lowercased) filter?
fn binding_matches(e: &BindingEntry, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {} {} {}",
        e.keystrokes,
        e.desc,
        e.action,
        e.category,
        context_label(e.context)
    )
    .to_lowercase();
    hay.contains(filter)
}

impl Render for KeymapView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };

        // Snapshot root-owned inputs (style + the whole binding model) into owned
        // locals, releasing the borrow before we build elements.
        let (st, editor_bg, sections, cursor_idx, conflicts, changed_count) = {
            let r = root_ent.read(cx);
            let scale = r.text_scale;
            let st = DetailStyle {
                fg: r.editor_fg(),
                dim: nc(r.theme.agent.dim),
                accent: nc(r.theme.agent.warm_accent),
                err: rgb(0xff6b6b).into(),
                mono: r.code_font.clone(),
                prose: r.body_font.clone(),
                base: px(14.0 * scale),
                pt: 14.0 * scale,
            };
            let reg = &r.keymap_registry;
            let filter = self.filter.to_lowercase();
            let cursor_entry = keymap_visible_order(reg, &self.filter)
                .get(self.cursor)
                .copied();
            let conflicts: std::collections::HashSet<usize> = cursor_entry
                .map(|i| {
                    let mut s: std::collections::HashSet<usize> =
                        reg.conflicts(i).into_iter().collect();
                    if !s.is_empty() {
                        s.insert(i);
                    }
                    s
                })
                .unwrap_or_default();
            let sections = self.build_sections(reg, &filter, cursor_entry);
            (
                st,
                r.editor_bg(),
                sections,
                cursor_entry,
                conflicts,
                reg.changed_count(),
            )
        };

        let header = self.render_header(&st, changed_count, cursor_idx, &conflicts);
        let list = self.render_sections(&st, &sections, cursor_idx, &conflicts);

        let scroll = self.scroll.clone();
        div()
            .id("keymap-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(header)
            .child(
                div()
                    .id("keymap-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px_4()
                    .py_3()
                    .child(list),
            )
            .into_any_element()
    }
}

impl KeymapView {
    /// Build the full section model: keybinding sections (grouped by context →
    /// theme) followed by the read-only leader-menu reference. `cursor_entry` is
    /// unused here (kept for symmetry with the row highlight, resolved by index
    /// in `render_sections`).
    fn build_sections(
        &self,
        reg: &KeymapRegistry,
        filter: &str,
        _cursor_entry: Option<usize>,
    ) -> Vec<SectionVM> {
        let mut sections = Vec::new();

        for ctx in CONTEXT_ORDER {
            // Ordered categories within this context (first-appearance order).
            let mut cats: Vec<&'static str> = Vec::new();
            for e in reg.entries.iter().filter(|e| &e.context == ctx) {
                if binding_matches(e, filter) && !cats.contains(&e.category) {
                    cats.push(e.category);
                }
            }
            if cats.is_empty() {
                continue;
            }
            let mut groups = Vec::new();
            for cat in cats {
                let rows: Vec<RowVM> = reg
                    .entries
                    .iter()
                    .filter(|e| &e.context == ctx && e.category == cat && binding_matches(e, filter))
                    .map(|e| RowVM {
                        entry_idx: Some(e.idx),
                        keys: e.keystrokes.clone(),
                        desc: e.desc.to_string(),
                        detail: e.action.to_string(),
                        changed: e.is_changed(),
                        default_keys: e.default_keystrokes.to_string(),
                    })
                    .collect();
                if !rows.is_empty() {
                    groups.push(GroupVM {
                        title: cat.to_string(),
                        rows,
                    });
                }
            }
            sections.push(SectionVM {
                title: context_label(*ctx).to_string(),
                subtitle: Some(context_subtitle(*ctx).to_string()),
                groups,
            });
        }

        // Read-only leader-menu reference. Rebinding menu keys isn't supported
        // yet (they live in nested `MenuNode` trees), but showing them makes the
        // sheet complete: every command the user can reach is listed.
        let mut menu_groups = Vec::new();
        for (title, rows) in menu_reference() {
            let rows: Vec<RowVM> = rows
                .into_iter()
                .filter(|(keys, label, cmd)| {
                    filter.is_empty()
                        || format!("{keys} {label} {cmd}").to_lowercase().contains(filter)
                })
                .map(|(keys, label, cmd)| RowVM {
                    entry_idx: None,
                    keys,
                    desc: label,
                    detail: cmd,
                    changed: false,
                    default_keys: String::new(),
                })
                .collect();
            if !rows.is_empty() {
                menu_groups.push(GroupVM { title, rows });
            }
        }
        if !menu_groups.is_empty() {
            sections.push(SectionVM {
                title: "Leader menus".to_string(),
                subtitle: Some(
                    "reference only · open with space (tile) · . (workspace) · ? (global)"
                        .to_string(),
                ),
                groups: menu_groups,
            });
        }

        sections
    }

    fn render_header(
        &self,
        st: &DetailStyle,
        changed: usize,
        cursor_idx: Option<usize>,
        conflicts: &std::collections::HashSet<usize>,
    ) -> AnyElement {
        let filtering = self.mode == KeymapMode::Filter;
        let filter_text = if filtering {
            format!("{}\u{2588}", self.filter)
        } else if self.filter.is_empty() {
            "(all)".to_string()
        } else {
            self.filter.clone()
        };

        // Contextual help line depends on mode / capture.
        let help = if let Some(cap) = &self.capture {
            let so_far = if cap.chords.is_empty() {
                "…".to_string()
            } else {
                cap.chords.join(" ")
            };
            match &cap.error {
                Some(e) => format!("REBIND — {e}  ·  press keys, ⏎ save · esc cancel"),
                None => format!(
                    "REBIND — press the new keys: [{so_far}]  ·  ⏎ save · ⌫ undo · esc cancel"
                ),
            }
        } else if filtering {
            "filter · type to search · ⏎/esc done".to_string()
        } else {
            "j/k move · ⏎ or r rebind · x reset · / filter · space menu".to_string()
        };

        let mut status_bits: Vec<String> = Vec::new();
        if changed > 0 {
            status_bits.push(format!("{changed} changed"));
        }
        if let Some(c) = cursor_idx
            && conflicts.contains(&c)
            && conflicts.len() > 1
        {
            status_bits.push("⚠ conflict".to_string());
        }
        let status = status_bits.join("  ·  ");

        let capturing = self.capture.is_some();
        let accent = if capturing { st.err } else { st.accent };

        div()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .px_4()
            .py_2()
            .gap_1()
            .bg(Hsla::from(rgba(0x00000022)))
            .border_b_1()
            .border_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(st.base)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .flex_none()
                            .text_color(accent)
                            .font_weight(FontWeight::BOLD)
                            .child(SharedString::from("Keybindings")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(st.dim)
                            .child(SharedString::from("filter:")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(st.fg)
                            .child(SharedString::from(filter_text)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(st.accent)
                            .child(SharedString::from(status)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_color(if capturing { st.err } else { st.dim })
                    .text_size(px(st.pt * 0.9))
                    .child(SharedString::from(help)),
            )
            .into_any_element()
    }

    fn render_sections(
        &self,
        st: &DetailStyle,
        sections: &[SectionVM],
        cursor_idx: Option<usize>,
        conflicts: &std::collections::HashSet<usize>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().w_full().gap_3();
        for section in sections {
            let mut sec = div().flex().flex_col().w_full().gap_1();
            sec = sec.child(section_heading(&section.title, st));
            if let Some(sub) = &section.subtitle {
                sec = sec.child(
                    div()
                        .w_full()
                        .text_color(st.dim)
                        .font_family(st.mono.clone())
                        .text_size(px(st.pt * 0.85))
                        .child(SharedString::from(sub.clone())),
                );
            }
            for group in &section.groups {
                sec = sec.child(
                    div()
                        .w_full()
                        .pt_1()
                        .text_color(st.accent)
                        .font_family(st.mono.clone())
                        .text_size(px(st.pt * 0.9))
                        .font_weight(FontWeight::BOLD)
                        .child(SharedString::from(group.title.clone())),
                );
                for row in &group.rows {
                    sec = sec.child(self.render_row(st, row, cursor_idx, conflicts));
                }
            }
            col = col.child(sec);
        }
        col.into_any_element()
    }

    fn render_row(
        &self,
        st: &DetailStyle,
        row: &RowVM,
        cursor_idx: Option<usize>,
        conflicts: &std::collections::HashSet<usize>,
    ) -> gpui::Div {
        let is_cursor = row.entry_idx.is_some() && row.entry_idx == cursor_idx;
        let is_conflict = row.entry_idx.map(|i| conflicts.contains(&i)).unwrap_or(false);
        let capturing_here = self
            .capture
            .as_ref()
            .is_some_and(|c| Some(c.idx) == row.entry_idx);

        let transparent: Hsla = rgba(0x00000000).into();
        let mut sel_bg = st.accent;
        sel_bg.a = 0.16;
        let bg = if capturing_here {
            let mut b = st.err;
            b.a = 0.14;
            b
        } else if is_cursor {
            sel_bg
        } else {
            transparent
        };

        // The keystroke cell. During capture, show what's been pressed so far.
        let (keys_text, keys_color) = if capturing_here {
            let cap = self.capture.as_ref().unwrap();
            let s = if cap.chords.is_empty() {
                "press keys…".to_string()
            } else {
                cap.chords.join(" ")
            };
            (s, st.err)
        } else if is_conflict {
            (row.keys.clone(), st.err)
        } else if row.entry_idx.is_none() {
            (row.keys.clone(), st.dim)
        } else {
            (row.keys.clone(), st.accent)
        };

        let mut trailing = String::new();
        if row.changed {
            trailing = format!("changed · was {}", row.default_keys);
        }

        div()
            .flex()
            .flex_row()
            .items_start()
            .w_full()
            .px_1()
            .py(px(1.0))
            .bg(bg)
            .font_family(st.mono.clone())
            .text_size(st.base)
            .child(
                // Cursor gutter marker (keeps UXI-TextEditing-1: the selected row is
                // always visibly marked, not just background-tinted).
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_color(st.accent)
                    .child(SharedString::from(if is_cursor { "›" } else { " " })),
            )
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .text_color(keys_color)
                    .font_weight(if is_cursor { FontWeight::BOLD } else { FontWeight::NORMAL })
                    .child(SharedString::from(keys_text)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(st.fg)
                    .child(SharedString::from(row.desc.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .pl_2()
                    .text_color(if row.changed { st.accent } else { st.dim })
                    .text_size(px(st.pt * 0.85))
                    .child(SharedString::from(if trailing.is_empty() {
                        row.detail.clone()
                    } else {
                        trailing
                    })),
            )
    }
}

/// A one-line description of what a context's bindings apply to.
fn context_subtitle(ctx: Option<&str>) -> &'static str {
    match ctx {
        None => "work from any screen (Cmd shortcuts, splits, workspaces)",
        Some("YaldaView") => "active while reading a rendered markdown buffer",
        Some("AgentView") => "active while focused in an agent tile",
        Some("BrowserView") => "active in the file browser / picker",
        Some("RailView") => "active while the side rail holds focus",
        Some(_) => "",
    }
}

/// Flatten the leader-menu trees (workspace `.` menu + each tile's `space`
/// menu) into `(key-path, label, command)` rows for the read-only reference.
/// Reads the SAME `MenuNode` builders the live menus use, so it can never drift.
fn menu_reference() -> Vec<(String, Vec<(String, String, String)>)> {
    fn walk(prefix: &str, nodes: &[MenuNode], out: &mut Vec<(String, String, String)>) {
        for node in nodes {
            let key = yalda::keys::format_key_sequence(&node.key);
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix} {key}")
            };
            match &node.action {
                MenuAction::Command(cmd) => {
                    out.push((path, node.label.clone(), cmd.clone()));
                }
                MenuAction::Submenu(children) => {
                    walk(&path, children, out);
                }
                MenuAction::Separator | MenuAction::Label(_) => {}
            }
        }
    }
    let menus: [(&str, Vec<MenuNode>); 6] = [
        (". workspace menu", gpui_menu()),
        ("space · document", doc_local_menu()),
        ("space · edit", edit_local_menu()),
        ("space · agent", agent_local_menu()),
        ("space · linear", linear_local_menu()),
        ("space · browser", browser_local_menu()),
    ];
    menus
        .into_iter()
        .map(|(title, nodes)| {
            let mut rows = Vec::new();
            walk("", &nodes, &mut rows);
            (title.to_string(), rows)
        })
        .collect()
}
