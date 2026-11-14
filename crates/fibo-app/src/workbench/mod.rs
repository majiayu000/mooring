use crate::host::store::{CUT_OFF_NOTICE, Store, record_left};
use crate::model::{search::*, session_state::*, time::*};
use crate::ui::app_icon::{AppIcon, Chosen};
use crate::{
    terminal::pane::Pane,
    ui::{
        field::Field,
        theme::{Status, StatusStyle, Theme as T},
        tone::Tone,
        widgets::*,
    },
};
use fibo_core::{model::*, protocol::*};
use gpui::{Context, Entity, Focusable, Window, div, prelude::*, px};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct Workbench {
    away: Option<u64>,
    search: Search,
    store: Store,
    workspace: Workspace,
    facts: frame_facts::FactsCache,
    recent_dirs: recent_dirs::RecentDirs,
    /// A press on the top bar's empty space that has not moved the window yet.
    title_drag: bool,
    error: Option<String>,
    /// The request whose outcome is unknown, for 重试 to send again under its id.
    unsure: Option<Request>,
    /// A session a notification asked for before it was loaded, and the page the user was
    /// on then: opened once it loads, unless the user has moved on.
    pending_route: Option<(String, nav::Page)>,
    /// The older host is being stopped; the app quits and reopens once it is gone.
    restarting: bool,
    /// 「重新连接」 is checking for the host and starting one if it is gone.
    reconnecting: bool,
    nav: Nav,
    overview: OverviewState,
    tasks: TaskUi,
    layout: LayoutState,
    attention: Attention,
    resuming: Resuming,
    diagnostics: Option<Entity<diagnostics::DiagnosticView>>,
    settings: Option<Entity<settings::SettingsView>>,
    sidebar: sidebar_list::SidebarPrefs,
    /// A session's live screen floated over the current tab from the sidebar.
    peek: Option<peek::Peek>,
    redraw: Option<redraw::Redraw>,
    phones: phones::Phones,
    /// Other machines shown under this one's sessions (docs/multi-machine/spec.md).
    machines: Vec<crate::machines::Remote>,
    /// The bottom bar's open popover.
    statusbar: Option<statusbar::StatusPop>,
    /// The CLI the quota button always shows, picked in its popover; `None` shows the
    /// tightest. Kept in `<config>/quota-pin`.
    quota_pin: Option<fibo_core::agent::Agent>,
    /// Which copy button just copied, shown as 「✓ 已复制」 for a moment so the click is
    /// seen to have done something; the number tells a newer copy from an older timer.
    copied: Option<(&'static str, u64)>,
    copied_seq: u64,
    /// A fresh install's welcome where an empty tab would be.
    first_run: first_run::FirstRun,
    hooks: hooks::Hooks,
}
#[derive(Clone)]
struct PaneMenu {
    terminal: String,
    position: gpui::Point<gpui::Pixels>,
    page: MenuPage,
    /// The path right-clicked on, which puts its own entries at the top.
    path: Option<crate::terminal::pane::Hovered>,
}
/// The first of `candidates` that still exists, else the home directory: a directory
/// removed since (a deleted worktree) must not stop a new terminal.
pub(super) fn usable_cwd(candidates: impl IntoIterator<Item = String>) -> String {
    // An attached host's folders are on its machine: that host checks them, not this one.
    if crate::lifecycle::attached() {
        let mut candidates = candidates.into_iter();
        return (candidates.find(|d| d.starts_with('/')))
            .or_else(crate::lifecycle::home_str)
            .unwrap_or_else(|| "/".into());
    }
    candidates
        .into_iter()
        .find(|d| Path::new(d).is_dir())
        .or_else(fibo_platform::dirs::home_str)
        .unwrap_or_else(|| "/".into())
}
/// A space whose folder was set but is gone. A space with no folder set is not missing:
/// its terminals start in `$HOME` until one is chosen.
pub(super) fn space_dir_missing(space: &Space) -> bool {
    !space.cwd.is_empty() && !crate::lifecycle::attached() && !Path::new(&space.cwd).is_dir()
}
/// The space's folder as shown: its own as `~/…`; for one with none, `start` (where it
/// will start: its last folder or the one worked in last anywhere), else home.
pub(super) fn space_dir_label(space: &Space, start: &str) -> String {
    if !space.cwd.is_empty() {
        return home_relative(&space.cwd);
    }
    match start {
        "" => t!("未设目录 · 从 ~ 开始").to_owned(),
        last => t!("从上次的 {dir} 开始", dir = home_relative(last)),
    }
}
/// The new terminal's command line. A resume is the shell's first command, so it cannot be
/// lost when its pane is not on screen yet to type it; `-i` keeps the user's aliases, as
/// typing it would, and the shell stays afterwards.
pub(super) fn launch_argv(shell: String, resume: Option<&str>) -> Vec<String> {
    use fibo_platform::shell;
    match resume {
        Some(command) => shell::run_then_stay(&shell, command, None),
        None => shell::stay(&shell),
    }
}
/// What a new shell types once it is ready: a todo's Agent with its text for the Agent's
/// input, else a plugin action's command, else the Agent picked for a plain new terminal
/// (the empty tab's 「运行 X」, the right-click 新会话 list).
pub(super) fn start_command(
    agent: Option<Agent>,
    task: Option<&Task>,
    pending: Option<&str>,
) -> Option<(String, Option<String>)> {
    if let Some(task) = task {
        return Some((agent.unwrap_or(task.agent).command(), Some(task.prompt())));
    }
    pending
        .map(|c| (c.to_owned(), None))
        .or_else(|| agent.map(|a| (a.command(), None)))
}
/// What a close removes: a whole tab, one pane by its terminal, or the whole host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Closing {
    Tab(String),
    Pane(String),
    /// Restarting an older host: every terminal ends with it.
    Host,
    /// Stopping the Agent in this terminal; the terminal and its shell stay.
    Agent(String),
    /// A pane 标记完成 could not tie to a conversation: closing is all that is left of it.
    Unmatched(String),
}
#[derive(Clone, Copy)]
enum MenuPage {
    Main,
    Sessions,
    Assign,
    Move,
    Why,
}
struct Form {
    kind: FormKind,
    fields: Vec<Entity<Field>>,
    agent: Agent,
    space: String,
    /// A todo's acceptance criteria, one line each in order.
    criteria: Vec<forms::CriterionRow>,
    /// A save was pressed, so a missing title now shows as an error.
    tried: bool,
    /// Saved and waiting for the host's receipt: closes on it, stays open if refused.
    sending: Option<RequestId>,
    /// Typing in any field redraws the form's checks and preview.
    _watch: Vec<gpui::Subscription>,
}
#[derive(Clone, Copy)]
enum FormKind {
    Task,
    Defer,
    TaskDefer,
}
fn id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
fn home_relative(path: &str) -> String {
    match crate::lifecycle::home_str() {
        Some(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}
/// Where a pane runs, short enough for its title: home as `~`, and past two folders only
/// the last two, `~/…/tools/mooring`.
fn short_place(path: &str) -> String {
    let full = home_relative(path);
    let (root, rest) = match full.strip_prefix("~/") {
        Some(rest) => ("~/", rest),
        None => ("/", full.trim_start_matches('/')),
    };
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return full;
    }
    format!("{root}…/{}", parts[parts.len() - 2..].join("/"))
}
/// Whether closing these Agents would break into a turn or leave a question unanswered.
pub(super) fn interrupts(agents: &[(String, Option<Phase>)]) -> bool {
    agents
        .iter()
        .any(|(_, p)| matches!(p, Some(Phase::Working | Phase::Waiting)))
}
/// Split panes that closing a tab takes down; one pane, or a pane or host close, counts none.
pub(super) fn splits_closed(workspace: &Workspace, closing: &Closing) -> usize {
    match closing {
        Closing::Tab(id) => Some(workspace.tab_terminals(id).len())
            .filter(|n| *n > 1)
            .unwrap_or(0),
        _ => 0,
    }
}
fn outcome_label(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Open => t!("未结束").into(),
        Outcome::Completed => t!("已完成").into(),
        Outcome::Abandoned => t!("已放弃").into(),
        Outcome::Deferred { reason, until } => t!(
            "稍后再看：{reason} · {hours} 小时后",
            reason,
            hours = until.saturating_sub(now()).div_ceil(3600)
        ),
    }
}

mod actions;
mod board;
mod board_drawer;
mod cleanup;
mod complete;
mod complete_unbound;
mod context_ring;
mod decorations;
mod diagnostics;
mod divergence;
mod filters;
mod first_run;
mod forms;
mod frame_facts;
mod hooks;
mod layout;
mod machines;
mod recent_dirs;
mod review_queue;
#[cfg(feature = "visual-test")]
pub use layout::ChangesProbe;
use layout::LayoutState;
mod nav;
use nav::Nav;
mod overview;
mod pace;
mod pane_menu;
mod pressure;
use overview::OverviewState;
mod palette;
use palette::Search;
mod peek;
mod phones;
mod redraw;
mod render;
mod requests;
mod resume;
use resume::reopen_candidate;
mod sessions;
mod settings;
use sessions::{Attention, Resuming};
mod sidebar;
use sidebar::closed_recently;
mod sidebar_flat;
mod sidebar_list;
mod sidebar_machine_views;
mod sidebar_machines;
mod sidebar_motion;
mod sidebar_recent;
mod sidebar_spaces;
mod sidebar_subagents;
mod sidebar_wasm;
mod statusbar;
mod statusbar_update;
mod tabs;
mod task_detail;
mod tasks;
use tasks::TaskUi;
#[cfg(test)]
mod tests;
#[cfg(feature = "visual-test")]
mod visual;
#[cfg(feature = "visual-test")]
mod visual_attach;
#[cfg(feature = "visual-test")]
mod visual_unknown;

mod closing;
mod launch_slot;
mod pane_sync;
impl Workbench {
    pub fn new(
        socket: PathBuf,
        workspace: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Opening where you left off is the host's to say; a space put aside is not where
        // to start.
        let space = (workspace.spaces.iter())
            .find(|s| !workspace.space_quiet(&s.id))
            .or_else(|| workspace.spaces.first())
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let tab = workspace
            .tabs
            .iter()
            .find(|t| t.space == space)
            .map(|t| t.id.clone());
        let (status_tx, mut status_rx) = futures::channel::mpsc::unbounded();
        let store = Store::new(socket, status_tx);
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(link) = status_rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        let crate::connection::StatusLink::Status(status) = link else {
                            this.store.take_link(link);
                            // Repaint once the outage is long enough to show its strip.
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(CUT_OFF_NOTICE).await;
                                let _ = this.update(cx, |_, cx| cx.notify());
                            })
                            .detach();
                            cx.notify();
                            return;
                        };
                        let revision = status.workspace_revision;
                        this.follow_plugins(status.plugins_revision, cx);
                        if let Some(view) = &this.settings {
                            let remote = status.remote.clone();
                            view.update(cx, |v, cx| {
                                if v.remote != remote {
                                    v.remote = remote;
                                    cx.notify();
                                }
                            });
                        }
                        this.store.set_recognition(*status);
                        if revision > this.workspace.revision {
                            this.sync(cx);
                        }
                        this.mark_pane_agents(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.observe_global::<crate::desktop::Status>(|_, cx| cx.notify())
            .detach();
        cx.observe_global::<crate::update::Updates>(|_, cx| cx.notify())
            .detach();
        pressure::watch(cx);
        let since = workspace.last_left;
        let overview = show_on_return(since, now(), workspace.settings.away_minutes);
        let search = Search::new(window, cx);
        // 跟随系统 switches with macOS; a fixed choice ignores it.
        cx.observe_window_appearance(window, |_, _, cx| crate::ui::appearance::apply(cx))
            .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                // A hook written from a terminal (`fibo hooks install`, another session)
                // while the window was open would otherwise keep the status bar's
                // "过期了" until the next restart.
                this.read_hooks(cx);
                if let Some(left) = this.away.take() {
                    if show_on_return(left, now(), this.workspace.settings.away_minutes) {
                        this.overview.since = left;
                        this.overview.cleanup.clear();
                        this.overview.welcome = true;
                        this.nav.overview = true;
                        this.nav.pending_focus = true;
                        this.sync_panes(cx);
                    }
                    cx.notify();
                }
            } else if this.away.is_none() {
                let at = now();
                this.away = Some(at);
                let socket = this.store.socket().to_owned();
                cx.background_spawn(async move {
                    record_left(&socket, at);
                })
                .detach();
            }
        })
        .detach();
        cx.on_release(|this, _| record_left(this.store.socket(), this.away.unwrap_or_else(now)))
            .detach();
        let mut this = Self {
            away: None,
            diagnostics: None,
            settings: None,
            sidebar: sidebar_list::SidebarPrefs::load(),
            peek: None,
            redraw: None,
            phones: Default::default(),
            machines: Self::start_machines(cx),
            statusbar: None,
            quota_pin: statusbar::load_quota_pin(),
            copied: None,
            copied_seq: 0,
            first_run: first_run::FirstRun::load(&workspace),
            hooks: Default::default(),
            search,
            store,
            workspace,
            facts: Default::default(),
            recent_dirs: Default::default(),
            title_drag: false,
            error: None,
            unsure: None,
            pending_route: None,
            restarting: false,
            reconnecting: false,
            nav: Nav {
                space,
                tab,
                overview,
                task_detail: None,
                focused: None,
                pending_focus: true,
                zoomed: None,
            },
            overview: OverviewState {
                filter_space: None,
                changed_only: false,
                closed_only: false,
                hover_task: None,
                fresh_todo: None,
                fold_on_release: false,
                was_shown: false,
                since,
                show_stale: false,
                show_closed: false,
                show_running: false,
                show_review: false,
                show_hidden: false,
                agent: None,
                drawer: None,
                drop_over: None,
                lift: None,
                lift_seq: 0,
                shake: None,
                column_width: None,
                spots: Default::default(),
                toast: None,
                toast_seq: 0,
                card_menu: None,
                here: None,
                here_more: None,
                cleanup: vec![],
                cleanup_index: 0,
                cleanup_results: HashMap::new(),
                cleanup_review: false,
                review_closing: None,
                scroll: gpui::ScrollHandle::new(),
                columns: Default::default(),
                welcome: overview,
                focus: cx.focus_handle(),
                #[cfg(feature = "visual-test")]
                toggle_at: None,
            },
            tasks: TaskUi {
                form: None,
                editing_task: None,
                start_after_save: None,
                quick_task: None,
                board_quick: None,
                board_draft_space: None,
                board_draft: String::new(),
                todos_open: false,
                picking: None,
                last_agent: None,
                chosen_agent: None,
                found: None,
                probing: false,
                expanded: HashSet::new(),
                attaching: None,
            },
            layout: LayoutState {
                panes: HashMap::new(),
                drag: None,
                split_bounds: HashMap::new(),
                drop: Default::default(),
                split_target: None,
                pending_command: None,
                menu: None,
                tab_menu: None,
                hovered_tab: None,
                session_menu: None,
                done_open: None,
                subagents_toggled: HashMap::new(),
                subagents_all: HashSet::new(),
                space_menu: None,
                space_removing: false,
                choosing_dir: None,
                space_quieting: false,
                adding_machine_space: None,
                space_switch: false,
                renaming_space: None,
                icon_menu: None,
                confirm_close: None,
                interrupting: None,
                renaming_tab: None,
                tab_scroll: gpui::ScrollHandle::new(),
                revealed_tab: None,
                renaming_session: None,
                changes: HashMap::new(),
                changes_seen: 0,
                reopened: HashSet::new(),
                completing: HashMap::new(),
                completing_seq: 0,
                spots: Default::default(),
            },
            attention: Attention {
                phases: HashMap::new(),
                unseen: HashSet::new(),
            },
            resuming: Resuming {
                resume: None,
                resumed: HashMap::new(),
            },
        };
        this.sync_panes(cx);
        this.read_hooks(cx);
        // A fresh install is greeted where the empty tab is, instead of with a bare shell.
        if this.first_run.on {
            this.start_first_run(cx);
        } else if this.workspace.terminals.is_empty() && !this.nav.overview {
            this.launch(false, None, cx);
        }
        this
    }
    /// Takes a snapshot unless a newer one already arrived by another reply.
    fn apply_workspace(&mut self, mut w: Workspace, cx: &mut Context<Self>) {
        if w.revision < self.workspace.revision {
            return;
        }
        if let Some(id) = self.overview.cleanup.get(self.overview.cleanup_index) {
            let old = self.workspace.agent_sessions.iter().find(|s| &s.id == id);
            let new = w.agent_sessions.iter().find(|s| &s.id == id);
            if old
                .zip(new)
                .is_some_and(|(a, b)| a.outcome != b.outcome && b.outcome != Outcome::Open)
            {
                // Remembered per queued session so the steps show what each one became.
                if let Some(b) = new {
                    self.overview
                        .cleanup_results
                        .insert(id.clone(), b.outcome.clone());
                }
                self.overview.cleanup_index += 1;
            }
        }
        // The host still holds the pre-drag ratio until mouse-up. Keep the local
        // preview while accepting everything else in its snapshot.
        if let Some(drag) = &mut self.layout.drag
            && !drag.preserve_preview(&mut self.workspace, &mut w)
        {
            self.layout.drag = None;
        }
        self.workspace = w;
        self.facts.forget();
        self.close_reviewed(cx);
        self.first_run_follow(cx);
        if let Some(view) = &self.settings {
            view.update(cx, |v, cx| {
                v.values = self.workspace.settings.clone();
                cx.notify();
            });
        }
        self.sync_panes(cx);
    }
    /// Background refresh after the host publishes a newer revision. Runs beside user
    /// actions and coalesces bursts into one follow-up fetch.
    pub(super) fn sync(&mut self, cx: &mut Context<Self>) {
        if !self.store.begin_sync() {
            return;
        }
        let task = cx.background_spawn(self.store.send(Request::Workspace));
        cx.spawn(async move |this, cx| {
            let reply = task.await;
            let _ = this.update(cx, |this, cx| {
                let again = this.store.end_sync();
                match reply {
                    Ok(Reply::Workspace(w)) => this.apply_workspace(w, cx),
                    Ok(_) => (),
                    Err(e) => this.error = Some(e.to_string()),
                }
                if again || this.store.behind(this.workspace.revision) {
                    this.sync(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn launch(&mut self, split: bool, task_id: Option<String>, cx: &mut Context<Self>) {
        if !split
            && self.resuming.resume.is_none()
            && let Some(task) = task_id.as_deref()
            && let Some(terminal) = self.workspace.task_terminal(task)
        {
            let id = terminal.id.clone();
            if let Some(tab) = self.workspace.terminal_tab(&id) {
                self.nav.space = tab.space.clone();
                self.nav.tab = Some(tab.id.clone());
                self.nav.focused = Some(id);
                self.nav.pending_focus = true;
                self.nav.overview = false;
                self.nav.task_detail = None;
                self.sync_panes(cx);
                cx.notify();
                return;
            }
        }
        if let Some(task) = task_id
            .as_ref()
            .and_then(|id| self.workspace.tasks.iter().find(|t| &t.id == id))
        {
            self.nav.space = task.space.clone();
        }
        self.nav.task_detail = None;
        self.launch_in_slot(split, task_id, None, cx);
    }
    /// Puts `text` on the clipboard and marks the button `key` as having copied it.
    pub(super) fn copy_text(&mut self, key: &'static str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copied_seq += 1;
        let seq = self.copied_seq;
        self.copied = Some((key, seq));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.copied.is_some_and(|(_, s)| s == seq) {
                    this.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    /// A copy button's text: its own label, or 「✓ 已复制」 right after it copied.
    pub(super) fn copy_label(&self, key: &'static str, label: &'static str) -> &'static str {
        if self.copied.is_some_and(|(k, _)| k == key) {
            t!("✓ 已复制")
        } else {
            label
        }
    }
}
