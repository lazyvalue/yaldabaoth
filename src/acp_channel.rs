//! ACP (Agent Client Protocol) channel for sketch.
//!
//! This module is an alternative path to the existing
//! [`claude_channel`](crate::claude_channel) UNIX-socket integration. Where the
//! sketch-channel route requires Claude Code to be running and to have spawned
//! sketch-channel as an MCP server, the ACP route lets sketch *itself* spawn a
//! local agent subprocess and talk to it directly over JSON-RPC stdio (the
//! [Agent Client Protocol](https://agentclientprotocol.com/)). That means
//! sketch can ride the user's Claude Max subscription via the
//! `claude-agent-acp` (formerly `@zed-industries/claude-code-acp`) adapter
//! without ever touching an API key — Claude Code handles auth itself.
//!
//! ## Architecture
//!
//! The official `agent-client-protocol` crate is async/Tokio-based, but the
//! rest of sketch is sync (single-threaded `App::run` loop with
//! `crossterm::event::poll`). To bridge this without rewriting `app.rs`, this
//! module follows the same pattern as `claude_channel.rs`:
//!
//! 1. Spawn a dedicated background **worker thread** that owns a multi-thread
//!    Tokio runtime.
//! 2. Inside that runtime, spawn the agent subprocess and run the ACP
//!    `Client.builder().connect_with(...)` driver loop. The closure stays
//!    alive for the lifetime of the connection — when sketch's drop signal
//!    fires, the closure returns and the worker thread tears the runtime
//!    down.
//! 3. Communicate between sketch (sync) and the worker (async) via two
//!    `std::sync::mpsc` channels:
//!       - **outbound**: `(prompt: String) -> ()` — `App` pushes prompts here
//!         on `:claude-acp-send`. The worker async-loop drains this channel
//!         and forwards each prompt to the active ACP session.
//!       - **inbound**: `(reply_chunk: String) -> ()` — the worker pushes
//!         each `agent_message_chunk` text payload here as it arrives. `App`
//!         drains it on every tick (`pump_acp_replies`) and splices into
//!         the `*claude*` buffer via the same `append_to_claude_buffer` path
//!         used by the UNIX-socket variant.
//!
//! ## What's wired up vs. what's not
//!
//! - Initialize handshake, `session/new`, `session/prompt`, streaming
//!   `agent_message_chunk` — all working.
//! - Cancellation: dropping `AcpChannelClient` signals the worker to exit,
//!   which kills the child process. There is also an explicit `detach()`.
//! - **Permission requests**: the agent may ask the client to approve tool
//!   use (`session/request_permission`). For now we auto-decline so the agent
//!   can't surprise-write files; future work could surface a prompt in the
//!   sketch UI. The agent can still respond with text (its own commentary)
//!   even without tool permission.
//! - **Tool calls / thoughts / plans**: ignored at this layer — only
//!   `agent_message_chunk` text is plumbed back to the buffer. Extending this
//!   to render tool calls inline would mean picking a richer reply format and
//!   teaching `append_to_claude_buffer` to handle it.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread::{self, JoinHandle};

use agent_client_protocol::schema::{
    ContentBlock, ContentChunk, InitializeRequest, NewSessionRequest, ProtocolVersion,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Emit a diagnostic line to stderr when SKETCH_ACP_DEBUG is set in the env.
/// Gated this way so the chatter doesn't corrupt sketch's TUI in normal use
/// but is one env-var away when something looks wrong.
macro_rules! acp_debug {
    ($($arg:tt)*) => {
        if std::env::var("SKETCH_ACP_DEBUG").is_ok() {
            eprintln!("[sketch-acp] {}", format_args!($($arg)*));
        }
    };
}

/// Default agent command. Resolved by `which`-style PATH lookup at spawn time.
///
/// `claude-agent-acp` is the npm-installed adapter that wraps Claude Code in
/// an ACP-compatible server. It used to be `@zed-industries/claude-code-acp`
/// (which still works under the older binary name `claude-code-acp`).
pub const DEFAULT_AGENT_COMMAND: &str = "claude-agent-acp";

/// A live ACP connection to a locally-spawned agent subprocess.
///
/// API mirrors `claude_channel::ChannelClient` so that `app.rs` can drive
/// either by trait-like sniffing without inheriting any of the protocol
/// details.
pub struct AcpChannelClient {
    /// Outbound prompts: `App::claude_acp_send_text` → worker.
    prompt_tx: std_mpsc::Sender<String>,
    /// Inbound text chunks: worker → `App::pump_acp_replies`.
    reply_rx: std_mpsc::Receiver<String>,
    /// Shared liveness flag. Worker flips this to false on EOF/error/exit;
    /// `App` checks it before sending and on each pump tick.
    connected: Arc<AtomicBool>,
    /// Joined on Drop so the worker has a chance to clean up the runtime
    /// (kill the child, drop streams) before sketch exits.
    worker: Option<JoinHandle<()>>,
    /// Cosmetic: surfaced via `:claude-acp-status` so the user can see what
    /// command was spawned.
    command: String,
    /// Cosmetic: working directory the session was started in (echoed to the
    /// user; the agent itself sees this via `session/new`).
    cwd: PathBuf,
}

impl AcpChannelClient {
    /// Spawn an ACP agent process and complete the initialize/new-session
    /// handshake.
    ///
    /// `command_str` is parsed shell-style: `"claude-agent-acp --debug"` →
    /// argv `["claude-agent-acp", "--debug"]`. If empty, `DEFAULT_AGENT_COMMAND`
    /// is used.
    ///
    /// Returns once the initial handshake (initialize → new session) has
    /// completed; subsequent `send`/`try_recv` calls drive prompts in and
    /// pull streamed text out.
    pub fn spawn(command_str: &str, cwd: Option<PathBuf>) -> io::Result<Self> {
        let command = if command_str.trim().is_empty() {
            DEFAULT_AGENT_COMMAND.to_string()
        } else {
            command_str.trim().to_string()
        };
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
        });

        let parts = shell_words::split(&command).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("could not parse agent command: {e}"),
            )
        })?;
        if parts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent command was empty",
            ));
        }

        let (prompt_tx, prompt_rx) = std_mpsc::channel::<String>();
        let (reply_tx, reply_rx) = std_mpsc::channel::<String>();
        let (ready_tx, ready_rx) = std_mpsc::channel::<io::Result<()>>();
        let connected = Arc::new(AtomicBool::new(true));
        let connected_for_worker = connected.clone();

        let worker_cwd = cwd.clone();
        let worker = thread::Builder::new()
            .name("sketch-acp-worker".into())
            .spawn(move || {
                run_worker(parts, worker_cwd, prompt_rx, reply_tx, ready_tx, connected_for_worker);
            })?;

        // Wait for the initialize+new-session handshake to either succeed or
        // fail. We drop the channel afterwards — readiness is signalled once.
        let initial = ready_rx.recv().map_err(|_| {
            io::Error::other("acp worker exited before reporting readiness")
        })?;
        if let Err(e) = initial {
            // The worker has bailed; tear it down before returning.
            connected.store(false, Ordering::SeqCst);
            let _ = worker.join();
            return Err(e);
        }

        Ok(Self {
            prompt_tx,
            reply_rx,
            connected,
            worker: Some(worker),
            command,
            cwd,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// User-facing label for `:claude-acp-status`.
    pub fn description(&self) -> String {
        format!("ACP agent: {} (cwd: {})", self.command, self.cwd.display())
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// Send a prompt to the agent. Returns Err if the worker has died (e.g.
    /// the child crashed) so the caller can drop the connection.
    pub fn send(&mut self, prompt: &str) -> io::Result<()> {
        if !self.is_connected() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ACP agent gone (worker exited) — re-attach to recover",
            ));
        }
        self.prompt_tx.send(prompt.to_string()).map_err(|_| {
            self.connected.store(false, Ordering::SeqCst);
            io::Error::new(io::ErrorKind::BrokenPipe, "ACP worker channel closed")
        })
    }

    /// Pull one streamed reply chunk if any are queued. Non-blocking —
    /// safe to call every tick.
    pub fn try_recv(&self) -> Option<String> {
        self.reply_rx.try_recv().ok()
    }
}

impl Drop for AcpChannelClient {
    fn drop(&mut self) {
        // **Order matters**: we must drop `prompt_tx` BEFORE waiting on the
        // worker, otherwise the worker's bridge task is still blocked on
        // `prompt_rx.recv()` and will never let the driver loop unwind.
        // Replace the Sender with a fresh dummy that we then drop, which
        // guarantees the kernel-side close happens here regardless of
        // field-drop ordering.
        self.connected.store(false, Ordering::SeqCst);
        // Dropping prompt_tx by replacing it: we can't move out of `&mut self`
        // for a `Sender<String>`, but we *can* swap it with a fresh disposable
        // pair whose receiver we throw away — that releases the original tx.
        let (dummy_tx, _dummy_rx) = std_mpsc::channel::<String>();
        let real_tx = std::mem::replace(&mut self.prompt_tx, dummy_tx);
        drop(real_tx);

        if let Some(handle) = self.worker.take() {
            // Now safe to join: the worker's blocking recv is unblocked,
            // bridge_task exits, async_prompt_rx returns None, the driver
            // loop returns, connect_with returns, and the runtime drops.
            let _ = handle.join();
        }
    }
}

/// Internal: messages forwarded from the agent's notification handler to the
/// async driver task that owns the std-mpsc reply channel.
enum WorkerEvent {
    /// Streaming text chunk to splice into the *claude* buffer.
    Chunk(String),
}

fn run_worker(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<String>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
) {
    // Build a small multi-thread runtime — the ACP crate spawns several
    // tasks internally (read loop, write loop, response router) and a
    // current-thread runtime can deadlock if any of them blocks the only
    // worker. Two threads is plenty.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            connected.store(false, Ordering::SeqCst);
            let _ = ready_tx.send(Err(io::Error::other(format!("tokio runtime: {e}"))));
            return;
        }
    };

    let connected_for_async = connected.clone();
    let result = rt.block_on(async move {
        worker_async(parts, cwd, prompt_rx, reply_tx, ready_tx, connected_for_async).await
    });
    if let Err(e) = result {
        // Errors after the readiness handshake bubble out here — log to
        // stderr (sketch is in alt-screen, so this is mostly diagnostic for
        // people running with `2>log`).
        connected.store(false, Ordering::SeqCst);
        eprintln!("[sketch-acp] worker exited with error: {e}");
    }
    // Runtime drops here, killing any straggling tokio tasks (and the child
    // process via Drop on tokio::process::Child).
    connected.store(false, Ordering::SeqCst);
}

async fn worker_async(
    parts: Vec<String>,
    cwd: PathBuf,
    prompt_rx: std_mpsc::Receiver<String>,
    reply_tx: std_mpsc::Sender<String>,
    ready_tx: std_mpsc::Sender<io::Result<()>>,
    connected: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1) Spawn the agent process.
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..]);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Pipe-and-discard the agent's stderr by default. Agents (including
        // claude-agent-acp) may log diagnostics there; keeping it inherit
        // would corrupt sketch's TUI. Set SKETCH_ACP_AGENT_STDERR=inherit to
        // surface it for debugging.
        .stderr(if std::env::var("SKETCH_ACP_AGENT_STDERR").as_deref() == Ok("inherit") {
            std::process::Stdio::inherit()
        } else {
            std::process::Stdio::null()
        })
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to spawn '{}': {}", parts[0], e),
            )));
            return Ok(());
        }
    };
    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = ready_tx.send(Err(io::Error::other("failed to open child stdin")));
            return Ok(());
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = ready_tx.send(Err(io::Error::other("failed to open child stdout")));
            return Ok(());
        }
    };

    // 2) Set up an inbound text-chunk channel from the notification handler
    //    into the driver task. We ferry through tokio::mpsc so the
    //    notification callback (async) and the prompt driver loop (also
    //    async, in the same runtime) can both touch it without locks.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<WorkerEvent>();

    // The inbound-chunk pump: drains event_rx and pushes onto the std mpsc
    // that App::pump_acp_replies polls. Run as a separate task so it stays
    // alive even while the driver is awaiting send_request.
    let reply_tx_for_pump = reply_tx.clone();
    let connected_for_pump = connected.clone();
    let pump_task = tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            match ev {
                WorkerEvent::Chunk(text) => {
                    if reply_tx_for_pump.send(text).is_err() {
                        // App side dropped the receiver — connection torn
                        // down. Stop pumping; the driver loop will notice
                        // when it tries to read prompts.
                        connected_for_pump.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
    });

    // 3) Bridge the std mpsc prompt channel into a tokio mpsc the async
    //    driver loop can await on. spawn_blocking holds the std recv() call.
    let (async_prompt_tx, mut async_prompt_rx) =
        tokio::sync::mpsc::unbounded_channel::<String>();
    let bridge_task = tokio::task::spawn_blocking(move || {
        while let Ok(prompt) = prompt_rx.recv() {
            if async_prompt_tx.send(prompt).is_err() {
                break;
            }
        }
        // Sender side dropped → done. Closing async_prompt_tx (by drop here)
        // signals the driver loop to exit cleanly.
    });

    // 4) Run the ACP client. The closure passed to connect_with stays alive
    //    until we explicitly return — that's our "session lifetime".
    let event_tx_for_handlers = event_tx.clone();
    let connect_result = Client
        .builder()
        .name("sketch")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                acp_debug!("notification: {:?}", notification.update);
                // Extract text from chunks; ignore everything else for now.
                if let SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(text),
                    ..
                }) = notification.update
                {
                    let _ = event_tx_for_handlers.send(WorkerEvent::Chunk(text.text));
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |_req: RequestPermissionRequest, responder, _cx| {
                // For now: decline. The agent can still respond with text;
                // it just can't run tools. Surfacing a permission UI is
                // future work.
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat()),
            move |connection: ConnectionTo<Agent>| {
                let cwd = cwd.clone();
                let ready_tx = ready_tx;
                async move {
                    acp_debug!("sending initialize");
                    // === Initialize handshake ===
                    if let Err(e) = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await
                    {
                        let _ = ready_tx.send(Err(io::Error::other(format!(
                            "ACP initialize failed: {e}"
                        ))));
                        return Err(e);
                    }

                    // === Create a session bound to the cwd ===
                    let session_resp = match connection
                        .send_request(NewSessionRequest::new(cwd))
                        .block_task()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            let _ = ready_tx.send(Err(io::Error::other(format!(
                                "ACP new session failed: {e}"
                            ))));
                            return Err(e);
                        }
                    };
                    let session_id = session_resp.session_id;
                    acp_debug!("session ready: {session_id:?}");

                    // Handshake done — App can start sending.
                    let _ = ready_tx.send(Ok(()));

                    // === Driver loop: forward prompts as session/prompt
                    //     requests until the App side closes the channel.  ===
                    while let Some(prompt) = async_prompt_rx.recv().await {
                        acp_debug!("prompt → agent: {prompt:?}");
                        // Each prompt is a turn — fire and forget the
                        // request future. Streamed chunks reach the user
                        // through the notification handler, which doesn't
                        // need to live inside this loop.
                        let req = agent_client_protocol::schema::PromptRequest::new(
                            session_id.clone(),
                            vec![ContentBlock::Text(
                                agent_client_protocol::schema::TextContent::new(prompt),
                            )],
                        );
                        // Block on the prompt response (turn end) before
                        // accepting the next prompt — simpler than
                        // managing concurrent in-flight turns and matches
                        // how a chat UI flows.
                        match connection.send_request(req).block_task().await {
                            Ok(resp) => acp_debug!("prompt response: {resp:?}"),
                            Err(e) => eprintln!("[sketch-acp] prompt failed: {e}"),
                        }
                    }
                    acp_debug!("driver loop exiting");
                    Ok::<_, agent_client_protocol::Error>(())
                }
            },
        )
        .await;

    acp_debug!("connect_with returned: {:?}", connect_result.is_ok());
    // Tear down regardless of outcome. Drop the local event_tx, then abort
    // pump_task — even if the SDK is still holding a handler closure (and
    // therefore a clone of `event_tx_for_handlers`), abort makes sure we
    // don't dangle. Same for bridge_task: spawn_blocking can outlive the
    // runtime drop, and aborting is a no-op for already-finished tasks.
    drop(event_tx);
    pump_task.abort();
    bridge_task.abort();
    let _ = pump_task.await;
    // bridge_task is spawn_blocking; abort signals its JoinHandle but the
    // OS thread inside isn't actually killable. Hence we don't await it
    // here — its still-blocked recv() will return Err once prompt_tx (held
    // by `AcpChannelClient`) is dropped, then the closure returns and the
    // OS thread exits naturally on the next runtime tick.
    drop(child);

    if let Err(e) = connect_result {
        return Err(Box::new(io::Error::other(format!("acp connection: {e}"))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    /// Build a tiny Python script that pretends to be an ACP agent: it
    /// answers `initialize` and `session/new`, then for each `session/prompt`
    /// streams two `agent_message_chunk` notifications and a final
    /// `PromptResponse` with `endTurn`. Everything is line-delimited JSON
    /// per ACP framing.
    ///
    /// This is intentionally hand-rolled rather than going through the
    /// agent-side of the SDK so the test exercises the wire format we
    /// actually care about — what real agents emit.
    fn write_fake_agent_script(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("fake_acp_agent.py");
        let script = r#"#!/usr/bin/env python3
import sys, json

def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

# Use readline (not `for line in sys.stdin:`) so we react as each request
# arrives rather than buffering until EOF.
while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    method = msg.get("method", "")
    msg_id = msg.get("id")
    if method == "initialize":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"sessionId": "sess-1"}})
    elif method == "session/prompt":
        # Stream two chunks, then return.
        emit({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "hello "}}}})
        emit({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": "sess-1",
                         "update": {"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": "world"}}}})
        emit({"jsonrpc": "2.0", "id": msg_id,
              "result": {"stopReason": "end_turn"}})
    # else: ignore (notifications, unknown methods)
"#;
        let mut f = std::fs::File::create(&path).expect("create script");
        f.write_all(script.as_bytes()).expect("write");
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[test]
    fn round_trip_with_fake_agent() {
        // Skip if the test host has no python3 — the fake-agent script
        // depends on it for JSON parsing. CI machines reliably have it.
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
        {
            eprintln!("python3 not available — skipping ACP fake-agent round-trip");
            return;
        }

        let tmp = tempfile::tempdir().expect("tmpdir");
        let script = write_fake_agent_script(tmp.path());

        let mut client = AcpChannelClient::spawn(
            script.to_str().unwrap(),
            Some(tmp.path().to_path_buf()),
        )
        .expect("spawn ACP agent");
        assert!(client.is_connected());
        assert!(client.description().contains("fake_acp_agent"));

        client.send("hi there").expect("send prompt");

        // Poll for chunks. The fake agent emits two: "hello " and "world".
        let mut got = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(chunk) = client.try_recv() {
                got.push_str(&chunk);
                if got.contains("hello world") {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            got.contains("hello world"),
            "expected streamed reply 'hello world', got {got:?}"
        );
    }

    #[test]
    fn spawn_fails_with_missing_binary() {
        let err = match AcpChannelClient::spawn(
            "/no/such/binary/that/exists-please",
            None,
        ) {
            Ok(_) => panic!("expected spawn failure for missing binary"),
            Err(e) => e,
        };
        // Either the spawn itself failed, or readiness reported failure;
        // both surface the same way.
        let msg = err.to_string();
        assert!(
            msg.contains("failed to spawn") || msg.contains("No such file"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn empty_command_uses_default() {
        // We don't actually expect this to succeed (claude-agent-acp may
        // not be installed in CI), but spawn() should at least try to
        // run the default binary, not panic on empty input.
        let result = AcpChannelClient::spawn("", None);
        // If it succeeded, drop the client; if it failed, the failure
        // message should reference the default command name.
        match result {
            Ok(c) => drop(c),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains(DEFAULT_AGENT_COMMAND) || msg.contains("No such file"),
                    "expected error to mention default command, got: {msg}"
                );
            }
        }
    }
}
