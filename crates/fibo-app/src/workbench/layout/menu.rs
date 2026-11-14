//! The pane, tab and icon menus and what their items do.
use super::*;

impl Workbench {
    pub(in crate::workbench) fn menu_action(
        &mut self,
        id: &str,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.layout.menu.take();
        match action {
            tk!("向右分屏") | tk!("向下分屏") => {
                self.nav.zoomed = None;
                self.layout.split_target = Some((
                    id.into(),
                    if action == tk!("向右分屏") {
                        Axis::Right
                    } else {
                        Axis::Down
                    },
                ));
                self.launch(true, None, cx);
            }
            tk!("放大") | tk!("还原") => {
                self.nav.zoomed = if self.nav.zoomed.as_deref() == Some(id) {
                    None
                } else {
                    Some(id.into())
                };
                self.sync_panes(cx);
                if let Some(pane) = self.layout.panes.get(id) {
                    window.focus(&pane.focus_handle(cx), cx);
                }
            }
            tk!("关闭画面") => self.close_pane(id, cx),
            tk!("复制") => {
                if let Some(pane) = self.layout.panes.get(id) {
                    pane.update(cx, |p, cx| p.copy(cx));
                }
            }
            tk!("粘贴") => {
                if let Some(pane) = self.layout.panes.get(id) {
                    pane.update(cx, |p, cx| p.paste(cx));
                    window.focus(&pane.focus_handle(cx), cx);
                }
            }
            tk!("在编辑器里看滚动内容") => self.scrollback_in_editor(id, cx),
            tk!("搜索滚动内容") => {
                if let Some(pane) = self.layout.panes.get(id) {
                    pane.update(cx, |p, cx| p.open_search(window, cx));
                }
            }
            super::super::pane_menu::INTERRUPT_SHELL => {
                if let Some(pane) = self.layout.panes.get(id) {
                    pane.update(cx, |p, cx| p.interrupt(cx));
                }
            }
            super::super::pane_menu::INTERRUPT_TURN => self.interrupt_turn(id, cx),
            tk!("停止 Agent") => self.request_close(Closing::Agent(id.into()), cx),
            tk!("取消关联") => {
                if let Some(session) = self.current_agent(id) {
                    self.edit(
                        Edit::AssignTask {
                            id: session.id.clone(),
                            task: None,
                        },
                        cx,
                    );
                }
            }
            tk!("挂到待办…")
            | tk!("为这个待办开一个会话")
            | tk!("移到标签…")
            | tk!("为什么是这个状态") => {
                if let Some(mut menu) = menu {
                    menu.page = match action {
                        tk!("挂到待办…") => MenuPage::Assign,
                        tk!("移到标签…") => MenuPage::Move,
                        tk!("为什么是这个状态") => MenuPage::Why,
                        _ => MenuPage::Sessions,
                    };
                    self.layout.menu = Some(menu);
                }
            }
            _ => (),
        }
        cx.notify();
    }
    /// The right-clicked path's entries: open it, show it, copy it whole or relative.
    fn path_entries(
        &self,
        terminal: &str,
        target: &crate::terminal::pane::Hovered,
        cx: &Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        use crate::terminal::{open_path, pane::Pane};
        let relative = open_path::relative(&target.path, target.cwd.as_deref());
        let name = target
            .path
            .file_name()
            .map_or_else(|| target.path.to_string_lossy(), |n| n.to_string_lossy())
            .into_owned();
        let mut entries = vec![
            label(name)
                .h(px(T::ROW))
                .px(px(T::S10))
                .flex()
                .items_center()
                .truncate()
                .into_any_element(),
        ];
        let items: [(&str, String, bool); 4] = [
            ("open", Pane::open_title(target), true),
            ("reveal", crate::ui::words::local(t!("在访达中显示")), true),
            ("copy", t!("复制路径").into(), true),
            (
                "copy-relative",
                t!("复制相对路径").into(),
                relative.is_some(),
            ),
        ];
        for (key, title, enabled) in items {
            let terminal = terminal.to_owned();
            let target = target.clone();
            let relative = relative.clone();
            entries.push(
                button(gpui::SharedString::from(format!("menu-path-{key}")), title)
                    .w_full()
                    .justify_start()
                    .border_0()
                    .when(!enabled, |d| d.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !enabled {
                            return;
                        }
                        this.layout.menu = None;
                        if let Some(pane) = this.layout.panes.get(&terminal) {
                            pane.update(cx, |p, cx| match key {
                                "open" => p.open_target(&target, false, cx),
                                "reveal" => p.open_target(&target, true, cx),
                                "copy" => p.copy_note(
                                    target.path.to_string_lossy().into_owned(),
                                    t!("已复制路径"),
                                    cx,
                                ),
                                _ => p.copy_note(
                                    relative.clone().unwrap_or_default(),
                                    t!("已复制相对路径"),
                                    cx,
                                ),
                            });
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        entries.push(
            div()
                .h(px(T::S4))
                .border_b(px(T::BORDER_WIDTH))
                .border_color(T::color(T::border()))
                .mb(px(T::S4))
                .into_any_element(),
        );
        entries
    }
    /// The steps a pane's title once showed as buttons, now at the top of its menu:
    /// 标记完成 when it is due, 看进度, 放入待办 while the tab has a todo to hand over,
    /// and 待办详情.
    fn pane_entries(&self, terminal: &str, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        let session = self.pane_session(terminal);
        let status = self.pane_status(terminal).map(|(g, _)| g);
        let mut out = Vec::new();
        let entry = |key: &str, title: &'static str| {
            button(gpui::SharedString::from(format!("menu-{key}")), title)
                .w_full()
                .justify_start()
                .border_0()
        };
        if let Some(s) = session.filter(|s| {
            s.outcome == Outcome::Open
                && self.completing(terminal).is_none()
                && matches!(
                    status,
                    Some(Status::Review | Status::Background | Status::Paused)
                )
        }) {
            let (t, sid) = (terminal.to_owned(), s.id.clone());
            out.push(
                entry("accept", t!("标记完成"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.menu = None;
                        this.complete_pane(t.clone(), sid.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        if self.unbound_agent(terminal) && self.completing(terminal).is_none() {
            let t = terminal.to_owned();
            out.push(
                entry("accept-unbound", t!("标记完成"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.menu = None;
                        this.complete_unbound(t.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        if let Some(s) = session {
            let sid = s.id.clone();
            out.push(
                entry("here", t!("看进度"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.menu = None;
                        this.overview.here = Some(sid.clone());
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let pane = self.layout.panes.get(terminal);
        let tab_task = self
            .workspace
            .terminal_tab(terminal)
            .and_then(|t| t.default_task.as_ref())
            .and_then(|tid| self.workspace.tasks.iter().find(|t| &t.id == tid));
        if let Some(text) = tab_task
            .filter(|_| session.is_some())
            .filter(|_| pane.is_some_and(|p| p.read(cx).handoff_state().is_none()))
            .map(|t| self.task_handoff(t))
        {
            let target = terminal.to_owned();
            out.push(
                entry("place-task", t!("放入待办"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.menu = None;
                        if let Some(p) = this.layout.panes.get(&target) {
                            p.update(cx, |p, cx| p.place_text(text.clone(), cx));
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if let Some(task) = self.pane_task(terminal, session) {
            let tid = task.id.clone();
            out.push(
                entry("task-detail", t!("待办详情"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.menu = None;
                        this.nav.task_detail = Some(tid.clone());
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        out
    }
    pub(in crate::workbench) fn menu_view(
        &self,
        menu: &PaneMenu,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let session = self.current_agent(&menu.terminal);
        let task = self
            .workspace
            .terminal_tab(&menu.terminal)
            .and_then(|t| t.default_task.clone());
        let mut entries = Vec::new();
        match menu.page {
            MenuPage::Main => {
                if let Some(target) = &menu.path {
                    entries.extend(self.path_entries(&menu.terminal, target, cx));
                }
                let own = self.pane_entries(&menu.terminal, cx);
                if !own.is_empty() {
                    entries.extend(own);
                    entries.push(menu_rule(usize::MAX));
                }
                let keys = crate::keymap::current(cx);
                let zoomed = self.nav.zoomed.as_deref() == Some(&menu.terminal);
                let agent = session.is_some();
                for (i, row) in super::super::pane_menu::menu_rows(zoomed, agent)
                    .into_iter()
                    .enumerate()
                {
                    let Some(title) = row else {
                        entries.push(menu_rule(i));
                        continue;
                    };
                    // A greyed item says why on hover, so it doesn't read as broken.
                    let blocked = match title {
                        tk!("为这个待办开一个会话") => {
                            task.is_none().then_some(t!("这个标签没有设默认待办"))
                        }
                        tk!("挂到待办…") | tk!("停止 Agent") => session
                            .is_none()
                            .then_some(t!("这个终端里没有识别到 Agent")),
                        super::super::pane_menu::INTERRUPT_TURN => self.interrupt_blocked(session),
                        tk!("在编辑器里看滚动内容") => super::scrollback_blocked(),
                        tk!("取消关联") => match session {
                            None => Some(t!("这个终端里没有识别到 Agent")),
                            Some(s) if s.task.is_none() => Some(t!("这个会话没有挂到待办")),
                            Some(_) => None,
                        },
                        _ => None,
                    };
                    let shortcut = menu_shortcut(title, &keys);
                    let terminal = menu.terminal.clone();
                    let action = title.to_owned();
                    entries.push(
                        button(gpui::SharedString::from(format!("menu-{title}")), "")
                            .aria_label(fibo_core::i18n::say(title))
                            .w_full()
                            .justify_between()
                            .border_0()
                            // No row gap: `button` puts its (empty) text first, and a gap
                            // would indent these titles past the ones above the rule.
                            .child(
                                div()
                                    .flex_1()
                                    .when(blocked.is_some(), |d| d.opacity(0.4))
                                    .child(fibo_core::i18n::say(title)),
                            )
                            .when_some(shortcut, |d, keys| {
                                d.child(
                                    div()
                                        .ml(px(T::S16))
                                        .text_size(px(T::META))
                                        .text_color(T::color(T::muted()))
                                        .child(keys),
                                )
                            })
                            .when_some(blocked, |d, why| {
                                d.cursor_default().tooltip(move |_, cx| {
                                    cx.new(|_| super::render::TextTip(why.into())).into()
                                })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if blocked.is_none() {
                                    this.menu_action(&terminal, &action, window, cx);
                                }
                            }))
                            .into_any_element(),
                    );
                }
                // Plugin actions this pane offers, after the built-ins.
                let offered = self.actions_for(&menu.terminal);
                if !offered.is_empty() {
                    entries.push(
                        label(t!("插件"))
                            .h(px(T::ROW))
                            .px(px(T::S10))
                            .flex()
                            .items_center()
                            .border_t(px(T::BORDER_WIDTH))
                            .border_color(T::color(T::border()))
                            .into_any_element(),
                    );
                }
                for action in offered {
                    let terminal = menu.terminal.clone();
                    entries.push(
                        button(
                            gpui::SharedString::from(format!("menu-action-{}", action.id)),
                            action.title.clone(),
                        )
                        .w_full()
                        .justify_start()
                        .border_0()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.run_action(&terminal, action, cx);
                        }))
                        .into_any_element(),
                    );
                }
                // A program that took the mouse gets plain drags; say how to select instead.
                if self
                    .layout
                    .panes
                    .get(&menu.terminal)
                    .is_some_and(|p| p.read(cx).takes_mouse())
                {
                    entries.push(
                        label(t!("选中文字：按住 Shift 再拖动"))
                            .h(px(T::ROW))
                            .px(px(T::S10))
                            .flex()
                            .items_center()
                            .into_any_element(),
                    );
                }
            }
            MenuPage::Sessions => {
                for agent in Agent::all() {
                    let terminal = menu.terminal.clone();
                    let task = task.clone();
                    entries.push(
                        button(
                            gpui::SharedString::from(format!("new-{}", agent.executable())),
                            agent.name(),
                        )
                        .w_full()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.layout.menu = None;
                            this.tasks.chosen_agent = Some(agent);
                            this.layout.split_target = Some((terminal.clone(), Axis::Right));
                            this.launch(true, task.clone(), cx);
                        }))
                        .into_any_element(),
                    );
                }
            }
            MenuPage::Why => {
                let hints = &self.store.recognition().terminal_hints;
                for (i, line) in state_reasons(
                    &self.workspace,
                    &menu.terminal,
                    hints.get(&menu.terminal).map(String::as_str),
                    now(),
                )
                .into_iter()
                .enumerate()
                {
                    entries.push(
                        div()
                            .id(gpui::SharedString::from(format!("why-{i}")))
                            .w_full()
                            .px(px(T::S10))
                            .py(px(T::S4))
                            .text_size(px(if i == 0 { T::META } else { T::SMALL }))
                            .text_color(T::color(if i == 0 {
                                T::secondary()
                            } else {
                                T::primary()
                            }))
                            .child(line)
                            .into_any_element(),
                    );
                }
            }
            MenuPage::Move => {
                let own = self
                    .workspace
                    .terminal_tab(&menu.terminal)
                    .map(|t| t.id.clone());
                let targets: Vec<_> = self
                    .workspace
                    .tabs
                    .iter()
                    .filter(|t| t.space == self.nav.space && Some(&t.id) != own.as_ref())
                    .map(|t| (Some(t.id.clone()), self.tab_name(t, cx)))
                    .chain([(None, t!("新标签").to_owned())])
                    .collect();
                for (tab, name) in targets {
                    let terminal = menu.terminal.clone();
                    entries.push(
                        button(
                            gpui::SharedString::from(format!(
                                "move-to-{}",
                                tab.as_deref().unwrap_or("new")
                            )),
                            name,
                        )
                        .w_full()
                        .justify_start()
                        .border_0()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.layout.menu = None;
                            this.move_pane(&terminal, tab.as_deref(), cx);
                        }))
                        .into_any_element(),
                    );
                }
            }
            MenuPage::Assign => {
                if let Some(session) = session {
                    for task in &self.workspace.tasks {
                        let sid = session.id.clone();
                        let tid = task.id.clone();
                        entries.push(
                            button(
                                gpui::SharedString::from(format!("assign-{tid}")),
                                task.title.clone(),
                            )
                            .w_full()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.layout.menu = None;
                                this.edit(
                                    Edit::AssignTask {
                                        id: sid.clone(),
                                        task: Some(tid.clone()),
                                    },
                                    cx,
                                );
                            }))
                            .into_any_element(),
                        );
                    }
                }
            }
        }
        let position = menu.position;
        self.popup(position, entries, window, cx, |this| {
            this.layout.menu = None
        })
    }
    /// A tab's right-click menu.
    pub(in crate::workbench) fn tab_menu_view(
        &self,
        id: &str,
        position: gpui::Point<gpui::Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let rename = id.to_owned();
        let close = id.to_owned();
        let tabs = self.sibling_tabs(id);
        let at = tabs.iter().position(|t| t == id).unwrap_or(0);
        let mut entries: Vec<gpui::AnyElement> = [
            (tk!("向左移"), -1, at > 0),
            (tk!("向右移"), 1, at + 1 < tabs.len()),
        ]
        .into_iter()
        .map(|(name, step, enabled)| {
            let id = id.to_owned();
            button(
                gpui::SharedString::from(format!("tab-menu-{name}")),
                fibo_core::i18n::say(name),
            )
            .w_full()
            .justify_start()
            .border_0()
            .when(!enabled, |d| d.opacity(0.4))
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled {
                    this.layout.tab_menu = None;
                    this.step_tab(&id, step, cx);
                }
            }))
            .into_any_element()
        })
        .collect();
        entries.extend([
            button("tab-menu-rename", t!("重命名"))
                .w_full()
                .justify_start()
                .border_0()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.layout.tab_menu = None;
                    this.rename_tab(&rename, window, cx);
                }))
                .into_any_element(),
            button("tab-menu-close", t!("关闭标签"))
                .w_full()
                .justify_start()
                .border_0()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_tab(&close, cx);
                }))
                .into_any_element(),
        ]);
        self.popup(position, entries, window, cx, |this| {
            this.layout.tab_menu = None
        })
    }
    /// Ink ring or mascot: the title bar and the Dock tile change at once, and the
    /// choice is remembered for the next launch.
    pub(in crate::workbench) fn icon_menu_view(
        &self,
        position: gpui::Point<gpui::Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let current = cx.global::<Chosen>().icon;
        let entries = AppIcon::ALL
            .into_iter()
            .map(|icon| {
                div()
                    .id(gpui::SharedString::from(format!(
                        "app-icon-{}",
                        icon.label()
                    )))
                    .role(gpui::accesskit::Role::Button)
                    .aria_label(icon.label())
                    .h(px(T::ROW))
                    .px(px(T::S8))
                    .flex()
                    .items_center()
                    .gap(px(T::S8))
                    .rounded(px(T::CONTROL_RADIUS))
                    .text_size(px(T::SMALL))
                    .cursor_pointer()
                    .hover(|s| s.bg(T::color(T::button_hover())))
                    .child(gpui::img(icon.title_image()).size(px(T::LOGO)))
                    .child(div().flex_1().child(icon.label()))
                    .when(icon == current, |d| {
                        d.child(div().text_color(T::color(T::link())).child("✓"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.icon_menu = None;
                        choose_icon(icon, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        self.popup(position, entries, window, cx, |this| {
            this.layout.icon_menu = None
        })
    }
}
