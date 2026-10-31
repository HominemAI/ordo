#![allow(dead_code)]
//! A stand-in for `ordo-supervisor`'s socket, for the console's tests
//! (`plan-identity-service.md` 3.2). It speaks the real protocol with the
//! supervisor crate's own types: the requests the console's client sends,
//! the answers and errors it reads, and the supervisor's own checks of an
//! apply (`ApplyRequest::resolve`). It runs no process. Each revision it
//! makes live is written as the environment the deployment would start
//! with, so the stand-in door reads what the "running" deployment took.
//!
//! - `ORDO_FAKE_CHECK=refuse` in an apply: refused as `check-config` would.
//! - `ORDO_FAKE_COMES_UP=no` in an apply: rolled back to the live one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ordo_supervisor::socket::{Request, Response};
use ordo_supervisor::spec::{diff_summary, ApplyRequest, Revision, Secret};
use ordo_supervisor::state::{ChildState, Phase, Privilege, Status};
use ordo_supervisor::{Applied, SupervisorError};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;

#[derive(Default)]
pub struct State {
    pub live: Option<Revision>,
    /// Every apply the console sent, in order.
    pub applied: Vec<ApplyRequest>,
    pub restarts: u64,
    /// Where the live revision is written as an environment, if anywhere.
    pub env_file: Option<PathBuf>,
}

#[derive(Clone)]
pub struct FakeSupervisor {
    pub socket: PathBuf,
    pub state: Arc<Mutex<State>>,
}

impl FakeSupervisor {
    /// Serve at a short path under /tmp (a unix socket's path is at most
    /// 104 bytes on macOS), with revision 1 built from `container`, as the
    /// supervisor builds it at first boot.
    pub async fn start(name: &str, container: &[(&str, &str)], env_file: Option<PathBuf>) -> Self {
        let socket = PathBuf::from(format!("/tmp/ocs-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let env: Vec<(String, String)> = container
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let live = Revision::from_container(&env, 1_800_000_000);
        if let Some(f) = &env_file {
            write_env(f, &live);
        }
        let state = Arc::new(Mutex::new(State {
            live: Some(live),
            env_file,
            ..State::default()
        }));
        let listener = UnixListener::bind(&socket).expect("bind the fake socket");
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let st = st.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let answer = match serde_json::from_str::<Request>(&line) {
                            Ok(req) => handle(&st, req),
                            Err(e) => Err(SupervisorError::BadRequest(e.to_string())),
                        };
                        let r = match answer {
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
                        };
                        let mut text = serde_json::to_string(&r).unwrap();
                        text.push('\n');
                        if write.write_all(text.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        FakeSupervisor { socket, state }
    }

    pub fn live(&self) -> Revision {
        self.state.lock().unwrap().live.clone().unwrap()
    }

    pub fn applied(&self) -> Vec<ApplyRequest> {
        self.state.lock().unwrap().applied.clone()
    }
}

fn status(child: &str, live: u64) -> Status {
    Status {
        child: child.into(),
        state: ChildState::Ready,
        phase: Phase::Running,
        pid: Some(4242),
        revision: (child == "deployment").then_some(live),
        live: if child == "deployment" { live } else { 0 },
        restarts: 0,
        last_exit: None,
        ready_since: Some(1_800_000_000),
        held: false,
        applying: None,
        privilege: Privilege {
            separate: true,
            uid: Some(65533),
            note: None,
        },
    }
}

fn handle(st: &Arc<Mutex<State>>, req: Request) -> Result<Value, SupervisorError> {
    let mut s = st.lock().unwrap();
    let live = s.live.clone().unwrap();
    match req {
        Request::Status { child } => Ok(json!(status(&child, live.number))),
        Request::Logs { .. } => Ok(json!(["ordo-host: listening"])),
        Request::Current { .. } => Ok(live.public_json()),
        Request::Restart { child } if child == "deployment" => {
            s.restarts += 1;
            Ok(json!({}))
        }
        Request::Restart { child } => Err(SupervisorError::NotAllowed(format!(
            "`restart` is for the deployment only, not the {child}"
        ))),
        Request::Apply { apply, .. } => {
            s.applied.push(apply.clone());
            let new = apply.resolve(&live, &[], 1_800_000_100)?;
            if new.spec.settings.get("ORDO_FAKE_CHECK").map(String::as_str) == Some("refuse") {
                return Err(SupervisorError::SettingsRefused {
                    revision: new.number,
                    output: "ORDO_FAKE_CHECK is `refuse`, which boot refuses".into(),
                });
            }
            if new
                .spec
                .settings
                .get("ORDO_FAKE_COMES_UP")
                .map(String::as_str)
                == Some("no")
            {
                return Err(SupervisorError::RolledBack {
                    revision: new.number,
                    to: live.number,
                    why: "not ready within 5 s".into(),
                });
            }
            let diff = diff_summary(&live.spec, &new.spec);
            if let Some(f) = &s.env_file {
                write_env(f, &new);
            }
            s.live = Some(new.clone());
            Ok(json!(Applied {
                revision: new.number,
                diff
            }))
        }
    }
}

/// The deployment's environment for a revision: each setting, and each
/// secret as `<NAME>_FILE`, as the supervisor starts it.
fn write_env(path: &Path, rev: &Revision) {
    let mut text = String::new();
    for (k, v) in &rev.spec.settings {
        text.push_str(&format!("{k}={v}\n"));
    }
    for (k, s) in &rev.spec.secrets {
        let file = match s {
            Secret::File { path, .. } => path.clone(),
            Secret::Value { .. } => format!("/run/ordo/deployment/{k}"),
        };
        text.push_str(&format!("{k}_FILE={file}\n"));
    }
    text.push_str(&format!("ORDO_RUNTIME_REVISION={}\n", rev.number));
    std::fs::write(path, text).unwrap();
}

/// The value a revision holds for a secret, if the supervisor holds one.
#[allow(dead_code)]
pub fn value_of(rev: &Revision, name: &str) -> Option<String> {
    match rev.spec.secrets.get(name) {
        Some(Secret::Value { value, .. }) => Some(value.clone()),
        _ => None,
    }
}
