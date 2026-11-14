//! Other machines in the sidebar (docs/multi-machine/spec.md, step 1: look only). Each
//! machine is a heading with how it is connected, then its Agent sessions as that
//! machine's host worked them out: state, word and title come from its cards, never
//! decided again here. A row opens the terminal's screen in the peek overlay, read only.
use super::peek::{Peek, RemoteTitle};
use super::*;
use crate::machines::{Link, Remote};
use crate::terminal::pane::Pane;
use crate::ui::widgets::{dot, label};
use fibo_core::status::Status;
use fibo_remote::protocol::{Card, Op, Phase, Space};

impl Workbench {
    /// One connection per machine in `<config>/machines.json`; with none, nothing runs.
    pub(super) fn start_machines(cx: &mut Context<Self>) -> Vec<Remote> {
        // An attached window is that machine's own: no other machines beside it.
        if crate::lifecycle::attached() {
            return Vec::new();
        }
        let specs = match crate::machines::load() {
            Ok(specs) => specs,
            Err(e) => {
                eprintln!("读取其他电脑配置失败：{e}");
                return Vec::new();
            }
        };
        if specs.is_empty() {
            return Vec::new();
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        specs
            .into_iter()
            .map(|spec| {
                let tx = tx.clone();
                Remote::start(
                    spec,
                    |s| fibo_remote::client::Client::spawn(&s.command()),
                    move || {
                        let _ = tx.unbounded_send(());
                    },
                )
            })
            .collect()
    }

    pub(super) fn machine_rows(&self, recent: bool, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        for (index, remote) in self.machines.iter().enumerate() {
            let view = remote.view.lock().unwrap().clone();
            if view.spec.quiet {
                continue;
            }
            let name = view
                .machine
                .as_ref()
                .map_or_else(|| view.spec.host.clone(), |m| m.name.clone());
            let sessions: Vec<&Card> = view
                .terminals
                .iter()
                // An Agent with no conversation tied to it is listed too, as at home.
                .filter(|c| c.phase != Phase::Shell || c.note.is_some())
                .collect();
            let shells = view.terminals.len() - sessions.len();
            let (note, bad) = match &view.link {
                // A host older than this window still lists; say what it cannot do yet.
                Link::Online => match remote.slot().lock().unwrap().as_ref() {
                    Some(l) if !l.typing => (
                        t!("{len} 个会话 · 旧版，只能看", len = sessions.len()),
                        false,
                    ),
                    Some(l) if !l.managing => (
                        t!("{len} 个会话 · 旧版，不能开关终端", len = sessions.len()),
                        false,
                    ),
                    _ => (t!("{len} 个会话", len = sessions.len()), false),
                },
                Link::Connecting => (t!("连接中…").to_owned(), false),
                Link::Offline(why) => (t!("没连上：{detail}", detail = why), true),
            };
            let heading = self.machine_menu_on(
                div()
                    .id(gpui::SharedString::from(format!("machine-{index}")))
                    .when(recent, |d| d.mx(px(T::S8)))
                    .mt(px(T::S12))
                    .px(px(T::S8))
                    .flex()
                    .items_center()
                    .gap(px(T::S8))
                    .child(
                        label(name.clone())
                            .flex_shrink_0()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_size(px(T::META)),
                    )
                    .child(
                        label(note)
                            .min_w_0()
                            .truncate()
                            .text_size(px(T::CHIP_SIZE))
                            .text_color(if bad {
                                Tone::Danger.ink()
                            } else {
                                T::color(T::muted())
                            }),
                    )
                    .child(div().flex_1())
                    .when(
                        view.link == Link::Online && self.machine_manages(index),
                        |d| {
                            d.child(
                                button(
                                    gpui::SharedString::from(format!("machine-open-{index}")),
                                    t!("＋"),
                                )
                                .aria_label(t!("在 {name} 上开一个新终端", name = &name))
                                .h(px(T::SEGMENT_HEIGHT))
                                .px(px(T::S6))
                                .text_color(T::color(T::secondary()))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let space = this.machine_space_of(index);
                                        this.open_remote_shell(index, space, window, cx)
                                    },
                                )),
                            )
                        },
                    ),
                index,
                cx,
            );
            out.push(heading.into_any_element());
            if view.link != Link::Online {
                continue;
            }
            for card in sessions {
                let status = card.status.as_deref().and_then(Status::from_key);
                let title = shown_title(card);
                let word = card.status_label.clone();
                let shown = self
                    .peek
                    .as_ref()
                    .is_some_and(|p| p.terminal == remote_key(&view, &card.id));
                let open = (index, card.clone(), name.clone());
                out.push(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "machine-{index}-{}",
                            card.id
                        )))
                        .when(recent, |d| d.mx(px(T::S8)))
                        .h(px(T::ROW))
                        .px(px(T::S8))
                        .flex()
                        .items_center()
                        .gap(px(T::S8))
                        .rounded(px(T::CONTROL_RADIUS))
                        .cursor_pointer()
                        .when(shown, |d| d.bg(T::color(T::raised())))
                        .hover(|s| s.bg(T::color(T::raised())))
                        .child(status.map_or_else(
                            || {
                                crate::ui::widgets::unknown_mark(
                                    T::DOT,
                                    T::CHIP_SIZE,
                                    T::color(T::muted()),
                                )
                            },
                            dot,
                        ))
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .when_some(word, |d, w| {
                            d.child(
                                label(w)
                                    .flex_shrink_0()
                                    .text_size(px(T::CHIP_SIZE))
                                    .text_color(
                                        status.map_or(T::color(T::muted()), |g| g.colors().1),
                                    ),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let (index, card, name) = open.clone();
                            this.open_remote_peek(index, card, &name, window, cx);
                        }))
                        .into_any_element(),
                );
            }
            if shells > 0 {
                out.push(
                    label(t!("另有 {shells} 个普通终端", shells))
                        .when(recent, |d| d.mx(px(T::S8)))
                        .px(px(T::S8))
                        .text_size(px(T::CHIP_SIZE))
                        .text_color(T::color(T::muted()))
                        .into_any_element(),
                );
            }
        }
        out
    }

    pub(super) fn open_remote_peek(
        &mut self,
        index: usize,
        card: Card,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(remote) = self.machines.get(index) else {
            return;
        };
        let key = remote_key(&remote.view.lock().unwrap(), &card.id);
        if self.peek.as_ref().is_none_or(|p| p.terminal != key) {
            let slot = remote.slot();
            let id = card.id.clone();
            let pane = cx.new(|cx| Pane::remote(slot, &id, cx));
            cx.observe(&pane, |this, _, cx| this.pane_changed(cx))
                .detach();
            // Typing here is typing: it brings a machine put aside back, as it does a space.
            let host = remote.view.lock().unwrap().spec.host.clone();
            cx.subscribe(&pane, move |this, _, event, cx| {
                if matches!(event, crate::terminal::pane::PaneEvent::Typed) {
                    this.work_began_on_machine(&host, cx);
                }
            })
            .detach();
            let title = shown_title(&card);
            self.peek = Some(Peek {
                terminal: key,
                session: String::new(),
                pane,
                answering: false,
                note: None,
                confirm_end: false,
                remote: Some(RemoteTitle {
                    title,
                    host: name.to_owned(),
                    status: card.status.as_deref().and_then(Status::from_key),
                    machine: index,
                    terminal: card.id.clone(),
                    slot: remote.slot(),
                }),
            });
        }
        if let Some(peek) = &self.peek {
            window.focus(&peek.pane.focus_handle(cx), cx);
        }
        cx.notify();
    }
}

impl Workbench {
    /// 「＋」 on a machine's heading: a shell in a new tab there, in `space` (else where its
    /// host puts it), then that shell in the peek. Named, so a resend opens one.
    pub(super) fn open_remote_shell(
        &mut self,
        index: usize,
        space: Option<Space>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(remote) = self.machines.get(index) else {
            return;
        };
        let Some(linked) = remote.slot().lock().unwrap().clone() else {
            return;
        };
        let name = {
            let view = remote.view.lock().unwrap();
            (view.machine.as_ref()).map_or_else(|| view.spec.host.clone(), |m| m.name.clone())
        };
        let op = open_shell(space.as_ref(), fibo_core::protocol::RequestId::fresh().id);
        let sent = cx.background_spawn(async move {
            linked
                .client
                .request(op, std::time::Duration::from_secs(15))
        });
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let opened = sent.await;
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match opened {
                    Ok(reply) => {
                        let terminal = reply["terminal"].as_str().unwrap_or_default();
                        let card: Card = serde_json::from_value(serde_json::json!({
                            "id": terminal, "space": "", "tab": "", "phase": "shell",
                            "cwd": t!("新终端"),
                        }))
                        .expect("a card needs only these");
                        this.open_remote_peek(index, card, &name, window, cx);
                    }
                    Err(e) => {
                        this.error = Some(t!("没能在 {name} 上开终端：{e}", name = &name, e = e));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// 「关闭终端」 in a remote peek, once confirmed: ends that terminal there and closes
    /// the peek; a refusal stays in the peek's title bar.
    pub(super) fn end_remote(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.peek.as_ref().and_then(|p| p.remote.as_ref()) else {
            return;
        };
        let Some(linked) = r.slot.lock().unwrap().clone().filter(|l| l.managing) else {
            return;
        };
        let op = fibo_remote::protocol::Op::End {
            terminal: r.terminal.clone(),
            request: Some(fibo_core::protocol::RequestId::fresh().id),
        };
        let key = self.peek.as_ref().map(|p| p.terminal.clone());
        let sent = cx.background_spawn(async move {
            linked
                .client
                .request(op, std::time::Duration::from_secs(10))
        });
        cx.spawn(async move |this, cx| {
            let ended = sent.await;
            let _ = this.update(cx, |this, cx| {
                if this.peek.as_ref().map(|p| p.terminal.clone()) != key {
                    return;
                }
                match ended {
                    Ok(_) => this.close_peek(cx),
                    Err(e) => {
                        if let Some(p) = &mut this.peek {
                            p.confirm_end = false;
                            p.note = Some(t!("没关掉：{e}", e));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// The `open` for a shell there. A space picked in this window is named, so one with no
/// folder (its host cannot match it by `cwd`) still gets it; its folder goes too, for a host
/// too old to read `space`. With none, the host picks, as before.
fn open_shell(space: Option<&Space>, request: String) -> Op {
    Op::Open {
        cwd: space.map(|s| s.cwd.clone()).filter(|c| !c.is_empty()),
        space: space.map(|s| s.id.clone()),
        agent: None,
        prompt: None,
        resume: None,
        request: Some(request),
    }
}

/// A peek's key for another machine's terminal: never equal to a terminal of this one.
pub(super) fn remote_key(view: &crate::machines::View, terminal: &str) -> String {
    let machine = view
        .machine
        .as_ref()
        .map_or(view.spec.host.as_str(), |m| m.id.as_str());
    format!("remote:{machine}:{terminal}")
}

/// The final title supplied by the host, also for unbound terminals.
pub(super) fn shown_title(card: &Card) -> String {
    card.title.clone().unwrap_or_else(|| card.id.clone())
}

impl Workbench {
    /// The approval another machine's terminal asks now, as that machine's list says.
    pub(super) fn remote_approval(&self, r: &super::peek::RemoteTitle) -> Option<String> {
        let view = self.machines.get(r.machine)?.view.lock().unwrap();
        let card = view.terminals.iter().find(|c| c.id == r.terminal)?;
        card.approval.clone().filter(|a| !a.is_empty())
    }
}

/// Answers another machine's approval over its link, named so a resend answers once.
pub(super) fn answer_remote(
    r: &super::peek::RemoteTitle,
    question: String,
    allow: bool,
) -> impl std::future::Future<Output = std::io::Result<fibo_core::protocol::Reply>> + use<> {
    use fibo_remote::protocol::Op;
    let linked = r.slot.lock().unwrap().clone();
    let terminal = r.terminal.clone();
    async move {
        let linked = linked.ok_or_else(|| std::io::Error::other(t!("那台电脑没连上#workbench")))?;
        if !linked.answering {
            return Err(std::io::Error::other(t!("那台电脑不让这里回答审批")));
        }
        let op = Op::Answer {
            terminal,
            question,
            allow,
            always: false,
            subject: None,
            always_label: None,
            request: Some(fibo_core::protocol::RequestId::fresh().id),
        };
        linked
            .client
            .request(op, std::time::Duration::from_secs(10))
            .map(|_| fibo_core::protocol::Reply::Ok)
    }
}

#[cfg(feature = "visual-test")]
impl Workbench {
    /// Opens machine `index`'s terminal `id` the way a click on its sidebar row does,
    /// once that machine lists it; false until then.
    pub fn visual_peek_remote(
        &mut self,
        index: usize,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(remote) = self.machines.get(index) else {
            return false;
        };
        let (card, name) = {
            let view = remote.view.lock().unwrap();
            let Some(card) = view.terminals.iter().find(|c| c.id == id).cloned() else {
                return false;
            };
            let name = view
                .machine
                .as_ref()
                .map_or(view.spec.host.clone(), |m| m.name.clone());
            (card, name)
        };
        self.open_remote_peek(index, card, &name, window, cx);
        true
    }
    /// Other machines' cards on the board now: terminal id and column.
    pub fn visual_remote_columns(&self) -> Vec<(String, &'static str)> {
        (self.remote_cards().into_iter())
            .map(|r| (r.card.id, r.column.key()))
            .collect()
    }
    /// Opens the board, as 鸟瞰 does.
    pub fn visual_show_board(&mut self, cx: &mut Context<Self>) {
        self.show_overview(cx);
    }
    /// Drops machine `index`'s card for `terminal` on 做完了 (or back on 要你处理), as a
    /// drag does once the board accepted it.
    pub fn visual_settle_remote(
        &mut self,
        index: usize,
        terminal: &str,
        done: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self
            .remote_cards()
            .into_iter()
            .find(|r| r.index == index && r.card.id == terminal)
        else {
            return;
        };
        let card = super::board_drawer::DraggedCard {
            id: super::board::remote::remote_id(index, terminal),
            title: shown_title(&r.card),
            from: r.column,
        };
        let to = if done {
            super::board::Column::Done
        } else {
            super::board::Column::Needs
        };
        self.settle_remote(&card, to, cx);
    }
    /// The sidebar's 「＋」 on machine `index`'s heading.
    pub fn visual_open_remote_shell(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let space = self.machine_space_of(index);
        self.open_remote_shell(index, space, window, cx);
    }
    /// Into the open peek: keys as you type them, or a mouse report.
    pub fn visual_in_peek(&mut self, text: &str, mouse: bool, cx: &mut Context<Self>) {
        let pane = self.peek.as_ref().expect("a peek is open").pane.clone();
        pane.update(cx, |p, cx| {
            if mouse {
                p.visual_pointer(text, cx);
            } else {
                p.visual_type(text, cx);
            }
        });
    }
    /// The open peek's terminal key, `remote:<machine>:<terminal>` for another machine's.
    pub fn visual_peek_key(&self) -> Option<String> {
        self.peek.as_ref().map(|p| p.terminal.clone())
    }
    /// 「关闭终端」 then 「关闭」 in a remote peek.
    pub fn visual_end_remote(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.peek {
            p.confirm_end = true;
        }
        self.end_remote(cx);
    }
    /// The open peek's pane, and whether its title bar says 只读.
    pub fn visual_peek_pane(&self) -> Option<(Entity<Pane>, bool)> {
        let peek = self.peek.as_ref()?;
        let read_only = (peek.remote.as_ref())
            .is_some_and(|r| !r.slot.lock().unwrap().as_ref().is_some_and(|l| l.typing));
        Some((peek.pane.clone(), read_only))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: &str, cwd: &str) -> Space {
        Space {
            id: id.into(),
            name: id.into(),
            cwd: cwd.into(),
        }
    }
    fn picked(op: Op) -> (Option<String>, Option<String>) {
        match op {
            Op::Open { cwd, space, .. } => (cwd, space),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_shell_opened_in_a_picked_space_names_that_space() {
        // A space with no folder: only its id can put the shell there.
        let blank = space("blank", "");
        assert_eq!(
            picked(open_shell(Some(&blank), "r1".into())),
            (None, Some("blank".into()))
        );
        let own = space("own", "/work/own");
        assert_eq!(
            picked(open_shell(Some(&own), "r2".into())),
            (Some("/work/own".into()), Some("own".into()))
        );
        assert_eq!(picked(open_shell(None, "r3".into())), (None, None));
    }
}
