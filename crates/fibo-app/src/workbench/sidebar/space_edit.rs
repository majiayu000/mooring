//! Renaming, creating and removing spaces.
use super::*;

impl Workbench {
    /// Turns the space's name in the sidebar into a field: Enter or a click elsewhere
    /// keeps it, Esc puts the old name back.
    pub(in crate::workbench) fn rename_space(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(space) = self.workspace.spaces.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        self.start_space_rename(space, window, cx);
    }
    fn start_space_rename(&mut self, space: Space, window: &mut Window, cx: &mut Context<Self>) {
        self.save_space_name(cx);
        let field = cx.new(|cx| {
            Field::new(t!("空间名称"), &space.name, cx)
                .inline()
                .ends_on_click_out()
        });
        field.update(cx, |f, _| f.select_all());
        let events = cx.subscribe(&field, |this, _, event, cx| match event {
            crate::ui::field::FieldEvent::Submit | crate::ui::field::FieldEvent::ClickOut => {
                this.save_space_name(cx);
                this.nav.pending_focus = true;
            }
            crate::ui::field::FieldEvent::Cancel => {
                this.layout.renaming_space = None;
                this.nav.pending_focus = true;
                cx.notify();
            }
        });
        let blur = cx.on_blur(&field.focus_handle(cx), window, |this, _, cx| {
            this.save_space_name(cx)
        });
        window.focus(&field.focus_handle(cx), cx);
        self.layout.renaming_space = Some((space, field, [events, blur]));
        cx.notify();
    }
    fn save_space_name(&mut self, cx: &mut Context<Self>) {
        let Some((mut space, field, _)) = self.layout.renaming_space.take() else {
            return;
        };
        let name = field.read(cx).text.trim().to_owned();
        // An empty or unchanged name leaves the space as it was.
        if !name.is_empty() && name != space.name {
            space.name = name;
            self.edit(Edit::UpdateSpace(space), cx);
        }
        cx.notify();
    }
    /// Starts a space with no folder and shows it with its name in a field once the host
    /// has it, so it can be named at once. Its terminals start in `$HOME` until 设置目录… in its menu picks one.
    pub(in crate::workbench) fn new_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.space_menu = None;
        let space = Space {
            id: id(),
            name: super::sidebar_spaces::new_space_name(&self.workspace.spaces),
            cwd: String::new(),
        };
        self.edit(Edit::AddSpace(space.clone()), cx);
        self.nav.space = space.id.clone();
        self.nav.tab = None;
        self.sync_panes(cx);
        // The field lives in the space's sidebar row, which is drawn once the host's
        // reply carries the space; focused before that, it would blur and close at once.
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..40 {
                let added = this.update_in(cx, |this, window, cx| {
                    let found = this.workspace.spaces.iter().find(|s| s.id == space.id);
                    if let Some(found) = found.cloned() {
                        this.start_space_rename(found, window, cx);
                        return true;
                    }
                    false
                });
                if !matches!(added, Ok(false)) {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
            }
        })
        .detach();
    }
    /// Asks macOS for a folder: for `target` it becomes that space's directory; with no
    /// target it starts a new space named after the folder and shows it, its empty tab
    /// offering an Agent first. 重命名 in its menu changes the name later.
    pub(in crate::workbench) fn choose_space_dir(
        &mut self,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.layout.space_menu = None;
        // The Finder shows this machine's folders, not the attached one's.
        if crate::lifecycle::attached() {
            self.error = Some(t!(
                "{host} 上的目录不能在访达里选：在空间的目录列表里选，或在终端里 cd 过去",
                host = crate::lifecycle::here()
            ));
            cx.notify();
            return;
        }
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("选择").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, _, cx| {
                let cwd = dir.to_string_lossy().into_owned();
                if let Some(target) = target {
                    if let Some(space) = this.workspace.spaces.iter().find(|s| s.id == target) {
                        this.edit(
                            Edit::UpdateSpace(Space {
                                cwd,
                                ..space.clone()
                            }),
                            cx,
                        );
                    }
                    return;
                }
                let space = Space {
                    id: id(),
                    name: dir
                        .file_name()
                        .map_or_else(|| cwd.clone(), |n| n.to_string_lossy().into_owned()),
                    cwd,
                };
                this.edit(Edit::AddSpace(space.clone()), cx);
                this.nav.space = space.id.clone();
                this.nav.tab = None;
                this.sync_panes(cx);
            });
        })
        .detach();
    }
    /// From the space menu: the first press asks, the second removes the space and
    /// closes its tabs; the host refuses while it still has unsettled todos.
    pub(in crate::workbench) fn remove_space(&mut self, space: &str, cx: &mut Context<Self>) {
        if !self.layout.space_removing {
            self.layout.space_removing = true;
            cx.notify();
            return;
        }
        self.layout.space_removing = false;
        self.layout.space_menu = None;
        if self.nav.space == space
            && let Some(other) = (self.workspace.spaces.iter())
                .find(|s| s.id != space && !self.workspace.space_quiet(&s.id))
        {
            self.nav.space = other.id.clone();
            self.nav.tab = self
                .workspace
                .tabs
                .iter()
                .find(|t| t.space == other.id)
                .map(|t| t.id.clone());
            self.sync_panes(cx);
        }
        self.edit(Edit::RemoveSpace { id: space.into() }, cx);
        cx.notify();
    }
}
