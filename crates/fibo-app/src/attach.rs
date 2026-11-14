//! `--attach <host>`: this window works another machine's own host, with the same code as
//! this machine's (docs/decisions/2026-11-14-one-window-many-hosts.md). An `ssh -L` keeps
//! that host's socket at `<runtime>/m/<host>/workbench.sock` here; a thread starts
//! it again whenever it ends, until the window quits. The host there is never started,
//! stopped or replaced from here (`lifecycle::attached`).
use crate::machines::Spec;
use fibo_platform::local;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Forward {
    spec: Spec,
    /// That host's socket on its machine, as it said (`fibo socket`).
    remote: String,
    /// `$HOME` there.
    home: String,
    /// `$SHELL` there: what its terminals start, as its own window's do.
    shell: String,
    local: PathBuf,
    child: Mutex<Option<Child>>,
}
static FORWARD: OnceLock<Forward> = OnceLock::new();
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Where this machine keeps the forwarded socket of `host`.
pub fn root(host: &str) -> io::Result<PathBuf> {
    let home = fibo_platform::dirs::home().ok_or_else(|| io::Error::other("HOME unavailable"))?;
    Ok(fibo_core::legacy::runtime_dir(&home)
        .join("m")
        .join(dir_name(host)))
}
/// `user@host` or an ssh alias as one short folder name: a readable start and a hash of
/// the whole destination, so `alice@server` and `alice-server` never share a socket or a
/// lock, and the socket path stays under the 104-byte limit however long the host is.
fn dir_name(host: &str) -> String {
    let start: String = (host.chars())
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    // FNV-1a: the same name on every run and every build.
    let hash = (host.bytes()).fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{start}-{:08x}", hash as u32)
}

/// Asks `host` where its host listens, starts the forward and keeps it up.
pub fn start(host: &str, root: &Path) -> io::Result<()> {
    let spec = (crate::machines::load().unwrap_or_default().into_iter())
        .find(|s| s.host == host)
        .unwrap_or_else(|| Spec::new(host.to_owned()));
    let (remote, home, shell) = remote_socket(&spec)?;
    fibo_platform::fs::create_private_dir(root)?;
    let forward = Forward {
        spec,
        remote,
        home,
        shell,
        local: root.join("workbench.sock"),
        child: Mutex::new(None),
    };
    if FORWARD.set(forward).is_err() {
        return Err(io::Error::other("attached twice"));
    }
    std::thread::spawn(supervise);
    Ok(())
}

/// `$HOME` on the attached machine.
pub fn home() -> Option<String> {
    FORWARD.get().map(|f| f.home.clone())
}
/// `fibo` on the attached machine as its terminals reach it: the path written down for it,
/// else the name on their PATH.
pub fn program() -> Option<String> {
    let forward = FORWARD.get()?;
    Some(
        forward
            .spec
            .path
            .clone()
            .unwrap_or_else(|| fibo_core::cli_entry::NAME.to_owned()),
    )
}
/// `$SHELL` on the attached machine.
pub fn shell() -> Option<String> {
    FORWARD.get().map(|f| f.shell.clone())
}

/// `fibo <args>` on the attached machine, as the program finds it there, quoted.
pub fn run(args: &[&str]) -> Option<io::Result<std::process::Output>> {
    let forward = FORWARD.get()?;
    let quoted: Vec<String> = args
        .iter()
        .map(|a| fibo_platform::shell::quote_posix(a))
        .collect();
    let script = format!(
        "{}; exec \"$p\" {}",
        forward.spec.find_program(),
        quoted.join(" ")
    );
    let argv = forward.spec.ssh(&script);
    Some(
        Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .output(),
    )
}

/// Waits until the forwarded socket answers; what `spawn_host` does for an attached host.
pub fn ensure(root: &Path) -> io::Result<()> {
    let forward = FORWARD
        .get()
        .ok_or_else(|| io::Error::other("not attached"))?;
    let socket = root.join("workbench.sock");
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if local::Stream::connect(&socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other(t!(
        "连不上 {host} 上的 Fibo：ssh 转发没有在 20 秒内接通",
        host = &forward.spec.host
    )))
}

/// 在新窗口打开: this app again, attached to `host`; one already open for it comes forward
/// instead (its own instance lock).
pub fn open_window(host: &str) -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--attach", host])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    fibo_platform::process::detach(&mut command);
    command.spawn().map(drop)
}

/// Ends the forward as the window quits.
pub fn stop() {
    if let Some(forward) = FORWARD.get()
        && let Some(mut child) = forward.child.lock().unwrap().take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// `fibo socket` there; a Fibo from before it has none, so its socket is found as that
/// version finds its runtime (`legacy::runtime_dir`).
/// Its home and login shell come back on the lines after: the folders this window offers
/// there, and what its terminals start.
fn remote_socket(spec: &Spec) -> io::Result<(String, String, String)> {
    let script = format!(
        "{}; {{ \"$p\" socket 2>/dev/null || for d in fibo-workbench mooring-workbench keepline-workbench; \
         do [ -d \"$HOME/.local/share/$d\" ] && {{ echo \"$HOME/.local/share/$d/workbench.sock\"; break; }}; done; }}; \
         echo \"$HOME\"; echo \"${{SHELL:-/bin/sh}}\"",
        spec.find_program()
    );
    let argv = spec.ssh(&script);
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut lines = text.lines().map(str::trim);
    let (path, home) = (
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
    );
    // The script answers `${SHELL:-/bin/sh}`, so an empty line is a broken answer.
    let shell = lines.next().unwrap_or_default();
    if ![path, home, shell].iter().all(|l| l.starts_with('/')) {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(io::Error::other(t!(
            "在 {host} 上找不到 Fibo 的后台：{why}",
            host = &spec.host,
            why
        )));
    }
    Ok((path.to_owned(), home.to_owned(), shell.to_owned()))
}

/// Runs the `ssh -L` and starts it again after it ends, waiting longer each time it ends
/// quickly, until `stop` takes it.
fn supervise() {
    let Some(forward) = FORWARD.get() else {
        return;
    };
    let mut backoff = Duration::from_millis(500);
    loop {
        let mut argv = forward.spec.ssh_options("-N");
        argv.extend([
            "-o".into(),
            "ExitOnForwardFailure=yes".into(),
            "-o".into(),
            "StreamLocalBindUnlink=yes".into(),
            "-L".into(),
            format!("{}:{}", forward.local.display(), forward.remote),
            forward.spec.host.clone(),
        ]);
        let child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let started = Instant::now();
        match child {
            Ok(child) => *forward.child.lock().unwrap() = Some(child),
            Err(e) => {
                eprintln!("ssh -L {}: {e}", forward.spec.host);
                std::thread::sleep(BACKOFF_MAX);
                continue;
            }
        }
        // Waits for it to end without holding the lock, so `stop` can take it meanwhile.
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let mut slot = forward.child.lock().unwrap();
            let Some(child) = slot.as_mut() else {
                return;
            };
            if !matches!(child.try_wait(), Ok(None)) {
                *slot = None;
                break;
            }
        }
        backoff = if started.elapsed() > BACKOFF_MAX {
            Duration::from_millis(500)
        } else {
            (backoff * 2).min(BACKOFF_MAX)
        };
        std::thread::sleep(backoff);
    }
}

/// The running `ssh -L`, for the evidence that drops it and sees it come back.
#[cfg(feature = "visual-test")]
pub fn visual_link_pid() -> Option<u32> {
    (FORWARD.get()?.child.lock().unwrap().as_ref()).map(Child::id)
}
/// Ends the running `ssh -L` as a dropped network would; the supervisor starts another.
#[cfg(feature = "visual-test")]
pub fn visual_drop_link() {
    if let Some(forward) = FORWARD.get()
        && let Some(child) = forward.child.lock().unwrap().as_mut()
    {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_one_short_folder_of_its_own() {
        let alias = dir_name("alice-server");
        assert_ne!(
            dir_name("alice@server"),
            alias,
            "two destinations, one socket"
        );
        assert_eq!(alias, dir_name("alice-server"), "the same name every run");
        assert!(!dir_name("../x").contains(['/', '.']));
        // The longest path under it, the window's lock socket, stays under macOS's 104
        // bytes for a long host under a usual home.
        let root = Path::new("/Users/someone-long/.local/share/mooring-workbench/m")
            .join(dir_name(
                "lifcc@agent-os.tailc3193d.ts.net.example.internal",
            ))
            .join("client/client.sock");
        assert!(root.as_os_str().len() < 104, "{}", root.display());
    }
    #[test]
    fn the_socket_is_asked_for_then_found_as_an_older_fibo_finds_it() {
        let spec = Spec {
            host: "far".into(),
            path: Some("/opt/fibo".into()),
            quiet: false,
        };
        let argv = spec.ssh(&format!("{}; \"$p\" socket", spec.find_program()));
        assert_eq!(argv[0], "ssh");
        assert!(argv.contains(&"BatchMode=yes".to_owned()));
        assert!(argv.last().unwrap().starts_with("p='/opt/fibo'"));
    }
}
