//! Integration test for the `sketch-channel` MCP server binary.
//!
//! Spawns the binary, drives the MCP protocol over stdin/stdout, opens a Unix
//! socket connection like sketch would, and verifies:
//!  - the `initialize` handshake declares the channel capability
//!  - sketch sends become `notifications/claude/channel`
//!  - calling the `reply` tool forwards text back to the connected client

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn binary_path() -> std::path::PathBuf {
    // Assume cargo test is run from project root and the bin is under target/debug.
    let mut p = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent")
        .to_path_buf();
    // ./target/debug/deps/<test-bin>  →  ./target/debug/sketch-channel
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("sketch-channel");
    p
}

struct Harness {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    socket_path: std::path::PathBuf,
}

impl Harness {
    fn start() -> Self {
        let bin = binary_path();
        assert!(
            bin.exists(),
            "sketch-channel binary not built at {} — run `cargo build --bin sketch-channel`",
            bin.display()
        );

        let socket_path = std::env::temp_dir().join(format!(
            "sketch-channel-int-{}-{}.sock",
            std::process::id(),
            unique()
        ));
        let _ = std::fs::remove_file(&socket_path);

        let mut child = Command::new(&bin)
            .env("SKETCH_CHANNEL_SOCKET", &socket_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sketch-channel");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        // Wait for socket to appear (the bin binds before reading stdin).
        let deadline = Instant::now() + Duration::from_secs(2);
        while !socket_path.exists() {
            if Instant::now() > deadline {
                panic!("sketch-channel never bound socket {}", socket_path.display());
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        Self {
            child,
            stdin,
            stdout,
            socket_path,
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
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

#[test]
fn initialize_declares_channel_capability() {
    let mut h = Harness::start();

    h.send_rpc(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}
    }));

    let resp = h.read_rpc();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    let caps = &resp["result"]["capabilities"];
    assert!(
        caps["experimental"]["claude/channel"].is_object(),
        "missing claude/channel capability: {}",
        resp
    );
    assert!(caps["tools"].is_object(), "missing tools capability");
    assert_eq!(resp["result"]["serverInfo"]["name"], "sketch");
}

#[test]
fn tools_list_exposes_reply() {
    let mut h = Harness::start();
    h.send_rpc(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name":"t","version":"0"}}
    }));
    let _ = h.read_rpc();
    h.send_rpc(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}));

    h.send_rpc(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    let resp = h.read_rpc();
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert!(tools.iter().any(|t| t["name"] == "reply"), "no reply tool: {}", resp);
}

#[test]
fn sketch_send_becomes_channel_notification() {
    let mut h = Harness::start();
    // Skip the initialize handshake — the bin starts reading the socket immediately.
    let mut sock = UnixStream::connect(&h.socket_path).expect("connect socket");
    sock.write_all(b"{\"type\":\"send\",\"content\":\"hello!\",\"meta\":{\"label\":\"buffer\"}}\n")
        .expect("write socket");
    sock.flush().expect("flush");

    let resp = h.read_rpc();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["method"], "notifications/claude/channel");
    assert_eq!(resp["params"]["content"], "hello!");
    assert_eq!(resp["params"]["meta"]["label"], "buffer");
}

#[test]
fn invalid_meta_keys_are_dropped() {
    let mut h = Harness::start();
    let mut sock = UnixStream::connect(&h.socket_path).expect("connect socket");
    // Hyphen + non-string value should both be dropped; underscore key kept.
    sock.write_all(
        br#"{"type":"send","content":"x","meta":{"good_key":"v","bad-key":"v","numeric":42}}
"#,
    )
    .expect("write");
    sock.flush().expect("flush");

    let resp = h.read_rpc();
    let meta = &resp["params"]["meta"];
    assert_eq!(meta["good_key"], "v");
    assert!(meta.get("bad-key").is_none());
    assert!(meta.get("numeric").is_none());
}

#[test]
fn reply_tool_forwards_to_sketch() {
    let mut h = Harness::start();
    let sock = UnixStream::connect(&h.socket_path).expect("connect socket");

    h.send_rpc(&json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}
    }));
    let _ = h.read_rpc();
    h.send_rpc(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}));

    // Trigger the reply tool — should send back over the socket.
    h.send_rpc(&json!({
        "jsonrpc":"2.0","id":99,"method":"tools/call",
        "params":{"name":"reply","arguments":{"text":"pong"}}
    }));

    // Read tool response from stdout.
    let tool_resp = h.read_rpc();
    assert_eq!(tool_resp["id"], 99);
    assert!(
        tool_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Forwarded"),
        "{}",
        tool_resp
    );

    // Read socket message.
    let mut reader = BufReader::new(sock);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read socket");
    let v: Value = serde_json::from_str(line.trim()).expect("parse");
    assert_eq!(v["type"], "reply");
    assert_eq!(v["text"], "pong");
}
