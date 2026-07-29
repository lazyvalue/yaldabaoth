//! Yalda's drop-down system console (`UXI-SystemConsole-1..4`).
//!
//! The component owns a bounded log and scroll state, persists recent lifecycle
//! messages across the process replacement performed by self-rebuild, and
//! renders as a cached yux child. The initial log policy is intentionally
//! narrow: app lifecycle + Cargo build output.

use super::*;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
#[cfg(not(test))]
use std::process::Stdio;
#[cfg(not(test))]
use std::sync::mpsc;

pub(crate) const SYSTEM_CONSOLE_MAX_LINES: usize = 1_000;
pub(crate) const SYSTEM_CONSOLE_HEIGHT_RATIO: f32 = 1.0 / 3.0;
#[cfg(not(test))]
const SYSTEM_CONSOLE_FILE: &str = "system-console.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConsoleLevel {
    Info,
    Warn,
    Error,
    Command,
}

impl ConsoleLevel {
    fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Command => "CMD",
        }
    }

    fn parse(tag: &str) -> Self {
        match tag {
            "WARN" => Self::Warn,
            "ERROR" => Self::Error,
            "CMD" => Self::Command,
            _ => Self::Info,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConsoleLine {
    pub(crate) level: ConsoleLevel,
    pub(crate) text: String,
}

#[derive(Default)]
pub(crate) struct ConsoleLog {
    lines: VecDeque<ConsoleLine>,
    pushed: usize,
}

impl ConsoleLog {
    pub(crate) fn from_lines(lines: impl IntoIterator<Item = ConsoleLine>) -> Self {
        let mut log = Self::default();
        for line in lines {
            log.push(line.level, line.text);
        }
        log
    }

    /// Append one logical row and drop the oldest row at the fixed bound.
    pub(crate) fn push(&mut self, level: ConsoleLevel, text: impl Into<String>) {
        let text = text.into().replace(['\r', '\n'], " ");
        self.lines.push_back(ConsoleLine { level, text });
        self.pushed += 1;
        while self.lines.len() > SYSTEM_CONSOLE_MAX_LINES {
            self.lines.pop_front();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.lines.clear();
    }

    pub(crate) fn lines(&self) -> &VecDeque<ConsoleLine> {
        &self.lines
    }

    fn should_compact_file(&self) -> bool {
        self.lines.len() == SYSTEM_CONSOLE_MAX_LINES && self.pushed % 100 == 0
    }
}

pub(crate) fn classify_build_line(line: &str) -> ConsoleLevel {
    let trimmed = line.trim_start();
    if trimmed.starts_with("error")
        || trimmed.contains("could not compile")
        || trimmed.contains("aborting due to")
    {
        ConsoleLevel::Error
    } else if trimmed.starts_with("warning") {
        ConsoleLevel::Warn
    } else {
        ConsoleLevel::Info
    }
}

#[cfg(not(test))]
pub(crate) enum BuildEvent {
    Line(ConsoleLevel, String),
    Finished(Result<(), String>),
}

/// Run Cargo on an OS thread and forward each stdout/stderr row as it arrives.
/// GPUI's async task polls the returned receiver, so neither compiler I/O nor
/// `Child::wait` can block paint.
#[cfg(not(test))]
pub(crate) fn spawn_self_build(
    manifest_dir: String,
    restart_server: bool,
) -> mpsc::Receiver<BuildEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut build_args = vec!["build", "--release", "--bin", "yalda-gpui"];
        if restart_server {
            build_args.extend_from_slice(&["--bin", "yalda-session-server"]);
        }
        let mut child = match std::process::Command::new("cargo")
            .args(&build_args)
            .current_dir(&manifest_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send(BuildEvent::Finished(Err(format!(
                    "could not start cargo: {error}"
                ))));
                return;
            }
        };

        let mut pipes: Vec<Box<dyn std::io::Read + Send>> = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            pipes.push(Box::new(stdout));
        }
        if let Some(stderr) = child.stderr.take() {
            pipes.push(Box::new(stderr));
        }
        let mut readers = Vec::new();
        for pipe in pipes {
            let line_tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    let level = classify_build_line(&line);
                    let _ = line_tx.send(BuildEvent::Line(level, line));
                }
            }));
        }

        let status = child.wait();
        for reader in readers {
            let _ = reader.join();
        }
        match status {
            Ok(status) if status.success() => {
                if restart_server {
                    // Mirror dev-server.sh: the new GUI must launch the binary
                    // just built, not reconnect to an older resident server.
                    for pat in [
                        "target/debug/yalda-session-server",
                        "target/release/yalda-session-server",
                    ] {
                        let _ = std::process::Command::new("pkill")
                            .args(["-f", pat])
                            .status();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = std::fs::remove_file(yalda::session_proto::socket_path());
                    let _ = std::fs::remove_file(yalda::session_proto::pid_file_path());
                }
                let _ = tx.send(BuildEvent::Finished(Ok(())));
            }
            Ok(status) => {
                let _ = tx.send(BuildEvent::Finished(Err(format!(
                    "cargo exited with {status}"
                ))));
            }
            Err(error) => {
                let _ = tx.send(BuildEvent::Finished(Err(format!(
                    "could not wait for cargo: {error}"
                ))));
            }
        }
    });
    rx
}

fn console_log_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return SYSTEM_CONSOLE_PATH_OVERRIDE.with(|p| p.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join(SYSTEM_CONSOLE_FILE))
    }
}

#[cfg(test)]
thread_local! {
    static SYSTEM_CONSOLE_PATH_OVERRIDE: RefCell<Option<PathBuf>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_system_console_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    SYSTEM_CONSOLE_PATH_OVERRIDE.with(|p| *p.borrow_mut() = Some(path));
    let out = f();
    SYSTEM_CONSOLE_PATH_OVERRIDE.with(|p| *p.borrow_mut() = None);
    out
}

fn encode_line(line: &ConsoleLine) -> String {
    format!("{}\t{}\n", line.level.tag(), line.text)
}

fn decode_line(line: &str) -> ConsoleLine {
    let (level, text) = line.split_once('\t').unwrap_or(("INFO", line));
    ConsoleLine {
        level: ConsoleLevel::parse(level),
        text: text.to_string(),
    }
}

fn load_console_log() -> ConsoleLog {
    let Some(path) = console_log_path() else {
        return ConsoleLog::default();
    };
    let Ok(file) = std::fs::File::open(path) else {
        return ConsoleLog::default();
    };
    ConsoleLog::from_lines(
        BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .map(|line| decode_line(&line)),
    )
}

fn append_persisted(line: &ConsoleLine) {
    let Some(path) = console_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(encode_line(line).as_bytes());
    }
}

fn save_console_log(log: &ConsoleLog) {
    let Some(path) = console_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut body = String::new();
    for line in log.lines() {
        body.push_str(&encode_line(line));
    }
    let _ = std::fs::write(path, body);
}

/// Record a lifecycle message before GPUI has constructed the console view.
pub(crate) fn record_system_message(level: ConsoleLevel, text: impl Into<String>) {
    let line = ConsoleLine {
        level,
        text: text.into().replace(['\r', '\n'], " "),
    };
    append_persisted(&line);
}

pub(crate) struct SystemConsoleView {
    root: WeakEntity<YaldaGpuiView>,
    log: ConsoleLog,
    scroll: ScrollHandle,
    building: bool,
}

impl SystemConsoleView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        Self {
            root,
            log: load_console_log(),
            scroll: ScrollHandle::new(),
            building: false,
        }
    }

    pub(crate) fn push(
        &mut self,
        level: ConsoleLevel,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let line = ConsoleLine {
            level,
            text: text.replace(['\r', '\n'], " "),
        };
        self.log.push(line.level, line.text.clone());
        if self.log.should_compact_file() {
            save_console_log(&self.log);
        } else {
            append_persisted(&line);
        }
        // Follow the newest output. Scroll offsets are negative in GPUI;
        // a very large negative value clamps to the bottom at layout time.
        self.scroll
            .set_offset(gpui::point(px(0.0), px(-1_000_000.0)));
        record_notify("system_console", MissReason::Dirtied);
        cx.notify();
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.log.clear();
        save_console_log(&self.log);
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        record_notify("system_console", MissReason::Dirtied);
        cx.notify();
    }

    pub(crate) fn building(&self) -> bool {
        self.building
    }

    pub(crate) fn set_building(&mut self, building: bool, cx: &mut Context<Self>) {
        if self.building != building {
            self.building = building;
            record_notify("system_console", MissReason::Dirtied);
            cx.notify();
        }
    }
}

impl YaldaGpuiView {
    pub(crate) fn system_console_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<SystemConsoleView> {
        if let Some(view) = &self.system_console_view {
            return view.clone();
        }
        let weak = cx.entity().downgrade();
        let view = cx.new(|_| SystemConsoleView::new(weak));
        self.system_console_view = Some(view.clone());
        view
    }

    pub(crate) fn append_system_console(
        &mut self,
        level: ConsoleLevel,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let view = self.system_console_view(cx);
        let text = text.into();
        view.update(cx, |view, cx| view.push(level, text, cx));
    }

    pub(crate) fn open_system_console(&mut self, cx: &mut Context<Self>) {
        self.system_console_view(cx);
        self.transient_status = None;
        self.open_overlay(ActiveOverlay::SystemConsole);
        cx.notify();
    }

    pub(crate) fn overlay_is_system_console(&self) -> bool {
        matches!(self.active_overlay, ActiveOverlay::SystemConsole)
    }

    /// Push a changed global theme/font snapshot into the cached console. The
    /// component reads those values from the root during render, so parent
    /// notification alone cannot invalidate its cached subtree.
    pub(crate) fn notify_system_console(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        if let Some(view) = &self.system_console_view {
            record_notify("system_console", reason);
            view.update(cx, |_view, cx| cx.notify());
        }
    }

    pub(crate) fn handle_system_console_key(
        &mut self,
        ev: &KeyDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = keystroke_to_keypress(&ev.keystroke);
        match press.key {
            Key::Esc | Key::Char('q') => {
                self.clear_overlay();
                cx.notify();
            }
            Key::Char('r') => self.dev_rebuild_restart_gui(cx),
            Key::Char('R') => self.dev_rebuild_restart_all(cx),
            Key::Char('c') => {
                if let Some(view) = &self.system_console_view {
                    view.update(cx, |view, cx| view.clear(cx));
                }
            }
            _ => {}
        }
    }

    pub(crate) fn render_system_console_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let view = self.system_console_view(cx);
        let panel_bg = menu_panel_bg(self.editor_bg());
        let border = nc(self.theme.overlay.border);
        // Compact operational chrome: the console uses the same themed surface
        // and border vocabulary as Yalda's menus and jump panel, while leaving
        // most of the desktop visible.
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(gpui::relative(SYSTEM_CONSOLE_HEIGHT_RATIO))
            .bg(panel_bg)
            .border_b_1()
            .border_color(border)
            .shadow_lg()
            .child(cached_child(view))
            .into_any_element()
    }
}

impl Render for SystemConsoleView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render("system_console");
        let Some(root) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let (mono, panel_bg, header_bg, fg, dim, border, accent, key, error, warning) = {
            let r = root.read(cx);
            let overlay = &r.theme.overlay;
            (
                r.code_font.clone(),
                menu_panel_bg(r.editor_bg()),
                nc(overlay.selected_bg),
                nc(overlay.fg),
                nc(overlay.label),
                nc(overlay.border),
                nc(overlay.accent),
                nc(overlay.key),
                nc(r.theme.agent.tool_failed),
                nc(overlay.modified),
            )
        };

        let mut rows = div()
            .id("system-console-lines")
            .flex()
            .flex_col()
            .min_h_full()
            .px_3()
            .py_1()
            .gap(px(1.0));
        if self.log.lines().is_empty() {
            rows = rows.child(
                div()
                    .text_color(dim)
                    .child("No system messages yet. Press r to rebuild Yalda."),
            );
        } else {
            for (idx, line) in self.log.lines().iter().enumerate() {
                let level_color = match line.level {
                    ConsoleLevel::Error => error,
                    ConsoleLevel::Warn => warning,
                    ConsoleLevel::Command => key,
                    ConsoleLevel::Info => dim,
                };
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("system-console-line-{idx}")))
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .w(px(42.0))
                                .flex_none()
                                .text_color(level_color)
                                .font_weight(FontWeight::BOLD)
                                .child(line.level.tag()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(if line.level == ConsoleLevel::Error {
                                    error
                                } else {
                                    fg
                                })
                                .child(SharedString::from(line.text.clone())),
                        ),
                );
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(panel_bg)
            .text_color(fg)
            .font_family(mono)
            .text_size(px(11.0))
            .child(
                div()
                    .id("system-console-header")
                    .h(px(28.0))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_3()
                    .gap_2()
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_color(accent)
                            .font_weight(FontWeight::BOLD)
                            .child("▾"),
                    )
                    .child(div().font_weight(FontWeight::BOLD).child("system console"))
                    .child(
                        div()
                            .text_color(if self.building { warning } else { dim })
                            .child(if self.building {
                                "building…"
                            } else {
                                "ready"
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_color(dim)
                            .child("[r] rebuild · [R] gui + server · [c] clear · esc close"),
                    ),
            )
            .child(
                div()
                    .id("system-console-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(rows),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[test]
fn system_console_height_stays_near_one_third() {
    assert!(
        (0.30..=0.36).contains(&SYSTEM_CONSOLE_HEIGHT_RATIO),
        "console should preserve roughly two thirds of the desktop"
    );
}
