//! Panes, their split layout, resizing and pane menus.
use super::*;

/// The most a terminal's text opened in an editor holds: the host's own read limit.
const SCROLLBACK_ROWS: usize = 10_000;
mod drag;
mod resize;
use drag::DropState;
use resize::SplitDrag;
mod changes_files;
mod changes_panel;
mod changes_rows;
mod changes_slots;
#[cfg(feature = "visual-test")]
pub use changes_slots::ChangesProbe;
mod confirm_close;
mod menu;
mod pane_view;
mod turn_bar;

/// The visible panes and what is open over them: menus, drags and pending closes.
pub(super) struct LayoutState {
    pub(super) panes: HashMap<String, Entity<Pane>>,
    pub(super) menu: Option<PaneMenu>,
    pub(super) split_target: Option<(String, Axis)>,
    /// A plugin action waiting for its new pane: the command and the directory to run it in.
    pub(super) pending_command: Option<(String, String)>,
    pub(super) drag: Option<SplitDrag>,
    pub(super) split_bounds: HashMap<Vec<bool>, gpui::Bounds<gpui::Pixels>>,
    /// Where a pane or tab being dragged would land.
    pub(super) drop: DropState,
    /// The tab strip's sideways scroll, read to tell which tabs are out of sight.
    pub(super) tab_scroll: gpui::ScrollHandle,
    /// The tab the strip last scrolled into view.
    pub(super) revealed_tab: Option<String>,
    /// A tab being renamed in place after a double-click.
    pub(super) renaming_tab: Option<(String, Entity<Field>, [gpui::Subscription; 2])>,
    /// A sidebar conversation being renamed in place after a double-click.
    pub(super) renaming_session: Option<(String, Entity<Field>, [gpui::Subscription; 2])>,
    /// Each pane's 看改动 panel, kept closed so reopening returns to the same place.
    pub(super) changes: HashMap<String, changes_panel::ChangesSlot>,
    /// The workspace revision `changes` was last checked against.
    pub(super) changes_seen: u64,
    /// A space renamed in place in the sidebar; it carries the space so a just-added
    /// one can be named before the host's reply lists it.
    pub(super) renaming_space: Option<(Space, Entity<Field>, [gpui::Subscription; 2])>,
    /// A space's right-click menu: the space and where it was opened.
    pub(super) space_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    /// The space menu was opened from the dense presets' space line: it lists the spaces
    /// to switch to and a new one above the shown space's own entries.
    pub(super) space_switch: bool,
    /// 删除空间 was pressed once in the open space menu; the second press removes it.
    pub(super) space_removing: bool,
    /// The space whose empty tab has 「换目录」 open.
    pub(super) choosing_dir: Option<String>,
    /// 先不管 pressed once on a space with something running: the menu says so and asks.
    pub(super) space_quieting: bool,
    /// A new space being chosen for another machine, from under its chips.
    pub(super) adding_machine_space: Option<super::sidebar_machines::AddingSpace>,
    /// A sidebar session row's right-click menu: the session and where it was opened.
    pub(super) session_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    /// The ✓ row unfolded in place: a session you marked complete opens nothing on a
    /// click, it shows what it ended with and offers 继续对话 and 撤销完成.
    pub(super) done_open: Option<String>,
    /// Session rows whose subagent list you opened (true) or closed (false); the rest
    /// follow the default, open on the session you are in.
    pub(super) subagents_toggled: HashMap<String, bool>,
    /// Session rows whose subagent list shows every subagent, not the first few.
    pub(super) subagents_all: HashSet<String>,
    /// A tab's right-click menu: the tab and where it was opened.
    pub(super) tab_menu: Option<(String, gpui::Point<gpui::Pixels>)>,
    /// The tab under the pointer: its turning mark stands still while × shows in its slot.
    pub(super) hovered_tab: Option<String>,
    /// The title-bar logo's menu for choosing the app icon, where it was opened.
    pub(super) icon_menu: Option<gpui::Point<gpui::Pixels>>,
    /// A close that would stop a running Agent, waiting for the user to confirm it.
    pub(super) confirm_close: Option<Closing>,
    /// The terminal whose turn 中断这一轮 is watching end.
    pub(super) interrupting: Option<String>,
    /// Interrupted terminals already reopened in place once; a failed try keeps the button.
    pub(super) reopened: HashSet<String>,
    /// Panes playing their 标记完成 run, each with the run's number so a late timer
    /// from an earlier, kept run does not close the pane.
    pub(super) completing: HashMap<String, super::complete::Run>,
    pub(super) completing_seq: u64,
    /// Where panes and sidebar rows were drawn, for the run's overlay to fly between.
    pub(super) spots: super::complete::SharedSpots,
}
impl Workbench {
    pub(super) fn layout_view(
        &self,
        tree: &Layout,
        path: Vec<bool>,
        borders: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        match tree {
            Layout::Leaf(id) => self.pane_view(id, borders, window, cx),
            Layout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let down = *axis == Axis::Down;
                let mut left = path.clone();
                left.push(false);
                let mut right = path.clone();
                right.push(true);
                let drag_path = path.clone();
                let entity = cx.entity();
                let bounds_path = path.clone();
                div()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .relative()
                    .flex()
                    .when(down, |d| d.flex_col())
                    .child(
                        gpui::canvas(
                            |_, _, _| (),
                            move |bounds, _, _, cx| {
                                entity.update(cx, |this, _| {
                                    this.layout.split_bounds.insert(bounds_path.clone(), bounds);
                                });
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .min_h_0()
                            .flex_basis(px(0.))
                            .flex_grow(*ratio as f32)
                            .when(down, |d| d.w_full())
                            .when(!down, |d| d.h_full())
                            .child(self.layout_view(first, left, true, window, cx)),
                    )
                    .child(
                        // The gutter is the gap between the panes' cards, with a handle
                        // on hover or while dragging.
                        div()
                            .id(gpui::SharedString::from(format!("divider-{path:?}")))
                            .group("divider")
                            .relative()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(down, |d| {
                                d.h(px(T::S8))
                                    .w_full()
                                    .cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .when(!down, |d| {
                                d.w(px(T::S8))
                                    .h_full()
                                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                            })
                            .child(
                                div()
                                    .rounded_full()
                                    .when(down, |d| d.h(px(T::S2)).w(px(T::S20 * 2.)))
                                    .when(!down, |d| d.w(px(T::S2)).h(px(T::S20 * 2.)))
                                    .when(
                                        self.layout
                                            .drag
                                            .as_ref()
                                            .is_some_and(|d| d.path == path && d.saving.is_none()),
                                        |d| d.bg(T::color(T::focus())),
                                    )
                                    .group_hover("divider", |d| d.bg(T::color(T::focus()))),
                            )
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    this.begin_resize(drag_path.clone(), event.position);
                                    this.layout.menu = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .min_h_0()
                            .flex_basis(px(0.))
                            .flex_grow((10000 - *ratio) as f32)
                            .when(down, |d| d.w_full())
                            .when(!down, |d| d.h_full())
                            .child(self.layout_view(second, right, true, window, cx)),
                    )
                    .into_any_element()
            }
        }
    }
    pub(super) fn resize_drag(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &self.layout.drag else {
            return;
        };
        if drag.saving.is_some() {
            return;
        }
        if self.nav.tab.as_ref() != Some(&drag.tab) {
            self.layout.drag = None;
            return;
        }
        let Some(Layout::Split { ratio, .. }) = drag.split(&mut self.workspace) else {
            return;
        };
        *ratio = drag.ratio_at(event.position);
        cx.notify();
    }
    pub(super) fn finish_drag(&mut self, cx: &mut Context<Self>) {
        self.layout.drop = DropState::default();
        let Some(drag) = &self.layout.drag else {
            return;
        };
        if drag.saving.is_some() {
            return;
        }
        let (path, tab, anchors) = (drag.path.clone(), drag.tab.clone(), drag.anchors.clone());
        let at = (self.workspace.tabs.iter_mut().find(|t| t.id == tab))
            .and_then(|t| t.layout.as_mut()?.at_mut(&path).cloned());
        if let Some(Layout::Split { axis, ratio, .. }) = at {
            let request = self.edit_named(
                Edit::ResizeDivider {
                    tab,
                    path,
                    axis,
                    anchors,
                    ratio,
                },
                cx,
            );
            self.layout.drag.as_mut().unwrap().saving = Some(request);
        } else {
            self.layout.drag = None;
        }
    }
    /// Moves a pane, process and all, beside the panes of another tab or into a tab of
    /// its own, and follows it there. Its old tab closes with its last pane.
    pub(super) fn move_pane(&mut self, terminal: &str, to: Option<&str>, cx: &mut Context<Self>) {
        let (id, edit) = match to {
            Some(to) => {
                let Some(target) = self.workspace.tab_terminals(to).last().cloned() else {
                    return;
                };
                let side = Some((Axis::Right, false));
                let terminal = terminal.to_owned();
                (
                    to.to_owned(),
                    Edit::MovePane {
                        terminal,
                        target,
                        side,
                    },
                )
            }
            None => {
                let tab = Tab {
                    id: super::id(),
                    space: self.nav.space.clone(),
                    default_task: None,
                    title: String::new(),
                    layout: Some(Layout::Leaf(terminal.to_owned())),
                };
                (tab.id.clone(), Edit::CreateTab { tab })
            }
        };
        self.nav.zoomed = None;
        self.nav.tab = Some(id);
        self.nav.focused = Some(terminal.to_owned());
        self.nav.pending_focus = true;
        self.edit(edit, cx);
    }
    /// The terminal's text, scrollback included, saved under the host's private runtime
    /// directory and opened with `$EDITOR` in a new tab. The host reads it,
    /// so a pane's own screen is never locked for it; one file per terminal, replaced.
    fn scrollback_in_editor(&mut self, id: &str, cx: &mut Context<Self>) {
        // The capture is written here, and the editor would open in a terminal there.
        if let Some(why) = scrollback_blocked() {
            self.error = Some(why.to_owned());
            cx.notify();
            return;
        }
        let socket = self.store.socket().to_owned();
        let terminal = id.to_owned();
        let cwd = self
            .workspace
            .terminal_tab(id)
            .and_then(|t| self.workspace.spaces.iter().find(|s| s.id == t.space))
            .map(|s| s.cwd.clone())
            .unwrap_or_default();
        let task = cx.background_spawn(async move {
            let Reply::Text(text) = rpc(
                &socket,
                &Request::ReadScreen {
                    id: terminal.clone(),
                    rows: SCROLLBACK_ROWS,
                },
            )?
            else {
                return Err(std::io::Error::other(t!("后台没有返回文字")));
            };
            let dir = socket
                .parent()
                .ok_or_else(|| std::io::Error::other(t!("运行目录未知")))?
                .join("scrollback");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(format!("{terminal}.txt"));
            let mut file = fibo_platform::fs::private_file()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)?;
            std::io::Write::write_all(&mut file, text.trim_end().as_bytes())?;
            Ok(path)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(path) => {
                    // Through sh so the user's $EDITOR applies whatever their shell is.
                    let command = format!(
                        "sh -c '${{EDITOR:-vi}} \"$1\"' sh {}",
                        shell_quote(&path.to_string_lossy())
                    );
                    this.layout.pending_command = Some((command, cwd));
                    this.launch(false, None, cx);
                }
                Err(e) => {
                    this.error = Some(t!("在编辑器里看滚动内容失败：{e}", e));
                    cx.notify();
                }
            })
        })
        .detach();
    }
    /// A floating menu at `position`; a click outside it runs `dismiss`.
    pub(super) fn popup(
        &self,
        position: gpui::Point<gpui::Pixels>,
        entries: Vec<gpui::AnyElement>,
        window: &Window,
        cx: &Context<Self>,
        dismiss: fn(&mut Self),
    ) -> gpui::AnyElement {
        let height = px(T::ROW * entries.len() as f32 + T::S12);
        let viewport = window.viewport_size();
        div()
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    dismiss(this);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("pane-menu")
                    .absolute()
                    .left(
                        position
                            .x
                            .min(viewport.width - px(T::MENU_WIDTH))
                            .max(px(0.)),
                    )
                    .top(position.y.min(viewport.height - height).max(px(0.)))
                    .w(px(T::MENU_WIDTH))
                    .p(px(T::S6))
                    .bg(T::color(T::raised()))
                    .border(px(T::BORDER_WIDTH))
                    .border_color(T::color(T::control()))
                    .rounded(px(T::CONTROL_RADIUS))
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(entries),
            )
            .into_any_element()
    }
}
/// Applies `icon` to the title bar and the Dock, and remembers it.
/// One look for the pane header's text actions: link-colored, filled only on hover.
pub(super) trait PaneAction {
    fn pane_action(self) -> Self;
}
impl PaneAction for gpui::Stateful<gpui::Div> {
    fn pane_action(self) -> Self {
        self.h(px(T::PANE_ACTION))
            .px(px(T::S6))
            .rounded(px(T::PANE_ACTION_RADIUS))
            .text_size(px(T::SMALL))
            .text_color(T::color(T::link()))
    }
}
/// The pane header's status: the group's wash, its ink and its dot.
/// The state word, and after it why the session needs you when a reasons rule knows:
/// 「等你回答 · 批准 WebSearch」. The phrase is already cut to the rules' width.
fn state_chip(group: Status, reason: Option<String>, mark: gpui::AnyElement) -> gpui::Div {
    let (_, ink, wash) = group.colors();
    div()
        .flex_shrink_0()
        .h(px(T::STATE_CHIP))
        .px(px(T::S6))
        .flex()
        .items_center()
        .gap(px(T::PULSE_GAP))
        .rounded(px(T::STATE_CHIP_RADIUS))
        .bg(wash)
        .text_color(ink)
        .text_size(px(T::META))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(mark)
        .child(group.label())
        .when_some(reason, |d, r| {
            d.child(div().opacity(0.6).child("·"))
                .child(div().font_weight(gpui::FontWeight::NORMAL).child(r))
        })
}
/// Lower is more urgent: what a tab's dot shows when its panes disagree.
pub(super) fn urgency(group: Status) -> u8 {
    match group {
        Status::Waiting => 0,
        Status::Error => 1,
        Status::Review => 2,
        Status::Running => 3,
        Status::Background => 4,
        Status::Paused => 5,
        Status::Deferred => 6,
        Status::Stale => 7,
        Status::Fresh => 8,
        Status::Closed => 9,
    }
}
/// `3:12`, or `1:03:12` past an hour.
/// A question as the host words it, `【范围】问题（a / b）`, down to `问题`.
pub(super) fn ask_prompt(line: &str) -> &str {
    let line = line.split_once('】').map_or(line, |(_, rest)| rest);
    line.rsplit_once('（').map_or(line, |(head, _)| head).trim()
}
/// `+128 −14`: lines a turn added in green and removed in red; a side that is zero is left
/// out.
pub(super) fn line_counts((added, removed): (u32, u32)) -> gpui::Div {
    div()
        .flex_shrink_0()
        .flex()
        .gap(px(T::S4))
        .font_features(T::tabular())
        .when(added > 0, |d| {
            d.child(div().text_color(Tone::Ok.ink()).child(format!("+{added}")))
        })
        .when(removed > 0, |d| {
            d.child(
                div()
                    .text_color(Tone::Danger.ink())
                    .child(format!("−{removed}")),
            )
        })
}
pub(super) fn turn_clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
pub(super) fn choose_icon(icon: AppIcon, cx: &mut gpui::App) {
    crate::desktop::set_dock_icon(icon.dock_png());
    let file = cx.global::<Chosen>().file.clone();
    if let Err(e) = icon.save(&file) {
        eprintln!("记住图标选择失败: {e}");
    }
    cx.set_global(Chosen { icon, file });
}
/// The rule between two groups of the pane menu.
pub(super) const MENU_RULE: f32 = T::S8 + T::BORDER_WIDTH;
fn menu_rule(i: usize) -> gpui::AnyElement {
    div()
        .id(gpui::SharedString::from(format!("menu-rule-{i}")))
        .w_full()
        .h(px(MENU_RULE))
        .flex()
        .items_center()
        .child(
            div()
                .w_full()
                .h(px(T::BORDER_WIDTH))
                .bg(T::color(T::border())),
        )
        .into_any_element()
}
/// The chord that runs a menu item without the menu, as the user has it bound; copy
/// and paste are the terminal's own fixed ⌘C and ⌘V.
/// Why 在编辑器里看滚动内容 is off: an attached machine's terminal opens the editor there,
/// where the file written here is not.
pub(super) fn scrollback_blocked() -> Option<&'static str> {
    crate::lifecycle::attached().then(|| t!("滚动内容存在这台电脑上，那台电脑上的编辑器打不开"))
}
pub(super) fn menu_shortcut(title: &str, keys: &crate::keymap::Keymap) -> Option<String> {
    use crate::keymap::Action;
    let action = match title {
        tk!("复制") => return Some(crate::ui::words::local("⌘C")),
        tk!("粘贴") => return Some(crate::ui::words::local("⌘V")),
        tk!("向右分屏") => Action::SplitRight,
        tk!("向下分屏") => Action::SplitDown,
        tk!("搜索滚动内容") => Action::Find,
        tk!("关闭画面") => Action::Close,
        _ => return None,
    };
    keys.chords(action).first().map(|c| c.display())
}

#[cfg(test)]
mod tests {
    #[test]
    fn turn_clock_reads_minutes_and_hours() {
        assert_eq!(super::turn_clock(0), "0:00");
        assert_eq!(super::turn_clock(192), "3:12");
        assert_eq!(super::turn_clock(3792), "1:03:12");
    }
}
