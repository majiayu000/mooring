//! The window's bottom edge: this machine and how many sessions it lists on the left, the
//! usage limits, diagnostics and settings on the right. The limits get one cell per CLI,
//! each with its 5h and week windows and a thin bar (decided 11-03, replacing the single
//! fixed-width quota button); past `BAR_CELLS` the rest fold into `+N ▴`, and the popover
//! lists them all.
use super::statusbar_update::divider;
use super::*;
use crate::ui::widgets::gauge;
use fibo_core::quota::{NEAR, OVER, Quota, Window as QuotaWindow};

mod sessions;

/// Which of the bar's popovers is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum StatusPop {
    Sessions,
    Quota,
}

impl Workbench {
    pub(super) fn statusbar_view(
        &self,
        width: f32,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let fit = Fit::of(width);
        let open = self.statusbar;
        let cut = self.store.cut_off();
        let detail = (self.store.cut_off_reason())
            .unwrap_or(&self.store.recognition().detail)
            .to_owned();
        // 「本机 · 11 个会话」: this machine and how many sessions it lists. What needs you
        // is the title row's 要你处理 alone; a click here still lists what works or waits.
        let listed = self.facts().listed;
        let host = item("status-host", open == Some(StatusPop::Sessions))
            .aria_label(t!(
                "{name}，{listed} 个会话",
                name = crate::lifecycle::here(),
                listed
            ))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_status(StatusPop::Sessions, cx)))
            .child(div().size(px(T::DOT)).rounded_full().bg(if cut {
                Tone::Danger.dot()
            } else {
                Tone::Ok.dot()
            }))
            // A cut connection always says so; a healthy one can go down to its dot.
            .when(cut || fit.host_label, |d| {
                d.child(
                    div()
                        .text_color(if cut {
                            Tone::Danger.ink()
                        } else {
                            T::color(T::secondary())
                        })
                        .child(if cut {
                            t!("后台断开，正在重连").to_owned()
                        } else if fit.working {
                            t!(
                                "{name} · {listed} 个会话",
                                name = crate::lifecycle::here(),
                                listed
                            )
                        } else {
                            crate::lifecycle::here()
                        }),
                )
            })
            .tooltip(super::diagnostics::tip(vec![
                if cut {
                    t!("和后台 Fibo 的连接断了，侧栏和看板停在断开那一刻").into()
                } else {
                    t!("后台 Fibo 在运行，按事件更新，不定时轮询").into()
                },
                detail,
            ]));
        div()
            .id("statusbar")
            .h(px(T::STATUSBAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(T::S2))
            .px(px(T::S6))
            .border_t(px(T::BORDER_WIDTH))
            .border_color(T::color(T::border()))
            .bg(T::surface(T::panel()))
            .text_size(px(T::CHIP_SIZE))
            .font_features(T::tabular())
            .whitespace_nowrap()
            .child(host)
            .child(div().flex_1())
            .children(self.update_item(cx))
            .children(fit.working.then(|| self.hook_item(cx)).flatten())
            .child(self.quota_cells(cx))
            // Only while memory is tight: a click opens 本机诊断, memory row first.
            .when_some(super::pressure::tight(), |d, (pressure, swap)| {
                let tone = if pressure == fibo_core::diagnostics::Pressure::Critical {
                    Tone::Danger
                } else {
                    Tone::Warn
                };
                d.child(divider()).child(
                    item("status-memory", false)
                        .aria_label(t!("内存{label}", label = pressure.label()))
                        .gap(px(T::S4))
                        .child(div().size(px(T::DOT)).rounded_full().bg(tone.dot()))
                        .child(
                            div()
                                .text_color(tone.ink())
                                .child(t!("内存{label}", label = pressure.label())),
                        )
                        .when(swap > 0 && fit.working, |d| {
                            d.child(
                                div()
                                    .text_color(T::color(T::secondary()))
                                    .child(t!("交换 {v1}", v1 = fibo_core::diagnostics::gib(swap))),
                            )
                        })
                        .tooltip(super::diagnostics::tip(vec![
                            t!("系统内存压力{label}", label = pressure.label()),
                            t!("点开本机诊断，看谁占得多").into(),
                        ]))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_diagnostics(window, cx)),
                        ),
                )
            })
            .when(fit.diagnose || fit.settings, |d| d.child(divider()))
            .when(fit.diagnose, |d| {
                d.child(
                    item("diagnose-host", false)
                        .aria_label(t!("本机诊断"))
                        .child(t!("本机诊断"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.show_diagnostics(window, cx)),
                        ),
                )
            })
            // 设置 is at the sidebar's foot; the bar ends on the palette, as in the v3 design.
            .when(fit.settings, |d| {
                d.child(
                    item("open-palette", false)
                        .aria_label(t!("命令面板"))
                        .gap(px(T::S4))
                        .child(
                            div()
                                .text_color(T::color(T::muted()))
                                .child(crate::keymap::keycap(cx, crate::keymap::Action::Search)),
                        )
                        .child(t!("命令"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
                )
            })
    }

    pub(super) fn toggle_status(&mut self, pop: StatusPop, cx: &mut Context<Self>) {
        self.statusbar = if self.statusbar == Some(pop) {
            None
        } else {
            Some(pop)
        };
        cx.notify();
    }

    /// The sessions the sidebar would show: working ones and those that need you.
    fn status_sessions(&self) -> (Vec<&AgentSession>, Vec<&AgentSession>) {
        let needs = self.needs_you();
        let working = self
            .workspace
            .agent_sessions
            .iter()
            .filter(|s| {
                s.current
                    && self.session_hidden(s).is_none()
                    && !self.session_closed(s)
                    && status_of(&self.workspace, s).working()
                    && !needs.iter().any(|n| n.id == s.id)
            })
            .collect();
        (working, needs)
    }

    /// One cell per CLI, the pinned one first and then the tightest: its mark, then each
    /// window as `5h ▬ 42%`, joined by `·`; past `BAR_CELLS` a `+N ▴`. One click target
    /// for the popover, which lists them all.
    fn quota_cells(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let now = now();
        let all = super::redraw::quotas(&self.workspace, now);
        let readings = bar_order(&tightest_first(&all, now), self.quota_pin);
        let on = self.statusbar == Some(StatusPop::Quota);
        let mut b = item("status-quota", on)
            .gap(px(T::S10))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_status(StatusPop::Quota, cx)));
        if readings.is_empty() {
            return b
                .aria_label(t!("额度：还没有读数"))
                .child(div().text_color(T::color(T::muted())).child(t!("额度")))
                .child(caret());
        }
        // The marks tell the cells apart once there are two; alone, the usual rule.
        let marks = true;
        let mut said = Vec::new();
        for (agent, quota, _) in readings.iter().take(BAR_CELLS) {
            let stale = quota.stale(now);
            let mut cell = div()
                .flex()
                .items_center()
                .gap(px(T::S4))
                .when(marks, |d| {
                    d.child(super::board::agent_mark(*agent, T::color(T::secondary())))
                })
                .when(stale, |d| {
                    d.child(div().text_color(T::color(T::muted())).child("*"))
                });
            let windows: Vec<_> = (quota.windows.iter())
                .filter(|w| !w.reset(now) && w.on_bar())
                .collect();
            if windows.is_empty() {
                cell = cell.child(div().text_color(T::color(T::muted())).child(t!("已重置")));
            }
            for (i, w) in windows.iter().enumerate() {
                let (fill, ink) = colors(w, stale);
                said.push(format!("{} {} {}%", agent.name(), w.label(), w.used));
                cell = cell
                    .when(i > 0, |d| {
                        d.child(div().text_color(T::color(T::muted())).child("·"))
                    })
                    .child(div().text_color(T::color(T::muted())).child(w.label()))
                    .child(thin_bar(share(w), fill))
                    .child(
                        div()
                            .text_color(ink)
                            .when(w.used >= NEAR && !stale, |d| {
                                d.font_weight(gpui::FontWeight::SEMIBOLD)
                            })
                            .child(format!("{}%", w.used)),
                    );
            }
            b = b.child(cell);
        }
        b = b.aria_label(t!("额度：{v1}", v1 = said.join(t!("，"))));
        if readings.len() > BAR_CELLS {
            b = b.child(
                div()
                    .text_color(T::color(T::secondary()))
                    .child(format!("+{} ▴", readings.len() - BAR_CELLS)),
            );
        }
        b
    }

    /// The open popover over the bar; a click outside closes it.
    pub(super) fn statusbar_popover(&self, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        let pop = self.statusbar?;
        let panel = div()
            .id("status-popover")
            .absolute()
            .bottom(px(T::STATUSBAR + T::S4))
            .p(px(T::S12))
            .flex()
            .flex_col()
            .gap(px(T::S6))
            .bg(T::color(T::popover()))
            .border(px(T::BORDER_WIDTH))
            .border_color(T::color(T::control()))
            .rounded(px(T::CONTROL_RADIUS))
            .shadow_lg()
            .text_size(px(T::SMALL))
            .font_features(T::tabular())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let panel = match pop {
            StatusPop::Quota => self.quota_popover(panel.right(px(T::S6)).w(px(T::STATUS_POP)), cx),
            StatusPop::Sessions => {
                self.sessions_popover(panel.left(px(T::S6)).w(px(T::STATUS_POP)), cx)
            }
        };
        Some(
            gpui::deferred(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.statusbar = None;
                            cx.notify();
                        }),
                    )
                    .child(panel),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn quota_popover(
        &self,
        mut panel: gpui::Stateful<gpui::Div>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let now = now();
        let all = super::redraw::quotas(&self.workspace, now);
        let readings = tightest_first(&all, now);
        panel = panel.child(heading(
            t!("额度"),
            match self.quota_pin {
                Some(_) => t!("按用得最多排序 · 状态栏每家一格，固定的在最前"),
                None => t!("按用得最多排序 · 状态栏每家一格"),
            },
        ));
        let on_bar: Vec<_> = bar_order(&readings, self.quota_pin)
            .into_iter()
            .take(BAR_CELLS)
            .map(|(a, ..)| a)
            .collect();
        if readings.is_empty() {
            panel = panel.child(label(t!("这一周用过的 Agent 都还没留下额度记录")));
        }
        for (agent, quota, _) in &readings {
            let stale = quota.stale(now);
            let mut head = div().flex().items_baseline().gap(px(T::S6)).child(
                div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                    "{}{}",
                    agent.name(),
                    if stale { " *" } else { "" }
                )),
            );
            if !quota.plan.is_empty() {
                head = head.child(label(quota.plan.clone()));
            }
            head = head
                .child(div().flex_1())
                .child(label(t!("读数 {v1}", v1 = ago(quota.at, now))))
                .child(self.pin_chip(*agent, on_bar.contains(agent), cx));
            let mut block = div()
                .flex()
                .flex_col()
                .gap(px(T::S4))
                .py(px(T::S4))
                .child(head);
            let named =
                (readings.iter()).any(|(_, q, _)| q.windows.iter().any(|w| !w.name.is_empty()));
            let label_w = T::S20 + if named { T::S20 + T::S20 } else { T::S10 };
            for w in &quota.windows {
                let (fill, ink) = colors(w, stale);
                let reset = w.reset(now);
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(T::S8))
                        .text_size(px(T::CHIP_SIZE))
                        .child(
                            div()
                                .w(px(label_w))
                                .text_color(T::color(T::muted()))
                                .child(w.label()),
                        )
                        .child(div().w(px(T::STATUS_POP_GAUGE)).child(gauge(
                            if reset { 0. } else { share(w) },
                            fill,
                            T::METER + 2.,
                        )))
                        .child(
                            div()
                                .w(px(T::S20 + T::S20))
                                .flex()
                                .justify_end()
                                .text_color(ink)
                                .child(if reset {
                                    t!("已重置").into()
                                } else {
                                    format!("{}%", w.used)
                                }),
                        )
                        .child(div().flex_1())
                        .child(label(if reset {
                            t!("等下次使用时更新").into()
                        } else if w.resets_at == 0 {
                            String::new()
                        } else if let Some(spare) = w.spare(now) {
                            t!(
                                "{v1} 后重置 · 还有 {spare}% 没用",
                                v1 = left(w.resets_at, now),
                                spare
                            )
                        } else {
                            t!(
                                "{v1} 后 · {v2} 重置",
                                v1 = left(w.resets_at, now),
                                v2 = upcoming(w.resets_at, now)
                            )
                        })),
                );
            }
            panel = panel.child(
                block
                    .border_t(px(T::BORDER_WIDTH))
                    .border_color(T::color(T::border())),
            );
        }
        panel.child(
            label(t!(
                "读各家留在本机的记录和插件的读数。* 表示读数超过 1 小时"
            ))
            .pt(px(T::S4))
            .border_t(px(T::BORDER_WIDTH))
            .border_color(T::color(T::border())),
        )
    }

    /// 「状态栏显示」 on the CLI the bar shows; 「固定到状态栏」 on the others, shown on
    /// hover. Clicking the pinned one's chip goes back to showing the tightest.
    fn pin_chip(
        &self,
        agent: fibo_core::agent::Agent,
        on_bar: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let pinned = self.quota_pin == Some(agent);
        let text = match (on_bar, pinned) {
            (true, true) => t!("✓ 固定显示"),
            (true, false) => t!("状态栏显示中"),
            _ => t!("固定到状态栏"),
        };
        div()
            .id(gpui::SharedString::from(format!(
                "quota-pin-{}",
                agent.key()
            )))
            .role(gpui::accesskit::Role::Button)
            .aria_label(if pinned {
                t!("取消固定 {name}，改回显示用得最多的", name = agent.name())
            } else {
                t!("状态栏固定显示 {name}", name = agent.name())
            })
            .px(px(T::S6))
            .rounded(px(T::CHIP_RADIUS))
            .text_size(px(T::CHIP_SIZE))
            .cursor_pointer()
            .when(on_bar, |d| {
                d.bg(T::color(T::chip()))
                    .text_color(T::color(T::secondary()))
            })
            .when(!on_bar, |d| d.text_color(T::color(T::muted())))
            .hover(|s| {
                s.bg(T::color(T::button_hover()))
                    .text_color(T::color(T::primary()))
            })
            .child(text)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.quota_pin = if this.quota_pin == Some(agent) {
                    None
                } else {
                    Some(agent)
                };
                save_quota_pin(this.quota_pin);
                cx.notify();
            }))
    }

    /// The pane header's warning when the CLI running there has used up a window.
    pub(super) fn quota_alert(&self, session: &AgentSession) -> Option<String> {
        let now = now();
        let all = super::redraw::quotas(&self.workspace, now);
        let (_, quota) = all.iter().find(|(a, _)| *a == session.runtime)?;
        let w = quota
            .windows
            .iter()
            .filter(|w| !w.reset(now) && w.used >= OVER)
            .max_by_key(|w| w.used)?;
        Some(if w.resets_at > now {
            t!(
                "{name} {label}额度已用 {used}%，{v4}后重置",
                name = session.runtime.name(),
                label = w.label(),
                used = w.used,
                v4 = left(w.resets_at, now)
            )
        } else {
            t!(
                "{name} {label}额度已用 {used}%",
                name = session.runtime.name(),
                label = w.label(),
                used = w.used
            )
        })
    }
}

/// Every CLI with a reading, the one closest to its limit first, with that window.
fn tightest_first(all: &[(fibo_core::agent::Agent, Quota)], now: u64) -> Vec<Reading<'_>> {
    let mut out: Vec<_> = all.iter().map(|(a, q)| (*a, q, q.tightest(now))).collect();
    out.sort_by_key(|(_, _, w)| std::cmp::Reverse(w.map_or(0, |w| w.used)));
    out
}

type Reading<'a> = (fibo_core::agent::Agent, &'a Quota, Option<&'a QuotaWindow>);

/// The bar's cells before the rest fold into `+N ▴`.
const BAR_CELLS: usize = 2;

/// The bar's order: the pinned CLI first while it has a reading, then the others as
/// given (tightest first).
fn bar_order<'a>(
    readings: &[Reading<'a>],
    pin: Option<fibo_core::agent::Agent>,
) -> Vec<Reading<'a>> {
    let mut out = readings.to_vec();
    if let Some(i) = pin.and_then(|p| out.iter().position(|(a, ..)| *a == p)) {
        let pinned = out.remove(i);
        out.insert(0, pinned);
    }
    out
}

fn quota_pin_path() -> Option<std::path::PathBuf> {
    crate::ui::status_colors::config_dir().map(|d| d.join("quota-pin"))
}

pub(super) fn load_quota_pin() -> Option<fibo_core::agent::Agent> {
    let text = std::fs::read_to_string(quota_pin_path()?).ok()?;
    let key = text.trim();
    (!key.is_empty()).then(|| fibo_core::agent::Agent::from_key(key))
}

/// Best effort: a choice that cannot be written still holds until quit.
fn save_quota_pin(pin: Option<fibo_core::agent::Agent>) {
    let Some(path) = quota_pin_path() else { return };
    match pin {
        Some(a) => {
            let _ = path.parent().map(std::fs::create_dir_all);
            let _ =
                crate::ui::status_colors::write_atomic(&path, format!("{}\n", a.key()).as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A quota cell's bar: a thin track, filled to the share used.
fn thin_bar(share: f64, fill: gpui::Hsla) -> gpui::Div {
    div()
        .w(px(T::STATUS_GAUGE))
        .h(px(T::METER))
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .bg(T::color(T::control()))
        .child(
            div()
                .h_full()
                .w(gpui::relative(share as f32))
                .rounded_full()
                .bg(fill),
        )
}

fn share(w: &QuotaWindow) -> f64 {
    f64::from(w.used.min(100)) / 100.
}

/// The bar's fill and the number's ink: green, orange from 80%, red from 95%; gray once stale.
fn colors(w: &QuotaWindow, stale: bool) -> (gpui::Hsla, gpui::Hsla) {
    match w.level() {
        _ if stale => (T::color(T::muted()), T::color(T::muted())),
        "high" => (Tone::Danger.dot(), Tone::Danger.ink()),
        "mid" => (Tone::Warn.dot(), Tone::Warn.ink()),
        _ => (Tone::Ok.dot(), T::color(T::body())),
    }
}

/// Time left as the bar has room for: `2d4h`, `4h20m`, `12m`.
pub(super) fn left(at: u64, now: u64) -> String {
    let s = at.saturating_sub(now);
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else {
        format!("{}m", m.max(1))
    }
}

/// One clickable spot on the bar, filled while its popover is open.
/// What fits in the status bar at a window width. Things give way from the right: 本机诊断
/// first, then ⌘P 命令, the word 后台 and the running count; the dot, 等你 and the quota button
/// stay at every width (quota-meter/statusbar.html).
#[derive(Debug, PartialEq, Eq)]
struct Fit {
    diagnose: bool,
    settings: bool,
    host_label: bool,
    working: bool,
}
/// Below this, 本机诊断 goes (it is also in ⌘P and the app menu).
const FIT_DIAGNOSE: f32 = 820.;
/// Below this, only the dot, 等你 and the quota button stay (⌘P still opens the palette).
const FIT_MINIMAL: f32 = 640.;
impl Fit {
    fn of(width: f32) -> Self {
        let roomy = width >= FIT_MINIMAL;
        Fit {
            diagnose: width >= FIT_DIAGNOSE,
            settings: roomy,
            host_label: roomy,
            working: roomy,
        }
    }
}

pub(super) fn item(id: &'static str, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::accesskit::Role::Button)
        .h(px(T::STATUSBAR - T::S6))
        .px(px(T::S8))
        .flex()
        .items_center()
        .gap(px(T::S6))
        .rounded(px(T::CHIP_RADIUS))
        .text_color(T::color(T::secondary()))
        .cursor_pointer()
        .hover(|s| s.bg(T::color(T::button_hover())))
        .when(on, |d| {
            d.bg(T::color(T::selection()))
                .text_color(T::color(T::primary()))
        })
}

fn caret() -> gpui::Div {
    div()
        .text_size(px(T::S8))
        .text_color(T::color(T::muted()))
        .child("▲")
}

fn heading(title: &'static str, note: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
        .child(div().flex_1())
        .child(label(note))
}

#[cfg(test)]
mod tests {
    use super::{bar_order, left};
    use fibo_core::agent::Agent;
    use fibo_core::quota::Quota;

    #[test]
    fn the_bar_puts_the_pinned_cli_first_then_the_tightest() {
        let q = Quota {
            at: 0,
            plan: String::new(),
            windows: Vec::new(),
        };
        let readings = [
            (Agent::Codex, &q, None),
            (Agent::Claude, &q, None),
            (Agent::Grok, &q, None),
        ];
        let keys = |pin| {
            bar_order(&readings, pin)
                .into_iter()
                .map(|(a, ..)| a.key())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(None), ["Codex", "Claude", "Grok"]);
        assert_eq!(keys(Some(Agent::Grok)), ["Grok", "Codex", "Claude"]);
        // Pinned but no reading this week: the tightest order stands.
        assert_eq!(bar_order(&readings[..2], Some(Agent::Grok)).len(), 2);
        assert!(bar_order(&[], Some(Agent::Claude)).is_empty());
    }

    #[test]
    fn time_left_fits_the_bar() {
        assert_eq!(left(1000 + 2 * 86_400 + 4 * 3600 + 59, 1000), "2d4h");
        assert_eq!(left(1000 + 4 * 3600 + 20 * 60, 1000), "4h20m");
        assert_eq!(left(1000 + 30, 1000), "1m");
        assert_eq!(left(500, 1000), "1m");
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn things_give_way_from_the_right_as_the_window_narrows() {
        let all = Fit {
            diagnose: true,
            settings: true,
            host_label: true,
            working: true,
        };
        assert_eq!(Fit::of(1440.), all);
        assert_eq!(Fit::of(820.), all);
        assert_eq!(
            Fit::of(819.),
            Fit {
                diagnose: false,
                ..all
            }
        );
        assert_eq!(
            Fit::of(639.),
            Fit {
                diagnose: false,
                settings: false,
                host_label: false,
                working: false
            }
        );
    }
}
