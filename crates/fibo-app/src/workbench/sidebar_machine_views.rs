//! The three layouts around other machines' rows, as the preset says: machine chips over
//! one mixed list, a rail that picks one machine, or a folding section per machine. The
//! column keeps its width in all three, so nothing right of it moves.
use super::sidebar_list::{recent_order, split_today};
use super::sidebar_machines::{RemoteRow, machine_label, machine_letter, quiet};
use super::*;
use crate::machines::Link;
use crate::ui::theme::Flat as F;
use fibo_core::sidebar::{Bucket, Machines};

const RAIL: f32 = 44.;
const RAIL_BUTTON: f32 = 32.;

fn need_badge(need: usize) -> gpui::Div {
    div()
        .flex_shrink_0()
        .min_w(px(F::BADGE))
        .h(px(F::BADGE))
        .px(px(T::S4))
        .rounded(px(F::BADGE / 2.))
        .bg(Status::Waiting.colors().0)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(F::BADGE_TEXT))
        .line_height(px(F::BADGE))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(T::color(T::WHITE))
        .child(need.to_string())
}

/// What the local heading says. Folded, the spaces put aside fold with it, so it names them.
fn local_note(sessions: usize, open: bool, quiet: usize) -> String {
    if open || quiet == 0 {
        t!("{sessions} 个会话", sessions)
    } else {
        t!("{sessions} 个会话 · {quiet} 个先不管", sessions, quiet)
    }
}

/// A folding section's heading.
struct Heading {
    key: String,
    mark: Option<gpui::Div>,
    name: String,
    /// How it stands: its session count, or why it is not connected.
    note: String,
    need: usize,
    open: bool,
    /// The machine `＋` opens a terminal on.
    add: Option<usize>,
    /// The other machine this heads, whose menu it opens.
    machine: Option<usize>,
}

impl Workbench {
    /// What the list opens with: the space chips, and what the layout adds to them.
    pub(super) fn column_top(&self, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        match (self.machine_layout(), self.machine_scope()) {
            (Machines::Rail, Some(index)) => {
                let rows = self.listed_remote_rows().len();
                self.machine_spaces(index, false, rows, cx)
            }
            (Machines::Sections, _) => {
                let open = self.sidebar.open("machine:local", true);
                let mut out = vec![self.local_section_heading(open, cx)];
                if self.sidebar.showing("machine:local", true) {
                    out.extend(self.space_tiles(cx));
                }
                self.sidebar.reveal_block("machine:local", 0, 1, &mut out);
                out
            }
            (Machines::Mixed, _) => {
                let mut out = self.space_tiles(cx);
                if (0..self.machines.len()).any(|i| !self.machine_quiet(i)) {
                    out.push(self.machine_chips(cx).into_any_element());
                }
                out
            }
            _ => self.space_tiles(cx),
        }
    }
    /// A chip per other machine under the space chips: a click lists only that machine,
    /// a second click lists every one again.
    fn machine_chips(&self, cx: &Context<Self>) -> gpui::Div {
        let scope = self.machine_scope();
        let attended = |(i, _): &(usize, _)| !self.machine_quiet(*i);
        let listed = self.machines.iter().enumerate().filter(attended);
        let chips = listed.map(|(index, remote)| {
            let (host, online) = {
                let view = remote.view.lock().unwrap();
                (view.spec.host.clone(), view.link == Link::Online)
            };
            let name = machine_label(&host).to_owned();
            let need = self.remote_needs(index);
            let on = scope == Some(index);
            let chip = div()
                .id(gpui::SharedString::from(format!("machine-chip-{index}")))
                .role(gpui::accesskit::Role::Button)
                .aria_label(if on {
                    t!("正在只看 {name}，点一下看全部", name)
                } else {
                    t!("只看 {name}", name)
                })
                .h(px(F::CHIP))
                .max_w_full()
                .pl(px(T::S4))
                .pr(px(T::S8))
                .flex()
                .items_center()
                .gap(px(F::CHIP_GAP))
                .rounded(px(F::CHIP / 2.))
                .border(px(T::BORDER_WIDTH))
                .border_color(T::color(if on {
                    F::chip_on_line()
                } else {
                    F::chip_line()
                }))
                .when(on, |d| d.bg(T::color(F::chip_on())))
                .cursor_pointer()
                .hover(|s| s.border_color(T::color(F::chip_on_line())))
                .opacity(self.sidebar.motion.source_shown(&host))
                .child(
                    self.spotted_mark(&host, &name, false)
                        .when_some(self.machine_breath(index, cx), |d, lit| d.opacity(lit)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(F::CHIP_TEXT))
                        .line_height(px(F::LINE_SMALL))
                        .text_color(T::color(if !online {
                            F::third()
                        } else if on {
                            F::bright()
                        } else {
                            F::quiet()
                        }))
                        .child(if online {
                            name.clone()
                        } else {
                            t!("{name} · 断开", name)
                        }),
                )
                .when(need > 0, |d| d.child(need_badge(need)))
                .tooltip(super::diagnostics::tip(vec![
                    match self.machine_limit(index) {
                        Some(limit) => format!("{name} · {limit}"),
                        None => name.clone(),
                    },
                ]))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let again = this.sidebar.machine_scope.as_deref() == Some(host.as_str());
                    this.set_machine_scope((!again).then(|| host.clone()));
                    this.sidebar.space_filter = None;
                    cx.notify();
                }));
            self.machine_menu_on(chip, index, cx)
        });
        div()
            .mx(px(T::S12))
            .mt(px(T::S8))
            .pt(px(T::S8))
            .flex()
            .flex_wrap()
            .gap(px(F::CHIP_GAP))
            .border_t(px(T::BORDER_WIDTH))
            .border_color(T::color(F::line()))
            .font_family(T::UI_FONT)
            .children(chips)
    }
    /// That machine's spaces, in the chips' look; they only name what is there.
    pub(super) fn remote_space_chips(
        &self,
        index: usize,
        add: bool,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let rows = self.remote_rows_of(index);
        let spaces = self
            .machines
            .get(index)
            .map_or_else(Vec::new, |m| m.view.lock().unwrap().spaces.clone());
        let chips = spaces.into_iter().map(|space| {
            let need = rows
                .iter()
                .filter(|r| r.online && r.card.needs_you && r.card.space == space.id)
                .count();
            let on = self.sidebar.machine_space.as_deref() == Some(space.id.as_str());
            let pick = space.id.clone();
            div()
                .id(gpui::SharedString::from(format!(
                    "machine-space-{}",
                    space.id
                )))
                .role(gpui::accesskit::Role::Button)
                .aria_label(if on {
                    t!("{name}，点一下看全部空间", name = space.name)
                } else {
                    t!("只看 {name}", name = space.name)
                })
                .h(px(F::CHIP))
                .max_w_full()
                .pl(px(T::S6))
                .pr(px(T::S8))
                .flex()
                .items_center()
                .gap(px(F::CHIP_GAP))
                .rounded(px(F::CHIP / 2.))
                .border(px(T::BORDER_WIDTH))
                .border_color(T::color(if on {
                    F::chip_on_line()
                } else {
                    F::chip_line()
                }))
                .when(on, |d| d.bg(T::color(F::chip_on())))
                .cursor_pointer()
                .hover(|s| s.border_color(T::color(F::chip_on_line())))
                .child(self.space_dot(&space.id, F::CHIP_DOT))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(F::CHIP_TEXT))
                        .line_height(px(F::LINE_SMALL))
                        .text_color(T::color(if on { F::bright() } else { F::quiet() }))
                        .child(space.name),
                )
                .when(need > 0, |d| d.child(need_badge(need)))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_machine_space(&pick, cx)))
        });
        div()
            .flex()
            .flex_wrap()
            .gap(px(F::CHIP_GAP))
            .px(px(T::S12))
            .pt(px(T::S10))
            .pb(px(T::S2))
            .font_family(T::UI_FONT)
            .children(chips)
            .when(add, |d| d.child(self.add_space_chip(index, cx)))
    }

    fn section_heading(&self, h: Heading, cx: &Context<Self>) -> gpui::AnyElement {
        const HEADING: f32 = 34.;
        let Heading {
            key,
            mark,
            name,
            note,
            need,
            open,
            add,
            machine,
        } = h;
        let toggle = key.clone();
        let key_for_mark = key.clone();
        let shown = key
            .strip_prefix("machine:")
            .map_or(1., |host| self.sidebar.motion.source_shown(host));
        let heading = div()
            .id(gpui::SharedString::from(format!("section-{key}")))
            .opacity(shown)
            .role(gpui::accesskit::Role::Button)
            .aria_label(if open {
                t!("{name}，点一下收起", name)
            } else {
                t!("{name}，点一下展开", name)
            })
            .mt(px(T::S10))
            .h(px(HEADING))
            .flex_shrink_0()
            .pl(px(T::S14))
            .pr(px(T::S10))
            .flex()
            .items_center()
            .gap(px(T::S6))
            .border_t(px(T::BORDER_WIDTH))
            .border_b(px(T::BORDER_WIDTH))
            .border_color(T::color(F::line()))
            .bg(T::color(F::hover()))
            .cursor_pointer()
            .font_family(T::UI_FONT)
            .child(self.sidebar.fold_mark(&key_for_mark, open))
            .children(mark)
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(T::SMALL))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(T::color(F::bright()))
                    .child(name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(F::TAG))
                    .text_color(T::color(F::third()))
                    .child(note),
            )
            .when(need > 0, |d| d.child(need_badge(need)))
            .when_some(add, |d, index| {
                d.child(
                    div()
                        .id(gpui::SharedString::from(format!("machine-open-{index}")))
                        .role(gpui::accesskit::Role::Button)
                        .aria_label(t!("在 {name} 上开一个新终端", name))
                        .size(px(T::S16 + T::S4))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(T::S4))
                        .text_color(T::color(F::second()))
                        .hover(|s| s.bg(T::color(F::selected())))
                        .child("+")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.show_remote_start(index, this.machine_space_of(index), cx);
                        })),
                )
            })
            .on_click(
                cx.listener(move |this, _, _, cx| this.toggle_sidebar_group(&toggle, open, cx)),
            );
        match machine {
            Some(index) => self.machine_menu_on(heading, index, cx).into_any_element(),
            None => heading.into_any_element(),
        }
    }
    fn local_section_heading(&self, open: bool, cx: &Context<Self>) -> gpui::AnyElement {
        let sessions = self
            .workspace
            .agent_sessions
            .iter()
            .filter(|s| self.session_hidden(s).is_none() && self.session_open(s))
            .count();
        self.section_heading(
            Heading {
                key: "machine:local".into(),
                mark: None,
                name: crate::lifecycle::here(),
                note: local_note(
                    sessions,
                    open,
                    self.workspace
                        .spaces
                        .iter()
                        .filter(|s| self.workspace.space_quiet(&s.id))
                        .count(),
                ),
                need: self.needs_you().len(),
                open,
                add: None,
                machine: None,
            },
            cx,
        )
    }
    /// One other machine's section: its heading, then its rows in 会话优先's order. Folded
    /// by default while it is not connected.
    fn machine_section(&self, index: usize, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        let Some(host) = self.machine_host(index) else {
            return Vec::new();
        };
        let link = self.machines[index].view.lock().unwrap().link.clone();
        let mut rows = self.remote_rows_of(index);
        let key = Self::machine_section_key(&host);
        let online = link == Link::Online;
        let open = self.sidebar.open(&key, online);
        let name = machine_label(&host).to_owned();
        let note = match &link {
            Link::Online => match self.machine_limit(index) {
                Some(limit) => t!("{len} 个会话 · {limit}", len = rows.len(), limit),
                None => t!("{len} 个会话", len = rows.len()),
            },
            Link::Connecting => t!("连接中…").to_owned(),
            Link::Offline(why) => t!("断开：{why}", why),
        };
        let mut out = vec![
            self.section_heading(
                Heading {
                    key: key.clone(),
                    mark: Some(
                        self.spotted_mark(&host, &name, false)
                            .when_some(self.machine_breath(index, cx), |d, lit| d.opacity(lit)),
                    ),
                    name,
                    note,
                    need: self.remote_needs(index),
                    open,
                    add: (online && self.machine_manages(index)).then_some(index),
                    machine: Some(index),
                },
                cx,
            ),
        ];
        if !self.sidebar.showing(&key, online) {
            return out;
        }
        if let Some(space) = self.machine_space_of(index) {
            rows.retain(|r| r.card.space == space.id);
        }
        out.extend(self.machine_spaces(index, true, rows.len(), cx));
        let keyed: Vec<(&RemoteRow, Bucket, u64)> =
            rows.iter().map(|r| (r, r.bucket, r.when)).collect();
        let order = recent_order(&keyed);
        let history = order
            .stopped
            .into_iter()
            .chain(order.quiet)
            .map(|r| (r, r.when))
            .collect();
        let (today, older) = split_today(history, crate::model::time::now());
        for (part, title, rows) in [
            ("need", fibo_core::status::needs_you_word(), order.need),
            ("running", Status::Running.label(), order.running),
            ("today", t!("今天"), today),
            ("older", t!("更早"), older),
        ] {
            let part = format!("{key}:{part}");
            if !rows.is_empty() {
                out.push(self.flat_fold(&part, title, rows.len(), true, cx));
                let showing = self.sidebar.showing(&part, true);
                let (start, mut keep) = (out.len(), Vec::new());
                for r in rows.into_iter().filter(|r| showing || r.shown) {
                    if r.shown && !self.sidebar.open(&part, true) {
                        keep.push(out.len());
                    }
                    out.push(self.remote_row(r, cx));
                }
                self.sidebar.reveal(&part, start, &keep, &mut out);
            }
        }
        self.sidebar.reveal_block(&key, 0, 1, &mut out);
        out
    }
    /// What follows this machine's list.
    pub(super) fn after_local_list(
        &self,
        recent: bool,
        cx: &Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        // Spaces belong to this machine: they end its list, folded away with it.
        let mut own = Vec::new();
        if !quiet::foot_holds_spaces(self.machine_layout())
            && self.sidebar.showing("machine:local", true)
        {
            own = self.quiet_space_lines(cx);
            self.sidebar.reveal_block("machine:local", 0, 0, &mut own);
        }
        let mut out = match self.machine_layout() {
            Machines::Below => self.machine_rows(recent, cx),
            Machines::Sections => (0..self.machines.len())
                .filter(|i| {
                    !self.machine_quiet(*i)
                        || (self.machine_host(*i))
                            .is_some_and(|h| self.sidebar.motion.block_closing(&h))
                })
                .flat_map(|i| {
                    let mut section = self.machine_section(i, cx);
                    if let Some(host) = self.machine_host(i) {
                        let key = super::sidebar_motion::block_key(&host);
                        self.sidebar.reveal_block(&key, 0, 0, &mut section);
                    }
                    section
                })
                .collect(),
            Machines::Mixed | Machines::Rail => Vec::new(),
        };
        own.append(&mut out);
        let mut out = own;
        // 会话优先 keeps them in its foot (`flat_column`); other lists end with them.
        if !recent {
            out.extend(self.quiet_lines(cx));
        }
        out
    }

    /// The rail's button for this machine (`None`) or another.
    fn rail_button(&self, index: Option<usize>, cx: &Context<Self>) -> gpui::AnyElement {
        let view = index.and_then(|i| Some(self.machines.get(i)?.view.lock().unwrap().clone()));
        let online = view.as_ref().is_none_or(|v| v.link == Link::Online);
        let host = view.as_ref().map(|v| v.spec.host.clone());
        let name =
            (host.as_deref()).map_or_else(crate::lifecycle::here, |h| machine_label(h).to_owned());
        // While the mark slides to it the mark says so.
        let on = self.machine_scope() == index;
        let lit = on && self.sidebar.motion.rail_at().is_none();
        let need = index.map_or_else(|| self.needs_you().len(), |i| self.remote_needs(i));
        let face = match index {
            None => gpui::svg()
                .path("icons/terminal.svg")
                .size(px(F::ICON))
                .text_color(T::color(if on { F::bright() } else { F::second() }))
                .into_any_element(),
            Some(_) => div()
                .text_size(px(T::SMALL))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(T::color(if !online {
                    F::third()
                } else if on {
                    F::bright()
                } else {
                    F::quiet()
                }))
                .child(machine_letter(&name))
                .into_any_element(),
        };
        let label = if online {
            name.clone()
        } else {
            t!("{name}，断开", name)
        };
        let button = div()
            .id(gpui::SharedString::from(format!(
                "rail-{}",
                index.map_or_else(|| "local".to_owned(), |i| i.to_string())
            )))
            .role(gpui::accesskit::Role::Button)
            .aria_label(label.clone())
            .relative()
            .when_some(host.as_deref(), |d, h| {
                d.opacity(self.sidebar.motion.source_shown(h)).child(
                    self.sidebar
                        .motion
                        .mark_spot(h, false)
                        .absolute()
                        .size_full(),
                )
            })
            .size(px(RAIL_BUTTON))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(T::S8))
            .border(px(T::BORDER_WIDTH))
            .when(!online, |d| d.border_dashed())
            .border_color(T::color(if lit {
                F::chip_on_line()
            } else {
                F::chip_line()
            }))
            .when(lit, |d| d.bg(T::color(F::selected())))
            .cursor_pointer()
            .hover(|s| s.bg(T::color(F::hover())))
            .child(face)
            .tooltip(super::diagnostics::tip(vec![
                match index.and_then(|i| self.machine_limit(i)) {
                    Some(limit) => format!("{label} · {limit}"),
                    None => label,
                },
            ]))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_machine_scope(host.clone());
                this.sidebar.space_filter = None;
                cx.notify();
            }));
        let button = match index {
            Some(i) => self.machine_menu_on(button, i, cx),
            None => button,
        };
        // The badge lies beside the button, not in it: a div paints its border after its
        // children, so the button's corner would cross the digits.
        div()
            .relative()
            .size(px(RAIL_BUTTON))
            .flex_shrink_0()
            .when_some(index.and_then(|i| self.machine_breath(i, cx)), |d, lit| {
                d.opacity(lit)
            })
            .child(button)
            .when(need > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-T::S4))
                        .right(px(-T::S4))
                        .child(need_badge(need)),
                )
            })
            .into_any_element()
    }
    fn machine_rail(&self, cx: &Context<Self>) -> gpui::Div {
        const GAP: f32 = T::S10;
        div()
            .w(px(RAIL))
            .flex_shrink_0()
            .pt(px(T::S6))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(GAP))
            .border_r(px(T::BORDER_WIDTH))
            .border_t(px(T::BORDER_WIDTH))
            .border_color(T::color(F::line()))
            .when_some(self.sidebar.motion.rail_at(), |d, slot| {
                d.child(
                    div()
                        .absolute()
                        .left(px((RAIL - T::BORDER_WIDTH - RAIL_BUTTON) / 2.))
                        .top(px(T::S6 + slot * (RAIL_BUTTON + GAP)))
                        .size(px(RAIL_BUTTON))
                        .rounded(px(T::S8))
                        .border(px(T::BORDER_WIDTH))
                        .border_color(T::color(F::chip_on_line()))
                        .bg(T::color(F::selected())),
                )
            })
            .child(self.rail_button(None, cx))
            .children(
                (0..self.machines.len())
                    .filter(|i| !self.machine_quiet(*i))
                    .map(|i| self.rail_button(Some(i), cx)),
            )
            .child(div().flex_1())
            .child(self.rail_add(cx))
    }
    /// At the rail's foot: another computer, added where they all are, in settings.
    fn rail_add(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("rail-add")
            .role(gpui::accesskit::Role::Button)
            .aria_label(t!("添加一台电脑"))
            .size(px(RAIL_BUTTON))
            .mb(px(T::S10))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(T::S8))
            .border(px(T::BORDER_WIDTH))
            .border_dashed()
            .border_color(T::color(F::chip_line()))
            .text_color(T::color(F::third()))
            .cursor_pointer()
            .hover(|s| s.bg(T::color(F::hover())))
            .tooltip(super::diagnostics::tip(vec![t!("添加一台电脑").to_owned()]))
            .child("+")
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_settings_page(settings::Page::Phones, window, cx);
            }))
    }
    /// 会话优先's column at its usual width, with the rail inside it when the layout has one.
    pub(super) fn flat_column(
        &self,
        list: gpui::Stateful<gpui::Div>,
        listed: usize,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let column = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.flat_brand())
            .child(self.flat_search(cx))
            .child(list)
            .children(self.quiet_lines(cx))
            .child(self.flat_keys(cx))
            .child(self.flat_foot(listed, cx));
        // The rail lies over the left of the column rather than beside it: in a row, taffy
        // measures the whole list's content to size it (117 M instructions a redraw).
        let body = match (self.machine_layout() == Machines::Rail).then(|| self.machine_rail(cx)) {
            Some(rail) => div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .pl(px(RAIL))
                .child(column)
                .child(rail.absolute().left_0().top_0().bottom_0()),
            None => column,
        };
        div()
            .id("sidebar")
            .relative()
            .w(px(F::WIDTH))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(T::surface(F::bg()))
            .border_r(px(T::BORDER_WIDTH))
            .border_color(T::color(F::line()))
            .child(self.flat_lights(cx))
            .child(body)
            .children(self.sidebar.motion.pill())
    }
    /// What the brand row names: this machine, or the one shown alone, and whether it is up.
    pub(super) fn column_machine(&self) -> (String, bool) {
        self.machine_scope()
            .filter(|_| self.machine_layout() == Machines::Rail)
            .and_then(|i| {
                let view = self.machines.get(i)?.view.lock().unwrap();
                let name = machine_label(&view.spec.host).to_owned();
                Some(if view.link == Link::Online {
                    (name, true)
                } else {
                    (t!("{name} · 断开", name), false)
                })
            })
            .unwrap_or_else(|| (crate::lifecycle::here(), true))
    }
    /// The foot's first words.
    pub(super) fn column_foot(&self, sessions: usize) -> String {
        match self
            .machine_scope()
            .filter(|_| self.machine_layout() == Machines::Rail)
        {
            Some(i) => t!(
                "{machine} · {len} 个会话",
                machine = self.column_machine().0,
                len = self.remote_rows_of(i).len()
            ),
            None if crate::lifecycle::attached() => t!(
                "{machine} · {len} 个空间 · {sessions} 个会话",
                machine = crate::lifecycle::here(),
                len = self.workspace.spaces.len(),
                sessions
            ),
            None => t!(
                "本机 · {len} 个空间 · {sessions} 个会话",
                len = self.workspace.spaces.len(),
                sessions
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::local_note;

    #[test]
    fn a_folded_heading_names_the_spaces_put_aside_that_folded_with_it() {
        assert_eq!(local_note(20, false, 1), "20 个会话 · 1 个先不管");
        assert_eq!(local_note(20, true, 1), "20 个会话");
        assert_eq!(local_note(20, false, 0), "20 个会话");
    }
}
