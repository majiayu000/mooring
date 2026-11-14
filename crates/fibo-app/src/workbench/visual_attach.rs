//! What the attach evidence drives: the window's own keymap actions, as the keys reach them.
use super::*;

impl Workbench {
    /// Runs keymap action `id` (`new_tab`, `split_right`, `split_down`, `close`) as its
    /// shortcut does; whether it ran.
    pub fn visual_shortcut(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::keymap::Action;
        let action = match id {
            "new_tab" => Action::NewTab,
            "split_right" => Action::SplitRight,
            "split_down" => Action::SplitDown,
            "close" => Action::Close,
            _ => return false,
        };
        self.shortcut(action, window, cx)
    }
    /// 运行 <first Agent> on the empty tab: a new terminal with the Agent started in it.
    pub fn visual_run_first_agent(&mut self, cx: &mut Context<Self>) -> String {
        let agent = self.first_agent();
        self.run_agent_here(agent, cx);
        agent.name().to_owned()
    }
    pub fn visual_tab_ids(&self) -> Vec<String> {
        self.workspace.tabs.iter().map(|t| t.id.clone()).collect()
    }
    /// Closes tab `id` on the host, its terminals with it, with no question: the
    /// evidence leaves the attached machine as it found it.
    pub fn visual_close_tab_id(&mut self, id: &str, cx: &mut Context<Self>) {
        let edit = Edit::CloseTab {
            id: id.to_owned(),
            expect: None,
        };
        self.request(
            Request::Edit {
                request: None,
                edit,
            },
            cx,
        );
    }
    /// What the window shows as its last error, and whether a launch waits on a busy host.
    pub fn visual_attach_error(&self) -> String {
        format!("error {:?}, busy {}", self.error, self.store.busy())
    }
    /// Spaces, tabs and the shown tab's panes as this window holds them.
    pub fn visual_attach_state(&self) -> (usize, usize, usize) {
        let panes = (self.nav.tab.as_ref()).map_or(0, |t| self.workspace.tab_terminals(t).len());
        (
            self.workspace.spaces.len(),
            self.workspace.tabs.len(),
            panes,
        )
    }
}
