//! One UI process per user. The lock lives independently of the daemon/runtime.
use fibo_platform::local;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use std::{
    fs::{File, TryLockError},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

/// `--attach`: this window works a host it does not own, such as another machine's through
/// a forwarded socket. It never starts, stops or replaces that host, and a build that
/// differs from this app's is not stale here: replacing it would hand its terminals to a
/// host on this machine, or stop whatever local process has its pid.
static ATTACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// This window works `host`'s own host from now on.
pub fn attach(host: &str) {
    let _ = ATTACHED.set(host.to_owned());
}
pub fn attached() -> bool {
    ATTACHED.get().is_some()
}
/// What this window calls the machine whose host it works: 本机, or the attached one.
pub fn here() -> String {
    ATTACHED
        .get()
        .cloned()
        .unwrap_or_else(|| t!("本机").to_owned())
}
/// `$HOME` of the machine whose host this window works: the folders it offers are there.
pub fn home_str() -> Option<String> {
    match attached() {
        true => crate::attach::home(),
        false => fibo_platform::dirs::home_str(),
    }
}
/// What a new terminal starts: the attached machine's login shell, else this one's.
pub fn shell() -> String {
    crate::attach::shell().unwrap_or_else(fibo_platform::shell::interactive)
}
fn not_attached() -> io::Result<()> {
    refuse_attached(attached())
}
fn refuse_attached(attached: bool) -> io::Result<()> {
    match attached {
        true => Err(io::Error::other(t!("挂接的后台不由这个窗口启动或停止"))),
        false => Ok(()),
    }
}
/// A host running other code than this app is stale, unless this window is attached to
/// it: then it is another machine's, and replacing it is that machine's to do.
fn treated_stale(stale: bool, attached: bool) -> bool {
    stale && !attached
}

pub struct Instance {
    _lock: File,
    socket: PathBuf,
}
pub enum Admission {
    Primary(Instance, UnboundedReceiver<()>),
    Forwarded,
}
impl Instance {
    pub fn acquire(directory: &Path) -> io::Result<Admission> {
        fibo_platform::fs::create_private_dir(directory)?;
        let lock = fibo_platform::fs::lock_file(&directory.join("client.lock"))?;
        let socket = directory.join("client.sock");
        // The lock is released by the OS even on a crash; the socket is only a wakeup channel.
        match lock.try_lock() {
            Ok(()) => (),
            Err(TryLockError::Error(error)) => return Err(error),
            Err(TryLockError::WouldBlock) => {
                for _ in 0..40 {
                    if local::Stream::connect(&socket).is_ok() {
                        return Ok(Admission::Forwarded);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                return Err(io::Error::other(t!("已有 Fibo 客户端，但无法唤醒其窗口")));
            }
        }
        match std::fs::remove_file(&socket) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let listener = local::Listener::bind(&socket)?;
        let (tx, rx) = unbounded();
        std::thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok(_) => {
                        if tx.unbounded_send(()).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        eprintln!("client wakeup listener: {error}");
                        break;
                    }
                }
            }
        });
        Ok(Admission::Primary(
            Self {
                _lock: lock,
                socket,
            },
            rx,
        ))
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

pub const OLD_BACKEND: &str = tk!("后台版本过旧，需要重启后台（会中断正在运行的终端）");
pub const RESET_NOTICE: &str = tk!(
    "此版本不迁移旧库。切换到新库后，旧空间、待办、标签布局、终端记录、Agent 会话归属和结局不会保留。原始 CLI 日志文件不会删除。请先处理运行中的终端，再手动备份旧库并切换；本窗口不会重启后台或删除数据。"
);

/// The running host when it runs other code than this app's own; set on connect.
static STALE_HOST: std::sync::Mutex<Option<fibo_core::protocol::Build>> =
    std::sync::Mutex::new(None);
pub fn stale_build() -> Option<fibo_core::protocol::Build> {
    STALE_HOST.lock().ok()?.clone()
}
pub fn stale_host() -> bool {
    stale_build().is_some()
}
/// What to tell a starting terminal past its built-in scheme: an older host reads only
/// that, and neither a built-in nor such a host needs more.
pub fn host_colors() -> Option<fibo_core::terminal::TermColors> {
    let term = crate::ui::theme::Theme::term();
    (fibo_core::terminal::Scheme::of(&term).is_none() && !stale_host()).then_some(term)
}
/// What tells a running terminal's host about the colors.
pub fn scheme_request() -> fibo_core::protocol::Request {
    match host_colors() {
        Some(colors) => fibo_core::protocol::Request::Colors(colors),
        None => fibo_core::protocol::Request::Scheme(crate::ui::theme::Theme::scheme()),
    }
}
fn set_stale_host(build: Option<fibo_core::protocol::Build>) {
    if let Ok(mut stale) = STALE_HOST.lock() {
        *stale = build;
    }
}
pub fn forget_stale_host() {
    set_stale_host(None);
}
#[cfg(feature = "visual-test")]
pub fn visual_stale_host() {
    set_stale_host(Some(Default::default()));
}

/// The host program, which ships beside this one.
pub fn host_program() -> io::Result<PathBuf> {
    let name = format!("fibo{}", std::env::consts::EXE_SUFFIX);
    Ok(std::env::current_exe()?.with_file_name(name))
}

/// Stops the host serving `root` and waits until its socket stops answering; the
/// next start runs this app's host. Its terminals end with it.
pub fn stop_host(root: &Path, host: &fibo_core::protocol::Build) -> io::Result<()> {
    not_attached()?;
    let pid = if host.pid != 0 {
        host.pid
    } else {
        // Hosts before `pid` are found by their exact command line.
        let pattern = format!("{} serve --runtime {}", host.binary, root.display());
        let out = std::process::Command::new("/usr/bin/pgrep")
            .args(["-fx", &pattern])
            .output()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
            .ok_or_else(|| io::Error::other(t!("找不到正在运行的后台进程")))?
    };
    let socket = root.join("workbench.sock");
    // Gone already: its pid may belong to something else by now.
    if local::Stream::connect(&socket).is_err() {
        return Ok(());
    }
    let pid = pid as i32;
    let ask = |pid| {
        let _ = fibo_platform::process::terminate(pid);
    };
    for (end, wait) in [(ask as fn(i32), 50), (fibo_platform::process::kill, 20)] {
        end(pid);
        for _ in 0..wait {
            if local::Stream::connect(&socket).is_err() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(io::Error::other(t!("后台没有在 7 秒内退出")))
}
/// Starts this app's own host for `root` and waits until it serves. With `takeover` it
/// first takes the running terminals over from the host serving there now, which exits.
pub fn spawn_host(root: &Path, takeover: bool) -> io::Result<()> {
    // An attached host is not started from here: what is started is the link to it.
    if attached() {
        return crate::attach::ensure(root);
    }
    if !root.exists() {
        fibo_platform::fs::create_private_dir(root)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("host.log"))?;
    let daemon = host_program()?;
    let digest = fibo_core::protocol::Build::digest(&daemon);
    let mut command = std::process::Command::new(daemon);
    command
        .args(["serve", "--runtime"])
        .arg(root)
        .args(takeover.then_some("--takeover"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    fibo_platform::process::detach(&mut command);
    let mut child = command.spawn()?;
    let socket = root.join("workbench.sock");
    // 20 s either way: the first start of a freshly downloaded app waits on macOS checking
    // the host binary before it runs, and a busy machine adds to it; 5 s showed a stranger
    // 「后台启动超时」 on their first launch while the host came up a moment later.
    let until = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        // During a takeover the old host still answers; wait for this build's.
        let serving = if takeover {
            hello_build(&socket).is_ok_and(|b| Some(&b.digest) == digest.as_ref())
        } else {
            fibo_core::protocol::rpc(&socket, &fibo_core::protocol::Request::Workspace).is_ok()
        };
        if serving {
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("host wait: {e}");
                }
            });
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(if takeover {
                t!(
                    "新后台没能接手终端（{status}），旧后台照常运行；详情见 host.log",
                    status
                )
            } else {
                format!("host exited {status}; see host.log")
            }));
        }
        if std::time::Instant::now() >= until {
            return Err(io::Error::other(t!("后台启动超时，请检查 host.log")));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
/// The build a host reports when greeted.
fn hello_build(socket: &Path) -> io::Result<fibo_core::protocol::Build> {
    use fibo_core::protocol::*;
    let mut stream = local::Stream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    send(&mut stream, CONTROL, &Request::Hello { major: MAJOR })?;
    match receive::<Reply>(&mut stream)? {
        Reply::Hello { build, .. } => Ok(build),
        other => Err(io::Error::other(format!("{other:?}"))),
    }
}
/// Replaces the stale host, then opens this app again once this process has quit; the
/// new process connects to the current host. A host that can hand over keeps every
/// terminal running; an older one is stopped and the new process starts a fresh host.
pub fn restart_host(root: &Path) -> io::Result<()> {
    let host = stale_build().ok_or_else(|| io::Error::other(t!("后台已是最新")))?;
    if host.handoff {
        spawn_host(root, true)?;
    } else {
        stop_host(root, &host)?;
    }
    fibo_platform::process::start_after_exit(
        std::process::id(),
        &std::env::current_exe()?,
        std::env::args_os().skip(1),
    )
}

pub fn connected_host(socket: &Path) -> io::Result<bool> {
    use fibo_core::protocol::*;
    let mut stream = match local::Stream::connect(socket) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    send(&mut stream, CONTROL, &Request::Hello { major: MAJOR })?;
    match receive::<Reply>(&mut stream)? {
        Reply::Hello {
            major: MAJOR,
            build,
            ..
        } => {
            // An already running host keeps serving the code it started with, however
            // often its binary is rebuilt; fixes then look like they never landed.
            let sibling = host_program()?;
            set_stale_host(treated_stale(build.stale(&sibling), attached()).then_some(build));
            Ok(true)
        }
        Reply::Error(message) if message.starts_with("protocol major ") => {
            let major = message
                .split_whitespace()
                .nth(2)
                .and_then(|s| s.parse::<u32>().ok());
            if major.is_some_and(|v| v < MAJOR) {
                let notice = if major == Some(1) {
                    fibo_core::i18n::text(RESET_NOTICE)
                } else {
                    t!(
                        "空间、待办和会话结局沿用现有对象表，不作迁移；重启会中断正在运行的终端。本窗口不会重启后台或删除数据。"
                    )
                };
                let old = fibo_core::i18n::text(OLD_BACKEND);
                Err(io::Error::other(format!("{old}\n\n{notice}")))
            } else {
                Err(io::Error::other(t!(
                    "后台协议不匹配：{message}。后台未被重启。",
                    message
                )))
            }
        }
        other => Err(io::Error::other(t!(
            "无法连接后台：{other}。后台未被重启。",
            other = format!("{other:?}")
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_forwards_and_lock_is_reusable() {
        use futures::StreamExt;
        let dir = std::env::temp_dir().join(format!("kl-instance-{}", std::process::id()));
        let Admission::Primary(owner, mut events) = Instance::acquire(&dir).unwrap() else {
            panic!()
        };
        assert!(matches!(
            Instance::acquire(&dir).unwrap(),
            Admission::Forwarded
        ));
        assert_eq!(futures::executor::block_on(events.next()), Some(()));
        drop(owner);
        assert!(matches!(
            Instance::acquire(&dir).unwrap(),
            Admission::Primary(..)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn old_hosts_are_reported_by_one_handshake_without_mutation() {
        use fibo_core::protocol::*;
        for major in [1, 2] {
            let path = std::env::temp_dir()
                .join(format!("kl-old-host-{}-{major}.sock", std::process::id()));
            let listener = local::Listener::bind(&path).unwrap();
            let peer = std::thread::spawn(move || {
                let mut s = listener.accept().unwrap();
                assert!(matches!(
                    receive::<Request>(&mut s).unwrap(),
                    Request::Hello { major: MAJOR }
                ));
                send(
                    &mut s,
                    CONTROL,
                    &Reply::Error(format!("protocol major {major} required")),
                )
                .unwrap();
            });
            let error = connected_host(&path).unwrap_err().to_string();
            assert!(error.contains(OLD_BACKEND));
            if major == 1 {
                assert!(error.contains("结局不会保留"));
            } else {
                assert!(error.contains("沿用现有对象表"));
            }
            peer.join().unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[cfg(test)]
mod attach_tests {
    use super::*;

    #[test]
    fn an_attached_host_is_never_stopped_started_or_stale() {
        assert!(refuse_attached(true).is_err());
        assert!(refuse_attached(false).is_ok());
        assert!(
            !treated_stale(true, true),
            "another machine's host would be replaced"
        );
        assert!(treated_stale(true, false));
        assert!(!treated_stale(false, false));
    }
}
