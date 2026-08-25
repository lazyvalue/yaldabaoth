//! Root/shell lifecycle for the singleton `App::AgentStats` tile.
//!
//! The root owns the one cached body and performs repository scans off the
//! paint path. Versioned observations are restored at boot and cloned to a
//! background writer so agent streaming never waits on telemetry I/O.

use super::*;
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

/// Build the stable generic repository catalog. Every registered project gets
/// its own selectable scan input. A retained analysis is added as a standalone
/// choice only when no registered project is rooted at (or below) that Git
/// root; `latest_repository` already restores the retained parent observation
/// for a nested project cwd.
pub(crate) fn agent_stats_repository_catalog(
    projects: &Projects,
    telemetry: &TelemetryStore,
) -> Vec<RepositoryChoice> {
    let persisted_roots: Vec<_> = telemetry
        .repositories()
        .map(|(root, _)| (root.to_string(), PathBuf::from(root)))
        .collect();
    let mut registered_keys = HashSet::new();
    let mut choices: Vec<_> = projects
        .iter()
        .map(|(_, project)| {
            let key = repository_root_key(&project.cwd);
            registered_keys.insert(key.clone());
            RepositoryChoice {
                key,
                label: project.name.clone(),
                root: project.cwd.clone(),
                registered: true,
                has_observation: telemetry.latest_repository(&project.cwd).is_some(),
            }
        })
        .collect();

    for (key, root) in persisted_roots {
        let represented_by_project = registered_keys.contains(&key)
            || registered_keys
                .iter()
                .any(|project| Path::new(project).starts_with(&root));
        if represented_by_project {
            continue;
        }
        let label = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| root.display().to_string());
        choices.push(RepositoryChoice {
            key,
            label,
            root,
            registered: false,
            has_observation: true,
        });
    }

    choices.sort_by(|left, right| {
        right
            .registered
            .cmp(&left.registered)
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
            .then_with(|| left.key.cmp(&right.key))
    });
    choices
}

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
        let catalog = agent_stats_repository_catalog(&self.projects, &self.telemetry_store);
        let selected_root = view.update(cx, |view, cx| {
            view.set_repository_catalog(catalog, repository_root.as_deref(), true, cx)
        });

        if let Some(id) = self.agent_stats_tile_id() {
            self.workspace.focus_tile(id);
        } else {
            let id = self.workspace.push_detached(App::AgentStats, project);
            self.workspace.present_solo(id);
        }

        match selected_root {
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
        let active_root = self.projects.cwd_of(project).map(Path::to_path_buf);
        let catalog = agent_stats_repository_catalog(&self.projects, &self.telemetry_store);
        let view = self.ensure_agent_stats_view(cx);
        let selected_root = view.update(cx, |view, cx| {
            view.set_repository_catalog(catalog, active_root.as_deref(), false, cx)
        });
        match selected_root {
            Some(root) => self.refresh_agent_stats_repository_at(root, cx),
            None => {
                view.update(cx, |view, cx| {
                    view.set_repository(RepositoryStatsState::Empty, cx)
                });
            }
        }
    }

    /// Select a repository by the stable key exposed in the generic catalog and
    /// immediately restore/refresh exactly that scan input. Unknown or stale
    /// keys are ignored without changing the current selection.
    pub(crate) fn select_agent_stats_repository(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let view = self.ensure_agent_stats_view(cx);
        let root = view.update(cx, |view, cx| view.select_repository(key, cx));
        let Some(root) = root else {
            return false;
        };
        self.refresh_agent_stats_repository_at(root, cx);
        true
    }

    fn refresh_agent_stats_repository_at(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let view = self.ensure_agent_stats_view(cx);
        self.agent_stats_scan_generation = self.agent_stats_scan_generation.wrapping_add(1);
        let generation = self.agent_stats_scan_generation;
        let restored = self.telemetry_store.latest_repository(&root).cloned();
        view.update(cx, |view, cx| {
            view.begin_repository_refresh(root.clone(), restored, cx)
        });

        let requested_root = root.clone();
        cx.spawn(async move |this, cx| {
            let scan = cx
                .background_executor()
                .spawn(async move { scan_repository(&root) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.apply_agent_stats_repository_for(generation, &requested_root, scan, cx);
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
        let requested_root = self.agent_stats_view.as_ref().and_then(|view| {
            view.read(cx)
                .selected_repository()
                .map(|choice| choice.root.clone())
        });
        let Some(requested_root) = requested_root else {
            return false;
        };
        self.apply_agent_stats_repository_for(generation, &requested_root, scan, cx)
    }

    /// Strong completion seam: both the monotonic request generation and the
    /// normalized selected scan input must still match. This guards stale
    /// completions even when callers retain a request across repository changes.
    pub(crate) fn apply_agent_stats_repository_for(
        &mut self,
        generation: u64,
        requested_root: &Path,
        scan: RepositoryScan,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.agent_stats_scan_generation {
            return false;
        }
        let requested_key = repository_root_key(requested_root);
        let selection_matches = self.agent_stats_view.as_ref().is_some_and(|view| {
            view.read(cx)
                .selected_repository()
                .is_some_and(|choice| choice.key == requested_key)
        });
        if !selection_matches {
            return false;
        }
        let captured_at_unix_ms = unix_millis_now();
        let recorded = self
            .telemetry_store
            .record_repository(captured_at_unix_ms, &scan);
        if recorded {
            self.mark_telemetry_dirty(cx);
        }
        let Some(view) = self.agent_stats_view.clone() else {
            return true;
        };
        view.update(cx, |view, cx| {
            if recorded {
                view.mark_repository_analyzed(&requested_key, cx);
            }
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
        let repository_picker = view.read(cx).active_tab() == AgentStatsTab::Repository
            && view.read(cx).repository_picker_open();
        if repository_picker {
            match press.key {
                Key::Char('j') | Key::Down => {
                    view.update(cx, |view, cx| view.move_repository_picker(1, cx));
                    return;
                }
                Key::Char('k') | Key::Up => {
                    view.update(cx, |view, cx| view.move_repository_picker(-1, cx));
                    return;
                }
                Key::Enter => {
                    let key = view.update(cx, |view, cx| view.activate_repository_picker(cx));
                    if let Some(key) = key {
                        self.select_agent_stats_repository(&key, cx);
                    }
                    return;
                }
                Key::Esc => {
                    view.update(cx, |view, cx| view.close_repository_picker(cx));
                    return;
                }
                _ => {}
            }
        } else if view.read(cx).active_tab() == AgentStatsTab::Repository && press.key == Key::Enter
        {
            view.update(cx, |view, cx| view.toggle_repository_picker(cx));
            return;
        }
        match press.key {
            Key::Char('1') => {
                view.update(cx, |view, cx| view.select_tab(AgentStatsTab::Agents, cx));
            }
            Key::Char('2') => {
                view.update(cx, |view, cx| view.select_tab(AgentStatsTab::Inactive, cx));
            }
            Key::Char('3') => {
                view.update(cx, |view, cx| {
                    view.select_tab(AgentStatsTab::Repository, cx)
                });
            }
            Key::Char('h') | Key::Left => {
                let tab = match view.read(cx).active_tab() {
                    AgentStatsTab::Agents => AgentStatsTab::Repository,
                    AgentStatsTab::Inactive => AgentStatsTab::Agents,
                    AgentStatsTab::Repository => AgentStatsTab::Inactive,
                };
                view.update(cx, |view, cx| view.select_tab(tab, cx));
            }
            Key::Char('l') | Key::Right | Key::Tab => {
                let tab = match view.read(cx).active_tab() {
                    AgentStatsTab::Agents => AgentStatsTab::Inactive,
                    AgentStatsTab::Inactive => AgentStatsTab::Repository,
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

#[cfg(test)]
mod repository_catalog_tests {
    use super::*;

    fn scan(root: PathBuf) -> RepositoryScan {
        RepositoryScan::Ready(RepositorySnapshot {
            root,
            head: Some("test-head".into()),
            tracked_dirty: false,
            tracked_files: 0,
            source_files: 0,
            top_level: CountProjection {
                distinct: 0,
                items: Vec::new(),
            },
            extensions: CountProjection {
                distinct: 0,
                items: Vec::new(),
            },
            instruction_files: PathProjection {
                total: 0,
                items: Vec::new(),
            },
            workspace_manifests: PathProjection {
                total: 0,
                items: Vec::new(),
            },
            large_source_files: LargeFileProjection {
                source_files: 0,
                items: Vec::new(),
            },
            recent_churn: ChurnProjection {
                commit_limit: 500,
                commits_scanned: 0,
                distinct_paths: 0,
                items: Vec::new(),
            },
        })
    }

    #[test]
    fn catalog_contains_every_project_and_only_orphan_retained_roots() {
        let temp = tempfile::tempdir().unwrap();
        let yalda = temp.path().join("yalda");
        let fulcrum = temp.path().join("fulcrum");
        let monorepo = temp.path().join("monorepo");
        let nested = monorepo.join("services").join("api");
        let retained_only = temp.path().join("retained-only");
        for root in [&yalda, &fulcrum, &nested, &retained_only] {
            std::fs::create_dir_all(root).unwrap();
        }

        let mut projects = Projects::new();
        projects.create("Yalda".into(), yalda.clone()).unwrap();
        projects.create("Fulcrum".into(), fulcrum.clone()).unwrap();
        projects.create("API".into(), nested.clone()).unwrap();

        let mut telemetry = TelemetryStore::default();
        assert!(telemetry.record_repository(1, &scan(fulcrum.clone())));
        assert!(telemetry.record_repository(2, &scan(monorepo.clone())));
        assert!(telemetry.record_repository(3, &scan(retained_only.clone())));

        let catalog = agent_stats_repository_catalog(&projects, &telemetry);
        assert_eq!(catalog.iter().filter(|choice| choice.registered).count(), 3);
        assert_eq!(
            catalog.iter().filter(|choice| !choice.registered).count(),
            1
        );
        for (name, root) in [("Yalda", &yalda), ("Fulcrum", &fulcrum), ("API", &nested)] {
            let choice = catalog
                .iter()
                .find(|choice| choice.label == name)
                .expect("every registered project remains selectable");
            assert_eq!(&choice.root, root);
        }
        assert!(
            catalog
                .iter()
                .find(|choice| choice.label == "Fulcrum")
                .unwrap()
                .has_observation
        );
        assert!(
            catalog
                .iter()
                .find(|choice| choice.label == "API")
                .unwrap()
                .has_observation,
            "a nested project inherits its retained repository-root analysis"
        );
        let orphan = catalog.iter().find(|choice| !choice.registered).unwrap();
        assert_eq!(
            repository_root_key(&orphan.root),
            repository_root_key(&retained_only)
        );
        assert_eq!(orphan.label, "retained-only");
    }
}
