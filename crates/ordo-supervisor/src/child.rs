//! Starting, watching and stopping one process (`plan-console.md` 6 and
//! 10, moved from the console by `plan-identity-service.md` 3.1).
//!
//! Each child is started through the supervisor's own binary in launcher
//! mode ([`LAUNCH_ARG`]): a short single-threaded step between fork and the
//! real program that drops to the child's unix user, sets the parent-death
//! signal (Linux), checks the supervisor is still its parent, and then
//! execs. It runs before exec in its own process, so no `unsafe` code is
//! needed and the workspace's `unsafe_code = "forbid"` holds.
//!
//! With the parent-death signal, a supervisor that dies takes its children
//! with it, so no orphan holds a port.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, Command};

use crate::config::{ChildUser, LAUNCH_USER_ENV};

/// The first argument that runs `ordo-supervisor` as the launcher.
pub const LAUNCH_ARG: &str = "--supervised-child";
/// Launcher exit code: switching to the child's user was refused.
pub const EXIT_PRIVILEGE: i32 = 77;
/// The launcher's program name for a privilege probe: drop, then exit 0.
const PROBE_BIN: &str = "-";
/// How many of a child's output lines the supervisor keeps.
pub const LOG_LINES: usize = 200;

/// A child's output, newest last.
pub type LogRing = Arc<Mutex<VecDeque<String>>>;

pub fn push_log(ring: &LogRing, line: String) {
    if let Ok(mut r) = ring.lock() {
        if r.len() >= LOG_LINES {
            r.pop_front();
        }
        r.push_back(line);
    }
}

/// What starting one process needs.
pub struct Launch<'a> {
    /// The supervisor's own binary, run in launcher mode.
    pub launcher: &'a Path,
    pub bin: &'a Path,
    pub args: &'a [&'a str],
    /// The child's whole environment: nothing else is inherited.
    pub env: &'a [(String, String)],
    pub user: Option<ChildUser>,
    /// Start the child under a relay ([`launcher_main`]), so a stop needs
    /// no CAP_KILL. Always so when the child runs as another user.
    pub relay: bool,
}

/// Set by the supervisor when the launcher is to stay as the child's relay.
pub const LAUNCH_RELAY_ENV: &str = "ORDO_SUPERVISOR_LAUNCH_RELAY";
/// What the supervisor writes to a relay's stdin: SIGTERM, then SIGKILL.
/// Its stdin closing (the supervisor gone) is SIGKILL too.
pub const RELAY_TERM: u8 = b'T';
pub const RELAY_KILL: u8 = b'K';
/// The launcher's program name that reads one file as the target user and
/// prints it ([`read_as`]).
const READ_BIN: &str = "-read";

/// A running child as the supervisor holds it: the process, and the relay's
/// control pipe when it has one.
pub struct Started {
    pub proc: Child,
    pub ctl: Option<tokio::process::ChildStdin>,
}

/// The launcher's command: argv carries the supervisor's pid and the
/// program; the user to switch to travels in [`LAUNCH_USER_ENV`], which
/// the launcher removes before it execs. The child's own environment never
/// carries it.
fn launcher_command(launcher: &Path, user: Option<ChildUser>) -> Command {
    let mut cmd = Command::new(launcher);
    cmd.arg(LAUNCH_ARG)
        .arg(std::process::id().to_string())
        .env_clear();
    if let Some(u) = user {
        cmd.env(LAUNCH_USER_ENV, format!("{}:{}", u.uid, u.gid));
    }
    cmd
}

fn command(l: &Launch<'_>) -> Command {
    let mut cmd = launcher_command(l.launcher, None);
    cmd.arg(l.bin).args(l.args).envs(
        l.env
            .iter()
            .filter(|(k, _)| k != LAUNCH_USER_ENV && k != LAUNCH_RELAY_ENV)
            .map(|(k, v)| (k.as_str(), v.as_str())),
    );
    if let Some(u) = l.user {
        cmd.env(LAUNCH_USER_ENV, format!("{}:{}", u.uid, u.gid));
    }
    if l.relay {
        cmd.env(LAUNCH_RELAY_ENV, "1").stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Start the child and copy its output into the ring and the supervisor's
/// own log, each line marked with the child's program name.
pub fn spawn(l: &Launch<'_>, ring: &LogRing, mark: &'static str) -> std::io::Result<Started> {
    let mut child = command(l).spawn()?;
    if let Some(out) = child.stdout.take() {
        pipe(out, ring.clone(), mark);
    }
    if let Some(err) = child.stderr.take() {
        pipe(err, ring.clone(), mark);
    }
    let ctl = child.stdin.take();
    Ok(Started { proc: child, ctl })
}

/// Read one file as `user`, through the launcher: for a secret file the
/// deployment owner mounted readable by a child's user only, such as an
/// old `console-database-url` owned by the console (65532, mode 0400).
/// The supervisor never needs it changed on the host.
pub async fn read_as(launcher: &Path, user: ChildUser, path: &Path) -> Result<String, String> {
    let out = launcher_command(launcher, Some(user))
        .arg(READ_BIN)
        .arg(path)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("the launcher {} did not run: {e}", launcher.display()))?;
    if out.status.success() {
        String::from_utf8(out.stdout).map_err(|_| "the file is not UTF-8".to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn pipe(
    stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    ring: LogRing,
    mark: &'static str,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("{mark}: {line}");
            push_log(&ring, line);
        }
    });
}

/// Send a relay one control byte. `false` when it is gone.
async fn tell(ctl: &mut Option<tokio::process::ChildStdin>, byte: u8) -> bool {
    match ctl {
        Some(w) => {
            let sent = w.write_all(&[byte]).await.and(w.flush().await);
            sent.is_ok()
        }
        None => false,
    }
}

/// Ask a child to stop: SIGTERM, then SIGKILL after `grace`.
///
/// A child that runs as another unix user is started under a relay, its
/// launcher, which runs as that same user ([`launcher_main`]): the stop is
/// a byte on the relay's stdin, and the relay signals the child. So the
/// supervisor needs no CAP_KILL, and a compose file made before the one
/// image, which grants none, still stops cleanly (review, step 7). A child
/// without a relay runs as the supervisor's own user and is signalled
/// directly. A child that does not stop is said, and the wait ends after a
/// second grace period instead of hanging.
pub async fn stop(
    child: &mut Child,
    ctl: &mut Option<tokio::process::ChildStdin>,
    grace: Duration,
) -> Option<ExitStatus> {
    let pid = child.id();
    let relayed = ctl.is_some();
    if relayed {
        if !tell(ctl, RELAY_TERM).await {
            eprintln!(
                "ordo-supervisor: the relay of pid {} did not take the stop",
                pid.unwrap_or_default()
            );
        }
    } else {
        #[cfg(unix)]
        if let Some(p) = pid.and_then(|p| rustix::process::Pid::from_raw(p as i32)) {
            if let Err(e) = rustix::process::kill_process(p, rustix::process::Signal::TERM) {
                eprintln!(
                    "ordo-supervisor: SIGTERM to pid {} was refused ({e})",
                    p.as_raw_nonzero()
                );
            }
        }
    }
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(Ok(s)) => Some(s),
        _ => {
            if relayed {
                let _ = tell(ctl, RELAY_KILL).await;
                // Closing the pipe is SIGKILL as well, should the byte not
                // have reached it.
                *ctl = None;
            } else if let Err(e) = child.start_kill() {
                eprintln!(
                    "ordo-supervisor: SIGKILL to pid {} was refused ({e})",
                    pid.unwrap_or_default()
                );
            }
            match tokio::time::timeout(grace, child.wait()).await {
                Ok(Ok(s)) => Some(s),
                _ => {
                    eprintln!(
                        "ordo-supervisor: pid {} did not stop; going on without it",
                        pid.unwrap_or_default()
                    );
                    None
                }
            }
        }
    }
}

/// Whether `/health/ready` at `addr` answers 200.
pub async fn ready(addr: SocketAddr) -> bool {
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(addr).await?;
        s.write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut head = Vec::with_capacity(16);
        let mut buf = [0u8; 16];
        while head.len() < 12 {
            let n = s.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        Ok::<bool, std::io::Error>(
            head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200"),
        )
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(1), probe).await,
        Ok(Ok(true))
    )
}

/// Run a child's program once, with arguments, and wait for it: the
/// deployment's `check-config` (G17), the console's `forget-door-key`.
/// `Ok` holds what it printed; `Err` what it said when it failed.
///
/// It waits for the process to exit, not for its output to close: on macOS
/// a pipe is made and marked close-on-exec in 2 steps, so a sibling started
/// at the same moment by another task can inherit its write end and hold it
/// open for as long as that sibling runs. The output is read for at most
/// [`OUTPUT_GRACE`] after the exit.
pub async fn run_once(l: &Launch<'_>, timeout: Duration) -> Result<String, String> {
    let what = format!("`{} {}`", l.bin.display(), l.args.join(" "));
    let mut child = command(l)
        .spawn()
        .map_err(|e| format!("{what} could not run: {e}"))?;
    let out = collect(child.stdout.take());
    let err = collect(child.stderr.take());
    // A relay's control pipe stays open for the run; dropping it is SIGKILL.
    let mut ctl = child.stdin.take();
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Err(_) => {
            let _ = tell(&mut ctl, RELAY_KILL).await;
            drop(ctl);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
            return Err(format!(
                "{what} did not finish within {} s",
                timeout.as_secs()
            ));
        }
        Ok(Err(e)) => return Err(format!("{what} could not be read: {e}")),
        Ok(Ok(s)) => s,
    };
    let stdout = out.finish().await;
    let stderr = err.finish().await;
    if status.success() {
        return Ok(stdout.trim().to_string());
    }
    let mut text = stderr.trim().to_string();
    if text.is_empty() {
        text = stdout.trim().to_string();
    }
    if text.len() > 2000 {
        let cut = text.len() - 2000;
        let cut = (cut..text.len())
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(cut);
        text = text[cut..].to_string();
    }
    Err(if text.is_empty() {
        format!("{what} exited with {:?}", status.code())
    } else {
        text
    })
}

/// How long [`run_once`] reads a program's output after it exited.
pub const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// One output stream, read into memory as it comes.
struct Collected {
    buf: Arc<Mutex<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

fn collect(stream: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> Collected {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let task = stream.map(|mut s| {
        let buf = buf.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = s.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut b) = buf.lock() {
                    if b.len() < 1 << 20 {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        })
    });
    Collected { buf, task }
}

impl Collected {
    async fn finish(self) -> String {
        if let Some(task) = self.task {
            if tokio::time::timeout(OUTPUT_GRACE, task).await.is_err() {
                // A sibling holds the pipe open; what came is what there is.
            }
        }
        let bytes = self.buf.lock().map(|b| b.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Whether the supervisor may run a child as `user`: the launcher tries the
/// switch and exits. `Err` says why not.
pub async fn probe_user(launcher: &Path, user: ChildUser) -> Result<(), String> {
    let out = launcher_command(launcher, Some(user))
        .arg(PROBE_BIN)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("the launcher {} did not run: {e}", launcher.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The mode of the run folder and each child's folder: the supervisor owns
/// them; anyone may pass through to a file whose name they know, and no one
/// else may list or write them. Each file is 0600 and the child's, so a
/// sibling that guesses the name still cannot read it.
const DIR_MODE: u32 = 0o711;

/// Make sure the run folder exists with [`DIR_MODE`]. It may be a mount
/// point (a tmpfs), so it is never removed. A folder already at that mode
/// whose mode cannot be set (a read-only root) is taken as it is.
pub fn run_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(DIR_MODE)) {
            let mode = std::fs::metadata(dir)?.permissions().mode() & 0o7777;
            if mode != DIR_MODE {
                return Err(e);
            }
        }
    }
    Ok(())
}

/// Whether `dir` can be the run folder: made if missing, owned by the
/// supervisor (taken over with CAP_CHOWN when another user owns it, such
/// as the tmpfs an old compose file mounts for the console, `uid=65532`),
/// at [`DIR_MODE`], and writable. A folder already at that mode on a
/// read-only root passes the mode check; it then fails the write, said.
/// `Err` says why not.
pub fn usable_run_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not be made: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let meta = std::fs::metadata(dir).map_err(|e| format!("could not be read: {e}"))?;
        let me = rustix::process::geteuid().as_raw();
        if meta.uid() != me {
            let gid = rustix::process::getegid().as_raw();
            std::os::unix::fs::chown(dir, Some(me), Some(gid)).map_err(|e| {
                format!(
                    "belongs to uid {}, and the supervisor could not take it over ({e})",
                    meta.uid()
                )
            })?;
        }
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(DIR_MODE)) {
            let mode = std::fs::metadata(dir)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0);
            if mode != DIR_MODE {
                return Err(format!(
                    "is mode {mode:o}, and could not be made {DIR_MODE:o} ({e})"
                ));
            }
        }
    }
    let probe = dir.join(format!(".probe-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| format!("is not writable ({e})"))
}

/// Make an empty folder for one start's secret files, [`DIR_MODE`].
pub fn fresh_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        b.mode(DIR_MODE);
    }
    b.create(dir)
}

/// Write one secret file, mode 0600, owned by the child's user.
pub fn write_secret(path: &Path, value: &[u8], user: Option<ChildUser>) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(value)?;
    f.sync_all()?;
    give(path, user)
}

fn give(path: &Path, user: Option<ChildUser>) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(u) = user {
        std::os::unix::fs::chown(path, Some(u.uid), Some(u.gid))?;
    }
    #[cfg(not(unix))]
    let _ = (path, user);
    Ok(())
}

/// Launcher mode: `ordo-supervisor --supervised-child <parent pid>
/// <program> [args...]`, with the user to switch to in [`LAUNCH_USER_ENV`]
/// (unset: no switch). Never returns: it execs the program or exits.
///
/// The binary carries CAP_SETUID and CAP_SETGID, so on Linux the launcher
/// first proves its caller is the supervisor ([`verify_caller`]) and
/// refuses root and the supervisor's own identity as the target
/// ([`target_user`]), all with [`EXIT_PRIVILEGE`]. Before exec it empties
/// the capability bounding set and sets no-new-privileges, so nothing a
/// child runs can gain a privilege back.
///
/// Order matters. Changing user clears the parent-death signal (the kernel
/// resets it when credentials change), so the user is switched first and
/// the signal set after. Then the parent is checked again, in case the
/// supervisor died before the signal was set.
///
/// **The relay** ([`LAUNCH_RELAY_ENV`], set for every child of another
/// user). The kernel lets a process signal another only of its own uid,
/// unless it holds CAP_KILL. So the launcher does not become the child: it
/// starts a second launcher, its own child, which switches user and execs
/// the program as above, with this launcher as the parent it checks and
/// dies with. Once that one has exec'd (its stdin, a pipe from this one,
/// closes on exec), this launcher drops to the same user and stays as the
/// relay: a byte on its stdin from the supervisor signals the program
/// ([`RELAY_TERM`], [`RELAY_KILL`]), its stdin closing (the supervisor
/// gone) is SIGKILL, and it exits as the program did. It holds no
/// privilege and no secret, and dies with the supervisor.
pub fn launcher_main(args: Vec<OsString>) -> ! {
    let fail = |code: i32, msg: String| -> ! {
        eprintln!("ordo-supervisor launcher: {msg}");
        std::process::exit(code)
    };
    let parent: u32 = args
        .get(2)
        .and_then(|a| a.to_str())
        .and_then(|a| a.parse().ok())
        .unwrap_or(0);
    let program = args.get(3).cloned().unwrap_or_default();
    let rest: Vec<OsString> = args.iter().skip(4).cloned().collect();
    let wanted = std::env::var(LAUNCH_USER_ENV).ok();
    let relay = std::env::var_os(LAUNCH_RELAY_ENV).is_some()
        && program.to_str() != Some(PROBE_BIN)
        && program.to_str() != Some(READ_BIN);
    #[cfg(target_os = "linux")]
    let target = {
        if let Err(e) = verify_caller(parent) {
            fail(EXIT_PRIVILEGE, e);
        }
        match target_user(wanted.as_deref()) {
            Ok(t) => t,
            Err(e) => fail(EXIT_PRIVILEGE, e),
        }
    };
    #[cfg(not(target_os = "linux"))]
    {
        if wanted.is_some() {
            fail(
                EXIT_PRIVILEGE,
                "switching user is built for Linux only".into(),
            );
        }
    }
    // The relay starts the second launcher while it still runs as the
    // supervisor's user, so that one's own checks of its parent pass.
    let relayed = if relay {
        match start_inner(&program, &rest) {
            Ok(c) => Some(c),
            Err(e) => fail(127, e),
        }
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = drop_privileges(target) {
            fail(EXIT_PRIVILEGE, e);
        }
        if let Err(e) =
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))
        {
            fail(1, format!("the parent-death signal could not be set: {e}"));
        }
        let now = rustix::process::getppid().map(|p| p.as_raw_nonzero().get() as u32);
        if now != Some(parent) {
            fail(1, "the supervisor exited before its child started".into());
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = parent;
    if let Some(inner) = relayed {
        relay_until_exit(inner);
    }
    if program.to_str() == Some(PROBE_BIN) {
        std::process::exit(0);
    }
    if program.to_str() == Some(READ_BIN) {
        let Some(path) = rest.first() else {
            fail(2, "no file named to read".into());
        };
        match std::fs::read(path) {
            Ok(bytes) => {
                use std::io::Write as _;
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(&bytes);
                let _ = out.flush();
                std::process::exit(0);
            }
            Err(e) => fail(
                1,
                format!(
                    "{} could not be read as this user: {e}",
                    PathBuf::from(path).display()
                ),
            ),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // stdin is /dev/null for the program. For the inner launcher of a
        // relay it was the pipe that tells the relay this exec happened.
        let e = std::process::Command::new(&program)
            .args(&rest)
            .env_remove(LAUNCH_USER_ENV)
            .env_remove(LAUNCH_RELAY_ENV)
            .stdin(Stdio::null())
            .exec();
        fail(
            127,
            format!("{} could not start: {e}", PathBuf::from(&program).display()),
        )
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(&program)
            .args(&rest)
            .env_remove(LAUNCH_USER_ENV)
            .env_remove(LAUNCH_RELAY_ENV)
            .status()
        {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(e) => fail(127, format!("could not start: {e}")),
        }
    }
}

/// The relay's child: this binary again, as a plain launcher whose parent
/// is the relay. Returns once it has exec'd the program, or has failed.
#[cfg(unix)]
fn start_inner(program: &OsString, rest: &[OsString]) -> Result<std::process::Child, String> {
    use std::io::Read as _;
    let me = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().map_err(|e| format!("the launcher could not find itself: {e}"))?
    };
    let (mut done, told) =
        std::io::pipe().map_err(|e| format!("the relay's pipe could not be made: {e}"))?;
    let child = std::process::Command::new(&me)
        .arg(LAUNCH_ARG)
        .arg(std::process::id().to_string())
        .arg(program)
        .args(rest)
        .env_remove(LAUNCH_RELAY_ENV)
        .stdin(Stdio::from(told))
        .spawn()
        .map_err(|e| {
            format!(
                "the relay could not start the launcher {}: {e}",
                me.display()
            )
        })?;
    // The inner launcher's stdin closes when it execs or exits; until then
    // this process keeps the identity it checks.
    let mut sink = [0u8; 16];
    while matches!(done.read(&mut sink), Ok(n) if n > 0) {}
    Ok(child)
}

/// The relay's loop (see [`launcher_main`]). Never returns.
#[cfg(unix)]
fn relay_until_exit(mut inner: std::process::Child) -> ! {
    let pid = inner.id();
    #[cfg(target_os = "linux")]
    let pidfd = rustix::process::Pid::from_raw(pid as i32)
        .and_then(|p| rustix::process::pidfd_open(p, rustix::process::PidfdFlags::empty()).ok());
    let signal = move |sig: rustix::process::Signal| {
        #[cfg(target_os = "linux")]
        if let Some(fd) = &pidfd {
            let _ = rustix::process::pidfd_send_signal(fd, sig);
            return;
        }
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(p, sig);
        }
    };
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut stdin = std::io::stdin().lock();
        let mut byte = [0u8; 1];
        loop {
            match stdin.read(&mut byte) {
                Ok(1) if byte[0] == RELAY_TERM => signal(rustix::process::Signal::TERM),
                Ok(1) if byte[0] == RELAY_KILL => signal(rustix::process::Signal::KILL),
                Ok(1) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                // The supervisor is gone, or let go of this child.
                _ => {
                    signal(rustix::process::Signal::KILL);
                    return;
                }
            }
        }
    });
    let status = match inner.wait() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ordo-supervisor relay: the child could not be waited for: {e}");
            std::process::exit(1)
        }
    };
    use std::os::unix::process::ExitStatusExt as _;
    if let Some(code) = status.code() {
        std::process::exit(code);
    }
    // Ended by a signal: end the same way, so the supervisor sees it.
    if let Some(sig) = status
        .signal()
        .and_then(rustix::process::Signal::from_named_raw)
    {
        let _ = rustix::process::kill_process(rustix::process::getpid(), sig);
    }
    std::process::exit(128 + status.signal().unwrap_or(0))
}

/// The identity the supervisor binary admits: the owner and group of the
/// running executable (`/proc/self/exe`, the very file with the
/// capabilities; a copy carries none). The image installs it `root:65530`
/// mode 0750, so only the supervisor's group may run it at all.
#[cfg(target_os = "linux")]
fn exe_owner() -> Result<(u32, u32), String> {
    use std::os::unix::fs::MetadataExt as _;
    let m = std::fs::metadata("/proc/self/exe")
        .map_err(|e| format!("the launcher could not read its own executable: {e}"))?;
    Ok((m.uid(), m.gid()))
}

/// The real and effective uid of `pid`, from `/proc/<pid>/status`, which the
/// kernel writes and a caller cannot forge.
#[cfg(target_os = "linux")]
fn proc_uids(pid: u32) -> Option<(u32, u32)> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    let mut f = line.split_whitespace().skip(1);
    Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
}

/// Refuse a launcher the supervisor did not start. Its real uid must be
/// the supervisor's: root, or a uid that owns the binary, or whose real
/// group owns it; with no setuid games (real equal to effective). Its
/// parent, read from the kernel (not from argv), must be the pid the
/// supervisor named and must run as that same uid. A child's user that
/// execs the launcher fails the first; a stand-in parent fails the second.
#[cfg(target_os = "linux")]
fn verify_caller(named_parent: u32) -> Result<(), String> {
    let ruid = rustix::process::getuid().as_raw();
    let euid = rustix::process::geteuid().as_raw();
    let rgid = rustix::process::getgid().as_raw();
    if ruid != euid {
        return Err(format!(
            "refused: real uid {ruid} differs from effective uid {euid}"
        ));
    }
    let (owner, group) = exe_owner()?;
    if ruid != 0 && ruid != owner && rgid != group {
        return Err(format!(
            "refused: uid {ruid} (group {rgid}) is not the supervisor's; the supervisor binary \
             belongs to {owner}:{group}, and only the supervisor starts the launcher"
        ));
    }
    let ppid = rustix::process::getppid().map(|p| p.as_raw_nonzero().get() as u32);
    if ppid != Some(named_parent) {
        return Err(format!(
            "refused: the launcher's parent is {} and the supervisor named {named_parent}; only \
             the supervisor starts the launcher",
            ppid.map(|p| p.to_string()).unwrap_or_else(|| "none".into())
        ));
    }
    match proc_uids(named_parent) {
        Some((r, e)) if r == ruid && e == ruid => Ok(()),
        Some((r, e)) => Err(format!(
            "refused: the parent runs as uid {r} (effective {e}), and the launcher as {ruid}"
        )),
        None => Err(format!(
            "refused: the parent {named_parent} could not be read from /proc"
        )),
    }
}

/// The user to switch to, from the supervisor's setting only. Root (uid
/// or gid 0) is refused, as are the supervisor's own uid and the binary's
/// owner and group: a child must never become the supervisor.
#[cfg(target_os = "linux")]
fn target_user(wanted: Option<&str>) -> Result<Option<ChildUser>, String> {
    let Some(spec) = wanted.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let (uid, gid) = spec
        .split_once(':')
        .and_then(|(u, g)| Some((u.parse::<u32>().ok()?, g.parse::<u32>().ok()?)))
        .ok_or_else(|| format!("{LAUNCH_USER_ENV} `{spec}` is not uid:gid"))?;
    if uid == 0 || gid == 0 {
        return Err(format!(
            "refused: {LAUNCH_USER_ENV} `{spec}` names root, and a child never runs as root"
        ));
    }
    let me = rustix::process::getuid().as_raw();
    let (owner, group) = exe_owner()?;
    if uid == me || uid == owner || gid == group {
        return Err(format!(
            "refused: {LAUNCH_USER_ENV} `{spec}` is the supervisor's own identity (uid {me}, \
             binary {owner}:{group}); give each child a user of its own"
        ));
    }
    Ok(Some(ChildUser { uid, gid }))
}

/// Raise the capabilities the launcher may use (permitted by the file,
/// never effective until here).
#[cfg(target_os = "linux")]
fn raise(need: rustix::thread::CapabilitySet) -> bool {
    use rustix::thread::{capabilities, set_capabilities};
    match capabilities(None) {
        Ok(mut caps) if caps.permitted.contains(need) => {
            caps.effective |= need;
            set_capabilities(None, caps).is_ok()
        }
        _ => false,
    }
}

/// Empty the capability bounding set, so no later exec can gain one, even
/// from a file capability. Needs CAP_SETPCAP.
#[cfg(target_os = "linux")]
fn empty_bounding_set() -> Result<(), String> {
    use rustix::io::Errno;
    use rustix::thread::{
        capability_is_in_bounding_set, remove_capability_from_bounding_set, CapabilitySet,
    };
    for bit in 0..64u32 {
        let cap = CapabilitySet::from_bits_retain(1u64 << bit);
        match capability_is_in_bounding_set(cap) {
            Ok(false) => {}
            Ok(true) => remove_capability_from_bounding_set(cap)
                .map_err(|e| format!("emptying the capability bounding set was refused: {e}"))?,
            // Past the kernel's last capability.
            Err(Errno::INVAL) => break,
            Err(e) => return Err(format!("reading the capability bounding set failed: {e}")),
        }
    }
    Ok(())
}

/// Everything the launcher gives up before exec: the bounding set (when
/// it may; required when it switches user), the user and groups, every
/// capability, and then no-new-privileges.
#[cfg(target_os = "linux")]
fn drop_privileges(target: Option<ChildUser>) -> Result<(), String> {
    use rustix::thread::{set_capabilities, CapabilitySet, CapabilitySets};
    let may_bound = raise(CapabilitySet::SETPCAP);
    if may_bound {
        empty_bounding_set()?;
    } else if target.is_some() {
        return Err(
            "switching user needs CAP_SETPCAP as well, to empty the capability bounding set; \
             grant the supervisor binary cap_chown,cap_setuid,cap_setgid,cap_setpcap=p"
                .into(),
        );
    }
    if let Some(u) = target {
        switch_user(u)?;
    }
    let none = CapabilitySets {
        effective: CapabilitySet::empty(),
        permitted: CapabilitySet::empty(),
        inheritable: CapabilitySet::empty(),
    };
    set_capabilities(None, none).map_err(|e| format!("dropping capabilities was refused: {e}"))?;
    rustix::thread::set_no_new_privs(true)
        .map_err(|e| format!("setting no-new-privileges was refused: {e}"))
}

/// Harden the supervisor process itself. Call first in `main`, before any
/// thread exists: capabilities are per thread, and every thread made later
/// copies these.
///
/// - Not dumpable: the kernel then refuses any other process, even one of
///   the supervisor's own unix user, its memory, environment and open files
///   under `/proc`, and refuses to attach a debugger.
/// - CAP_CHOWN raised from the permitted set to the effective one, when
///   the image granted it, so the supervisor can give each secret file to
///   its child's user and the socket to the console's group. CAP_SETUID
///   and CAP_SETGID stay unraised here; only the launcher uses them. No
///   CAP_KILL is needed: a child of another user is stopped through its
///   relay ([`stop`]), so a compose file that grants none still works.
pub fn harden() {
    #[cfg(target_os = "linux")]
    {
        use rustix::thread::{capabilities, set_capabilities, CapabilitySet};
        if let Err(e) =
            rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
        {
            eprintln!("ordo-supervisor: could not mark itself not dumpable: {e}");
        }
        if let Ok(mut caps) = capabilities(None) {
            let want = caps.permitted & CapabilitySet::CHOWN;
            if !want.is_empty() {
                caps.effective |= want;
                if let Err(e) = set_capabilities(None, caps) {
                    eprintln!("ordo-supervisor: could not raise CAP_CHOWN: {e}");
                }
            }
        }
    }
}

/// Switch this single-threaded process to `uid:gid`, dropping every
/// supplementary group. Linux only; [`drop_privileges`] then clears every
/// capability.
#[cfg(target_os = "linux")]
fn switch_user(u: ChildUser) -> Result<(), String> {
    use rustix::process::{Gid, Uid};
    use rustix::thread::CapabilitySet;
    raise(CapabilitySet::SETUID | CapabilitySet::SETGID);
    rustix::thread::set_thread_groups(&[])
        .map_err(|e| format!("dropping supplementary groups was refused: {e}"))?;
    rustix::thread::set_thread_gid(Gid::from_raw(u.gid))
        .map_err(|e| format!("switching to group {} was refused: {e}", u.gid))?;
    rustix::thread::set_thread_uid(Uid::from_raw(u.uid))
        .map_err(|e| format!("switching to user {} was refused: {e}", u.uid))?;
    Ok(())
}
