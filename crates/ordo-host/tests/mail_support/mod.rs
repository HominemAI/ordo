//! Stand-ins for a mail relay and a mail API: real listeners on loopback.
//!
//! **Nothing here reaches a real provider.** The TLS listeners present a leaf
//! for `localhost` signed by the test CA (`fixture/`), and the mailers under
//! test are given that CA as their only anchor and a deny list that permits
//! loopback. That permission exists only behind `ordo-egress`'s `test-pin`
//! feature, a dev-dependency here. Certificate validation stays on.
//!
//! Every listener binds **every** address `localhost` resolves to, on one
//! port: the mailer pins the first answer, and CI's resolver answers `::1`
//! first where a laptop answers `127.0.0.1` first.

#![allow(dead_code)]

use std::io;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ordo_egress::{DenyList, TlsRoots};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("../fixture/ca.der");
const LEAF: &[u8] = include_bytes!("../fixture/localhost.der");
const LEAF_KEY: &[u8] = include_bytes!("../fixture/localhost-key.der");

/// The relay user and password every test uses. The password is distinctive
/// so a test can say it appears nowhere it should not.
pub const USER: &str = "ordo-mailer";
pub const PASSWORD: &str = "pw-7f3c9e-never-printed";
/// `AUTH PLAIN` of the two: base64 of `\0ordo-mailer\0pw-7f3c9e-never-printed`.
pub const PLAIN: &str = "AG9yZG8tbWFpbGVyAHB3LTdmM2M5ZS1uZXZlci1wcmludGVk";
pub const FROM: &str = "signin@example.com";
pub const TO: &str = "kim@example.org";

/// The test CA as the only trust anchor.
pub fn roots() -> TlsRoots {
    TlsRoots::custom(&[CA.to_vec()]).expect("anchor")
}

/// The structural deny list, with loopback let through for the listener.
pub fn deny() -> DenyList {
    DenyList::categories_only().permitting_loopback_for_tests()
}

fn acceptor() -> TlsAcceptor {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("pkcs8 key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    TlsAcceptor::from(Arc::new(cfg))
}

/// Listeners on every address `localhost` answers, sharing one port.
pub async fn bind_localhost() -> (u16, Vec<TcpListener>) {
    let answers: Vec<std::net::SocketAddr> = tokio::net::lookup_host("localhost:0")
        .await
        .expect("localhost resolves")
        .collect();
    let first = *answers.first().expect("localhost answers an address");
    let tcp = TcpListener::bind((first.ip(), 0)).await.expect("bind");
    let port = tcp.local_addr().unwrap().port();
    let mut listeners = vec![tcp];
    for other in answers.iter().skip(1) {
        if other.ip() == first.ip() {
            continue;
        }
        if let Ok(l) = TcpListener::bind((other.ip(), port)).await {
            listeners.push(l);
        }
    }
    (port, listeners)
}

/// What a relay saw.
#[derive(Default, Debug, Clone)]
pub struct Session {
    pub commands: Vec<String>,
    pub data: Option<String>,
}

/// A relay over implicit TLS that speaks enough SMTP for one submission.
/// `fail` makes the reply to the first command starting with its prefix the
/// given code.
pub struct FakeRelay {
    pub port: u16,
    pub sessions: Arc<Mutex<Vec<Session>>>,
    pub accepted: Arc<AtomicUsize>,
}

impl FakeRelay {
    pub async fn start(fail: Option<(&'static str, u16)>) -> FakeRelay {
        FakeRelay::start_with(fail, true).await
    }

    /// A relay with no TLS at all, as Mailpit is by default: the same
    /// conversation, in plaintext.
    pub async fn start_plaintext() -> FakeRelay {
        FakeRelay::start_with(None, false).await
    }

    /// The same, answering the first command with `fail`'s prefix with its
    /// code.
    pub async fn start_plaintext_failing(fail: Option<(&'static str, u16)>) -> FakeRelay {
        FakeRelay::start_with(fail, false).await
    }

    async fn start_with(fail: Option<(&'static str, u16)>, tls: bool) -> FakeRelay {
        let (port, listeners) = bind_localhost().await;
        let sessions: Arc<Mutex<Vec<Session>>> = Arc::default();
        let accepted = Arc::new(AtomicUsize::new(0));
        let acceptor = acceptor();
        for tcp in listeners {
            let (acceptor, sessions, accepted) =
                (acceptor.clone(), sessions.clone(), accepted.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let (acceptor, sessions) = (acceptor.clone(), sessions.clone());
                    tokio::spawn(async move {
                        let session = if tls {
                            let Ok(tls) = acceptor.accept(sock).await else {
                                return;
                            };
                            converse(tls, fail).await
                        } else {
                            converse(sock, fail).await
                        };
                        sessions.lock().unwrap().push(session);
                    });
                }
            });
        }
        FakeRelay {
            port,
            sessions,
            accepted,
        }
    }

    /// The sessions so far, after waiting up to two seconds for `n` of
    /// them: the client may close before the relay has filed its notes.
    pub async fn sessions_after(&self, n: usize) -> Vec<Session> {
        for _ in 0..200 {
            if self.sessions.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        self.sessions()
    }

    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.lock().unwrap().clone()
    }
}

async fn line<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> io::Result<String> {
    loop {
        if let Some(i) = buf.windows(2).position(|w| w == b"\r\n") {
            let rest = buf.split_off(i + 2);
            let mut l = std::mem::replace(buf, rest);
            l.truncate(i);
            return Ok(String::from_utf8_lossy(&l).to_string());
        }
        let mut chunk = [0u8; 2048];
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::other("closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn converse<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    fail: Option<(&'static str, u16)>,
) -> Session {
    let _ = s.write_all(b"220 localhost ESMTP test relay\r\n").await;
    converse_in_tls(s, fail, Session::default()).await
}

/// The conversation after the greeting, inside TLS. `session` carries what
/// was said in plaintext before `STARTTLS`.
async fn converse_in_tls<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    fail: Option<(&'static str, u16)>,
    mut session: Session,
) -> Session {
    let mut buf = Vec::new();
    let mut failed = false;
    loop {
        let Ok(cmd) = line(&mut s, &mut buf).await else {
            return session;
        };
        session.commands.push(cmd.clone());
        let upper = cmd.to_ascii_uppercase();
        if let Some((prefix, code)) = fail {
            if !failed && upper.starts_with(prefix) {
                failed = true;
                let _ = s
                    .write_all(format!("{code} no, says the test\r\n").as_bytes())
                    .await;
                continue;
            }
        }
        let reply: &[u8] = if upper.starts_with("EHLO") {
            b"250-localhost greets you\r\n250-SIZE 1000000\r\n250 AUTH PLAIN\r\n"
        } else if upper.starts_with("AUTH PLAIN") {
            b"235 2.7.0 accepted\r\n"
        } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
            b"250 2.1.0 ok\r\n"
        } else if upper == "DATA" {
            let _ = s.write_all(b"354 go ahead\r\n").await;
            let mut data = String::new();
            loop {
                let Ok(l) = line(&mut s, &mut buf).await else {
                    return session;
                };
                if l == "." {
                    break;
                }
                data.push_str(&l);
                data.push_str("\r\n");
            }
            session.data = Some(data);
            b"250 2.0.0 queued\r\n"
        } else if upper == "QUIT" {
            let _ = s.write_all(b"221 bye\r\n").await;
            let _ = s.shutdown().await;
            return session;
        } else {
            b"500 what\r\n"
        };
        let _ = s.write_all(reply).await;
    }
}

/// A relay that answers in plaintext and records every byte it is sent.
pub struct PlainRelay {
    pub port: u16,
    pub received: Arc<Mutex<Vec<u8>>>,
}

impl PlainRelay {
    pub async fn start() -> PlainRelay {
        let (port, listeners) = bind_localhost().await;
        let received: Arc<Mutex<Vec<u8>>> = Arc::default();
        for tcp in listeners {
            let received = received.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let received = received.clone();
                    tokio::spawn(async move {
                        let _ = sock.write_all(b"220 plaintext relay ESMTP\r\n").await;
                        let mut chunk = [0u8; 4096];
                        loop {
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => {
                                    received.lock().unwrap().extend_from_slice(&chunk[..n]);
                                    let _ = sock.write_all(b"250 ok\r\n").await;
                                }
                            }
                        }
                    });
                }
            });
        }
        PlainRelay { port, received }
    }
}

/// How a [`StartTlsRelay`] behaves around the upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartTlsMode {
    /// Offers `STARTTLS`, answers `220`, and speaks TLS after it.
    Honest,
    /// Its `EHLO` reply leaves `STARTTLS` out, as a stripping attacker would.
    NotOffered,
    /// Sends a second reply in the same write as its `220` to `STARTTLS`,
    /// the response injection of CVE-2011-0411.
    Injects,
    /// Offers `STARTTLS`, answers `220`, and stays in plaintext.
    StaysPlain,
}

/// A relay on a plaintext port that upgrades with `STARTTLS`. `plaintext`
/// is every byte it was sent before any handshake; `sessions` is what it
/// saw inside TLS, after the plaintext commands.
pub struct StartTlsRelay {
    pub port: u16,
    pub plaintext: Arc<Mutex<Vec<u8>>>,
    pub sessions: Arc<Mutex<Vec<Session>>>,
    pub handshakes: Arc<AtomicUsize>,
}

impl StartTlsRelay {
    pub async fn start(mode: StartTlsMode) -> StartTlsRelay {
        let (port, listeners) = bind_localhost().await;
        let plaintext: Arc<Mutex<Vec<u8>>> = Arc::default();
        let sessions: Arc<Mutex<Vec<Session>>> = Arc::default();
        let handshakes = Arc::new(AtomicUsize::new(0));
        let acceptor = acceptor();
        for tcp in listeners {
            let (acceptor, plaintext, sessions, handshakes) = (
                acceptor.clone(),
                plaintext.clone(),
                sessions.clone(),
                handshakes.clone(),
            );
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let (acceptor, plaintext, sessions, handshakes) = (
                        acceptor.clone(),
                        plaintext.clone(),
                        sessions.clone(),
                        handshakes.clone(),
                    );
                    tokio::spawn(async move {
                        starttls_session(sock, mode, acceptor, plaintext, sessions, handshakes)
                            .await;
                    });
                }
            });
        }
        StartTlsRelay {
            port,
            plaintext,
            sessions,
            handshakes,
        }
    }

    pub fn plaintext(&self) -> String {
        String::from_utf8_lossy(&self.plaintext.lock().unwrap()).to_string()
    }

    /// The sessions inside TLS, after waiting up to two seconds for `n`.
    pub async fn sessions_after(&self, n: usize) -> Vec<Session> {
        for _ in 0..200 {
            if self.sessions.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        self.sessions.lock().unwrap().clone()
    }
}

async fn starttls_session(
    mut sock: tokio::net::TcpStream,
    mode: StartTlsMode,
    acceptor: TlsAcceptor,
    plaintext: Arc<Mutex<Vec<u8>>>,
    sessions: Arc<Mutex<Vec<Session>>>,
    handshakes: Arc<AtomicUsize>,
) {
    let mut session = Session::default();
    let _ = sock.write_all(b"220 localhost ESMTP test relay\r\n").await;
    // Read line by line, recording every byte, until STARTTLS.
    let mut buf = Vec::new();
    loop {
        let Ok(cmd) = line(&mut sock, &mut buf).await else {
            plaintext.lock().unwrap().extend_from_slice(&buf);
            return;
        };
        {
            let mut p = plaintext.lock().unwrap();
            p.extend_from_slice(cmd.as_bytes());
            p.extend_from_slice(b"\r\n");
        }
        session.commands.push(cmd.clone());
        let upper = cmd.to_ascii_uppercase();
        if upper.starts_with("EHLO") {
            let reply: &[u8] = if mode == StartTlsMode::NotOffered {
                b"250-localhost greets you\r\n250 AUTH PLAIN\r\n"
            } else {
                b"250-localhost greets you\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n"
            };
            let _ = sock.write_all(reply).await;
        } else if upper == "STARTTLS" && mode != StartTlsMode::NotOffered {
            break;
        } else {
            let _ = sock
                .write_all(b"530 5.7.0 must issue STARTTLS first\r\n")
                .await;
        }
    }
    match mode {
        StartTlsMode::Injects => {
            let _ = sock
                .write_all(b"220 2.0.0 ready\r\n250-injected\r\n250 AUTH PLAIN\r\n")
                .await;
        }
        StartTlsMode::StaysPlain => {
            let _ = sock.write_all(b"220 2.0.0 ready\r\n").await;
            let mut chunk = [0u8; 4096];
            loop {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        plaintext.lock().unwrap().extend_from_slice(&chunk[..n]);
                        let _ = sock.write_all(b"250 ok\r\n").await;
                    }
                }
            }
        }
        _ => {
            let _ = sock.write_all(b"220 2.0.0 ready\r\n").await;
        }
    }
    let Ok(tls) = acceptor.accept(sock).await else {
        return;
    };
    handshakes.fetch_add(1, Ordering::SeqCst);
    let session = converse_in_tls(tls, None, session).await;
    sessions.lock().unwrap().push(session);
}

/// What a mail API saw: the request head and body.
#[derive(Debug, Clone)]
pub struct Seen {
    pub head: String,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")))
            .map(str::to_string)
    }
}

/// A mail API over TLS that answers every request with `status`.
pub struct FakeApi {
    pub port: u16,
    pub status: Arc<AtomicU16>,
    /// A `Retry-After` value to answer with, when set.
    pub retry_after: Arc<Mutex<Option<String>>>,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeApi {
    pub async fn start(status: u16) -> FakeApi {
        let (port, listeners) = bind_localhost().await;
        let status = Arc::new(AtomicU16::new(status));
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let retry_after: Arc<Mutex<Option<String>>> = Arc::default();
        let acceptor = acceptor();
        for tcp in listeners {
            let (acceptor, status, seen) = (acceptor.clone(), status.clone(), seen.clone());
            let retry_after = retry_after.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let (acceptor, status, seen) = (acceptor.clone(), status.clone(), seen.clone());
                    let retry_after = retry_after.lock().unwrap().clone();
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(sock).await else {
                            return;
                        };
                        let Ok((head, body)) = read_request(&mut tls).await else {
                            return;
                        };
                        seen.lock().unwrap().push(Seen { head, body });
                        let code = status.load(Ordering::SeqCst);
                        let mut extra = if (300..400).contains(&code) {
                            "location: https://elsewhere.example/\r\n".to_string()
                        } else {
                            String::new()
                        };
                        if let Some(v) = retry_after {
                            extra.push_str(&format!("retry-after: {v}\r\n"));
                        }
                        let body = r#"{"id":"test"}"#;
                        let response = format!(
                            "HTTP/1.1 {code} X\r\n{extra}content-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = tls.write_all(response.as_bytes()).await;
                        let _ = tls.flush().await;
                        let _ = tls.shutdown().await;
                    });
                }
            });
        }
        FakeApi {
            port,
            status,
            retry_after,
            seen,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.port)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn read_request<S: AsyncRead + Unpin>(s: &mut S) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::other("closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let want: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok((head, body))
}
