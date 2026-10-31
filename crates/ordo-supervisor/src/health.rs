//! The supervisor's own `/health/live`, for the container's probe
//! (`plan-identity-service.md` 3.1). It answers on `ORDO_SUPERVISOR_LISTEN`
//! (`127.0.0.1:8079`) as long as the supervisor runs, whatever its
//! children do, with each child's state in the body. Each child keeps its
//! own health routes.

use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use crate::config::{ChildName, LISTEN_ENV};
use crate::error::SupervisorError;
use crate::run::Supervisor;

pub async fn bind(addr: std::net::SocketAddr) -> Result<TcpListener, SupervisorError> {
    TcpListener::bind(addr).await.map_err(|e| {
        SupervisorError::setting_caused(
            LISTEN_ENV,
            format!("is `{addr}`, which could not be bound"),
            format!("Set {LISTEN_ENV} to a free address such as 127.0.0.1:8079"),
            e,
        )
    })
}

/// The wait after a failed `accept`: it doubles from 50 ms to 5 s while
/// they keep failing, and starts again after one that works. An `accept`
/// that keeps failing (no file descriptors left, say) would otherwise spin
/// a core. The first failure of each run is said.
#[derive(Debug)]
pub struct AcceptBackoff {
    next: std::time::Duration,
    what: &'static str,
}

impl AcceptBackoff {
    pub const FIRST: std::time::Duration = std::time::Duration::from_millis(50);
    pub const MAX: std::time::Duration = std::time::Duration::from_secs(5);

    pub fn new(what: &'static str) -> AcceptBackoff {
        AcceptBackoff {
            next: Self::FIRST,
            what,
        }
    }

    /// An `accept` worked.
    pub fn reset(&mut self) {
        self.next = Self::FIRST;
    }

    /// The wait for this failure, and the one after it doubled.
    pub fn failed(&mut self, e: &std::io::Error) -> std::time::Duration {
        if self.next == Self::FIRST {
            eprintln!(
                "ordo-supervisor: {} could not accept a connection ({e}); waiting before \
                 trying again",
                self.what
            );
        }
        let wait = self.next;
        self.next = (self.next * 2).min(Self::MAX);
        wait
    }
}

/// Answer until the task is dropped.
pub async fn serve(listener: TcpListener, sup: Supervisor) {
    let mut backoff = AcceptBackoff::new("/health/live");
    loop {
        let mut stream = match listener.accept().await {
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
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                stream.read(&mut buf),
            )
            .await
            {
                Ok(Ok(n)) => n,
                _ => return,
            };
            let head = String::from_utf8_lossy(&buf[..n]);
            let path = head.split_whitespace().nth(1).unwrap_or("");
            let (status, body) = if path == "/health/live" {
                let children: serde_json::Map<String, serde_json::Value> = ChildName::ALL
                    .iter()
                    .map(|c| (c.as_str().to_string(), json!(sup.status(*c).state)))
                    .collect();
                (
                    "200 OK",
                    json!({ "status": "live", "children": children }).to_string(),
                )
            } else {
                ("404 Not Found", json!({ "error": "not_found" }).to_string())
            };
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Review, step 7: a persistent `accept` failure waits, doubling to
    /// 5 s, instead of spinning; one success starts it again.
    #[test]
    fn a_failing_accept_backs_off_and_a_success_resets_it() {
        let mut b = AcceptBackoff::new("test");
        let e = std::io::Error::other("too many open files");
        let waits: Vec<Duration> = (0..9).map(|_| b.failed(&e)).collect();
        assert_eq!(waits[0], AcceptBackoff::FIRST);
        assert_eq!(waits[1], Duration::from_millis(100));
        assert!(waits.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*waits.last().unwrap(), AcceptBackoff::MAX);
        b.reset();
        assert_eq!(b.failed(&e), AcceptBackoff::FIRST);
    }
}
