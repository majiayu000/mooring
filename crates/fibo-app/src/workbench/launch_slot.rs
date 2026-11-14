//! Starting a terminal in a new tab, a split or a lost pane's place.
use super::*;

impl Workbench {
    pub(super) fn launch_in_slot(
        &mut self,
        mut split: bool,
        task_id: Option<String>,
        replace: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.store.admit_launch(split, &task_id, &replace) {
            return;
        }
        let previous = replace
            .as_ref()
            .and_then(|id| self.workspace.terminals.iter().find(|t| &t.id == id))
            .cloned();
        if replace.is_some()
            && !previous
                .as_ref()
                .is_some_and(|t| t.state == ProcessState::Interrupted)
        {
            return;
        }
        if !split
            && let Some(task_id) = &task_id
            && let Some(tab) = self
                .workspace
                .tabs
                .iter()
                .find(|t| t.default_task.as_ref() == Some(task_id))
        {
            self.nav.tab = Some(tab.id.clone());
            self.sync_panes(cx);
            split = true;
        }
        let source = self.layout.split_target.take().or_else(|| {
            if !split {
                return None;
            }
            self.nav
                .focused
                .clone()
                .filter(|id| {
                    self.workspace
                        .terminal_tab(id)
                        .is_some_and(|t| Some(&t.id) == self.nav.tab.as_ref())
                })
                .or_else(|| {
                    self.nav
                        .tab
                        .as_ref()
                        .and_then(|id| self.workspace.tab_terminals(id).first().cloned())
                })
                .map(|id| (id, Axis::Right))
        });
        let Some(space) = self
            .workspace
            .spaces
            .iter()
            .find(|s| {
                s.id == previous
                    .as_ref()
                    .map(|t| &t.space)
                    .unwrap_or(&self.nav.space)
                    .as_str()
            })
            .cloned()
        else {
            return;
        };
        let agent = self.tasks.chosen_agent.take();
        let resume = self.resuming.resume.take();
        let new_work = replace.is_none() && resume.is_none();
        // A plugin action's command and the pane it ran for; it runs like a resume does.
        let pending = self.layout.pending_command.take();
        let resuming = resume.as_ref().map(|r| r.2.clone());
        // A resume is in the launch itself; other commands are typed into the new shell, and
        // task text only ever reaches the Agent's input.
        let start = if resume.is_some() {
            None
        } else {
            let task = task_id
                .as_ref()
                .and_then(|id| self.workspace.tasks.iter().find(|t| &t.id == id));
            start_command(agent, task, pending.as_ref().map(|(c, _)| c.as_str())).map(
                |(command, text)| {
                    let handoff = task.map(|t| self.task_handoff(t));
                    (command, handoff.or(text))
                },
            )
        };
        let argv = launch_argv(
            crate::lifecycle::shell(),
            resume.as_ref().map(|r| r.0.as_str()),
        );
        // A new tab may move to the space of the project it opens in; a split or a
        // relaunch stays in its tab's space.
        let new_tab = !split && previous.is_none();
        let mut tab = if split {
            let Some(tab) = self
                .workspace
                .tabs
                .iter()
                .find(|t| Some(&t.id) == self.nav.tab.as_ref())
                .cloned()
            else {
                return;
            };
            tab
        } else {
            let title = String::new();
            Tab {
                id: id(),
                space: space.id.clone(),
                default_task: task_id.clone(),
                title,
                layout: None,
            }
        };
        let terminal = id();
        // Where the pane goes, as one operation the host applies to its layout then.
        let place = if let Some(old) = replace {
            if !tab
                .layout
                .as_mut()
                .is_some_and(|l| l.replace(&old, &terminal))
            {
                return;
            }
            Edit::ReplacePane {
                old,
                new: terminal.clone(),
            }
        } else if let Some(tree) = &mut tab.layout {
            let Some((target, axis)) = source.clone() else {
                return;
            };
            if !tree.split(&target, terminal.clone(), axis) {
                return;
            }
            let (terminal, before, share, forced) = (terminal.clone(), false, 5000, true);
            Edit::SplitPane {
                target,
                terminal,
                axis,
                before,
                share,
                forced,
            }
        } else {
            tab.layout = Some(Layout::Leaf(terminal.clone()));
            Edit::CreateTab { tab: tab.clone() }
        };
        let task_cwd = task_id
            .as_ref()
            .and_then(|id| self.workspace.tasks.iter().find(|t| &t.id == id))
            .and_then(|t| self.workspace.spaces.iter().find(|s| s.id == t.space))
            .map(|s| s.cwd.clone());
        // A plain new tab follows the focused pane of the current tab, like a split does.
        let inherit = source.as_ref().map(|(id, _)| id.clone()).or_else(|| {
            let tab = self.nav.tab.as_ref().filter(|_| previous.is_none())?;
            let panes = self.workspace.tab_terminals(tab);
            self.nav
                .focused
                .clone()
                .filter(|id| panes.contains(id))
                .or_else(|| panes.first().cloned())
        });
        let inherited = inherit
            .as_ref()
            .and_then(|id| self.workspace.terminals.iter().find(|t| &t.id == id))
            .filter(|t| t.space == space.id);
        // The space's own folder, else where its terminals last were, else the folder
        // worked in last anywhere (core decides).
        let home = crate::lifecycle::home_str();
        let space_dir = self.workspace.space_start(&space.id, home.as_deref());
        let cwd = resume
            .map(|r| r.1)
            .or(task_cwd)
            .or(pending.map(|(_, cwd)| cwd))
            .unwrap_or_else(|| {
                previous
                    .map(|t| t.cwd)
                    .or_else(|| inherited.map(|t| t.cwd.clone()))
                    .unwrap_or_else(|| space_dir.clone())
            });
        let cwd = usable_cwd([cwd, space_dir]);
        let cwd_source = inherited
            .filter(|t| t.state == ProcessState::Running)
            .filter(|_| task_id.is_none() && resuming.is_none())
            .map(|t| t.id.clone());
        let request = Request::Launch {
            id: terminal.clone(),
            space: space.id,
            argv,
            cwd,
            cols: 100,
            rows: 30,
            scheme: crate::ui::theme::Theme::scheme(),
            colors: crate::lifecycle::host_colors(),
        };
        let tab_id = tab.id.clone();
        // New work brings the space back. Showing an interrupted pane or a closed conversation
        // restores it by itself, which is only looking until you type.
        if new_work {
            self.work_began_in_space(&tab.space, cx);
        }
        self.begin_busy(None, cx);
        let spaces = new_tab.then(|| self.workspace.spaces.clone());
        let job = cx.background_spawn(self.store.launch(cwd_source, spaces, request, place));
        cx.spawn(async move |this, cx| {
            let reply = job.await;
            let _ = this.update(cx, |this, cx| {
                this.end_busy();
                match reply {
                    Ok(Reply::Workspace(w)) => {
                        this.apply_workspace(w, cx);
                        if let Some(session) = resuming {
                            this.resuming.resumed.insert(session, terminal.clone());
                        }
                        if let Some(tab) = this.workspace.tabs.iter().find(|t| t.id == tab_id) {
                            this.nav.space = tab.space.clone();
                        }
                        this.nav.tab = Some(tab_id);
                        this.nav.zoomed = None;
                        this.layout.menu = None;
                        this.nav.focused = Some(terminal.clone());
                        this.nav.pending_focus = true;
                        this.nav.overview = false;
                        this.sync_panes(cx);
                        if let (Some((command, text)), Some(pane)) =
                            (&start, this.layout.panes.get(&terminal))
                        {
                            pane.update(cx, |pane, cx| pane.start_agent(command, text.clone(), cx));
                        }
                        if this.store.behind(this.workspace.revision) {
                            this.sync(cx);
                        }
                    }
                    Ok(_) => this.error = Some(t!("缺少工作台响应").into()),
                    Err(e) => this.error = Some(e.to_string()),
                }
                this.run_waiting(cx);
            });
        })
        .detach();
    }
}
