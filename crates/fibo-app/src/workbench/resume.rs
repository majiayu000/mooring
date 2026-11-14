//! Picking a conversation back up without leaving a nameless tab behind or running it twice.
use super::*;

/// The first shown pane whose terminal the host lost and that has not been tried yet.
pub(super) fn reopen_candidate(
    w: &Workspace,
    shown: &[String],
    tried: &HashSet<String>,
) -> Option<String> {
    shown
        .iter()
        .find(|id| interrupted(w, id) && !tried.contains(*id))
        .cloned()
}
impl Workbench {
    pub(super) fn open_session(
        &mut self,
        session: &AgentSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.hide();
        self.nav.task_detail = None;
        self.overview.here = None;
        self.nav.zoomed = None;
        let Some(terminal) = live_terminal(&self.workspace, session)
            .or_else(|| resume_terminal(&self.workspace, &self.resuming.resumed, session))
        else {
            // Its old tab comes back as it shows (`sync_panes`); a click retries a failed try.
            if let Some(lost) = interrupted_pane(&self.workspace, &session.id) {
                let lost = lost.id.clone();
                // Showing a closed conversation's pane removes it; this click asked for it.
                if lost_session_settled(&self.workspace, &lost, now()) {
                    self.layout.reopened.insert(lost.clone());
                    self.focus_terminal(&lost, window, cx);
                    self.reopen(lost, cx);
                    return;
                }
                self.layout.reopened.remove(&lost);
                self.focus_terminal(&lost, window, cx);
                return;
            }
            if self.resume_unknown_here(session, cx) {
                return;
            }
            if let Some(space) = self.workspace.session_space(session) {
                self.nav.space = space.into();
            }
            self.resuming.resume = Some((
                self.workspace.resume_command(session),
                session.cwd.clone(),
                session.id.clone(),
            ));
            self.launch(false, session.task.clone(), cx);
            return;
        };
        let terminal_id = terminal.id.clone();
        if !self.focus_terminal(&terminal_id, window, cx) {
            let tab = Tab {
                id: id(),
                space: self.nav.space.clone(),
                default_task: session.task.clone(),
                title: String::new(),
                layout: Some(Layout::Leaf(terminal_id.clone())),
            };
            self.nav.tab = Some(tab.id.clone());
            self.nav.overview = false;
            self.edit(Edit::CreateTab { tab }, cx);
        }
    }
    /// What showing a lost pane does before it comes back by itself: true when the pane
    /// went away, because its conversation runs elsewhere or you already closed it.
    pub(super) fn drop_lost_pane(&mut self, lost: &str, cx: &mut Context<Self>) -> bool {
        if self.drop_pane_running_elsewhere(lost, cx) {
            return true;
        }
        if !lost_session_settled(&self.workspace, lost, now()) {
            return false;
        }
        self.layout.reopened.insert(lost.to_owned());
        self.close_now(Closing::Pane(lost.to_owned()), cx);
        self.sync_panes(cx);
        true
    }
    /// A lost pane whose conversation already runs elsewhere goes away instead of starting
    /// it again; alone in the shown tab, the view moves to where it runs. True when it did.
    fn drop_pane_running_elsewhere(&mut self, lost: &str, cx: &mut Context<Self>) -> bool {
        let Some(running) = lost_session_runs_in(&self.workspace, &self.resuming.resumed, lost)
            .map(|t| t.id.clone())
        else {
            return false;
        };
        self.layout.reopened.insert(lost.to_owned());
        let alone = self.workspace.terminal_tab(lost).is_some_and(|t| {
            Some(&t.id) == self.nav.tab.as_ref() && self.workspace.tab_terminals(&t.id).len() == 1
        });
        if alone && let Some(tab) = self.workspace.terminal_tab(&running) {
            (self.nav.space, self.nav.tab) = (tab.space.clone(), Some(tab.id.clone()));
            self.nav.focused = Some(running);
            self.nav.pending_focus = true;
        }
        self.close_now(Closing::Pane(lost.to_owned()), cx);
        self.sync_panes(cx);
        true
    }
    /// An attached machine's session whose Agent only that machine has installed: its
    /// resume command is in that machine's plugin, not here, and guessing would start a new
    /// conversation instead. Says so, and true when nothing should launch.
    fn resume_unknown_here(&mut self, session: &AgentSession, cx: &mut Context<Self>) -> bool {
        if !crate::lifecycle::attached() || session.runtime.spec().is_some() {
            return false;
        }
        self.error = Some(t!(
            "{agent} 只装在 {host} 上，这里还接不上那段对话；请在那台的窗口里接着",
            agent = session.runtime.name(),
            host = crate::lifecycle::here()
        ));
        cx.notify();
        true
    }
    /// Reopens a lost terminal in place, resuming the conversation it was running, even one
    /// you closed: this is you asking (the pane's button, its row), or a still open one.
    pub(super) fn reopen(&mut self, id: String, cx: &mut Context<Self>) {
        if self.drop_pane_running_elsewhere(&id, cx) {
            return;
        }
        if let Some(s) = interrupted_session(&self.workspace, &id).cloned() {
            if self.resume_unknown_here(&s, cx) {
                return;
            }
            let s = &s;
            self.resuming.resume = Some((
                self.workspace.resume_command(s),
                s.cwd.clone(),
                s.id.clone(),
            ));
        }
        self.launch_in_slot(true, None, Some(id), cx);
        // Taken unless the launch bailed early; a leftover would hijack the next launch.
        self.resuming.resume = None;
    }
    /// Opens a conversation by its CLI id, as clicking its sidebar row does.
    #[cfg(feature = "visual-test")]
    pub fn visual_open_cli_session(
        &mut self,
        cli_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let found = self.workspace.agent_sessions.iter();
        let session = found
            .into_iter()
            .find(|s| s.cli_session_id == cli_id)
            .expect("session")
            .clone();
        self.open_session(&session, window, cx);
    }
    /// Whether this window sees `lost` as lost, `tab` no longer holding `reopened`, and no
    /// launch running.
    #[cfg(feature = "visual-test")]
    pub fn visual_panes_lost(&self, lost: &[&str], tab: &str, reopened: &str) -> bool {
        lost.iter().all(|id| interrupted(&self.workspace, id))
            && !self.store.busy()
            && !self
                .workspace
                .tab_terminals(tab)
                .iter()
                .any(|t| t == reopened)
    }
    #[cfg(feature = "visual-test")]
    pub fn visual_revision(&self) -> u64 {
        self.workspace.revision
    }
    /// The shown tab's id.
    #[cfg(feature = "visual-test")]
    pub fn visual_shown_tab(&self) -> Option<String> {
        self.nav.tab.clone()
    }
}
