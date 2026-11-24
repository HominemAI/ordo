//! **The deployment's one client to another deployment's door**: the step
//! before it in its chain (`plan-versions.md` §3.1, owner ruling 2026-11-24
//! on Q11).
//!
//! The promotion gate reads the version it promotes from the step before,
//! never from the caller. It reads it as the person promoting: the bearer
//! is that person's own token for the step before, from the same sign-in at
//! the identity service. No machine key is involved.
//!
//! # Where it reaches
//!
//! The address recorded on this deployment for the step before
//! ([`crate::environments::Door`]). `https://` anywhere; plain `http://`
//! to loopback; plain `http://` to another host only with
//! `ORDO_PROMOTION_PLAIN_HTTP=on`, because the person's token would cross
//! the wire in the clear. It is the deployment owner's own deployment, so
//! it is reached directly and not through the egress kernel, which is for
//! hosts a customer names (the rule `identity_keys` follows for the
//! identity service).
//!
//! # What it sends and keeps
//!
//! One `GET /op/std.deployment.versionSource`, bound to the step before's
//! own space, with the bearer. The token is held for that one request and
//! never stored or logged. The answer is bounded to 4 MiB.
//!
//! Staging reaches production through the same client for a clone
//! (`plan-versions.md` §3.4): one `POST /op/std.deployment.cloneOut`, as the
//! person cloning, with their own token for production. That answer carries
//! the sealed bundle and is bounded to [`MAX_CLONE_BYTES`].
//!
//! Production reads staging's signed rehearsal of a step the same way
//! (`crate::rehearsals`): one `GET /op/std.deployment.rehearsals`, as the
//! person accepting, with their own token for staging.
//!
//! # One error
//!
//! Every failure is a [`StepDoorError`], which keeps its cause and says
//! whether a retry can help and what to do.

use std::fmt;

use ordo_blob::http::{self, Endpoint, Request};
use serde_json::Value;

/// `on` lets a step before's door be plain `http://` to a host that is not
/// loopback.
pub const PLAIN_HTTP_ENV: &str = "ORDO_PROMOTION_PLAIN_HTTP";

/// The largest answer read from the step before.
pub const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// How long one read may take, connect to last byte.
pub const READ_SECS: u64 = 30;

/// The largest clone answer read from production: the sealed bundle,
/// base64, with its record.
pub const MAX_CLONE_BYTES: usize = 64 * 1024 * 1024;

/// How long a clone may take, connect to last byte: production masks and
/// seals the whole tenant before it answers.
pub const CLONE_SECS: u64 = 300;

/// Whether `ORDO_PROMOTION_PLAIN_HTTP=on` in this process.
pub fn plain_http_allowed() -> bool {
    std::env::var(PLAIN_HTTP_ENV).is_ok_and(|v| v.trim() == "on")
}

/// Why reading from the step before failed, and what to do.
#[derive(Debug)]
pub enum StepDoorError {
    /// The recorded address is not usable. Record it again.
    Address { address: String, why: String },
    /// The step before could not be reached, or did not answer in time.
    /// A retry can help.
    Unreachable { address: String, cause: String },
    /// The step before refused the person's token: not signed in there,
    /// not an admin there, or a token for another deployment.
    Refused {
        address: String,
        status: u16,
        message: String,
    },
    /// The step before has no such version.
    NoVersion { address: String, message: String },
    /// The step before answered something this cannot read.
    Answer { address: String, why: String },
}

impl StepDoorError {
    /// Whether the same request could succeed later.
    pub fn retryable(&self) -> bool {
        matches!(self, StepDoorError::Unreachable { .. })
    }
}

impl fmt::Display for StepDoorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepDoorError::Address { address, why } => {
                write!(f, "the address `{address}` {why}")
            }
            StepDoorError::Unreachable { address, cause } => write!(
                f,
                "the deployment at {address} could not be reached ({cause}). Check it is \
                 running, then try again"
            ),
            StepDoorError::Refused {
                address,
                status,
                message,
            } => write!(
                f,
                "the deployment at {address} refused your sign-in ({status}: {message}). \
                 This reads there as you: sign in to it through the same identity service as \
                 this deployment (`ordo login`), as an admin of both"
            ),
            StepDoorError::NoVersion { address, message } => {
                write!(
                    f,
                    "the deployment at {address} has no such version: {message}"
                )
            }
            StepDoorError::Answer { address, why } => {
                write!(f, "the deployment at {address} answered {why}")
            }
        }
    }
}

impl std::error::Error for StepDoorError {}

/// The step before's door, parsed.
#[derive(Debug, Clone)]
pub struct StepDoor {
    endpoint: Endpoint,
    address: String,
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl StepDoor {
    /// `https://host[:port]`, or `http://` to loopback; plain `http://`
    /// elsewhere only when `plain_ok` (`ORDO_PROMOTION_PLAIN_HTTP=on`).
    pub fn parse(address: &str, plain_ok: bool) -> Result<StepDoor, StepDoorError> {
        let address = address.trim().trim_end_matches('/').to_string();
        let endpoint = Endpoint::parse(&address).map_err(|e| StepDoorError::Address {
            address: address.clone(),
            why: format!(
                "is not usable ({e}). Give it as https://host[:port], where that \
                 deployment's door answers"
            ),
        })?;
        if !endpoint.tls && !plain_ok && !is_loopback(&endpoint.host) {
            return Err(StepDoorError::Address {
                address,
                why: format!(
                    "is plain http:// to a host that is not loopback, and promoting sends \
                     your sign-in there. Use https://, or set {PLAIN_HTTP_ENV}=on if the two \
                     deployments share a private network you trust"
                ),
            });
        }
        Ok(StepDoor { endpoint, address })
    }

    /// **Read one stored version as the person promoting**:
    /// `std.deployment.versionSource` in the step before's own `space`,
    /// with `bearer`. The answer's first row.
    pub async fn version_source(
        &self,
        space: &str,
        bearer: &str,
        version: &str,
    ) -> Result<Value, StepDoorError> {
        let path = format!(
            "/op/{}?version={}",
            crate::deploy_ops::VERSION_SOURCE,
            query_escape(version)
        );
        let read = self.send("GET", &path, space, bearer, None, MAX_ANSWER_BYTES);
        self.first_row(read, READ_SECS).await
    }

    /// **Ask production for a masked, sealed clone of one tenant**, as the
    /// person cloning: `std.deployment.cloneOut` in production's own
    /// `space`, with `bearer`. The answer's first row.
    pub async fn clone_out(
        &self,
        space: &str,
        bearer: &str,
        tenant: &str,
        env: &str,
    ) -> Result<Value, StepDoorError> {
        let body = serde_json::json!({ "$space": space, "tenant": tenant, "env": env });
        let path = format!("/op/{}", crate::clones::CLONE_OUT);
        let read = self.send(
            "POST",
            &path,
            space,
            bearer,
            Some(body.to_string().into_bytes()),
            MAX_CLONE_BYTES,
        );
        self.first_row(read, CLONE_SECS).await
    }

    /// **Read staging's signed rehearsals of one step** as the person
    /// accepting: `std.deployment.rehearsals` in staging's own `space`,
    /// with `bearer` (`crate::rehearsals`, §3.2). The answer's first row.
    pub async fn rehearsals(
        &self,
        space: &str,
        bearer: &str,
        from: &str,
        to: &str,
    ) -> Result<Value, StepDoorError> {
        let path = format!(
            "/op/{}?from={}&to={}",
            crate::rehearsals::REHEARSALS,
            query_escape(from),
            query_escape(to)
        );
        let read = self.send("GET", &path, space, bearer, None, MAX_ANSWER_BYTES);
        self.first_row(read, READ_SECS).await
    }

    async fn first_row(
        &self,
        read: impl std::future::Future<Output = Result<(u16, Vec<u8>), StepDoorError>>,
        secs: u64,
    ) -> Result<Value, StepDoorError> {
        let (status, body) =
            match tokio::time::timeout(std::time::Duration::from_secs(secs), read).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(StepDoorError::Unreachable {
                        address: self.address.clone(),
                        cause: format!("no answer in {secs} seconds"),
                    })
                }
            };
        let value: Value = serde_json::from_slice(&body).map_err(|_| StepDoorError::Answer {
            address: self.address.clone(),
            why: format!("{status} with something that is not JSON"),
        })?;
        let message = || {
            value["error"]["message"]
                .as_str()
                .or_else(|| value["message"].as_str())
                .unwrap_or("no message")
                .to_string()
        };
        match status {
            200 => value["rows"]
                .as_array()
                .and_then(|r| r.first())
                .cloned()
                .ok_or_else(|| StepDoorError::Answer {
                    address: self.address.clone(),
                    why: "no row".to_string(),
                }),
            401 | 403 => Err(StepDoorError::Refused {
                address: self.address.clone(),
                status,
                message: message(),
            }),
            404 => Err(StepDoorError::NoVersion {
                address: self.address.clone(),
                message: message(),
            }),
            s if s >= 500 => Err(StepDoorError::Unreachable {
                address: self.address.clone(),
                cause: format!("{s}: {}", message()),
            }),
            s => Err(StepDoorError::Answer {
                address: self.address.clone(),
                why: format!("{s}: {}", message()),
            }),
        }
    }

    async fn send(
        &self,
        method: &'static str,
        path: &str,
        space: &str,
        bearer: &str,
        body: Option<Vec<u8>>,
        max: usize,
    ) -> Result<(u16, Vec<u8>), StepDoorError> {
        use futures::StreamExt;
        let unreachable = |cause: String| StepDoorError::Unreachable {
            address: self.address.clone(),
            cause,
        };
        let mut headers = vec![
            ("authorization".into(), format!("Bearer {bearer}")),
            ("accept".into(), "application/json".into()),
            (ordo_api::router::SPACE_HEADER.into(), space.to_string()),
        ];
        let body = body.map(|bytes| {
            headers.push(("content-type".into(), "application/json".into()));
            let len = bytes.len() as u64;
            let stream: ordo_blob::ByteStream =
                Box::pin(futures::stream::once(async move { Ok(bytes) }));
            (len, stream)
        });
        let response = http::send(
            &self.endpoint,
            Request {
                method,
                path: path.to_string(),
                headers,
                body,
            },
        )
        .await
        .map_err(|e| unreachable(e.to_string()))?;
        let status = response.status;
        let mut out = Vec::new();
        let mut stream = response.body;
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.map_err(|e| unreachable(e.to_string()))?);
            if out.len() > max {
                return Err(StepDoorError::Answer {
                    address: self.address.clone(),
                    why: format!("more than {} MiB", max / (1024 * 1024)),
                });
            }
        }
        Ok((status, out))
    }
}

/// Percent-encode a query value: everything but unreserved characters.
fn query_escape(v: &str) -> String {
    let mut out = String::new();
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_reaches_loopback_alone_unless_allowed() {
        assert!(StepDoor::parse("http://127.0.0.1:4000", false).is_ok());
        assert!(StepDoor::parse("http://localhost:4000/", false).is_ok());
        assert!(StepDoor::parse("https://dev.example.com", false).is_ok());
        let e = StepDoor::parse("http://dev.internal:4000", false).unwrap_err();
        assert!(e.to_string().contains(PLAIN_HTTP_ENV), "{e}");
        assert!(StepDoor::parse("http://dev.internal:4000", true).is_ok());
        let e = StepDoor::parse("dev.example.com", false).unwrap_err();
        assert!(e.to_string().contains("https://host"), "{e}");
        assert!(!e.retryable());
    }

    #[test]
    fn a_query_value_is_escaped() {
        assert_eq!(query_escape("#4"), "%234");
        assert_eq!(query_escape("0.1.0"), "0.1.0");
    }
}
