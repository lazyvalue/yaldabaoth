//! LIVE integration test for the in-app model switcher (UXI-AgentTile-16).
//!
//! Drives the REAL `AcpChannelClient` worker against the REAL `claude-agent-acp`
//! agent: waits for the model picklist the agent advertises on `session/new`,
//! issues a `set_model` (which the worker turns into `session/set_config_option`
//! for the `model` option), and asserts the agent echoes the switch back as a
//! `ModelChanged` / `ModelsAvailable` reply with the new current model. This is
//! the one piece the headless harness can't reach — the real ACP round-trip
//! (`apply_server_batch` feeds the reducer directly because `sent` can't be true
//! with no daemon, gap #2).
//!
//! Ignored by default (needs `claude-agent-acp` on PATH + working Claude auth +
//! network). Run explicitly:
//!     cargo test --test model_switch_live -- --ignored --nocapture

use std::time::{Duration, Instant};
use yalda::acp_channel::{AcpChannelClient, ReplyEvent};

#[test]
#[ignore = "live: needs claude-agent-acp on PATH + auth + network"]
fn set_model_round_trips_against_real_agent_live() {
    let client = AcpChannelClient::spawn("claude-agent-acp", Some("/tmp".into()))
        .expect("spawn claude-agent-acp");

    // Wait for session/new to complete.
    let start = Instant::now();
    while client.session_id().is_none() && start.elapsed() < Duration::from_secs(25) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(client.session_id().is_some(), "agent never opened a session");

    // Drain the initial `ModelsAvailable` the worker emits from session/new's
    // config_options. Pick a target model that ISN'T the current one so the
    // switch is observable.
    let mut current: Option<String> = None;
    let mut options: Vec<yalda::acp_channel::ModelOption> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        while let Some(ev) = client.try_recv() {
            if let ReplyEvent::ModelsAvailable { current: c, options: o } = ev {
                current = Some(c);
                options = o;
            }
        }
        if current.is_some() && !options.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let current = current.expect("agent advertised a current model");
    assert!(!options.is_empty(), "agent advertised a model picklist");
    let target = options
        .iter()
        .find(|m| m.id != current)
        .expect("at least two models to switch between")
        .id
        .clone();

    // Issue the switch and wait for the agent to echo the new current model.
    client.set_model(&target);
    let mut switched_to: Option<String> = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        while let Some(ev) = client.try_recv() {
            match ev {
                ReplyEvent::ModelChanged(m) => switched_to = Some(m),
                ReplyEvent::ModelsAvailable { current: c, .. } => switched_to = Some(c),
                _ => {}
            }
        }
        if switched_to.as_deref() == Some(target.as_str()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    assert_eq!(
        switched_to.as_deref(),
        Some(target.as_str()),
        "agent must confirm the model switch to {target} (got {switched_to:?})"
    );
}
