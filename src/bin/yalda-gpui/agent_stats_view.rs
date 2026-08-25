//! Cached body for the singleton Agent Stats system tile.
//!
//! This is a yux component: it owns only presentation state (the selected tab,
//! scroll offset, and the latest immutable telemetry snapshots), reads global
//! chrome from the root, and invalidates itself at its mutation sites. The
//! shell owns opening/focusing the singleton tile and the background repository
//! scan; neither concern belongs in this render component.

use super::*;
use std::path::Path;

const MAX_AGENT_ROWS: usize = 200;
const ROW_TEXT_PT: f32 = 11.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentStatsTab {
    #[default]
    Agents,
    Inactive,
    Repository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservationSource {
    Live,
    Restored,
}

#[derive(Debug, Clone, PartialEq)]
struct AgentStatsAgentObservation {
    captured_at_unix_ms: u64,
    snapshot: FleetMetricSnapshot,
    source: ObservationSource,
}

/// One generic repository source offered by Agent Stats. `key` is the
/// normalized scan-input path and remains stable across catalog rebuilds;
/// successful scans may resolve that input to a parent Git root in the durable
/// observation. Registered projects win the display label while retained-only
/// analyses use an honest path-derived label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryChoice {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) root: PathBuf,
    pub(crate) registered: bool,
    pub(crate) has_observation: bool,
}

/// Repository-page state. `Empty` is distinct from `Loading`: it means there
/// is no project/repository selection to inspect, not that a scan is pending.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum RepositoryStatsState {
    #[default]
    Empty,
    Loading {
        cwd: PathBuf,
    },
    Scan(RepositoryScan),
    Observed {
        observation: RepositoryObservation,
        source: ObservationSource,
        refreshing: bool,
        refresh_error: Option<RepositoryScan>,
    },
}

pub(crate) struct AgentStatsView {
    root: WeakEntity<YaldaGpuiView>,
    active_tab: AgentStatsTab,
    agents: Option<AgentStatsAgentObservation>,
    repository_choices: Vec<RepositoryChoice>,
    selected_repository_key: Option<String>,
    repository_selection_explicit: bool,
    repository_picker_open: bool,
    repository_picker_index: usize,
    repository: RepositoryStatsState,
    scroll: ScrollHandle,
}

impl AgentStatsView {
    pub(crate) fn new(root: WeakEntity<YaldaGpuiView>) -> Self {
        Self::with_agent_observation(root, None, ObservationSource::Live)
    }

    /// Construct with the current fleet snapshot without notifying. Persisted
    /// tiles may lazily create this cached child during paint; supplying the
    /// initial snapshot here keeps that construction render-side-effect free.
    pub(crate) fn with_agent_observation(
        root: WeakEntity<YaldaGpuiView>,
        observation: Option<AgentFleetObservation>,
        source: ObservationSource,
    ) -> Self {
        Self {
            root,
            active_tab: AgentStatsTab::Agents,
            agents: observation.map(|observation| AgentStatsAgentObservation {
                captured_at_unix_ms: observation.captured_at_unix_ms,
                snapshot: observation.snapshot,
                source,
            }),
            repository_choices: Vec::new(),
            selected_repository_key: None,
            repository_selection_explicit: false,
            repository_picker_open: false,
            repository_picker_index: 0,
            repository: RepositoryStatsState::Empty,
            scroll: ScrollHandle::new(),
        }
    }

    pub(crate) fn active_tab(&self) -> AgentStatsTab {
        self.active_tab
    }

    pub(crate) fn agents(&self) -> Option<&FleetMetricSnapshot> {
        self.agents
            .as_ref()
            .map(|observation| &observation.snapshot)
    }

    pub(crate) fn agent_observation_source(&self) -> Option<ObservationSource> {
        self.agents.as_ref().map(|observation| observation.source)
    }

    pub(crate) fn repository(&self) -> &RepositoryStatsState {
        &self.repository
    }

    pub(crate) fn repository_choices(&self) -> &[RepositoryChoice] {
        &self.repository_choices
    }

    pub(crate) fn selected_repository(&self) -> Option<&RepositoryChoice> {
        let key = self.selected_repository_key.as_deref()?;
        self.repository_choices
            .iter()
            .find(|choice| choice.key == key)
    }

    pub(crate) fn repository_picker_open(&self) -> bool {
        self.repository_picker_open
    }

    pub(crate) fn toggle_repository_picker(&mut self, cx: &mut Context<Self>) {
        if self.repository_choices.is_empty() {
            return;
        }
        self.repository_picker_open = !self.repository_picker_open;
        if self.repository_picker_open {
            self.repository_picker_index = self
                .selected_repository_key
                .as_deref()
                .and_then(|key| {
                    self.repository_choices
                        .iter()
                        .position(|choice| choice.key == key)
                })
                .unwrap_or(0);
        }
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn close_repository_picker(&mut self, cx: &mut Context<Self>) {
        if !self.repository_picker_open {
            return;
        }
        self.repository_picker_open = false;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn move_repository_picker(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.repository_picker_open || self.repository_choices.is_empty() {
            return;
        }
        let count = self.repository_choices.len() as i32;
        self.repository_picker_index =
            (self.repository_picker_index as i32 + delta).rem_euclid(count) as usize;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    /// Close the picker and return the stable key currently highlighted. The
    /// root applies that key after this child update completes, avoiding a
    /// re-entrant update of the cached view.
    pub(crate) fn activate_repository_picker(&mut self, cx: &mut Context<Self>) -> Option<String> {
        if !self.repository_picker_open {
            self.toggle_repository_picker(cx);
            return None;
        }
        let key = self
            .repository_choices
            .get(self.repository_picker_index)
            .map(|choice| choice.key.clone());
        self.repository_picker_open = false;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
        key
    }

    /// Replace the available catalog. An explicit picker choice stays sticky;
    /// otherwise `follow_preferred_root` makes an open/refocus follow the active
    /// project. Explicit refresh passes false so `r` always refreshes the root
    /// currently on screen.
    pub(crate) fn set_repository_catalog(
        &mut self,
        choices: Vec<RepositoryChoice>,
        preferred_root: Option<&Path>,
        follow_preferred_root: bool,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let current_is_valid = self
            .selected_repository_key
            .as_deref()
            .is_some_and(|key| choices.iter().any(|choice| choice.key == key));
        if !current_is_valid {
            self.repository_selection_explicit = false;
        }
        let retain_current = self.repository_selection_explicit || !follow_preferred_root;
        let selected = repository_selection_key(
            &choices,
            self.selected_repository_key.as_deref(),
            preferred_root,
            retain_current,
        );
        let changed = self.repository_choices != choices
            || self.selected_repository_key.as_ref() != selected.as_ref();
        self.repository_choices = choices;
        self.selected_repository_key = selected;
        self.repository_picker_index = self
            .selected_repository_key
            .as_deref()
            .and_then(|key| {
                self.repository_choices
                    .iter()
                    .position(|choice| choice.key == key)
            })
            .unwrap_or(0);
        if self.repository_choices.is_empty() {
            self.repository_picker_open = false;
        }
        let root = self.selected_repository().map(|choice| choice.root.clone());
        if changed {
            record_notify("agent_stats", MissReason::Refresh);
            cx.notify();
        }
        root
    }

    /// Select a current catalog entry by stable key. The caller owns starting
    /// the off-thread scan for the returned root.
    pub(crate) fn select_repository(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let root = self
            .repository_choices
            .iter()
            .find(|choice| choice.key == key)
            .map(|choice| choice.root.clone())?;
        self.repository_selection_explicit = true;
        if self.selected_repository_key.as_deref() != Some(key) {
            self.selected_repository_key = Some(key.to_string());
            self.repository_picker_index = self
                .repository_choices
                .iter()
                .position(|choice| choice.key == key)
                .unwrap_or(0);
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            record_notify("agent_stats", MissReason::Refresh);
            cx.notify();
        }
        Some(root)
    }

    pub(crate) fn mark_repository_analyzed(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(choice) = self
            .repository_choices
            .iter_mut()
            .find(|choice| choice.key == key)
        else {
            return;
        };
        if choice.has_observation {
            return;
        }
        choice.has_observation = true;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn select_tab(&mut self, tab: AgentStatsTab, cx: &mut Context<Self>) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        if tab != AgentStatsTab::Repository {
            self.repository_picker_open = false;
        }
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn set_agent_observation(
        &mut self,
        observation: Option<AgentFleetObservation>,
        source: ObservationSource,
        cx: &mut Context<Self>,
    ) {
        let next = observation.map(|observation| AgentStatsAgentObservation {
            captured_at_unix_ms: observation.captured_at_unix_ms,
            snapshot: observation.snapshot,
            source,
        });
        if self.agents == next {
            return;
        }
        self.agents = next;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn set_repository(&mut self, state: RepositoryStatsState, cx: &mut Context<Self>) {
        if self.repository == state {
            return;
        }
        self.repository = state;
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    pub(crate) fn begin_repository_refresh(
        &mut self,
        cwd: PathBuf,
        restored: Option<RepositoryObservation>,
        cx: &mut Context<Self>,
    ) {
        let state = match restored {
            Some(observation) => RepositoryStatsState::Observed {
                observation,
                source: ObservationSource::Restored,
                refreshing: true,
                refresh_error: None,
            },
            None => RepositoryStatsState::Loading { cwd },
        };
        self.set_repository(state, cx);
    }

    pub(crate) fn apply_repository_scan(
        &mut self,
        captured_at_unix_ms: u64,
        scan: RepositoryScan,
        cx: &mut Context<Self>,
    ) {
        let next = match scan {
            RepositoryScan::Ready(snapshot) => RepositoryStatsState::Observed {
                observation: RepositoryObservation {
                    captured_at_unix_ms,
                    snapshot,
                },
                source: ObservationSource::Live,
                refreshing: false,
                refresh_error: None,
            },
            error => match &self.repository {
                RepositoryStatsState::Observed {
                    observation,
                    source: ObservationSource::Restored,
                    ..
                } => RepositoryStatsState::Observed {
                    observation: observation.clone(),
                    source: ObservationSource::Restored,
                    refreshing: false,
                    refresh_error: Some(error),
                },
                _ => RepositoryStatsState::Scan(error),
            },
        };
        self.set_repository(next, cx);
    }

    /// Explicit push path for global theme/zoom changes. Parent notification
    /// cannot dirty a cached child, so the shell calls this on the live view.
    pub(crate) fn refresh(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        record_notify("agent_stats", reason);
        cx.notify();
    }

    pub(crate) fn scroll_by(&mut self, down: f32, cx: &mut Context<Self>) {
        let current = self.scroll.offset();
        self.scroll
            .set_offset(point(current.x, (current.y - px(down)).min(px(0.0))));
        record_notify("agent_stats", MissReason::Refresh);
        cx.notify();
    }

    fn render_agents(&self, st: &DetailStyle, palette: StatsPalette) -> AnyElement {
        let Some(observation) = &self.agents else {
            return empty_state(
                "agent-stats-agents-loading",
                "Collecting agent metrics…",
                "Live fleet facts will appear as sessions are discovered.",
                st,
            );
        };
        let snapshot = &observation.snapshot;
        let active_agents = snapshot
            .agents
            .iter()
            .filter(|agent| agent.state.is_active())
            .collect::<Vec<_>>();
        if active_agents.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(observation_row(
                    observation.source,
                    observation.captured_at_unix_ms,
                    false,
                    st,
                ))
                .child(empty_state(
                    "agent-stats-agents-empty",
                    "No active agent sessions",
                    "Ready and Working agents appear here. Archived and unavailable agents are under Inactive.",
                    st,
                ))
                .into_any_element();
        }

        let averages = &snapshot.averages;
        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap_2()
            .child(observation_row(
                observation.source,
                observation.captured_at_unix_ms,
                false,
                st,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(metric_card("Active", active_agents.len().to_string(), st))
                    .child(metric_card(
                        "Working",
                        snapshot.working.to_string(),
                        &DetailStyle {
                            accent: palette.working,
                            ..clone_style(st)
                        },
                    ))
                    .child(metric_card(
                        "Ready",
                        snapshot.ready.to_string(),
                        &DetailStyle {
                            accent: palette.ready,
                            ..clone_style(st)
                        },
                    )),
            )
            .child(section_heading("Active averages", st))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .child(average_row("Turns", averages.settled_turns, 1, "", st))
                    .child(average_row("Tools", averages.tool_total, 1, "", st))
                    .child(average_row(
                        "Tool failures",
                        averages.tool_failures,
                        1,
                        "",
                        st,
                    ))
                    .child(average_row("Context", averages.context_percent, 1, "%", st))
                    .child(average_row("Cost", averages.cost_usd, 2, " USD", st))
                    .child(average_row(
                        "Active turn",
                        averages.current_turn_elapsed_secs,
                        1,
                        "s",
                        st,
                    )),
            )
            .child(section_heading("Active agents", st))
            .child(agent_table_header(st));

        let (shown, omitted) = bounded_agent_row_counts(active_agents.len());
        for (index, agent) in active_agents.into_iter().take(shown).enumerate() {
            body = body.child(probe_bounds_dyn(
                format!("agent-stats-row-{}", agent.row_id),
                agent_row(index, agent, st, palette),
            ));
        }
        if omitted > 0 {
            body = body.child(omitted_row(omitted, "additional agents", st));
        }
        body.into_any_element()
    }

    fn render_inactive_agents(&self, st: &DetailStyle, palette: StatsPalette) -> AnyElement {
        let Some(observation) = &self.agents else {
            return empty_state(
                "agent-stats-inactive-loading",
                "Collecting inactive agents…",
                "Archived and unavailable sessions will appear when the roster is loaded.",
                st,
            );
        };
        let snapshot = &observation.snapshot;
        let inactive_agents = snapshot
            .agents
            .iter()
            .filter(|agent| !agent.state.is_active())
            .collect::<Vec<_>>();
        if inactive_agents.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(observation_row(
                    observation.source,
                    observation.captured_at_unix_ms,
                    false,
                    st,
                ))
                .child(empty_state(
                    "agent-stats-inactive-empty",
                    "No inactive agent sessions",
                    "Archived and disconnected unarchived sessions are kept here when present.",
                    st,
                ))
                .into_any_element();
        }

        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap_2()
            .child(observation_row(
                observation.source,
                observation.captured_at_unix_ms,
                false,
                st,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(metric_card(
                        "Inactive",
                        inactive_agents.len().to_string(),
                        st,
                    ))
                    .child(metric_card("Archived", snapshot.archived.to_string(), st))
                    .child(metric_card(
                        "Unavailable",
                        snapshot.unavailable.to_string(),
                        &DetailStyle {
                            accent: st.dim,
                            ..clone_style(st)
                        },
                    )),
            )
            .child(section_heading("Inactive agents", st))
            .child(agent_table_header(st));

        let (shown, omitted) = bounded_agent_row_counts(inactive_agents.len());
        for (index, agent) in inactive_agents.into_iter().take(shown).enumerate() {
            body = body.child(probe_bounds_dyn(
                format!("agent-stats-row-{}", agent.row_id),
                agent_row(index, agent, st, palette),
            ));
        }
        if omitted > 0 {
            body = body.child(omitted_row(omitted, "additional inactive agents", st));
        }
        body.into_any_element()
    }

    fn render_repository(
        &self,
        st: &DetailStyle,
        palette: StatsPalette,
        border: Hsla,
        selected_bg: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = self.render_repository_selector(st, border, selected_bg, cx);
        let content = match &self.repository {
            RepositoryStatsState::Empty => empty_state(
                "agent-stats-repository-empty",
                "No repository selected",
                "Select a project to inspect its tracked source layout.",
                st,
            ),
            RepositoryStatsState::Loading { cwd } => empty_state(
                "agent-stats-repository-loading",
                "Scanning repository…",
                &format!("Reading bounded Git metadata from {}", cwd.display()),
                st,
            ),
            RepositoryStatsState::Scan(RepositoryScan::NotGit { cwd }) => empty_state(
                "agent-stats-repository-not-git",
                "Not a Git repository",
                &format!("{} has no discoverable Git worktree.", cwd.display()),
                st,
            ),
            RepositoryStatsState::Scan(RepositoryScan::CommandError(error)) => {
                repository_error(error, st)
            }
            RepositoryStatsState::Scan(RepositoryScan::Ready(snapshot)) => {
                repository_ready(snapshot, st, palette)
            }
            RepositoryStatsState::Observed {
                observation,
                source,
                refreshing,
                refresh_error,
            } => {
                let mut body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(observation_row(
                        *source,
                        observation.captured_at_unix_ms,
                        *refreshing,
                        st,
                    ))
                    .child(repository_ready(&observation.snapshot, st, palette));
                if let Some(error) = refresh_error {
                    body = body.child(match error {
                        RepositoryScan::CommandError(error) => repository_error(error, st),
                        RepositoryScan::NotGit { cwd } => dim_line(
                            &format!(
                                "Refresh could not find a Git repository at {}; showing the last durable analysis.",
                                cwd.display()
                            ),
                            st,
                        ),
                        RepositoryScan::Ready(_) => div().into_any_element(),
                    });
                }
                body.into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_3()
            .child(selector)
            .child(content)
            .into_any_element()
    }

    fn render_repository_selector(
        &self,
        st: &DetailStyle,
        border: Hsla,
        selected_bg: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(selected) = self.selected_repository() else {
            return empty_state(
                "agent-stats-repository-selector-empty",
                "No repositories available",
                "Register a project or retain a repository analysis to add it here.",
                st,
            );
        };
        let selected_label = selected.label.clone();
        let selected_root = selected.root.display().to_string();
        let selected_key = selected.key.clone();
        let mut selector = div()
            .id("agent-stats-repository-selector")
            .flex()
            .flex_col()
            .w_full()
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(
                div()
                    .id("agent-stats-repository-selector-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(move |style| style.bg(selected_bg))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .font_family(st.prose.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(st.base)
                                    .child(SharedString::from(selected_label)),
                            )
                            .child(
                                div()
                                    .font_family(st.mono.clone())
                                    .text_size(px(10.5))
                                    .text_color(st.dim)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(selected_root)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(st.mono.clone())
                            .text_size(px(11.0))
                            .text_color(st.accent)
                            .child(if self.repository_picker_open {
                                "▴"
                            } else {
                                "▾"
                            }),
                    )
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.toggle_repository_picker(cx)),
                    ),
            );

        if self.repository_picker_open {
            let mut choices = div()
                .id("agent-stats-repository-options")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p_2()
                .max_h(px(294.0))
                .overflow_y_scroll()
                .border_t_1()
                .border_color(border);
            for (index, choice) in self.repository_choices.iter().enumerate() {
                let label = format!("{} — {}", choice.label, choice.root.display());
                let badge = if choice.key == selected_key {
                    Some(("Selected", st.accent))
                } else if !choice.registered {
                    Some(("Stored", st.dim))
                } else if choice.has_observation {
                    Some(("Analyzed", st.dim))
                } else {
                    None
                };
                let row = picker_option_row(
                    SharedString::from(format!("agent-stats-repository-option-{index}")),
                    "⌂",
                    &label,
                    badge,
                    index == self.repository_picker_index,
                    st.accent,
                    st.fg,
                    selected_bg,
                    &st.prose,
                    &st.mono,
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let Some(key) = this
                        .repository_choices
                        .get(index)
                        .map(|choice| choice.key.clone())
                    else {
                        return;
                    };
                    this.repository_picker_open = false;
                    record_notify("agent_stats", MissReason::Refresh);
                    cx.notify();
                    let root = this.root.clone();
                    cx.spawn(async move |_this, cx| {
                        if let Some(root) = root.upgrade() {
                            let _ = root.update(cx, |root, cx| {
                                root.select_agent_stats_repository(&key, cx)
                            });
                        }
                    })
                    .detach();
                }));
                choices = choices.child(probe_bounds_dyn(
                    format!("agent-stats-repository-choice-{index}"),
                    row.into_any_element(),
                ));
            }
            selector = selector.child(choices);
        }
        selector.into_any_element()
    }
}

fn repository_selection_key(
    choices: &[RepositoryChoice],
    current: Option<&str>,
    preferred_root: Option<&Path>,
    retain_current: bool,
) -> Option<String> {
    current
        .filter(|_| retain_current)
        .filter(|key| choices.iter().any(|choice| choice.key == *key))
        .map(str::to_string)
        .or_else(|| {
            preferred_root.map(repository_root_key).and_then(|key| {
                choices
                    .iter()
                    .any(|choice| choice.key == key)
                    .then_some(key)
            })
        })
        .or_else(|| {
            current
                .filter(|key| choices.iter().any(|choice| choice.key == *key))
                .map(str::to_string)
        })
        .or_else(|| choices.first().map(|choice| choice.key.clone()))
}

impl Render for AgentStatsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render("agent_stats");
        let Some(root) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let (st, bg, border, selected_bg, palette) = {
            let root = root.read(cx);
            let scale = root.text_scale;
            (
                DetailStyle {
                    fg: root.editor_fg(),
                    dim: nc(root.theme.agent.dim),
                    accent: nc(root.theme.agent.warm_accent),
                    err: nc(root.theme.agent.tool_failed),
                    mono: root.code_font.clone(),
                    prose: root.body_font.clone(),
                    base: px(13.0 * scale),
                    pt: 13.0 * scale,
                },
                root.editor_bg(),
                nc(root.theme.overlay.border),
                nc(root.theme.overlay.selected_bg),
                StatsPalette {
                    working: nc(root.theme.agent.jump_working),
                    ready: nc(root.theme.agent.tool_completed),
                },
            )
        };

        let (active_count, inactive_count) = self
            .agents
            .as_ref()
            .map(|observation| {
                let snapshot = &observation.snapshot;
                (
                    snapshot.working + snapshot.ready,
                    snapshot.archived + snapshot.unavailable,
                )
            })
            .unwrap_or((0, 0));
        let agents_selected = self.active_tab == AgentStatsTab::Agents;
        let inactive_selected = self.active_tab == AgentStatsTab::Inactive;
        let repository_selected = self.active_tab == AgentStatsTab::Repository;
        let tabs = div()
            .id("agent-stats-tabs")
            .flex()
            .flex_row()
            .flex_none()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(border)
            .child(
                compact_tab(
                    "agent-stats-tab-agents",
                    "Agents",
                    Some(
                        compact_count_indicator(
                            "agent-stats-agent-count",
                            active_count,
                            st.accent,
                            &st,
                        )
                        .into_any_element(),
                    ),
                    agents_selected,
                    selected_bg,
                    &st,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.select_tab(AgentStatsTab::Agents, cx);
                })),
            )
            .child(
                compact_tab(
                    "agent-stats-tab-inactive",
                    "Inactive",
                    Some(
                        compact_count_indicator(
                            "agent-stats-inactive-count",
                            inactive_count,
                            st.dim,
                            &st,
                        )
                        .into_any_element(),
                    ),
                    inactive_selected,
                    selected_bg,
                    &st,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.select_tab(AgentStatsTab::Inactive, cx);
                })),
            )
            .child(
                compact_tab(
                    "agent-stats-tab-repository",
                    "Repository",
                    None,
                    repository_selected,
                    selected_bg,
                    &st,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.select_tab(AgentStatsTab::Repository, cx);
                })),
            );

        let body = match self.active_tab {
            AgentStatsTab::Agents => self.render_agents(&st, palette),
            AgentStatsTab::Inactive => self.render_inactive_agents(&st, palette),
            AgentStatsTab::Repository => {
                self.render_repository(&st, palette, border, selected_bg, cx)
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(st.fg)
            .child(tabs)
            .child(
                div()
                    .id("agent-stats-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().flex().flex_col().w_full().p_3().pb_6().child(body)),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
struct StatsPalette {
    working: Hsla,
    ready: Hsla,
}

fn clone_style(st: &DetailStyle) -> DetailStyle {
    DetailStyle {
        fg: st.fg,
        dim: st.dim,
        accent: st.accent,
        err: st.err,
        mono: st.mono.clone(),
        prose: st.prose.clone(),
        base: st.base,
        pt: st.pt,
    }
}

fn observation_row(
    source: ObservationSource,
    captured_at_unix_ms: u64,
    refreshing: bool,
    st: &DetailStyle,
) -> AnyElement {
    let (source_label, accent) = match source {
        ObservationSource::Live => ("Live observation", st.accent),
        ObservationSource::Restored => ("Restored observation", st.dim),
    };
    let suffix = if refreshing { " · refreshing…" } else { "" };
    let at = fmt_epoch_ns(
        captured_at_unix_ms
            .saturating_mul(1_000_000)
            .min(i64::MAX as u64) as i64,
    );
    kv_row(
        "Telemetry",
        format!("{source_label} · {at} UTC{suffix}"),
        &DetailStyle {
            accent,
            ..clone_style(st)
        },
    )
    .into_any_element()
}

fn empty_state(id: &'static str, title: &str, detail: &str, st: &DetailStyle) -> AnyElement {
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .min_h(px(220.0))
        .gap_2()
        .text_center()
        .child(
            div()
                .font_family(st.prose.clone())
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(st.pt * 1.2))
                .text_color(st.fg)
                .child(SharedString::from(title.to_string())),
        )
        .child(
            div()
                .max_w(px(560.0))
                .font_family(st.prose.clone())
                .text_size(st.base)
                .text_color(st.dim)
                .child(SharedString::from(detail.to_string())),
        )
        .into_any_element()
}

fn metric_card(label: &str, value: String, st: &DetailStyle) -> AnyElement {
    let mut wash = st.accent;
    wash.a *= 0.08;
    div()
        .flex_1()
        .min_w(px(100.0))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_sm()
        .bg(wash)
        .border_1()
        .border_color(st.accent)
        .font_family(st.mono.clone())
        .child(
            div()
                .text_size(px(st.pt * 0.72))
                .text_color(st.dim)
                .child(SharedString::from(label.to_uppercase())),
        )
        .child(
            div()
                .text_size(px(st.pt * 1.35))
                .font_weight(FontWeight::BOLD)
                .text_color(st.accent)
                .child(SharedString::from(value)),
        )
        .into_any_element()
}

fn average_row(
    label: &str,
    average: MetricAverage,
    precision: usize,
    suffix: &str,
    st: &DetailStyle,
) -> AnyElement {
    let value = format_average(average, precision, suffix);
    div()
        .w(px(230.0))
        .child(kv_row(label, value, st))
        .into_any_element()
}

fn format_average(average: MetricAverage, precision: usize, suffix: &str) -> String {
    match average.mean {
        Some(mean) => format!(
            "{mean:.precision$}{suffix}  ·  {}/{} known",
            average.denominator, average.population
        ),
        None => format!("—  ·  0/{} known", average.population),
    }
}

fn agent_table_header(st: &DetailStyle) -> AnyElement {
    dense_row("agent-stats-agent-header", st.dim, &st.mono)
        .font_weight(FontWeight::BOLD)
        .border_b_1()
        .border_color(st.dim)
        .child(fixed_cell("State", 86.0))
        .child(flex_cell("Agent"))
        .child(fixed_cell("Provider · model", 172.0))
        .child(fixed_cell("Turns", 58.0))
        .child(fixed_cell("Tools", 72.0))
        .child(fixed_cell("Context", 112.0))
        .child(fixed_cell("Cost", 72.0))
        .child(fixed_cell("Elapsed", 66.0))
        .into_any_element()
}

fn agent_row(
    index: usize,
    agent: &AgentMetricSnapshot,
    st: &DetailStyle,
    palette: StatsPalette,
) -> AnyElement {
    let state_color = match agent.state {
        AgentMetricState::Working => palette.working,
        AgentMetricState::Ready => palette.ready,
        AgentMetricState::Archived => st.dim,
        AgentMetricState::Unavailable => st.dim,
    };
    let provider = agent.provider.map(AgentProvider::label).unwrap_or("—");
    let provider_model = match agent.model.as_deref() {
        Some(model) => format!("{provider} · {model}"),
        None => provider.to_string(),
    };
    let tools = match (agent.tool_total, agent.tool_failures) {
        (Some(total), Some(failures)) if failures > 0 => format!("{total} / {failures}f"),
        (Some(total), _) => total.to_string(),
        _ => "—".to_string(),
    };
    let context = agent
        .context
        .and_then(ContextOccupancy::percent)
        .map(|percent| format!("{percent:.1}%"))
        .unwrap_or_else(|| "—".to_string());
    let cost = agent
        .cost_usd
        .map(|cost| format!("${cost:.2}"))
        .unwrap_or_else(|| "—".to_string());
    let elapsed = agent
        .current_turn_elapsed
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    let mut row = dense_row(("agent-stats-agent-row", index), st.fg, &st.mono)
        .child(div().w(px(86.0)).flex_none().child(compact_status_mark(
            ("agent-stats-agent-state", index),
            agent.state.label(),
            state_color,
            st,
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(agent.label.clone())),
        )
        .child(fixed_cell(&provider_model, 172.0))
        .child(fixed_cell(
            &agent
                .settled_turns
                .map(|turns| turns.to_string())
                .unwrap_or_else(|| "—".to_string()),
            58.0,
        ))
        .child(fixed_cell(&tools, 72.0))
        .child(fixed_cell(&context, 112.0))
        .child(fixed_cell(&cost, 72.0))
        .child(fixed_cell(&elapsed, 66.0));
    if !agent.loaded {
        row = row.text_color(st.dim);
    }
    row.into_any_element()
}

fn dense_row(
    id: impl Into<ElementId>,
    color: Hsla,
    mono: &SharedString,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .min_h(px(25.0))
        .py(px(2.0))
        .font_family(mono.clone())
        .text_size(px(ROW_TEXT_PT))
        .text_color(color)
}

fn fixed_cell(value: &str, width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .flex_none()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(SharedString::from(value.to_string()))
}

fn flex_cell(value: &str) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(SharedString::from(value.to_string()))
}

fn omitted_row(count: usize, label: &str, st: &DetailStyle) -> AnyElement {
    div()
        .w_full()
        .py_1()
        .font_family(st.mono.clone())
        .text_size(px(st.pt * 0.78))
        .text_color(st.dim)
        .child(SharedString::from(format!("… {count} {label} omitted")))
        .into_any_element()
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn bounded_agent_row_counts(total: usize) -> (usize, usize) {
    let shown = total.min(MAX_AGENT_ROWS);
    (shown, total.saturating_sub(shown))
}

fn repository_error(error: &RepositoryCommandError, st: &DetailStyle) -> AnyElement {
    div()
        .id("agent-stats-repository-error")
        .flex()
        .flex_col()
        .gap_2()
        .child(section_heading("Repository scan failed", st))
        .child(kv_row(
            "Operation",
            repository_operation_label(error.operation).to_string(),
            st,
        ))
        .child(div().text_color(st.err).child(multiline_text(
            &error.detail,
            st.err,
            &st.mono,
            st.base,
        )))
        .into_any_element()
}

fn repository_operation_label(operation: RepositoryOperation) -> &'static str {
    match operation {
        RepositoryOperation::ResolveRoot => "resolve repository root",
        RepositoryOperation::ListTrackedFiles => "list tracked files",
        RepositoryOperation::ReadStatus => "read working-tree status",
        RepositoryOperation::ReadHead => "read HEAD",
        RepositoryOperation::ReadHistory => "read recent history",
        RepositoryOperation::CountHistory => "count recent commits",
    }
}

fn repository_ready(
    snapshot: &RepositorySnapshot,
    st: &DetailStyle,
    palette: StatsPalette,
) -> AnyElement {
    let mut body = div()
        .id("agent-stats-repository-ready")
        .flex()
        .flex_col()
        .w_full()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(metric_card(
                    "Tracked files",
                    snapshot.tracked_files.to_string(),
                    st,
                ))
                .child(metric_card(
                    "Source files",
                    snapshot.source_files.to_string(),
                    st,
                ))
                .child(metric_card(
                    "Instructions",
                    snapshot.instruction_files.total.to_string(),
                    st,
                ))
                .child(metric_card(
                    "Manifests",
                    snapshot.workspace_manifests.total.to_string(),
                    st,
                )),
        )
        .child(section_heading("Repository", st))
        .child(kv_row("Root", snapshot.root.display().to_string(), st))
        .child(kv_row(
            "HEAD",
            snapshot
                .head
                .clone()
                .unwrap_or_else(|| "unborn".to_string()),
            st,
        ))
        .child(kv_row(
            "Working tree",
            if snapshot.tracked_dirty {
                "tracked changes".to_string()
            } else {
                "clean".to_string()
            },
            &DetailStyle {
                accent: if snapshot.tracked_dirty {
                    palette.working
                } else {
                    palette.ready
                },
                ..clone_style(st)
            },
        ))
        .child(section_heading("Top-level distribution", st))
        .child(count_projection(&snapshot.top_level, st))
        .child(section_heading("Source and file extensions", st))
        .child(count_projection(&snapshot.extensions, st))
        .child(section_heading("Agent instructions", st))
        .child(path_projection(&snapshot.instruction_files, st))
        .child(section_heading("Workspace manifests", st))
        .child(path_projection(&snapshot.workspace_manifests, st))
        .child(section_heading("Largest source files", st));

    if snapshot.large_source_files.items.is_empty() {
        body = body.child(dim_line("No source files found.", st));
    } else {
        for file in &snapshot.large_source_files.items {
            let lines = file
                .lines
                .map(|lines| format!(" · {lines} lines"))
                .unwrap_or_default();
            body = body.child(dense_named_count_row(
                &file.path,
                &format!("{}{}", format_bytes(file.bytes), lines),
                st,
            ));
        }
        let omitted = snapshot.large_source_files.omitted();
        if omitted > 0 {
            body = body.child(omitted_row(omitted, "source files", st));
        }
    }

    body = body.child(section_heading("Recent churn", st));
    if snapshot.recent_churn.commits_scanned == 0 {
        body = body.child(dim_line("No committed history to inspect.", st));
    } else {
        body = body.child(kv_row(
            "Coverage",
            format!(
                "{} commits · {} changed paths",
                snapshot.recent_churn.commits_scanned, snapshot.recent_churn.distinct_paths
            ),
            st,
        ));
        for item in &snapshot.recent_churn.items {
            body = body.child(dense_named_count_row(
                &item.label,
                &format!("{} touches", item.count),
                st,
            ));
        }
        let omitted = snapshot.recent_churn.omitted();
        if omitted > 0 {
            body = body.child(omitted_row(omitted, "changed paths", st));
        }
    }
    body.into_any_element()
}

fn count_projection(projection: &CountProjection, st: &DetailStyle) -> AnyElement {
    if projection.items.is_empty() {
        return dim_line("No tracked entries.", st);
    }
    let mut body = div().flex().flex_col().w_full();
    for item in &projection.items {
        body = body.child(dense_named_count_row(
            &item.label,
            &item.count.to_string(),
            st,
        ));
    }
    if projection.omitted() > 0 {
        body = body.child(omitted_row(projection.omitted(), "categories", st));
    }
    body.into_any_element()
}

fn path_projection(projection: &PathProjection, st: &DetailStyle) -> AnyElement {
    if projection.items.is_empty() {
        return dim_line("None found.", st);
    }
    let mut body = div().flex().flex_col().w_full();
    for path in &projection.items {
        body = body.child(dense_named_count_row(path, "", st));
    }
    if projection.omitted() > 0 {
        body = body.child(omitted_row(projection.omitted(), "paths", st));
    }
    body.into_any_element()
}

fn dense_named_count_row(label: &str, value: &str, st: &DetailStyle) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_h(px(23.0))
        .gap_2()
        .font_family(st.mono.clone())
        .text_size(px(ROW_TEXT_PT))
        .text_color(st.fg)
        .child(flex_cell(label))
        .child(
            div()
                .flex_none()
                .text_color(st.dim)
                .child(SharedString::from(value.to_string())),
        )
        .into_any_element()
}

fn dim_line(text: &str, st: &DetailStyle) -> AnyElement {
    div()
        .w_full()
        .py_1()
        .font_family(st.mono.clone())
        .text_size(px(ROW_TEXT_PT))
        .text_color(st.dim)
        .child(SharedString::from(text.to_string()))
        .into_any_element()
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository_choice(label: &str, root: PathBuf) -> RepositoryChoice {
        RepositoryChoice {
            key: repository_root_key(&root),
            label: label.into(),
            root,
            registered: true,
            has_observation: false,
        }
    }

    #[test]
    fn averages_expose_partial_coverage_and_unknown_values() {
        let known = MetricAverage {
            sum: Some(12.0),
            mean: Some(6.0),
            denominator: 2,
            population: 5,
        };
        let unknown = MetricAverage {
            sum: None,
            mean: None,
            denominator: 0,
            population: 5,
        };
        assert_eq!(format_average(known, 1, "s"), "6.0s  ·  2/5 known");
        assert_eq!(format_average(unknown, 1, "s"), "—  ·  0/5 known");
    }

    #[test]
    fn compact_duration_and_byte_labels_are_stable() {
        assert_eq!(format_duration(Duration::from_secs(5)), "0:05");
        assert_eq!(format_duration(Duration::from_secs(125)), "2:05");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn agent_rows_are_bounded_and_report_omissions() {
        assert_eq!(bounded_agent_row_counts(0), (0, 0));
        assert_eq!(
            bounded_agent_row_counts(MAX_AGENT_ROWS),
            (MAX_AGENT_ROWS, 0)
        );
        assert_eq!(
            bounded_agent_row_counts(MAX_AGENT_ROWS + 7),
            (MAX_AGENT_ROWS, 7)
        );
    }

    #[test]
    fn every_repository_error_operation_has_user_facing_copy() {
        for operation in [
            RepositoryOperation::ResolveRoot,
            RepositoryOperation::ListTrackedFiles,
            RepositoryOperation::ReadStatus,
            RepositoryOperation::ReadHead,
            RepositoryOperation::ReadHistory,
            RepositoryOperation::CountHistory,
        ] {
            assert!(!repository_operation_label(operation).is_empty());
        }
    }

    #[test]
    fn repository_selection_defaults_to_active_then_retains_an_explicit_choice() {
        let yalda = PathBuf::from("/test/yalda");
        let fulcrum = PathBuf::from("/test/fulcrum");
        let choices = vec![
            repository_choice("Fulcrum", fulcrum.clone()),
            repository_choice("Yalda", yalda.clone()),
        ];
        let yalda_key = repository_root_key(&yalda);
        let fulcrum_key = repository_root_key(&fulcrum);

        assert_eq!(
            repository_selection_key(&choices, None, Some(&yalda), false),
            Some(yalda_key.clone()),
            "first open follows the active project"
        );
        assert_eq!(
            repository_selection_key(&choices, Some(&fulcrum_key), Some(&yalda), true),
            Some(fulcrum_key.clone()),
            "an explicit selection remains sticky across refresh/open"
        );
        assert_eq!(
            repository_selection_key(&choices, Some(&yalda_key), Some(&fulcrum), false),
            Some(fulcrum_key),
            "an implicit selection follows a newly active project on refocus"
        );
    }
}
