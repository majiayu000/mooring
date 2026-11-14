//! Another machine's work in the main area, as this machine's is in a tab: its terminals
//! docked where the panes go, worked from the start and sized to them, split from a
//! pane's right-click menu, and 「＋」's start page that offers its Agents as an empty tab
//! does. Both stay until the tab underneath, or the view, changes.
use super::peek::Peek;
use super::render::marked_button;
use super::*;
use std::collections::HashMap;

/// 「＋」 on a machine: where to start there, shown in place of the current tab.
pub(super) struct Start {
    pub(super) machine: usize,
    /// The space there to start in (picked in the sidebar); unset, that machine picks.
    pub(super) space: Option<fibo_remote::protocol::Space>,
    /// The tab this covers; showing another one drops it.
    pub(super) home: Option<String>,
}

/// Other machines' terminals in the main area, laid out as a tab's panes are. Lives only in
/// this window: each terminal is in a tab of its own over there.
pub(super) struct Dock {
    pub(super) tree: Layout,
    pub(super) panes: HashMap<String, Peek>,
    pub(super) focused: String,
    /// The tab this covers; showing another one drops it.
    pub(super) home: Option<String>,
    /// The right-clicked pane and where.
    pub(super) menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    /// 放大: this pane alone fills the main area.
    pub(super) zoomed: Option<String>,
    /// Where the terminal being opened goes: beside this pane, on this side.
    pub(super) split: Option<(String, Axis)>,
}

impl Workbench {
    pub(super) fn show_remote_start(
        &mut self,
        machine: usize,
        space: Option<fibo_remote::protocol::Space>,
        cx: &mut Context<Self>,
    ) {
        self.leave_for_dock(cx);
        self.peek = None;
        self.dock = None;
        self.remote_start = Some(Start {
            machine,
            space,
            home: self.nav.tab.clone(),
        });
        cx.notify();
    }
    /// The board and a todo's page give way, as they do when a local session opens.
    pub(super) fn leave_for_dock(&mut self, cx: &mut Context<Self>) {
        self.nav.task_detail = None;
        if self.nav.overview {
            self.leave_overview(cx);
        }
    }
    /// Another tab shown, or the board: what was docked over the old one goes.
    pub(super) fn drop_moved_dock(&mut self) {
        let away = |home: &Option<String>| self.nav.overview || home != &self.nav.tab;
        if self.dock.as_ref().is_some_and(|d| away(&d.home)) {
            self.dock = None;
        }
        if self.remote_start.as_ref().is_some_and(|s| away(&s.home)) {
            self.remote_start = None;
        }
    }
    pub(super) fn close_dock(&mut self, cx: &mut Context<Self>) {
        if self.remote_start.take().is_some() || self.dock.take().is_some() {
            self.nav.pending_focus = true;
        }
        self.close_peek(cx);
        cx.notify();
    }
    /// The floating peek or a docked pane showing `key`.
    pub(super) fn peek_by(&self, key: &str) -> Option<&Peek> {
        (self.peek.as_ref().filter(|p| p.terminal == key))
            .or_else(|| self.dock.as_ref()?.panes.get(key))
    }
    pub(super) fn peek_by_mut(&mut self, key: &str) -> Option<&mut Peek> {
        if self.peek.as_ref().is_some_and(|p| p.terminal == key) {
            return self.peek.as_mut();
        }
        self.dock.as_mut()?.panes.get_mut(key)
    }
    pub(super) fn dock_shows(&self, key: &str) -> bool {
        self.dock
            .as_ref()
            .is_some_and(|d| d.panes.contains_key(key))
    }
    /// A docked terminal goes beside the pane its split was asked from; anything else
    /// opened replaces what was docked, as opening a session elsewhere switches tabs.
    pub(super) fn dock_in(&mut self, peek: Peek) {
        let key = peek.terminal.clone();
        if let Some(dock) = &mut self.dock
            && let Some((target, axis)) = dock.split.take()
            && dock.tree.split(&target, key.clone(), axis)
        {
            dock.zoomed = None;
            dock.panes.insert(key.clone(), peek);
            dock.focused = key;
            return;
        }
        self.dock = Some(Dock {
            tree: Layout::Leaf(key.clone()),
            panes: HashMap::from([(key.clone(), peek)]),
            focused: key,
            home: self.nav.tab.clone(),
            menu: None,
            zoomed: None,
            split: None,
        });
    }
    pub(super) fn focus_dock_pane(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dock) = &mut self.dock
            && let Some(peek) = dock.panes.get(key)
        {
            dock.focused = key.to_owned();
            // A pane shown while another is zoomed brings the split back, as it must show.
            if dock.zoomed.as_deref().is_some_and(|z| z != key) {
                dock.zoomed = None;
            }
            window.focus(&peek.pane.focus_handle(cx), cx);
        }
        cx.notify();
    }
    /// ✕ on a docked pane: it leaves this window; its terminal keeps running there.
    pub(super) fn close_dock_pane(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(dock) = &mut self.dock else {
            return;
        };
        dock.panes.remove(key);
        if dock.zoomed.as_deref() == Some(key) {
            dock.zoomed = None;
        }
        match dock.tree.clone().without(key) {
            Some(tree) => {
                if dock.focused == key {
                    dock.focused = tree.ids().first().cloned().unwrap_or_default();
                }
                dock.tree = tree;
            }
            None => {
                self.dock = None;
                self.nav.pending_focus = true;
            }
        }
        cx.notify();
    }
    /// What the main area shows instead of the tab's panes, if anything is docked there.
    pub(super) fn docked_view(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        if let Some(dock) = &self.dock {
            let zoomed = dock.zoomed.clone().map(Layout::Leaf);
            return Some(self.dock_tree(dock, zoomed.as_ref().unwrap_or(&dock.tree), cx));
        }
        let start = self.remote_start.as_ref()?;
        Some(self.remote_start_view(start, cx))
    }
    /// The Agents another machine knows, the one to offer first, and its name.
    pub(super) fn machine_agents(&self, index: usize) -> (String, Vec<Agent>, Option<Agent>) {
        let (name, known) = match self.machines.get(index) {
            Some(remote) => {
                let view = remote.view.lock().unwrap();
                match &view.machine {
                    Some(m) => (m.name.clone(), m.agent_names.clone()),
                    None => (view.spec.host.clone(), Default::default()),
                }
            }
            None => return (String::new(), Vec::new(), None),
        };
        // An older machine that names none is offered them all.
        let agents: Vec<Agent> = Agent::all()
            .into_iter()
            .filter(|a| known.is_empty() || known.contains_key(&a.key().to_lowercase()))
            .collect();
        let first = Some(self.first_agent())
            .filter(|a| agents.contains(a))
            .or(agents.first().copied());
        (name, agents, first)
    }
    fn remote_start_view(&self, start: &Start, cx: &Context<Self>) -> gpui::AnyElement {
        let index = start.machine;
        let (name, agents, first) = self.machine_agents(index);
        if name.is_empty() {
            return div().into_any_element();
        }
        let mark = |agent: Agent, ink| super::board::agent_mark_in(agent, ink, T::SMALL);
        let run = |agent: Option<Agent>| {
            let space = start.space.clone();
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, window, cx| {
                this.open_remote_shell(index, space.clone(), None, agent, window, cx)
            })
        };
        let mut others: Vec<gpui::AnyElement> = agents
            .iter()
            .copied()
            .filter(|a| Some(*a) != first)
            .map(|agent| {
                marked_button(
                    format!("remote-start-{}", agent.executable()),
                    agent.name().to_owned(),
                    Some(mark(agent, T::color(T::link()))),
                )
                .px(px(T::S4))
                .text_color(T::color(T::link()))
                .on_click(run(Some(agent)))
                .into_any_element()
            })
            .collect();
        others.push(
            marked_button("remote-start-shell".into(), t!("空终端").into(), None)
                .px(px(T::S4))
                .text_color(T::color(T::link()))
                .on_click(run(None))
                .into_any_element(),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(T::S12))
            .child(
                div()
                    .text_size(px(T::CARD_TITLE))
                    .text_color(T::color(T::primary()))
                    .child(t!("在 {name} 上开始", name)),
            )
            // Which space there, and its folder when it has one.
            .when_some(start.space.clone(), |d, s| {
                let line = match s.cwd.is_empty() {
                    true => s.name,
                    false => format!("{} · {}", s.name, s.cwd),
                };
                d.child(label(line).text_color(T::color(T::muted())))
            })
            .when_some(first, |d, first| {
                d.child(
                    super::overview::solid(marked_button(
                        format!("remote-start-{}", first.executable()),
                        t!("运行 {name}", name = first.name()),
                        Some(mark(first, T::color(T::on_accent()))),
                    ))
                    .px(px(T::S16))
                    .on_click(run(Some(first))),
                )
            })
            .child(
                div()
                    .max_w(px(T::HERE_WIDTH * 1.5))
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .items_center()
                    .gap_x(px(T::S12))
                    .gap_y(px(T::S4))
                    .text_size(px(T::SMALL))
                    .children(others),
            )
            .child(
                button("remote-start-close", t!("取消"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_dock(cx))),
            )
            .into_any_element()
    }
}

#[cfg(feature = "visual-test")]
impl Workbench {
    pub fn visual_remote_start(&mut self, machine: usize, cx: &mut Context<Self>) {
        self.show_remote_start(machine, None, cx);
    }
    pub fn visual_remote_start_shown(&self) -> bool {
        self.remote_start.is_some()
    }
}
