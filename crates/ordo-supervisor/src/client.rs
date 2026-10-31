//! The one client to the supervisor's socket (AGENTS.md rule 9). The
//! console reaches the supervisor through this and nothing else; every
//! failure is a [`SupervisorError`] that says whether to retry.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

use crate::config::ChildName;
use crate::error::SupervisorError;
use crate::socket::{Request, Response};
use crate::spec::ApplyRequest;
use crate::state::{Applied, Status};

/// How long a call other than `apply` may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an `apply` may take: `check-config`, the old process's grace
/// period and the new one's ready limit, with room.
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(600);

/// A client for the socket at one path.
#[derive(Clone, Debug)]
pub struct SupervisorClient {
    path: PathBuf,
}

impl SupervisorClient {
    pub fn new(path: impl Into<PathBuf>) -> SupervisorClient {
        SupervisorClient { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn call(&self, req: &Request, limit: Duration) -> Result<Value, SupervisorError> {
        let shown = self.path.display().to_string();
        let exchange = async {
            let stream = UnixStream::connect(&self.path).await.map_err(|e| {
                SupervisorError::unavailable(
                    format!("the supervisor's socket {shown} did not answer"),
                    e,
                )
            })?;
            let (read, mut write) = stream.into_split();
            let mut line = serde_json::to_string(req)
                .map_err(|e| SupervisorError::Internal(format!("the request: {e}")))?;
            line.push('\n');
            write.write_all(line.as_bytes()).await.map_err(|e| {
                SupervisorError::unavailable(format!("the request to {shown} was not sent"), e)
            })?;
            let mut answer = String::new();
            BufReader::new(read)
                .read_line(&mut answer)
                .await
                .map_err(|e| {
                    SupervisorError::unavailable(format!("{shown} closed before answering"), e)
                })?;
            if answer.trim().is_empty() {
                return Err(SupervisorError::Unavailable {
                    what: format!("{shown} closed before answering"),
                    cause: None,
                });
            }
            serde_json::from_str::<Response>(answer.trim()).map_err(|e| {
                SupervisorError::Internal(format!("the supervisor's answer did not parse: {e}"))
            })
        };
        let response = tokio::time::timeout(limit, exchange).await.map_err(|_| {
            SupervisorError::Unavailable {
                what: format!("the supervisor did not answer within {} s", limit.as_secs()),
                cause: None,
            }
        })??;
        match (response.ok, response.result, response.error) {
            (true, Some(v), _) => Ok(v),
            (true, None, _) => Ok(Value::Null),
            (false, _, Some(w)) => Err(w.into()),
            (false, _, None) => Err(SupervisorError::Internal(
                "the supervisor refused without saying why".into(),
            )),
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, SupervisorError> {
        serde_json::from_value(v).map_err(|e| {
            SupervisorError::Internal(format!("the supervisor's answer did not parse: {e}"))
        })
    }

    pub async fn status(&self, child: ChildName) -> Result<Status, SupervisorError> {
        let v = self
            .call(
                &Request::Status {
                    child: child.as_str().into(),
                },
                CALL_TIMEOUT,
            )
            .await?;
        Self::parse(v)
    }

    pub async fn logs(
        &self,
        child: ChildName,
        tail: usize,
    ) -> Result<Vec<String>, SupervisorError> {
        let v = self
            .call(
                &Request::Logs {
                    child: child.as_str().into(),
                    tail: Some(tail),
                },
                CALL_TIMEOUT,
            )
            .await?;
        Self::parse(v)
    }

    /// The deployment's live revision, secrets by name only.
    pub async fn current(&self) -> Result<Value, SupervisorError> {
        self.call(
            &Request::Current {
                child: ChildName::Deployment.as_str().into(),
            },
            CALL_TIMEOUT,
        )
        .await
    }

    pub async fn apply(&self, apply: &ApplyRequest) -> Result<Applied, SupervisorError> {
        let v = self
            .call(
                &Request::Apply {
                    child: ChildName::Deployment.as_str().into(),
                    apply: apply.clone(),
                },
                APPLY_TIMEOUT,
            )
            .await?;
        Self::parse(v)
    }

    /// Restart a child. Only the deployment is accepted; the others are
    /// refused with [`crate::error::Kind::NotAllowed`].
    pub async fn restart(&self, child: ChildName) -> Result<(), SupervisorError> {
        self.call(
            &Request::Restart {
                child: child.as_str().into(),
            },
            CALL_TIMEOUT.max(Duration::from_secs(180)),
        )
        .await
        .map(|_| ())
    }
}
