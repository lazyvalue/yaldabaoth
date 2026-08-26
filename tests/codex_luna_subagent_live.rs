//! LIVE integration test for UXI-AgentTile-44.
//!
//! Drives Yalda's real Codex ACP path against the installed authenticated
//! `codex-acp`. The parent must spawn one child without an explicit model. The
//! test then checks the child's durable rollout for the model settings Codex
//! actually applied. (codex-acp currently reports the parent model when it
//! reopens a spawned child, even when that child ran as Luna.) Run explicitly:
//!     cargo test --test codex_luna_subagent_live -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use yalda::acp_channel::{AcpChannelClient, AgentProvider, ReplyEvent, YaldaFrontend};

const LUNA: &str = "gpt-5.6-luna";

fn find_rollout_for_thread(root: &Path, thread_id: &str) -> Option<PathBuf> {
    let suffix = format!("{thread_id}.jsonl");
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
            {
                return Some(path);
            }
        }
    }
    None
}

fn applied_model_from_rollout(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()?
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|event| {
            (event["type"] == "event_msg"
                && event["payload"]["type"] == "thread_settings_applied")
                .then(|| {
                    event["payload"]["thread_settings"]["model"]
                        .as_str()
                        .map(str::to_owned)
                })
                .flatten()
        })
}

fn codex_child_thread_id(call: &yalda::acp_channel::ToolCall) -> Option<String> {
    call.meta
        .as_ref()
        .and_then(|meta| meta.get("codex"))
        .and_then(|value| value.get("subagent"))
        .and_then(|value| value.get("threadId"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            call.raw_input
                .as_ref()
                .and_then(|value| value.get("agentThreadId"))
                .and_then(|value| value.as_str())
        })
        .map(str::to_owned)
}

#[test]
#[ignore = "live: needs codex-acp on PATH + Codex auth + network"]
fn spawned_codex_child_advertises_luna_as_its_model() {
    let cwd = std::env::current_dir().expect("current directory");
    let mut parent = AcpChannelClient::spawn_with_resume_in_for(
        AgentProvider::Codex,
        "codex-acp",
        Some(cwd.clone()),
        None,
        YaldaFrontend::Gpui,
    )
    .expect("spawn authenticated Codex adapter");

    parent
        .send(
            "Spawn exactly one subagent for this task. Omit the model argument so the configured \
             default applies. Ask the child to reply with only LUNA-CHILD, wait for it, then reply \
             with only DONE.",
        )
        .expect("ask Codex to spawn one default-model child");

    let deadline = Instant::now() + Duration::from_secs(180);
    let mut child_thread = None;
    let mut settled = false;
    while Instant::now() < deadline && (!settled || child_thread.is_none()) {
        while let Some(event) = parent.try_recv() {
            match event {
                ReplyEvent::ToolCallStarted(call) => {
                    child_thread = child_thread.or_else(|| codex_child_thread_id(&call));
                }
                ReplyEvent::TurnSettled { .. } => settled = true,
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let child_thread = child_thread.expect("Codex reported a durable spawned-child thread id");
    assert!(settled, "parent turn did not settle after spawning its child");
    drop(parent);

    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .expect("CODEX_HOME or HOME is set");
    let rollout = find_rollout_for_thread(&codex_home.join("sessions"), &child_thread)
        .expect("find the spawned child's durable rollout");
    let applied_model = applied_model_from_rollout(&rollout);

    assert_eq!(
        applied_model.as_deref(),
        Some(LUNA),
        "a child spawned without a model override must run with Yalda's Luna default"
    );
}
