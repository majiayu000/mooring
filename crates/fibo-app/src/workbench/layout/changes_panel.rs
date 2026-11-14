//! 看改动: this turn's net changes, read only, over part of the terminal. It covers the
//! terminal rather than pushing it down, so the program under it is not resized, and leaves
//! the Agent's last words and the input line showing, so you can answer as you read.
//!
//! It is a cached view of its own and lays out only the rows on screen: the window redraws
//! for any pane's output, and this should cost nothing when it has not changed. Opening asks
//! the host for the list of files; choosing one asks for that file. So however many files a
//! turn touched, only the one on screen costs anything, and none of it is ever in the
//! workspace snapshot.
use super::changes_rows::{ROW_HEIGHT, Row, empty_words, matching, row_element, rows_of, warning};
use super::*;
use crate::terminal::pane::{Hovered, Pane};
use crate::ui::field::Field;
use crate::ui::tone::Tone;
use fibo_core::turn_diff::{FileSummary, LineKind, TurnChanges, TurnFile};
use gpui::{ScrollStrategy, UniformListScrollHandle};
use std::collections::VecDeque;
use std::sync::Arc;

/// How much of the terminal it covers.
pub(super) const COVERS: f32 = 0.56;
/// Past this many files a list needs a way to find one.
const FILTER_FROM: usize = 20;
/// Files kept read, with their place; going back to an older one reads it again.
const KEEP: usize = 32;
/// An arrow key's scroll.
const STEP: f32 = 60.;
/// A read shorter than this shows no 「正在读取」, which would only flash.
const READING_AFTER: Duration = Duration::from_millis(150);

pub(super) enum Load {
    Reading { say: bool },
    Failed,
    Ready(Arc<TurnChanges>),
}

/// One file as read: its own scroll stays with it.
pub(super) struct Shown {
    pub(super) file: Arc<TurnFile>,
    pub(super) rows: Arc<Vec<Row>>,
    pub(super) list: gpui::ListState,
}

pub(super) enum Detail {
    Reading { say: bool },
    Failed,
    Ready(Shown),
}

/// One pane's panel, kept when closed so reopening returns to the same file and place.
pub(in crate::workbench) struct ChangesSlot {
    /// The turn it shows: another means it is out of date.
    pub(super) key: (u64, u64),
    pub(super) open: bool,
    /// It covers the lower part of the terminal rather than the upper.
    pub(super) below: bool,
    pub(super) view: Entity<ChangesPanel>,
}

pub(in crate::workbench) struct ChangesPanel {
    pub(super) workbench: gpui::WeakEntity<Workbench>,
    pub(super) terminal: String,
    pub(super) session: String,
    pub(super) pane: Entity<Pane>,
    pub(super) focus: gpui::FocusHandle,
    pub(super) load: Load,
    /// An index into the files, whatever the filter hides.
    pub(super) selected: usize,
    pub(super) details: HashMap<usize, Detail>,
    pub(super) recent: VecDeque<usize>,
    /// The files the filter leaves, in order.
    pub(super) matches: Arc<Vec<usize>>,
    pub(super) filter: Option<Entity<Field>>,
    pub(super) filtered_by: String,
    pub(super) file_scroll: UniformListScrollHandle,
    pub(super) watching: Vec<gpui::Subscription>,
    #[cfg(feature = "visual-test")]
    pub(super) drew: u32,
}

impl ChangesPanel {
    pub(super) fn new(
        workbench: gpui::WeakEntity<Workbench>,
        terminal: String,
        session: String,
        pane: Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workbench,
            terminal,
            session,
            pane,
            focus: cx.focus_handle(),
            load: Load::Reading { say: false },
            selected: 0,
            details: HashMap::new(),
            recent: VecDeque::new(),
            matches: Arc::new(Vec::new()),
            filter: None,
            filtered_by: String::new(),
            file_scroll: UniformListScrollHandle::new(),
            watching: Vec::new(),
            #[cfg(feature = "visual-test")]
            drew: 0,
        }
    }

    pub(super) fn changes(&self) -> Option<&Arc<TurnChanges>> {
        match &self.load {
            Load::Ready(c) => Some(c),
            _ => None,
        }
    }

    /// Asks again, after a failure.
    fn read_again(&mut self, cx: &mut Context<Self>) {
        let request = Request::TurnChanges {
            session: self.session.clone(),
        };
        let asked = self.workbench.read_with(cx, |w, _| w.store.send(request));
        if let Ok(asked) = asked {
            self.read(asked, cx);
        }
    }

    /// `asked` is the request to the host, made by whoever can reach the store from where
    /// they are: the workbench is mid-update when it opens a panel.
    pub(super) fn read(
        &mut self,
        asked: impl std::future::Future<Output = std::io::Result<Reply>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.load = Load::Reading { say: false };
        let read = cx.background_spawn(asked);
        cx.spawn(async move |this, cx| {
            let reply = read.await;
            this.update(cx, |this, cx| this.loaded(reply, cx)).ok();
        })
        .detach();
        let wait = cx.background_executor().timer(READING_AFTER);
        cx.spawn(async move |this, cx| {
            wait.await;
            this.update(cx, |this, cx| {
                if let Load::Reading { say } = &mut this.load {
                    *say = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn loaded(&mut self, reply: std::io::Result<Reply>, cx: &mut Context<Self>) {
        let Ok(Reply::TurnChanges(changes)) = reply else {
            self.load = Load::Failed;
            cx.notify();
            return;
        };
        self.matches = Arc::new((0..changes.files.len()).collect());
        let many = changes.files.len() > FILTER_FROM;
        self.load = Load::Ready(Arc::new(changes));
        if many {
            self.make_filter(cx);
        }
        self.show(0, cx);
    }

    fn make_filter(&mut self, cx: &mut Context<Self>) {
        let field = cx.new(|cx| Field::new(t!("过滤文件"), "", cx).inline());
        self.watching = vec![cx.observe(&field, |this, _, cx| this.filter_changed(cx))];
        self.filter = Some(field);
    }

    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.filter else { return };
        let text = field.read(cx).text.clone();
        if text == self.filtered_by {
            return;
        }
        let Some(changes) = self.changes().cloned() else {
            return;
        };
        self.matches = Arc::new(matching(
            changes.files.iter().map(|f| f.path.as_str()),
            &text,
        ));
        self.filtered_by = text;
        self.file_scroll.scroll_to_item(0, ScrollStrategy::Top);
        // What is on the right stays when nothing matches: it is still a file of this turn.
        let first = self.matches.first().copied();
        match first.filter(|_| !self.matches.contains(&self.selected)) {
            Some(first) => self.show(first, cx),
            None => cx.notify(),
        }
    }

    /// Shows a file, asking for it unless it is already read.
    pub(super) fn show(&mut self, file: usize, cx: &mut Context<Self>) {
        self.selected = file;
        if let Some(at) = self.matches.iter().position(|m| *m == file) {
            self.file_scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        }
        let Some(summary) = self.changes().and_then(|c| c.files.get(file)).cloned() else {
            cx.notify();
            return;
        };
        let known = matches!(
            self.details.get(&file),
            Some(Detail::Ready(_) | Detail::Reading { .. })
        );
        if summary.edits > 0 && !known {
            self.read_file(file, summary, cx);
        }
        self.recent.retain(|f| *f != file);
        self.recent.push_back(file);
        while self.recent.len() > KEEP {
            if let Some(oldest) = self.recent.pop_front() {
                self.details.remove(&oldest);
            }
        }
        cx.notify();
    }

    fn read_file(&mut self, file: usize, summary: FileSummary, cx: &mut Context<Self>) {
        let Some(log) = self.changes().map(|c| c.log) else {
            return;
        };
        let request = Request::TurnFile {
            session: self.session.clone(),
            log,
            path: summary.path,
            lines: summary.lines,
        };
        let Ok(asked) = self.workbench.read_with(cx, |w, _| w.store.send(request)) else {
            return;
        };
        self.details.insert(file, Detail::Reading { say: false });
        // Building the rows copies every line, so that happens away from the window too.
        let read = cx.background_spawn(async move {
            match asked.await {
                Ok(Reply::TurnFile(file)) => {
                    let rows = rows_of(&file);
                    Some((file, rows))
                }
                _ => None,
            }
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                // It may have been let go while being read.
                if !this.recent.contains(&file) {
                    this.details.remove(&file);
                    return;
                }
                let detail = match read {
                    Some((read, rows)) => Detail::Ready(Shown {
                        list: gpui::ListState::new(
                            rows.len(),
                            gpui::ListAlignment::Top,
                            px(ROW_HEIGHT),
                        ),
                        file: Arc::new(read),
                        rows: Arc::new(rows),
                    }),
                    None => Detail::Failed,
                };
                this.details.insert(file, detail);
                cx.notify();
            })
            .ok();
        })
        .detach();
        let wait = cx.background_executor().timer(READING_AFTER);
        cx.spawn(async move |this, cx| {
            wait.await;
            this.update(cx, |this, cx| {
                if let Some(Detail::Reading { say }) = this.details.get_mut(&file) {
                    *say = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The next or previous file the filter leaves, going round.
    fn step(&mut self, back: bool, cx: &mut Context<Self>) {
        let count = self.matches.len();
        if count < 2 {
            return;
        }
        let at = (self.matches.iter())
            .position(|m| *m == self.selected)
            .unwrap_or(0);
        let next = (at + if back { count - 1 } else { 1 }) % count;
        self.show(self.matches[next], cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.terminal.clone();
        self.workbench
            .update(cx, |w, cx| w.close_changes(&terminal, window, cx))
            .ok();
    }

    fn open_file(&mut self, cx: &mut Context<Self>) {
        let Some(summary) = self.changes().and_then(|c| c.files.get(self.selected)) else {
            return;
        };
        let path = summary.path.clone().into();
        let first = match self.details.get(&self.selected) {
            Some(Detail::Ready(shown)) => (shown.file.hunks.iter().flat_map(|h| &h.lines))
                .find(|l| l.kind == LineKind::Added)
                .map(|l| (l.new, None)),
            _ => None,
        };
        let target = Hovered {
            path,
            at: first,
            whole: true,
            cwd: None,
        };
        self.pane
            .update(cx, |pane, cx| pane.open_target(&target, false, cx));
    }

    /// While typing in the filter the arrows and Tab still choose among what it leaves, and
    /// Enter and Esc hand the keyboard back to the panel.
    fn capture_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.filter.clone() else {
            return;
        };
        let key = &event.keystroke;
        if !field.focus_handle(cx).is_focused(window) || field.read(cx).composing() {
            return;
        }
        match key.key.as_str() {
            "down" | "up" | "tab" => {
                self.step(key.key == "up" || key.modifiers.shift, cx);
                cx.stop_propagation();
            }
            // Enter keeps what it found; Esc gives up on it. Either way reading goes on.
            "enter" => {
                window.focus(&self.focus.clone(), cx);
                cx.stop_propagation();
            }
            "escape" => {
                field.update(cx, |f, cx| {
                    f.set_text("");
                    cx.notify();
                });
                window.focus(&self.focus.clone(), cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Reading keys are the panel's. Any other key is typing: the keyboard goes back to the
    /// terminal and the key carries on to the input handler below, which is the terminal's,
    /// so a first letter, composed or not, lands there as if it had been focused.
    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = &event.keystroke;
        let held = &key.modifiers;
        if crate::keymap::action(key, cx) == Some(crate::keymap::Action::Find) {
            if let Some(field) = self.filter.clone() {
                window.focus(&field.focus_handle(cx), cx);
                cx.stop_propagation();
            }
            return;
        }
        if held.platform || held.control || held.function {
            return;
        }
        let list = match self.details.get(&self.selected) {
            Some(Detail::Ready(shown)) => Some(shown.list.clone()),
            _ => None,
        };
        match key.key.as_str() {
            "escape" => self.close(window, cx),
            "up" | "down" => {
                if let Some(list) = list {
                    list.scroll_by(px(if key.key == "up" { -STEP } else { STEP }));
                }
                cx.notify();
            }
            "pageup" | "pagedown" => {
                if let Some(list) = list {
                    let page = f32::from(list.viewport_bounds().size.height) - ROW_HEIGHT * 2.;
                    list.scroll_by(px(if key.key == "pageup" { -page } else { page }));
                }
                cx.notify();
            }
            "tab" => self.step(held.shift, cx),
            // Never sends: the terminal is what sends, and it does not have the keyboard.
            "enter" | "backspace" | "delete" | "left" | "right" | "home" | "end" => {}
            _ => {
                let pane = self.pane.read(cx).focus_handle(cx);
                window.focus(&pane, cx);
                return;
            }
        }
        cx.stop_propagation();
    }

    fn title(&self, summary: Option<&FileSummary>, cx: &mut Context<Self>) -> gpui::Div {
        let row = div()
            .h(px(T::PANE_HEADER))
            .flex_shrink_0()
            .pl(px(T::S14))
            .pr(px(T::S6))
            .flex()
            .items_center()
            .gap(px(T::S8))
            .border_b(px(T::BORDER_WIDTH))
            .border_color(T::color(T::border()))
            .text_size(px(T::SMALL));
        let row = match summary {
            Some(file) => {
                let (dir, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
                let note = if file.created {
                    Some(t!("新文件").to_owned())
                } else if file.edits > 1 {
                    Some(t!(
                        "编辑 {edits} 次，这里是合并后的结果",
                        edits = file.edits
                    ))
                } else {
                    None
                };
                row.child(
                    div()
                        .flex_shrink_0()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(T::color(T::primary()))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font(T::mono(cx))
                        .text_color(T::color(T::muted()))
                        .child(super::super::home_relative(dir)),
                )
                .child(line_counts((file.added, file.removed)))
                .children(note.map(|n| {
                    div()
                        .flex_shrink_0()
                        .text_color(T::color(T::muted()))
                        .child(n)
                }))
            }
            None => row.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(T::color(T::primary()))
                    .child(t!("看改动")),
            ),
        };
        row.child(div().flex_1())
            // Files on an attached machine do not open here (`Pane::open_target`).
            .when(summary.is_some() && !crate::lifecycle::attached(), |d| {
                d.child(
                    button("changes-open-file", t!("打开文件"))
                        .flex_shrink_0()
                        .h(px(T::ICON_HEIGHT))
                        .px(px(T::S6))
                        .text_color(T::color(T::secondary()))
                        .on_click(cx.listener(|this, _, _, cx| this.open_file(cx))),
                )
            })
            .child(
                button("changes-close", "✕")
                    .flex_shrink_0()
                    .h(px(T::ICON_HEIGHT))
                    .w(px(T::ICON_HEIGHT))
                    .px(px(0.))
                    .text_color(T::color(T::muted()))
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
    }

    fn middle_words(words: impl Into<gpui::SharedString>) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(T::S8))
            .items_center()
            .justify_center()
            .text_size(px(T::SMALL))
            .text_color(T::color(T::secondary()))
            .child(words.into())
    }

    fn retry_button(id: &'static str) -> gpui::Stateful<gpui::Div> {
        button(id, t!("再试一次"))
            .h(px(T::PANE_ACTION))
            .border(px(T::BORDER_WIDTH))
            .border_color(T::color(T::control()))
    }

    /// What goes on the right: the chosen file's lines, or what to say instead.
    fn main(&self, summary: Option<&FileSummary>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.load {
            Load::Reading { say } => {
                return Self::middle_words(if *say { t!("正在读取…") } else { "" })
                    .into_any_element();
            }
            Load::Failed => {
                return Self::middle_words(t!("读不到这一轮的会话记录"))
                    .child(
                        Self::retry_button("changes-retry")
                            .on_click(cx.listener(|this, _, _, cx| this.read_again(cx))),
                    )
                    .into_any_element();
            }
            Load::Ready(_) => {}
        }
        let Some(summary) = summary else {
            return Self::middle_words(t!("这一轮没有文件改动")).into_any_element();
        };
        let detail = self.details.get(&self.selected);
        let read = match detail {
            Some(Detail::Ready(shown)) => Some(shown),
            _ => None,
        };
        if let Some(words) = empty_words(summary, read.map(|s| s.file.as_ref())) {
            return Self::middle_words(words).into_any_element();
        }
        match detail {
            Some(Detail::Ready(shown)) => {
                let rows = shown.rows.clone();
                let font = T::mono(cx);
                gpui::list(shown.list.clone(), move |i, _, _| {
                    row_element(&rows[i], &font)
                })
                .size_full()
                .into_any_element()
            }
            Some(Detail::Failed) => {
                let file = self.selected;
                Self::middle_words(t!("读不到这个文件的改动"))
                    .child(
                        Self::retry_button("changes-file-retry").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.details.remove(&file);
                                this.show(file, cx);
                            },
                        )),
                    )
                    .into_any_element()
            }
            Some(Detail::Reading { say: true }) => {
                Self::middle_words(t!("正在读取…")).into_any_element()
            }
            _ => Self::middle_words("").into_any_element(),
        }
    }
}

impl Render for ChangesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "visual-test")]
        {
            self.drew += 1;
        }
        let focused = self.focus.contains_focused(window, cx);
        let hint = |keys: &'static str, what: &'static str| {
            div().flex_shrink_0().child(format!("{keys} {what}"))
        };
        let changes = self.changes().cloned();
        let summary = changes.as_ref().and_then(|c| c.files.get(self.selected));
        let many = changes.as_ref().is_some_and(|c| c.files.len() > 1);
        let shown = match self.details.get(&self.selected) {
            Some(Detail::Ready(shown)) => Some(shown.file.clone()),
            _ => None,
        };
        let main = self.main(summary, cx);
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .when_some(changes.as_ref().filter(|_| many), |d, c| {
                d.child(self.file_column(c, cx))
            })
            .child(div().flex_1().min_w_0().h_full().child(main));
        let notice = |words: gpui::SharedString| {
            div()
                .flex_shrink_0()
                .px(px(T::S14))
                .py(px(T::S4))
                .border_b(px(T::BORDER_WIDTH))
                .border_color(T::color(T::border()))
                .bg(Tone::Warn.wash())
                .text_color(Tone::Warn.ink())
                .text_size(px(T::SMALL))
                .child(words)
        };
        let warn = (shown.as_deref())
            .and_then(warning)
            .map(|words| notice(words.into()));
        let cut = shown.as_deref().filter(|f| f.cut).map(|f| {
            notice(
                t!(
                    "太长了，只显示前面一部分（共 +{added} −{removed}）。完整内容请打开文件。",
                    added = f.added,
                    removed = f.removed
                )
                .into(),
            )
        });
        let foot = div()
            .h(px(T::FOOTER))
            .flex_shrink_0()
            .px(px(T::S14))
            .flex()
            .items_center()
            .gap(px(T::S12))
            .overflow_hidden()
            .border_t(px(T::BORDER_WIDTH))
            .border_color(T::color(T::border()))
            .text_size(px(T::PANE_FOOTER_SIZE))
            .text_color(T::color(T::muted()))
            .child(
                div()
                    .flex_shrink_0()
                    .child(t!("用命令或脚本改的文件不在这里")),
            )
            .child(div().flex_1())
            .when(many, |d| d.child(hint("Tab", t!("下一个文件"))))
            .when(self.filter.is_some(), |d| {
                let keys = crate::keymap::keycap(cx, crate::keymap::Action::Find);
                d.child(div().flex_shrink_0().child(t!("{keys} 过滤", keys)))
            })
            .child(hint("↑↓", t!("滚动")))
            .child(hint("Esc", t!("关闭")));
        let focus = self.focus.clone();
        let pane = self.pane.clone();
        div()
            .id("changes-panel")
            .track_focus(&self.focus)
            .key_context("ChangesPanel")
            .capture_key_down(cx.listener(Self::capture_key))
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus.clone(), cx);
                    cx.stop_propagation();
                }),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(T::surface(T::pane()))
            .border_y(px(T::BORDER_WIDTH))
            .border_color(T::color(T::control()))
            // Text typed while it has the keyboard is the terminal's.
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus,
                            gpui::ElementInputHandler::new(bounds, pane.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(self.title(summary, cx))
            .children(warn)
            .children(cut)
            .child(body)
            .child(foot)
            .when(focused, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(px(T::BORDER_WIDTH))
                        .border_color(T::color(T::focus())),
                )
            })
    }
}
