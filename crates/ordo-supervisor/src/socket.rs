//! The control socket (`plan-identity-service.md` 3.2): how the console
//! still manages the deployment once it no longer runs it.
//!
//! A Unix socket at `ORDO_SUPERVISOR_SOCKET` (`/run/ordo/supervisor.sock`),
//! owned by the supervisor, group the console's, mode 0660. Only the
//! console's unix user can open it; the deployment's and the identity
//! service's cannot. Each connection is also checked by the kernel's word
//! for who is calling (the peer's uid): the console's user, or the
//! supervisor's own.
//!
//! One JSON request per line, one JSON answer per line. The calls, the 5
//! the console has (`plan-console.md` 6.1), each naming a child:
//!
//! | Call | Children | Answer |
//! |---|---|---|
//! | `status` | all 3 | [`crate::state::Status`] |
//! | `logs` (`tail`) | all 3 | the newest lines, oldest first |
//! | `current` | `deployment` | the live revision, secrets by name only |
//! | `apply` | `deployment` | [`crate::state::Applied`] |
//! | `restart` | `deployment` | `{}` |
//!
//! `apply` and `restart` for the console or the identity service are
//! refused (owner, 2026-10-27, Q10): the supervisor restarts a crashed
//! identity service itself.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::config::{ChildName, ChildUser, SOCKET_ENV};
use crate::error::{SupervisorError, Wire};
use crate::run::Supervisor;
use crate::spec::ApplyRequest;

/// The longest request line: an apply with every secret at its largest is
/// far below it.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// How many lines `logs` answers when `tail` is not given.
pub const DEFAULT_TAIL: usize = 100;

/// One request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Status {
        child: String,
    },
    Logs {
        child: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<usize>,
    },
    Current {
        child: String,
    },
    Apply {
        child: String,
        apply: ApplyRequest,
    },
    Restart {
        child: String,
    },
}

/// One answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Wire>,
}

impl Response {
    fn answer(r: Result<Value, SupervisorError>) -> Response {
        match r {
            Ok(v) => Response {
                ok: true,
                result: Some(v),
                error: None,
            },
            Err(e) => Response {
                ok: false,
                result: None,
                error: Some(e.to_wire()),
            },
        }
    }
}

/// Bind the socket: a stale file from a crash is removed, then the socket
/// is made mode 0660 with the console's group. `console` unset: the
/// supervisor's own group, as the console then runs as the supervisor.
pub fn bind(path: &Path, console: Option<ChildUser>) -> Result<UnixListener, SupervisorError> {
    let refuse = |what: &str, e: std::io::Error| {
        SupervisorError::setting_caused(
            SOCKET_ENV,
            format!("is `{}`, and {what}", path.display()),
            format!(
                "Set {SOCKET_ENV} to a path in a folder the supervisor owns, such as \
                 /run/ordo/supervisor.sock on a tmpfs"
            ),
            e,
        )
    };
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(refuse("an old socket there could not be removed", e)),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| refuse("its folder could not be made", e))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| refuse("it could not be bound", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
            .map_err(|e| refuse("its mode could not be set to 0660", e))?;
        if let Some(c) = console {
            std::os::unix::fs::chown(path, None, Some(c.gid))
                .map_err(|e| refuse("it could not be given to the console's group", e))?;
        }
    }
    #[cfg(not(unix))]
    let _ = console;
    Ok(listener)
}

/// Who may call: the console's uid, and the supervisor's own.
#[derive(Clone, Copy, Debug)]
pub struct Callers {
    pub console: Option<u32>,
    pub own: u32,
}

impl Callers {
    pub fn of(console: Option<ChildUser>) -> Callers {
        #[cfg(unix)]
        let own = rustix::process::getuid().as_raw();
        #[cfg(not(unix))]
        let own = 0;
        Callers {
            console: console.map(|c| c.uid),
            own,
        }
    }

    fn admits(&self, uid: u32) -> bool {
        uid == self.own || Some(uid) == self.console
    }
}

/// Answer connections until the task is dropped.
pub async fn serve(listener: UnixListener, callers: Callers, sup: Supervisor) {
    let mut backoff = crate::health::AcceptBackoff::new("the socket");
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => {
                backoff.reset();
                s
            }
            Err(e) => {
                tokio::time::sleep(backoff.failed(&e)).await;
                continue;
            }
        };
        let sup = sup.clone();
        tokio::spawn(async move { connection(stream, callers, sup).await });
    }
}

async fn connection(stream: UnixStream, callers: Callers, sup: Supervisor) {
    let uid = stream.peer_cred().map(|c| c.uid()).ok();
    let (read, mut write) = stream.into_split();
    if !uid.is_some_and(|u| callers.admits(u)) {
        let refused = SupervisorError::NotAllowed(format!(
            "uid {} may not use the supervisor's socket; only the console's user may",
            uid.map(|u| u.to_string())
                .unwrap_or_else(|| "unknown".into())
        ));
        let _ = send(&mut write, &Response::answer(Err(refused))).await;
        eprintln!(
            "ordo-supervisor: refused a socket caller, uid {}",
            uid.map(|u| u.to_string())
                .unwrap_or_else(|| "unknown".into())
        );
        return;
    }
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        let limit = MAX_LINE as u64 + 1;
        match (&mut reader).take(limit).read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(n) if n > MAX_LINE => {
                let e =
                    SupervisorError::BadRequest(format!("a request is at most {MAX_LINE} bytes"));
                let _ = send(&mut write, &Response::answer(Err(e))).await;
                return;
            }
            Ok(_) => {}
        }
        let answer = match serde_json::from_str::<Request>(line.trim()) {
            Ok(req) => handle(&sup, req).await,
            Err(e) => Err(SupervisorError::BadRequest(format!(
                "the request is not one this socket answers: {e}"
            ))),
        };
        if send(&mut write, &Response::answer(answer)).await.is_err() {
            return;
        }
    }
}

async fn send(write: &mut tokio::net::unix::OwnedWriteHalf, r: &Response) -> std::io::Result<()> {
    let mut text = serde_json::to_string(r).unwrap_or_else(|_| "{\"ok\":false}".into());
    text.push('\n');
    write.write_all(text.as_bytes()).await
}

fn child(name: &str) -> Result<ChildName, SupervisorError> {
    ChildName::parse(name).ok_or_else(|| {
        SupervisorError::BadRequest(format!(
            "`{name}` is not a child; name `identity`, `console` or `deployment`"
        ))
    })
}

fn deployment_only(name: &str, call: &str) -> Result<(), SupervisorError> {
    match child(name)? {
        ChildName::Deployment => Ok(()),
        other => Err(SupervisorError::NotAllowed(format!(
            "`{call}` is for the deployment only; the supervisor restarts the {other} itself \
             when it exits"
        ))),
    }
}

async fn handle(sup: &Supervisor, req: Request) -> Result<Value, SupervisorError> {
    match req {
        Request::Status { child: c } => Ok(json!(sup.status(child(&c)?))),
        Request::Logs { child: c, tail } => {
            Ok(json!(sup.logs(child(&c)?, tail.unwrap_or(DEFAULT_TAIL))))
        }
        Request::Current { child: c } => {
            deployment_only(&c, "current")?;
            Ok(sup.current().public_json())
        }
        Request::Apply { child: c, apply } => {
            deployment_only(&c, "apply")?;
            let applied = sup.apply(&apply).await?;
            eprintln!(
                "ordo-supervisor: revision {} applied by {} ({})",
                applied.revision, apply.actor, applied.diff
            );
            Ok(json!(applied))
        }
        Request::Restart { child: c } => {
            deployment_only(&c, "restart")?;
            sup.restart(ChildName::Deployment).await?;
            Ok(json!({}))
        }
    }
}
