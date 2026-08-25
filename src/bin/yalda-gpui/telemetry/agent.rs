//! Honest live agent metrics derived from the universal roster and any session
//! state this GUI has loaded.
//!
//! The roster is the fleet authority: a session remains present even when this
//! GUI has never attached to it. Loaded state only enriches that row with facts
//! the roster does not carry (model, tools, usage, and the local turn clock).
//! Optional fields deliberately stay optional so an unloaded session is never
//! presented as having zero tools, zero context occupancy, or zero cost.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use super::super::*;
use yalda::acp_channel::{AgentProvider, ToolCallStatus};

/// Coarse live state available for every roster row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentMetricState {
    Working,
    Ready,
    Archived,
    Unavailable,
}

impl AgentMetricState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Ready => "Ready",
            Self::Archived => "Archived",
            Self::Unavailable => "Unavailable",
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Working | Self::Ready)
    }
}

/// Context-window occupancy, not historical or consumed-token usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ContextOccupancy {
    pub(crate) used: u64,
    pub(crate) capacity: u64,
}

impl ContextOccupancy {
    pub(crate) fn percent(self) -> Option<f64> {
        (self.capacity > 0).then(|| self.used as f64 * 100.0 / self.capacity as f64)
    }
}

/// One arithmetic mean with the population that actually supplied the fact.
/// `population` is the size of the cohort being summarized; `denominator` is
/// the number of known values. When the denominator is zero both `sum` and
/// `mean` are unavailable.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct MetricAverage {
    pub(crate) sum: Option<f64>,
    pub(crate) mean: Option<f64>,
    pub(crate) denominator: usize,
    pub(crate) population: usize,
}

impl MetricAverage {
    fn from_known(values: impl IntoIterator<Item = Option<f64>>, population: usize) -> Self {
        let (sum, denominator) = values
            .into_iter()
            .flatten()
            .fold((0.0, 0usize), |(sum, count), value| {
                (sum + value, count + 1)
            });
        if denominator == 0 {
            Self {
                sum: None,
                mean: None,
                denominator,
                population,
            }
        } else {
            Self {
                sum: Some(sum),
                mean: Some(sum / denominator as f64),
                denominator,
                population,
            }
        }
    }
}

/// A row shown on the Agents page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AgentMetricSnapshot {
    /// Stable within the live snapshot. Server-backed rows use their server sid;
    /// direct/local sessions use a namespaced local id.
    pub(crate) row_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) label: String,
    pub(crate) provider: Option<AgentProvider>,
    pub(crate) model: Option<String>,
    pub(crate) state: AgentMetricState,
    pub(crate) settled_turns: Option<usize>,
    pub(crate) tool_total: Option<usize>,
    pub(crate) tool_failures: Option<usize>,
    pub(crate) context: Option<ContextOccupancy>,
    /// Cumulative provider-reported cost when available.
    pub(crate) cost_usd: Option<f64>,
    pub(crate) current_turn_elapsed: Option<Duration>,
    /// Whether this GUI currently has the conversation state loaded.
    pub(crate) loaded: bool,
}

/// Active-fleet means. Archived and unavailable rows remain in the snapshot for
/// inspection, but do not dilute the population describing agents that can do
/// work now. Each metric has its own explicit denominator because the roster
/// and loaded-session sources have different coverage.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FleetMetricAverages {
    pub(crate) settled_turns: MetricAverage,
    pub(crate) tool_total: MetricAverage,
    pub(crate) tool_failures: MetricAverage,
    pub(crate) context_percent: MetricAverage,
    pub(crate) cost_usd: MetricAverage,
    pub(crate) current_turn_elapsed_secs: MetricAverage,
}

/// Complete Agents-page snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FleetMetricSnapshot {
    pub(crate) agents: Vec<AgentMetricSnapshot>,
    pub(crate) working: usize,
    pub(crate) ready: usize,
    /// Additive within telemetry store v1 so documents written before archived
    /// became a distinct state continue to deserialize.
    #[serde(default)]
    pub(crate) archived: usize,
    pub(crate) unavailable: usize,
    pub(crate) averages: FleetMetricAverages,
}

/// Pure roster input used by the aggregation boundary and unit tests.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RosterAgentMetricFacts {
    pub(crate) session_id: String,
    pub(crate) label: String,
    pub(crate) provider: Option<AgentProvider>,
    pub(crate) connected: bool,
    pub(crate) busy: bool,
    pub(crate) archived: bool,
    pub(crate) settled_turns: Option<usize>,
}

impl From<&yalda::session_proto::SessionInfo> for RosterAgentMetricFacts {
    fn from(info: &yalda::session_proto::SessionInfo) -> Self {
        Self {
            session_id: info.session_id.clone(),
            label: info.label.clone(),
            provider: Some(info.provider),
            connected: info.connected,
            busy: info.busy,
            archived: info.archived,
            settled_turns: Some(info.turns),
        }
    }
}

impl RosterAgentMetricFacts {
    fn state(&self) -> AgentMetricState {
        if self.archived {
            AgentMetricState::Archived
        } else if !self.connected {
            AgentMetricState::Unavailable
        } else if self.busy {
            AgentMetricState::Working
        } else {
            AgentMetricState::Ready
        }
    }

    fn into_snapshot(self) -> AgentMetricSnapshot {
        let state = self.state();
        AgentMetricSnapshot {
            row_id: self.session_id.clone(),
            session_id: Some(self.session_id),
            label: self.label,
            provider: self.provider,
            model: None,
            state,
            settled_turns: self.settled_turns,
            tool_total: None,
            tool_failures: None,
            context: None,
            cost_usd: None,
            current_turn_elapsed: None,
            loaded: false,
        }
    }
}

/// Pure enrichment input for one locally loaded conversation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedAgentMetricFacts {
    pub(crate) local_id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) label: String,
    pub(crate) provider: Option<AgentProvider>,
    pub(crate) model: Option<String>,
    pub(crate) state: AgentMetricState,
    pub(crate) settled_turns: Option<usize>,
    pub(crate) tool_total: usize,
    pub(crate) tool_failures: usize,
    pub(crate) context: Option<ContextOccupancy>,
    pub(crate) cost_usd: Option<f64>,
    pub(crate) current_turn_elapsed: Option<Duration>,
}

impl LoadedAgentMetricFacts {
    fn into_snapshot(self) -> AgentMetricSnapshot {
        AgentMetricSnapshot {
            row_id: self
                .session_id
                .clone()
                .unwrap_or_else(|| self.local_id.clone()),
            session_id: self.session_id,
            label: self.label,
            provider: self.provider,
            model: self.model,
            state: self.state,
            settled_turns: self.settled_turns,
            tool_total: Some(self.tool_total),
            tool_failures: Some(self.tool_failures),
            context: self.context,
            cost_usd: self.cost_usd,
            current_turn_elapsed: self.current_turn_elapsed,
            loaded: true,
        }
    }
}

/// Merge the universal roster with locally loaded facts, calculate state totals,
/// and summarize the active cohort. This function has no GPUI or clock
/// dependency.
pub(crate) fn aggregate_agent_metrics(
    roster: impl IntoIterator<Item = RosterAgentMetricFacts>,
    loaded: impl IntoIterator<Item = LoadedAgentMetricFacts>,
) -> FleetMetricSnapshot {
    let mut rows: BTreeMap<String, AgentMetricSnapshot> = roster
        .into_iter()
        .map(|facts| {
            let row = facts.into_snapshot();
            (row.row_id.clone(), row)
        })
        .collect();

    for facts in loaded {
        let matched_sid = facts
            .session_id
            .as_ref()
            .filter(|sid| rows.contains_key(*sid))
            .cloned();
        if let Some(sid) = matched_sid {
            let row = rows.get_mut(&sid).expect("matched roster row exists");
            row.loaded = true;
            row.model = facts.model;
            row.tool_total = Some(facts.tool_total);
            row.tool_failures = Some(facts.tool_failures);
            row.context = facts.context;
            row.cost_usd = facts.cost_usd;
            row.current_turn_elapsed = facts.current_turn_elapsed;
            row.provider = row.provider.or(facts.provider);
            row.settled_turns = row.settled_turns.or(facts.settled_turns);
            // The server roster owns availability. The local phase may lead a
            // not-yet-delivered busy broadcast, but it must never resurrect a
            // disconnected or archived server session as Working.
            if row.state == AgentMetricState::Ready && facts.state == AgentMetricState::Working {
                row.state = AgentMetricState::Working;
            }
        } else {
            let row = facts.into_snapshot();
            rows.insert(row.row_id.clone(), row);
        }
    }

    let mut agents: Vec<_> = rows.into_values().collect();
    agents.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.row_id.cmp(&b.row_id)));

    let working = agents
        .iter()
        .filter(|agent| agent.state == AgentMetricState::Working)
        .count();
    let ready = agents
        .iter()
        .filter(|agent| agent.state == AgentMetricState::Ready)
        .count();
    let archived = agents
        .iter()
        .filter(|agent| agent.state == AgentMetricState::Archived)
        .count();
    let unavailable = agents
        .iter()
        .filter(|agent| agent.state == AgentMetricState::Unavailable)
        .count();
    let active_agents = agents
        .iter()
        .filter(|agent| agent.state.is_active())
        .collect::<Vec<_>>();
    let population = active_agents.len();
    let averages = FleetMetricAverages {
        settled_turns: MetricAverage::from_known(
            active_agents
                .iter()
                .map(|agent| agent.settled_turns.map(|value| value as f64)),
            population,
        ),
        tool_total: MetricAverage::from_known(
            active_agents
                .iter()
                .map(|agent| agent.tool_total.map(|value| value as f64)),
            population,
        ),
        tool_failures: MetricAverage::from_known(
            active_agents
                .iter()
                .map(|agent| agent.tool_failures.map(|value| value as f64)),
            population,
        ),
        context_percent: MetricAverage::from_known(
            active_agents
                .iter()
                .map(|agent| agent.context.and_then(ContextOccupancy::percent)),
            population,
        ),
        cost_usd: MetricAverage::from_known(
            active_agents.iter().map(|agent| agent.cost_usd),
            population,
        ),
        current_turn_elapsed_secs: MetricAverage::from_known(
            active_agents.iter().map(|agent| {
                agent
                    .current_turn_elapsed
                    .map(|duration| duration.as_secs_f64())
            }),
            population,
        ),
    };

    FleetMetricSnapshot {
        agents,
        working,
        ready,
        archived,
        unavailable,
        averages,
    }
}

/// Production adapter: snapshot the universal roster and enrich it from the
/// `AgentSession` entities currently loaded in this GUI.
pub(crate) fn collect_agent_metrics(
    roster: &AgentRoster,
    sessions: &AgentSessions,
    now: Instant,
    cx: &GpuiApp,
) -> FleetMetricSnapshot {
    let roster_facts = roster
        .entries_by_label()
        .into_iter()
        .map(RosterAgentMetricFacts::from)
        .collect::<Vec<_>>();

    let roster_state: HashMap<&str, AgentMetricState> = roster_facts
        .iter()
        .map(|facts| (facts.session_id.as_str(), facts.state()))
        .collect();

    let loaded_facts = sessions
        .iter()
        .map(|(id, entity)| {
            let sid = sessions.sid_of(id).map(|sid| sid.as_str().to_string());
            let session = entity.read(cx);
            let local_state = if session.turn_phase.is_awaiting() {
                AgentMetricState::Working
            } else if let Some(state) = sid
                .as_deref()
                .and_then(|server_sid| roster_state.get(server_sid))
                .copied()
            {
                state
            } else if session
                .channel
                .as_ref()
                .is_some_and(|channel| channel.is_connected())
            {
                AgentMetricState::Ready
            } else {
                AgentMetricState::Unavailable
            };
            let turn_started = session.turn_phase.turn_started();
            let tool_failures = session
                .tools
                .calls
                .values()
                .filter(|call| call.status == ToolCallStatus::Failed)
                .count();
            let context = session.usage.as_ref().map(|usage| ContextOccupancy {
                used: usage.tokens_used,
                capacity: usage.tokens_total,
            });

            LoadedAgentMetricFacts {
                local_id: format!("local:{}", id.0),
                session_id: sid,
                label: session.label.clone(),
                provider: Some(session.provider),
                model: session.agent_model.clone(),
                state: local_state,
                settled_turns: Some(session.replay_turns.last_seen),
                tool_total: session.tools.calls.len(),
                tool_failures,
                context,
                cost_usd: session.usage.as_ref().and_then(|usage| usage.cost_usd),
                current_turn_elapsed: turn_started
                    .and_then(|started| now.checked_duration_since(started)),
            }
        })
        .collect::<Vec<_>>();

    aggregate_agent_metrics(roster_facts, loaded_facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(
        sid: &str,
        label: &str,
        connected: bool,
        busy: bool,
        turns: usize,
    ) -> RosterAgentMetricFacts {
        RosterAgentMetricFacts {
            session_id: sid.into(),
            label: label.into(),
            provider: Some(AgentProvider::Claude),
            connected,
            busy,
            archived: false,
            settled_turns: Some(turns),
        }
    }

    fn loaded(sid: Option<&str>, label: &str) -> LoadedAgentMetricFacts {
        LoadedAgentMetricFacts {
            local_id: format!("local:{label}"),
            session_id: sid.map(str::to_string),
            label: label.into(),
            provider: Some(AgentProvider::Codex),
            model: Some("gpt-test".into()),
            state: AgentMetricState::Ready,
            settled_turns: Some(4),
            tool_total: 6,
            tool_failures: 2,
            context: Some(ContextOccupancy {
                used: 25,
                capacity: 100,
            }),
            cost_usd: Some(1.5),
            current_turn_elapsed: None,
        }
    }

    #[test]
    fn roster_only_sessions_remain_visible_with_unloaded_facts_unknown() {
        let snapshot = aggregate_agent_metrics(
            [
                roster("ready", "Ready", true, false, 3),
                roster("working", "Working", true, true, 5),
                roster("offline", "Offline", false, false, 8),
                {
                    let mut archived = roster("archived", "Archived", false, false, 13);
                    archived.archived = true;
                    archived
                },
            ],
            [],
        );

        assert_eq!(snapshot.agents.len(), 4);
        assert_eq!(
            (
                snapshot.working,
                snapshot.ready,
                snapshot.archived,
                snapshot.unavailable,
            ),
            (1, 1, 1, 1)
        );
        let ready = snapshot
            .agents
            .iter()
            .find(|agent| agent.session_id.as_deref() == Some("ready"))
            .unwrap();
        assert!(!ready.loaded);
        assert_eq!(ready.settled_turns, Some(3), "roster turns are known");
        assert_eq!(ready.tool_total, None, "unloaded tools are unavailable");
        assert_eq!(ready.tool_failures, None);
        assert_eq!(ready.context, None);
        assert_eq!(ready.cost_usd, None);
        assert_eq!(ready.model, None);
    }

    #[test]
    fn loaded_facts_enrich_matching_rows_and_local_only_rows_join_the_fleet() {
        let mut matching = loaded(Some("server"), "Loaded label");
        matching.state = AgentMetricState::Working;
        matching.current_turn_elapsed = Some(Duration::from_secs(12));
        let local_only = loaded(None, "Local only");

        let snapshot = aggregate_agent_metrics(
            [roster("server", "Roster label", true, false, 9)],
            [matching, local_only],
        );

        assert_eq!(snapshot.agents.len(), 2);
        let server = snapshot
            .agents
            .iter()
            .find(|agent| agent.session_id.as_deref() == Some("server"))
            .unwrap();
        assert_eq!(server.label, "Roster label", "roster owns fleet identity");
        assert_eq!(server.provider, Some(AgentProvider::Claude));
        assert_eq!(server.model.as_deref(), Some("gpt-test"));
        assert_eq!(server.state, AgentMetricState::Working);
        assert_eq!(
            server.settled_turns,
            Some(9),
            "roster turns remain authoritative"
        );
        assert_eq!(server.tool_total, Some(6));
        assert_eq!(server.tool_failures, Some(2));
        assert_eq!(server.current_turn_elapsed, Some(Duration::from_secs(12)));
        assert!(
            snapshot
                .agents
                .iter()
                .any(|agent| agent.row_id == "local:Local only")
        );
    }

    #[test]
    fn every_average_exposes_its_actual_denominator() {
        let mut partial = loaded(Some("loaded"), "Loaded");
        partial.current_turn_elapsed = Some(Duration::from_secs(10));
        let snapshot = aggregate_agent_metrics(
            [
                roster("loaded", "Loaded", true, true, 4),
                roster("roster-only", "Roster only", true, false, 2),
                roster("offline", "Offline", false, false, 100),
                {
                    let mut archived = roster("archived", "Archived", false, false, 200);
                    archived.archived = true;
                    archived
                },
            ],
            [partial],
        );

        assert_eq!(snapshot.averages.settled_turns.population, 2);
        assert_eq!(snapshot.averages.settled_turns.denominator, 2);
        assert_eq!(snapshot.averages.settled_turns.mean, Some(3.0));
        assert_eq!(snapshot.averages.tool_total.denominator, 1);
        assert_eq!(snapshot.averages.tool_total.mean, Some(6.0));
        assert_eq!(snapshot.averages.tool_failures.denominator, 1);
        assert_eq!(snapshot.averages.context_percent.denominator, 1);
        assert_eq!(snapshot.averages.context_percent.mean, Some(25.0));
        assert_eq!(snapshot.averages.cost_usd.denominator, 1);
        assert_eq!(snapshot.averages.current_turn_elapsed_secs.denominator, 1);
        assert_eq!(snapshot.averages.current_turn_elapsed_secs.mean, Some(10.0));
    }

    #[test]
    fn absent_values_do_not_turn_into_zero_averages() {
        let snapshot = aggregate_agent_metrics([roster("only", "Only", true, false, 0)], []);

        assert_eq!(snapshot.averages.tool_total.sum, None);
        assert_eq!(snapshot.averages.tool_total.mean, None);
        assert_eq!(snapshot.averages.tool_total.denominator, 0);
        assert_eq!(snapshot.averages.tool_total.population, 1);
        assert_eq!(snapshot.averages.context_percent.mean, None);
        assert_eq!(snapshot.averages.cost_usd.mean, None);
        assert_eq!(snapshot.averages.current_turn_elapsed_secs.mean, None);
    }

    #[test]
    fn disconnected_and_archived_are_distinct_and_cannot_be_resurrected_locally() {
        let mut archived = roster("archived", "Archived", true, true, 1);
        archived.archived = true;
        let disconnected = roster("disconnected", "Disconnected", false, true, 2);
        let mut archived_local = loaded(Some("archived"), "Archived");
        archived_local.state = AgentMetricState::Working;
        let mut disconnected_local = loaded(Some("disconnected"), "Disconnected");
        disconnected_local.state = AgentMetricState::Working;

        let snapshot = aggregate_agent_metrics(
            [archived, disconnected],
            [archived_local, disconnected_local],
        );
        assert_eq!(
            snapshot
                .agents
                .iter()
                .find(|agent| agent.row_id == "archived")
                .unwrap()
                .state,
            AgentMetricState::Archived
        );
        assert_eq!(
            snapshot
                .agents
                .iter()
                .find(|agent| agent.row_id == "disconnected")
                .unwrap()
                .state,
            AgentMetricState::Unavailable
        );
        assert_eq!(snapshot.archived, 1);
        assert_eq!(snapshot.unavailable, 1);
        assert_eq!(snapshot.working, 0);
        assert_eq!(snapshot.averages.settled_turns.population, 0);
        assert_eq!(snapshot.averages.settled_turns.denominator, 0);
    }
}
