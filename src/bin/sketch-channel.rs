//! sketch-channel — a Claude Code Channels MCP server that bridges the
//! `sketch` markdown editor to a running `claude` CLI session.
//!
//! ## What it does
//!
//! 1. Speaks JSON-RPC over stdio to Claude Code (per the Channels API),
//!    declaring `experimental.claude/channel` and a `reply` tool.
//! 2. Listens on a Unix domain socket (default: `/tmp/sketch-channel-$USER.sock`,
//!    overridable via `SKETCH_CHANNEL_SOCKET`).
//! 3. When sketch sends `{"type":"send","content":"...","meta":{...}}`, we emit
//!    `notifications/claude/channel` to Claude Code.
//! 4. When Claude calls the `reply` tool, we forward the text back to the
//!    connected sketch client as `{"type":"reply","text":"..."}`.
//!
//! ## Setup
//!
//! Add to `.mcp.json` in your project:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "sketch": {
//!       "command": "/absolute/path/to/sketch-channel"
//!     }
//!   }
//! }
//! ```
//!
//! Then launch claude with the channels flag:
//!
//! ```
//! claude --dangerously-load-development-channels server:sketch
//! ```
//!
//! In sketch, `:claude-attach` (no path = use default socket) connects.
//! `:claude-send` ships the current buffer; `:claude-send-selection` ships
//! the active selection. Replies land in a `*claude*` buffer.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;

use serde_json::{Value, json};

/// (client_id, write-side of the socket). The id distinguishes generations of
/// sketch connections so a stale handler can't wipe out the slot of a fresher
/// one when it exits.
type ActiveClient = Arc<Mutex<Option<(u64, UnixStream)>>>;

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

fn next_client_id() -> u64 {
    NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed)
}

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "sketch";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTRUCTIONS: &str = "\
The 'sketch' channel relays content from the user's sketch markdown editor.

Sketch-sourced messages arrive as `<channel source=\"sketch\" label=\"buffer|selection\" \
file=\"...\">BODY</channel>`. Treat the body as a user message — they shared it from \
their editor.

CRITICAL: When responding to a sketch-sourced message, ALWAYS call the `reply` tool with \
your full response text (in addition to whatever you'd normally output in chat). The \
user has sketch open in another terminal and wants to inline-edit your reply there — \
that flow only works if your prose reaches sketch via this tool. Don't omit the tool \
call. Don't shorten or summarize the tool argument; pass the same full reply you'd \
otherwise write in chat.";

fn default_socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SKETCH_CHANNEL_SOCKET") {
        return PathBuf::from(p);
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    PathBuf::from(format!("/tmp/sketch-channel-{}.sock", user))
}

/// Optional log file (set `SKETCH_CHANNEL_LOG=/path/to/log` to enable). Useful
/// because Claude Code spawns this binary with stdio captured; stderr may not
/// be visible. Diagnostic events go here.
static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn log_file_handle() -> &'static Mutex<Option<std::fs::File>> {
    LOG_FILE.get_or_init(|| {
        let f = std::env::var_os("SKETCH_CHANNEL_LOG").and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        Mutex::new(f)
    })
}

fn log(msg: &str) {
    let line = format!("[sketch-channel] {}\n", msg);
    let _ = io::stderr().write_all(line.as_bytes());
    if let Ok(mut guard) = log_file_handle().lock()
        && let Some(ref mut f) = *guard
    {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

fn print_setup_help() {
    println!(
        "sketch-channel — Claude Code MCP channel for the sketch editor

Setup:

  1. Build (`cargo build --release --bin sketch-channel`)
  2. Add to .mcp.json in your Claude project:

     {{
       \"mcpServers\": {{
         \"sketch\": {{
           \"command\": \"{}\"
         }}
       }}
     }}

  3. Launch claude:

     claude --dangerously-load-development-channels server:sketch

  4. In sketch: `:claude-attach` (uses {})

Environment:

  SKETCH_CHANNEL_SOCKET   override the Unix socket path

This binary is normally spawned by Claude Code (not run directly). When run
without --setup, it speaks JSON-RPC on stdio.",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/path/to/sketch-channel".into()),
        default_socket_path().display(),
    );
}

fn main() -> io::Result<()> {
    // Simple flag handling — no clap to keep the binary small.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--setup" || a == "--help" || a == "-h") {
        print_setup_help();
        return Ok(());
    }

    let socket_path = default_socket_path();

    // Single owner of stdout — every message body is sent through this.
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
    let stdout_writer = thread::Builder::new()
        .name("stdout-writer".into())
        .spawn(move || {
            let stdout = io::stdout();
            let mut h = stdout.lock();
            for msg in stdout_rx {
                if h.write_all(msg.as_bytes()).is_err() {
                    return;
                }
                if h.write_all(b"\n").is_err() {
                    return;
                }
                let _ = h.flush();
            }
        })?;

    // Currently-attached sketch client (single-client model). Tagged with a
    // generational ID so an exiting handler only clears the slot if its own
    // generation is still installed.
    let active_client: ActiveClient = Arc::new(Mutex::new(None));

    // Bind socket (unlink any stale path first).
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    let listener = UnixListener::bind(&socket_path).map_err(|e| {
        log(&format!(
            "Failed to bind socket {}: {}",
            socket_path.display(),
            e
        ));
        e
    })?;
    let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    log(&format!("Listening on {}", socket_path.display()));

    // Cleanup hook: remove the socket on drop. The accept thread holds an Arc
    // to the path so we can clean up here too.
    let socket_path_for_cleanup = socket_path.clone();

    // Accept loop in a background thread.
    {
        let stdout_tx = stdout_tx.clone();
        let active_client = active_client.clone();
        thread::Builder::new()
            .name("socket-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let id = next_client_id();
                            log(&format!("sketch client #{} connected", id));
                            // Replace any prior client (single-client policy).
                            {
                                let mut guard = active_client.lock().unwrap();
                                if let Some((old_id, old)) = guard.take() {
                                    log(&format!(
                                        "displacing previous client #{}",
                                        old_id
                                    ));
                                    let _ = old.shutdown(std::net::Shutdown::Both);
                                }
                                match stream.try_clone() {
                                    Ok(write_clone) => *guard = Some((id, write_clone)),
                                    Err(e) => {
                                        log(&format!("clone for write side failed: {}", e));
                                        continue;
                                    }
                                }
                            }
                            let stdout_tx2 = stdout_tx.clone();
                            let active_client2 = active_client.clone();
                            let _ = thread::Builder::new()
                                .name("sketch-reader".into())
                                .spawn(move || {
                                    handle_sketch_client(
                                        stream,
                                        id,
                                        stdout_tx2,
                                        active_client2,
                                    );
                                });
                        }
                        Err(e) => {
                            log(&format!("accept error: {}", e));
                            break;
                        }
                    }
                }
            })?;
    }

    // Stdin → MCP dispatcher (main thread).
    let stdin = io::stdin();
    let reader = BufReader::new(stdin);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log(&format!("stdin read error: {}", e));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                log(&format!("malformed JSON-RPC: {} :: {}", e, line));
                continue;
            }
        };
        handle_mcp_message(&msg, &stdout_tx, &active_client);
    }

    // Cleanup on stdin close (Claude Code exited).
    let _ = std::fs::remove_file(&socket_path_for_cleanup);
    drop(stdout_tx);
    let _ = stdout_writer.join();
    Ok(())
}

fn handle_sketch_client(
    stream: UnixStream,
    my_id: u64,
    stdout_tx: mpsc::Sender<String>,
    active_client: ActiveClient,
) {
    let read_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log(&format!(
                "[#{}] clone for read side failed: {}",
                my_id, e
            ));
            return;
        }
    };
    let reader = BufReader::new(read_stream);
    log(&format!("[#{}] reader loop started", my_id));
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log(&format!("[#{}] read error: {}", my_id, e));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                log(&format!("[#{}] parse error: {}", my_id, e));
                continue;
            }
        };

        let msg_type = v.get("type").and_then(Value::as_str).unwrap_or("");
        if msg_type != "send" {
            continue;
        }

        let content = v.get("content").and_then(Value::as_str).unwrap_or("");
        let meta_filtered = filter_meta(v.get("meta"));

        log(&format!(
            "[#{}] forwarding send ({} chars) to claude",
            my_id,
            content.len()
        ));
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel",
            "params": {
                "content": content,
                "meta": meta_filtered,
            }
        });
        if stdout_tx.send(notification.to_string()).is_err() {
            break;
        }
    }
    log(&format!("[#{}] reader loop exited", my_id));
    // Only clear the slot if it still holds OUR generation. Otherwise a newer
    // client has taken over and we must not stomp on it.
    let mut guard = active_client.lock().unwrap();
    let still_mine = guard.as_ref().map(|(id, _)| *id == my_id).unwrap_or(false);
    if still_mine {
        log(&format!("[#{}] clearing active_client", my_id));
        *guard = None;
    } else {
        log(&format!(
            "[#{}] not clearing — slot now holds #{:?}",
            my_id,
            guard.as_ref().map(|(id, _)| *id)
        ));
    }
}

/// Validate and clone meta keys per Claude Code's constraints:
/// - keys must be alphanumeric or underscore
/// - values must be strings
fn filter_meta(meta: Option<&Value>) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(Value::Object(map)) = meta {
        for (k, v) in map {
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), Value::String(s.to_string()));
            }
        }
    }
    Value::Object(out)
}

fn handle_mcp_message(
    msg: &Value,
    stdout_tx: &mpsc::Sender<String>,
    active_client: &ActiveClient,
) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();

    match method {
        "initialize" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {
                        "experimental": {
                            "claude/channel": {}
                        },
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    },
                    "instructions": INSTRUCTIONS
                }
            });
            let _ = stdout_tx.send(resp.to_string());
        }
        "notifications/initialized" => {
            // Notification — no response.
        }
        "tools/list" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "reply",
                            "description":
                                "Relay your response back to the user's sketch editor. \
                                 The text is appended to a *claude* buffer where the \
                                 user can read and inline-edit it. Always call this when \
                                 responding to a <channel source=\"sketch\"> message — \
                                 pass your FULL response text, not a summary.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "The text to append to sketch's *claude* buffer."
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    ]
                }
            });
            let _ = stdout_tx.send(resp.to_string());
        }
        "tools/call" => {
            let params = msg.get("params");
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = params.and_then(|p| p.get("arguments"));

            let resp = if name == "reply" {
                let text = args
                    .and_then(|a| a.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let sent = forward_reply_to_sketch(active_client, text);
                let resp_text = if sent {
                    format!("Forwarded {} chars to sketch.", text.len())
                } else {
                    "No sketch client attached; reply dropped.".to_string()
                };
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": resp_text}]
                    }
                })
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Unknown tool: {}", name)
                    }
                })
            };
            let _ = stdout_tx.send(resp.to_string());
        }
        "ping" => {
            let resp = json!({"jsonrpc": "2.0", "id": id, "result": {}});
            let _ = stdout_tx.send(resp.to_string());
        }
        _ => {
            // If this was a request (has id), respond with method-not-found.
            // Notifications get silently ignored.
            if id.is_some() {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method not found: {}", method)
                    }
                });
                let _ = stdout_tx.send(resp.to_string());
            }
        }
    }
}

fn forward_reply_to_sketch(active_client: &ActiveClient, text: &str) -> bool {
    let payload = json!({"type": "reply", "text": text}).to_string();
    let mut buf = payload.into_bytes();
    buf.push(b'\n');
    let mut guard = active_client.lock().unwrap();
    match guard.as_mut() {
        Some((id, s)) => {
            log(&format!("forwarding reply to client #{} ({} bytes)", id, buf.len()));
            if s.write_all(&buf).is_ok() && s.flush().is_ok() {
                return true;
            }
            log(&format!("write to client #{} failed; dropping slot", id));
            *guard = None;
            false
        }
        None => {
            log("forward_reply_to_sketch: no client attached");
            false
        }
    }
}
