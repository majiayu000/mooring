//! Where a space with no folder starts, and the folders it can switch to: a new space
//! starts in the folder you worked in last; 「换目录」 under the empty tab's title lists
//! the others, and one press makes it the space's folder.
use super::*;
use std::{cell::RefCell, rc::Rc};

/// How many folders 「换目录」 lists.
const SHOWN: usize = 6;

/// Core's offered folders as of a workspace revision: the empty tab draws each frame,
/// the list only changes with the workspace.
#[derive(Default)]
pub(super) struct RecentDirs(RefCell<Option<(Key, Rc<Vec<String>>)>>);
/// The workspace revision and how many sessions it holds.
type Key = (u64, usize);

impl Workbench {
    fn recent_dirs(&self) -> Rc<Vec<String>> {
        let key = (self.workspace.revision, self.workspace.agent_sessions.len());
        let mut cache = self.recent_dirs.0.borrow_mut();
        if let Some((at, dirs)) = cache.as_ref()
            && *at == key
        {
            return Rc::clone(dirs);
        }
        let home = crate::lifecycle::home_str();
        let dirs = Rc::new(self.workspace.offered_dirs(home.as_deref(), SHOWN + 1));
        *cache = Some((key, Rc::clone(&dirs)));
        dirs
    }
    /// `Workspace::space_start`, from the cached list.
    fn space_start_shown(&self, space: &Space) -> String {
        match self.workspace.space_start_dir(&space.id) {
            "" => self.recent_dirs().first().cloned().unwrap_or_default(),
            own => own.to_owned(),
        }
    }
    /// Where the space's terminals start, as every surface names it.
    pub(super) fn space_label(&self, space: &Space) -> String {
        space_dir_label(space, &self.space_start_shown(space))
    }
    /// The empty tab's line under its title: where it starts, and 「换目录」.
    pub(super) fn space_dir_line(&self, space: &Space, cx: &Context<Self>) -> gpui::AnyElement {
        let open = self.layout.choosing_dir.as_deref() == Some(space.id.as_str());
        let id = space.id.clone();
        div()
            .flex()
            .items_center()
            .gap(px(T::S6))
            .child(label(self.space_label(space)))
            .child(
                button(
                    "empty-tab-change-dir",
                    if open { t!("收起") } else { t!("换目录") },
                )
                .border_0()
                .px(px(T::S4))
                .text_size(px(T::SMALL))
                .text_color(T::color(T::link()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let open = this.layout.choosing_dir.as_deref() == Some(id.as_str());
                    this.layout.choosing_dir = (!open).then(|| id.clone());
                    cx.notify();
                })),
            )
            .into_any_element()
    }
    /// Opened by 「换目录」: the other recent folders, then 其他目录… for the system
    /// picker; a press makes it the space's folder.
    pub(super) fn dir_picker(&self, space: &Space, cx: &Context<Self>) -> Option<gpui::AnyElement> {
        if self.layout.choosing_dir.as_deref() != Some(space.id.as_str()) {
            return None;
        }
        let start = self.space_start_shown(space);
        let mut rows = vec![];
        let others = self.recent_dirs();
        for (n, dir) in others
            .iter()
            .filter(|d| **d != start)
            .take(SHOWN)
            .enumerate()
        {
            let pinned = Space {
                cwd: dir.clone(),
                ..space.clone()
            };
            rows.push(
                button(format!("empty-tab-dir-{n}"), home_relative(dir))
                    .h(px(T::ROW * 0.8))
                    .border_0()
                    .px(px(T::S8))
                    .text_size(px(T::SMALL))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.layout.choosing_dir = None;
                        this.edit(Edit::UpdateSpace(pinned.clone()), cx);
                    }))
                    .into_any_element(),
            );
        }
        let other = space.id.clone();
        rows.push(
            button("empty-tab-dir-other", t!("其他目录…"))
                .h(px(T::ROW * 0.8))
                .border_0()
                .px(px(T::S8))
                .text_size(px(T::SMALL))
                .text_color(T::color(T::link()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.layout.choosing_dir = None;
                    this.choose_space_dir(Some(other.clone()), window, cx);
                }))
                .into_any_element(),
        );
        Some(
            div()
                .flex()
                .flex_col()
                .items_center()
                .children(rows)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_folders_first_each_once_home_and_gone_left_out() {
        let base = std::env::temp_dir().join(format!("kl-rd-{}", std::process::id()));
        let dir = |n: &str| {
            let d = base.join(n);
            std::fs::create_dir_all(&d).unwrap();
            d.to_string_lossy().into_owned()
        };
        let (old, new, pinned, last, home) =
            (dir("old"), dir("new"), dir("pin"), dir("last"), dir("home"));
        let mut w = Workspace::default();
        let session = |id: &str, updated: u64, cwd: &str| AgentSession {
            id: id.into(),
            runtime: Agent::Codex,
            cli_session_id: id.into(),
            terminal: None,
            current: false,
            task: None,
            outcome: Outcome::Completed,
            updated,
            cwd: cwd.into(),
            transcript: String::new(),
            binding: None,
            name: String::new(),
        };
        w.agent_sessions = vec![
            session("a", 1, &old),
            session("b", 5, &format!("{new}/")),
            session("c", 3, &old),
            session("d", 9, &home),
            session("e", 8, "/nonexistent/kl-gone"),
        ];
        w.spaces.push(Space {
            id: "p".into(),
            name: "p".into(),
            cwd: pinned.clone(),
        });
        w.space_last_cwd.insert("q".into(), last.clone());
        assert_eq!(
            w.offered_dirs(Some(&home), 9),
            [new.clone(), old, pinned, last]
        );
        // A space with nothing of its own starts where you worked last.
        assert_eq!(w.space_start("fresh", Some(&home)), new);
        let _ = std::fs::remove_dir_all(base);
    }
}
