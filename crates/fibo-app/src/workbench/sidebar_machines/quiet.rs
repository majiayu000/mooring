//! 先不管这台 (docs/multi-machine/spec.md): whether you attend to a machine, kept in
//! `machines.json` and independent of how it is connected. A quiet machine stays connected
//! but counts nowhere, ⌘J skips it, and it leaves the chips, the rail, the sections and
//! the board for one gray line per machine; links dropping change nothing on that line.
//! Looking at its sessions never ends it; 接着管 does, and so does starting work there.
use super::*;
use crate::workbench::palette::Hit;
use crate::workbench::sidebar_motion::mix;
use std::rc::Rc;

/// What a gray line's 接着管 does.
pub(in crate::workbench) type Resume = Rc<dyn Fn(&mut Workbench, &mut Context<Workbench>)>;

/// What differs between one gray line and another.
pub(in crate::workbench) struct LineFacts {
    pub id: String,
    pub resume_id: String,
    /// The key its fold is remembered under.
    pub fold: String,
    /// The key the flight finds its square under.
    pub seat: String,
    pub name: String,
    /// All its words, starting with `name`.
    pub words: String,
    pub resume: Option<Resume>,
    /// Others whose flight lands on this line, since they have none of their own showing.
    pub also_seats: Vec<String>,
    pub kind: LineKind,
}

/// What a gray line is the line of, since a machine and a space are not the same size of
/// thing: a machine reads as a folded heading beside 本机, a space as the end of its
/// machine's list in the dot it has among the chips.
pub(in crate::workbench) enum LineKind {
    Machine,
    /// The spaces whose dots it shows; `first` draws the line above the group.
    Space {
        dots: Vec<String>,
        first: bool,
    },
}

/// Whether the column's foot holds the spaces put aside, under no machine's heading: only
/// 按机器 has one for them to end.
pub(in crate::workbench) fn foot_holds_spaces(layout: fibo_core::sidebar::Machines) -> bool {
    layout != fibo_core::sidebar::Machines::Sections
}

fn fold_key(host: &str) -> String {
    format!("quiet:{host}")
}

/// A state with no color, so nothing a quiet machine does shows on its rows.
pub(in crate::workbench) fn quiet_ring() -> gpui::Div {
    div()
        .size(px(F::STATE))
        .flex_shrink_0()
        .rounded_full()
        .border(px(T::BORDER_WIDTH * 1.5))
        .border_color(T::color(F::third()))
}

/// A space's name on a quiet machine's row.
pub(in crate::workbench) fn quiet_tag(name: &str) -> gpui::Div {
    div()
        .flex_shrink_0()
        .max_w(px(F::TAG_MAX))
        .truncate()
        .font_family(T::UI_FONT)
        .text_size(px(F::TAG))
        .line_height(px(F::LINE_SMALL))
        .text_color(T::color(F::third()))
        .child(name.to_owned())
}

/// The gray line's words. The count is what its host listed last, connected or not, so a
/// link coming and going does not change it.
pub(super) fn quiet_words(name: &str, waiting: usize) -> String {
    if waiting == 0 {
        t!("{name} · 先不管", name)
    } else {
        t!("{name} · 先不管 · {waiting} 个在等", name, waiting)
    }
}

/// What the palette says for a machine.
fn switch_words(name: &str, quiet: bool) -> String {
    if quiet {
        t!("接着管 {name}", name)
    } else {
        t!("先不管 {name}", name)
    }
}

impl Workbench {
    pub(in crate::workbench) fn machine_quiet(&self, index: usize) -> bool {
        (self.machines.get(index)).is_some_and(|m| m.view.lock().unwrap().spec.quiet)
    }
    /// Both ways between attending and 先不管. Saved at once; a failed save still applies
    /// here and says the next launch will not remember it.
    pub(in crate::workbench) fn set_machine_quiet(
        &mut self,
        index: usize,
        quiet: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.machine_host(index) else {
            return;
        };
        self.machines[index].view.lock().unwrap().spec.quiet = quiet;
        self.sidebar.machine_menu = None;
        if quiet {
            // Its heading goes, and what was under it.
            if self.machine_space_of(index).is_some() {
                self.sidebar.machine_space = None;
            }
            self.cancel_machine_space(cx);
        }
        let name = machine_label(&host).to_owned();
        let rail = self.machine_layout() == Machines::Rail;
        self.sidebar.motion.fly(&host, &name, quiet, rail);
        // You just left it: the column does not go on showing it alone.
        if quiet && self.sidebar.machine_scope.as_deref() == Some(host.as_str()) {
            self.set_machine_scope(None);
        }
        if let Err(e) = crate::machines::set_quiet(&host, quiet) {
            self.error = Some(t!("没存上，下次打开不会记得：{e}", e));
        }
        cx.notify();
    }
    /// You began work there, so it is back: an answer you gave it can lead to a question
    /// you would not hear of otherwise. One press too many costs less, as for a space.
    pub(in crate::workbench) fn work_began_on_machine(
        &mut self,
        host: &str,
        cx: &mut Context<Self>,
    ) {
        let index = (0..self.machines.len())
            .find(|i| self.machine_quiet(*i) && self.machine_host(*i).as_deref() == Some(host));
        if let Some(index) = index {
            self.set_machine_quiet(index, false, cx);
        }
    }
    /// A machine's square that remembers where it was drawn, for the flight.
    pub(in crate::workbench) fn spotted_mark(
        &self,
        host: &str,
        name: &str,
        line: bool,
    ) -> gpui::Div {
        machine_mark(machine_letter(name), line).relative().child(
            self.sidebar
                .motion
                .mark_spot(host, line)
                .absolute()
                .size_full(),
        )
    }
    /// Right button on anything naming a machine, in either state.
    pub(in crate::workbench) fn machine_menu_on<E: gpui::InteractiveElement + 'static>(
        &self,
        element: E,
        index: usize,
        cx: &Context<Self>,
    ) -> E {
        let host = self.machine_host(index).unwrap_or_default();
        element.on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                this.sidebar.machine_menu = Some((host.clone(), event.position));
                cx.stop_propagation();
                cx.notify();
            }),
        )
    }
    /// The menus opened from the left column, drawn over the whole window.
    pub(in crate::workbench) fn sidebar_menus(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        if let Some((id, position)) = &self.layout.space_menu {
            out.push(self.space_menu_view(id, *position, window, cx));
        }
        let menu = self.sidebar.machine_menu.as_ref().and_then(|(host, at)| {
            let index = (0..self.machines.len())
                .find(|i| self.machine_host(*i).as_deref() == Some(host.as_str()))?;
            Some((index, *at))
        });
        if let Some((index, at)) = menu {
            let quiet = self.machine_quiet(index);
            let entry = button(
                "machine-menu-quiet",
                if quiet {
                    t!("接着管这台")
                } else {
                    t!("先不管这台")
                },
            )
            .w_full()
            .justify_start()
            .border_0()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_machine_quiet(index, !quiet, cx);
            }));
            // That machine's own host in a window of its own, worked as this one's is.
            let host = self.machine_host(index).unwrap_or_default();
            let open = button("machine-menu-open-window", t!("在新窗口打开"))
                .w_full()
                .justify_start()
                .border_0()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar.machine_menu = None;
                    if let Err(e) = crate::attach::open_window(&host) {
                        this.error = Some(t!("没能打开 {host} 的窗口：{e}", host = &host, e));
                    }
                    cx.notify();
                }));
            let entries = vec![open.into_any_element(), entry.into_any_element()];
            out.push(self.popup(at, entries, window, cx, |this| {
                this.sidebar.machine_menu = None
            }));
        }
        out
    }

    fn quiet_indexes(&self) -> impl Iterator<Item = usize> + use<'_> {
        (0..self.machines.len()).filter(|i| self.machine_quiet(*i))
    }
    /// One gray line per quiet machine, each unfolding to its sessions: in the column's foot
    /// for 会话优先, at the list's end elsewhere. Empty when you attend to all of them.
    pub(in crate::workbench) fn quiet_lines(&self, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        // Rows fit this many before the foot scrolls, and the height is definite (docs/
        // performance: a wrapper sized by max_h measures its content again every frame).
        const SHOWN: usize = 6;
        let row = F::ONE_PAD_Y * 2. + F::TITLE_LINE;
        let mut out = if foot_holds_spaces(self.machine_layout()) {
            self.quiet_space_lines(cx)
        } else {
            Vec::new()
        };
        for index in self.quiet_indexes().collect::<Vec<_>>() {
            let Some(host) = self.machine_host(index) else {
                continue;
            };
            let key = fold_key(&host);
            let open = self.sidebar.open(&key, false);
            let name = machine_label(&host).to_owned();
            let words = quiet_words(&name, self.quiet_waiting(index));
            let state = self.sidebar.motion.line_state(&host);
            let line = self.quiet_line(
                LineFacts {
                    id: format!("quiet-{index}"),
                    resume_id: format!("quiet-resume-{index}"),
                    fold: key,
                    seat: host.clone(),
                    name,
                    words,
                    resume: Some(Rc::new(move |this, cx| {
                        this.set_machine_quiet(index, false, cx);
                    })),
                    also_seats: Vec::new(),
                    kind: LineKind::Machine,
                },
                cx,
            );
            out.push(self.machine_menu_on(line, index, cx).into_any_element());
            if open {
                let mut rows = self.remote_rows_of(index);
                // What needs you first, then the latest, as the other lists read.
                rows.sort_by_key(|r| (!r.card.needs_you, std::cmp::Reverse(r.when)));
                let height = row * rows.len().min(SHOWN) as f32;
                out.push(
                    div()
                        .id(gpui::SharedString::from(format!("quiet-rows-{index}")))
                        .h(px(height))
                        .flex_shrink_0()
                        .opacity(state.shown)
                        .overflow_y_scroll()
                        .children(rows.iter().map(|r| self.remote_row(r, cx)))
                        .into_any_element(),
                );
            }
        }
        out
    }
    /// One gray line, a machine's or a space's: its fold, its square and words, and the way
    /// back when it has one of its own.
    pub(in crate::workbench) fn quiet_line(
        &self,
        facts: LineFacts,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        const LINE: f32 = 34.;
        let LineFacts {
            id,
            resume_id,
            fold,
            seat,
            name,
            words,
            resume,
            also_seats,
            kind,
        } = facts;
        let machine = matches!(kind, LineKind::Machine);
        let open = self.sidebar.open(&fold, false);
        let state = self.sidebar.motion.line_state(&seat);
        // A heading names itself and then how it stands.
        let rest = words[name.len()..].to_owned();
        let rest = if machine {
            rest.trim_start_matches(" · ").to_owned()
        } else {
            rest
        };
        let seats = also_seats.iter().map(|seat| {
            (self.sidebar.motion.mark_spot(seat, true))
                .absolute()
                .size_full()
        });
        let (mark, first) = match kind {
            LineKind::Machine => (
                self.spotted_mark(&seat, &name, true)
                    .children(seats)
                    .into_any_element(),
                true,
            ),
            LineKind::Space { dots, first } => {
                // A square's room around the dots, so a flight lands the same way on both.
                let stack = dots.iter().map(|id| self.space_dot(id, F::CHIP_DOT));
                (
                    div()
                        .relative()
                        .h(px(MARK))
                        .min_w(px(F::CHIP_DOT))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(T::S2))
                        .child(
                            (self.sidebar.motion.mark_spot(&seat, true))
                                .absolute()
                                .size_full(),
                        )
                        .children(seats)
                        .children(stack)
                        .into_any_element(),
                    first,
                )
            }
        };
        div()
            .id(gpui::SharedString::from(id))
            .relative()
            .opacity(state.shown)
            .when(state.glow > 0., |d| {
                d.bg(T::color(F::selected_line()).opacity(state.glow * 0.6))
            })
            .role(gpui::accesskit::Role::Button)
            .aria_label(if open {
                t!("{words}，点一下收起", words)
            } else {
                t!("{words}，点一下展开", words)
            })
            .h(px(LINE))
            .flex_shrink_0()
            .pl(px(if machine { T::S14 } else { F::EDGE }))
            .pr(px(T::S10))
            .flex()
            .items_center()
            .gap(px(T::S6))
            .when(first, |d| {
                d.border_t(px(T::BORDER_WIDTH))
                    .border_color(T::color(F::line()))
            })
            .when(machine, |d| {
                d.border_b(px(T::BORDER_WIDTH)).bg(T::color(F::hover()))
            })
            .when(!machine && first, |d| d.mt(px(T::S8)))
            .font_family(T::UI_FONT)
            .cursor_pointer()
            .hover(|s| s.bg(T::color(F::hover())))
            .when(machine, |d| d.child(self.sidebar.fold_mark(&fold, open)))
            .child(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .when(machine, |d| d.gap(px(T::S6)))
                    .text_size(px(F::TAG))
                    .text_color(mix(T::color(F::third()), T::color(F::bright()), state.glow))
                    .child(
                        div()
                            .flex_shrink_0()
                            .when(machine, |d| {
                                d.font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(T::color(F::second()))
                            })
                            .child(name.clone()),
                    )
                    .child(div().min_w_0().truncate().opacity(state.rest).child(rest)),
            )
            .when_some(resume, |d, resume| {
                let press = mix(T::color(F::hover()), T::color(F::bright()), 0.1);
                d.child(
                    div()
                        .id(gpui::SharedString::from(resume_id))
                        .role(gpui::accesskit::Role::Button)
                        .aria_label(t!("接着管 {name}", name))
                        .group(gpui::SharedString::from("quiet-resume"))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .px(px(T::S6))
                        .py(px(T::S2))
                        .rounded(px(T::S4))
                        .text_size(px(F::TAG))
                        .text_color(T::color(F::second()))
                        .cursor_pointer()
                        // Words and an arrow, no box; under the pointer a step darker than
                        // the gray line, black words, and the arrow leans on.
                        .hover(move |s| s.bg(press).text_color(T::color(F::bright())))
                        .child(t!("接着管"))
                        .child(
                            div()
                                .pl(px(T::S2))
                                .pr(px(T::S2))
                                .group_hover("quiet-resume", |s| s.pl(px(T::S4)).pr(px(0.)))
                                .child("→"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            resume(this, cx);
                        })),
                )
            })
            .when(!machine, |d| d.child(self.sidebar.fold_mark(&fold, open)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sidebar.toggled.insert(fold.clone(), !open);
                cx.notify();
            }))
    }
    /// A board line for something it leaves out: its words and the way back.
    pub(in crate::workbench) fn quiet_board_line(
        &self,
        words: String,
        resume_id: gpui::SharedString,
        resume: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> gpui::Div {
        div()
            .px(px(T::OVERVIEW_X))
            .pb(px(T::S8))
            .flex()
            .items_center()
            .gap(px(T::S8))
            .text_size(px(T::SMALL))
            .text_color(T::color(T::muted()))
            .child(words)
            .child(
                button(resume_id, t!("接着管"))
                    .h(px(T::SEGMENT_HEIGHT))
                    .px(px(T::S6))
                    .on_click(resume),
            )
    }
    /// The board's line for what it leaves out, under its header.
    pub(in crate::workbench) fn quiet_board_lines(
        &self,
        cx: &Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        self.quiet_indexes()
            .filter_map(|index| {
                let host = self.machine_host(index)?;
                let name = machine_label(&host).to_owned();
                let words = quiet_words(&name, self.quiet_waiting(index));
                Some(
                    self.quiet_board_line(
                        words,
                        gpui::SharedString::from(format!("board-quiet-resume-{index}")),
                        cx.listener(move |this, _, _, cx| {
                            this.set_machine_quiet(index, false, cx);
                        }),
                    )
                    .into_any_element(),
                )
            })
            .collect()
    }

    /// ⌘P: 先不管 or 接着管, found by those words or the machine's name.
    pub(in crate::workbench) fn machine_hits(&self, query: &str) -> Vec<Hit> {
        (0..self.machines.len())
            .filter(|i| {
                self.machine_host(*i).is_some_and(|host| {
                    t!(
                        "先不管 接着管 恢复 电脑 机器 {name}",
                        name = machine_label(&host)
                    )
                    .to_lowercase()
                    .contains(&query.to_lowercase())
                })
            })
            .map(Hit::Machine)
            .collect()
    }
    pub(in crate::workbench) fn machine_hit_title(&self, index: usize) -> String {
        let host = self.machine_host(index).unwrap_or_default();
        switch_words(machine_label(&host), self.machine_quiet(index))
    }
    pub(in crate::workbench) fn switch_machine_quiet(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let quiet = self.machine_quiet(index);
        self.set_machine_quiet(index, !quiet, cx);
    }
}

#[cfg(feature = "visual-test")]
impl Workbench {
    pub fn visual_set_machine_quiet(&mut self, index: usize, quiet: bool, cx: &mut Context<Self>) {
        self.set_machine_quiet(index, quiet, cx);
    }
    pub fn visual_machine_quiet(&self, index: usize) -> bool {
        self.machine_quiet(index)
    }
    pub fn visual_quiet_waiting(&self, index: usize) -> usize {
        self.quiet_waiting(index)
    }
    /// Where machine `index`'s square was drawn on its heading and line, and is in flight.
    pub fn visual_flight_marks(&self, index: usize) -> [Option<(f32, f32)>; 3] {
        let host = self.machine_host(index).unwrap_or_default();
        self.sidebar.motion.visual_marks(&host)
    }
    pub fn visual_quiet_moving(&self) -> bool {
        self.sidebar.motion.moving()
    }
    pub fn visual_quiet_unfold(&mut self, index: usize, open: bool, cx: &mut Context<Self>) {
        if let Some(host) = self.machine_host(index) {
            self.sidebar.toggled.insert(fold_key(&host), open);
            cx.notify();
        }
    }
    pub fn visual_machine_menu(&mut self, index: usize, open: bool, cx: &mut Context<Self>) {
        self.sidebar.machine_menu = self
            .machine_host(index)
            .filter(|_| open)
            .map(|host| (host, gpui::point(px(F::WIDTH * 0.4), px(F::WIDTH * 0.6))));
        cx.notify();
    }
    /// What ⌘P lists for machines on `query`, and picks the first as a click would.
    pub fn visual_machine_hit(&mut self, query: &str, cx: &mut Context<Self>) -> Option<String> {
        let index = self.machine_hits(query).into_iter().find_map(|h| match h {
            Hit::Machine(i) => Some(i),
            _ => None,
        })?;
        let title = self.machine_hit_title(index);
        self.switch_machine_quiet(index, cx);
        Some(title)
    }
    pub fn visual_leave_board(&mut self, cx: &mut Context<Self>) {
        self.nav.overview = false;
        self.sync_panes(cx);
        cx.notify();
    }
    pub fn visual_close_peek(&mut self, cx: &mut Context<Self>) {
        self.close_peek(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_says_how_many_wait_only_when_some_do() {
        assert_eq!(quiet_words("apple", 2), "apple · 先不管 · 2 个在等");
        assert_eq!(quiet_words("apple", 0), "apple · 先不管");
    }
    #[test]
    fn only_a_layout_with_a_heading_for_this_machine_takes_its_spaces_off_the_foot() {
        use fibo_core::sidebar::Machines;
        assert!(!foot_holds_spaces(Machines::Sections));
        for layout in [Machines::Below, Machines::Mixed, Machines::Rail] {
            assert!(foot_holds_spaces(layout));
        }
    }
    #[test]
    fn the_palette_offers_the_way_back_from_where_it_is() {
        assert_eq!(switch_words("apple", false), "先不管 apple");
        assert_eq!(switch_words("apple", true), "接着管 apple");
    }
}
