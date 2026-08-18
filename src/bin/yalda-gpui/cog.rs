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
use std::collections::BTreeMap;

// ── Data model (mirrors the `cog` CLI JSON shapes) ───────────────────────────

/// One graph, as returned by `cog graph list` / `cog graph get`.
#[derive(Clone, Deserialize)]
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
#[derive(Clone, Default, Deserialize)]
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
#[derive(Clone, Deserialize)]
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
#[derive(Clone, Deserialize)]
pub(crate) struct CogEdge {
    pub(crate) from: String,
    pub(crate) to: String,
}

/// One entry in a node's log (`cog node log`). Status transitions carry
/// `kind == "status_changed"` with `data.to`; notes carry `kind == "note"`.
#[derive(Clone, Deserialize)]
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
#[derive(Clone, Deserialize)]
pub(crate) struct CogNodeNotes {
    pub(crate) node: String,
    #[serde(default)]
    pub(crate) notes: Vec<CogNote>,
}

/// One note on a node.
#[derive(Clone, Deserialize)]
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
pub(crate) struct CogBundle {
    pub(crate) graph: CogGraph,
    pub(crate) status: CogGraphStatus,
    pub(crate) nodes: Vec<CogNode>,
    pub(crate) edges: Vec<CogEdge>,
    /// node id → its status-transition + note log, keyed for O(1) lookup.
    pub(crate) logs: BTreeMap<String, Vec<CogLogEntry>>,
    /// node id → its notes.
    pub(crate) notes: BTreeMap<String, Vec<CogNote>>,
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
    /// The graph explorer list (opening the tile / going back).
    Graphs(Vec<CogGraph>),
    /// A fully-loaded graph.
    Graph(Box<CogBundle>),
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
}

impl CogTile {
    pub(crate) fn new() -> Self {
        CogTile {
            title: "Cog".into(),
            req: 0,
            view: None,
            watch: None,
            watch_gen: 0,
        }
    }

    pub(crate) fn title(&self) -> String {
        self.title.clone()
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
            format!("`cog {}` failed (exit {:?})", args.join(" "), out.status.code())
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
