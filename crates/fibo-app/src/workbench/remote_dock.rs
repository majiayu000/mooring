//! Drawing the docked terminals of other machines and their right-click menu: split a pane
//! right or down with an Agent or a shell in the same folder, or close it here.
use super::remote_tab::Dock;
use super::*;

impl Workbench {
    pub(super) fn dock_tree(
        &self,
        dock: &Dock,
        tree: &Layout,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        match tree {
            Layout::Leaf(key) => self.dock_leaf(dock, key, cx),
            Layout::Split {
                axis,
                first,
                second,
                ..
            } => {
                let half = |t: &Layout| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .child(self.dock_tree(dock, t, cx))
                };
                div()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .when(*axis == Axis::Down, |d| d.flex_col())
                    .gap(px(T::S4))
                    .child(half(first))
                    .child(half(second))
                    .into_any_element()
            }
        }
    }
    fn dock_leaf(&self, dock: &Dock, key: &str, cx: &Context<Self>) -> gpui::AnyElement {
        let Some(peek) = dock.panes.get(key) else {
            return div().into_any_element();
        };
        let several = dock.panes.len() > 1;
        let focused = several && dock.focused == key;
        let (left, right) = (key.to_owned(), key.to_owned());
        div()
            .id(gpui::SharedString::from(format!("dock-{key}")))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .overflow_hidden()
            .when(several, |d| {
                d.rounded(px(T::PANE_RADIUS))
                    .border(px(T::BORDER_WIDTH))
                    .border_color(T::color(if focused { T::control() } else { T::border() }))
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.focus_dock_pane(&left, window, cx)),
            )
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(dock) = &mut this.dock {
                        dock.menu = Some((right.clone(), event.position));
                        dock.focused = right.clone();
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(self.peek_view(peek, cx))
            .into_any_element()
    }
    /// The open right-click menu on a docked pane, over everything: the native pane menu's
    /// rows that mean something for another machine's terminal.
    pub(super) fn dock_menu(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let dock = self.dock.as_ref()?;
        let (key, at) = dock.menu.clone()?;
        let r = dock.panes.get(&key)?.remote.as_ref()?;
        let manages = r.slot.lock().unwrap().as_ref().is_some_and(|l| l.managing);
        let zoomed = dock.zoomed.as_deref() == Some(key.as_str());
        let keys = crate::keymap::current(cx);
        let mut entries = Vec::new();
        let rows = [
            manages.then_some(tk!("向右分屏")),
            manages.then_some(tk!("向下分屏")),
            Some(if zoomed { tk!("还原") } else { tk!("放大") }),
            None,
            Some(tk!("复制")),
            Some(tk!("粘贴")),
            None,
            Some(tk!("关闭画面")),
        ];
        for title in rows.into_iter() {
            let Some(title) = title else {
                if !entries.is_empty() {
                    entries.push(
                        div()
                            .h(px(T::ROW))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .w_full()
                                    .h(px(T::BORDER_WIDTH))
                                    .bg(T::color(T::border())),
                            )
                            .into_any_element(),
                    );
                }
                continue;
            };
            let key = key.clone();
            let shortcut = super::layout::menu_shortcut(title, &keys);
            entries.push(
                button(gpui::SharedString::from(format!("dock-menu-{title}")), "")
                    .aria_label(fibo_core::i18n::say(title))
                    .w_full()
                    .justify_between()
                    .border_0()
                    .child(div().flex_1().child(fibo_core::i18n::say(title)))
                    .when_some(shortcut, |d, keys| {
                        d.child(div().text_color(T::color(T::muted())).child(keys))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dock_menu_action(&key, title, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        Some(self.popup(at, entries, window, cx, |this| {
            if let Some(dock) = &mut this.dock {
                dock.menu = None;
            }
        }))
    }
    fn dock_menu_action(
        &mut self,
        key: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dock) = &mut self.dock else {
            return;
        };
        dock.menu = None;
        dock.focused = key.to_owned();
        match title {
            tk!("向右分屏") => self.split_dock(Axis::Right, window, cx),
            tk!("向下分屏") => self.split_dock(Axis::Down, window, cx),
            tk!("放大") | tk!("还原") => {
                dock.zoomed = (dock.zoomed.as_deref() != Some(key)).then(|| key.to_owned());
                self.focus_dock_pane(key, window, cx);
            }
            tk!("复制") | tk!("粘贴") => {
                if let Some(pane) = dock.panes.get(key).map(|p| p.pane.clone()) {
                    pane.update(cx, |p, cx| {
                        if title == tk!("复制") {
                            p.copy(cx)
                        } else {
                            p.paste(cx)
                        }
                    });
                    window.focus(&pane.focus_handle(cx), cx);
                }
            }
            tk!("关闭画面") => self.close_dock_pane(key, cx),
            _ => (),
        }
        cx.notify();
    }
    /// 向右分屏 / 向下分屏 on the focused docked pane, as on this machine's: an empty
    /// terminal beside it, in its folder, on the same machine.
    pub(super) fn split_dock(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = &mut self.dock else {
            return;
        };
        let focused = dock.focused.clone();
        dock.split = Some((focused.clone(), axis));
        let Some(r) = self.peek_by(&focused).and_then(|p| p.remote.as_ref()) else {
            return;
        };
        if !r.slot.lock().unwrap().as_ref().is_some_and(|l| l.managing) {
            return;
        }
        // Beside it: in its space there, in the folder it is in now.
        let (machine, space, cwd) = (
            r.machine,
            self.machine_card_space(r),
            self.machine_card_cwd(r),
        );
        self.open_remote_shell(machine, space, cwd, None, window, cx);
    }
}

#[cfg(feature = "visual-test")]
impl Workbench {
    /// The focused docked pane's right-click menu.
    pub fn visual_dock_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(dock) = &mut self.dock {
            let at = gpui::point(px(T::SIDEBAR * 2.), px(T::TOP * 4.));
            dock.menu = Some((dock.focused.clone(), at));
        }
        cx.notify();
    }
    /// The menu's 向右分屏 / 向下分屏 on the focused pane.
    pub fn visual_dock_split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self
            .dock
            .as_ref()
            .map(|d| d.focused.clone())
            .unwrap_or_default();
        self.dock_menu_action(
            &focused,
            if axis == Axis::Down {
                tk!("向下分屏")
            } else {
                tk!("向右分屏")
            },
            window,
            cx,
        );
    }
    /// The start page's solid button on `machine`: the Agent it offers first.
    pub fn visual_open_remote_agent(
        &mut self,
        machine: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (_, _, first) = self.machine_agents(machine);
        self.open_remote_shell(machine, None, None, first, window, cx);
    }
    /// The docked panes in layout order, each with whether it is worked from here.
    pub fn visual_dock_panes(&self, cx: &gpui::App) -> Vec<(String, bool)> {
        let Some(dock) = &self.dock else {
            return Vec::new();
        };
        (dock.tree.ids().into_iter())
            .filter_map(|k| Some((k.clone(), !dock.panes.get(&k)?.pane.read(cx).peek)))
            .collect()
    }
}
