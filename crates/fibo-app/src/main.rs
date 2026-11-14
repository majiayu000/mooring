// A window, not a console program: Windows opens no command prompt beside it. Other systems
// ignore the attribute.
#![windows_subsystem = "windows"]
// `t!` and `not_ui!` (fibo_core::i18n) in every module.
#[macro_use]
extern crate fibo_core;

mod agent_hooks;
mod attach;
mod connection;
mod desktop;
#[cfg(feature = "visual-test")]
mod harness;
mod host;
mod keymap;
mod lifecycle;
mod machines;
mod model;
mod terminal;
mod ui;
mod update;
mod workbench;

use ui::theme;

use fibo_core::{model::*, protocol::*};
use gpui::{
    App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use std::{
    io,
    path::{Path, PathBuf},
};

fn ensure_host(root: &Path) -> io::Result<()> {
    if lifecycle::connected_host(&root.join("workbench.sock"))? {
        let Some(stale) = lifecycle::stale_build() else {
            return Ok(());
        };
        // A host that can hand its terminals over is updated at once, terminals and all.
        if stale.handoff {
            lifecycle::spawn_host(root, true)?;
            lifecycle::forget_stale_host();
            return Ok(());
        }
        // An older host with no terminal running is swapped for this app's own at once;
        // one with terminals waits for the user, since a restart ends them.
        let idle = matches!(
            rpc(&root.join("workbench.sock"), &Request::Workspace),
            Ok(Reply::Workspace(w)) if !w.terminals.iter().any(|t| t.state == ProcessState::Running)
        );
        if !idle {
            return Ok(());
        }
        lifecycle::stop_host(root, &stale)?;
        lifecycle::forget_stale_host();
    }
    lifecycle::spawn_host(root, false)
}

gpui::actions!(mooring, [Quit, ShowMenuPanel, ShowSettings]);
/// Fibo → 外观: one item per choice, the current one checked.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = mooring, no_json)]
pub(crate) struct ChooseAppearance(pub usize);
pub(crate) fn set_menus(cx: &mut App) {
    use ui::appearance::{Appearance, Mode};
    let chosen = cx.try_global::<Appearance>().cloned().unwrap_or_default();
    let mut looks = Mode::ALL
        .iter()
        .enumerate()
        .map(|(i, m)| {
            gpui::MenuItem::action(m.label(), ChooseAppearance(i)).checked(*m == chosen.mode)
        })
        .collect::<Vec<_>>();
    // Choosing below would change nothing in this space, so the menu says why.
    if let Some(shown) = ui::appearance::shown(cx)
        && shown.source == ui::appearance::Source::Space
        && let Some(entry) = shown.entry
    {
        looks.push(gpui::MenuItem::separator());
        looks.push(gpui::MenuItem::action(
            t!("当前空间单独用着「{name}」", name = entry.name),
            ShowSettings,
        ));
    }
    // Past the modes, an entry's place in `Themes`: each side has its own check mark.
    if let Some(themes) = cx.try_global::<ui::themes::Themes>() {
        for light in [true, false] {
            let on = chosen.entry(themes, light).map(|entry| entry.id.as_str());
            looks.push(gpui::MenuItem::separator());
            for (i, entry) in themes.entries.iter().enumerate() {
                if entry.light() == light {
                    looks.push(
                        gpui::MenuItem::action(
                            entry.name.as_str(),
                            ChooseAppearance(Mode::ALL.len() + i),
                        )
                        .checked(on == Some(entry.id.as_str())),
                    );
                }
            }
        }
    }
    cx.set_menus([gpui::Menu::new("Fibo").items([
        gpui::MenuItem::action(t!("设置…"), ShowSettings),
        gpui::MenuItem::action(t!("菜单栏面板"), ShowMenuPanel),
        gpui::MenuItem::submenu(gpui::Menu::new(t!("外观")).items(looks)),
        gpui::MenuItem::separator(),
        gpui::MenuItem::action(t!("退出 Fibo"), Quit),
    ])]);
}

/// The top bar is the title bar: the traffic lights sit inside it, over the window's own gray.
fn window_options(bounds: Bounds<gpui::Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Fibo".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(theme::Theme::TRAFFIC_LIGHT_X),
                px(theme::Theme::TRAFFIC_LIGHT_Y),
            )),
        }),
        window_background: ui::backdrop::window_background(),
        ..Default::default()
    }
}
fn load_workspace(root: &Path) -> io::Result<Workspace> {
    ensure_host(root)?;
    let socket = root.join("workbench.sock");
    let mut workspace = match rpc(&socket, &Request::Workspace)? {
        Reply::Workspace(w) => w,
        _ => return Err(io::Error::other("workspace response missing")),
    };
    if workspace.spaces.is_empty() {
        let cwd = default_space_dir()?.to_string_lossy().into_owned();
        if let Reply::Workspace(w) = rpc(
            &socket,
            &Request::Edit {
                request: None,
                edit: Edit::AddSpace(Space {
                    id: "default".into(),
                    name: t!("工作空间").into(),
                    cwd,
                }),
            },
        )? {
            workspace = w;
        }
    }
    Ok(workspace)
}
/// The desktop starts apps somewhere nobody means to open a terminal: use the home directory
/// then, and the launch directory otherwise (`run.sh`, a shell).
fn default_space_dir() -> io::Result<PathBuf> {
    // The first space of an attached host starts in that machine's home.
    if let Some(home) = attach::home() {
        return Ok(PathBuf::from(home));
    }
    let cwd = std::env::current_dir()?;
    Ok(match fibo_platform::dirs::home() {
        Some(home) if fibo_platform::dirs::is_system_start(&cwd) => home,
        _ => cwd,
    })
}
fn reopen_window(root: &Path, cx: &mut App) {
    if let Some(handle) = cx.windows().first().copied() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return;
    }
    match load_workspace(root) {
        Ok(workspace) => {
            let bounds = Bounds::centered(
                None,
                size(
                    px(theme::Theme::WINDOW_WIDTH),
                    px(theme::Theme::WINDOW_HEIGHT),
                ),
                cx,
            );
            if let Err(e) = cx.open_window(window_options(bounds), |window, cx| {
                cx.new(|cx| {
                    workbench::Workbench::new(root.join("workbench.sock"), workspace, window, cx)
                })
            }) {
                eprintln!("reopen: {e}");
            }
            cx.activate(true);
        }
        Err(e) => show_startup_error(e.to_string(), cx),
    }
}
struct StartupError {
    message: String,
    focus: gpui::FocusHandle,
}
impl gpui::Render for StartupError {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::prelude::*;
        use theme::Theme as T;
        gpui::div()
            .size_full()
            .p(px(T::S16))
            .bg(T::color(T::app()))
            .text_color(T::color(T::primary()))
            .text_size(px(T::BODY_SIZE))
            .track_focus(&self.focus)
            .child(self.message.clone())
    }
}
fn show_startup_error(message: String, cx: &mut App) {
    eprintln!("Fibo: {message}");
    if let Err(e) = cx.open_window(WindowOptions::default(), |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            StartupError { message, focus }
        })
    }) {
        eprintln!("error window: {e}");
    }
    cx.activate(true);
}
// Finder launches do not inherit run.sh. Enable the same event-driven idle policy
// before GPUI or any application thread is created. The retained back buffer redraws
// only what changed between frames, so a spinner repaints a few cells, not the window.
fn main() {
    // SAFETY: this is the first operation in main, before spawning threads.
    unsafe {
        std::env::set_var("GPUI_PAUSE_IDLE_DISPLAY_LINK", "1");
        std::env::set_var("GPUI_RETAINED_BACKBUFFER", "1");
    }
    record_panics();
    ui::language::apply();
    if let Err(error) = run() {
        eprintln!("Fibo: {error}");
        std::process::exit(1);
    }
}
/// The isolated runtime of a visual-evidence run, so every way out can stop its host.
#[cfg(feature = "visual-test")]
static EVIDENCE_RUNTIME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
/// A visual-evidence run owns the host it started: the host outlives the app by design
/// (terminals survive a restart), so left alone it keeps the run's shells, and any load
/// a script typed into them, running with nobody attached. Ending the host closes its
/// PTYs and the shells with them.
#[cfg(feature = "visual-test")]
fn stop_evidence_host() {
    let Some(root) = EVIDENCE_RUNTIME.get() else {
        return;
    };
    let escaped: String = root
        .display()
        .to_string()
        .chars()
        .flat_map(|c| {
            let special = ".^$*+?()[]{}|\\".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect();
    let _ = std::process::Command::new("/usr/bin/pkill")
        .args([
            "-TERM",
            "-f",
            &format!("(mooring|fibo) serve --runtime {escaped}( |$)"),
        ])
        .status();
}
/// macOS crash reports keep only the stack, so every panic's message and place is also
/// appended to ~/Library/Logs/Fibo/crash.log. A failed visual-evidence check exits
/// with 101 instead of aborting, so a scripted run raises no "quit unexpectedly" dialog.
fn record_panics() {
    let log = fibo_platform::dirs::home().map(|home| home.join("Library/Logs/Fibo/crash.log"));
    let scripted = std::env::args().any(|arg| arg == "--visual-evidence");
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if let Some(log) = &log {
            use std::io::Write;
            let thread = std::thread::current();
            let entry = format!(
                "[{}] {} pid {} thread {}{}\n{info}\n{}\n\n",
                local_time(),
                std::env::args().collect::<Vec<_>>().join(" "),
                std::process::id(),
                thread.name().unwrap_or("unnamed"),
                if scripted { " (visual evidence)" } else { "" },
                std::backtrace::Backtrace::force_capture(),
            );
            let _ = std::fs::create_dir_all(log.parent().unwrap_or(Path::new("/")));
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
            {
                let _ = file.write_all(entry.as_bytes());
            }
        }
        if scripted {
            #[cfg(feature = "visual-test")]
            stop_evidence_host();
            let _ = io::Write::flush(&mut io::stderr());
            fibo_platform::process::exit_now(101);
        }
    }));
}
fn local_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let t = fibo_platform::clock::local(now);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}
fn run() -> io::Result<()> {
    // Declared CLIs first: menus, settings and commands all read the registry.
    // An install from before the rename to Mooring: its variables and config first.
    fibo_core::legacy::adopt_old_env();
    fibo_core::legacy::move_old_config();
    fibo_core::agent::install();
    fibo_core::action::install();
    fibo_core::rule::install();
    fibo_core::sidebar::install();
    fibo_core::hook::install();
    fibo_core::decoration::install();
    fibo_core::effect::install();
    fibo_core::quota::install();
    let args: Vec<String> = std::env::args().collect();
    // Without the feature the flag would be ignored and the app would just run, so a
    // scripted run waits out its timeout with no picture. Say so and stop instead: the
    // shared target/debug binary is often a plain build made by another checkout.
    #[cfg(not(feature = "visual-test"))]
    if args.iter().any(|a| a == "--visual-evidence") {
        return Err(io::Error::other(t!(
            "这个版本没编进截图场景（visual-test），--visual-evidence 不起作用；用 cargo build -p fibo-app --features fibo-app/visual-test 重编"
        )));
    }
    // `--attach <host>`: another machine's host, through an ssh forward this window keeps.
    let attach = match args.iter().position(|a| a == "--attach") {
        Some(i) => Some(
            (args.get(i + 1).filter(|h| !h.starts_with("--")))
                .ok_or_else(|| io::Error::other("--attach needs a host from machines.json"))?
                .clone(),
        ),
        None => None,
    };
    if let Some(host) = &attach {
        lifecycle::attach(host);
    }
    let root = if let Some(i) = args.iter().position(|a| a == "--runtime") {
        PathBuf::from(
            args.get(i + 1)
                .ok_or_else(|| io::Error::other("--runtime needs a directory"))?,
        )
    } else if let Some(host) = &attach {
        attach::root(host)?
    } else {
        fibo_core::legacy::runtime_dir(
            &fibo_platform::dirs::home().ok_or_else(|| io::Error::other("HOME unavailable"))?,
        )
    };
    fibo_core::plugin_context::set_runtime(&root);
    // An attached window is one beside this machine's own, with its own lock: opening it
    // again brings the same window back, before a second forward starts.
    let instance_dir = match attach {
        Some(_) => root.join("client"),
        None => fibo_platform::dirs::home()
            .ok_or_else(|| io::Error::other("HOME unavailable"))?
            .join(".local/share/mooring-client"),
    };
    // Visual evidence uses its explicitly isolated runtime without forwarding to or
    // replacing the user's running workbench.
    #[cfg(feature = "visual-test")]
    let instance_dir = if args.iter().any(|arg| arg == "--visual-evidence") {
        root.join("visual-client")
    } else {
        instance_dir
    };
    let (_instance, mut reopen) = match lifecycle::Instance::acquire(&instance_dir)? {
        lifecycle::Admission::Forwarded => return Ok(()),
        lifecycle::Admission::Primary(guard, events) => (guard, events),
    };
    if let Some(host) = &attach {
        attach::start(host, &root)?;
    }
    let initial = load_workspace(&root);
    let icon_file = instance_dir.join("app-icon");
    #[cfg(feature = "visual-test")]
    let evidence = args
        .iter()
        .position(|a| a == "--visual-evidence")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    #[cfg(feature = "visual-test")]
    if evidence.is_some() {
        let _ = EVIDENCE_RUNTIME.set(root.clone());
    }
    let app = gpui_platform::application().with_assets(ui::icons::Assets);
    let reopen_root = root.clone();
    app.on_reopen(move |cx| reopen_window(&reopen_root, cx));
    app.run(move |cx: &mut App| {
        use futures::StreamExt;
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_app_quit(|_| async { attach::stop() }).detach();
        #[cfg(feature = "visual-test")]
        cx.on_app_quit(|_| async { stop_evidence_host() }).detach();
        cx.on_action(|_: &ShowMenuPanel, _| desktop::show());
        cx.bind_keys([gpui::KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|choice: &ChooseAppearance, cx| {
            use ui::appearance::{self, Mode};
            let done = if let Some(&mode) = Mode::ALL.get(choice.0) {
                appearance::set_mode(mode, cx)
            } else {
                let entry = cx
                    .try_global::<ui::themes::Themes>()
                    .and_then(|t| t.entries.get(choice.0 - Mode::ALL.len()))
                    .map(|e| (e.id.clone(), e.light()));
                let Some((id, light)) = entry else { return };
                appearance::set_theme(&id, light, true, cx)
            };
            if let Err(error) = done {
                cx.set_global(desktop::Status(error));
            }
            set_menus(cx);
        });
        cx.on_action(|_: &ShowSettings, cx| {
            for handle in cx.windows() {
                if let Some(handle) = handle.downcast::<workbench::Workbench>() {
                    let _ = handle.update(cx, |view, window, cx| {
                        window.activate_window();
                        view.show_settings(window, cx);
                    });
                    break;
                }
            }
        });
        cx.set_global(ui::appearance::Appearance::load());
        ui::themes::start(cx);
        ui::color_migration::run(cx);
        cx.set_global(ui::space_themes::SpaceThemes::load());
        cx.set_global(ui::style::Style::load());
        cx.set_global(ui::peek::PeekChoice::load());
        cx.set_global(ui::hook_hint::HookHint::load());
        cx.set_global(ui::pinned_ask::PinnedAsk::load());
        cx.set_global(ui::running_spin::RunningSpin::load());
        cx.set_global(keymap::Keymap::load());
        ui::appearance::apply(cx);
        update::start(cx);
        set_menus(cx);
        let wake_root = root.clone();
        cx.spawn(async move |cx| {
            while reopen.next().await.is_some() {
                cx.update(|cx| reopen_window(&wake_root, cx));
            }
        })
        .detach();
        let icon = ui::app_icon::AppIcon::load(&icon_file);
        desktop::set_dock_icon(icon.dock_png());
        cx.set_global(ui::app_icon::Chosen {
            icon,
            file: icon_file.clone(),
        });
        if let Err(error) = register_symbols() {
            eprintln!("bundled symbol font: {error}");
            cx.quit();
            return;
        }
        // Ghostty's default face ships inside Ghostty; without a bundled copy, machines
        // lacking an installed JetBrains Mono fall back to Menlo and look different.
        if let Err(error) = cx
            .text_system()
            .add_fonts(ui::theme::BUNDLED_MONO.map(Into::into).to_vec())
        {
            eprintln!("bundled terminal font: {error}");
        }
        let workspace = match initial {
            Ok(w) => w,
            Err(e) => {
                show_startup_error(e.to_string(), cx);
                return;
            }
        };
        let socket = root.join("workbench.sock");
        let mut desktop_events = desktop::start(&socket, workspace.clone(), cx);
        let desktop_root = root.clone();
        cx.spawn(async move |cx| {
            while let Some(route) = desktop_events.next().await {
                cx.update(|cx| {
                    if let Some(message) = route
                        .strip_prefix("error:")
                        .or_else(|| route.strip_prefix("notice:"))
                    {
                        cx.set_global(desktop::Status(message.into()));
                        return;
                    }
                    reopen_window(&desktop_root, cx);
                    if !route.is_empty() {
                        for handle in cx.windows() {
                            if let Some(handle) = handle.downcast::<workbench::Workbench>() {
                                let _ = handle.update(cx, |view, window, cx| {
                                    // The panel's 查看诊断 while the host cannot be reached.
                                    if route == desktop::DIAGNOSTICS {
                                        view.show_diagnostics(window, cx)
                                    } else {
                                        view.navigate_to(&route, window, cx)
                                    }
                                });
                                break;
                            }
                        }
                    }
                });
            }
        })
        .detach();
        let bounds = Bounds::centered(
            None,
            size(
                px(theme::Theme::WINDOW_WIDTH),
                px(theme::Theme::WINDOW_HEIGHT),
            ),
            cx,
        );
        let result = cx.open_window(window_options(bounds), |window, cx| {
            cx.new(|cx| workbench::Workbench::new(socket, workspace, window, cx))
        });
        #[cfg(feature = "visual-test")]
        harness::start(result.as_ref().ok().copied(), evidence.clone(), cx);
        if let Err(e) = result {
            eprintln!("open workbench: {e}");
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}

/// Register only in this process. GPUI's memory font loader skips symbol-only
/// faces without an ASCII `m`; CoreText must see this face for its cascade.
fn register_symbols() -> Result<(), &'static str> {
    fibo_platform::desktop::register_font(include_bytes!(
        "../assets/SymbolsNerdFontMono-Regular.ttf"
    ))
}
