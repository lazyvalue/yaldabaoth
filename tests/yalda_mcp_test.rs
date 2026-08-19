//! Integration test for the `yalda-mcp` MCP server binary.
//!
//! Spawns the real binary and drives the MCP protocol over stdin/stdout,
//! verifying:
//!  - the `initialize` handshake reports the `yalda` server + tools capability
//!  - `tools/list` exposes `create_session` with the agent enum (claude/codex)
//!    and `agent`/`prompt` required
//!  - calling `create_session` with no session server reachable returns a
//!    graceful `isError` tool result rather than crashing

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use serde_json::{Value, json};

fn binary_path() -> std::path::PathBuf {
    // ./target/debug/deps/<test-bin>  →  ./target/debug/yalda-mcp
    let mut p = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent")
        .to_path_buf();
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("yalda-mcp");
    p
}

struct Harness {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Harness {
    /// Start with a session socket path guaranteed NOT to have a server behind
    /// it, so `create_session` exercises the graceful-failure path.
    fn start() -> Self {
        let bin = binary_path();
        assert!(
            bin.exists(),
            "yalda-mcp binary not built at {} — run `cargo build --bin yalda-mcp`",
            bin.display()
        );

        let dead_socket = std::env::temp_dir().join(format!(
            "yalda-mcp-int-nonexistent-{}-{}.sock",
            std::process::id(),
            unique()
        ));
        let _ = std::fs::remove_file(&dead_socket);

        let mut child = Command::new(&bin)
            .env("YALDA_SESSION_SOCKET", &dead_socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn yalda-mcp");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send_rpc(&mut self, payload: &Value) {
        let line = format!("{}\n", payload);
        self.stdin.write_all(line.as_bytes()).expect("write stdin");
        self.stdin.flush().expect("flush stdin");
    }

    fn read_rpc(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read stdout");
        serde_json::from_str(line.trim()).expect("parse rpc")
    }

    fn handshake(&mut self) {
        self.send_rpc(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name":"t","version":"0"}}
        }));
        let _ = self.read_rpc();
        self.send_rpc(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}));
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

#[test]
fn initialize_reports_yalda_server() {
    let mut h = Harness::start();
    h.send_rpc(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}
    }));
    let resp = h.read_rpc();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "yalda");
    assert!(
        resp["result"]["capabilities"]["tools"].is_object(),
        "missing tools capability: {}",
        resp
    );
}

#[test]
fn tools_list_exposes_create_session() {
    let mut h = Harness::start();
    h.handshake();

    h.send_rpc(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let resp = h.read_rpc();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let cs = tools
        .iter()
        .find(|t| t["name"] == "create_session")
        .unwrap_or_else(|| panic!("no create_session tool: {}", resp));

    // agent is an enum of exactly claude + codex.
    let agent_enum = cs["inputSchema"]["properties"]["agent"]["enum"]
        .as_array()
        .expect("agent enum");
    let variants: Vec<&str> = agent_enum.iter().filter_map(Value::as_str).collect();
    assert!(
        variants.contains(&"claude") && variants.contains(&"codex"),
        "agent enum should offer claude + codex: {:?}",
        variants
    );

    // agent + prompt are required.
    let required: Vec<&str> = cs["inputSchema"]["required"]
        .as_array()
        .expect("required array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(required.contains(&"agent"), "agent must be required");
    assert!(required.contains(&"prompt"), "prompt must be required");
}

#[test]
fn create_session_without_server_returns_graceful_error() {
    let mut h = Harness::start(); // points at a dead session socket
    h.handshake();

    h.send_rpc(&json!({
        "jsonrpc":"2.0","id":42,"method":"tools/call",
        "params":{"name":"create_session","arguments":{"agent":"claude","prompt":"do a thing"}}
    }));
    let resp = h.read_rpc();
    assert_eq!(resp["id"], 42);
    // A protocol-level "result" (not "error"), flagged isError, with a helpful
    // message — NOT a crash / dropped connection.
    assert_eq!(
        resp["result"]["isError"], true,
        "expected a graceful isError result: {}",
        resp
    );
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("session server") || text.contains("Yalda running"),
        "error text should explain the server is unreachable: {}",
        text
    );
}

#[test]
fn create_session_rejects_unknown_agent() {
    let mut h = Harness::start();
    h.handshake();
    h.send_rpc(&json!({
        "jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"create_session","arguments":{"agent":"gpt","prompt":"x"}}
    }));
    let resp = h.read_rpc();
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("Invalid 'agent'"),
        "expected invalid-agent message: {}",
        text
    );
}
