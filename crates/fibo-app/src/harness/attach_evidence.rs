//! MOORING_ATTACH_EVIDENCE: a window attached (`--attach`) to another machine's host
//! through a forwarded socket works it with this machine's own code: a tab (⌘T), splits
//! (⌘D, ⇧⌘D), an Agent, ⌘W; its home and its Agents' hooks are that machine's. The outer
//! script checks the tabs and layouts on that host.
use super::*;

pub(super) fn start(
    handle: Option<WindowHandle<workbench::Workbench>>,
    evidence: Option<PathBuf>,
    cx: &mut App,
) -> bool {
    let (Some(handle), Some(directory)) = (handle, evidence) else {
        return false;
    };
    if std::env::var_os("MOORING_ATTACH_EVIDENCE").is_none() {
        return false;
    }
    assert!(crate::lifecycle::attached(), "run it with --attach");
    cx.spawn(async move |cx| {
        let executor = cx.background_executor().clone();
        let wait = move |ms| executor.timer(Duration::from_millis(ms));
        std::fs::create_dir_all(&directory).unwrap();
        let shoot = |name: &str, cx: &mut gpui::AsyncApp| {
            cx.update_window(handle.into(), |_, window, app| {
                window.refresh();
                window.draw(app).clear(app);
                let shot = window.render_to_image().unwrap();
                shot.save(directory.join(name)).unwrap();
            })
            .unwrap();
        };
        let state = |cx: &mut gpui::AsyncApp| {
            handle
                .update(cx, |v, _, _| v.visual_attach_state())
                .unwrap()
        };
        // Waits until `ok` holds for this window's copy of the host's workspace.
        macro_rules! until {
            ($what:expr, $ok:expr) => {{
                let ok = $ok;
                let mut now = state(cx);
                for _ in 0..80 {
                    if ok(now) {
                        break;
                    }
                    wait(250).await;
                    now = state(cx);
                }
                if !ok(now) {
                    shoot("attach-failed.png", cx);
                    let why = handle
                        .update(cx, |v, _, _| v.visual_attach_error())
                        .unwrap();
                    panic!("{}: (spaces, tabs, panes) = {now:?}; {why}", $what);
                }
                now
            }};
        }
        let start = until!("the host's workspace", |s: (usize, usize, usize)| s.0 > 0);
        let first_tabs = handle.update(cx, |v, _, _| v.visual_tab_ids()).unwrap();
        // What is that machine's is read there: its home, and its Agents' hooks.
        let home = crate::lifecycle::home_str().expect("the attached machine's home");
        assert_ne!(
            Some(&home),
            fibo_platform::dirs::home_str().as_ref(),
            "this machine's home"
        );
        let hooks = cx
            .background_executor()
            .spawn(async { crate::agent_hooks::check() })
            .await
            .expect("hooks status on the attached machine");
        let hooks: Vec<String> = (hooks.iter())
            .map(|h| format!("{} {:?} {}", h.name, h.state, h.settings))
            .collect();
        assert!(
            hooks.iter().all(|h| h.contains(&home)),
            "hook settings outside {home}: {hooks:?}"
        );
        eprintln!("attach: home {home}; hooks {hooks:?}");
        wait(800).await;
        shoot("attach-1-start.png", cx);
        let key = |id: &'static str, cx: &mut gpui::AsyncApp| {
            assert!(
                handle
                    .update(cx, |v, window, cx| v.visual_shortcut(id, window, cx))
                    .unwrap(),
                "{id} did not run"
            );
        };
        key("new_tab", cx);
        until!("⌘T opened a tab with a shell", |s: (
            usize,
            usize,
            usize
        )| s.1
            == start.1 + 1
            && s.2 == 1);
        wait(1500).await;
        shoot("attach-2-shell.png", cx);
        key("split_right", cx);
        until!("⌘D split it", |s: (usize, usize, usize)| s.2 == 2);
        key("split_down", cx);
        until!("⇧⌘D split again", |s: (usize, usize, usize)| s.2 == 3);
        wait(1500).await;
        shoot("attach-3-split.png", cx);
        key("close", cx);
        until!("⌘W closed a pane", |s: (usize, usize, usize)| s.2 == 2);
        wait(800).await;
        shoot("attach-4-closed.png", cx);
        // A dropped link: the supervisor starts the ssh again and the window carries on.
        let before = crate::attach::visual_link_pid().expect("an ssh -L runs");
        crate::attach::visual_drop_link();
        let mut after = None;
        for _ in 0..120 {
            wait(250).await;
            after = crate::attach::visual_link_pid().filter(|p| *p != before);
            if after.is_some() {
                break;
            }
        }
        let after = after.expect("the ssh -L never came back");
        eprintln!("attach: link dropped (ssh {before}) and back (ssh {after})");
        let agent = handle
            .update(cx, |v, _, cx| v.visual_run_first_agent(cx))
            .unwrap();
        until!("运行 the Agent opened a tab", |s: (
            usize,
            usize,
            usize
        )| s.1
            == start.1 + 2);
        wait(6000).await;
        shoot("attach-5-agent.png", cx);
        // Leaves that machine as it was: every tab opened here closes, its terminals too.
        let opened: Vec<String> = handle
            .update(cx, |v, _, _| v.visual_tab_ids())
            .unwrap()
            .into_iter()
            .filter(|t| !first_tabs.contains(t))
            .collect();
        for id in &opened {
            handle
                .update(cx, |v, _, cx| v.visual_close_tab_id(id, cx))
                .unwrap();
            let gone = |cx: &mut gpui::AsyncApp| {
                !handle
                    .update(cx, |v, _, _| v.visual_tab_ids())
                    .unwrap()
                    .contains(id)
            };
            for _ in 0..40 {
                if gone(cx) {
                    break;
                }
                wait(250).await;
            }
            assert!(gone(cx), "tab {id} opened here did not close");
        }
        eprintln!("attach: closed the {} tabs opened here", opened.len());
        eprintln!("attach PASS: ⌘T, ⌘D, ⇧⌘D, ⌘W and 运行 {agent} on the attached host");
        cx.update(|cx| cx.quit());
    })
    .detach();
    true
}
