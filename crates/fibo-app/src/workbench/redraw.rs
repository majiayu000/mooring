//! What a frame redraws. A terminal printing asks for a frame every time it draws, and
//! re-laying out the sidebar's rows on each was nearly 90% of such a frame (2000
//! sessions, 45 rows listed: 74 M instructions a frame, 9 M without the sidebar). So
//! the sidebar is its own cached view: it draws again when the workbench notifies,
//! never because only a pane did.
use super::*;
use crate::ui::theme::Flat as F;
use fibo_core::{agent::Agent, quota::Quota};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

mod turning;

/// How long the sidebar may show what it drew while the rest of the window keeps
/// drawing: a change that reached the window without a workbench notify (a child view's
/// own state, a clock) shows within this.
const CATCH_UP: Duration = Duration::from_secs(1);

/// The newest account limits per CLI, worked out at most once a second per workspace
/// revision: every pane header asks, and finding them looks at each session's content.
pub(super) fn quotas(w: &Workspace, now: u64) -> Rc<[(Agent, Quota)]> {
    type Seen = Option<(u64, u64, Rc<[(Agent, Quota)]>)>;
    thread_local!(static SEEN: std::cell::RefCell<Seen> = const { std::cell::RefCell::new(None) });
    SEEN.with_borrow_mut(|seen| match seen {
        Some((r, t, q)) if *r == w.revision && *t == now => q.clone(),
        _ => {
            // Plugins' readings are files on this machine: an attached window shows the
            // other machine's, which its host reads from its own sessions.
            let plugins = match crate::lifecycle::attached() {
                true => Vec::new(),
                false => fibo_core::quota::from_plugins(),
            };
            let q: Rc<[_]> = fibo_core::quota::latest(w, &plugins, now).into();
            *seen = Some((w.revision, now, q.clone()));
            q
        }
    })
}

/// Each open conversation's context reading, once a second per revision like `quotas`: every
/// pane header asks each frame, and Claude's are files.
pub(super) fn context_fills(w: &Workspace, now: u64) -> Rc<fibo_core::context_fill::Fills> {
    type Seen = Option<(u64, u64, Rc<fibo_core::context_fill::Fills>)>;
    thread_local!(static SEEN: std::cell::RefCell<Seen> = const { std::cell::RefCell::new(None) });
    SEEN.with_borrow_mut(|seen| match seen {
        Some((r, t, f)) if *r == w.revision && *t == now => f.clone(),
        _ => {
            // Read from transcripts on this machine; another machine's are not here.
            let fills = Rc::new(match crate::lifecycle::attached() {
                true => Default::default(),
                false => fibo_core::context_fill::of(w),
            });
            *seen = Some((w.revision, now, fills.clone()));
            fills
        }
    })
}

/// Tab names, worked out at most once a second per workspace revision, terminal titles
/// and focus: every frame names every tab, and naming one looks at every session.
pub(super) fn tab_name(
    w: &Workspace,
    tab: &Tab,
    titles: &std::collections::HashMap<String, String>,
    focused: Option<&str>,
    now: u64,
) -> String {
    type Key = (
        u64,
        u64,
        Option<String>,
        std::collections::HashMap<String, String>,
    );
    type Seen = Option<(Key, Vec<(Tab, String)>)>;
    thread_local!(static SEEN: std::cell::RefCell<Seen> = const { std::cell::RefCell::new(None) });
    SEEN.with_borrow_mut(|seen| {
        let fresh = seen.as_ref().is_some_and(|((r, t, f, ts), _)| {
            *r == w.revision && *t == now && f.as_deref() == focused && ts == titles
        });
        if !fresh {
            let key = (w.revision, now, focused.map(str::to_owned), titles.clone());
            *seen = Some((key, Default::default()));
        }
        // A rename asks with the title cleared, so the whole tab is the key.
        let names = &mut seen.as_mut().expect("set above").1;
        if let Some((_, name)) = names.iter().find(|(t, _)| t == tab) {
            return name.clone();
        }
        let name = w.tab_title(tab, titles, focused);
        names.push((tab.clone(), name.clone()));
        name
    })
}

pub(super) struct Redraw {
    pub(super) sidebar: Entity<SidebarView>,
    /// An empty view inside the workbench: notifying it redraws the workbench (its
    /// parent) without asking the sidebar to draw again.
    tick: Entity<Tick>,
    marks: Rc<turning::Marks>,
    turning: Entity<turning::Turning>,
}

pub(super) struct Tick;
impl Render for Tick {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub(super) struct SidebarView {
    workbench: gpui::WeakEntity<Workbench>,
    drawn: Cell<Instant>,
    /// The workbench notified since the sidebar last drew, so it draws this frame.
    asked: Cell<bool>,
    /// A catch-up draw is already set for later.
    due: Cell<bool>,
    _watch: gpui::Subscription,
}
impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drawn.set(Instant::now());
        self.asked.set(false);
        #[cfg(feature = "visual-test")]
        FIRED.store(false, std::sync::atomic::Ordering::Relaxed);
        let view = cx.entity();
        self.workbench
            .update(cx, |w, cx| {
                // The column's motion draws it again next frame, it alone; once that
                // settles nothing asks.
                w.sidebar.motion.settle();
                if let Some(r) = &w.redraw {
                    r.marks.clear();
                }
                if w.sidebar.motion.moving() {
                    window.on_next_frame(move |_, cx| ask(&view, cx));
                }
                let name = w
                    .workspace
                    .spaces
                    .iter()
                    .find(|s| s.id == w.nav.space)
                    .map_or(t!("工作空间").to_owned(), |s| s.name.clone());
                w.sidebar_view(&name, cx).map(IntoElement::into_any_element)
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

impl Workbench {
    fn redraw(&mut self, cx: &mut Context<Self>) -> &Redraw {
        self.redraw.get_or_insert_with(|| {
            let workbench = cx.entity();
            let sidebar = cx.new(|cx| SidebarView {
                workbench: workbench.downgrade(),
                drawn: Cell::new(Instant::now()),
                asked: Cell::new(true),
                due: Cell::new(false),
                _watch: cx.observe(&workbench, |view: &mut SidebarView, _, cx| {
                    view.asked.set(true);
                    cx.notify();
                }),
            });
            let marks = Rc::new(turning::Marks::default());
            Redraw {
                sidebar,
                tick: cx.new(|_| Tick),
                turning: cx.new(|_| turning::Turning {
                    marks: marks.clone(),
                }),
                marks,
            }
        })
    }
    /// The sidebar for this frame: drawn again only if the workbench notified, else the
    /// last frame's, with a catch-up draw set for at most `CATCH_UP` from its last.
    pub(super) fn cached_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let view = self.redraw.as_ref()?.sidebar.clone();
        if self.sidebar.collapsed {
            return None;
        }
        let state = view.read(cx);
        let catch_up = (catch_up_on()
            && !self.turning_alone(cx)
            && !state.asked.get()
            && !state.due.replace(true))
        .then(|| CATCH_UP.saturating_sub(state.drawn.get().elapsed()));
        if let Some(wait) = catch_up {
            #[cfg(feature = "visual-test")]
            {
                use std::sync::atomic::Ordering::Relaxed;
                CATCH_UPS.fetch_add(1, Relaxed);
                CHAINED.fetch_add(usize::from(FIRED.load(Relaxed)), Relaxed);
            }
            let later = view.clone();
            let draw = move |cx: &mut gpui::App| ask(&later, cx);
            if wait.is_zero() {
                window.on_next_frame(move |_, cx| draw(cx));
            } else {
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(wait).await;
                    cx.update(draw);
                })
                .detach();
            }
        }
        let recent = self.sidebar.preset().group == fibo_core::sidebar::Grouping::Recent;
        let style = gpui::StyleRefinement::default()
            .w(px(if recent { F::WIDTH } else { T::SIDEBAR }))
            .h_full()
            .min_h_0()
            .flex_shrink_0();
        Some(view.cached(style).into_any_element())
    }
    /// Drawn over the window's content: the status bar's popover, and the empty view a
    /// pane's change redraws through.
    pub(super) fn overlays(&self, cx: &Context<Self>) -> Vec<gpui::AnyElement> {
        let tick = self
            .redraw
            .as_ref()
            .map(|r| r.tick.clone().into_any_element());
        let popover = self
            .statusbar_popover(cx)
            .map(IntoElement::into_any_element);
        popover.into_iter().chain(tick).collect()
    }
    /// A pane drew or changed: the workbench shows its title, bell and header, so it
    /// draws again, the sidebar does not.
    pub(super) fn pane_changed(&mut self, cx: &mut Context<Self>) {
        match self.redraw.as_ref() {
            Some(r) => r.tick.update(cx, |_, cx| cx.notify()),
            None => cx.notify(),
        }
    }
    /// A new pane: redraws through `pane_changed`, and its asks (`$EDITOR` for a
    /// ⌘-clicked `file:line` runs in a new tab) reach the workbench.
    pub(super) fn adopt_pane(pane: &Entity<Pane>, cx: &mut Context<Self>) {
        cx.observe(pane, |this, _, cx| this.pane_changed(cx))
            .detach();
        cx.subscribe(pane, |this, pane, event, cx| match event {
            crate::terminal::pane::PaneEvent::Typed => {
                if !this.workspace.quiet_spaces.is_empty() {
                    let terminal = pane.read(cx).terminal_id().to_owned();
                    this.work_began_on_terminal(&terminal, cx);
                }
            }
            crate::terminal::pane::PaneEvent::Run { command, cwd } => {
                this.layout.pending_command = Some((command.clone(), cwd.clone()));
                this.launch(false, None, cx);
            }
            crate::terminal::pane::PaneEvent::PinnedAsk => cx.notify(),
        })
        .detach();
    }
    /// The start of a render: the sidebar's view exists, and a route that waited for
    /// its session opens. Inside a render a notify does not reach the sidebar, so the
    /// sidebar draws the new place next frame.
    pub(super) fn before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.redraw(cx);
        self.begin_turning_frame();
        if self.pending_route.is_none() {
            return;
        }
        self.open_pending_route(window, cx);
        if self.pending_route.is_none()
            && let Some(view) = self.redraw.as_ref().map(|r| r.sidebar.clone())
        {
            window.on_next_frame(move |_, cx| ask(&view, cx));
        }
    }
}

/// Draws the sidebar next frame. Marked as asked, so the workbench drawing beside it
/// in that frame sets no further catch-up: a catch-up never sets off the next one.
pub(super) fn ask(view: &Entity<SidebarView>, cx: &mut gpui::App) {
    #[cfg(feature = "visual-test")]
    FIRED.store(true, std::sync::atomic::Ordering::Relaxed);
    view.update(cx, |v, cx| {
        v.due.set(false);
        v.asked.set(true);
        cx.notify();
    });
}

#[cfg(feature = "visual-test")]
impl Workbench {
    /// What a frame of the column's motion asks: the sidebar alone.
    pub fn visual_motion_frame(&self, cx: &mut gpui::App) {
        if let Some(r) = self.redraw.as_ref() {
            ask(&r.sidebar, cx);
        }
    }
}
#[cfg(feature = "visual-test")]
static CATCH_UPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// A catch-up fired and the sidebar has not drawn since: one set now is set off by it.
#[cfg(feature = "visual-test")]
static FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "visual-test")]
static CHAINED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Checks turn the catch-up off, so a sidebar that missed a change shows it.
fn catch_up_on() -> bool {
    #[cfg(feature = "visual-test")]
    if std::env::var_os("MOORING_NO_CATCH_UP").is_some() {
        return false;
    }
    true
}

#[cfg(feature = "visual-test")]
impl Workbench {
    /// Catch-up draws set so far, and whether the sidebar waits to draw: a window that
    /// only draws when asked must fall quiet once nothing changes.
    /// The last is how many a catch-up's own frame set: never one in a working build,
    /// however often the window draws by itself.
    pub fn visual_catch_ups(&self, cx: &gpui::App) -> (usize, bool, bool, usize) {
        use std::sync::atomic::Ordering::Relaxed;
        let view = self.redraw.as_ref().map(|r| r.sidebar.read(cx));
        let asked = view.is_some_and(|v| v.asked.get());
        let due = view.is_some_and(|v| v.due.get());
        (CATCH_UPS.load(Relaxed), asked, due, CHAINED.load(Relaxed))
    }
    /// Asks the sidebar to draw next frame, as a catch-up does.
    pub fn visual_ask_sidebar(&self, cx: &mut gpui::App) {
        if let Some(r) = &self.redraw {
            ask(&r.sidebar, cx);
        }
    }
    /// When the sidebar last drew, to check that terminal output leaves it alone.
    pub fn visual_sidebar_drawn(&self, cx: &gpui::App) -> Option<Instant> {
        Some(self.redraw.as_ref()?.sidebar.read(cx).drawn.get())
    }
    /// One change the sidebar must show: done the way a click does it.
    pub fn visual_redraw_step(&mut self, step: &str, window: &mut Window, cx: &mut Context<Self>) {
        let space = |n: usize| self.workspace.spaces[n].id.clone();
        match step {
            "collapse" | "expand" => self.toggle_sidebar(cx),
            "filter" | "unfilter" => self.toggle_space_filter(&space(1), cx),
            "pick-space" => self.pick_space(&space(2), cx),
            "focus" => {
                let id = self.workspace.terminals[5].id.clone();
                self.focus_terminal(&id, window, cx);
            }
            "rename" => {
                let mut space = self.workspace.spaces[3].clone();
                space.name = not_ui!("改过名字的空间").into();
                self.edit(Edit::UpdateSpace(space), cx);
            }
            other => panic!("unknown redraw step {other}"),
        }
    }
    /// Whether the rename step's new space name came back from the host.
    pub fn visual_renamed(&self) -> bool {
        self.workspace
            .spaces
            .iter()
            .any(|s| s.name == not_ui!("改过名字的空间"))
    }
}

#[cfg(test)]
mod tests {
    use super::quotas;
    use fibo_core::agent::Agent;
    use fibo_core::content::Content;
    use fibo_core::model::{AgentSession, Outcome, Workspace};
    use fibo_core::quota::{Quota, Window};

    #[test]
    fn tab_names_follow_the_title_and_revision() {
        use fibo_core::model::Tab;
        let mut w = Workspace::default();
        let tab = |title: &str| Tab {
            id: "t".into(),
            space: "s".into(),
            default_task: None,
            title: title.into(),
            layout: None,
        };
        let titles = std::collections::HashMap::new();
        let name = |w: &Workspace, t: &Tab| super::tab_name(w, t, &titles, None, 5);
        assert_eq!(name(&w, &tab("甲")), "甲");
        // Renaming shows the automatic name with the title cleared, in the same second.
        assert_eq!(name(&w, &tab("")), w.tab_title(&tab(""), &titles, None));
        assert_eq!(name(&w, &tab("乙")), "乙");
        w.revision += 1;
        assert_eq!(name(&w, &tab("甲")), "甲");
    }

    #[test]
    fn quotas_show_a_new_reading_by_the_next_second_or_revision() {
        let reading = |used| Content {
            quota: Some(Quota {
                at: 1000,
                plan: String::new(),
                windows: vec![Window {
                    minutes: 300,
                    used,
                    resets_at: 9000,
                    name: String::new(),
                }],
            }),
            ..Default::default()
        };
        let mut w = Workspace {
            agent_sessions: vec![AgentSession {
                id: "a".into(),
                runtime: Agent::Codex,
                cli_session_id: "a".into(),
                terminal: None,
                current: false,
                task: None,
                outcome: Outcome::Open,
                updated: 0,
                cwd: "/tmp".into(),
                transcript: String::new(),
                binding: None,
                name: String::new(),
            }],
            ..Default::default()
        };
        w.content.insert("a".into(), reading(10));
        let used = |w: &Workspace, now| quotas(w, now)[0].1.windows[0].used;
        assert_eq!(used(&w, 2000), 10);
        w.content.insert("a".into(), reading(90));
        assert_eq!(
            used(&w, 2000),
            10,
            "same second and revision: the worked-out one"
        );
        assert_eq!(used(&w, 2001), 90);
        w.content.insert("a".into(), reading(50));
        w.revision += 1;
        assert_eq!(used(&w, 2001), 50);
    }
}
