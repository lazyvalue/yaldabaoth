//! Client side of the yalda ↔ yalda-channel MCP bridge.
//!
//! `yalda-channel` is a separate binary that Claude Code spawns as an MCP
//! server (over stdio). It listens on a Unix domain socket; yalda (this
//! editor) connects to that socket and exchanges line-delimited JSON:
//!
//! - yalda → server: `{"type":"send","content":"...","meta":{...}}`
//! - server → yalda: `{"type":"reply","text":"..."}`
//!
//! The server translates outbound messages into MCP `notifications/claude/channel`
//! and inbound `reply` tool calls into messages routed back to yalda.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct OutMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    content: &'a str,
    meta: HashMap<String, String>,
}

#[derive(Deserialize)]
struct InMessage {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// A live connection to a `yalda-channel` server.
///
/// Holds the writer half of the Unix socket plus a receiver fed by a background
/// reader thread. Drop the client to disconnect.
pub struct ChannelClient {
    writer: UnixStream,
    rx: mpsc::Receiver<String>,
    socket_path: PathBuf,
    /// Set to false by the reader thread on EOF/error. The send path checks
    /// this before writing — a stale connection (e.g. yalda-channel was
    /// replaced by Claude restart) gets detected here even though the local
    /// write fd may still accept data into the kernel buffer.
    connected: Arc<AtomicBool>,
}

impl ChannelClient {
    /// Connect to a yalda-channel server listening at `path`. Spawns a reader
    /// thread that pushes inbound replies into a channel readable via `try_recv`.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let writer = UnixStream::connect(path)?;
        let reader_stream = writer.try_clone()?;

        let (tx, rx) = mpsc::channel();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_for_reader = connected.clone();
        thread::Builder::new()
            .name("yalda-claude-rx".into())
            .spawn(move || {
                let reader = BufReader::new(reader_stream);
                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => break,
                    };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(msg) = serde_json::from_str::<InMessage>(&line)
                        && msg.kind == "reply"
                        && let Some(text) = msg.text
                        && tx.send(text).is_err()
                    {
                        break;
                    }
                }
                // EOF or error → peer closed (or thread couldn't enqueue).
                connected_for_reader.store(false, Ordering::SeqCst);
            })?;

        Ok(Self {
            writer,
            rx,
            socket_path: path.to_path_buf(),
            connected,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Default socket path. Honours `YALDA_CHANNEL_SOCKET` if set, otherwise
    /// `/tmp/yalda-channel-<USER>.sock`.
    pub fn default_socket_path() -> PathBuf {
        if let Some(p) = std::env::var_os("YALDA_CHANNEL_SOCKET") {
            return PathBuf::from(p);
        }
        let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
        PathBuf::from(format!("/tmp/yalda-channel-{}.sock", user))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Send a payload to the channel. `meta` keys must be alphanumeric or
    /// underscore (the server drops invalid keys to match Claude Code's
    /// `notifications/claude/channel` constraints).
    pub fn send(&mut self, content: &str, meta: HashMap<String, String>) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "channel server gone (reader saw EOF) — re-attach to recover",
            ));
        }
        let msg = OutMessage {
            kind: "send",
            content,
            meta,
        };
        let line = serde_json::to_string(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Err(e) = self
            .writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.write_all(b"\n"))
            .and_then(|_| self.writer.flush())
        {
            self.connected.store(false, Ordering::SeqCst);
            return Err(e);
        }
        Ok(())
    }

    /// Try to receive a reply pushed by Claude via the `reply` tool. Non-blocking.
    pub fn try_recv(&self) -> Option<String> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ChannelClient {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn fresh_socket() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "yalda-channel-test-{}-{}.sock",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Spawn a fake server that records each line it receives and optionally
    /// emits replies fed via a channel.
    struct FakeServer {
        path: PathBuf,
        received: mpsc::Receiver<String>,
        send_to_client: mpsc::Sender<String>,
    }

    fn start_fake_server() -> FakeServer {
        let path = fresh_socket();
        let listener = UnixListener::bind(&path).expect("bind");
        let (recv_tx, recv_rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel::<String>();

        let path_clone = path.clone();
        thread::spawn(move || {
            // accept first client
            if let Ok((stream, _)) = listener.accept() {
                let read_stream = stream.try_clone().expect("clone");
                let mut write_stream = stream;
                // reader
                let recv_tx2 = recv_tx.clone();
                thread::spawn(move || {
                    let reader = BufReader::new(read_stream);
                    for line in reader.lines() {
                        match line {
                            Ok(l) => {
                                if recv_tx2.send(l).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
                // writer pump
                while let Ok(line) = out_rx.recv() {
                    if write_stream.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                    if write_stream.write_all(b"\n").is_err() {
                        break;
                    }
                    let _ = write_stream.flush();
                }
            }
            let _ = std::fs::remove_file(&path_clone);
        });

        FakeServer {
            path,
            received: recv_rx,
            send_to_client: out_tx,
        }
    }

    #[test]
    fn send_writes_framed_json() {
        let server = start_fake_server();
        // Give the listener a moment.
        thread::sleep(Duration::from_millis(50));
        let mut client = ChannelClient::connect(&server.path).expect("connect");
        let mut meta = HashMap::new();
        meta.insert("label".into(), "buffer".into());
        client.send("hello world", meta).expect("send");

        let received = server
            .received
            .recv_timeout(Duration::from_secs(1))
            .expect("recv");
        let v: serde_json::Value = serde_json::from_str(&received).expect("parse");
        assert_eq!(v["type"], "send");
        assert_eq!(v["content"], "hello world");
        assert_eq!(v["meta"]["label"], "buffer");
    }

    #[test]
    fn try_recv_pulls_replies() {
        let server = start_fake_server();
        thread::sleep(Duration::from_millis(50));
        let client = ChannelClient::connect(&server.path).expect("connect");

        server
            .send_to_client
            .send(r#"{"type":"reply","text":"howdy"}"#.to_string())
            .unwrap();

        // Poll briefly
        let mut got = None;
        for _ in 0..50 {
            if let Some(t) = client.try_recv() {
                got = Some(t);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got.as_deref(), Some("howdy"));
    }

    #[test]
    fn try_recv_ignores_non_reply() {
        let server = start_fake_server();
        thread::sleep(Duration::from_millis(50));
        let client = ChannelClient::connect(&server.path).expect("connect");

        // Random message type: should be ignored.
        server
            .send_to_client
            .send(r#"{"type":"junk","text":"nope"}"#.to_string())
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(client.try_recv().is_none());
    }
}
