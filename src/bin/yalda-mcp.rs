//! yalda-mcp — a Model Context Protocol server that lets an agent control
//! Yalda. It speaks JSON-RPC over stdio (the MCP stdio transport) and drives
//! the running `yalda-session-server` over its Unix socket.
//!
//! ## What it does
//!
//! Exposes one tool, `create_session`, which starts a brand-new Yalda agent
//! session and delivers an initial prompt to it:
//!
//! - `agent`  — which agent backs the session: `"claude"` or `"codex"`.
//! - `prompt` — the first message to send once the session exists.
//! - `cwd`    — optional working directory for the session (defaults to this
//!              process's current dir, i.e. the caller session's project root).
//! - `label`  — optional human-readable label for the session.
//!
//! Internally it connects to the already-running session server
//! (`SessionServerClient::connect_existing`), issues `create_session` with the
//! chosen `AgentProvider`, then `admin_prompt` to enqueue the initial prompt
//! headlessly (ADR-0015 — no ownership required, definitive Ack/Error).
//!
//! ## Injection
//!
//! This binary is auto-registered as an MCP server on every agent session Yalda
//! spawns (see `acp_channel::yalda_mcp_servers`), so an agent running *inside*
//! Yalda can recursively spin up more Yalda sessions.
//!
//! ## Manual setup
//!
//! Add to `.mcp.json` in a project (only needed for an agent Yalda did NOT
//! spawn):
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "yalda": { "command": "/absolute/path/to/yalda-mcp" }
//!   }
//! }
//! ```
//!
//! Run with `--setup`/`--help` for the same guidance. Otherwise it speaks
//! JSON-RPC on stdio and is normally spawned by an ACP agent.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;

use serde_json::{Value, json};

use yalda::acp_channel::AgentProvider;
use yalda::session_client::SessionServerClient;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "yalda";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTRUCTIONS: &str = "\
The 'yalda' MCP server controls the user's Yalda agentic workspace.

Use `create_session` to start a new autonomous agent session inside Yalda: pick \
the agent backend ('claude' or 'codex') and give it an initial prompt describing \
the task. The session runs to completion on its own; the tool returns the new \
session id. Use this to delegate work to a fresh agent that has its own Yalda \
tile, transcript, and working directory.";

/// Optional log file (set `YALDA_MCP_LOG=/path/to/log` to enable). Useful
/// because an ACP agent spawns this binary with stdio captured; stderr may not
/// be visible. Diagnostic events go here.
static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn log_file_handle() -> &'static Mutex<Option<std::fs::File>> {
    LOG_FILE.get_or_init(|| {
        let f = std::env::var_os("YALDA_MCP_LOG").and_then(|p| {
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
    let line = format!("[yalda-mcp] {}\n", msg);
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
        "yalda-mcp — Model Context Protocol server to control Yalda

Setup (only for an agent Yalda did NOT spawn — spawned agents get this
automatically):

  1. Build (`cargo build --release --bin yalda-mcp`)
  2. Add to .mcp.json in your project:

     {{
       \"mcpServers\": {{
         \"yalda\": {{
           \"command\": \"{}\"
         }}
       }}
     }}

Tool:

  create_session(agent: \"claude\"|\"codex\", prompt, cwd?, label?)
    Start a new Yalda agent session and send it an initial prompt.

Environment:

  YALDA_MCP_LOG          write diagnostics to this file path
  YALDA_SESSION_SOCKET   override the session-server Unix socket path

This binary is normally spawned by an ACP agent (not run directly). When run
without --setup, it speaks JSON-RPC on stdio.",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/path/to/yalda-mcp".into()),
    );
}

fn main() -> io::Result<()> {
    // Simple flag handling — no clap, to keep the binary small (mirrors
    // yalda-channel).
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|a| a == "--setup" || a == "--help" || a == "-h")
    {
        print_setup_help();
        return Ok(());
    }

    // Single owner of stdout — every response frame goes through this so
    // concurrent handlers can't interleave partial lines.
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

    log("started; reading MCP frames on stdin");

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
        handle_mcp_message(&msg, &stdout_tx);
    }

    log("stdin closed; exiting");
    drop(stdout_tx);
    let _ = stdout_writer.join();
    Ok(())
}

fn create_session_tool_schema() -> Value {
    json!({
        "name": "create_session",
        "description":
            "Start a NEW autonomous agent session inside the user's Yalda \
             workspace and send it an initial prompt. Pick the agent backend \
             ('claude' or 'codex'). The session gets its own tile, transcript, \
             and working directory, and runs the prompt to completion on its \
             own. Returns the new session id.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "enum": ["claude", "codex"],
                    "description": "Which agent backs the session: 'claude' or 'codex'."
                },
                "prompt": {
                    "type": "string",
                    "description": "The initial prompt / task to send to the new session."
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory for the session. \
                                    Defaults to the current directory."
                },
                "label": {
                    "type": "string",
                    "description": "Optional human-readable label for the session."
                }
            },
            "required": ["agent", "prompt"]
        }
    })
}

fn handle_mcp_message(msg: &Value, stdout_tx: &mpsc::Sender<String>) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();

    match method {
        "initialize" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
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
                "result": { "tools": [ create_session_tool_schema() ] }
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

            let resp = if name == "create_session" {
                match call_create_session(args) {
                    Ok(text) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [{"type": "text", "text": text}] }
                    }),
                    Err(text) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": text}],
                            "isError": true
                        }
                    }),
                }
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Unknown tool: {}", name) }
                })
            };
            let _ = stdout_tx.send(resp.to_string());
        }
        "ping" => {
            let resp = json!({"jsonrpc": "2.0", "id": id, "result": {}});
            let _ = stdout_tx.send(resp.to_string());
        }
        _ => {
            // Requests (with id) get method-not-found; notifications are ignored.
            if id.is_some() {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Method not found: {}", method) }
                });
                let _ = stdout_tx.send(resp.to_string());
            }
        }
    }
}

/// Execute the `create_session` tool. Returns `Ok(text)` on success or
/// `Err(text)` with a human-readable failure message (surfaced to the agent as
/// an `isError` tool result rather than a protocol error).
fn call_create_session(args: Option<&Value>) -> Result<String, String> {
    let agent_str = args
        .and_then(|a| a.get("agent"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let provider = match agent_str.to_ascii_lowercase().as_str() {
        "claude" => AgentProvider::Claude,
        "codex" => AgentProvider::Codex,
        other => {
            return Err(format!(
                "Invalid 'agent': {:?}. Expected \"claude\" or \"codex\".",
                other
            ));
        }
    };

    let prompt = args
        .and_then(|a| a.get("prompt"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if prompt.trim().is_empty() {
        return Err("Missing 'prompt': provide an initial task for the session.".to_string());
    }

    let cwd: PathBuf = args
        .and_then(|a| a.get("cwd"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let label = args
        .and_then(|a| a.get("label"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} session", agent_str));

    log(&format!(
        "create_session: agent={} cwd={} label={:?}",
        agent_str,
        cwd.display(),
        label
    ));

    // Connect to the ALREADY-RUNNING server only — an MCP call must never spawn
    // a throwaway daemon (ADR-0015).
    let client = SessionServerClient::connect_existing().map_err(|e| {
        format!(
            "Could not reach the Yalda session server: {}. Is Yalda running?",
            e
        )
    })?;

    let info = client
        .create_session_with_provider(cwd.clone(), label.clone(), provider, None)
        .map_err(|e| format!("create_session failed: {}", e))?;

    // Deliver the initial prompt headlessly (no ownership, definitive Ack).
    client
        .admin_prompt(&info.session_id, prompt)
        .map_err(|e| format!("session {} created but the initial prompt failed: {}", info.session_id, e))?;

    Ok(format!(
        "Created {} session {} in {} and sent the initial prompt.",
        agent_str,
        info.session_id,
        cwd.display()
    ))
}
