//! The editor form: todo and defer.
use super::*;
use crate::ui::field::FieldEvent;

mod card;
mod criteria;
mod preview;

/// One acceptance criterion line in the todo form. `id` stays with the line while its
/// text is edited or moved, so evidence already attached to it is kept.
pub(super) struct CriterionRow {
    pub(super) id: String,
    pub(super) field: Entity<Field>,
    _events: [gpui::Subscription; 2],
}
/// A criterion line being dragged to a new place, drawn under the pointer by its text.
#[derive(Clone)]
pub(super) struct DraggedCriterion {
    index: usize,
    text: String,
}
impl gpui::Render for DraggedCriterion {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(T::CRITERION_ROW))
            .max_w(px(T::TODO_PREVIEW_WIDTH))
            .px(px(T::S12))
            .flex()
            .items_center()
            .rounded(px(T::CONTROL_RADIUS))
            .bg(T::color(T::raised()))
            .border(px(T::BORDER_WIDTH))
            .border_color(T::color(T::card_border()))
            .text_color(T::color(T::primary()))
            .opacity(0.9)
            .child(div().truncate().child(self.text.clone()))
    }
}
/// What stops the todo form: a missing title blocks both saves, a space whose directory
/// is gone blocks 「保存并开始」, and an Agent the login shell cannot find is said up front.
pub(super) struct FormChecks {
    pub(super) title_missing: bool,
    pub(super) dir_missing: bool,
    pub(super) agent_missing: bool,
}
/// The commands among `names` that a login shell finds, functions and aliases included,
/// as a new terminal would; None when the shell is not one we can ask or it does not answer.
fn login_commands(names: &[String]) -> Option<HashSet<String>> {
    use std::process::{Command, Stdio};
    // This machine's shell says nothing of what another machine's has.
    if crate::lifecycle::attached() {
        return None;
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| s.ends_with("/zsh") || s.ends_with("/bash"))?;
    let script: String = names
        .iter()
        .map(|n| {
            let q = shell_quote(n);
            format!("command -v {q} >/dev/null 2>&1 && printf 'fibo-found:%s\\n' {q};")
        })
        .collect();
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    Some(
        out.lines()
            .filter_map(|l| l.strip_prefix("fibo-found:"))
            .map(str::to_owned)
            .collect(),
    )
}
/// A small segmented-control button: raised when chosen.
fn segment(id: impl Into<gpui::ElementId>, chosen: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::accesskit::Role::Button)
        .h(px(T::SEGMENT_HEIGHT))
        .px(px(T::S12))
        .flex()
        .items_center()
        .gap(px(T::S6))
        .rounded(px(T::SEGMENT_RADIUS))
        .text_size(px(T::SMALL))
        .cursor_pointer()
        .text_color(T::color(T::secondary()))
        .hover(|s| s.text_color(T::color(T::primary())))
        .when(chosen, |d| {
            d.bg(T::color(T::popover()))
                .text_color(T::color(T::primary()))
                .font_weight(gpui::FontWeight::MEDIUM)
                .shadow_sm()
        })
}
fn segments() -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .p(px(T::S2))
        .rounded(px(T::SEGMENT_TRACK_RADIUS))
        .bg(T::color(T::raised()))
}
/// A form label with an optional grey note after it and one at the right end.
fn form_label(text: &str, note: Option<&str>, right: Option<String>) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(T::S8))
        .text_size(px(T::SMALL))
        .text_color(T::color(T::secondary()))
        .child(text.to_owned())
        .when_some(note, |d, n| {
            d.child(
                div()
                    .text_size(px(T::META))
                    .text_color(T::color(T::muted()))
                    .child(n.to_owned()),
            )
        })
        .when_some(right, |d, r| {
            d.child(
                div()
                    .ml_auto()
                    .text_size(px(T::META))
                    .text_color(T::color(T::muted()))
                    .child(r),
            )
        })
}
fn hint(text: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .text_size(px(T::META))
        .text_color(T::color(T::muted()))
        .child(text.into())
}
/// A primary or plain button that cannot be pressed now.
fn disabled(b: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    b.opacity(T::DISABLED_OPACITY).cursor_default()
}
pub(super) fn solid(b: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    b.bg(T::color(T::accent()))
        .text_color(T::color(T::on_accent()))
        .font_weight(gpui::FontWeight::SEMIBOLD)
}
pub(super) fn plain(b: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    b.bg(T::color(T::button()))
        .text_color(T::color(T::primary()))
}

impl Workbench {
    pub(super) fn show_form(
        &mut self,
        kind: FormKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nav.overview = false;
        self.tasks.editing_task = None;
        self.sync_panes(cx);
        let fields = match kind {
            FormKind::Defer | FormKind::TaskDefer => vec![
                cx.new(|cx| Field::new(t!("为什么稍后再看（必填）"), "", cx)),
                cx.new(|cx| Field::new(t!("多少小时后再看（必填）"), "24", cx)),
            ],
            FormKind::Task => vec![
                cx.new(|cx| {
                    Field::new(t!("要做什么"), "", cx)
                        .compact()
                        .placeholder(t!("要做什么"))
                }),
                cx.new(|cx| Field::new(t!("说明（可选 · 原样交给 Agent）"), "", cx)),
            ],
        };
        let watch = fields
            .iter()
            .map(|f| cx.observe(f, |_, _, cx| cx.notify()))
            .collect();
        window.focus(&fields[0].focus_handle(cx), cx);
        self.tasks.form = Some(Form {
            kind,
            fields,
            agent: self.workspace.settings.default_agent,
            space: self.attended_or_first(self.nav.space.clone()),
            criteria: vec![],
            tried: false,
            sending: None,
            _watch: watch,
        });
        if matches!(kind, FormKind::Task) {
            self.probe_agents(cx);
        }
        cx.notify();
    }
    /// Asks the login shell once which Agent commands it has, in the background.
    pub(super) fn probe_agents(&mut self, cx: &mut Context<Self>) {
        if self.tasks.found.is_some() || self.tasks.probing {
            return;
        }
        self.tasks.probing = true;
        let names: Vec<String> = Agent::all()
            .into_iter()
            .map(|a| a.executable().to_owned())
            .collect();
        cx.spawn(async move |this, cx| {
            let asked = names.clone();
            let found = cx
                .background_executor()
                .spawn(async move { login_commands(&names) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let now: Vec<String> = Agent::all()
                    .into_iter()
                    .map(|a| a.executable().to_owned())
                    .collect();
                if this.tasks.probe_answered(&asked, &now, found) {
                    this.probe_agents(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    /// Puts a criterion line at `at` (clamped), focused when `focus`.
    pub(super) fn add_criterion(
        &mut self,
        at: usize,
        criterion: Option<&AcceptanceCriterion>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = criterion.map_or("", |c| c.description.as_str());
        let field = cx.new(|cx| {
            Field::new(t!("验收标准"), text, cx)
                .inline()
                .placeholder(t!("写一条完成时怎么核对…"))
        });
        // Enter on a written line opens the next one below it.
        let events = cx.subscribe_in(&field, window, |this, field, event, window, cx| {
            if let FieldEvent::Submit = event
                && let Some(form) = &this.tasks.form
                && let Some(i) = form.criteria.iter().position(|r| &r.field == field)
                && !field.read(cx).text.trim().is_empty()
            {
                this.add_criterion(i + 1, None, true, window, cx);
            }
        });
        let watch = cx.observe(&field, |_, _, cx| cx.notify());
        if focus {
            window.focus(&field.focus_handle(cx), cx);
        }
        if let Some(form) = &mut self.tasks.form {
            let at = at.min(form.criteria.len());
            form.criteria.insert(
                at,
                CriterionRow {
                    id: criterion.map_or_else(id, |c| c.id.clone()),
                    field,
                    _events: [events, watch],
                },
            );
        }
        cx.notify();
    }
    /// Drops the dragged line right before line `before`, or last.
    fn move_criterion(&mut self, from: usize, before: usize, cx: &mut Context<Self>) {
        let Some(form) = &mut self.tasks.form else {
            return;
        };
        if from >= form.criteria.len() || before == from || before == from + 1 {
            return;
        }
        let row = form.criteria.remove(from);
        let to = if before > from { before - 1 } else { before };
        form.criteria.insert(to.min(form.criteria.len()), row);
        cx.notify();
    }
    /// The todo the form would save now; duplicate criteria are merged with their evidence.
    pub(super) fn form_task(&self, form: &Form, cx: &gpui::App) -> Task {
        let existing = self
            .tasks
            .editing_task
            .as_ref()
            .and_then(|id| self.workspace.tasks.iter().find(|t| &t.id == id));
        let acceptance = form
            .criteria
            .iter()
            .map(|row| {
                let description = row.field.read(cx).text.trim().to_owned();
                existing
                    .and_then(|t| t.acceptance.iter().find(|c| c.id == row.id))
                    .map(|c| AcceptanceCriterion {
                        description: description.clone(),
                        ..c.clone()
                    })
                    .unwrap_or_else(|| AcceptanceCriterion {
                        id: row.id.clone(),
                        description: description.clone(),
                        evidence: vec![],
                    })
            })
            .collect();
        Task {
            id: self.tasks.editing_task.clone().unwrap_or_default(),
            space: form.space.clone(),
            title: form.fields[0].read(cx).text.trim().to_owned(),
            description: form.fields[1].read(cx).text.clone(),
            acceptance: merge_criteria(acceptance),
            agent: form.agent,
        }
    }
    pub(super) fn form_checks(&self, form: &Form, cx: &gpui::App) -> FormChecks {
        FormChecks {
            title_missing: form.fields[0].read(cx).text.trim().is_empty(),
            dir_missing: self
                .workspace
                .spaces
                .iter()
                .find(|s| s.id == form.space)
                .is_some_and(space_dir_missing),
            agent_missing: self
                .tasks
                .found
                .as_ref()
                .is_some_and(|f| !f.contains(form.agent.executable())),
        }
    }
    /// Saves the open form; it closes on the host's receipt, and a second press while
    /// that is pending does nothing.
    pub(super) fn save_form(&mut self, start: bool, cx: &mut Context<Self>) {
        let Some(mut form) = self.tasks.form.take_if(|f| f.sending.is_none()) else {
            return;
        };
        let edit = match form.kind {
            FormKind::Defer | FormKind::TaskDefer => {
                let values: Vec<String> = form
                    .fields
                    .iter()
                    .map(|f| f.read(cx).text.clone())
                    .collect();
                let hours = values[1]
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|h| *h > 0 && *h <= 24 * 365);
                if values[0].trim().is_empty() || hours.is_none() {
                    self.error = Some(t!("请写明为什么稍后再看，并填 1–8760 小时后再看").into());
                    self.tasks.form = Some(form);
                    return;
                }
                let outcome = Outcome::Deferred {
                    reason: values[0].clone(),
                    until: now() + hours.unwrap() * 3600,
                };
                if matches!(form.kind, FormKind::TaskDefer) {
                    let Some(id) = self.nav.task_detail.clone() else {
                        return;
                    };
                    Edit::SetTaskOutcome {
                        id,
                        outcome,
                        seen: None,
                    }
                } else {
                    let Some(id) = self.overview.here.clone() else {
                        return;
                    };
                    Edit::SetOutcome {
                        id,
                        outcome,
                        seen: None,
                    }
                }
            }
            FormKind::Task => {
                let checks = self.form_checks(&form, cx);
                if checks.title_missing || (start && checks.dir_missing) {
                    form.tried = true;
                    self.tasks.form = Some(form);
                    cx.notify();
                    return;
                }
                let mut task = self.form_task(&form, cx);
                if task.id.is_empty() {
                    task.id = id();
                }
                if start {
                    self.tasks.start_after_save = Some(task.id.clone());
                }
                if self.tasks.editing_task.is_some() {
                    Edit::UpdateTask(task)
                } else {
                    Edit::AddTask(task)
                }
            }
        };
        form.sending = Some(self.edit_named(edit, cx));
        self.tasks.form = Some(form);
    }
    pub(super) fn form_panel(
        &self,
        form: &Form,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        if !matches!(form.kind, FormKind::Task) {
            return self.defer_panel(form, cx);
        }
        let checks = self.form_checks(form, cx);
        let draft = self.form_task(form, cx);
        div()
            .id("editor-form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(T::OVERVIEW_X))
            .py(px(T::S20))
            // ⌘⏎ from any field saves and starts, as the button says.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let k = &event.keystroke;
                if k.key == "enter" && k.modifiers.secondary() {
                    this.save_form(true, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w(px(T::TODO_FORM_WIDTH))
                    .max_w_full()
                    .mx_auto()
                    .flex()
                    .items_start()
                    .gap(px(T::S16))
                    .child(self.form_card(form, &checks, window, cx))
                    .child(self.form_preview(form, &draft, cx)),
            )
    }
    fn defer_panel(&self, form: &Form, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut panel = div()
            .id("editor-form")
            .m(px(T::S20))
            .p(px(T::S20))
            .w(px(T::FORM_WIDTH))
            .max_w_full()
            .overflow_y_scroll()
            .border(px(T::BORDER_WIDTH))
            .border_color(T::color(T::control()))
            .rounded(px(T::POPOVER_RADIUS))
            .bg(T::color(T::header()))
            .flex()
            .flex_col()
            .gap(px(T::S16))
            .child(div().text_size(px(T::FORM_TITLE)).child(t!("稍后再看")));
        for field in &form.fields {
            panel = panel.child(field.clone());
        }
        panel
            .child(
                div()
                    .flex()
                    .gap(px(T::S8))
                    .child(
                        button("save", t!("保存"))
                            .on_click(cx.listener(|this, _, _, cx| this.save_form(false, cx))),
                    )
                    .child(
                        button("cancel", t!("取消")).on_click(cx.listener(|this, _, _, cx| {
                            this.tasks.form = None;
                            cx.notify();
                        })),
                    ),
            )
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    this.tasks.form = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
    }
    pub(super) fn edit_task_form(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_form(FormKind::Task, window, cx);
        self.fill_task_form(task, window, cx);
        self.tasks.editing_task = Some(task.id.clone());
    }
    /// Puts a todo's title, description, space, Agent and criteria into the open form.
    pub(super) fn fill_task_form(
        &mut self,
        task: &Task,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = &mut self.tasks.form else {
            return;
        };
        f.space = task.space.clone();
        f.agent = task.agent;
        f.criteria.clear();
        for (field, value) in f
            .fields
            .clone()
            .iter()
            .zip([task.title.clone(), task.description.clone()])
        {
            field.update(cx, |f, cx| {
                f.set_text(&value);
                cx.notify();
            });
        }
        for (i, criterion) in task.acceptance.iter().enumerate() {
            self.add_criterion(i, Some(criterion), false, window, cx);
        }
    }
}
