//! Headless unit tests for the bridge. The pure pieces (config assembly,
//! `TopicRouter`) are tested in-place; here we cover the async handlers
//! (`handle_event` topic lifecycle, `handle_inbound` injection + allowlist)
//! against a `FakeTransport` + a `FakeDriver`, with no real Manager, socket, or
//! agent. Router/reconcile logic is covered in `router.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yalda::acp_channel::{PermissionMode, ToolCall, ToolKind};
use yalda::agent_event::{AgentEvent, AgentEventKind, ChunkRole, TurnOutcome};
use yalda::session_proto::{Notification, SessionInfo};

use super::router::TopicRouter;
use super::transport::{FakeOp, FakeTransport, InboundMsg, ThreadId};
use super::*;

// ── Fakes ───────────────────────────────────────────────────────────

#[derive(Default)]
struct DriverState {
    prompts: Vec<(String, String)>,
    fail_prompt: bool,
    list: Vec<SessionInfo>,
    created: Vec<(String, PathBuf)>,
    modes: Vec<(String, PermissionMode)>,
    cancels: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeDriver {
    state: Arc<Mutex<DriverState>>,
}

impl FakeDriver {
    fn with_sessions(list: Vec<SessionInfo>) -> Self {
        let d = FakeDriver::default();
        d.state.lock().unwrap().list = list;
        d
    }
    fn prompts(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().prompts.clone()
    }
    fn created(&self) -> Vec<(String, PathBuf)> {
        self.state.lock().unwrap().created.clone()
    }
    fn modes(&self) -> Vec<(String, PermissionMode)> {
        self.state.lock().unwrap().modes.clone()
    }
    fn cancels(&self) -> Vec<String> {
        self.state.lock().unwrap().cancels.clone()
    }
    fn set_fail_prompt(&self, v: bool) {
        self.state.lock().unwrap().fail_prompt = v;
    }
}

impl SessionDriver for FakeDriver {
    async fn create(&self, label: String, cwd: PathBuf) -> SessionInfo {
        self.state
            .lock()
            .unwrap()
            .created
            .push((label.clone(), cwd.clone()));
        info(&format!("sid-{label}"), &label, cwd)
    }
    async fn admin_prompt(&self, sid: String, text: String) -> Result<(), String> {
        if self.state.lock().unwrap().fail_prompt {
            return Err("boom".to_string());
        }
        self.state.lock().unwrap().prompts.push((sid, text));
        Ok(())
    }
    async fn cancel(&self, sid: String) -> Result<(), String> {
        self.state.lock().unwrap().cancels.push(sid);
        Ok(())
    }
    async fn set_permission_mode(&self, sid: String, mode: PermissionMode) -> Result<(), String> {
        self.state.lock().unwrap().modes.push((sid, mode));
        Ok(())
    }
    async fn list(&self) -> Vec<SessionInfo> {
        self.state.lock().unwrap().list.clone()
    }
}

fn info(sid: &str, label: &str, cwd: PathBuf) -> SessionInfo {
    SessionInfo {
        session_id: sid.to_string(),
        acp_session_id: None,
        label: label.to_string(),
        cwd,
        turns: 0,
        connected: false,
        permission_mode: PermissionMode::ReadOnly,
        busy: false,
    }
}

fn cfg(allowed: Vec<i64>) -> BridgeConfig {
    BridgeConfig {
        token: "t".to_string(),
        chat_id: -100,
        allowed_user_ids: allowed,
        default_cwd: PathBuf::from("/tmp"),
    }
}

// ── Config assembly (pure rules, spec §7) ───────────────────────────

#[test]
fn config_disabled_without_token() {
    assert_eq!(build_config(None, Some(1), vec![1], None), Ok(None));
    assert_eq!(build_config(Some("  ".into()), Some(1), vec![1], None), Ok(None));
}

#[test]
fn config_refuses_empty_allowlist() {
    // A token with no allowlist is an open RCE surface — must be an error.
    let err = build_config(Some("tok".into()), Some(1), vec![], None).unwrap_err();
    assert!(err.contains("allowed_user_ids"), "got: {err}");
}

#[test]
fn config_requires_chat_id() {
    let err = build_config(Some("tok".into()), None, vec![7], None).unwrap_err();
    assert!(err.contains("chat_id"), "got: {err}");
}

#[test]
fn config_valid_builds() {
    let c = build_config(
        Some("tok".into()),
        Some(-100),
        vec![7, 8],
        Some(PathBuf::from("/ws")),
    )
    .unwrap()
    .expect("should be enabled");
    assert_eq!(c.token, "tok");
    assert_eq!(c.chat_id, -100);
    assert_eq!(c.allowed_user_ids, vec![7, 8]);
    assert_eq!(c.default_cwd, PathBuf::from("/ws"));
}

// ── Topic lifecycle (handle_event) ──────────────────────────────────

#[tokio::test]
async fn created_event_opens_and_binds_a_topic() {
    let t = FakeTransport::new();
    let mut router = TopicRouter::new();

    handle_event(
        &t,
        &mut router,
        &mut HashMap::new(),
        &mut HashMap::new(),
        BridgeEvent::SessionCreated(info("s1", "Build", PathBuf::from("/w"))),
    )
    .await;

    // A topic was opened named after the label, and the session is now bound.
    let ops = t.ops();
    assert!(matches!(&ops[..], [FakeOp::Open { name, .. }] if name == "Build"), "ops: {ops:?}");
    let thread = router.thread_of("s1").expect("bound");
    // The bound thread matches the one the fake minted.
    assert_eq!(ThreadId(1), thread);
}

#[tokio::test]
async fn rename_then_close_target_the_bound_topic() {
    let t = FakeTransport::new();
    let mut router = TopicRouter::new();
    handle_event(
        &t,
        &mut router,
        &mut HashMap::new(),
        &mut HashMap::new(),
        BridgeEvent::SessionCreated(info("s1", "Old", PathBuf::from("/w"))),
    )
    .await;
    let thread = router.thread_of("s1").unwrap();

    handle_event(
        &t,
        &mut router,
        &mut HashMap::new(),
        &mut HashMap::new(),
        BridgeEvent::SessionRenamed {
            session_id: "s1".into(),
            label: "New".into(),
        },
    )
    .await;
    handle_event(
        &t,
        &mut router,
        &mut HashMap::new(),
        &mut HashMap::new(),
        BridgeEvent::SessionClosed("s1".into()),
    )
    .await;

    let ops = t.ops();
    assert!(
        ops.contains(&FakeOp::Rename {
            thread,
            name: "New".to_string()
        }),
        "ops: {ops:?}"
    );
    assert!(ops.contains(&FakeOp::Close(thread)), "ops: {ops:?}");
    // Closing unbinds, so the session no longer maps to a topic.
    assert!(!router.is_bound("s1"));
}

#[tokio::test]
async fn duplicate_create_does_not_open_a_second_topic() {
    let t = FakeTransport::new();
    let mut router = TopicRouter::new();
    let ev = || BridgeEvent::SessionCreated(info("s1", "X", PathBuf::from("/w")));
    handle_event(&t, &mut router, &mut HashMap::new(), &mut HashMap::new(), ev()).await;
    handle_event(&t, &mut router, &mut HashMap::new(), &mut HashMap::new(), ev()).await; // idempotent

    let opens = t.ops().iter().filter(|o| matches!(o, FakeOp::Open { .. })).count();
    assert_eq!(opens, 1, "second create must not open another topic");
}

// ── Inbound injection + allowlist (handle_inbound) ──────────────────

#[tokio::test]
async fn message_in_session_topic_drives_a_prompt() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "  hello agent  ".into(),
        },
    )
    .await;

    // Routed to the bound session, trimmed.
    assert_eq!(d.prompts(), vec![("s1".to_string(), "hello agent".to_string())]);
}

#[tokio::test]
async fn non_allowlisted_sender_is_dropped_silently() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]), // 999 is NOT allowed
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 999,
            text: "rm -rf /".into(),
        },
    )
    .await;

    // No prompt driven, and no reply sent (don't confirm the bot exists).
    assert!(d.prompts().is_empty(), "stranger must not drive a turn");
    assert!(t.ops().is_empty(), "stranger must get no reply");
}

#[tokio::test]
async fn message_in_general_topic_nudges_instead_of_prompting() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let router = TopicRouter::new(); // nothing bound; General maps to no session

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId::GENERAL,
            from_user: 42,
            text: "hi".into(),
        },
    )
    .await;

    assert!(d.prompts().is_empty());
    // A nudge was sent to the General topic.
    assert!(
        matches!(&t.ops()[..], [FakeOp::Send { thread, .. }] if *thread == ThreadId::GENERAL),
        "ops: {:?}",
        t.ops()
    );
}

#[tokio::test]
async fn failed_prompt_surfaces_a_warning_to_the_topic() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    d.set_fail_prompt(true);
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "do it".into(),
        },
    )
    .await;

    assert!(
        matches!(&t.ops()[..], [FakeOp::Send { thread, text, .. }]
            if *thread == ThreadId(5) && text.contains("couldn't send")),
        "ops: {:?}",
        t.ops()
    );
}

// ── Command dispatch (handle_inbound parse + locale, T-005) ─────────

#[tokio::test]
async fn new_in_general_creates_a_read_only_session() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let router = TopicRouter::new(); // General maps to no session

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId::GENERAL,
            from_user: 42,
            text: "/new Ship it".into(),
        },
    )
    .await;

    // The session was created with the typed label + default cwd…
    assert_eq!(
        d.created(),
        vec![("Ship it".to_string(), PathBuf::from("/tmp"))],
        "create must be driven with label + config default_cwd"
    );
    // …and immediately set read-only (§7 fail-safe). The fake mints "sid-<label>".
    assert_eq!(
        d.modes(),
        vec![("sid-Ship it".to_string(), PermissionMode::ReadOnly)],
        "new sessions must default to read-only"
    );
    // We do NOT open a topic here — that rides the SessionCreated broadcast.
    assert!(
        !t.ops().iter().any(|o| matches!(o, FakeOp::Open { .. })),
        "/new must not open a topic itself: {:?}",
        t.ops()
    );
}

#[tokio::test]
async fn new_with_trailing_cwd_uses_that_path() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let router = TopicRouter::new();

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId::GENERAL,
            from_user: 42,
            text: "/new fix bug /srv/app".into(),
        },
    )
    .await;

    assert_eq!(
        d.created(),
        vec![("fix bug".to_string(), PathBuf::from("/srv/app"))]
    );
}

#[tokio::test]
async fn sessions_in_general_lists_the_roster() {
    let t = FakeTransport::new();
    let d = FakeDriver::with_sessions(vec![info("a", "Alpha", PathBuf::from("/w"))]);
    let mut router = TopicRouter::new();
    router.bind("a".to_string(), ThreadId(7));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId::GENERAL,
            from_user: 42,
            text: "/sessions".into(),
        },
    )
    .await;

    match &t.ops()[..] {
        [FakeOp::Send { thread, text, .. }] => {
            assert_eq!(*thread, ThreadId::GENERAL);
            assert!(text.contains("Alpha"), "roster lists the label: {text:?}");
            assert!(text.contains("topic 7"), "roster shows the topic: {text:?}");
        }
        other => panic!("expected one Send with the roster, got {other:?}"),
    }
}

#[tokio::test]
async fn stop_in_bound_topic_cancels_that_session() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "/stop".into(),
        },
    )
    .await;

    assert_eq!(d.cancels(), vec!["s1".to_string()], "must cancel the topic's session");
    assert!(d.prompts().is_empty(), "/stop must not inject a prompt");
}

#[tokio::test]
async fn mode_yolo_in_bound_topic_sets_permission_mode() {
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "/mode yolo".into(),
        },
    )
    .await;

    assert_eq!(
        d.modes(),
        vec![("s1".to_string(), PermissionMode::Yolo)],
        "must set the topic's session to Yolo"
    );
}

#[tokio::test]
async fn status_in_bound_topic_reports_the_session() {
    let t = FakeTransport::new();
    let d = FakeDriver::with_sessions(vec![info("s1", "Alpha", PathBuf::from("/w"))]);
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "/status".into(),
        },
    )
    .await;

    match &t.ops()[..] {
        [FakeOp::Send { thread, text, .. }] => {
            assert_eq!(*thread, ThreadId(5));
            assert!(text.contains("Alpha"), "status names the session: {text:?}");
            assert!(text.contains("read-only"), "status shows the mode: {text:?}");
        }
        other => panic!("expected one status Send, got {other:?}"),
    }
}

#[tokio::test]
async fn plain_message_in_topic_still_injects() {
    // Regression of T-003: a non-slash message is still an injected prompt.
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]),
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 42,
            text: "keep going".into(),
        },
    )
    .await;

    assert_eq!(d.prompts(), vec![("s1".to_string(), "keep going".to_string())]);
}

#[tokio::test]
async fn allowlist_gate_runs_before_command_dispatch() {
    // A stranger's /stop must be dropped BEFORE parse/dispatch — no cancel, no
    // op. Guards the ordering of the §7 gate relative to command handling.
    let t = FakeTransport::new();
    let d = FakeDriver::default();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(5));

    handle_inbound(
        &cfg(vec![42]), // 999 is NOT allowed
        &t,
        &d,
        &router,
        InboundMsg {
            thread: ThreadId(5),
            from_user: 999,
            text: "/stop".into(),
        },
    )
    .await;

    assert!(d.cancels().is_empty(), "stranger's /stop must not cancel");
    assert!(t.ops().is_empty(), "stranger must get no reply");
}

// ── Startup reconcile drives topic ops through the transport ────────

#[tokio::test]
async fn run_bridge_reconciles_topics_on_startup() {
    // Two live sessions, no persisted topics ⇒ both get a topic opened.
    let t = FakeTransport::new();
    let d = FakeDriver::with_sessions(vec![
        info("a", "Alpha", PathBuf::from("/w")),
        info("b", "Beta", PathBuf::from("/w")),
    ]);
    let (event_tx, event_rx) = mpsc::unbounded_channel();

    // Run the bridge briefly: it reconciles, then blocks on events. Drop the
    // sender to make it exit cleanly after the startup pass.
    let handle = tokio::spawn(run_bridge(cfg(vec![1]), t.clone(), d, event_rx));
    // Give the startup reconcile a chance to run, then close the event channel.
    tokio::task::yield_now().await;
    drop(event_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;

    let opened: Vec<String> = t
        .ops()
        .into_iter()
        .filter_map(|o| match o {
            FakeOp::Open { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert!(opened.contains(&"Alpha".to_string()), "opened: {opened:?}");
    assert!(opened.contains(&"Beta".to_string()), "opened: {opened:?}");
}

// ── Outbound event fold (handle_event Transcript arm, T-004) ─────────

/// Wrap an `AgentEventKind` as a Transcript BridgeEvent for session `sid`.
fn transcript(sid: &str, kind: AgentEventKind) -> BridgeEvent {
    BridgeEvent::Transcript {
        session_id: sid.to_string(),
        note: Box::new(Notification::Agent {
            event: AgentEvent::new(sid.to_string(), 0, 0, 0, kind),
        }),
    }
}

fn msg_chunk(text: &str) -> AgentEventKind {
    AgentEventKind::Chunk {
        text: text.to_string(),
        role: ChunkRole::Message,
    }
}

/// The end-to-end fold through the REAL `handle_event` Transcript arm: a bound
/// topic receives a Send (running message), then Edit(s) coalescing prose + the
/// tool line, then no further edit after the turn boundary (Finalize clears the
/// running id, so the next turn Posts fresh).
#[tokio::test]
async fn transcript_agent_events_fold_into_the_bound_topic() {
    let t = FakeTransport::new();
    let mut router = TopicRouter::new();
    router.bind("s1".to_string(), ThreadId(9));
    let mut folders = HashMap::new();
    let mut running = HashMap::new();

    handle_event(&t, &mut router, &mut folders, &mut running, transcript("s1", msg_chunk("Hello "))).await;
    handle_event(&t, &mut router, &mut folders, &mut running, transcript("s1", msg_chunk("world"))).await;
    let tc: ToolCall = {
        let mut tc = ToolCall::new("t1", "Read File");
        tc.kind = ToolKind::Read;
        tc
    };
    handle_event(
        &t,
        &mut router,
        &mut folders,
        &mut running,
        transcript("s1", AgentEventKind::ToolCallStarted(tc)),
    )
    .await;
    handle_event(
        &t,
        &mut router,
        &mut folders,
        &mut running,
        transcript("s1", AgentEventKind::TurnEnded { outcome: TurnOutcome::Completed }),
    )
    .await;

    let ops = t.ops();
    // First op is a Send on the bound thread (the running message).
    let (send_thread, send_msg) = match ops.first() {
        Some(FakeOp::Send { thread, message, .. }) => (*thread, *message),
        other => panic!("expected first op Send, got {other:?}; ops: {ops:?}"),
    };
    assert_eq!(send_thread, ThreadId(9), "posted to the bound topic");
    // At least one Edit followed, on the SAME thread + message.
    assert!(
        ops[1..].iter().any(|o| matches!(
            o,
            FakeOp::Edit { thread, message, .. } if *thread == ThreadId(9) && *message == send_msg
        )),
        "expected an Edit on the running message; ops: {ops:?}"
    );
    // Every op targets the bound thread (nothing leaks elsewhere).
    assert!(
        ops.iter().all(|o| matches!(
            o,
            FakeOp::Send { thread, .. } | FakeOp::Edit { thread, .. } if *thread == ThreadId(9)
        )),
        "all ops on the bound thread; ops: {ops:?}"
    );
    // The final rendered text carries BOTH the prose and the tool line.
    let last_text = ops
        .iter()
        .rev()
        .find_map(|o| match o {
            FakeOp::Edit { text, .. } | FakeOp::Send { text, .. } => Some(text.clone()),
            _ => None,
        })
        .expect("some rendered text");
    assert!(last_text.contains("Hello world"), "prose present: {last_text:?}");
    assert!(last_text.contains("🔧"), "tool line present: {last_text:?}");

    // Finalize cleared the running message: a NEW turn Posts fresh (not Edit).
    let before = t.ops().len();
    handle_event(
        &t,
        &mut router,
        &mut folders,
        &mut running,
        transcript("s1", msg_chunk("next turn")),
    )
    .await;
    let after = t.ops();
    assert!(
        matches!(&after[before], FakeOp::Send { thread, .. } if *thread == ThreadId(9)),
        "post-turn message must be a fresh Send, got {:?}",
        &after[before..]
    );
}

/// A transcript event for an UNBOUND session (no topic) folds to nothing — no
/// transport op at all.
#[tokio::test]
async fn transcript_for_unbound_session_is_ignored() {
    let t = FakeTransport::new();
    let mut router = TopicRouter::new(); // nothing bound
    let mut folders = HashMap::new();
    let mut running = HashMap::new();

    handle_event(
        &t,
        &mut router,
        &mut folders,
        &mut running,
        transcript("ghost", msg_chunk("hi")),
    )
    .await;

    assert!(t.ops().is_empty(), "unbound session must drive no op: {:?}", t.ops());
}
