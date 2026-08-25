//! Root/shell lifecycle for the singleton `App::AgentStats` tile.
//!
//! The root owns the one cached body and performs repository scans off the
//! paint path. Versioned observations are restored at boot and cloned to a
//! background writer so agent streaming never waits on telemetry I/O.

use super::*;
use std::path::Path;
use std::time::Instant;

impl YaldaGpuiView {
    fn ensure_agent_stats_view(&mut self, cx: &mut Context<Self>) -> Entity<AgentStatsView> {
        if let Some(view) = &self.agent_stats_view {
            return view.clone();
        }
        let weak = cx.entity().downgrade();
        let restored = self.telemetry_store.latest_agent().cloned();
        let (observation, source) = match restored {
            Some(observation) => (Some(observation), ObservationSource::Restored),
            None => (
                Some(AgentFleetObservation {
                    captured_at_unix_ms: unix_millis_now(),
                    snapshot: collect_agent_metrics(
                        &self.agent_roster,
                        &self.sessions,
                        Instant::now(),
                        cx,
                    ),
                }),
                ObservationSource::Live,
            ),
        };
        let view = cx.new(|_| AgentStatsView::with_agent_observation(weak, observation, source));
        self.agent_stats_view = Some(view.clone());
        view
    }

    fn agent_stats_tile_id(&self) -> Option<workspace::WindowId> {
        self.workspace.all_window_ids().into_iter().find(|id| {
            self.workspace
                .tile(*id)
                .is_some_and(|window| matches!(&window.content, App::AgentStats))
        })
    }

    /// Open or focus the one global Agent Stats tile. The repository source is
    /// captured before focus moves, so opening from another project refreshes
    /// that project's repository instead of the stats tile's original owner.
    pub(crate) fn open_agent_stats(&mut self, cx: &mut Context<Self>) {
        let project = self
            .active_project(cx)
            .unwrap_or_else(|| self.workspace.inherited_project());
        let repository_root = self.projects.cwd_of(project).map(Path::to_path_buf);
        let view = self.ensure_agent_stats_view(cx);

        if let Some(id) = self.agent_stats_tile_id() {
            self.workspace.focus_tile(id);
        } else {
            let id = self.workspace.push_detached(App::AgentStats, project);
            self.workspace.present_solo(id);
        }

        match repository_root {
            Some(root) => self.refresh_agent_stats_repository_at(root, cx),
            None => view.update(cx, |view, cx| {
                view.set_repository(RepositoryStatsState::Empty, cx)
            }),
        }
        self.transient_status = None;
        self.save_workspace_state();
        cx.notify();
    }

    /// Capture live facts at model mutation sites even when the tile is closed.
    pub(crate) fn refresh_agent_stats_agents(&mut self, cx: &mut Context<Self>) {
        let captured_at_unix_ms = unix_millis_now();
        let snapshot =
            collect_agent_metrics(&self.agent_roster, &self.sessions, Instant::now(), cx);
        if self
            .telemetry_store
            .record_agent(captured_at_unix_ms, snapshot.clone())
        {
            self.mark_telemetry_dirty(cx);
        }
        if let Some(view) = self.agent_stats_view.clone() {
            view.update(cx, |view, cx| {
                view.set_agent_observation(
                    Some(AgentFleetObservation {
                        captured_at_unix_ms,
                        snapshot,
                    }),
                    ObservationSource::Live,
                    cx,
                )
            });
        }
    }

    pub(crate) fn refresh_agent_stats_repository(&mut self, cx: &mut Context<Self>) {
        let project = self
            .active_project(cx)
            .unwrap_or_else(|| self.workspace.inherited_project());
        let root = self.projects.cwd_of(project).map(Path::to_path_buf);
        match root {
            Some(root) => self.refresh_agent_stats_repository_at(root, cx),
            None => {
                let view = self.ensure_agent_stats_view(cx);
                view.update(cx, |view, cx| {
                    view.set_repository(RepositoryStatsState::Empty, cx)
                });
            }
        }
    }

    fn refresh_agent_stats_repository_at(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let view = self.ensure_agent_stats_view(cx);
        self.agent_stats_scan_generation = self.agent_stats_scan_generation.wrapping_add(1);
        let generation = self.agent_stats_scan_generation;
        let restored = self.telemetry_store.latest_repository(&root).cloned();
        view.update(cx, |view, cx| {
            view.begin_repository_refresh(root.clone(), restored, cx)
        });

        cx.spawn(async move |this, cx| {
            let scan = cx
                .background_executor()
                .spawn(async move { scan_repository(&root) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_agent_stats_repository(generation, scan, cx);
            });
        })
        .detach();
    }

    /// Generation-gated completion seam used by the async scanner and the
    /// headless stale-result guard.
    pub(crate) fn apply_agent_stats_repository(
        &mut self,
        generation: u64,
        scan: RepositoryScan,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.agent_stats_scan_generation {
            return false;
        }
        let captured_at_unix_ms = unix_millis_now();
        if self
            .telemetry_store
            .record_repository(captured_at_unix_ms, &scan)
        {
            self.mark_telemetry_dirty(cx);
        }
        let Some(view) = self.agent_stats_view.clone() else {
            return true;
        };
        view.update(cx, |view, cx| {
            view.apply_repository_scan(captured_at_unix_ms, scan, cx)
        });
        true
    }

    fn mark_telemetry_dirty(&mut self, cx: &mut Context<Self>) {
        self.telemetry_dirty_generation = self.telemetry_dirty_generation.wrapping_add(1);
        self.start_telemetry_save(cx);
    }

    /// Serialize writes so an older clone cannot finish after and replace a
    /// newer observation. Mutations during a save fold into one follow-up.
    fn start_telemetry_save(&mut self, cx: &mut Context<Self>) {
        if self.telemetry_save_in_flight
            || self.telemetry_dirty_generation == self.telemetry_saved_generation
        {
            return;
        }
        let generation = self.telemetry_dirty_generation;
        let store = self.telemetry_store.clone();
        self.telemetry_save_in_flight = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.save() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.telemetry_save_in_flight = false;
                if let Err(error) = &result {
                    this.append_system_console(
                        ConsoleLevel::Error,
                        &format!("Could not persist telemetry: {error}"),
                        cx,
                    );
                } else {
                    this.telemetry_saved_generation = generation;
                }
                if this.telemetry_dirty_generation != generation {
                    this.start_telemetry_save(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn notify_agent_stats_view(&mut self, reason: MissReason, cx: &mut Context<Self>) {
        if let Some(view) = self.agent_stats_view.clone() {
            view.update(cx, |view, cx| view.refresh(reason, cx));
        }
    }

    pub(crate) fn handle_agent_stats_key(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
            return;
        }
        let press = keystroke_to_keypress(&ev.keystroke);
        if self.leader_intercept(&press, cx) {
            return;
        }
        let Some(view) = self.agent_stats_view.clone() else {
            return;
        };
        match press.key {
            Key::Char('h') | Key::Left | Key::Char('1') => {
                view.update(cx, |view, cx| view.select_tab(AgentStatsTab::Agents, cx));
            }
            Key::Char('l') | Key::Right | Key::Char('2') => {
                view.update(cx, |view, cx| {
                    view.select_tab(AgentStatsTab::Repository, cx)
                });
            }
            Key::Tab => {
                let tab = match view.read(cx).active_tab() {
                    AgentStatsTab::Agents => AgentStatsTab::Repository,
                    AgentStatsTab::Repository => AgentStatsTab::Agents,
                };
                view.update(cx, |view, cx| view.select_tab(tab, cx));
            }
            Key::Char('j') | Key::Down => {
                view.update(cx, |view, cx| view.scroll_by(32.0, cx));
            }
            Key::Char('k') | Key::Up => {
                view.update(cx, |view, cx| view.scroll_by(-32.0, cx));
            }
            Key::PageDown => view.update(cx, |view, cx| view.scroll_by(400.0, cx)),
            Key::PageUp => view.update(cx, |view, cx| view.scroll_by(-400.0, cx)),
            Key::Char('r') => {
                self.refresh_agent_stats_agents(cx);
                self.refresh_agent_stats_repository(cx);
            }
            _ => {}
        }
    }

    pub(crate) fn render_agent_stats(
        &mut self,
        root: gpui::Div,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let view = self.ensure_agent_stats_view(cx);
        root.key_context("AgentStatsView")
            .on_key_down(cx.listener(Self::handle_agent_stats_key))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::restart))
            .on_action(cx.listener(Self::open_browser))
            .on_action(cx.listener(Self::open_agent))
            .on_action(cx.listener(Self::open_linear))
            .on_action(cx.listener(Self::open_cog))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_from_clipboard))
            .on_action(cx.listener(Self::rename_workspace))
            .on_action(cx.listener(Self::also_show_tile))
            .on_action(cx.listener(Self::toggle_file_browser_rail))
            .on_action(cx.listener(Self::toggle_jump_panel))
            .on_action(cx.listener(Self::open_jump_palette))
            .workspace_nav(cx)
            .on_action(cx.listener(Self::toggle_outline_rail))
            .on_action(cx.listener(Self::flip_rail_side))
            .flex()
            .flex_col()
            .size_full()
            .bg(self.editor_bg())
            .child(cached_child(view))
    }
}
