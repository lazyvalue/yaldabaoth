//! Cog graph explorer: a subprocess client over the local `cog` CLI plus the
//! `App::Cog` tile/state model.
//!
//! `App::Cog` is a read-only viewport onto a Cog planning graph. It shells out
//! to the `cog` binary on `PATH` (which itself talks to `cogd` over HTTP,
//! honoring `COG_ADDR`) and renders the returned JSON. The tile has two panes:
//! a left selector (first a graph explorer, then the chosen graph's node list)
//! and a right detail pane (the selected node's content, output, status,
//! status-transition timeline, and notes). See `cog_view.rs` for the body and
//! `cog_ui.rs` for the open/load/select/key methods.
//!
//! All `cog` calls are blocking and run on the background executor (see
//! `cog_ui.rs` — `cx.background_executor().spawn`), never on the paint thread.

use super::*;

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

// ── Data model (mirrors the `cog` CLI JSON shapes) ───────────────────────────

/// One graph, as returned by `cog graph list` / `cog graph get`.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogGraph {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) omega: String,
    #[serde(default)]
    pub(crate) sealed: bool,
    #[serde(default)]
    pub(crate) prototype: bool,
}

impl CogGraph {
    /// A human label for the left list — the name, falling back to the id.
    pub(crate) fn label(&self) -> String {
        if self.name.trim().is_empty() {
            self.id.clone()
        } else {
            self.name.clone()
        }
    }
}

/// A graph's derived state (`cog graph status`).
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct CogGraphStatus {
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) islands: serde_json::Value,
}

impl CogGraphStatus {
    /// Does the graph have islands (cycles / disconnected nodes)? `"none"`, an
    /// empty array, or null → no.
    pub(crate) fn has_islands(&self) -> bool {
        match &self.islands {
            serde_json::Value::Null => false,
            serde_json::Value::String(s) => !s.is_empty() && s != "none",
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => true,
        }
    }
}

/// One node (`cog graph nodes`). `content` and `output` are free-form JSON.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogNode {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) content: serde_json::Value,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: Option<serde_json::Value>,
}

/// A dependency edge: `from` must be `done` before `to` becomes ready.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogEdge {
    pub(crate) from: String,
    pub(crate) to: String,
}

/// One entry in a node's log (`cog node log`). Status transitions carry
/// `kind == "status_changed"` with `data.to`; notes carry `kind == "note"`.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogLogEntry {
    #[serde(default)]
    pub(crate) seq: i64,
    #[serde(default)]
    pub(crate) at: i64,
    #[serde(default)]
    pub(crate) actor: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) data: serde_json::Value,
}

/// A node's notes, grouped by node (`cog graph read-node-notes`).
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogNodeNotes {
    pub(crate) node: String,
    #[serde(default)]
    pub(crate) notes: Vec<CogNote>,
}

/// One note on a node.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CogNote {
    #[serde(default)]
    pub(crate) at: i64,
    #[serde(default)]
    pub(crate) actor: String,
    #[serde(default)]
    pub(crate) topic: Option<String>,
    #[serde(default)]
    pub(crate) data: serde_json::Value,
}

/// A live hierarchical Topic binding (`cog topic list`). Cog calls its
/// topic-addressable note object a Bulletin; the UI presents that kind as Note.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct CogTopicBinding {
    pub(crate) address: String,
    pub(crate) kind: CogTopicKind,
    pub(crate) object: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) created_at: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CogTopicKind {
    Graph,
    Bulletin,
    Chat,
}

impl CogTopicKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Bulletin => "note",
            Self::Chat => "chat",
        }
    }
}

/// One node in the topic file-explorer tree. Folders precede bindings and both
/// are sorted case-insensitively, so a server response order never leaks into
/// the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CogTopicNode {
    Folder {
        label: String,
        path: String,
        children: Vec<CogTopicNode>,
    },
    Binding(CogTopicBinding),
}

#[derive(Default)]
struct TopicFolderBuilder {
    folders: BTreeMap<String, TopicFolderBuilder>,
    bindings: Vec<CogTopicBinding>,
}

impl TopicFolderBuilder {
    fn finish(self, parent: &str) -> Vec<CogTopicNode> {
        let mut nodes = Vec::new();
        let mut folders: Vec<_> = self.folders.into_iter().collect();
        folders.sort_by(|(a, _), (b, _)| a.to_lowercase().cmp(&b.to_lowercase()));
        for (label, folder) in folders {
            let path = if parent.is_empty() {
                label.clone()
            } else {
                format!("{parent}/{label}")
            };
            nodes.push(CogTopicNode::Folder {
                label,
                children: folder.finish(&path),
                path,
            });
        }
        let mut bindings = self.bindings;
        bindings.sort_by(|a, b| {
            topic_leaf_label(a)
                .to_lowercase()
                .cmp(&topic_leaf_label(b).to_lowercase())
        });
        nodes.extend(bindings.into_iter().map(CogTopicNode::Binding));
        nodes
    }
}

/// A deterministic hierarchy assembled from flat `topic/path::leaf` bindings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CogTopicTree {
    pub(crate) roots: Vec<CogTopicNode>,
}

impl CogTopicTree {
    pub(crate) fn from_bindings(bindings: Vec<CogTopicBinding>) -> Self {
        let mut root = TopicFolderBuilder::default();
        let mut seen = BTreeSet::new();
        for binding in bindings {
            if !seen.insert(binding.address.clone()) {
                continue;
            }
            let Some((path, _)) = binding.address.split_once("::") else {
                continue;
            };
            let mut folder = &mut root;
            for component in path.split('/').filter(|part| !part.is_empty()) {
                folder = folder.folders.entry(component.to_string()).or_default();
            }
            folder.bindings.push(binding);
        }
        Self {
            roots: root.finish(""),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Resolve one exact binding from the hierarchy. Live refreshes use the
    /// stable Topic Address rather than an index, so inserts/re-sorts cannot
    /// silently move the right pane onto another object.
    pub(crate) fn binding(&self, address: &str) -> Option<CogTopicBinding> {
        fn find(nodes: &[CogTopicNode], address: &str) -> Option<CogTopicBinding> {
            for node in nodes {
                match node {
                    CogTopicNode::Folder { children, .. } => {
                        if let Some(binding) = find(children, address) {
                            return Some(binding);
                        }
                    }
                    CogTopicNode::Binding(binding) if binding.address == address => {
                        return Some(binding.clone());
                    }
                    CogTopicNode::Binding(_) => {}
                }
            }
            None
        }
        find(&self.roots, address)
    }
}

pub(crate) fn topic_leaf_label(binding: &CogTopicBinding) -> String {
    let key = binding
        .address
        .split_once("::")
        .map(|(_, key)| key)
        .unwrap_or(binding.address.as_str());
    if binding.name.trim().is_empty() || binding.name == key {
        key.to_string()
    } else {
        format!("{key}  ·  {}", binding.name)
    }
}

/// One registered durable agent route (`cog address list`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct CogAgentAddress {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) session: String,
    #[serde(default)]
    pub(crate) cwd: String,
    #[serde(default)]
    pub(crate) created_at: i64,
    #[serde(default)]
    pub(crate) retired_at: Option<i64>,
    #[serde(default)]
    pub(crate) retired_reason: Option<String>,
}

impl CogAgentAddress {
    pub(crate) fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired_at.is_some()
    }
}

/// Presence and broker cursor state (`cog address delivery-status`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct CogDeliveryStatus {
    #[serde(default)]
    pub(crate) address: String,
    #[serde(default)]
    pub(crate) presence: String,
    #[serde(default)]
    pub(crate) lease_expires_at: Option<i64>,
    #[serde(default)]
    pub(crate) cursor: i64,
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) retry_attempt: u32,
    #[serde(default)]
    pub(crate) retry_at: Option<i64>,
    #[serde(default)]
    pub(crate) blocked_event_id: Option<i64>,
    #[serde(default)]
    pub(crate) blocked_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogReference {
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) object: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogMailEntry {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) event_id: i64,
    #[serde(default)]
    pub(crate) mail: String,
    #[serde(default)]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) at: i64,
    #[serde(default)]
    pub(crate) actor: String,
    #[serde(default)]
    pub(crate) content: serde_json::Value,
    #[serde(default)]
    pub(crate) references: Vec<CogReference>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogMail {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) participants: Vec<String>,
    #[serde(default)]
    pub(crate) entries: Vec<CogMailEntry>,
    #[serde(default)]
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct CogMailSummary {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) participants: Vec<String>,
    #[serde(default)]
    pub(crate) created_at: i64,
    #[serde(default)]
    pub(crate) latest_event_id: i64,
    #[serde(default)]
    pub(crate) bulletin: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogMailFeedEntry {
    #[serde(default)]
    pub(crate) mail: String,
    #[serde(default)]
    pub(crate) mail_name: String,
    pub(crate) entry: CogMailEntry,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogChat {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) creator: String,
    #[serde(default)]
    pub(crate) created_at: i64,
    #[serde(default)]
    pub(crate) addresses: Vec<String>,
    #[serde(default)]
    pub(crate) members: Vec<String>,
    #[serde(default)]
    pub(crate) entries: Vec<CogChatEntry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct CogChatEntry {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) event_id: i64,
    #[serde(default)]
    pub(crate) chat: String,
    #[serde(default)]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) at: i64,
    #[serde(default)]
    pub(crate) actor: String,
    #[serde(default)]
    pub(crate) content: serde_json::Value,
    #[serde(default)]
    pub(crate) references: Vec<CogReference>,
}

#[derive(PartialEq)]
pub(crate) enum CogTopicDetail {
    Graph(CogGraph),
    Note(CogMail),
    Chat(CogChat),
}

#[derive(PartialEq)]
pub(crate) struct CogAgentDetail {
    pub(crate) address: CogAgentAddress,
    pub(crate) delivery: Result<CogDeliveryStatus, String>,
    pub(crate) inbox: Result<Vec<CogMailFeedEntry>, String>,
    pub(crate) threads: Result<Vec<CogMail>, String>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct CogHomeData {
    pub(crate) topics: CogTopicTree,
    pub(crate) agents: Vec<CogAgentAddress>,
    pub(crate) agent_presence: BTreeMap<String, String>,
}

impl CogNote {
    /// The note prose — `data.summary` when present, else the raw JSON.
    pub(crate) fn summary(&self) -> String {
        match self.data.get("summary").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => json_prose(&self.data),
        }
    }
}

/// Everything a loaded graph needs to render, fetched in one background pass.
#[derive(PartialEq, Eq)]
pub(crate) struct CogBundle {
    pub(crate) graph: CogGraph,
    pub(crate) status: CogGraphStatus,
    pub(crate) nodes: Vec<CogNode>,
    pub(crate) edges: Vec<CogEdge>,
    /// node id → its status-transition + note log, keyed for O(1) lookup.
    pub(crate) logs: BTreeMap<String, Vec<CogLogEntry>>,
    /// node id → its notes.
    pub(crate) notes: BTreeMap<String, Vec<CogNote>>,
    /// The ASCII DAG render (`cog graph render`), shown in the Overview.
    pub(crate) render: String,
}

/// Aggregate stats for a graph's Overview: node counts by status + node
/// claimed→done completion durations (nanoseconds).
pub(crate) struct CogStats {
    pub(crate) total: usize,
    pub(crate) done: usize,
    pub(crate) claimed: usize,
    pub(crate) open: usize,
    pub(crate) failed: usize,
    pub(crate) completed: usize,
    pub(crate) quickest_ns: Option<i64>,
    pub(crate) longest_ns: Option<i64>,
    pub(crate) average_ns: Option<i64>,
}

impl CogBundle {
    /// The effective, display-facing status of a node: the stored status, but
    /// `open` is split into `Ready` (all predecessors done) vs `Blocked`.
    pub(crate) fn effective_status(&self, node: &CogNode) -> EffStatus {
        match node.status.as_str() {
            "done" => EffStatus::Done,
            "claimed" => EffStatus::Claimed,
            "failed" => EffStatus::Failed,
            "abandoned" => EffStatus::Abandoned,
            "open" => {
                let ready = self
                    .edges
                    .iter()
                    .filter(|e| e.to == node.id)
                    .all(|e| self.node_status(&e.from) == Some("done"));
                if ready {
                    EffStatus::Ready
                } else {
                    EffStatus::Blocked
                }
            }
            _ => EffStatus::Blocked,
        }
    }

    fn node_status(&self, id: &str) -> Option<&str> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.status.as_str())
    }

    /// A node's completion duration (ns), computed from its EXISTING log — the
    /// span from when work started to when it finished. Start = the first
    /// `claimed` transition if present, else the node's earliest log entry (many
    /// cog nodes are closed straight to `done` after edits/notes, with no
    /// `claimed`). End = the `done` transition. `None` unless the node reached
    /// `done` with a positive span.
    pub(crate) fn completion_ns(&self, id: &str) -> Option<i64> {
        let log = self.logs.get(id)?;
        let status_at = |to: &str| -> Option<i64> {
            log.iter()
                .filter(|e| {
                    e.kind == "status_changed"
                        && e.data.get("to").and_then(|v| v.as_str()) == Some(to)
                })
                .map(|e| e.at)
                .max()
        };
        let done = status_at("done")?;
        let start = log
            .iter()
            .filter(|e| {
                e.kind == "status_changed"
                    && e.data.get("to").and_then(|v| v.as_str()) == Some("claimed")
            })
            .map(|e| e.at)
            .min()
            .or_else(|| log.iter().map(|e| e.at).filter(|&a| a > 0).min())?;
        let d = done - start;
        if d > 0 { Some(d) } else { None }
    }

    /// Aggregate Overview stats: status counts + completion-time min/max/avg.
    pub(crate) fn stats(&self) -> CogStats {
        let mut s = CogStats {
            total: self.nodes.len(),
            done: 0,
            claimed: 0,
            open: 0,
            failed: 0,
            completed: 0,
            quickest_ns: None,
            longest_ns: None,
            average_ns: None,
        };
        let mut sum: i128 = 0;
        for n in &self.nodes {
            match n.status.as_str() {
                "done" => s.done += 1,
                "claimed" => s.claimed += 1,
                "failed" | "abandoned" => s.failed += 1,
                _ => s.open += 1,
            }
            if let Some(d) = self.completion_ns(&n.id) {
                s.completed += 1;
                sum += d as i128;
                s.quickest_ns = Some(s.quickest_ns.map_or(d, |q| q.min(d)));
                s.longest_ns = Some(s.longest_ns.map_or(d, |l| l.max(d)));
            }
        }
        if s.completed > 0 {
            s.average_ns = Some((sum / s.completed as i128) as i64);
        }
        s
    }
}

/// Format a nanosecond duration compactly (e.g. `1.4s`, `2m 3s`, `1h 4m`).
pub(crate) fn fmt_duration_ns(ns: i64) -> String {
    let secs = ns / 1_000_000_000;
    if secs < 60 {
        let millis = ns / 1_000_000;
        return format!("{:.1}s", millis as f64 / 1000.0);
    }
    let (m, s) = (secs / 60, secs % 60);
    if m < 60 {
        return format!("{m}m {s}s");
    }
    let (h, m) = (m / 60, m % 60);
    format!("{h}h {m}m")
}

/// The display-facing status of a node (stored status + open→ready/blocked).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffStatus {
    Done,
    Claimed,
    Ready,
    Blocked,
    Failed,
    Abandoned,
}

/// Map a stored status string to an [`EffStatus`] for colouring (a transition
/// target has no edge context, so `open` shows as `Blocked`).
pub(crate) fn parse_eff_status(s: &str) -> EffStatus {
    match s {
        "done" => EffStatus::Done,
        "claimed" => EffStatus::Claimed,
        "failed" => EffStatus::Failed,
        "abandoned" => EffStatus::Abandoned,
        _ => EffStatus::Blocked,
    }
}

impl EffStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            EffStatus::Done => "done",
            EffStatus::Claimed => "claimed",
            EffStatus::Ready => "ready",
            EffStatus::Blocked => "blocked",
            EffStatus::Failed => "failed",
            EffStatus::Abandoned => "abandoned",
        }
    }
}

/// The result of a background fetch, folded back onto the tile in `cog_ui.rs`.
pub(crate) enum CogFetch {
    /// Topic hierarchy + registered-address directory for the primary browser.
    Home(Box<CogHomeData>),
    /// The graph explorer list (opening the tile / going back).
    Graphs(Vec<CogGraph>),
    /// A fully-loaded graph.
    Graph(Box<CogBundle>),
    /// A right-pane topic target; failures remain local to the selected leaf.
    TopicDetail {
        address: String,
        result: Result<CogTopicDetail, String>,
    },
    /// A selected registered address's delivery and mail detail.
    AgentDetail {
        address: String,
        detail: Box<CogAgentDetail>,
    },
}

// ── Durable semantic state + live revalidation ──────────────────────────────

/// Stable selector identity persisted for the Topic tree. Indices are
/// intentionally excluded because a newly-created sibling can reorder rows
/// between Yalda launches.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub(crate) enum CogRememberedTopic {
    Folder(String),
    Binding(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CogRememberedSource {
    #[default]
    Topics,
    Agents,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CogRememberedFocus {
    #[default]
    Selector,
    Detail,
    Events,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CogRememberedGraph {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    #[serde(default)]
    pub(crate) overview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) node: Option<String>,
}

/// The durable shadow of a Cog tile. It stores navigation and display choices,
/// never remote payloads; restore therefore always performs fresh Cog reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CogRemembered {
    #[serde(default)]
    pub(crate) source: CogRememberedSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) topic: Option<CogRememberedTopic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) graph: Option<CogRememberedGraph>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) topic_collapsed: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) json_collapsed: BTreeSet<String>,
    #[serde(default)]
    pub(crate) events_hidden: bool,
    #[serde(default)]
    pub(crate) focus: CogRememberedFocus,
}

impl CogRemembered {
    pub(crate) fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

pub(crate) type CogRememberedHandle = std::rc::Rc<std::cell::RefCell<CogRemembered>>;

/// Identity captured at the start of a non-graph refresh. Applying the result
/// requires an exact key match, which prevents a slow old selection from
/// replacing a newer right pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CogLiveHomeKey {
    pub(crate) source: CogRememberedSource,
    pub(crate) topic: Option<String>,
    pub(crate) agent: Option<String>,
}

pub(crate) struct CogLiveHome {
    pub(crate) home: CogHomeData,
    pub(crate) topic: Option<(String, Result<CogTopicDetail, String>)>,
    pub(crate) agent: Option<(String, CogAgentDetail)>,
}

// ── The tile ─────────────────────────────────────────────────────────────────

/// The `App::Cog` tile. Cheap, frequently-touched fields live here; the loaded
/// payload + selection + scroll live in the cached [`CogView`] (created lazily
/// on first render). `req` is a monotonic guard so a stale background response
/// for an old selection is discarded.
pub(crate) struct CogTile {
    pub(crate) title: String,
    pub(crate) req: u64,
    pub(crate) view: Option<Entity<CogView>>,
    /// The live `cog graph watch` child, if a graph is open. Killed on graph
    /// change and on tile drop (see `Drop`) so the subprocess never leaks.
    pub(crate) watch: Option<std::process::Child>,
    /// Monotonic generation for the watcher; the drain task tags events with it
    /// and stale events (from a killed prior watcher) are dropped.
    pub(crate) watch_gen: u64,
    /// A graph-refresh (triggered by a live event) is in flight — coalesces a
    /// burst of events into at most one in-flight reload plus one queued.
    pub(crate) refreshing: bool,
    /// An event arrived while a refresh was in flight — refresh once more when it
    /// completes so the final state isn't missed.
    pub(crate) refresh_pending: bool,
    /// True until the graph list has been kicked. A tile restored from disk never
    /// runs `open_cog_inner`, so its first render kicks the load (else it sits
    /// frozen on "loading graphs…"). Cleared by `cog_load_graphs`.
    pub(crate) needs_load: bool,
    /// Shared durable navigation shadow. The cached view owns its mutations;
    /// the tile retains a handle solely so workspace snapshotting needs no GPUI
    /// context and cannot serialize stale remote payloads.
    pub(crate) remembered: CogRememberedHandle,
    /// Per-instance identity for generation-guarding asynchronous live reads.
    pub(crate) live_token: u64,
    pub(crate) live_refreshing: bool,
    pub(crate) live_pending: bool,
    /// Dropping the tile cancels its bounded non-graph revalidation loop.
    pub(crate) live_task: Option<Task<()>>,
}

impl CogTile {
    pub(crate) fn new() -> Self {
        Self::restored(CogRemembered::default())
    }

    pub(crate) fn restored(remembered: CogRemembered) -> Self {
        static NEXT_LIVE_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let title = remembered
            .graph
            .as_ref()
            .and_then(|graph| graph.label.clone())
            .unwrap_or_else(|| "Cog".into());
        CogTile {
            title,
            req: 0,
            view: None,
            watch: None,
            watch_gen: 0,
            refreshing: false,
            refresh_pending: false,
            needs_load: true,
            remembered: std::rc::Rc::new(std::cell::RefCell::new(remembered)),
            live_token: NEXT_LIVE_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            live_refreshing: false,
            live_pending: false,
            live_task: None,
        }
    }

    pub(crate) fn title(&self) -> String {
        self.title.clone()
    }

    pub(crate) fn remembered(&self) -> CogRemembered {
        self.remembered.borrow().clone()
    }
}

impl Drop for CogTile {
    fn drop(&mut self) {
        // A std `Child` does NOT kill on drop — do it explicitly so a closed /
        // replaced Cog tile never leaves an orphaned `cog graph watch` running.
        if let Some(mut child) = self.watch.take() {
            let _ = child.kill();
        }
    }
}

/// One live event from `cog graph watch` — the parsed JSON plus a monotonic
/// sequence for a stable render key / display index.
pub(crate) struct CogEvent {
    pub(crate) seq: u64,
    pub(crate) raw: serde_json::Value,
}

// ── Subprocess client ────────────────────────────────────────────────────────

/// Run `cog <args>` and return stdout, surfacing a missing binary / non-zero
/// exit as a single human-readable string.
fn run_cog(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("cog")
        .args(args)
        .output()
        .map_err(|e| {
            format!("failed to run `cog` — is the cog CLI on your PATH and cogd running? ({e})")
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            format!(
                "`cog {}` failed (exit {:?})",
                args.join(" "),
                out.status.code()
            )
        } else {
            format!("`cog {}`: {err}", args.join(" "))
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `cog <args>` and parse its JSON stdout into `T`.
fn cog_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Result<T, String> {
    let raw = run_cog(args)?;
    serde_json::from_str(&raw)
        .map_err(|e| format!("parsing `cog {}` output failed: {e}", args.join(" ")))
}

/// List every graph (`cog graph list`).
pub(crate) fn list_graphs() -> Result<Vec<CogGraph>, String> {
    let mut graphs: Vec<CogGraph> = cog_json(&["graph", "list"])?;
    graphs.sort_by(|a, b| a.label().to_lowercase().cmp(&b.label().to_lowercase()));
    Ok(graphs)
}

/// List every live hierarchical Topic binding. The empty prefix is the public
/// root-browser contract documented by Cog; `--limit 1000` matches the server's
/// maximum page size for a dense first-draft explorer.
pub(crate) fn list_topic_bindings() -> Result<Vec<CogTopicBinding>, String> {
    let mut bindings: Vec<CogTopicBinding> = cog_json(&["topic", "list", "", "--limit", "1000"])?;
    bindings.sort_by(|a, b| a.address.cmp(&b.address));
    bindings.dedup_by(|a, b| a.address == b.address);
    Ok(bindings)
}

pub(crate) fn list_topics() -> Result<CogTopicTree, String> {
    Ok(CogTopicTree::from_bindings(list_topic_bindings()?))
}

pub(crate) fn load_home() -> Result<CogHomeData, String> {
    // Agent discovery is independent of Topics. Older Cog deployments simply
    // produce an empty directory, while a current deployment enriches every
    // active row with its broker presence before the first paint.
    let agents = list_agents().unwrap_or_default();
    let agent_presence = agents
        .iter()
        .filter(|address| !address.is_retired())
        .filter_map(|address| {
            cog_json::<CogDeliveryStatus>(&["address", "delivery-status", &address.id])
                .ok()
                .map(|status| (address.id.clone(), status.presence))
        })
        .collect();
    Ok(CogHomeData {
        topics: list_topics()?,
        agents,
        agent_presence,
    })
}

/// Revalidate the complete visible non-graph surface in one background pass.
/// The directory is loaded first, then the active detail is resolved from that
/// fresh directory so a rebinding can never fetch an obsolete object id.
pub(crate) fn load_live_home(key: CogLiveHomeKey) -> Result<CogLiveHome, String> {
    let home = load_home()?;
    let topic = if key.source == CogRememberedSource::Topics {
        key.topic.as_deref().and_then(|address| {
            home.topics.binding(address).map(|binding| {
                let address = binding.address.clone();
                let detail = load_topic_detail(&binding);
                (address, detail)
            })
        })
    } else {
        None
    };
    let agent = if key.source == CogRememberedSource::Agents {
        key.agent.as_deref().and_then(|id| {
            home.agents
                .iter()
                .find(|address| address.id == id)
                .cloned()
                .map(|address| {
                    let id = address.id.clone();
                    (id, load_agent_detail(address))
                })
        })
    } else {
        None
    };
    Ok(CogLiveHome { home, topic, agent })
}

/// Load the typed target behind one Topic leaf. Graph selection first paints a
/// compact record; activation separately enters the existing full graph loader.
pub(crate) fn load_topic_detail(binding: &CogTopicBinding) -> Result<CogTopicDetail, String> {
    match binding.kind {
        CogTopicKind::Graph => {
            cog_json(&["graph", "get", &binding.object]).map(CogTopicDetail::Graph)
        }
        CogTopicKind::Bulletin => {
            cog_json(&["mail", "get", &binding.object]).map(CogTopicDetail::Note)
        }
        CogTopicKind::Chat => cog_json(&["chat", "get", &binding.object]).map(CogTopicDetail::Chat),
    }
}

/// The installation-wide address directory, active routes first and then by
/// human label. Retired routes remain visible for historical mail inspection.
pub(crate) fn list_agents() -> Result<Vec<CogAgentAddress>, String> {
    let mut agents: Vec<CogAgentAddress> = cog_json(&["address", "list"])?;
    agents.sort_by(|a, b| {
        a.is_retired()
            .cmp(&b.is_retired())
            .then_with(|| a.label().to_lowercase().cmp(&b.label().to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(agents)
}

/// Load a selected address's broker state, incoming feed, and all direct Mail
/// threads in which it participates. The three reads are intentionally partial:
/// one unsupported/failed Cog endpoint does not hide the immutable address or
/// other readable data.
pub(crate) fn load_agent_detail(address: CogAgentAddress) -> CogAgentDetail {
    let delivery = cog_json(&["address", "delivery-status", &address.id]);
    let inbox = cog_json(&[
        "mail",
        "inbox",
        &address.id,
        "--since",
        "0",
        "--limit",
        "1000",
    ]);
    let threads = (|| {
        let summaries: Vec<CogMailSummary> = cog_json(&["mail", "list"])?;
        let mut relevant: Vec<_> = summaries
            .into_iter()
            .filter(|mail| mail.participants.iter().any(|id| id == &address.id))
            .collect();
        relevant.sort_by(|a, b| {
            b.latest_event_id
                .cmp(&a.latest_event_id)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let mut mails = Vec::with_capacity(relevant.len());
        for summary in relevant {
            let mut mail = cog_json::<CogMail>(&["mail", "get", &summary.id])?;
            mail.entries
                .sort_by(|a, b| a.event_id.cmp(&b.event_id).then_with(|| a.id.cmp(&b.id)));
            mails.push(mail);
        }
        Ok(mails)
    })();
    CogAgentDetail {
        address,
        delivery,
        inbox,
        threads,
    }
}

/// Load a graph and everything needed to render it: the graph record, derived
/// status, nodes, edges, per-node logs (status transitions + notes), and notes.
/// One background pass; per-node logs are N subprocess calls (`cog node log`).
pub(crate) fn load_graph(id: &str) -> Result<CogBundle, String> {
    let graph: CogGraph = cog_json(&["graph", "get", id])?;
    let status: CogGraphStatus = cog_json(&["graph", "status", id]).unwrap_or_default();
    let nodes: Vec<CogNode> = cog_json(&["graph", "nodes", id])?;
    let edges: Vec<CogEdge> = cog_json(&["graph", "edges", id]).unwrap_or_default();
    let node_notes: Vec<CogNodeNotes> =
        cog_json(&["graph", "read-node-notes", id]).unwrap_or_default();
    // The ASCII DAG render for the Overview (raw text, not JSON).
    let render = run_cog(&["graph", "render", id]).unwrap_or_default();

    let mut notes: BTreeMap<String, Vec<CogNote>> = BTreeMap::new();
    for nn in node_notes {
        notes.insert(nn.node, nn.notes);
    }

    let mut logs: BTreeMap<String, Vec<CogLogEntry>> = BTreeMap::new();
    for n in &nodes {
        // A per-node log failure is non-fatal — the node still renders without
        // its transition timeline.
        if let Ok(log) = cog_json::<Vec<CogLogEntry>>(&["node", "log", &n.id]) {
            logs.insert(n.id.clone(), log);
        }
    }

    Ok(CogBundle {
        graph,
        status,
        nodes,
        edges,
        logs,
        notes,
        render,
    })
}

/// Start `cog graph watch <id>` and stream its newline-delimited JSON events.
/// Returns the child (a kill handle) and an unbounded receiver fed by a
/// dedicated reader thread (blocking stdout reads can't run on the UI executor,
/// and `cog` has no tokio reactor here — so we bridge with a plain thread +
/// `futures` channel the UI task can await). The thread exits when the child is
/// killed (stdout hits EOF) or the receiver is dropped.
pub(crate) fn spawn_watch(
    id: &str,
) -> Result<
    (
        std::process::Child,
        futures::channel::mpsc::UnboundedReceiver<String>,
    ),
    String,
> {
    let mut child = std::process::Command::new("cog")
        .args(["graph", "watch", id])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start `cog graph watch`: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "cog graph watch produced no stdout".to_string())?;
    let (tx, rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(l) if !l.trim().is_empty() => {
                    if tx.unbounded_send(l).is_err() {
                        break; // receiver gone — stop reading
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
    Ok((child, rx))
}

// ── Small helpers ────────────────────────────────────────────────────────────

/// Render free-form JSON as text. A bare string is returned as-is (prose); any
/// structure (object / array) is pretty-printed with 2-space indentation; other
/// scalars use their JSON form. `null` → empty.
pub(crate) fn json_prose(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        }
        other => other.to_string(),
    }
}

/// Is this JSON a structure (object/array) that should render as a pretty-printed
/// code block rather than bare prose?
pub(crate) fn json_is_structured(v: &serde_json::Value) -> bool {
    matches!(
        v,
        serde_json::Value::Object(_) | serde_json::Value::Array(_)
    )
}

/// Format an epoch-nanoseconds timestamp as `YYYY-MM-DD HH:MM` (UTC). Cog log
/// entries carry `at` in ns; we have no `chrono`, so convert by hand
/// (civil-from-days). Zero / negative → empty.
pub(crate) fn fmt_epoch_ns(at: i64) -> String {
    if at <= 0 {
        return String::new();
    }
    let secs = at / 1_000_000_000;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m) = (tod / 3600, (tod % 3600) / 60);

    // days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{d:02} {h:02}:{m:02}")
}
