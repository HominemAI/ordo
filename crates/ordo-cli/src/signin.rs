//! `ordo login`, `logout`, `whoami`, and the token every door command
//! sends (`plan-identity-service.md` 5.5, 5.6, step 5).
//!
//! # Signing in
//!
//! `ordo login <identity url>` listens on `127.0.0.1` at a port the system
//! picks, makes a PKCE verifier (S256) and a random state, and opens the
//! browser at `<url>/cli?port&state&code_challenge`. It prints the link
//! too, for a browser on another screen. The person approves with a
//! passkey; the page sends the browser to
//! `http://127.0.0.1:<port>/callback?code&state`. The CLI checks the state,
//! redeems the code with the verifier (`POST /cli/redeem`) and stores the
//! session it gets.
//!
//! `ordo login --touch` runs the same flow for a fresh passkey touch on the
//! session it already holds; nothing new is opened.
//!
//! # The session file
//!
//! `sessions.json` in the config directory ([`config_dir`]), mode 0600 in
//! a 0700 directory, one entry per identity service (one service serves one
//! deployment). It holds the session's renewal secret and the last token.
//! Neither is ever printed. A token is asked for again from the session
//! when it has under [`REFRESH_MARGIN_SECS`] left (tokens live 10 minutes).
//!
//! # Which credential a command uses
//!
//! 1. `--token` or `ORDO_TOKEN`: a raw token, as before.
//! 2. `ORDO_MACHINE_KEY` or `ORDO_MACHINE_KEY_FILE`, with `ORDO_IDENTITY_URL`:
//!    a machine key signs a challenge for a 10 minute token. For CI.
//! 3. The session from `ordo login`: the one for `ORDO_IDENTITY_URL`, or the
//!    only one held.
//!
//! # One client, one error
//!
//! Every call to the identity service goes through [`IdentityClient`], and
//! every failure here is a [`SignInError`] (AGENTS.md rule 9). Each says
//! what to do next, and names the setting when one applies.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signer as _, SigningKey};
use ordo_blob::http::{self, Endpoint, Request};
use ordo_identity::clock::Clock;
use ordo_identity::codes::random_bytes;
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::operator::Outcome;

/// The identity service's public address.
pub const IDENTITY_URL_ENV: &str = "ORDO_IDENTITY_URL";
/// The deployment handle the token is for (`aud: ordo-deployment:<h>`).
pub const DEPLOYMENT_ENV: &str = "ORDO_IDENTITY_DEPLOYMENT";
/// The handle when none is given: the service's own default.
pub const DEFAULT_DEPLOYMENT: &str = "main";
/// A machine key's secret, `ordo-mk-v1:<id>:<seed>`.
pub const MACHINE_KEY_ENV: &str = "ORDO_MACHINE_KEY";
/// A file holding a machine key's secret.
pub const MACHINE_KEY_FILE_ENV: &str = "ORDO_MACHINE_KEY_FILE";
/// Where `sessions.json` lives, instead of the platform's config directory.
pub const CONFIG_DIR_ENV: &str = "ORDO_CONFIG_DIR";
/// `on` lets the identity URL be plain `http://` to a host that is not
/// this computer.
pub const PLAIN_HTTP_ENV: &str = "ORDO_IDENTITY_PLAIN_HTTP";
/// The raw token, as `wire.rs` reads it.
pub const TOKEN_ENV: &str = crate::wire::TOKEN_ENV;

/// The session file's name.
pub const SESSIONS_FILE: &str = "sessions.json";
/// A token with less than this left is asked for again.
pub const REFRESH_MARGIN_SECS: u64 = 60;
/// How long `ordo login` waits for the browser by default.
pub const DEFAULT_WAIT_SECS: u64 = 300;
/// How long one call to the identity service may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The prefix of a machine key's secret form.
const MACHINE_KEY_PREFIX: &str = "ordo-mk-v1";

// ───────────────────────────── the one error ─────────────────────────────

/// Every way signing in, or getting a token, can fail.
#[derive(Debug)]
pub enum SignInError {
    /// A setting is missing or wrong. `fix` says what to set.
    Setting {
        setting: &'static str,
        problem: String,
        fix: String,
    },
    /// No session is held for this identity service.
    NotSignedIn { origin: Option<String> },
    /// Several sessions are held and nothing says which one.
    WhichSession { origins: Vec<String> },
    /// The identity service could not be reached. A retry may help.
    Unreachable { url: String, cause: String },
    /// The identity service answered with a refusal.
    Refused {
        origin: String,
        status: u16,
        code: String,
        message: String,
        setting: Option<String>,
        retry: bool,
    },
    /// The identity service answered something this CLI cannot read.
    BadAnswer { origin: String, what: String },
    /// The browser never came back in time.
    TimedOut { secs: u64 },
    /// The browser came back with another login's state: refused, because
    /// that answer was not for this login.
    WrongState,
    /// The browser came back with no code.
    NoCode,
    /// The session file or the loopback socket failed.
    Local { what: String, cause: std::io::Error },
}

impl SignInError {
    /// May running the same command again help, unchanged?
    pub fn may_retry(&self) -> bool {
        match self {
            SignInError::Unreachable { .. } => true,
            SignInError::Refused { retry, .. } => *retry,
            _ => false,
        }
    }

    /// The setting a fixable refusal names.
    pub fn setting(&self) -> Option<&str> {
        match self {
            SignInError::Setting { setting, .. } => Some(setting),
            SignInError::Refused { setting, .. } => setting.as_deref(),
            SignInError::WhichSession { .. } => Some(IDENTITY_URL_ENV),
            _ => None,
        }
    }

    /// The service's refusal word, when it answered with one.
    pub fn code(&self) -> Option<&str> {
        match self {
            SignInError::Refused { code, .. } => Some(code),
            _ => None,
        }
    }

    /// What to do next, in one line.
    fn next(&self) -> String {
        match self {
            SignInError::Setting { fix, .. } => fix.clone(),
            SignInError::NotSignedIn { origin: Some(o) } => format!("run `ordo login {o}`"),
            SignInError::NotSignedIn { origin: None } => format!(
                "run `ordo login <identity url>`; in CI set {MACHINE_KEY_ENV} and \
                 {IDENTITY_URL_ENV}; or pass `--token <t>`"
            ),
            SignInError::WhichSession { origins } => format!(
                "set {IDENTITY_URL_ENV} (or `--identity <url>`) to one of: {}",
                origins.join(", ")
            ),
            SignInError::Unreachable { .. } => {
                format!("check the address and try again; it comes from {IDENTITY_URL_ENV} or `ordo login <url>`")
            }
            SignInError::Refused {
                code,
                origin,
                retry,
                ..
            } => match code.as_str() {
                "presence_required" => "run `ordo login --touch`, then run this again".into(),
                "not_signed_in" | "session_ended" | "token_refused" | "code_refused" => {
                    format!("run `ordo login {origin}`")
                }
                "machine_refused" => format!(
                    "check {MACHINE_KEY_ENV}: the key may be ended, or belong to another \
                     deployment; an admin can make a new one with `ordo people machine-key add`"
                ),
                "wrong_audience" => format!(
                    "set {DEPLOYMENT_ENV} (or `--deployment <handle>`) to the deployment this \
                     service serves, then run `ordo login {origin}` again"
                ),
                "enrol_a_new_passkey" => {
                    format!("open {origin}/account and enrol a passkey, then run this again")
                }
                "last_admin" => "invite another admin first with `ordo people invite`".into(),
                _ if *retry => "try again in a moment".into(),
                _ => "fix what the message says, then run this again".into(),
            },
            SignInError::BadAnswer { .. } => {
                "check that the address is the identity service, not the deployment or the console"
                    .into()
            }
            SignInError::TimedOut { .. } => {
                "run `ordo login` again; `--wait-secs <n>` waits longer".into()
            }
            SignInError::WrongState | SignInError::NoCode => {
                "run `ordo login` again, and approve only the request you just started".into()
            }
            SignInError::Local { .. } => "fix the file or folder named, then run this again".into(),
        }
    }

    /// The refusal as a person reads it: what happened, then what to do.
    pub fn render(&self) -> String {
        let mut out = format!("REFUSED  {self}\n         next: {}\n", self.next());
        if let Some(s) = self.setting() {
            out.push_str(&format!("         setting: {s}\n"));
        }
        out
    }

    /// The refusal as an exit: 2 for a usage or setting mistake, 1 otherwise.
    pub fn outcome(&self) -> Outcome {
        let code = match self {
            SignInError::Setting { .. } | SignInError::WhichSession { .. } => 2,
            _ => 1,
        };
        Outcome {
            report: self.render(),
            code,
        }
    }
}

impl fmt::Display for SignInError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignInError::Setting {
                setting, problem, ..
            } => write!(f, "{setting} {problem}"),
            SignInError::NotSignedIn { origin: Some(o) } => {
                write!(f, "you are not signed in to {o}")
            }
            SignInError::NotSignedIn { origin: None } => {
                f.write_str("you are not signed in to any identity service")
            }
            SignInError::WhichSession { origins } => write!(
                f,
                "you are signed in to {} identity services, and nothing says which one",
                origins.len()
            ),
            SignInError::Unreachable { url, cause } => {
                write!(f, "the identity service at {url} did not answer: {cause}")
            }
            SignInError::Refused {
                origin,
                code,
                message,
                ..
            } => write!(f, "[{code}] {message} ({origin})"),
            SignInError::BadAnswer { origin, what } => {
                write!(
                    f,
                    "{origin} answered something this CLI cannot read: {what}"
                )
            }
            SignInError::TimedOut { secs } => {
                write!(f, "the browser did not come back within {secs} seconds")
            }
            SignInError::WrongState => f.write_str(
                "the browser came back with a state this login did not make, so it was refused",
            ),
            SignInError::NoCode => f.write_str("the browser came back with no code"),
            SignInError::Local { what, cause } => write!(f, "{what}: {cause}"),
        }
    }
}

impl std::error::Error for SignInError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignInError::Local { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

fn local(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> SignInError {
    let what = what.into();
    move |cause| SignInError::Local { what, cause }
}

// ───────────────────────────── the context ─────────────────────────────

/// Reads one setting by name.
pub type Vars = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Opens a link in a browser.
pub type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// What these commands read from outside: settings, the clock, the
/// config directory and the browser. [`Ctx::from_process`] is the real one;
/// a test builds its own.
#[derive(Clone)]
pub struct Ctx {
    pub vars: Vars,
    pub clock: Clock,
    pub config_dir: Option<PathBuf>,
    pub opener: Opener,
    /// A person is at a terminal (stdin and stdout both). Only then does a
    /// `presence_required` run the touch flow itself (plan 5.5 step 5).
    pub interactive: bool,
}

impl Ctx {
    pub fn from_process() -> Ctx {
        Ctx {
            vars: Arc::new(|k| std::env::var(k).ok()),
            clock: Clock::system(),
            config_dir: None,
            opener: Arc::new(open_browser),
            interactive: {
                use std::io::IsTerminal as _;
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
            },
        }
    }

    fn var(&self, k: &str) -> Option<String> {
        (self.vars)(k).filter(|v| !v.trim().is_empty())
    }

    /// The directory `sessions.json` lives in.
    pub fn dir(&self) -> Result<PathBuf, SignInError> {
        if let Some(d) = &self.config_dir {
            return Ok(d.clone());
        }
        config_dir(&|k| self.var(k)).ok_or(SignInError::Setting {
            setting: CONFIG_DIR_ENV,
            problem: "is unset, and no home directory was found".into(),
            fix: format!("set {CONFIG_DIR_ENV} to a folder only you can read"),
        })
    }
}

/// The platform's config directory for Ordo: `ORDO_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/ordo`, else `%APPDATA%\ordo` on Windows, else
/// `~/.config/ordo` (the plan's path, on macOS and Linux both).
pub fn config_dir(get: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(d) = get(CONFIG_DIR_ENV) {
        return Some(PathBuf::from(d));
    }
    if let Some(x) = get("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(x).join("ordo"));
    }
    if cfg!(windows) {
        if let Some(a) = get("APPDATA") {
            return Some(PathBuf::from(a).join("ordo"));
        }
    }
    get("HOME").map(|h| PathBuf::from(h).join(".config").join("ordo"))
}

/// Open `url` with the system's browser.
fn open_browser(url: &str) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ───────────────────────────── PKCE and state ─────────────────────────────

/// A PKCE pair (RFC 7636, S256).
#[derive(Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl Pkce {
    /// 32 random bytes, base64url: a 43 character verifier.
    pub fn new() -> Pkce {
        let verifier = b64url_encode(&random_bytes::<32>());
        let challenge = challenge_of(&verifier);
        Pkce {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Pkce {
        Pkce::new()
    }
}

/// `BASE64URL(SHA256(verifier))`.
pub fn challenge_of(verifier: &str) -> String {
    b64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// A fresh state: 24 random bytes, base64url, 32 characters.
pub fn new_state() -> String {
    b64url_encode(&random_bytes::<24>())
}

/// Do two states match? Compared in constant time.
pub fn state_matches(expected: &str, got: &str) -> bool {
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ───────────────────────────── the address ─────────────────────────────

/// An identity service's address, checked: `https://h[:p]`, or plain
/// `http://` to this computer (or anywhere with `ORDO_IDENTITY_PLAIN_HTTP=on`),
/// with an optional base path, such as `https://console.example.com/identity`
/// when the console passes the service through on its own origin
/// (`ORDO_IDENTITY_BASE_PATH`). `text` is the whole address, path included,
/// and every route is asked for under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub text: String,
    pub endpoint: Endpoint,
    /// Empty, or `/` and plain segments, with no trailing `/`.
    pub base: String,
}

impl Origin {
    pub fn parse(raw: &str, plain_ok: bool) -> Result<Origin, SignInError> {
        let bad = |why: &str| SignInError::Setting {
            setting: IDENTITY_URL_ENV,
            problem: format!("is `{raw}`, which {why}"),
            fix: format!(
                "give the identity service's address, such as https://id.example.com, to \
                 `ordo login` or in {IDENTITY_URL_ENV}"
            ),
        };
        let t = raw.trim().trim_end_matches('/');
        let t = if t.contains("://") {
            t.to_string()
        } else {
            format!("https://{t}")
        };
        let (hostpart, base) = {
            let after = t.find("://").map(|i| i + 3).unwrap_or(0);
            match t[after..].find('/') {
                Some(i) => (t[..after + i].to_string(), t[after + i..].to_string()),
                None => (t.clone(), String::new()),
            }
        };
        let plain_segments = base.split('/').skip(1).all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
        });
        if !base.is_empty() && !plain_segments {
            return Err(bad(
                "has a path that is not plain segments, such as /identity",
            ));
        }
        let endpoint = Endpoint::parse(&hostpart).map_err(|_| bad("is not an address"))?;
        if !endpoint.tls && !plain_ok && !is_loopback(&endpoint.host) {
            return Err(SignInError::Setting {
                setting: PLAIN_HTTP_ENV,
                problem: format!(
                    "is not `on`, and `{raw}` is plain http:// to another computer, where a \
                     session secret could be read on the way"
                ),
                fix: format!("use https://, or set {PLAIN_HTTP_ENV}=on on a network you trust"),
            });
        }
        let scheme = if endpoint.tls { "https" } else { "http" };
        let text = format!("{scheme}://{}{base}", endpoint.authority());
        Ok(Origin {
            text,
            endpoint,
            base,
        })
    }
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h == "localhost"
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

// ───────────────────────────── the client ─────────────────────────────

/// The one client to the identity service.
pub struct IdentityClient {
    origin: Origin,
}

impl IdentityClient {
    pub fn new(origin: Origin) -> IdentityClient {
        IdentityClient { origin }
    }

    pub fn origin(&self) -> &str {
        &self.origin.text
    }

    /// `POST path` with a JSON body and an optional bearer.
    pub async fn post(
        &self,
        path: &str,
        body: Value,
        bearer: Option<&str>,
    ) -> Result<Value, SignInError> {
        self.call("POST", path, Some(body), bearer).await
    }

    /// `GET path` with a bearer.
    pub async fn get(&self, path: &str, bearer: Option<&str>) -> Result<Value, SignInError> {
        self.call("GET", path, None, bearer).await
    }

    async fn call(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> Result<Value, SignInError> {
        let url = format!("{}{path}", self.origin.text);
        match tokio::time::timeout(CALL_TIMEOUT, self.send(method, path, body, bearer)).await {
            Ok(r) => r,
            Err(_) => Err(SignInError::Unreachable {
                url,
                cause: format!("no answer in {} seconds", CALL_TIMEOUT.as_secs()),
            }),
        }
    }

    async fn send(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
        bearer: Option<&str>,
    ) -> Result<Value, SignInError> {
        use futures::StreamExt;

        let url = format!("{}{path}", self.origin.text);
        let mut headers = vec![("accept".to_string(), "application/json".to_string())];
        if let Some(b) = bearer {
            headers.push(("authorization".into(), format!("Bearer {b}")));
        }
        let body = body.map(|v| {
            headers.push(("content-type".into(), "application/json".into()));
            let bytes = v.to_string().into_bytes();
            let len = bytes.len() as u64;
            let stream: ordo_blob::ByteStream =
                Box::pin(futures::stream::once(async move { Ok(bytes) }));
            (len, stream)
        });
        let response = http::send(
            &self.origin.endpoint,
            Request {
                method,
                path: format!("{}{path}", self.origin.base),
                headers,
                body,
            },
        )
        .await
        .map_err(|e| SignInError::Unreachable {
            url: url.clone(),
            cause: e.to_string(),
        })?;
        let status = response.status;
        let mut out = Vec::new();
        let mut stream = response.body;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| SignInError::Unreachable {
                url: url.clone(),
                cause: e.to_string(),
            })?;
            out.extend_from_slice(&chunk);
            if out.len() > 1024 * 1024 {
                return Err(self.bad("an answer over a megabyte"));
            }
        }
        let value: Value = serde_json::from_slice(&out).map_err(|_| {
            self.bad(&format!(
                "{status} with a body that is not JSON: {}",
                String::from_utf8_lossy(&out[..out.len().min(200)])
            ))
        })?;
        if status >= 400 {
            let code = value["error"].as_str();
            let message = value["message"].as_str();
            let (Some(code), Some(message)) = (code, message) else {
                return Err(self.bad(&format!("{status}: {value}")));
            };
            return Err(SignInError::Refused {
                origin: self.origin.text.clone(),
                status,
                code: code.to_string(),
                message: message.to_string(),
                setting: value["setting"].as_str().map(str::to_string),
                retry: value["retry"].as_bool().unwrap_or(false),
            });
        }
        Ok(value)
    }

    fn bad(&self, what: &str) -> SignInError {
        SignInError::BadAnswer {
            origin: self.origin.text.clone(),
            what: what.to_string(),
        }
    }
}

fn field<'a>(v: &'a Value, name: &str, client: &IdentityClient) -> Result<&'a str, SignInError> {
    v[name]
        .as_str()
        .ok_or_else(|| client.bad(&format!("no `{name}` in {v}")))
}

// ───────────────────────────── the session file ─────────────────────────────

/// One session, as stored. Its secrets never reach `Debug` or a report.
#[derive(Clone, PartialEq, Eq)]
pub struct Stored {
    pub origin: String,
    pub deployment: String,
    pub person: String,
    pub sid: String,
    pub session: String,
    pub ends_at: u64,
    pub token: Option<String>,
    pub token_exp: u64,
}

impl fmt::Debug for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stored")
            .field("origin", &self.origin)
            .field("deployment", &self.deployment)
            .field("person", &self.person)
            .field("sid", &self.sid)
            .field("ends_at", &self.ends_at)
            .finish_non_exhaustive()
    }
}

impl Stored {
    fn to_json(&self) -> Value {
        json!({
            "deployment": self.deployment,
            "person": self.person,
            "sid": self.sid,
            "session": self.session,
            "sessionEndsAt": self.ends_at,
            "token": self.token,
            "tokenExp": self.token_exp,
        })
    }

    fn from_json(origin: &str, v: &Value) -> Option<Stored> {
        Some(Stored {
            origin: origin.to_string(),
            deployment: v["deployment"].as_str()?.to_string(),
            person: v["person"].as_str()?.to_string(),
            sid: v["sid"].as_str().unwrap_or("").to_string(),
            session: v["session"].as_str()?.to_string(),
            ends_at: v["sessionEndsAt"].as_u64().unwrap_or(0),
            token: v["token"].as_str().map(str::to_string),
            token_exp: v["tokenExp"].as_u64().unwrap_or(0),
        })
    }
}

/// `sessions.json`: every session held, keyed by the identity origin.
#[derive(Debug, Default)]
pub struct SessionFile {
    pub path: PathBuf,
    pub sessions: BTreeMap<String, Stored>,
}

impl SessionFile {
    pub fn load(dir: &Path) -> Result<SessionFile, SignInError> {
        let path = dir.join(SESSIONS_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SessionFile {
                    path,
                    sessions: BTreeMap::new(),
                })
            }
            Err(e) => return Err(local(format!("reading {}", path.display()))(e)),
        };
        let v: Value = serde_json::from_str(&text).map_err(|e| SignInError::Local {
            what: format!(
                "{} is not JSON; delete it and run `ordo login` again",
                path.display()
            ),
            cause: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        let mut sessions = BTreeMap::new();
        if let Some(m) = v["sessions"].as_object() {
            for (origin, s) in m {
                if let Some(s) = Stored::from_json(origin, s) {
                    sessions.insert(origin.clone(), s);
                }
            }
        }
        Ok(SessionFile { path, sessions })
    }

    /// Write the file whole, mode 0600, by a rename so a reader never sees
    /// half of it.
    pub fn save(&self) -> Result<(), SignInError> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        make_private_dir(dir)?;
        if self.sessions.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(local(format!("removing {}", self.path.display()))(e)),
            };
        }
        let mut m = serde_json::Map::new();
        for (k, s) in &self.sessions {
            m.insert(k.clone(), s.to_json());
        }
        let text = serde_json::to_string_pretty(&json!({ "version": 1, "sessions": m }))
            .unwrap_or_default();
        let tmp = dir.join(format!(".{SESSIONS_FILE}.{}", std::process::id()));
        write_private(&tmp, &text)?;
        std::fs::rename(&tmp, &self.path).map_err(local(format!("writing {}", self.path.display())))
    }

    /// The session for `origin`, or the only one held when `origin` is
    /// `None`.
    pub fn pick(&self, origin: Option<&str>) -> Result<&Stored, SignInError> {
        match origin {
            Some(o) => self
                .sessions
                .get(o)
                .ok_or_else(|| SignInError::NotSignedIn {
                    origin: Some(o.to_string()),
                }),
            None => match self.sessions.len() {
                0 => Err(SignInError::NotSignedIn { origin: None }),
                1 => Ok(self.sessions.values().next().expect("one session")),
                _ => Err(SignInError::WhichSession {
                    origins: self.sessions.keys().cloned().collect(),
                }),
            },
        }
    }
}

fn make_private_dir(dir: &Path) -> Result<(), SignInError> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        b.mode(0o700);
    }
    b.create(dir)
        .map_err(local(format!("making the folder {}", dir.display())))
}

fn write_private(path: &Path, text: &str) -> Result<(), SignInError> {
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(local(format!("writing {}", path.display())))?;
    std::io::Write::write_all(&mut f, text.as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(local(format!("writing {}", path.display())))
}

// ───────────────────────────── the loopback ─────────────────────────────

/// The listener the browser comes back to.
pub struct Loopback {
    listener: tokio::net::TcpListener,
    pub port: u16,
}

impl Loopback {
    /// Listen on `127.0.0.1` at a port the system picks.
    pub async fn bind() -> Result<Loopback, SignInError> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(local("listening on 127.0.0.1"))?;
        let port = listener
            .local_addr()
            .map_err(local("listening on 127.0.0.1"))?
            .port();
        Ok(Loopback { listener, port })
    }

    /// Wait for `GET /callback?code&state`. Anything else on the port gets
    /// a 404 and the wait goes on. A callback with another state is refused
    /// and ends the login.
    pub async fn wait(&self, state: &str, secs: u64) -> Result<String, SignInError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let accepted = tokio::time::timeout_at(deadline, self.listener.accept()).await;
            let Ok(accepted) = accepted else {
                return Err(SignInError::TimedOut { secs });
            };
            let Ok((mut sock, _)) = accepted else {
                continue;
            };
            let Some(target) = read_target(&mut sock).await else {
                continue;
            };
            let (path, query) = target.split_once('?').unwrap_or((&target, ""));
            if path != "/callback" {
                reply(&mut sock, 404, "Not found.").await;
                continue;
            }
            let q = parse_query(query);
            let got = q.get("state").map(String::as_str).unwrap_or("");
            if !state_matches(state, got) {
                reply(
                    &mut sock,
                    400,
                    "This answer was not for the login running on this computer. Nothing was \
                     signed in. Run ordo login again.",
                )
                .await;
                return Err(SignInError::WrongState);
            }
            let Some(code) = q.get("code").filter(|c| !c.is_empty()) else {
                reply(&mut sock, 400, "No code came back. Run ordo login again.").await;
                return Err(SignInError::NoCode);
            };
            reply(
                &mut sock,
                200,
                "Approved. You can close this tab and go back to the terminal.",
            )
            .await;
            return Ok(code.clone());
        }
    }
}

/// The request target of one HTTP request, or `None` for anything that is
/// not a `GET` read within 5 seconds and 8 KiB.
async fn read_target(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = async {
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 8192 {
            let n = sock.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .ok()??;
    let head = String::from_utf8_lossy(&buf);
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    if parts.next()? != "GET" {
        return None;
    }
    parts.next().map(str::to_string)
}

async fn reply(sock: &mut tokio::net::TcpStream, status: u16, text: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>ordo login</title>\
         <p style=\"font:16px system-ui;margin:2rem\">{text}</p>"
    );
    let res = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/html; charset=utf-8\r\n\
         content-length: {}\r\ncache-control: no-store\r\nreferrer-policy: no-referrer\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(res.as_bytes()).await;
    let _ = sock.shutdown().await;
}

/// `a=1&b=2`, percent-decoded.
pub fn parse_query(q: &str) -> BTreeMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The approval page's link.
pub fn approval_url(origin: &str, port: u16, state: &str, challenge: &str) -> String {
    format!(
        "{origin}/cli?port={port}&state={}&code_challenge={}",
        percent_encode(state),
        percent_encode(challenge)
    )
}

// ───────────────────────────── the commands ─────────────────────────────

/// The flags these commands take. Strict: an unknown flag is refused.
#[derive(Debug, Default)]
pub struct Args {
    pub positional: Vec<String>,
    pub named: BTreeMap<String, String>,
    pub switches: Vec<String>,
}

impl Args {
    pub fn parse(
        args: &[String],
        values: &[&str],
        switches: &[&str],
        usage: &str,
    ) -> Result<Args, String> {
        let mut out = Args::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a.starts_with("--") {
                if switches.contains(&a.as_str()) {
                    out.switches.push(a.clone());
                } else if values.contains(&a.as_str()) {
                    let v = it
                        .next()
                        .ok_or_else(|| format!("`{a}` needs a value\n{usage}"))?;
                    out.named.insert(a.clone(), v.clone());
                } else {
                    return Err(format!("`{a}` is not a flag of this command\n{usage}"));
                }
            } else {
                out.positional.push(a.clone());
            }
        }
        Ok(out)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.named.get(name).map(String::as_str)
    }

    pub fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

pub const LOGIN_USAGE: &str = "\
usage: ordo login [<identity url>] [--deployment <handle>] [--touch] [--no-browser] [--wait-secs <n>]
  Signs this computer in through your browser.
  <identity url>     the identity service, such as https://console.example.com/identity
                     (the one image's default) or https://id.example.com.
                     Default: ORDO_IDENTITY_URL, or the one you are signed in to.
  --deployment <h>   the deployment's handle. Default: ORDO_IDENTITY_DEPLOYMENT, or main.
  --touch            confirm with your passkey again, for writes. Keeps the session.
  --no-browser       only print the link, for a browser on another screen.
  --wait-secs <n>    how long to wait for the browser. Default 300.
";

pub const LOGOUT_USAGE: &str = "\
usage: ordo logout [<identity url>]
  Ends this computer's session at the identity service and deletes it here.
";

pub const WHOAMI_USAGE: &str = "\
usage: ordo whoami [<identity url>]
  Says who the next command acts as, and until when.
";

/// `ordo login | logout | whoami | people …`, from `main`.
pub fn run(cmd: &str, args: &[String]) -> Outcome {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}\n")),
    };
    rt.block_on(run_with(&Ctx::from_process(), cmd, args))
}

/// The same, with a context a test can build.
pub async fn run_with(ctx: &Ctx, cmd: &str, args: &[String]) -> Outcome {
    let r = match cmd {
        "login" => login(ctx, args).await,
        "logout" => logout(ctx, args).await,
        "whoami" => whoami(ctx, args).await,
        "people" => return crate::people::run(ctx, args).await,
        _ => return Outcome::usage(format!("unknown command `{cmd}`\n")),
    };
    match r {
        Ok(out) => out,
        Err(e) => e.outcome(),
    }
}

fn usage_err(e: String) -> Result<Outcome, SignInError> {
    Ok(Outcome::usage(e))
}

fn plain_ok(ctx: &Ctx) -> bool {
    ctx.var(PLAIN_HTTP_ENV).as_deref() == Some("on")
}

/// The origin a command names: its argument, then `ORDO_IDENTITY_URL`.
fn named_origin(ctx: &Ctx, arg: Option<&str>) -> Result<Option<Origin>, SignInError> {
    match arg
        .map(str::to_string)
        .or_else(|| ctx.var(IDENTITY_URL_ENV))
    {
        Some(u) => Origin::parse(&u, plain_ok(ctx)).map(Some),
        None => Ok(None),
    }
}

fn deployment(ctx: &Ctx, arg: Option<&str>) -> String {
    arg.map(str::to_string)
        .or_else(|| ctx.var(DEPLOYMENT_ENV))
        .unwrap_or_else(|| DEFAULT_DEPLOYMENT.into())
}

async fn login(ctx: &Ctx, args: &[String]) -> Result<Outcome, SignInError> {
    let a = match Args::parse(
        args,
        &["--deployment", "--wait-secs", "--identity"],
        &["--touch", "--no-browser"],
        LOGIN_USAGE,
    ) {
        Ok(a) => a,
        Err(e) => return usage_err(e),
    };
    if a.positional.len() > 1 {
        return usage_err(LOGIN_USAGE.into());
    }
    let wait = match a.get("--wait-secs").map(str::parse::<u64>) {
        None => DEFAULT_WAIT_SECS,
        Some(Ok(n)) if n > 0 => n,
        Some(_) => return usage_err(format!("`--wait-secs` takes whole seconds\n{LOGIN_USAGE}")),
    };
    let dir = ctx.dir()?;
    let mut file = SessionFile::load(&dir)?;
    let arg = a
        .positional
        .first()
        .map(String::as_str)
        .or(a.get("--identity"));
    let origin = match named_origin(ctx, arg)? {
        Some(o) => o,
        None => {
            let only = file.pick(None).map_err(|e| match e {
                SignInError::NotSignedIn { .. } => SignInError::Setting {
                    setting: IDENTITY_URL_ENV,
                    problem: "is unset, and no identity service was named".into(),
                    fix: format!(
                        "run `ordo login https://id.example.com` with your identity \
                         service's address, or set {IDENTITY_URL_ENV}"
                    ),
                },
                other => other,
            })?;
            Origin::parse(&only.origin, true)?
        }
    };
    let touch = a.has("--touch");
    let existing = file.sessions.get(&origin.text).cloned();
    if touch && existing.is_none() {
        return Err(SignInError::NotSignedIn {
            origin: Some(origin.text.clone()),
        });
    }
    let client = IdentityClient::new(origin.clone());

    let pkce = Pkce::new();
    let state = new_state();
    let lb = Loopback::bind().await?;
    let url = approval_url(&origin.text, lb.port, &state, &pkce.challenge);
    let what = if touch {
        "Confirm with your passkey"
    } else {
        "Sign this computer in"
    };
    eprintln!("{what} in the browser. If it did not open, open this link:\n  {url}");
    if !a.has("--no-browser") {
        if let Err(e) = (ctx.opener)(&url) {
            eprintln!("  (the browser could not be opened: {e})");
        }
    }
    eprintln!("Waiting up to {wait} seconds for the browser...");
    let code = lb.wait(&state, wait).await?;
    drop(lb);

    let mut body = json!({ "code": code, "verifier": pkce.verifier });
    if let Some(s) = &existing {
        if touch {
            body["session"] = json!(s.session);
        }
    }
    let answer = client.post("/cli/redeem", body, None).await?;
    if touch {
        let mut s = existing.expect("checked above");
        // The cached token carries the old touch; the next one is asked for
        // fresh.
        s.token = None;
        s.token_exp = 0;
        file.sessions.insert(origin.text.clone(), s);
        file.save()?;
        return Ok(Outcome::ok(format!(
            "Touched. Writes are allowed for the next 5 minutes ({}).\n",
            origin.text
        )));
    }
    let stored = Stored {
        origin: origin.text.clone(),
        deployment: deployment(ctx, a.get("--deployment")),
        person: field(&answer, "person", &client)?.to_string(),
        sid: answer["sid"].as_str().unwrap_or("").to_string(),
        session: field(&answer, "session", &client)?.to_string(),
        ends_at: answer["sessionEndsAt"].as_u64().unwrap_or(0),
        token: None,
        token_exp: 0,
    };
    // Ask for a token now, so a wrong deployment handle is refused here and
    // not at the first command.
    let mut stored = stored;
    if let Err(e) = refresh(&client, &mut stored, &ctx.clock).await {
        let _ = client
            .post("/sessions/end", json!({ "session": stored.session }), None)
            .await;
        return Err(e);
    }
    let person = stored.person.clone();
    let ends = stored.ends_at;
    file.sessions.insert(origin.text.clone(), stored);
    file.save()?;
    Ok(Outcome::ok(format!(
        "Signed in to {} as {person}.\nThe session ends {} at the latest.\nSaved in {}\n",
        origin.text,
        when(ends, ctx.clock.now()),
        file.path.display()
    )))
}

async fn logout(ctx: &Ctx, args: &[String]) -> Result<Outcome, SignInError> {
    let a = match Args::parse(args, &["--identity"], &[], LOGOUT_USAGE) {
        Ok(a) => a,
        Err(e) => return usage_err(e),
    };
    let arg = a
        .positional
        .first()
        .map(String::as_str)
        .or(a.get("--identity"));
    let named = named_origin(ctx, arg)?;
    let mut file = SessionFile::load(&ctx.dir()?)?;
    let s = file.pick(named.as_ref().map(|o| o.text.as_str()))?.clone();
    let client = IdentityClient::new(Origin::parse(&s.origin, true)?);
    match client
        .post("/sessions/end", json!({ "session": s.session }), None)
        .await
    {
        Ok(_) => {}
        // Already ended at the service: forgetting it here is all that is left.
        Err(e) if matches!(e.code(), Some("not_signed_in" | "session_ended")) => {}
        Err(e) => return Err(e),
    }
    file.sessions.remove(&s.origin);
    file.save()?;
    Ok(Outcome::ok(format!(
        "Signed out of {}. The session is ended and deleted from this computer.\n",
        s.origin
    )))
}

async fn whoami(ctx: &Ctx, args: &[String]) -> Result<Outcome, SignInError> {
    let a = match Args::parse(args, &["--identity"], &[], WHOAMI_USAGE) {
        Ok(a) => a,
        Err(e) => return usage_err(e),
    };
    let arg = a
        .positional
        .first()
        .map(String::as_str)
        .or(a.get("--identity"));
    let cred = credential(ctx, None, arg).await?;
    let claims = claims_of(&cred.token);
    let now = ctx.clock.now();
    let s = |k: &str| claims[k].as_str().unwrap_or("?").to_string();
    let mut out = format!(
        "who       {}\nkind      {}\nrole      {}\nfrom      {}\n",
        s("sub"),
        s("kind"),
        s("role"),
        cred.source.describe()
    );
    if let Some(o) = &cred.origin {
        out.push_str(&format!("identity  {o}\n"));
    }
    out.push_str(&format!("for       {}\n", s("aud")));
    if let Some(t) = claims["touched_at"].as_u64() {
        let age = now.saturating_sub(t);
        let fresh = if age <= 300 {
            "writes allowed"
        } else {
            "writes need `ordo login --touch`"
        };
        out.push_str(&format!("touched   {age} seconds ago ({fresh})\n"));
    }
    if let Some(exp) = claims["exp"].as_u64() {
        out.push_str(&format!("token     ends {}\n", when(exp, now)));
    }
    if let Some(ends) = cred.session_ends_at {
        out.push_str(&format!(
            "session   ends {} at the latest\n",
            when(ends, now)
        ));
    }
    Ok(Outcome::ok(out))
}

// ───────────────────────────── the token ─────────────────────────────

/// Where a token came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Flag,
    TokenEnv,
    MachineKey,
    Session,
}

impl Source {
    fn describe(&self) -> &'static str {
        match self {
            Source::Flag => "--token",
            Source::TokenEnv => "ORDO_TOKEN",
            Source::MachineKey => "a machine key (ORDO_MACHINE_KEY)",
            Source::Session => "the session from `ordo login`",
        }
    }
}

/// A token, where it came from, and the identity service behind it.
#[derive(Clone)]
pub struct Credential {
    pub token: String,
    pub source: Source,
    pub origin: Option<String>,
    pub session_ends_at: Option<u64>,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("source", &self.source)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

/// After a `presence_required`: when a person is at a terminal and the
/// token came from the `ordo login` session, run the `--touch` flow and
/// answer `true`, so the caller sends the same call once more with a new
/// token. Answers `false` for a script, CI, a raw token or a machine key:
/// the caller prints the hint instead.
pub async fn touch_if_interactive(
    ctx: &Ctx,
    flag: Option<&str>,
    identity: Option<&str>,
) -> Result<bool, SignInError> {
    if !ctx.interactive {
        return Ok(false);
    }
    let cred = credential(ctx, flag, identity).await?;
    let (Source::Session, Some(origin)) = (&cred.source, &cred.origin) else {
        return Ok(false);
    };
    eprintln!("This needs a fresh passkey touch. Running `ordo login --touch`.");
    let out = login(ctx, &["--touch".to_string(), origin.clone()]).await?;
    if out.code != 0 {
        return Err(SignInError::BadAnswer {
            origin: origin.clone(),
            what: out.report,
        });
    }
    eprint!("{}", out.report);
    Ok(true)
}

/// The token a command sends: `flag` (`--token`), then `ORDO_TOKEN`, then a
/// machine key, then the session. `identity` names the service, over
/// `ORDO_IDENTITY_URL`.
pub async fn credential(
    ctx: &Ctx,
    flag: Option<&str>,
    identity: Option<&str>,
) -> Result<Credential, SignInError> {
    let named = named_origin(ctx, identity)?;
    if let Some(t) = flag.filter(|t| !t.trim().is_empty()) {
        return Ok(Credential {
            token: t.trim().to_string(),
            source: Source::Flag,
            origin: named.map(|o| o.text),
            session_ends_at: None,
        });
    }
    if let Some(t) = ctx.var(TOKEN_ENV) {
        return Ok(Credential {
            token: t.trim().to_string(),
            source: Source::TokenEnv,
            origin: named.map(|o| o.text),
            session_ends_at: None,
        });
    }
    if let Some(secret) = machine_secret(ctx)? {
        let origin = named.ok_or_else(|| SignInError::Setting {
            setting: IDENTITY_URL_ENV,
            problem: format!("is unset, and {MACHINE_KEY_ENV} needs it"),
            fix: format!("set {IDENTITY_URL_ENV} to the identity service's address"),
        })?;
        let client = IdentityClient::new(origin);
        let token = machine_token(&client, &secret).await?;
        return Ok(Credential {
            token,
            source: Source::MachineKey,
            origin: Some(client.origin().to_string()),
            session_ends_at: None,
        });
    }
    let mut file = SessionFile::load(&ctx.dir()?)?;
    let mut s = file.pick(named.as_ref().map(|o| o.text.as_str()))?.clone();
    let client = IdentityClient::new(Origin::parse(&s.origin, true)?);
    let now = ctx.clock.now();
    let fresh = s.token.is_some() && s.token_exp > now + REFRESH_MARGIN_SECS;
    if !fresh {
        refresh(&client, &mut s, &ctx.clock).await?;
        file.sessions.insert(s.origin.clone(), s.clone());
        file.save()?;
    }
    Ok(Credential {
        token: s.token.clone().unwrap_or_default(),
        source: Source::Session,
        origin: Some(s.origin.clone()),
        session_ends_at: Some(s.ends_at),
    })
}

/// Ask the service for a token from the session.
async fn refresh(
    client: &IdentityClient,
    s: &mut Stored,
    clock: &Clock,
) -> Result<(), SignInError> {
    let aud = format!("ordo-deployment:{}", s.deployment);
    let answer = client
        .post("/token", json!({ "session": s.session, "aud": aud }), None)
        .await?;
    let token = field(&answer, "token", client)?.to_string();
    s.token_exp = answer["exp"]
        .as_u64()
        .unwrap_or_else(|| clock.now() + REFRESH_MARGIN_SECS);
    s.token = Some(token);
    Ok(())
}

/// The machine key's secret, from `ORDO_MACHINE_KEY` or the file
/// `ORDO_MACHINE_KEY_FILE` names.
fn machine_secret(ctx: &Ctx) -> Result<Option<(String, SigningKey)>, SignInError> {
    let (raw, setting) = if let Some(v) = ctx.var(MACHINE_KEY_ENV) {
        (v, MACHINE_KEY_ENV)
    } else if let Some(p) = ctx.var(MACHINE_KEY_FILE_ENV) {
        let text = std::fs::read_to_string(&p).map_err(|e| SignInError::Setting {
            setting: MACHINE_KEY_FILE_ENV,
            problem: format!("is `{p}`, which could not be read ({e})"),
            fix: format!("set {MACHINE_KEY_FILE_ENV} to the file holding the key"),
        })?;
        (text, MACHINE_KEY_FILE_ENV)
    } else {
        return Ok(None);
    };
    parse_machine_key(&raw)
        .map(Some)
        .ok_or_else(|| SignInError::Setting {
            setting,
            problem: format!("is not a machine key (`{MACHINE_KEY_PREFIX}:<id>:<key>`)"),
            fix: format!(
                "paste the whole value `ordo people machine-key add` printed into {setting}"
            ),
        })
}

/// `ordo-mk-v1:<id>:<base64url seed>`.
pub fn parse_machine_key(secret: &str) -> Option<(String, SigningKey)> {
    let mut parts = secret.trim().splitn(3, ':');
    let (Some(MACHINE_KEY_PREFIX), Some(id), Some(seed)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let seed: [u8; 32] = b64url_decode(seed).ok()?.try_into().ok()?;
    Some((id.to_string(), SigningKey::from_bytes(&seed)))
}

/// Sign in with a machine key: a challenge, signed, for a token.
async fn machine_token(
    client: &IdentityClient,
    (id, key): &(String, SigningKey),
) -> Result<String, SignInError> {
    let begun = client
        .post("/machine/begin", json!({ "key": id }), None)
        .await?;
    let challenge = field(&begun, "challenge", client)?.to_string();
    let bytes = b64url_decode(&challenge).map_err(|_| client.bad("a challenge not base64url"))?;
    let signature = b64url_encode(&key.sign(&bytes).to_bytes());
    let done = client
        .post(
            "/machine/complete",
            json!({ "key": id, "challenge": challenge, "signature": signature }),
            None,
        )
        .await?;
    Ok(field(&done, "token", client)?.to_string())
}

/// The token's claims, read without checking the signature: for showing,
/// never for deciding.
pub fn claims_of(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|p| b64url_decode(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

/// `at` as a date, and how far off it is.
pub fn when(at: u64, now: u64) -> String {
    let (y, m, d, hh, mm) = civil(at);
    let left = at.saturating_sub(now);
    let rel = if left >= 2 * 86_400 {
        format!("in {} days", left / 86_400)
    } else if left >= 7200 {
        format!("in {} hours", left / 3600)
    } else {
        format!("in {} minutes", left / 60)
    };
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC ({rel})")
}

/// `at` as a UTC date and time to the second.
pub fn utc(at: u64) -> String {
    let (y, m, d, hh, mm) = civil(at);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{:02}Z", at % 60)
}

fn civil(t: u64) -> (i64, u32, u32, u64, u64) {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, secs / 3600, (secs % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_is_s256_of_the_verifier() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let p = Pkce::new();
        assert_eq!(p.verifier.len(), 43);
        assert_eq!(p.challenge, challenge_of(&p.verifier));
        assert_ne!(Pkce::new().verifier, p.verifier);
        assert!(!format!("{p:?}").contains(&p.verifier));
    }

    #[test]
    fn the_state_is_random_and_fits_the_page() {
        let s = new_state();
        assert_eq!(s.len(), 32);
        assert!(s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(new_state(), s);
        assert!(state_matches(&s, &s.clone()));
        assert!(!state_matches(&s, &new_state()));
        assert!(!state_matches(&s, ""));
        assert!(!state_matches(&s, &s[..31]));
    }

    #[test]
    fn the_approval_link_carries_port_state_and_challenge() {
        let url = approval_url("https://id.example.com", 51234, "abc_DEF-123", "x+y/z=");
        assert_eq!(
            url,
            "https://id.example.com/cli?port=51234&state=abc_DEF-123&code_challenge=x%2By%2Fz%3D"
        );
        let q = parse_query(url.split_once('?').unwrap().1);
        assert_eq!(q["code_challenge"], "x+y/z=");
        assert_eq!(q["port"], "51234");
    }

    #[test]
    fn plain_http_is_for_this_computer_unless_the_setting_says_so() {
        assert!(Origin::parse("http://127.0.0.1:8082", false).is_ok());
        assert!(Origin::parse("http://localhost:8082/", false).is_ok());
        let e = Origin::parse("http://id.example.com", false).unwrap_err();
        assert_eq!(e.setting(), Some(PLAIN_HTTP_ENV));
        assert!(Origin::parse("http://id.example.com", true).is_ok());
        assert_eq!(
            Origin::parse("id.example.com/", false).unwrap().text,
            "https://id.example.com"
        );
    }

    #[test]
    fn a_base_path_is_kept_and_every_route_goes_under_it() {
        let o = Origin::parse("https://console.example.com/identity/", false).unwrap();
        assert_eq!(o.text, "https://console.example.com/identity");
        assert_eq!(o.base, "/identity");
        assert_eq!(o.endpoint.host, "console.example.com");
        assert_eq!(
            approval_url(&o.text, 1, "s", "c"),
            "https://console.example.com/identity/cli?port=1&state=s&code_challenge=c"
        );
        assert_eq!(
            Origin::parse("http://localhost:8081/a/b", false)
                .unwrap()
                .base,
            "/a/b"
        );
        for bad in [
            "https://h.example.com/../x",
            "https://h.example.com/a//b",
            "https://h.example.com/a?b",
        ] {
            let e = Origin::parse(bad, false).unwrap_err();
            assert_eq!(e.setting(), Some(IDENTITY_URL_ENV), "{bad}");
        }
    }

    #[test]
    fn a_machine_key_reads_back_and_a_bad_one_does_not() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let secret = format!("ordo-mk-v1:m_abc:{}", b64url_encode(&key.to_bytes()));
        let (id, back) = parse_machine_key(&secret).unwrap();
        assert_eq!(id, "m_abc");
        assert_eq!(back.to_bytes(), key.to_bytes());
        assert!(parse_machine_key("ordo-mk-v2:m:abc").is_none());
        assert!(parse_machine_key("ordo-mk-v1:m_abc:short").is_none());
    }

    #[test]
    fn every_refusal_says_what_to_do_next() {
        let refused = |code: &str| SignInError::Refused {
            origin: "https://id.example.com".into(),
            status: 403,
            code: code.into(),
            message: "m".into(),
            setting: None,
            retry: false,
        };
        assert!(refused("presence_required")
            .render()
            .contains("ordo login --touch"));
        assert!(refused("session_ended")
            .render()
            .contains("ordo login https://id.example.com"));
        assert!(SignInError::NotSignedIn { origin: None }
            .render()
            .contains("ordo login"));
        let which = SignInError::WhichSession {
            origins: vec!["https://a".into(), "https://b".into()],
        };
        assert!(which.render().contains("setting: ORDO_IDENTITY_URL"));
    }

    #[test]
    fn dates_print_in_utc() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0));
        assert_eq!(civil(1_800_000_000), (2027, 1, 15, 8, 0));
        assert!(when(1_800_000_000 + 7 * 86_400, 1_800_000_000).contains("in 7 days"));
    }
}
