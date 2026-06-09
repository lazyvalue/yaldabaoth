//! launchd LaunchAgent integration (macOS): make `sketch-session-server` a
//! **supervised, start-at-login** daemon so agent sessions keep running with no
//! GUI present — and get restarted automatically if the server crashes.
//!
//! Without this the server only exists because a GUI auto-launched it
//! (`SessionServerClient::connect_or_launch`); nothing restarts it if it dies
//! while no GUI is around, and it doesn't start at login. A per-user
//! **LaunchAgent** fixes both: `RunAtLoad` starts it at login, `KeepAlive`
//! (restart-on-failure) re-spawns it on a crash. Sessions survive the handoff
//! because each one's transcript is in its durable WAL (ADR-0009) — a killed
//! server's sessions are recovered by its replacement.
//!
//! We deliberately do NOT use launchd **socket activation** (lazy start on first
//! connect): the whole point here is to be *always present* for headless agents,
//! which is the opposite of lazy start. `RunAtLoad` + `KeepAlive` + the existing
//! single-instance guard is the right shape (see ADR-0013).
//!
//! Managed via subcommands: `sketch-session-server install | uninstall | status`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// launchd job label. Also the plist filename stem.
pub const LABEL: &str = "com.sketch.session-server";

/// `~/Library/LaunchAgents/com.sketch.session-server.plist`.
pub fn plist_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist"))
    })
}

/// Where the supervised server's stdout/stderr go — the same log the
/// GUI-auto-launched server uses, so diagnostics land in one place.
pub fn log_path() -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join("sketch").join("session-server.log"))
        .unwrap_or_else(|| PathBuf::from("/tmp/sketch-session-server.log"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render the LaunchAgent plist. `exe` is the absolute path to this binary;
/// `log` is the stdout/stderr destination.
///
/// `KeepAlive` uses `SuccessfulExit=false`: restart on a *crash* (non-zero /
/// signal) but NOT on a clean exit. That matters because the single-instance
/// guard exits 0 when another server already owns the socket — without
/// `SuccessfulExit=false` that clean exit would be restarted in a tight loop.
pub fn launch_agent_plist(exe: &Path, log: &Path) -> String {
    let exe = xml_escape(&exe.display().to_string());
    let log = xml_escape(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#
    )
}

/// Install + load the LaunchAgent. Writes the plist, hands off any
/// currently-running server (SIGTERM — its sessions are recovered from their
/// WALs by the launchd-started replacement), then loads the job so launchd
/// starts and supervises the canonical instance.
pub fn install() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let plist = plist_path().ok_or_else(|| io::Error::other("no home directory"))?;
    let log = log_path();

    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&plist, launch_agent_plist(&exe, &log))?;

    // Remove any prior incarnation of the job, ignore if not loaded.
    let _ = launchctl(&["unload", "-w", &plist.display().to_string()]);

    // Hand off any running (e.g. GUI-auto-launched) server so launchd's becomes
    // the socket owner. SIGTERM is graceful; the WAL makes the handoff lossless.
    handoff_running_server();

    // Load → launchd starts the supervised server (RunAtLoad).
    launchctl(&["load", "-w", &plist.display().to_string()])?;

    println!("Installed and loaded {LABEL}.");
    println!("  plist: {}", plist.display());
    println!("  log:   {}", log.display());
    println!("The session server now starts at login and restarts on crash.");
    Ok(())
}

/// Unload + remove the LaunchAgent. The running server gets SIGTERM from launchd
/// on unload and exits; its sessions remain in their WALs for the next start.
pub fn uninstall() -> io::Result<()> {
    let plist = plist_path().ok_or_else(|| io::Error::other("no home directory"))?;
    if plist.exists() {
        let _ = launchctl(&["unload", "-w", &plist.display().to_string()]);
        std::fs::remove_file(&plist)?;
        println!("Uninstalled {LABEL} (removed {}).", plist.display());
    } else {
        println!(
            "{LABEL} is not installed (no plist at {}).",
            plist.display()
        );
    }
    Ok(())
}

/// Report whether the LaunchAgent is installed, loaded, and whether the socket
/// is currently accepting connections.
pub fn status() -> io::Result<()> {
    let plist = plist_path().ok_or_else(|| io::Error::other("no home directory"))?;
    let installed = plist.exists();
    let loaded = Command::new("launchctl")
        .arg("list")
        .arg(LABEL)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let socket = socket_path();
    let listening = std::os::unix::net::UnixStream::connect(&socket).is_ok();

    println!("{LABEL}");
    println!("  installed (plist present): {installed}");
    println!("  loaded (launchctl):        {loaded}");
    println!(
        "  socket listening:          {listening}  ({})",
        socket.display()
    );
    if !installed {
        println!("  → run `sketch-session-server install` to supervise it via launchd.");
    }
    Ok(())
}

/// SIGTERM any server currently listening, so a freshly-loaded launchd job can
/// bind the socket. Best-effort: read the pid file and `kill` it; fall back to
/// nothing if there's no pid file. Waits briefly for the socket to free.
fn handoff_running_server() {
    let pid_path = pid_file_path();
    let Ok(pid) = std::fs::read_to_string(&pid_path) else {
        return;
    };
    let pid = pid.trim();
    if pid.is_empty() {
        return;
    }
    // SIGTERM (graceful): the server persists nothing extra — its WALs are
    // already durable — and exits, freeing the socket.
    let _ = Command::new("kill").arg(pid).status();
    // Wait up to ~2s for the socket to stop accepting, so launchd's load binds
    // cleanly rather than racing the dying server.
    let socket = socket_path();
    for _ in 0..40 {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn launchctl(args: &[&str]) -> io::Result<()> {
    let status = Command::new("launchctl").args(args).status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "`launchctl {}` failed: {status}",
            args.join(" ")
        )));
    }
    Ok(())
}

// `socket_path` / `pid_file_path` come from the proto crate; re-import here so
// the launchd helpers can find the running instance.
use sketch::session_proto::{pid_file_path, socket_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_required_keys() {
        let p = launch_agent_plist(
            Path::new("/Applications/sketch.app/Contents/MacOS/sketch-session-server"),
            Path::new("/Users/x/Library/Caches/sketch/session-server.log"),
        );
        assert!(p.contains("<string>com.sketch.session-server</string>"));
        assert!(p.contains("/Applications/sketch.app/Contents/MacOS/sketch-session-server"));
        assert!(p.contains("<key>RunAtLoad</key>"));
        // KeepAlive must be the SuccessfulExit=false dict form, NOT bare <true/>,
        // so the single-instance clean-exit guard can't cause a restart loop.
        assert!(p.contains("<key>KeepAlive</key>"));
        assert!(p.contains("<key>SuccessfulExit</key>"));
        assert!(p.contains("<false/>"));
        assert!(p.contains("session-server.log"));
        // Well-formed-ish: one plist open/close.
        assert_eq!(p.matches("<plist").count(), 1);
        assert_eq!(p.matches("</plist>").count(), 1);
    }

    #[test]
    fn plist_escapes_xml_special_chars_in_paths() {
        let p = launch_agent_plist(
            Path::new("/tmp/weird & <path>/sketch-session-server"),
            Path::new("/tmp/log"),
        );
        assert!(p.contains("/tmp/weird &amp; &lt;path&gt;/sketch-session-server"));
        // The raw, unescaped form must NOT appear.
        assert!(!p.contains("weird & <path>"));
    }
}
