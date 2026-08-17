//! Yalda's drop-down system console (`UXI-SystemConsole-1..5`).
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
use std::sync::{Arc, OnceLock};

pub(crate) const SYSTEM_CONSOLE_MAX_LINES: usize = 1_000;
pub(crate) const SYSTEM_CONSOLE_HEIGHT_RATIO: f32 = 1.0 / 3.0;
pub(crate) const SYSTEM_CONSOLE_WIDTH_RATIO: f32 = 2.0 / 3.0;
pub(crate) const SYSTEM_CONSOLE_LEFT_RATIO: f32 = 1.0 / 6.0;
pub(crate) const SYSTEM_CONSOLE_TOP_RATIO: f32 = 1.0 / 3.0;
const SYSTEM_CONSOLE_LINE_SCROLL_PX: f32 = 20.0;
const YALDABAOTH_LOGO_BYTES: &[u8] = include_bytes!("../../../yaldabaoth-logo.png");
#[cfg(not(test))]
const SYSTEM_CONSOLE_FILE: &str = "system-console.log";

/// Install the embedded artwork as the running macOS application's Dock icon.
///
/// Yalda currently launches as a bare executable rather than an `.app` bundle,
/// so there is no bundle `Info.plist` from which AppKit can load an icon.
#[cfg(target_os = "macos")]
pub(crate) fn install_yaldabaoth_app_icon() {
    use cocoa::appkit::{NSApp, NSApplication, NSImage};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSData, NSUInteger};

    // SAFETY: GPUI has initialized AppKit before its `Application::run`
    // callback. NSData copies the embedded bytes, NSImage accepts PNG data,
    // and the allocated image intentionally lives for the process lifetime
    // because NSApplication uses it as global application state.
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let data: id = NSData::dataWithBytes_length_(
            nil,
            YALDABAOTH_LOGO_BYTES.as_ptr().cast(),
            YALDABAOTH_LOGO_BYTES.len() as NSUInteger,
        );
        let image = NSImage::initWithData_(NSImage::alloc(nil), data);
        if image != nil {
            NSApp().setApplicationIconImage_(image);
        }
        pool.drain();
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn install_yaldabaoth_app_icon() {}

/// Pick the PNG bytes to stage for a pasted image, preferring a real PNG rep
/// over a TIFF that had to be transcoded, and rejecting empty payloads. Pure so
/// the preference/empty logic is headlessly testable — the mac-only FFI that
/// fetches the two pasteboard blobs lives in [`read_clipboard_image_png`].
pub(crate) fn select_clipboard_png_bytes(
    png: Option<Vec<u8>>,
    tiff_as_png: Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    png.filter(|b| !b.is_empty())
        .or_else(|| tiff_as_png.filter(|b| !b.is_empty()))
}

#[cfg(test)]
thread_local! {
    /// Test seam for [`read_clipboard_image_png`]: headless tests must NOT touch
    /// the real OS pasteboard (non-deterministic, and it would read the user's
    /// actual clipboard). Default `None` ⇒ "no image"; a test sets `Some(bytes)`
    /// to simulate a clipboard image. The real OS read is exercised only by the
    /// `#[ignore]` `read_clipboard_image_png_os_*` test.
    static CLIPBOARD_IMAGE_TEST_OVERRIDE: std::cell::RefCell<Option<Vec<u8>>> =
        const { std::cell::RefCell::new(None) };
}

/// Set the headless test override for the pasteboard image read. Passing
/// `Some(bytes)` makes the next [`read_clipboard_image_png`] return those bytes
/// without touching the OS; `None` clears it back to "no image".
#[cfg(test)]
pub(crate) fn set_clipboard_image_test_override(bytes: Option<Vec<u8>>) {
    CLIPBOARD_IMAGE_TEST_OVERRIDE.with(|c| *c.borrow_mut() = bytes);
}

/// Read a clipboard image as PNG bytes, or `None` if the board holds no image.
/// In production this reads the real macOS pasteboard
/// ([`read_clipboard_image_png_os`]); under `cfg(test)` it returns the injected
/// override (default `None`) so headless tests stay deterministic and never read
/// the developer's real clipboard.
#[cfg(not(test))]
pub(crate) fn read_clipboard_image_png() -> Option<Vec<u8>> {
    read_clipboard_image_png_os()
}

#[cfg(test)]
pub(crate) fn read_clipboard_image_png() -> Option<Vec<u8>> {
    CLIPBOARD_IMAGE_TEST_OVERRIDE.with(|c| c.borrow().clone())
}

/// Read an image directly off the macOS general pasteboard, returning PNG bytes.
///
/// This exists because GPUI 0.2.2's `read_from_clipboard`
/// (`platform/mac/platform.rs`) short-circuits to a **string-only**
/// `ClipboardItem` whenever the pasteboard advertises any `public.utf8-plain-text`
/// type — it never reaches its image branch. macOS image copies from browsers,
/// Finder, and many apps put a text/URL representation on the board alongside the
/// image, so GPUI silently drops the image and Cmd+V pastes nothing (or the URL).
/// We bypass GPUI and read `public.png` ourselves, transcoding a TIFF-only board
/// to PNG via `NSBitmapImageRep`. Returns `None` when the board holds no image.
#[cfg(target_os = "macos")]
pub(crate) fn read_clipboard_image_png_os() -> Option<Vec<u8>> {
    use cocoa::appkit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSData, NSUInteger};
    use objc::{class, msg_send, sel, sel_impl};

    // SAFETY: GPUI has initialized AppKit before its run callback, so the general
    // pasteboard is live. `dataForType:` returns a retained-by-autorelease NSData
    // (or nil); we copy its bytes into an owned Vec before draining the pool, so
    // nothing escapes the autorelease scope. `NSBitmapImageRep` is a standard
    // AppKit class reachable by name for the TIFF→PNG transcode.
    unsafe {
        let pool = NSAutoreleasePool::new(nil);

        // Copy the bytes behind an NSData into an owned Vec. `bytes` is nil for a
        // zero-length NSData (documented), which we treat as empty.
        unsafe fn nsdata_to_vec(data: id) -> Vec<u8> {
            if data == nil {
                return Vec::new();
            }
            unsafe {
                let len = NSData::length(data) as usize;
                let ptr = NSData::bytes(data) as *const u8;
                if ptr.is_null() || len == 0 {
                    return Vec::new();
                }
                std::slice::from_raw_parts(ptr, len).to_vec()
            }
        }

        let pb: id = NSPasteboard::generalPasteboard(nil);

        // Prefer a PNG representation already on the board.
        let png_data: id = pb.dataForType(NSPasteboardTypePNG);
        let png = Some(nsdata_to_vec(png_data)).filter(|b| !b.is_empty());

        // Otherwise transcode a TIFF rep to PNG via NSBitmapImageRep.
        let tiff_as_png = if png.is_some() {
            None
        } else {
            let tiff_data: id = pb.dataForType(NSPasteboardTypeTIFF);
            if tiff_data == nil {
                None
            } else {
                let rep: id = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff_data];
                if rep == nil {
                    None
                } else {
                    // NSBitmapImageFileType::PNG == 4; empty properties dict.
                    let props: id = msg_send![class!(NSDictionary), dictionary];
                    let out: id = msg_send![
                        rep,
                        representationUsingType: 4u64 as NSUInteger
                        properties: props
                    ];
                    Some(nsdata_to_vec(out)).filter(|b| !b.is_empty())
                }
            }
        };

        let result = select_clipboard_png_bytes(png, tiff_as_png);
        pool.drain();
        result
    }
}

/// Non-macOS stub: image paste falls back to GPUI's clipboard entries, which on
/// other platforms surface `ClipboardEntry::Image` without the mac short-circuit.
#[cfg(not(target_os = "macos"))]
pub(crate) fn read_clipboard_image_png_os() -> Option<Vec<u8>> {
    None
}

pub(crate) fn yaldabaoth_logo_image() -> Arc<gpui::Image> {
    static LOGO: OnceLock<Arc<gpui::Image>> = OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            YALDABAOTH_LOGO_BYTES.to_vec(),
        ))
    })
    .clone()
}

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

    pub(crate) fn scroll_by(&mut self, down: f32, cx: &mut Context<Self>) {
        let current = self.scroll.offset();
        let y = (current.y - px(down)).min(px(0.0));
        self.scroll.set_offset(gpui::point(current.x, y));
        record_notify("system_console", MissReason::Dirtied);
        cx.notify();
    }
}

fn system_console_scroll_delta(press: &KeyPress, viewport_height: f32) -> Option<f32> {
    let control = press.modifiers.contains(KMods::CONTROL);
    if control {
        let half_page = (viewport_height * SYSTEM_CONSOLE_HEIGHT_RATIO * 0.5).max(48.0);
        return match press.key {
            Key::Char('u') => Some(-half_page),
            Key::Char('d') => Some(half_page),
            _ => None,
        };
    }
    if !press.modifiers.is_empty() {
        return None;
    }
    match press.key {
        Key::Char('k') | Key::Up => Some(-SYSTEM_CONSOLE_LINE_SCROLL_PX),
        Key::Char('j') | Key::Down => Some(SYSTEM_CONSOLE_LINE_SCROLL_PX),
        _ => None,
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
        if let Some(delta) = system_console_scroll_delta(&press, self.viewport_height_px) {
            if let Some(view) = &self.system_console_view {
                view.update(cx, |view, cx| view.scroll_by(delta, cx));
            }
            return;
        }
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
            .top(gpui::relative(SYSTEM_CONSOLE_TOP_RATIO))
            .left(gpui::relative(SYSTEM_CONSOLE_LEFT_RATIO))
            .w(gpui::relative(SYSTEM_CONSOLE_WIDTH_RATIO))
            .h(gpui::relative(SYSTEM_CONSOLE_HEIGHT_RATIO))
            .bg(panel_bg)
            .border_1()
            .border_color(border)
            .rounded_md()
            .overflow_hidden()
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
            .relative()
            .flex()
            .flex_col()
            .bg(panel_bg)
            .text_color(fg)
            .font_family(mono)
            .text_size(px(11.0))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(yaldabaoth_logo_image())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .grayscale(true)
                            .opacity(0.07),
                    ),
            )
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
                            .child("[j/k ↑/↓] scroll · [^u/^d] page · [r/R] rebuild · esc close"),
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
fn system_console_geometry_stays_centered_and_compact() {
    assert!(
        (0.30..=0.36).contains(&SYSTEM_CONSOLE_HEIGHT_RATIO),
        "console should preserve roughly two thirds of the desktop"
    );
    assert_eq!(SYSTEM_CONSOLE_WIDTH_RATIO, 2.0 / 3.0);
    assert_eq!(SYSTEM_CONSOLE_TOP_RATIO, 1.0 / 3.0);
    assert_eq!(SYSTEM_CONSOLE_LEFT_RATIO, 1.0 / 6.0);
}

#[cfg(test)]
#[test]
fn system_console_navigation_uses_standard_scroll_keys() {
    let plain = KMods::NONE;
    let control = KMods::CONTROL;
    assert_eq!(
        system_console_scroll_delta(&KeyPress::new(Key::Char('j'), plain), 900.0),
        Some(SYSTEM_CONSOLE_LINE_SCROLL_PX)
    );
    assert_eq!(
        system_console_scroll_delta(&KeyPress::new(Key::Up, plain), 900.0),
        Some(-SYSTEM_CONSOLE_LINE_SCROLL_PX)
    );
    assert_eq!(
        system_console_scroll_delta(&KeyPress::new(Key::Char('d'), control), 900.0),
        Some(150.0)
    );
    assert_eq!(
        system_console_scroll_delta(&KeyPress::new(Key::Char('u'), control), 900.0),
        Some(-150.0)
    );
    assert!(YALDABAOTH_LOGO_BYTES.starts_with(b"\x89PNG\r\n\x1a\n"));
}

/// Guards the PNG-preference + empty-rejection logic that decides which
/// pasteboard blob becomes the staged image (`read_clipboard_image_png` calls
/// this after the mac FFI fetch). The mac string short-circuit is what dropped
/// pasted images (bug-0039); this is the pure half of the fix. Negative control:
/// change `png.filter(...).or_else(...)` to just `tiff_as_png` and the
/// prefers-PNG assert goes RED.
#[cfg(test)]
#[test]
fn select_clipboard_png_prefers_png_and_rejects_empty() {
    let png = vec![0x89, b'P', b'N', b'G'];
    let tiff = vec![1u8, 2, 3];

    // A real PNG rep wins over a transcoded TIFF.
    assert_eq!(
        select_clipboard_png_bytes(Some(png.clone()), Some(tiff.clone())),
        Some(png.clone())
    );
    // TIFF-only board falls back to the transcoded PNG.
    assert_eq!(
        select_clipboard_png_bytes(None, Some(tiff.clone())),
        Some(tiff.clone())
    );
    // Empty PNG is not a real image — fall through to TIFF.
    assert_eq!(
        select_clipboard_png_bytes(Some(Vec::new()), Some(tiff.clone())),
        Some(tiff)
    );
    // No image data at all.
    assert_eq!(select_clipboard_png_bytes(None, None), None);
    assert_eq!(
        select_clipboard_png_bytes(Some(Vec::new()), Some(Vec::new())),
        None
    );
}

/// Real-pasteboard round-trip for the mac image-paste fix (bug-0039). `#[ignore]`
/// because it CLOBBERS the developer's system clipboard and needs a live AppKit
/// pasteboard — this is the documented gap-2 (live OS integration) remedy, run
/// manually: `cargo test --bin yalda-gpui -- --ignored read_clipboard_image_png`.
/// Reproduces the exact failure: an image copied ALONGSIDE a text/URL rep (what
/// browsers/Finder put on the board). GPUI's `read_from_clipboard` returns the
/// string only and drops the image; `read_clipboard_image_png` must still recover
/// the PNG. Negative control: swap the body to `cx.read_from_clipboard()` and it
/// returns None because of the mac string short-circuit.
#[cfg(all(test, target_os = "macos"))]
#[test]
#[ignore]
fn read_clipboard_image_png_os_recovers_png_beside_text() {
    use cocoa::appkit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSAutoreleasePool, NSData, NSString, NSUInteger};
    use objc::{msg_send, sel, sel_impl};

    // SAFETY: standalone pasteboard access; no NSApplication run loop required.
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let pb: id = NSPasteboard::generalPasteboard(nil);
        let _: () = msg_send![pb, clearContents];

        // A real PNG (the embedded logo) plus a plain-text URL rep, mimicking a
        // browser/Finder image copy that triggers GPUI's string short-circuit.
        let png_data: id = NSData::dataWithBytes_length_(
            nil,
            YALDABAOTH_LOGO_BYTES.as_ptr().cast(),
            YALDABAOTH_LOGO_BYTES.len() as NSUInteger,
        );
        let _: bool = msg_send![pb, setData: png_data forType: NSPasteboardTypePNG];
        let url: id = NSString::alloc(nil).init_str("https://example.com/logo.png");
        let _: bool = msg_send![pb, setString: url forType: NSPasteboardTypeString];

        let got = read_clipboard_image_png_os();
        pool.drain();

        let png = got.expect("PNG must be recovered even with a text rep present");
        assert!(!png.is_empty(), "recovered PNG must be non-empty");
        assert!(
            png.starts_with(b"\x89PNG\r\n\x1a\n"),
            "recovered bytes must be a PNG"
        );
    }
}
