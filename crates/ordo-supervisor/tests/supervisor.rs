//! The supervisor as the image runs it (`plan-identity-service.md` 3.1,
//! 3.2, 8 "Supervisor"): the real `ordo-supervisor` binary, its 3
//! children played by `ordo-supervisor-fake-child`, reached through its
//! socket and its health port.
//!
//! Users are not switched here: that is Linux with capabilities, and the
//! images smoke checks it. Everything else is the binary the image runs.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ordo_supervisor::error::Kind;
use ordo_supervisor::spec::{ApplyRequest, SecretInput};
use ordo_supervisor::{ChildName, ChildState, SupervisorClient};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_ordo-supervisor");
const FAKE: &str = env!("CARGO_BIN_EXE_ordo-supervisor-fake-child");

/// 4 free ports, below every system's ephemeral range: a port is released
/// before the supervisor binds it, and an ephemeral one may meanwhile
/// become the source port of a probe's connection. Each is handed out once
/// per run and checked free.
fn free_ports() -> [u16; 4] {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let base = 12_000 + (std::process::id() % 400) * 40;
    let mut out = Vec::new();
    while out.len() < 4 {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let port = u16::try_from(base + n % 8_000).expect("a port");
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            out.push(port);
        }
    }
    [out[0], out[1], out[2], out[3]]
}

/// One supervisor and its folders. The run folder is under /tmp: a unix
/// socket's path is at most 104 bytes on macOS.
struct H {
    proc: Child,
    dir: PathBuf,
    run: PathBuf,
    env: BTreeMap<String, String>,
    client: SupervisorClient,
    health: u16,
}

/// Stop the supervisor as the container would, so it stops its children:
/// off Linux there is no parent-death signal, and a killed supervisor
/// leaves them running.
impl Drop for H {
    fn drop(&mut self) {
        term(self.proc.id());
        let deadline = Instant::now() + Duration::from_secs(15);
        while matches!(self.proc.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.proc.kill();
        let _ = self.proc.wait();
        let _ = std::fs::remove_dir_all(&self.run);
    }
}

fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("ordo-sup-t-{name}-{}", std::process::id()));
    let run = PathBuf::from(format!("/tmp/osup-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&run);
    std::fs::create_dir_all(&dir).unwrap();
    (dir, run)
}

/// The container's variables, as `compose.one.yml` sets them, with fakes
/// for the binaries and free ports.
fn base_env(dir: &Path, run: &Path) -> BTreeMap<String, String> {
    let secret = dir.join("console-database-url");
    std::fs::write(
        &secret,
        "postgres://console:pw@console-db:5432/ordo_console\n",
    )
    .unwrap();
    let pg = dir.join("pg-password");
    std::fs::write(&pg, "deployment-pw").unwrap();
    let [sup, console, identity, host] = free_ports();
    let mut env = BTreeMap::new();
    for (k, v) in [
        ("ORDO_SUPERVISOR_IDENTITY_BIN", FAKE.to_string()),
        ("ORDO_SUPERVISOR_CONSOLE_BIN", FAKE.to_string()),
        ("ORDO_SUPERVISOR_HOST_BIN", FAKE.to_string()),
        (
            "ORDO_SUPERVISOR_STATE_DIR",
            dir.join("state").display().to_string(),
        ),
        ("ORDO_SUPERVISOR_RUN_DIR", run.display().to_string()),
        ("ORDO_SUPERVISOR_LISTEN", format!("127.0.0.1:{sup}")),
        ("ORDO_SUPERVISOR_GRACE_SECS", "2".into()),
        ("ORDO_SUPERVISOR_READY_SECS", "10".into()),
        (
            "ORDO_CONSOLE_DATABASE_URL_FILE",
            secret.display().to_string(),
        ),
        ("ORDO_CONSOLE_ORIGIN", "http://localhost:8081".into()),
        ("ORDO_CONSOLE_DOOR_SPACE", "system".into()),
        ("ORDO_CONSOLE_LISTEN", format!("127.0.0.1:{console}")),
        (
            "ORDO_IDENTITY_DATABASE_URL",
            "postgres://identity:pw@console-db:5432/ordo_identity".into(),
        ),
        ("ORDO_IDENTITY_LISTEN", format!("127.0.0.1:{identity}")),
        ("ORDO_HOST_ADDR", format!("127.0.0.1:{host}")),
        ("ORDO_PG_HOST", "deployment-db".into()),
        ("ORDO_PG_DB", "ordo".into()),
        ("ORDO_PG_PASSWORD_FILE", pg.display().to_string()),
        ("ORDO_TOKEN_KEY", "a-token-key-given-as-a-value".into()),
        ("ORDO_SPACE", "system".into()),
        (
            "ORDO_FAKE_ENV_OUT",
            dir.join("deployment.env").display().to_string(),
        ),
        (
            "ORDO_CONSOLE_FAKE_ENV_OUT",
            dir.join("console.env").display().to_string(),
        ),
        (
            "ORDO_IDENTITY_FAKE_ENV_OUT",
            dir.join("identity.env").display().to_string(),
        ),
        (
            "ORDO_CONSOLE_FAKE_FORGET_OUT",
            dir.join("forget.log").display().to_string(),
        ),
        (
            "ORDO_FAKE_STARTS",
            dir.join("deployment.starts").display().to_string(),
        ),
    ] {
        env.insert(k.to_string(), v);
    }
    env
}

fn spawn(dir: &Path, run: &Path, env: &BTreeMap<String, String>, log: &str) -> Child {
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(log))
        .unwrap();
    Command::new(SUPERVISOR)
        .env_clear()
        .envs(env)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(out))
        .spawn()
        .unwrap_or_else(|e| panic!("{SUPERVISOR}: {e} ({})", run.display()))
}

/// Start a supervisor. Another program on this machine may take a port
/// between the check and the bind; that start is retried on new ports.
async fn start_with(name: &str, change: impl Fn(&mut BTreeMap<String, String>)) -> H {
    for attempt in 1..=4 {
        let (dir, run) = dirs(name);
        let mut env = base_env(&dir, &run);
        change(&mut env);
        let health = env["ORDO_SUPERVISOR_LISTEN"]
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let proc = spawn(&dir, &run, &env, "supervisor.log");
        let client = SupervisorClient::new(run.join("supervisor.sock"));
        let mut h = H {
            proc,
            dir,
            run,
            env,
            client,
            health,
        };
        if h.wait_socket().await {
            return h;
        }
        assert!(
            attempt < 4 && h.log().contains("could not be bound"),
            "the supervisor exited:\n{}",
            h.log()
        );
    }
    unreachable!("the last attempt returns or panics")
}

async fn start(name: &str) -> H {
    start_with(name, |_| {}).await
}

impl H {
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("supervisor.log")).unwrap_or_default()
    }

    /// Wait for the socket; `false` when the supervisor exited first.
    async fn wait_socket(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.client.status(ChildName::Identity).await.is_err() {
            if !matches!(self.proc.try_wait(), Ok(None)) {
                return false;
            }
            assert!(
                Instant::now() < deadline,
                "the socket never answered:\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        true
    }

    async fn wait_ready(&self, child: ChildName) -> ordo_supervisor::Status {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let s = self.client.status(child).await.unwrap();
            if s.state == ChildState::Ready {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "{child} never became ready: {s:?}\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn all_ready(&self) -> BTreeMap<ChildName, u32> {
        let mut pids = BTreeMap::new();
        for c in ChildName::ALL {
            pids.insert(c, self.wait_ready(c).await.pid.unwrap());
        }
        pids
    }

    fn env_of(&self, child: &str) -> BTreeMap<String, String> {
        std::fs::read_to_string(self.dir.join(format!("{child}.env")))
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn health_live(&self) -> String {
        use std::io::{Read as _, Write as _};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", self.health)).unwrap();
        s.write_all(b"GET /health/live HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }
}

fn kill(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

fn term(pid: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 3 children start as siblings, each with a cleared environment holding
/// only its own settings and its secrets as private files; revision 1 is
/// built from the container's variables; the identity defaults are filled
/// from the console's settings (3.1, 6.2).
#[tokio::test(flavor = "multi_thread")]
async fn three_children_start_each_with_only_its_own_settings_and_the_identity_defaults() {
    let h = start("three").await;
    h.all_ready().await;

    let id = h.env_of("identity");
    assert_eq!(id["ORDO_IDENTITY_ORIGIN"], "http://localhost:8081");
    assert_eq!(id["ORDO_IDENTITY_BASE_PATH"], "/identity");
    assert_eq!(id["ORDO_IDENTITY_RP_ID"], "localhost");
    assert_eq!(id["ORDO_IDENTITY_CLIENT_ORIGINS"], "http://localhost:8081");
    assert_eq!(
        id["ORDO_IDENTITY_IMPORT_CONSOLE_URL_FILE_CONTENT"],
        "postgres://console:pw@console-db:5432/ordo_console"
    );
    assert_eq!(
        id["ORDO_IDENTITY_DATABASE_URL_FILE_CONTENT"],
        "postgres://identity:pw@console-db:5432/ordo_identity"
    );
    assert!(id["ORDO_IDENTITY_IMPORT_CONSOLE_URL_FILE"].starts_with(&h.run.display().to_string()));
    assert!(!id.contains_key("ORDO_IDENTITY_DATABASE_URL"));
    assert!(!id.keys().any(|k| k.starts_with("ORDO_CONSOLE_")));
    assert!(!id.contains_key("ORDO_TOKEN_KEY_FILE"));

    let console = h.env_of("console");
    assert!(!console.contains_key("ORDO_CONSOLE_MODE"));
    assert_eq!(
        console["ORDO_CONSOLE_SUPERVISOR_SOCKET"],
        h.run.join("supervisor.sock").display().to_string()
    );
    assert_eq!(
        console["ORDO_IDENTITY_URL"],
        format!("http://{}", h.env["ORDO_IDENTITY_LISTEN"])
    );
    assert_eq!(console["ORDO_IDENTITY_ORIGIN"], "http://localhost:8081");
    assert_eq!(console["ORDO_IDENTITY_BASE_PATH"], "/identity");
    assert_eq!(
        console["ORDO_CONSOLE_DOOR_URL"],
        format!("http://{}", h.env["ORDO_HOST_ADDR"])
    );
    assert_eq!(
        console["ORDO_CONSOLE_DATABASE_URL_FILE_CONTENT"],
        "postgres://console:pw@console-db:5432/ordo_console"
    );
    assert!(!console.contains_key("ORDO_IDENTITY_DATABASE_URL_FILE"));
    assert!(!console.contains_key("ORDO_TOKEN_KEY_FILE"));

    let dep = h.env_of("deployment");
    assert_eq!(dep["ORDO_RUNTIME_REVISION"], "1");
    assert_eq!(dep["ORDO_RUNTIME_APPLIED_BY"], "system");
    assert_eq!(dep["ORDO_IDENTITY_ORIGIN"], "http://localhost:8081");
    assert_eq!(
        dep["ORDO_TOKEN_KEY_FILE_CONTENT"],
        "a-token-key-given-as-a-value"
    );
    assert_eq!(dep["ORDO_PG_PASSWORD_FILE"], h.env["ORDO_PG_PASSWORD_FILE"]);
    assert!(!dep.contains_key("ORDO_TOKEN_KEY"));
    assert!(!dep.keys().any(|k| k.starts_with("ORDO_CONSOLE_")
        || k.starts_with("ORDO_SUPERVISOR_")
        || k == "ORDO_IDENTITY_DATABASE_URL_FILE"));

    // No child's environment holds a secret value, and each secret file
    // is the child's alone.
    for (name, env) in [
        ("identity", &id),
        ("console", &console),
        ("deployment", &dep),
    ] {
        for (k, v) in env.iter().filter(|(k, _)| !k.ends_with("_CONTENT")) {
            assert!(
                !v.contains("pw@") && !v.contains("a-token-key"),
                "{name} holds a secret value in {k}"
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let m = std::fs::metadata(&id["ORDO_IDENTITY_DATABASE_URL_FILE"]).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
        let sock = std::fs::metadata(h.run.join("supervisor.sock")).unwrap();
        assert_eq!(sock.permissions().mode() & 0o777, 0o660);
    }

    let current = h.client.current().await.unwrap();
    assert_eq!(current["revision"], 1);
    assert_eq!(current["origin"], "container");
    assert!(!current.to_string().contains("a-token-key"), "{current}");

    let live = h.health_live();
    assert!(live.starts_with("HTTP/1.1 200"), "{live}");
    assert!(live.contains("\"deployment\":\"ready\""), "{live}");
    let log = h.log();
    assert!(
        log.contains("ORDO_IDENTITY_RP_ID unset: filled as `localhost`"),
        "{log}"
    );
    assert!(!log.contains("pw@"), "{log}");
}

/// Kill each child in turn: the supervisor starts it again, and the other
/// 2 keep running the same process (8 "Supervisor").
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_child_is_started_again_and_the_other_two_keep_running() {
    let h = start("kill").await;
    let mut pids = h.all_ready().await;
    for victim in ChildName::ALL {
        kill(pids[&victim]);
        let deadline = Instant::now() + Duration::from_secs(30);
        let s = loop {
            let s = h.client.status(victim).await.unwrap();
            if s.state == ChildState::Ready && s.pid.is_some_and(|p| p != pids[&victim]) {
                break s;
            }
            assert!(
                Instant::now() < deadline,
                "{victim} not back: {s:?}\n{}",
                h.log()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(s.restarts, 1, "{victim}");
        assert!(s.last_exit.unwrap().describe().contains("signal 9"));
        for other in ChildName::ALL.into_iter().filter(|c| *c != victim) {
            let o = h.client.status(other).await.unwrap();
            assert_eq!(
                o.pid,
                Some(pids[&other]),
                "{other} was restarted with {victim}"
            );
            assert!(alive(pids[&other]));
        }
        pids.insert(victim, s.pid.unwrap());
    }
}

/// One child that cannot start keeps restarting, and never stops the other
/// 2 (3.1).
#[tokio::test(flavor = "multi_thread")]
async fn a_child_that_keeps_crashing_never_stops_the_other_two() {
    let h = start_with("crash", |e| {
        e.insert("ORDO_IDENTITY_FAKE_MODE".into(), "crash".into());
    })
    .await;
    h.wait_ready(ChildName::Console).await;
    h.wait_ready(ChildName::Deployment).await;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = h.client.status(ChildName::Identity).await.unwrap();
        if s.restarts >= 2 {
            assert_ne!(s.state, ChildState::Ready);
            break;
        }
        assert!(Instant::now() < deadline, "{s:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for c in [ChildName::Console, ChildName::Deployment] {
        let s = h.client.status(c).await.unwrap();
        assert_eq!((s.state, s.restarts), (ChildState::Ready, 0), "{c}");
    }
    assert!(h.health_live().starts_with("HTTP/1.1 200"));
}

/// SIGTERM stops the deployment first, then the console, then the
/// identity service, each waited for (3.1).
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_the_deployment_then_the_console_then_the_identity_service() {
    let mut h = start("stop").await;
    let pids = h.all_ready().await;
    term(h.proc.id());
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = h.proc.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "{}", h.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(status.success(), "{status:?}\n{}", h.log());
    let log = h.log();
    let at = |needle: &str| {
        log.find(needle)
            .unwrap_or_else(|| panic!("no `{needle}` in\n{log}"))
    };
    let order = [
        at("stopping the deployment"),
        at("the deployment has stopped"),
        at("stopping the console"),
        at("the console has stopped"),
        at("stopping the identity service"),
        at("the identity service has stopped"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{log}");
    for (c, pid) in pids {
        assert!(!alive(pid), "{c} outlived the supervisor");
    }
    assert!(!h.run.join("supervisor.sock").exists());
}

/// The socket answers `status` and `logs` for all 3, and refuses `apply`,
/// `restart` and `current` for any child but the deployment (3.2, Q10).
#[tokio::test(flavor = "multi_thread")]
async fn the_socket_manages_the_deployment_only() {
    let h = start("socket").await;
    h.all_ready().await;
    for c in ChildName::ALL {
        let logs = h.client.logs(c, 5).await.unwrap();
        assert!(!logs.is_empty(), "{c}");
        assert!(logs.len() <= 5);
    }
    for c in [ChildName::Console, ChildName::Identity] {
        let e = h.client.restart(c).await.unwrap_err();
        assert_eq!(e.kind(), Kind::NotAllowed, "{e}");
    }
    let old = h.client.status(ChildName::Deployment).await.unwrap().pid;
    h.client.restart(ChildName::Deployment).await.unwrap();
    let s = h.wait_ready(ChildName::Deployment).await;
    assert_ne!(s.pid, old);
    // A raw request for another child's `apply` is refused too.
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
    let mut raw = tokio::net::UnixStream::connect(h.run.join("supervisor.sock"))
        .await
        .unwrap();
    raw.write_all(
        b"{\"call\":\"apply\",\"child\":\"identity\",\"apply\":{\"revision\":2,\"actor\":\"p\"}}\n",
    )
    .await
    .unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(&mut raw)
        .read_line(&mut line)
        .await
        .unwrap();
    assert!(line.contains("not-allowed"), "{line}");
}

/// An apply is checked by `check-config` first and refused with its
/// words; a good one comes up as live; the sealed live spec survives a
/// supervisor restart, and the container's variables are then ignored
/// (3.2).
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_is_checked_comes_up_and_survives_a_restart() {
    let mut h = start("apply").await;
    h.all_ready().await;
    let mut settings: BTreeMap<String, String> = [
        ("ORDO_PG_HOST", "deployment-db"),
        ("ORDO_PG_DB", "ordo"),
        ("ORDO_SPACE", "system"),
        ("ORDO_HOST_ADDR", h.env["ORDO_HOST_ADDR"].as_str()),
        ("ORDO_FAKE_ENV_OUT", h.env["ORDO_FAKE_ENV_OUT"].as_str()),
        ("ORDO_FAKE_STARTS", h.env["ORDO_FAKE_STARTS"].as_str()),
        ("ORDO_FAKE_CHECK", "refuse"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let keep = SecretInput {
        keep: true,
        ..SecretInput::default()
    };
    let mut req = ApplyRequest {
        revision: 2,
        actor: "p_ada".into(),
        reason: "a refused one".into(),
        settings: settings.clone(),
        secrets: [
            ("ORDO_TOKEN_KEY".to_string(), keep.clone()),
            ("ORDO_PG_PASSWORD".to_string(), keep),
        ]
        .into(),
    };
    let e = h.client.apply(&req).await.unwrap_err();
    assert_eq!(e.kind(), Kind::SettingsRefused, "{e}");
    assert!(e.to_string().contains("ORDO_FAKE_CHECK"), "{e}");
    assert_eq!(h.client.current().await.unwrap()["revision"], 1);

    // The deployment's port is the console's: refused before anything runs.
    let mut clash = req.clone();
    clash.settings.insert(
        "ORDO_HOST_ADDR".into(),
        h.env["ORDO_CONSOLE_LISTEN"].clone(),
    );
    let e = h.client.apply(&clash).await.unwrap_err();
    assert_eq!(e.kind(), Kind::BadRequest, "{e}");
    assert!(e.to_string().contains("the console"), "{e}");

    settings.remove("ORDO_FAKE_CHECK");
    settings.insert("ORDO_WORKERS".into(), "4".into());
    req.settings = settings;
    req.reason = "more workers".into();
    let applied = h.client.apply(&req).await.unwrap();
    assert_eq!(applied.revision, 2);
    assert_eq!(applied.diff, "added=ORDO_WORKERS");
    let s = h.wait_ready(ChildName::Deployment).await;
    assert_eq!((s.live, s.revision), (2, Some(2)));
    let dep = h.env_of("deployment");
    assert_eq!(dep["ORDO_RUNTIME_APPLIED_BY"], "p_ada");
    assert_eq!(
        dep["ORDO_TOKEN_KEY_FILE_CONTENT"],
        "a-token-key-given-as-a-value"
    );

    // Restart the supervisor, with a container variable changed: the
    // sealed revision 2 runs, and the change is named as ignored.
    term(h.proc.id());
    let _ = h.proc.wait();
    let raw = std::fs::read_to_string(h.dir.join("state/deployment-live.sealed")).unwrap();
    assert!(!raw.contains("a-token-key") && !raw.contains("ORDO_WORKERS"));
    let mut env = h.env.clone();
    env.insert("ORDO_SPACE".into(), "elsewhere".into());
    h.proc = spawn(&h.dir, &h.run, &env, "supervisor.log");
    assert!(h.wait_socket().await, "{}", h.log());
    let s = h.wait_ready(ChildName::Deployment).await;
    assert_eq!((s.live, s.revision), (2, Some(2)));
    assert_eq!(h.env_of("deployment")["ORDO_SPACE"], "system");
    assert!(h.log().contains("ignored: ORDO_SPACE"), "{}", h.log());
}

/// The upgrade forgets the old console key itself (step 6 made a console
/// on the identity service refuse to boot while it holds one): before the
/// console's first start in each boot the supervisor runs
/// `ordo-console forget-door-key` as the console, named `ordo-supervisor`
/// in the console's audit row. A failure is tried again before the next
/// start; a later console restart does not run it again.
#[tokio::test(flavor = "multi_thread")]
async fn the_old_console_key_is_forgotten_before_the_console_starts() {
    let h = start_with("forget", |e| {
        e.insert(
            "ORDO_CONSOLE_FAKE_FORGET_SAYS".into(),
            "forgot the console key ck-old".into(),
        );
    })
    .await;
    let pids = h.all_ready().await;
    let forget = || std::fs::read_to_string(h.dir.join("forget.log")).unwrap_or_default();
    assert_eq!(forget(), "USER=ordo-supervisor\n");
    assert!(
        h.log()
            .contains("forget-door-key: forgot the console key ck-old"),
        "{}",
        h.log()
    );
    kill(pids[&ChildName::Console]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.client.status(ChildName::Console).await.unwrap().restarts == 0
        || h.client.status(ChildName::Console).await.unwrap().state != ChildState::Ready
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(forget(), "USER=ordo-supervisor\n", "ran again on a restart");

    let failing = start_with("forget-fail", |e| {
        e.insert("ORDO_CONSOLE_FAKE_FORGET".into(), "fail".into());
        e.insert("ORDO_CONSOLE_FAKE_MODE".into(), "crash".into());
    })
    .await;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let runs = std::fs::read_to_string(failing.dir.join("forget.log"))
            .unwrap_or_default()
            .lines()
            .count();
        if runs >= 2 {
            break;
        }
        assert!(Instant::now() < deadline, "{}", failing.log());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        failing.log().contains("forget-door-key failed"),
        "{}",
        failing.log()
    );
}

/// Review, step 7: a child of another unix user is stopped through its
/// relay, a byte on a pipe, so the supervisor needs no CAP_KILL and a
/// compose file that grants none still restarts and stops cleanly. Users
/// are not switched here, so `ORDO_SUPERVISOR_RELAY=always` puts every
/// child under a relay; a restart, a `check-config` and a shutdown all go
/// through it, and nothing is left running.
#[tokio::test(flavor = "multi_thread")]
async fn relayed_children_restart_check_and_stop_without_a_signal_from_the_supervisor() {
    let mut h = start_with("relay", |e| {
        e.insert("ORDO_SUPERVISOR_RELAY".into(), "always".into());
    })
    .await;
    let pids = h.all_ready().await;
    // The fake child records its own pid; a relay's pid is its parent's.
    let starts = || std::fs::read_to_string(h.dir.join("deployment.starts")).unwrap_or_default();
    let program_pid: u32 = starts()
        .lines()
        .last()
        .and_then(|l| l.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .expect("the deployment wrote its pid");
    assert_ne!(
        program_pid,
        pids[&ChildName::Deployment],
        "no relay in between"
    );

    h.client.restart(ChildName::Deployment).await.unwrap();
    let s = h.wait_ready(ChildName::Deployment).await;
    assert_ne!(s.pid, Some(pids[&ChildName::Deployment]));
    assert!(!alive(program_pid), "the old program outlived its restart");

    let e = h
        .client
        .apply(&ApplyRequest {
            revision: 2,
            actor: "p_ada".into(),
            reason: "refused".into(),
            settings: [("ORDO_FAKE_CHECK".to_string(), "refuse".to_string())].into(),
            secrets: BTreeMap::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.kind(), Kind::SettingsRefused, "{e}");

    let last_program: u32 = starts()
        .lines()
        .last()
        .and_then(|l| l.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap();
    let pids = h.all_ready().await;
    term(h.proc.id());
    let deadline = Instant::now() + Duration::from_secs(30);
    while h.proc.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "{}", h.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for (c, pid) in pids {
        assert!(!alive(pid), "{c}'s relay outlived the supervisor");
    }
    assert!(!alive(last_program), "the program outlived the supervisor");
    assert!(!h.log().contains("did not stop"), "{}", h.log());
}

/// Review, step 7: a supervisor key that no longer opens the live spec
/// never stops the supervisor. The file is kept aside, the deployment runs
/// revision 1 from the container's variables, and the other 2 children
/// start as ever.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_spec_that_does_not_open_starts_everything_from_revision_one() {
    let mut h = start("unopened").await;
    h.all_ready().await;
    term(h.proc.id());
    let _ = h.proc.wait();
    std::fs::write(h.dir.join("state/supervisor.key"), "11".repeat(32)).unwrap();
    h.proc = spawn(&h.dir, &h.run, &h.env, "supervisor.log");
    assert!(h.wait_socket().await, "{}", h.log());
    h.all_ready().await;
    assert_eq!(h.client.current().await.unwrap()["revision"], 1);
    let log = h.log();
    assert!(
        log.contains("WARNING: the deployment's live revision could not be read"),
        "{log}"
    );
    assert!(log.contains("ORDO_SUPERVISOR_KEY_FILE"), "{log}");
    let kept = std::fs::read_dir(h.dir.join("state"))
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".unopened-"));
    assert!(kept, "the unopened file was not kept");
}

/// Owner, 2026-10-30: the identity database is explicit, or opted into.
/// An upgraded container that names neither runs as before: the identity
/// service is not started, the console and the deployment get no identity
/// wiring, the old console key is not forgotten (the console still signs
/// in with it), and the log names both settings.
#[tokio::test(flavor = "multi_thread")]
async fn with_no_identity_database_the_identity_service_is_not_started() {
    let h = start_with("no-identity", |e| {
        e.remove("ORDO_IDENTITY_DATABASE_URL");
    })
    .await;
    h.wait_ready(ChildName::Console).await;
    h.wait_ready(ChildName::Deployment).await;
    let id = h.client.status(ChildName::Identity).await.unwrap();
    assert_eq!((id.state, id.pid), (ChildState::Stopped, None));
    assert!(id
        .privilege
        .note
        .as_deref()
        .unwrap_or_default()
        .contains("ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER"));
    let console = h.env_of("console");
    assert!(
        !console.keys().any(|k| k.starts_with("ORDO_IDENTITY_")),
        "{console:?}"
    );
    let dep = h.env_of("deployment");
    assert!(
        !dep.keys().any(|k| k.starts_with("ORDO_IDENTITY_")),
        "{dep:?}"
    );
    assert!(
        !h.dir.join("identity.env").exists(),
        "the identity service ran"
    );
    assert_eq!(
        std::fs::read_to_string(h.dir.join("forget.log")).unwrap_or_default(),
        "",
        "the console key was forgotten though the console still needs it"
    );
    let log = h.log();
    assert!(log.contains("ORDO_IDENTITY_DATABASE_URL"), "{log}");
    assert!(
        log.contains("ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true"),
        "{log}"
    );
    assert!(h.health_live().contains("\"identity\":\"stopped\""));

    // The opt-in starts it, wired.
    let h = start_with("share-db", |e| {
        e.remove("ORDO_IDENTITY_DATABASE_URL");
        e.insert(
            "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER".into(),
            "true".into(),
        );
    })
    .await;
    h.all_ready().await;
    assert_eq!(
        h.env_of("identity")["ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER"],
        "true"
    );
    assert!(h.env_of("console").contains_key("ORDO_IDENTITY_URL"));
}

/// A deployment owner's own identity value wins over every default.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_owner_set_wins_over_the_defaults() {
    let h = start_with("owner-set", |e| {
        e.insert(
            "ORDO_IDENTITY_ORIGIN".into(),
            "http://localhost:9999".into(),
        );
        e.insert("ORDO_IDENTITY_RP_ID".into(), "localhost".into());
    })
    .await;
    h.all_ready().await;
    let id = h.env_of("identity");
    assert_eq!(id["ORDO_IDENTITY_ORIGIN"], "http://localhost:9999");
    assert_eq!(
        h.env_of("deployment")["ORDO_IDENTITY_ORIGIN"],
        "http://localhost:9999"
    );
    assert!(!h.log().contains("ORDO_IDENTITY_ORIGIN unset"));
}
