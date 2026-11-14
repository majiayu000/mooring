//! Links and path words under the pointer, and opening them.
use super::{Hovered, Pane, PaneEvent};
use crate::terminal::screen;
use fibo_core::protocol::{Reply, Request, rpc};
use gpui::{Context, prelude::*};
use std::path::Path;

impl Pane {
    /// Checks a hovered path word on disk. A relative one also asks the host where the
    /// program is now, then checks again, so a `cd` since the last hover is followed.
    pub(super) fn hover_word(&mut self, word: Option<screen::Link>, cx: &mut Context<Self>) {
        if word == self.links.hover_word {
            return;
        }
        self.links.hover_word = word.clone();
        self.check_hover_path(cx);
        // Asked for absolute paths too: the menu copies a path relative to it.
        let Some(word) = word else { return };
        let socket = self.socket.clone();
        let id = self.terminal.clone();
        cx.spawn(async move |this, cx| {
            let reply = cx
                .background_spawn(async move { rpc(&socket, &Request::TerminalCwd { id }) })
                .await;
            if let Ok(Reply::Cwd(cwd)) = reply {
                this.update(cx, |pane, cx| {
                    pane.cwd = Some(cwd.into());
                    if pane.links.hover_word.as_ref() == Some(&word) {
                        pane.check_hover_path(cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }
    /// The whole word when it names a file, else a piece of a broken one that does.
    fn check_hover_path(&mut self, cx: &mut Context<Self>) {
        let cwd = self.cwd.as_deref();
        let found = self.links.hover_word.as_ref().and_then(|w| {
            [(Some(w.uri.as_str()), true), (w.alone.as_deref(), false)]
                .into_iter()
                .find_map(|(word, whole)| {
                    let word = word?;
                    // An attached host's paths are on its machine: one here by the same name
                    // is another file, so nothing opens from there.
                    let path = screen::resolve_path(word, cwd)
                        .filter(|p| !crate::lifecycle::attached() && p.exists())?;
                    Some(Hovered {
                        path,
                        at: screen::split_position(word).1,
                        whole,
                        cwd: cwd.map(Path::to_owned),
                    })
                })
        });
        if found != self.links.hover_path {
            self.links.hover_path = found;
            cx.notify();
        }
    }
    /// The hovered path that exists, while its word is still on screen where it was.
    pub(super) fn shown_target(&self) -> Option<&Hovered> {
        let word = self.links.hover_word.as_ref()?;
        (self.screen.path_at(word.row, word.col) == Some(word))
            .then_some(self.links.hover_path.as_ref())
            .flatten()
    }
    /// The path under the pointer, for a click or the pane menu.
    pub fn path_target(&self) -> Option<Hovered> {
        self.shown_target().cloned()
    }
    /// While ⌘ is held over a link or path: what a click does, and how to only show it.
    pub(super) fn click_hint(&self) -> Option<String> {
        if !self.links.modifiers.secondary() {
            return None;
        }
        if self.links.hover_link.is_some() {
            return Some(crate::ui::words::local(t!("⌘ 点击在浏览器打开")));
        }
        let target = self.shown_target()?;
        Some(crate::ui::words::local(&super::super::open_path::hint(
            &Self::open_plan(target, self.links.modifiers.alt),
        )))
    }
    pub(super) fn set_modifiers(&mut self, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        let shows = |m: &gpui::Modifiers| (m.secondary(), m.secondary() && m.alt);
        if shows(&modifiers) != shows(&self.links.modifiers) {
            cx.notify();
        }
        self.links.modifiers = modifiers;
    }
    fn open_plan(target: &Hovered, reveal: bool) -> super::super::open_path::Plan {
        super::super::open_path::plan(
            &target.path,
            target.at,
            crate::ui::terminal_text::current().editor,
            reveal,
            super::super::open_path::installed,
        )
    }
    /// What clicking `target` would do, in the words of the menu entry.
    pub fn open_title(target: &Hovered) -> String {
        use super::super::open_path::Plan;
        match Self::open_plan(target, false) {
            Plan::Reveal { .. } => crate::ui::words::local(t!("在访达中显示（可执行文件不运行）")),
            Plan::OpenFolder => crate::ui::words::local(t!("在访达中打开")),
            Plan::OpenFile => t!("打开").into(),
            Plan::Editor { name, line, .. } => t!("在 {name} 打开第 {line} 行", name, line),
            Plan::Terminal { line, .. } => t!("用 $EDITOR 打开第 {line} 行", line),
        }
    }
    /// Opens `target` by the rules in `open_path`; `reveal` only shows it in Finder.
    pub fn open_target(&mut self, target: &Hovered, reveal: bool, cx: &mut Context<Self>) {
        use super::super::open_path::Plan;
        // Every way to open a path comes here: an attached host's file is on its machine,
        // and one here by the same path is another file.
        if crate::lifecycle::attached() {
            let host = crate::lifecycle::here();
            self.show_toast(t!("这个文件在 {host} 上，这里打不开", host), cx);
            return;
        }
        match Self::open_plan(target, reveal) {
            Plan::Reveal { executable } => {
                cx.reveal_path(&target.path);
                if executable {
                    self.show_toast(
                        crate::ui::words::local(t!("可执行文件只在访达中显示，不会运行")),
                        cx,
                    );
                }
            }
            Plan::OpenFolder | Plan::OpenFile => cx.open_with_system(&target.path),
            Plan::Editor {
                name,
                program,
                args,
                ..
            } => {
                let task = cx.background_spawn(async move {
                    let mut child = std::process::Command::new(program)
                        .args(args)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()?;
                    // The tool hands the file to the editor and exits; reap it.
                    child.wait().map(|_| ())
                });
                cx.spawn(async move |this, cx| {
                    if let Err(e) = task.await {
                        this.update(cx, |pane, cx| {
                            pane.show_toast(t!("没能用 {name} 打开：{e}", name, e), cx)
                        })
                        .ok();
                    }
                })
                .detach();
            }
            Plan::Terminal { command, .. } => {
                let cwd = target
                    .path
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_string_lossy()
                    .into_owned();
                cx.emit(PaneEvent::Run { command, cwd });
            }
        }
    }
    /// Copies `text` and says so in the corner.
    pub fn copy_note(&mut self, text: String, note: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.show_toast(note, cx);
    }
}
