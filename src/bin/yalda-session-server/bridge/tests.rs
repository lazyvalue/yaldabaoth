//! Headless unit tests for the bridge. The pure pieces (config assembly,
//! `TopicRouter`) are tested in-place; here we cover the async handlers
//! (`handle_event` topic lifecycle, `handle_inbound` injection + allowlist)
//! against a `FakeTransport` + a `FakeDriver`, with no real Manager, socket, or
//! agent. Router/reconcile logic is covered in `router.rs`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use yalda::acp_channel::PermissionMode;
use yalda::session_proto::SessionInfo;

use super::router::TopicRouter;
use super::transport::{FakeOp, FakeTransport, InboundMsg, ThreadId};
use super::*;

// ── Fakes ───────────────────────────────────────────────────────────

#[derive(Default)]
struct DriverState {
    prompts: Vec<(String, String)>,
    fail_prompt: bool,
    list: Vec<SessionInfo>,
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
    fn set_fail_prompt(&self, v: bool) {
        self.state.lock().unwrap().fail_prompt = v;
    }
}

impl SessionDriver for FakeDriver {
    async fn create(&self, label: String, cwd: PathBuf) -> SessionInfo {
        info(&format!("sid-{label}"), &label, cwd)
    }
    async fn admin_prompt(&self, sid: String, text: String) -> Result<(), String> {
        if self.state.lock().unwrap().fail_prompt {
            return Err("boom".to_string());
        }
        self.state.lock().unwrap().prompts.push((sid, text));
        Ok(())
    }
    async fn cancel(&self, _sid: String) -> Result<(), String> {
        Ok(())
    }
    async fn set_permission_mode(&self, _sid: String, _mode: PermissionMode) -> Result<(), String> {
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
        BridgeEvent::SessionCreated(info("s1", "Old", PathBuf::from("/w"))),
    )
    .await;
    let thread = router.thread_of("s1").unwrap();

    handle_event(
        &t,
        &mut router,
        BridgeEvent::SessionRenamed {
            session_id: "s1".into(),
            label: "New".into(),
        },
    )
    .await;
    handle_event(&t, &mut router, BridgeEvent::SessionClosed("s1".into())).await;

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
    handle_event(&t, &mut router, ev()).await;
    handle_event(&t, &mut router, ev()).await; // idempotent

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
