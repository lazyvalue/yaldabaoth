//! Linear GraphQL API client + the Linear App's tile/state model.
//!
//! `App::Linear` is a viewport onto Linear (the issue tracker). The user always
//! knows what they want — an issue identifier (e.g. `FUL-420`) or a project
//! name — so there is no browsing or filtering: type the tag, press Enter, we
//! fetch exactly that and render it.
//!
//! The client talks to <https://api.linear.app/graphql> over blocking HTTP
//! (`ureq`) and authenticates with a Linear **personal API key** read from the
//! `LINEAR_API_KEY` environment variable (header `Authorization: <key>` — raw,
//! no `Bearer`). The blocking call runs on a background thread; see
//! `linear_ui.rs` (`linear_submit` → `cx.background_executor().spawn`).

use super::*;

use serde::Deserialize;

const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

/// Read the personal API key, or `None` when unset/blank so the UI can show a
/// "how to fix it" message instead of failing opaquely.
pub(crate) fn api_key() -> Option<String> {
    std::env::var("LINEAR_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// Parse an issue identifier like `FUL-420` into `(team_key, number)`. The team
/// key is upcased (Linear keys are upper-case; users type lazily). Returns
/// `None` when the string doesn't look like `<KEY>-<number>` — the caller then
/// treats the input as a project name instead.
pub(crate) fn parse_identifier(s: &str) -> Option<(String, u64)> {
    let s = s.trim();
    let (team, num) = s.rsplit_once('-')?;
    if team.is_empty() || !team.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !team.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    let number: u64 = num.trim().parse().ok()?;
    Some((team.to_ascii_uppercase(), number))
}

/// POST a GraphQL query and return its `data` object, surfacing HTTP and
/// GraphQL-level errors as a single human-readable string.
fn graphql(
    key: &str,
    query: &str,
    variables: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({ "query": query, "variables": variables });
    let value: serde_json::Value = match ureq::post(LINEAR_GRAPHQL_URL)
        .set("Content-Type", "application/json")
        .set("Authorization", key)
        .send_json(body)
    {
        Ok(r) => r
            .into_json()
            .map_err(|e| format!("decoding Linear response failed: {e}"))?,
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            let detail = detail.trim();
            let hint = if code == 400 || code == 401 {
                " (is LINEAR_API_KEY a valid personal API key?)"
            } else {
                ""
            };
            return Err(format!("Linear API HTTP {code}{hint}: {detail}"));
        }
        Err(e) => return Err(format!("Linear API request failed: {e}")),
    };
    if let Some(errors) = value.get("errors").and_then(|e| e.as_array())
        && !errors.is_empty()
    {
        let msgs: Vec<String> = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
            .map(|s| s.to_string())
            .collect();
        return Err(format!("Linear GraphQL error: {}", msgs.join("; ")));
    }
    value
        .get("data")
        .cloned()
        .ok_or_else(|| "Linear response had no data".to_string())
}

// ── Deserialized response shapes ───────────────────────────────────────────
// Everything is `Option`, so a missing/null field renders as "—" rather than
// failing the whole parse — Linear omits nulls and field visibility depends on
// the key's scope.

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NamedUser {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub name: Option<String>,
}

impl NamedUser {
    pub(crate) fn label(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| "—".into())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WorkflowState {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NameRef {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct NodeList<T> {
    pub nodes: Vec<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Comment {
    pub body: Option<String>,
    pub user: Option<NamedUser>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IssueDetail {
    pub identifier: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "priorityLabel")]
    pub priority_label: Option<String>,
    pub state: Option<WorkflowState>,
    pub assignee: Option<NamedUser>,
    pub project: Option<NameRef>,
    #[serde(rename = "projectMilestone")]
    pub milestone: Option<NameRef>,
    pub labels: Option<NodeList<NameRef>>,
    pub comments: Option<NodeList<Comment>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Milestone {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "targetDate")]
    pub target_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IssueRef {
    pub identifier: Option<String>,
    pub title: Option<String>,
    pub state: Option<WorkflowState>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProjectUpdate {
    pub body: Option<String>,
    pub user: Option<NamedUser>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProjectDetail {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub state: Option<String>,
    pub url: Option<String>,
    pub lead: Option<NamedUser>,
    #[serde(rename = "targetDate")]
    pub target_date: Option<String>,
    #[serde(rename = "projectMilestones")]
    pub milestones: Option<NodeList<Milestone>>,
    pub issues: Option<NodeList<IssueRef>>,
    #[serde(rename = "projectUpdates")]
    pub updates: Option<NodeList<ProjectUpdate>>,
}

const ISSUE_QUERY: &str = r#"
query Issue($team: String!, $number: Float!) {
  issues(filter: { team: { key: { eq: $team } }, number: { eq: $number } }, first: 1) {
    nodes {
      identifier
      title
      description
      url
      priorityLabel
      state { name }
      assignee { displayName name }
      project { name }
      projectMilestone { name }
      labels { nodes { name } }
      comments(first: 100) { nodes { body createdAt user { displayName name } } }
    }
  }
}"#;

const PROJECT_QUERY: &str = r#"
query Project($q: String!) {
  projects(filter: { name: { containsIgnoreCase: $q } }, first: 1) {
    nodes {
      name
      description
      content
      state
      url
      targetDate
      lead { displayName name }
      projectMilestones(first: 100) { nodes { name description targetDate } }
      issues(first: 250) { nodes { identifier title state { name } } }
      projectUpdates(first: 50) { nodes { body createdAt user { displayName name } } }
    }
  }
}"#;

/// Fetch a single issue by `<team>-<number>`.
pub(crate) fn fetch_issue(key: &str, team: &str, number: u64) -> Result<IssueDetail, String> {
    let data = graphql(
        key,
        ISSUE_QUERY,
        serde_json::json!({ "team": team, "number": number }),
    )?;
    let node = data
        .get("issues")
        .and_then(|i| i.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|a| a.first())
        .cloned();
    match node {
        Some(n) => serde_json::from_value(n).map_err(|e| format!("parsing issue failed: {e}")),
        None => Err(format!(
            "No issue {team}-{number} found — check the identifier and that your API key can see that team."
        )),
    }
}

/// Fetch the best name-matching project for `query` (case-insensitive contains).
pub(crate) fn fetch_project(key: &str, query: &str) -> Result<ProjectDetail, String> {
    let data = graphql(key, PROJECT_QUERY, serde_json::json!({ "q": query }))?;
    let node = data
        .get("projects")
        .and_then(|p| p.get("nodes"))
        .and_then(|n| n.as_array())
        .and_then(|a| a.first())
        .cloned();
    match node {
        Some(n) => serde_json::from_value(n).map_err(|e| format!("parsing project failed: {e}")),
        None => Err(format!(
            "No project matching \"{query}\" — type part of the project name (not an issue id)."
        )),
    }
}

/// The result the background fetch hands back to the UI thread.
pub(crate) enum LinearFetch {
    Issue(Box<IssueDetail>),
    Project(Box<ProjectDetail>),
}

// ── Tile (layout-tree content) ──────────────────────────────────────────────

/// A Linear viewport. Lives in the workspace layout tree as `App::Linear`.
/// Holds the cheap, frequently-edited bits (the input line, the request id, a
/// denormalized title for the tab strip) inline; the EXPENSIVE part — the
/// loaded issue/project body — lives in a cached [`LinearView`] entity
/// (`linear_view.rs`) so a keystroke in the input doesn't re-render the body.
///
/// `view` is lazily created on first render (`restore_content` has no `cx`).
/// `req` is a monotonic guard so a slow response can't overwrite a newer query.
/// `title` mirrors the loaded entity's identifier/name so the tab strip /
/// window title can read it without a `cx` (the body's payload needs one).
pub(crate) struct LinearTile {
    pub(crate) input: String,
    pub(crate) req: u64,
    pub(crate) title: String,
    pub(crate) view: Option<Entity<LinearView>>,
}

impl LinearTile {
    pub(crate) fn new() -> Self {
        LinearTile {
            input: String::new(),
            req: 0,
            title: "Linear".into(),
            view: None,
        }
    }

    /// Tab / window title — the loaded entity's identifier/name, else "Linear".
    pub(crate) fn title(&self) -> String {
        self.title.clone()
    }
}
