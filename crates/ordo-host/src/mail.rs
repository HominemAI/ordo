//! The mail channel (`plan-auth-federated.md` §7.6, step 4).
//!
//! The deployment can send mail through one channel the app developer
//! configures: an SMTP relay over TLS, Resend, or SendGrid. It is
//! **off unless `ORDO_MAIL_KIND` is set**. Emailed sign-in codes
//! ([`crate::email_code`], step 5) are the first sender; a password reset is
//! the next. An app that runs its own mail needs
//! none of this, and this needs none of it (owner, 2026-10-08).
//!
//! # The parts
//!
//! - [`Mailer`]: submit one [`Message`]. [`SmtpMailer`] and [`ApiMailer`]
//!   are the two shapes, both through the egress kernel's pins: the relay or
//!   the API host is the whole allowlist, and `ORDO_EGRESS_DENY_CIDRS` with
//!   the structural categories is the deny list.
//! - **The outbox**, `$mailoutbox` in the control database. A send is a row,
//!   so it survives a restart and never blocks the request that asked for it.
//!   [`MailQueue::enqueue`] (or [`enqueue`] for an async caller) is one
//!   insert.
//! - [`Composer`]: what turns a queued row into a message, **at send time**.
//!   A row holds a kind, a recipient and an opaque payload, and never the
//!   message itself. A sign-in code is therefore minted by its composer in
//!   the worker, at the moment of sending, and is never at rest in the outbox
//!   (§7.2, E11). A composer may also decide that nothing is owed (§7.5).
//! - [`MailSend`], the `mail.send` worker: claims due rows, composes, submits,
//!   and deletes the row once the relay or API accepted it. A failure that
//!   can pass is retried with a growing delay, up to `ORDO_MAIL_MAX_ATTEMPTS`;
//!   one that cannot is dropped and counted.
//!
//! # Delivery is at least once
//!
//! A row is claimed with a lease. A replica that dies after the relay
//! accepted a message and before the row was deleted leaves the row to be
//! claimed again when the lease ends, and the message is sent twice. So is
//! one whose single send outlasts `ORDO_MAIL_LEASE_SECS` (floor 30 s). Those
//! are the only two windows. Ordo resends in both and accepts a rare
//! duplicate (owner, 2026-10-22). Every try of one row carries the **same
//! `Message-ID`**, `<ordo-ROWID-QUEUEDAT@domain>`, made from the row and the
//! sender address's domain, so a mail client can fold the copies. The window
//! rules: each row is leased when its own send starts,
//! the lease is rounded up past the whole-second clock, and a settle only
//! touches the claim it belongs to. For a
//! sign-in code that is harmless: the second send mints a fresh code. The
//! alternative, deleting first, loses mail on a crash.
//!
//! # What is never written down
//!
//! No secret reaches a log, a metric, an error or a `Debug` rendering: the
//! relay password and the API key are held in types with no `Debug`, and a
//! refusal names the setting, never its value. No recipient address reaches a
//! log either: a dropped message is reported by its row id and kind.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_egress::smtp::{self, SmtpLogin, SmtpTls};
use ordo_egress::{
    Allowlist, DenyList, FetchMethod, FetchRequest, HttpsUrl, Refusal, TlsRoots, DEFAULT_TIMEOUT,
};
use ordo_exec::{sql_lit, Pool, PoolError, TextRow};
use ordo_identity::FaultClass;

use crate::metrics::Metrics;
use crate::passkeys::{scrub, Source, SyncPg};
use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// `smtp`, `resend` or `sendgrid`. Unset is off.
pub const KIND_ENV: &str = "ORDO_MAIL_KIND";
/// The sender address, bare.
pub const FROM_ENV: &str = "ORDO_MAIL_FROM";
/// The relay's host name (`smtp`).
pub const HOST_ENV: &str = "ORDO_MAIL_HOST";
/// The relay's port (`smtp`), default 465.
pub const PORT_ENV: &str = "ORDO_MAIL_PORT";
/// A file: the relay user on the first line, the password on the second.
pub const CREDENTIALS_FILE_ENV: &str = "ORDO_MAIL_CREDENTIALS_FILE";
/// A file holding the API key (`resend`, `sendgrid`).
pub const KEY_FILE_ENV: &str = "ORDO_MAIL_KEY_FILE";
/// How many times one message is tried before it is dropped. Default 5.
pub const MAX_ATTEMPTS_ENV: &str = "ORDO_MAIL_MAX_ATTEMPTS";
/// How often the outbox is read, in seconds. Default 2.
pub const SEND_SECS_ENV: &str = "ORDO_MAIL_SEND_SECS";
/// How the relay connection becomes TLS (`smtp`): `implicit` or `starttls`.
pub const TLS_ENV: &str = "ORDO_MAIL_TLS";
/// The wait before the second attempt, in seconds. Default 15.
pub const RETRY_BASE_SECS_ENV: &str = "ORDO_MAIL_RETRY_BASE_SECS";
/// The longest wait between attempts, in seconds. Default 900.
pub const RETRY_MAX_SECS_ENV: &str = "ORDO_MAIL_RETRY_MAX_SECS";
/// Exact host names let through the private-address deny list. Empty by
/// default. Shared with every deployment-scoped egress.
pub const PRIVATE_HOSTS_ENV: &str = ordo_egress::PRIVATE_HOSTS_ENV;
/// Rows claimed per pass. Default 16.
pub const BATCH_ENV: &str = "ORDO_MAIL_BATCH";
/// How long a claimed row is hidden from other replicas, in seconds.
/// Default 120.
pub const LEASE_SECS_ENV: &str = "ORDO_MAIL_LEASE_SECS";

/// The worker's name: a metric label and a census line.
pub const WORKER_NAME: &str = "mail.send";

/// Mail APIs the owner placed after the first deployment (18z).
const LATER_KINDS: [&str; 5] = ["mailgun", "postmark", "brevo", "mailjet", "ses"];

// ───────────────────────────── addresses ──────────────────────────────

/// An address that matches the strict grammar ([`smtp::address_ok`]): no
/// display name, no control character, nothing a header could be built from
/// but the address itself (E13).
///
/// Its `Debug` does not print the address: an address is personal data, and
/// `Debug` is how values reach logs by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct MailAddress(String);

impl MailAddress {
    /// Check `s` and keep it as written. Normalising (trim, lowercase) is
    /// the caller's: step 5 normalises a sign-in address before it is
    /// hashed, and a sender address is the app developer's to spell.
    pub fn parse(s: &str) -> Result<MailAddress, MailError> {
        if smtp::address_ok(s) {
            Ok(MailAddress(s.to_string()))
        } else {
            Err(MailError::not_representable(
                "an address outside the accepted grammar",
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MailAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MailAddress(..)")
    }
}

/// One message to one recipient: a subject and plain text.
///
/// **No `Debug`, no `Clone`.** A sign-in code is in the text, and the only
/// place it goes is the relay. Both parts are checked here, so a message that
/// exists can be written exactly: the subject is printable ASCII, at most 200
/// bytes; the text is printable ASCII lines, at most 16 KiB.
pub struct Message {
    to: MailAddress,
    subject: String,
    text: String,
    /// `<local@domain>`, set by the worker from the row, never by a composer.
    message_id: Option<String>,
}

/// Why a subject or text cannot be sent. Over the setting's value it names
/// the setting and the value to set; past the most the setting allows it says
/// so; otherwise it names the shape rule (audit 2026-10-11, finding 22).
fn over_cap(part: &str, len: usize, cap: usize, setting: &str, max: usize, shape: &str) -> String {
    if len > max {
        format!(
            "the {part} is {len} bytes, over the most any setting allows ({setting} at most \
             {max}); shorten it"
        )
    } else if len > cap {
        format!(
            "the {part} is {len} bytes, over {setting}={cap}; set {setting}={len} or more \
             (up to {max}) to send it"
        )
    } else {
        format!("the {part} {shape}")
    }
}

impl Message {
    pub fn new(to: MailAddress, subject: &str, text: &str) -> Result<Message, MailError> {
        if !smtp::subject_ok(subject) {
            return Err(MailError::not_representable(over_cap(
                "subject",
                subject.len(),
                smtp::subject_cap(),
                "ORDO_MAIL_SUBJECT_MAX_BYTES",
                998,
                "must be printable ASCII and not empty",
            )));
        }
        if !smtp::text_ok(text) {
            return Err(MailError::not_representable(over_cap(
                "text",
                text.len(),
                smtp::text_cap(),
                "ORDO_MAIL_TEXT_MAX_BYTES",
                1024 * 1024,
                "must be printable ASCII, each line at most 998 bytes",
            )));
        }
        Ok(Message {
            to,
            subject: subject.to_string(),
            text: text.to_string(),
            message_id: None,
        })
    }

    /// Fix the `Message-ID` this message goes out with (`<local@domain>`).
    pub fn with_message_id(mut self, id: String) -> Message {
        self.message_id = Some(id);
        self
    }

    /// The `Message-ID`, when the worker fixed one for the row.
    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn to(&self) -> &MailAddress {
        &self.to
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The error underneath a [`MailError`], kept for `source()`.
type Cause = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// Why a message was not sent. The text is for the deployment's own log: it
/// names a step, a code or a setting, and never an address or a secret.
///
/// What a caller may do about it is read from the variant, through
/// [`MailError::class`] and the `is_*` methods, never from the words. The
/// error underneath (the egress `Refusal`, the pool's busy `PoolError`) is
/// its `source()`, and [`MailError::relay_code`] is the SMTP reply code or
/// HTTP status the relay answered, when it answered one.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MailError {
    /// The message, an address or a payload cannot be written as given.
    /// Refused before a socket opens. Never retried.
    NotRepresentable { detail: String },
    /// The relay or API could not be reached, or asked for a later attempt.
    /// Retried.
    Unavailable {
        detail: String,
        code: Option<u16>,
        source: Option<Cause>,
    },
    /// The API asked for a later attempt and said when: a 429 or a 5xx
    /// with `Retry-After` in seconds. Retried, no sooner than that.
    Deferred {
        detail: String,
        retry_after_secs: u64,
        code: Option<u16>,
        source: Option<Cause>,
    },
    /// The outbox's pool was full: nothing was queued, and the same
    /// request can be sent again shortly (`PoolError::is_busy`).
    Busy { source: Option<Cause> },
    /// The relay or API answered no, or answered something other than the
    /// one status that means accepted. Not retried: a retry could send the
    /// message twice.
    Refused {
        detail: String,
        code: Option<u16>,
        source: Option<Cause>,
    },
    /// The deployment's own settings stop the send: the key or login was
    /// not accepted, the host is outside the egress perimeter. Not retried
    /// until a setting changes.
    Misconfigured {
        detail: String,
        code: Option<u16>,
        source: Option<Cause>,
    },
}

impl MailError {
    pub fn not_representable(detail: impl Into<String>) -> MailError {
        MailError::NotRepresentable {
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> MailError {
        MailError::Unavailable {
            detail: detail.into(),
            code: None,
            source: None,
        }
    }

    pub fn deferred(detail: impl Into<String>, retry_after_secs: u64) -> MailError {
        MailError::Deferred {
            detail: detail.into(),
            retry_after_secs,
            code: None,
            source: None,
        }
    }

    pub fn refused(detail: impl Into<String>) -> MailError {
        MailError::Refused {
            detail: detail.into(),
            code: None,
            source: None,
        }
    }

    /// A full pool or a store that could not answer. The class is read from
    /// the pool error's type. A full pool names nothing, so it is kept as
    /// the cause; any other pool error can carry a statement, and with it a
    /// recipient, so only its scrubbed sentence is kept.
    pub fn from_pool(e: PoolError) -> MailError {
        if e.is_busy() {
            MailError::Busy {
                source: Some(Arc::new(e)),
            }
        } else {
            MailError::unavailable(scrub(OUTBOX, e))
        }
    }

    /// What a caller can do about this, in the one type every service
    /// shares. The outbox, the sign-in edge and the store mapping all read
    /// this and nothing else.
    pub fn class(&self) -> FaultClass {
        match self {
            MailError::Busy { .. } => FaultClass::Busy,
            MailError::Unavailable { .. } | MailError::Deferred { .. } => FaultClass::Retryable,
            MailError::NotRepresentable { .. } | MailError::Refused { .. } => FaultClass::Refused,
            MailError::Misconfigured { .. } => FaultClass::Broken,
        }
    }

    /// Will the same message, sent again later, be accepted?
    pub fn retryable(&self) -> bool {
        self.class().may_retry()
    }

    /// The relay or API asked for a later attempt (or could not be reached).
    pub fn is_deferred(&self) -> bool {
        matches!(
            self,
            MailError::Unavailable { .. } | MailError::Deferred { .. }
        )
    }

    /// The outbox's pool was full.
    pub fn is_busy(&self) -> bool {
        matches!(self, MailError::Busy { .. })
    }

    /// The relay or API said no, for good.
    pub fn is_permanently_refused(&self) -> bool {
        matches!(self, MailError::Refused { .. })
    }

    /// The message or an address cannot be written as given.
    pub fn is_bad_input(&self) -> bool {
        matches!(self, MailError::NotRepresentable { .. })
    }

    /// A setting of the deployment stops the send.
    pub fn is_misconfigured(&self) -> bool {
        matches!(self, MailError::Misconfigured { .. })
    }

    /// The wait the relay or API asked for, when it named one.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            MailError::Deferred {
                retry_after_secs, ..
            } => Some(*retry_after_secs),
            _ => None,
        }
    }

    /// The SMTP reply code or HTTP status the relay answered, when it did.
    pub fn relay_code(&self) -> Option<u16> {
        match self {
            MailError::Unavailable { code, .. }
            | MailError::Deferred { code, .. }
            | MailError::Refused { code, .. }
            | MailError::Misconfigured { code, .. } => *code,
            _ => None,
        }
    }

    /// The refusal's detail, then the setting that would fix it, if one
    /// would. Never an address or a secret. The class is read from the
    /// refusal's variant, status and code.
    fn from_refusal_with(r: Refusal, fix: Option<String>) -> MailError {
        let mut detail = r.operational();
        if let Some(fix) = fix {
            detail.push_str("; ");
            detail.push_str(&fix);
        }
        let code = match &r {
            Refusal::Rejected { status, .. } => Some(*status),
            Refusal::RelayRefused { code } => Some(*code),
            _ => None,
        };
        let wait = r.retry_after().map(|w| w.as_secs());
        let retryable = r.retryable();
        let config = matches!(
            r,
            Refusal::NotHttps { .. }
                | Refusal::MalformedUrl { .. }
                | Refusal::NotAllowlisted { .. }
                | Refusal::DeniedAddress { .. }
                | Refusal::Rejected {
                    status: 401 | 403,
                    ..
                }
                | Refusal::RelayRefused { code: 530 | 535 }
        );
        if matches!(r, Refusal::NotRepresentable { .. }) {
            return MailError::NotRepresentable { detail };
        }
        let source: Option<Cause> = Some(Arc::new(r));
        if let Some(retry_after_secs) = wait {
            MailError::Deferred {
                detail,
                retry_after_secs,
                code,
                source,
            }
        } else if retryable {
            MailError::Unavailable {
                detail,
                code,
                source,
            }
        } else if config {
            MailError::Misconfigured {
                detail,
                code,
                source,
            }
        } else {
            MailError::Refused {
                detail,
                code,
                source,
            }
        }
    }
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailError::NotRepresentable { detail } => write!(f, "not representable: {detail}"),
            MailError::Unavailable { detail, .. } => write!(f, "unavailable: {detail}"),
            MailError::Deferred {
                detail,
                retry_after_secs,
                ..
            } => write!(
                f,
                "unavailable: {detail}; asked to wait {retry_after_secs}s"
            ),
            MailError::Busy { .. } => {
                write!(f, "unavailable: {}", ordo_exec::SATURATED_MESSAGE)
            }
            MailError::Refused { detail, .. } => write!(f, "refused: {detail}"),
            MailError::Misconfigured { detail, .. } => write!(f, "misconfigured: {detail}"),
        }
    }
}

impl std::error::Error for MailError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MailError::Unavailable { source, .. }
            | MailError::Deferred { source, .. }
            | MailError::Busy { source }
            | MailError::Refused { source, .. }
            | MailError::Misconfigured { source, .. } => source
                .as_deref()
                .map(|s| s as &(dyn std::error::Error + 'static)),
            MailError::NotRepresentable { .. } => None,
        }
    }
}

// ───────────────────────────── mailers ────────────────────────────────

/// Submit one message, once. Synchronous, as the plan has it (§7.6): it is
/// called from the outbox worker's blocking thread, and an implementation
/// that does network I/O blocks on the runtime there, as
/// `federation::EgressTransport` does.
pub trait Mailer: Send + Sync {
    fn submit(&self, msg: &Message) -> Result<(), MailError>;
    /// The domain a `Message-ID` is made under: the sender address's domain.
    fn mail_domain(&self) -> String {
        "localhost.invalid".into()
    }
    /// One line for the startup log. No secret.
    fn label(&self) -> String;
}

fn block_on<F: std::future::Future>(f: F) -> Result<F::Output, MailError> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| MailError::unavailable("no runtime to send on"))?;
    Ok(handle.block_on(f))
}

fn domain_of(from: &MailAddress) -> String {
    from.as_str()
        .rsplit_once('@')
        .map(|(_, d)| d.to_ascii_lowercase())
        .unwrap_or_else(|| "localhost.invalid".into())
}

/// The `Message-ID` of one outbox row. Derived from the row's id and its
/// queue time, which never change, and the mail domain, so every try of one
/// email carries the same value.
pub fn message_id_for(id: i64, queued_at: u64, domain: &str) -> String {
    format!("<ordo-{id}-{queued_at}@{domain}>")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A relay over implicit TLS, required STARTTLS, or no TLS when the app
/// developer chose it, with `AUTH PLAIN`.
pub struct SmtpMailer {
    host: String,
    port: u16,
    tls: SmtpTls,
    from: MailAddress,
    login: Option<SmtpLogin>,
    roots: TlsRoots,
    deny: DenyList,
    timeout: Duration,
}

impl SmtpMailer {
    /// A relay checked against the public root program.
    pub fn new(
        host: &str,
        port: u16,
        from: MailAddress,
        login: Option<SmtpLogin>,
        deny: DenyList,
    ) -> SmtpMailer {
        SmtpMailer::with_roots(host, port, from, login, TlsRoots::webpki(), deny)
    }

    /// The same, with the trust anchors named: a relay behind a private CA,
    /// or a test's listener. Certificate validation stays on either way.
    pub fn with_roots(
        host: &str,
        port: u16,
        from: MailAddress,
        login: Option<SmtpLogin>,
        roots: TlsRoots,
        deny: DenyList,
    ) -> SmtpMailer {
        SmtpMailer {
            host: host.to_ascii_lowercase(),
            port,
            tls: SmtpTls::Implicit,
            from,
            login,
            roots,
            deny,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// How TLS starts: implicit (the default), required STARTTLS, or none.
    pub fn with_tls(mut self, tls: SmtpTls) -> SmtpMailer {
        self.tls = tls;
        self
    }

    async fn go(&self, msg: &Message) -> Result<(), Refusal> {
        let mail = smtp::Mail {
            from: self.from.as_str(),
            to: msg.to.as_str(),
            subject: &msg.subject,
            text: &msg.text,
            date: now_secs(),
            message_id: msg.message_id(),
        };
        // Every check on the message runs before the name is resolved.
        smtp::render(&mail)?;
        let dest = ordo_egress::resolve_and_pin_host(
            &self.host,
            self.port,
            &Allowlist::exactly(&self.host),
            &self.deny,
        )
        .await?;
        smtp::submit_over(
            self.tls,
            dest,
            &self.roots,
            self.login.as_ref(),
            mail,
            self.timeout,
        )
        .await?;
        Ok(())
    }
}

/// The setting that would fix a refusal any mailer can meet, if one would.
fn common_fix(r: &Refusal) -> Option<String> {
    use ordo_egress::DeniedCategory as C;
    match r {
        Refusal::DeniedAddress {
            host,
            category: C::Loopback | C::Private | C::UniqueLocal | C::Cgnat | C::DeploymentRange,
            ..
        } => Some(format!(
            "to let this deployment reach `{host}` at a private address, add it to \
                 {PRIVATE_HOSTS_ENV} (ORDO_EGRESS_PRIVATE_HOSTS={host})"
        )),
        _ => None,
    }
}

/// What to set when a mail API answers 400 or 422, the usual answer to a
/// sender it has not verified.
fn api_status_fix(status: u16, word: &str) -> String {
    match status {
        400 | 422 => format!(
            ". If the provider refused the sender, set {FROM_ENV} to an address or domain \
             verified with {word}"
        ),
        _ => String::new(),
    }
}

/// The setting that would fix an SMTP refusal, if one would. Names the
/// setting and the value, never a secret.
fn smtp_fix(r: &Refusal, tls: SmtpTls, has_login: bool) -> Option<String> {
    if let Some(fix) = common_fix(r) {
        return Some(fix);
    }
    match (r, tls) {
        (Refusal::TlsFailed { detail }, SmtpTls::StartTls) if detail == smtp::NO_STARTTLS => {
            Some(format!(
                "set {TLS_ENV}=none to send without encryption, or use port 465 with \
                 {TLS_ENV}=implicit"
            ))
        }
        (Refusal::TlsFailed { .. }, SmtpTls::StartTls) => Some(format!(
            "check the relay's certificate and its STARTTLS; {TLS_ENV}=implicit fits a \
             port 465 relay, and {TLS_ENV}=none sends without encryption"
        )),
        (Refusal::TlsFailed { .. }, SmtpTls::Implicit) => Some(format!(
            "if the relay expects STARTTLS (usually port 587), set {TLS_ENV}=starttls; if it \
             speaks no TLS at all, {TLS_ENV}=none sends without encryption"
        )),
        (Refusal::RelayRefused { code: 530 }, _) if !has_login => Some(format!(
            "the relay wants a login: set {CREDENTIALS_FILE_ENV} to a file with the user on \
             line 1 and the password on line 2{}",
            if tls == SmtpTls::None {
                format!(" (if it wants TLS first, set {TLS_ENV}=starttls)")
            } else {
                String::new()
            }
        )),
        (Refusal::RelayRefused { code: 530 }, SmtpTls::None) => Some(format!(
            "the relay requires TLS: set {TLS_ENV}=starttls, or {TLS_ENV}=implicit on port 465"
        )),
        (
            Refusal::RelayRefused {
                code: 535 | 534 | 530,
            },
            _,
        ) => Some(format!(
            "the relay did not accept the login: check the user and password in \
             {CREDENTIALS_FILE_ENV}"
        )),
        (Refusal::RelayRefused { code: 502 | 504 }, SmtpTls::None) => Some(format!(
            "the relay may not take a login without TLS: set {TLS_ENV}=starttls, or let the \
             relay accept AUTH in plaintext (Mailpit: --smtp-auth-accept-any \
             --smtp-auth-allow-insecure)"
        )),
        (Refusal::MalformedResponse { .. }, SmtpTls::Implicit) => Some(format!(
            "the relay answered something that is not SMTP over TLS: if it expects STARTTLS \
             (usually port 587), set {TLS_ENV}=starttls and {PORT_ENV}=587"
        )),
        (Refusal::TimedOut { .. }, _) => Some(format!(
            "the relay did not answer in time. The usual cause is the wrong port or TLS mode: \
             {PORT_ENV}=587 with {TLS_ENV}=starttls, or {PORT_ENV}=465 with {TLS_ENV}=implicit"
        )),
        (
            Refusal::RelayRefused {
                code: 550 | 553 | 554,
            },
            _,
        ) => Some(format!(
            "the relay refused the sender or the recipient. If it refused the sender, set \
             {FROM_ENV} to an address the relay lets this login send from"
        )),
        (Refusal::ConnectFailed { .. }, _) => Some(format!("check {HOST_ENV} and {PORT_ENV}")),
        (Refusal::Unresolvable { .. }, _) => Some(format!("check {HOST_ENV}")),
        _ => None,
    }
}

impl Mailer for SmtpMailer {
    fn submit(&self, msg: &Message) -> Result<(), MailError> {
        block_on(self.go(msg))?.map_err(|r| {
            let fix = smtp_fix(&r, self.tls, self.login.is_some());
            MailError::from_refusal_with(r, fix)
        })
    }

    fn mail_domain(&self) -> String {
        domain_of(&self.from)
    }

    fn label(&self) -> String {
        format!(
            "smtp to {}:{} over {}, from {}{}",
            self.host,
            self.port,
            match self.tls {
                SmtpTls::Implicit => "implicit TLS",
                SmtpTls::StartTls => "required STARTTLS",
                SmtpTls::None =>
                    "NO TLS (ORDO_MAIL_TLS=none: the relay password and every \
                                  message cross the network readable)",
            },
            self.from.as_str(),
            if self.login.is_some() {
                ""
            } else {
                ", no login"
            }
        )
    }
}

/// A mail API that takes one JSON `POST` with a bearer key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailApi {
    Resend,
    SendGrid,
}

impl MailApi {
    pub fn word(self) -> &'static str {
        match self {
            MailApi::Resend => "resend",
            MailApi::SendGrid => "sendgrid",
        }
    }

    /// The send endpoint.
    pub fn url(self) -> &'static str {
        match self {
            MailApi::Resend => "https://api.resend.com/emails",
            MailApi::SendGrid => "https://api.sendgrid.com/v3/mail/send",
        }
    }

    /// The one status that means the message was accepted. Any other answer,
    /// another 2xx included, is not counted as sent.
    pub fn accepted(self) -> u16 {
        match self {
            MailApi::Resend => 200,
            MailApi::SendGrid => 202,
        }
    }

    /// The request body for one message.
    pub fn body(self, from: &str, msg: &Message) -> String {
        let v = match self {
            MailApi::Resend => serde_json::json!({
                "from": from,
                "to": [msg.to.as_str()],
                "subject": msg.subject,
                "text": msg.text,
            }),
            MailApi::SendGrid => serde_json::json!({
                "personalizations": [{"to": [{"email": msg.to.as_str()}]}],
                "from": {"email": from},
                "subject": msg.subject,
                "content": [{"type": "text/plain", "value": msg.text}],
            }),
        };
        let mut v = v;
        if let Some(id) = msg.message_id() {
            v["headers"] = serde_json::json!({ "Message-ID": id });
        }
        v.to_string()
    }
}

/// An API key. **No `Debug`, no `Clone`, no `Display`.** It leaves this type
/// once per send, as the value of the `Authorization` header.
pub struct ApiKey(String);

impl ApiKey {
    /// Refused when empty or when it holds anything a header value cannot:
    /// a control character or a space. The refusal does not repeat it.
    pub fn new(key: &str) -> Result<ApiKey, String> {
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|b| (0x21..0x7f).contains(&b)) {
            return Err("the key is empty or holds a character a header cannot carry".into());
        }
        Ok(ApiKey(key.to_string()))
    }
}

/// Resend or SendGrid, through `ordo_egress::post`: the status is the answer
/// and no body is read.
pub struct ApiMailer {
    api: MailApi,
    url: HttpsUrl,
    key: ApiKey,
    from: MailAddress,
    roots: TlsRoots,
    deny: DenyList,
    timeout: Duration,
}

impl ApiMailer {
    /// The provider's own endpoint, checked against the public root program.
    pub fn new(api: MailApi, key: ApiKey, from: MailAddress, deny: DenyList) -> ApiMailer {
        let url = HttpsUrl::parse(api.url()).expect("the provider URLs are https");
        ApiMailer::at(api, url, TlsRoots::webpki(), key, from, deny)
    }

    /// Another endpoint of the same shape, with its own trust anchors: a
    /// test's listener, or a provider's regional host.
    pub fn at(
        api: MailApi,
        url: HttpsUrl,
        roots: TlsRoots,
        key: ApiKey,
        from: MailAddress,
        deny: DenyList,
    ) -> ApiMailer {
        ApiMailer {
            api,
            url,
            key,
            from,
            roots,
            deny,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    async fn go(&self, msg: &Message) -> Result<u16, Refusal> {
        let body = self.api.body(self.from.as_str(), msg);
        let dest = ordo_egress::resolve_and_pin(
            &self.url,
            &Allowlist::exactly(self.url.host()),
            &self.deny,
        )
        .await?;
        // The key joins the request here, per call, and nothing keeps it
        // (pin 5).
        let headers = [("authorization", format!("Bearer {}", self.key.0))];
        let accepted = ordo_egress::post(
            dest,
            &self.roots,
            FetchRequest {
                method: FetchMethod::PostJson,
                body: body.as_bytes(),
                accept: "application/json",
                headers: &headers,
            },
            self.timeout,
        )
        .await?;
        Ok(accepted.status)
    }
}

impl Mailer for ApiMailer {
    fn submit(&self, msg: &Message) -> Result<(), MailError> {
        let status = block_on(self.go(msg))?.map_err(|r| {
            let fix = common_fix(&r).or_else(|| match r {
                Refusal::Rejected {
                    status: 401 | 403, ..
                } => Some(format!(
                    "{} did not accept the key: check {KEY_FILE_ENV}",
                    self.api.word()
                )),
                _ => None,
            });
            MailError::from_refusal_with(r, fix)
        })?;
        if status == self.api.accepted() {
            Ok(())
        } else {
            let fix = api_status_fix(status, self.api.word());
            Err(MailError::Refused {
                detail: format!(
                    "{} answered {status}, and only {} means accepted{fix}",
                    self.api.word(),
                    self.api.accepted()
                ),
                code: Some(status),
                source: None,
            })
        }
    }

    fn mail_domain(&self) -> String {
        domain_of(&self.from)
    }

    fn label(&self) -> String {
        format!(
            "{} at {}, from {}",
            self.api.word(),
            self.url.host(),
            self.from.as_str()
        )
    }
}

// ───────────────────────────── settings ───────────────────────────────

/// How the outbox is drained: the app developer's numbers, each a setting
/// with a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPolicy {
    /// `ORDO_MAIL_MAX_ATTEMPTS`, default 5, from 1 to 20.
    pub max_attempts: u32,
    /// `ORDO_MAIL_SEND_SECS`, default 2, from 1 to 60. How often the outbox
    /// is read when it was empty; a full batch is followed at once.
    pub every: Duration,
    /// `ORDO_MAIL_BATCH`, default 16, from 1 to 500. Rows claimed per pass.
    pub batch: usize,
    /// `ORDO_MAIL_LEASE_SECS`, default 120, from 30 to 3600. How long a
    /// claimed row is hidden from other replicas. Each row is leased when
    /// its own send starts, so the lease covers one send. The floor of 30 is
    /// above the 20 s send timeout.
    pub lease_secs: u64,
    /// `ORDO_MAIL_RETRY_BASE_SECS`, default 15, from 1 to 3600. The wait
    /// before the second attempt.
    pub retry_base_secs: u64,
    /// `ORDO_MAIL_RETRY_MAX_SECS`, default 900, from 1 to 86400 and no less
    /// than the base. The longest wait between attempts.
    pub retry_max_secs: u64,
}

impl Default for SendPolicy {
    fn default() -> SendPolicy {
        SendPolicy {
            max_attempts: 5,
            every: Duration::from_secs(2),
            batch: 16,
            lease_secs: 120,
            retry_base_secs: 15,
            retry_max_secs: 900,
        }
    }
}

impl SendPolicy {
    /// The wait before attempt `attempt + 1`: the base, doubling, at most
    /// the max. With the defaults (15 s to 15 minutes, 5 attempts) a message
    /// is given up about 7½ minutes after it was queued.
    pub fn backoff(&self, attempt: u32) -> u64 {
        let n = attempt.saturating_sub(1).min(32);
        self.retry_base_secs
            .saturating_mul(1u64 << n)
            .min(self.retry_max_secs)
    }

    /// Does a failed attempt get another? The one place the outbox decides
    /// between retry and drop, from the error's class alone: a busy or
    /// deferred send is retried until the attempts run out, and anything
    /// else (bad input, a refusal, a broken setting) is dropped.
    pub fn retries(&self, attempt: u32, e: &MailError) -> bool {
        e.class().may_retry() && attempt < self.max_attempts
    }

    /// The wait before the next attempt after `e`: the backoff, or the
    /// relay's own `Retry-After` when that is longer. A relay that rate
    /// limits (429) is waited out, never counted as a refusal.
    pub fn wait_after(&self, attempt: u32, e: &MailError) -> u64 {
        self.backoff(attempt).max(e.retry_after_secs().unwrap_or(0))
    }
}

/// The configured channel: the mailer, how its outbox is drained, and the
/// line the startup log prints.
#[derive(Clone)]
pub struct MailChannel {
    pub mailer: Arc<dyn Mailer>,
    pub policy: SendPolicy,
    pub label: String,
}

impl MailChannel {
    /// Read the settings from the process environment. `Ok(None)` is off.
    pub fn from_env() -> Result<Option<MailChannel>, (&'static str, String)> {
        let var = |k: &str| std::env::var(k).ok();
        let read_file = |path: &str| std::fs::read_to_string(path).map_err(|e| e.to_string());
        MailChannel::from_vars(&var, &read_file)
    }

    /// Read the settings through `var`, and secret files through
    /// `read_file`.
    ///
    /// | Setting | |
    /// |---|---|
    /// | `ORDO_MAIL_KIND` | `smtp`, `resend` or `sendgrid`. Unset is off |
    /// | `ORDO_MAIL_FROM` | required: the sender address, bare |
    /// | `ORDO_MAIL_HOST` | `smtp`: required, the relay's host name |
    /// | `ORDO_MAIL_PORT` | `smtp`: default 465, any port from 1 to 65535 |
    /// | `ORDO_MAIL_TLS` | `smtp`: `implicit` (default), `starttls` (required, never opportunistic), or `none` (no encryption; the startup line says so) |
    /// | `ORDO_MAIL_CREDENTIALS_FILE` | `smtp`: optional; user on line 1, password on line 2. Without it no `AUTH` is sent |
    /// | `ORDO_MAIL_KEY_FILE` | `resend`, `sendgrid`: required; the API key |
    /// | `ORDO_MAIL_MAX_ATTEMPTS` | default 5, 1 to 20 |
    /// | `ORDO_MAIL_SEND_SECS` | default 2, 1 to 60 |
    /// | `ORDO_MAIL_RETRY_BASE_SECS` | default 15, 1 to 3600 |
    /// | `ORDO_MAIL_RETRY_MAX_SECS` | default 900, 1 to 86400, at least the base |
    /// | `ORDO_MAIL_BATCH` | default 16, 1 to 500 |
    /// | `ORDO_MAIL_LEASE_SECS` | default 120, 30 to 3600 |
    /// | `ORDO_EGRESS_DENY_CIDRS` | the deployment's deny list, shared with every egress |
    /// | `ORDO_EGRESS_PRIVATE_HOSTS` | exact host names, IP addresses or CIDR ranges let through the private categories; empty by default; checked at every boot, mail on or off |
    ///
    /// Every refusal names a setting and never a value read from a file.
    pub fn from_vars(
        var: &dyn Fn(&str) -> Option<String>,
        read_file: &dyn Fn(&str) -> Result<String, String>,
    ) -> Result<Option<MailChannel>, (&'static str, String)> {
        let get = |k: &str| {
            var(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        // The egress settings are checked whether or not mail is on, so a
        // bad entry refuses every boot (owner, 2026-10-09).
        let deny = DenyList::from_vars(&|k| var(k))?;
        let private_hosts: Vec<String> =
            deny.private_hosts().iter().map(|h| h.to_string()).collect();
        let Some(kind) = get(KIND_ENV) else {
            return Ok(None);
        };
        let kind = kind.to_ascii_lowercase();
        if LATER_KINDS.contains(&kind.as_str()) {
            return Err((
                KIND_ENV,
                format!(
                    "`{kind}` is planned for after the first deployment (checklist 18z); \
                     smtp, resend and sendgrid are served"
                ),
            ));
        }
        let smtp_kind = match kind.as_str() {
            "smtp" => true,
            "resend" | "sendgrid" => false,
            other => {
                return Err((
                    KIND_ENV,
                    format!("`{other}` is not a mail kind (smtp, resend, sendgrid)"),
                ))
            }
        };
        // A secret in a variable's value shows in a process listing.
        for (k, file) in [
            ("ORDO_MAIL_PASSWORD", CREDENTIALS_FILE_ENV),
            ("ORDO_MAIL_KEY", KEY_FILE_ENV),
        ] {
            if var(k).is_some() {
                return Err((
                    k,
                    format!(
                        "{k} is not read: a secret in a variable's value shows in a process \
                         listing; put it in a file and name the file in {file}"
                    ),
                ));
            }
        }
        // A setting for the other kind is a mistake worth saying out loud.
        let other_kind: &[&'static str] = if smtp_kind {
            &[KEY_FILE_ENV]
        } else {
            &[HOST_ENV, PORT_ENV, TLS_ENV, CREDENTIALS_FILE_ENV]
        };
        for k in other_kind {
            if get(k).is_some() {
                return Err((
                    *k,
                    format!("{k} does not apply to ORDO_MAIL_KIND={kind}; unset it"),
                ));
            }
        }
        let from = get(FROM_ENV).ok_or((FROM_ENV, format!("{FROM_ENV} is required")))?;
        let from = MailAddress::parse(&from).map_err(|_| {
            (
                FROM_ENV,
                "not a bare address (name@example.com, no display name)".to_string(),
            )
        })?;
        let ranged = |k: &'static str, default: u64, lo: u64, hi: u64| match get(k) {
            None => Ok(default),
            Some(v) => match v.parse::<u64>() {
                Ok(n) if (lo..=hi).contains(&n) => Ok(n),
                _ => Err((
                    k,
                    format!("`{v}` is not a whole number from {lo} to {hi} (default {default})"),
                )),
            },
        };
        let d = SendPolicy::default();
        let policy = SendPolicy {
            max_attempts: ranged(MAX_ATTEMPTS_ENV, u64::from(d.max_attempts), 1, 20)? as u32,
            every: Duration::from_secs(ranged(SEND_SECS_ENV, d.every.as_secs(), 1, 60)?),
            batch: ranged(BATCH_ENV, d.batch as u64, 1, 500)? as usize,
            lease_secs: ranged(LEASE_SECS_ENV, d.lease_secs, 30, 3600)?,
            retry_base_secs: ranged(RETRY_BASE_SECS_ENV, d.retry_base_secs, 1, 3600)?,
            retry_max_secs: ranged(RETRY_MAX_SECS_ENV, d.retry_max_secs, 1, 86_400)?,
        };
        if policy.retry_max_secs < policy.retry_base_secs {
            return Err((
                RETRY_MAX_SECS_ENV,
                format!(
                    "{RETRY_MAX_SECS_ENV} ({}) is less than {RETRY_BASE_SECS_ENV} ({})",
                    policy.retry_max_secs, policy.retry_base_secs
                ),
            ));
        }
        let mailer: Arc<dyn Mailer> = if smtp_kind {
            let host = get(HOST_ENV).ok_or((HOST_ENV, format!("{HOST_ENV} is required")))?;
            if host.contains("://") {
                return Err((
                    HOST_ENV,
                    "a host name, with no scheme; how TLS starts is ORDO_MAIL_TLS".into(),
                ));
            }
            let port = ranged(PORT_ENV, u64::from(smtp::SMTPS_PORT), 1, 65_535)? as u16;
            let tls = match get(TLS_ENV) {
                None => SmtpTls::Implicit,
                Some(v) => SmtpTls::parse(&v).ok_or_else(|| {
                    (
                        TLS_ENV,
                        format!(
                            "`{v}` is not a TLS mode: set {TLS_ENV} to implicit (port 465), \
                             starttls (port 587) or none (no encryption)"
                        ),
                    )
                })?,
            };
            HttpsUrl::parse(&format!("https://{host}:{port}/"))
                .map_err(|e| (HOST_ENV, format!("not a host name: {}", e.disclosable())))?;
            let login = match get(CREDENTIALS_FILE_ENV) {
                None => None,
                Some(file) => {
                    let text = read_file(&file).map_err(|_| {
                        (
                            CREDENTIALS_FILE_ENV,
                            format!("{CREDENTIALS_FILE_ENV} could not be read"),
                        )
                    })?;
                    let mut lines = text.lines();
                    let user = lines.next().unwrap_or("").trim_end_matches('\r');
                    let password = lines.next().unwrap_or("").trim_end_matches('\r');
                    Some(SmtpLogin::new(user, password).map_err(|_| {
                        (
                            CREDENTIALS_FILE_ENV,
                            format!(
                                "{CREDENTIALS_FILE_ENV} must hold the user on its first line \
                                 and the password on its second, with no control characters"
                            ),
                        )
                    })?)
                }
            };
            Arc::new(SmtpMailer::new(&host, port, from, login, deny).with_tls(tls))
        } else {
            let api = if kind == "resend" {
                MailApi::Resend
            } else {
                MailApi::SendGrid
            };
            let file = get(KEY_FILE_ENV).ok_or((
                KEY_FILE_ENV,
                format!("{KEY_FILE_ENV} is required for {kind}"),
            ))?;
            let text = read_file(&file)
                .map_err(|_| (KEY_FILE_ENV, format!("{KEY_FILE_ENV} could not be read")))?;
            let key =
                ApiKey::new(&text).map_err(|e| (KEY_FILE_ENV, format!("{KEY_FILE_ENV}: {e}")))?;
            Arc::new(ApiMailer::new(api, key, from, deny))
        };
        let mut label = format!(
            "{}; up to {} attempts, outbox read every {}s",
            mailer.label(),
            policy.max_attempts,
            policy.every.as_secs()
        );
        if !private_hosts.is_empty() {
            label.push_str(&format!(
                "; private hosts allowed ({PRIVATE_HOSTS_ENV}): {}",
                private_hosts.join(", ")
            ));
        }
        Ok(Some(MailChannel {
            mailer,
            policy,
            label,
        }))
    }
}

// ───────────────────────────── the outbox ─────────────────────────────

const OUTBOX: &str = "$mailoutbox";

const OUTBOX_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$mailoutbox" (
  "id"          bigserial PRIMARY KEY,
  "kind"        text    NOT NULL,
  "recipient"   text    NOT NULL,
  "payload"     text    NOT NULL,
  "attempts"    integer NOT NULL DEFAULT 0,
  "next_at"     bigint  NOT NULL,
  "queued_at"   bigint  NOT NULL
)"#;

const OUTBOX_INDEX: &str =
    r#"CREATE INDEX IF NOT EXISTS "$mailoutbox_next_at" ON "$mailoutbox" ("next_at")"#;

/// Create `$mailoutbox`. Retry-once, as the other `ensure_table`s are.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [OUTBOX_DDL, OUTBOX_INDEX] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// One send, as queued: which composer, to whom, and what the composer
/// needs. **Never the message.** For a sign-in code the payload is the
/// ceremony's handle, and the code is minted by the composer when the row is
/// sent.
#[derive(Clone, PartialEq, Eq)]
pub struct MailJob {
    kind: String,
    to: MailAddress,
    payload: String,
}

impl MailJob {
    /// `kind` is 1 to 40 of `a-z`, `0-9`, `.` and `-`, and names a
    /// registered [`Composer`]. `payload` is at most 4 KiB with no NUL.
    pub fn new(kind: &str, to: MailAddress, payload: &str) -> Result<MailJob, MailError> {
        let kind_ok = !kind.is_empty()
            && kind.len() <= 40
            && kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
        if !kind_ok {
            return Err(MailError::not_representable("the job kind"));
        }
        if payload.len() > 4096 || payload.contains('\0') {
            return Err(MailError::not_representable("the job payload"));
        }
        Ok(MailJob {
            kind: kind.to_string(),
            to,
            payload: payload.to_string(),
        })
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn to(&self) -> &MailAddress {
        &self.to
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    fn insert_sql(&self, now: u64) -> String {
        format!(
            r#"INSERT INTO "$mailoutbox" ("kind", "recipient", "payload", "attempts", "next_at", "queued_at")
               VALUES ({}, {}, {}, 0, {now}, {now})"#,
            sql_lit(&self.kind),
            sql_lit(self.to.as_str()),
            sql_lit(&self.payload),
        )
    }
}

/// Queue one send, from async code. One insert; due at once.
pub async fn enqueue(pool: &Pool, job: &MailJob, now: u64) -> Result<(), PoolError> {
    pool.rows(&job.insert_sql(now)).await.map(|_| ())
}

/// Queue one send, from synchronous code (a provider's `begin`). One insert,
/// so the caller's work is the same whatever the address (E11).
pub trait MailQueue: Send + Sync {
    fn enqueue(&self, job: &MailJob, now: u64) -> Result<(), MailError>;
}

/// The outbox, over the control database.
pub struct PgMailQueue {
    db: SyncPg,
}

impl PgMailQueue {
    /// On a connection the deployment already holds. Refuses when the table
    /// does not exist.
    pub fn open_on(pool: Pool) -> Result<PgMailQueue, MailError> {
        let db = SyncPg::start(Source::Shared(pool), "ordo-mailoutbox")
            .map_err(MailError::unavailable)?;
        let q = PgMailQueue { db };
        q.sql(r#"SELECT 1 FROM "$mailoutbox" LIMIT 1"#)?;
        Ok(q)
    }

    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, MailError> {
        self.db.sql(sql).map_err(MailError::from_pool)
    }
}

impl MailQueue for PgMailQueue {
    fn enqueue(&self, job: &MailJob, now: u64) -> Result<(), MailError> {
        self.sql(&job.insert_sql(now)).map(|_| ())
    }
}

/// Process-local. **Tests only**: what was queued, in order.
#[derive(Default)]
pub struct MemoryMailQueue {
    jobs: Mutex<Vec<(MailJob, u64)>>,
}

impl MemoryMailQueue {
    /// Everything queued so far, and empty the queue.
    pub fn take(&self) -> Vec<(MailJob, u64)> {
        std::mem::take(&mut *self.jobs.lock().expect("mail queue"))
    }
}

impl MailQueue for MemoryMailQueue {
    fn enqueue(&self, job: &MailJob, now: u64) -> Result<(), MailError> {
        self.jobs
            .lock()
            .expect("mail queue")
            .push((job.clone(), now));
        Ok(())
    }
}

// ───────────────────────────── composers ──────────────────────────────

/// A queued row, as its composer reads it.
pub struct QueuedMail {
    pub id: i64,
    pub kind: String,
    pub to: MailAddress,
    pub payload: String,
    /// This attempt's number, from 1.
    pub attempt: u32,
    pub queued_at: u64,
}

/// Turn a queued row into a message, at send time.
///
/// `Ok(None)` is "nothing is owed": the row is removed and nothing is sent
/// (§7.5: an address nobody admits, an account that was deleted, a ceremony
/// that expired). An error is retried when [`MailError::retryable`].
/// Called on a blocking thread, so a composer may use the synchronous stores.
pub trait Composer: Send + Sync {
    fn compose(&self, job: &QueuedMail, now: u64) -> Result<Option<Message>, MailError>;
}

/// The composers this deployment knows, by kind.
#[derive(Clone, Default)]
pub struct Composers {
    by_kind: BTreeMap<String, Arc<dyn Composer>>,
}

impl Composers {
    pub fn new() -> Composers {
        Composers::default()
    }

    /// Add `composer` for `kind`. A second registration for one kind is
    /// refused: two composers for one row would be a coin flip.
    pub fn register(&mut self, kind: &str, composer: Arc<dyn Composer>) -> Result<(), String> {
        if self.by_kind.contains_key(kind) {
            return Err(format!("a composer for `{kind}` is already registered"));
        }
        self.by_kind.insert(kind.to_string(), composer);
        Ok(())
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn Composer>> {
        self.by_kind.get(kind).cloned()
    }

    pub fn kinds(&self) -> Vec<String> {
        self.by_kind.keys().cloned().collect()
    }
}

// ───────────────────────────── the worker ─────────────────────────────

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    pub sent: usize,
    /// The composer said nothing was owed.
    pub skipped: usize,
    /// Failed in a way that may pass; tried again later.
    pub retrying: usize,
    /// Refused, not representable, or out of attempts. Removed.
    pub dropped: usize,
    /// A full batch was claimed, so there may be more due now.
    pub more: bool,
}

enum Outcome {
    Sent,
    Skipped,
    Failed(MailError),
}

/// Claim what is due, compose, submit, and settle each row, up to
/// `policy.batch` rows.
///
/// Each row is claimed **just before its own send**, one at a time: a lease
/// (`next_at = now + lease_secs`) under `FOR UPDATE SKIP LOCKED`, so two
/// replicas draining one outbox never take the same row. Leasing the whole
/// batch up front would let a lease expire while the batch's earlier rows
/// were still being sent, and another replica would claim the tail and send
/// it twice. `now` is the pass's start; each claim adds the time the pass
/// has run, so a late row's lease starts when its send does. The lease end
/// is rounded up to cover the whole-second clock, so it never ends sooner
/// than `lease_secs` after the send began. A row is deleted once sent,
/// skipped or dropped, and given a later `next_at` when it is to be tried
/// again. Each settle is fenced by the claim's `attempts`: a replica whose
/// lease ran out and whose row was claimed again cannot delete or
/// reschedule the newer claim's row.
pub async fn run_pass(
    pool: &Pool,
    mailer: &Arc<dyn Mailer>,
    composers: &Arc<Composers>,
    policy: &SendPolicy,
    metrics: &Metrics,
    now: u64,
) -> Result<Pass, PoolError> {
    let started = std::time::Instant::now();
    let pass_start = now;
    let mut pass = Pass::default();
    for claimed in 1..=policy.batch {
        // The clock is whole seconds, floored twice: once by the caller's
        // `now`, once here. A row is due by the floored time, but its lease
        // ends by the time rounded up, plus one second for the caller's
        // floor, so a lease never ends sooner in real time than
        // `lease_secs` after its send began.
        let elapsed = started.elapsed();
        let now = pass_start + elapsed.as_secs();
        let lease = pass_start
            + elapsed.as_secs()
            + u64::from(elapsed.subsec_nanos() > 0)
            + 1
            + policy.lease_secs;
        let rows = pool.rows(&claim_sql(now, lease)).await?;
        let Some(row) = rows.into_iter().next() else {
            break;
        };
        pass.more = claimed == policy.batch;
        let field = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
        let Ok(id) = field(0).parse::<i64>() else {
            continue;
        };
        let kind = field(1);
        let attempt = field(4).parse::<u32>().unwrap_or(u32::MAX);
        let outcome = match MailAddress::parse(&field(2)) {
            Err(e) => Outcome::Failed(e),
            Ok(to) => {
                let job = QueuedMail {
                    id,
                    kind: kind.clone(),
                    to,
                    payload: field(3),
                    attempt,
                    queued_at: field(5).parse().unwrap_or(0),
                };
                let (mailer, composers) = (mailer.clone(), composers.clone());
                let message_id = message_id_for(id, job.queued_at, &mailer.mail_domain());
                tokio::task::spawn_blocking(move || {
                    let Some(composer) = composers.get(&job.kind) else {
                        return Outcome::Failed(MailError::not_representable(format!(
                            "no composer is registered for `{}`",
                            job.kind
                        )));
                    };
                    match composer.compose(&job, now) {
                        Ok(None) => Outcome::Skipped,
                        Ok(Some(msg)) => match mailer.submit(&msg.with_message_id(message_id)) {
                            Ok(()) => Outcome::Sent,
                            Err(e) => Outcome::Failed(e),
                        },
                        Err(e) => Outcome::Failed(e),
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Outcome::Failed(MailError::unavailable("the send thread panicked"))
                })
            }
        };
        let labels = [("kind", kind.as_str())];
        match outcome {
            Outcome::Sent | Outcome::Skipped => {
                pool.rows(&format!(
                    r#"DELETE FROM "$mailoutbox" WHERE "id" = {id} AND "attempts" = {attempt}"#
                ))
                .await?;
                if matches!(outcome, Outcome::Sent) {
                    pass.sent += 1;
                    metrics.incr("ordo_mail_sent_total", &labels);
                } else {
                    pass.skipped += 1;
                    metrics.incr("ordo_mail_skipped_total", &labels);
                }
            }
            Outcome::Failed(e) if policy.retries(attempt, &e) => {
                pool.rows(&format!(
                    r#"UPDATE "$mailoutbox" SET "next_at" = {} WHERE "id" = {id} AND "attempts" = {attempt}"#,
                    now + policy.wait_after(attempt, &e)
                ))
                .await?;
                pass.retrying += 1;
                metrics.incr("ordo_mail_retried_total", &labels);
                eprintln!("{WORKER_NAME}: message {id} ({kind}) attempt {attempt} failed, will retry: {e}");
            }
            Outcome::Failed(e) => {
                pool.rows(&format!(
                    r#"DELETE FROM "$mailoutbox" WHERE "id" = {id} AND "attempts" = {attempt}"#
                ))
                .await?;
                pass.dropped += 1;
                metrics.incr("ordo_mail_dropped_total", &labels);
                eprintln!(
                    "{WORKER_NAME}: message {id} ({kind}) dropped after {attempt} attempt(s): {e}"
                );
            }
        }
    }
    Ok(pass)
}

/// The statement that claims the next due row: lease it until `lease` and
/// count the attempt, answering the row.
///
/// **Exactly one row, whatever plan Postgres picks** (reported fix
/// `outbox-claimed-not-sent`). The row is chosen in a `MATERIALIZED` CTE,
/// which runs once. It used to be chosen by `WHERE "id" IN (SELECT ...
/// LIMIT 1 FOR UPDATE SKIP LOCKED)`, and once the table's statistics made
/// Postgres plan that as a nested loop with the subquery inside, the
/// subquery ran again for each outer row. Each run skips the rows this same
/// statement has just updated, so it answered the next due row, and one
/// claim leased several rows. The pass sends only the first row a claim
/// answers, so the others sat leased and unsent for `lease_secs` (120 s):
/// the begin had answered 200, and the code arrived 2 minutes late.
pub fn claim_sql(now: u64, lease: u64) -> String {
    format!(
        r#"WITH "next" AS MATERIALIZED (
             SELECT "id" FROM "$mailoutbox" WHERE "next_at" <= {now}
             ORDER BY "next_at", "id" LIMIT 1 FOR UPDATE SKIP LOCKED)
           UPDATE "$mailoutbox" AS m SET "attempts" = m."attempts" + 1, "next_at" = {lease}
             FROM "next" WHERE m."id" = "next"."id"
           RETURNING m."id", m."kind", m."recipient", m."payload", m."attempts", m."queued_at""#
    )
}

/// The `mail.send` worker: [`run_pass`] on the supervisor's tick.
pub struct MailSend {
    pub mailer: Arc<dyn Mailer>,
    pub composers: Arc<Composers>,
    pub policy: SendPolicy,
}

impl Worker for MailSend {
    fn name(&self) -> &'static str {
        WORKER_NAME
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.policy.every)
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let pass = run_pass(
                &ctx.pool,
                &self.mailer,
                &self.composers,
                &self.policy,
                &ctx.metrics,
                ctx.now(),
            )
            .await
            .map_err(|e| WorkerError(scrub(OUTBOX, e)))?;
            Ok(Progress {
                units: (pass.sent + pass.skipped + pass.retrying + pass.dropped) as u64,
                more: pass.more,
            })
        })
    }
}

/// The composers a booted deployment registers: the emailed code's, when
/// `email` is a sign-in method (`plan-auth-federated.md` step 5) or a
/// signed-in person can confirm an address ([`crate::contact_email`]). The
/// same provider writes both kinds of code.
pub fn boot_composers(email: Option<Arc<crate::email_code::EmailCodeProvider>>) -> Composers {
    let mut c = Composers::new();
    // A routine's `notify` (`crate::routines`), whatever the ways in.
    c.register(
        crate::routines::NOTICE_KIND,
        Arc::new(crate::routines::RoutineNotice),
    )
    .expect("one composer per kind, and this is the first");
    // The note to an address that is no longer the account's.
    c.register(
        crate::contact_email::CHANGED_KIND,
        Arc::new(crate::contact_email::ChangedNotice),
    )
    .expect("one composer per kind, and this is the first");
    if let Some(p) = email {
        c.register(crate::email_code::MAIL_KIND, p)
            .expect("one composer per kind, and this is the first");
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_mail_error_classifies_by_its_type_and_keeps_its_cause() {
        use std::error::Error;
        let r = |r| MailError::from_refusal_with(r, None);
        let rej = |status, wait| Refusal::Rejected {
            status,
            retry_after_secs: wait,
        };
        let e = r(rej(429, Some(30)));
        assert_eq!(e.class(), FaultClass::Retryable);
        assert!(e.is_deferred() && e.retryable());
        assert_eq!(
            (e.retry_after_secs(), e.relay_code()),
            (Some(30), Some(429))
        );
        assert!(e.source().is_some());
        assert_eq!(r(rej(503, None)).class(), FaultClass::Retryable);
        let e = r(Refusal::RelayRefused { code: 451 });
        assert!(e.is_deferred());
        assert_eq!(e.relay_code(), Some(451));
        let e = r(Refusal::RelayRefused { code: 550 });
        assert!(e.is_permanently_refused() && !e.retryable());
        assert_eq!(e.class(), FaultClass::Refused);
        assert_eq!(e.relay_code(), Some(550));
        let e = r(Refusal::RelayRefused { code: 535 });
        assert!(e.is_misconfigured());
        assert_eq!(e.class(), FaultClass::Broken);
        assert!(r(rej(401, None)).is_misconfigured());
        let e = r(Refusal::NotRepresentable { detail: "x".into() });
        assert!(e.is_bad_input() && e.source().is_none());
        assert_eq!(e.class(), FaultClass::Refused);
        let busy = MailError::from_pool(PoolError::Saturated);
        assert!(busy.is_busy() && busy.source().is_some());
        assert_eq!(busy.class(), FaultClass::Busy);
    }

    #[test]
    fn the_outbox_retries_from_the_class_alone() {
        let policy = SendPolicy::default();
        let max = policy.max_attempts;
        assert!(policy.retries(1, &MailError::unavailable("x")));
        assert!(policy.retries(1, &MailError::deferred("x", 5)));
        assert!(policy.retries(1, &MailError::from_pool(PoolError::Saturated)));
        assert!(!policy.retries(max, &MailError::unavailable("x")));
        assert!(!policy.retries(1, &MailError::refused("x")));
        assert!(!policy.retries(1, &MailError::not_representable("x")));
    }

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn files(path: &str) -> Result<String, String> {
        match path {
            "/run/secrets/relay" => Ok("mailer\nhunter2-zq7\n".into()),
            "/run/secrets/key" => Ok("re_live_key-zq7\n".into()),
            "/run/secrets/bad" => Ok("only-one-line-zq7".into()),
            _ => Err("no such file".into()),
        }
    }

    fn parse(pairs: &[(&str, &str)]) -> Result<Option<MailChannel>, (&'static str, String)> {
        MailChannel::from_vars(&vars(pairs), &files)
    }

    #[test]
    fn unset_is_off() {
        assert!(parse(&[]).unwrap().is_none());
        assert!(parse(&[(KIND_ENV, "  ")]).unwrap().is_none());
    }

    #[test]
    fn a_bad_private_host_refuses_the_boot_even_with_mail_off() {
        for bad in ["*.internal", "relay:25", "10.1.0.0/8"] {
            let (var, detail) = parse(&[(PRIVATE_HOSTS_ENV, bad)]).err().unwrap();
            assert_eq!(var, PRIVATE_HOSTS_ENV, "{bad}");
            // The boot line prefixes the name; the detail does not repeat it.
            assert!(!detail.contains(PRIVATE_HOSTS_ENV), "{detail}");
        }
        assert!(
            parse(&[(PRIVATE_HOSTS_ENV, "mailpit.local, 10.0.0.5, 10.0.0.0/8")])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn each_kind_is_a_whole_channel_with_its_defaults() {
        let smtp = parse(&[
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (HOST_ENV, "smtp.example.com"),
            (CREDENTIALS_FILE_ENV, "/run/secrets/relay"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(smtp.policy, SendPolicy::default());
        assert!(
            smtp.label.contains("smtp.example.com:465"),
            "{}",
            smtp.label
        );
        for kind in ["resend", "sendgrid"] {
            let ch = parse(&[
                (KIND_ENV, kind),
                (FROM_ENV, "signin@example.com"),
                (KEY_FILE_ENV, "/run/secrets/key"),
                (MAX_ATTEMPTS_ENV, "3"),
            ])
            .unwrap()
            .unwrap();
            assert!(ch.label.starts_with(kind), "{}", ch.label);
            assert_eq!(ch.policy.max_attempts, 3);
        }
    }

    #[test]
    fn smtp_without_a_credentials_file_boots_with_no_login() {
        let ch = parse(&[
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (HOST_ENV, "localhost"),
        ])
        .unwrap()
        .unwrap();
        assert!(ch.label.contains("no login"), "{}", ch.label);
        let with = parse(&[
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (HOST_ENV, "localhost"),
            (CREDENTIALS_FILE_ENV, "/run/secrets/relay"),
        ])
        .unwrap()
        .unwrap();
        assert!(!with.label.contains("no login"), "{}", with.label);
        // A password in a value is still refused, with or without a file.
        let (k, _) = parse(&[
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (HOST_ENV, "localhost"),
            ("ORDO_MAIL_PASSWORD", "x"),
        ])
        .err()
        .expect("refused");
        assert_eq!(k, "ORDO_MAIL_PASSWORD");
    }

    #[test]
    fn no_refusal_or_label_carries_a_secret() {
        let cases: Vec<Vec<(&str, &str)>> = vec![
            vec![
                (KIND_ENV, "smtp"),
                (FROM_ENV, "signin@example.com"),
                (HOST_ENV, "smtp.example.com"),
                (CREDENTIALS_FILE_ENV, "/run/secrets/bad"),
            ],
            vec![
                (KIND_ENV, "resend"),
                (FROM_ENV, "signin@example.com"),
                (KEY_FILE_ENV, "/nope"),
            ],
            vec![
                (KIND_ENV, "resend"),
                (FROM_ENV, "signin@example.com"),
                ("ORDO_MAIL_KEY", "re_inline-zq7"),
            ],
            vec![
                (KIND_ENV, "smtp"),
                (FROM_ENV, "signin@example.com"),
                (HOST_ENV, "smtp.example.com"),
                ("ORDO_MAIL_PASSWORD", "inline-zq7"),
            ],
        ];
        for pairs in cases {
            let (_, detail) = parse(&pairs).err().expect("refused");
            assert!(!detail.contains("zq7"), "{detail}");
        }
        for (kind, file) in [("smtp", CREDENTIALS_FILE_ENV), ("sendgrid", KEY_FILE_ENV)] {
            let path = if kind == "smtp" {
                "/run/secrets/relay"
            } else {
                "/run/secrets/key"
            };
            let ch = parse(&[
                (KIND_ENV, kind),
                (FROM_ENV, "signin@example.com"),
                (HOST_ENV, "smtp.example.com"),
                (file, path),
            ]);
            if let Ok(Some(ch)) = ch {
                assert!(!ch.label.contains("zq7"), "{}", ch.label);
            }
        }
    }

    #[test]
    fn any_port_takes_either_tls_mode_and_no_plaintext_mode_exists() {
        let base = [
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (HOST_ENV, "smtp.example.com"),
            (CREDENTIALS_FILE_ENV, "/run/secrets/relay"),
        ];
        for (port, tls, label) in [
            (
                "25",
                "starttls",
                "smtp.example.com:25 over required STARTTLS",
            ),
            (
                "587",
                "STARTTLS",
                "smtp.example.com:587 over required STARTTLS",
            ),
            (
                "2465",
                "implicit",
                "smtp.example.com:2465 over implicit TLS",
            ),
            ("587", "", "smtp.example.com:587 over implicit TLS"),
            ("1025", "none", "smtp.example.com:1025 over NO TLS"),
        ] {
            let mut p = base.to_vec();
            p.push((PORT_ENV, port));
            p.push((TLS_ENV, tls));
            let ch = parse(&p).unwrap().unwrap();
            assert!(ch.label.contains(label), "{}", ch.label);
        }
        for bad in ["plain", "opportunistic", "tls", "off"] {
            let mut p = base.to_vec();
            p.push((TLS_ENV, bad));
            let (var, detail) = parse(&p).err().unwrap();
            assert_eq!(var, TLS_ENV, "{bad}");
            assert!(detail.contains("none (no encryption)"), "{detail}");
        }
        for bad in ["*.internal", "localhost:1025", "10.1.0.0/8"] {
            let mut p = base.to_vec();
            p.push((PRIVATE_HOSTS_ENV, bad));
            let (var, detail) = parse(&p).err().unwrap();
            assert_eq!(var, PRIVATE_HOSTS_ENV, "{bad}");
            // The boot line prefixes the name; the detail does not repeat it.
            assert!(!detail.contains(PRIVATE_HOSTS_ENV), "{detail}");
        }
        for bad in ["0", "65536", "x"] {
            let mut p = base.to_vec();
            p.push((PORT_ENV, bad));
            assert_eq!(parse(&p).err().unwrap().0, PORT_ENV, "{bad}");
        }
        assert_eq!(
            parse(&[
                (KIND_ENV, "resend"),
                (FROM_ENV, "signin@example.com"),
                (KEY_FILE_ENV, "/run/secrets/key"),
                (TLS_ENV, "starttls"),
            ])
            .err()
            .unwrap()
            .0,
            TLS_ENV
        );
    }

    #[test]
    fn schemes_and_later_kinds_are_refused_by_name() {
        let base = [
            (KIND_ENV, "smtp"),
            (FROM_ENV, "signin@example.com"),
            (CREDENTIALS_FILE_ENV, "/run/secrets/relay"),
        ];
        let mut p = base.to_vec();
        p.push((HOST_ENV, "smtp://smtp.example.com"));
        assert_eq!(parse(&p).err().unwrap().0, HOST_ENV);
        let (var, detail) = parse(&[(KIND_ENV, "mailgun")]).err().unwrap();
        assert_eq!(var, KIND_ENV);
        assert!(detail.contains("18z"), "{detail}");
        assert_eq!(
            parse(&[(KIND_ENV, "carrier-pigeon")]).err().unwrap().0,
            KIND_ENV
        );
        let mut p = base.to_vec();
        p.push((HOST_ENV, "smtp.example.com"));
        p.push((KEY_FILE_ENV, "/run/secrets/key"));
        assert_eq!(parse(&p).err().unwrap().0, KEY_FILE_ENV);
    }

    #[test]
    fn a_sender_with_a_display_name_or_a_line_break_is_refused() {
        for from in [
            "Ordo <signin@example.com>",
            "signin@example.com\r\nBcc: x@y.z",
        ] {
            let (var, _) = parse(&[
                (KIND_ENV, "resend"),
                (FROM_ENV, from),
                (KEY_FILE_ENV, "/run/secrets/key"),
            ])
            .err()
            .unwrap();
            assert_eq!(var, FROM_ENV);
        }
    }

    #[test]
    fn numbers_outside_their_range_are_refused_not_clamped() {
        for (k, v) in [
            (MAX_ATTEMPTS_ENV, "0"),
            (MAX_ATTEMPTS_ENV, "21"),
            (SEND_SECS_ENV, "0"),
            (SEND_SECS_ENV, "x"),
            (RETRY_BASE_SECS_ENV, "0"),
            (RETRY_BASE_SECS_ENV, "3601"),
            (RETRY_MAX_SECS_ENV, "0"),
            (RETRY_MAX_SECS_ENV, "86401"),
            (RETRY_MAX_SECS_ENV, "10"),
            (BATCH_ENV, "0"),
            (BATCH_ENV, "501"),
            (LEASE_SECS_ENV, "29"),
            (LEASE_SECS_ENV, "3601"),
        ] {
            let (var, _) = parse(&[
                (KIND_ENV, "resend"),
                (FROM_ENV, "signin@example.com"),
                (KEY_FILE_ENV, "/run/secrets/key"),
                (k, v),
            ])
            .err()
            .unwrap();
            assert_eq!(var, k);
        }
    }

    #[test]
    fn backoff_doubles_from_fifteen_seconds_to_fifteen_minutes() {
        let p = SendPolicy::default();
        let waits: Vec<u64> = (1..=8).map(|a| p.backoff(a)).collect();
        assert_eq!(waits, vec![15, 30, 60, 120, 240, 480, 900, 900]);
    }

    #[test]
    fn the_outbox_numbers_are_settings_with_todays_defaults() {
        let base = [
            (KIND_ENV, "resend"),
            (FROM_ENV, "signin@example.com"),
            (KEY_FILE_ENV, "/run/secrets/key"),
        ];
        let d = parse(&base).unwrap().unwrap().policy;
        assert_eq!(
            (d.batch, d.lease_secs, d.retry_base_secs, d.retry_max_secs),
            (16, 120, 15, 900)
        );
        let mut p = base.to_vec();
        p.extend([
            (RETRY_BASE_SECS_ENV, "5"),
            (RETRY_MAX_SECS_ENV, "30"),
            (BATCH_ENV, "100"),
            (LEASE_SECS_ENV, "600"),
        ]);
        let set = parse(&p).unwrap().unwrap().policy;
        assert_eq!((set.batch, set.lease_secs), (100, 600));
        let waits: Vec<u64> = (1..=5).map(|a| set.backoff(a)).collect();
        assert_eq!(waits, vec![5, 10, 20, 30, 30]);
        // The base may equal the max: a flat wait.
        let mut p = base.to_vec();
        p.extend([(RETRY_BASE_SECS_ENV, "60"), (RETRY_MAX_SECS_ENV, "60")]);
        let flat = parse(&p).unwrap().unwrap().policy;
        assert_eq!(flat.backoff(1), 60);
        assert_eq!(flat.backoff(20), 60);
    }

    #[test]
    fn the_api_bodies_carry_the_address_once_and_the_key_never() {
        let to = MailAddress::parse("kim@example.org").unwrap();
        let msg = Message::new(to, "Your code", "12345678").unwrap();
        for api in [MailApi::Resend, MailApi::SendGrid] {
            let body = api.body("signin@example.com", &msg);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body.matches("kim@example.org").count(), 1, "{body}");
            assert!(v.to_string().contains("12345678"));
        }
        assert_eq!(MailApi::Resend.accepted(), 200);
        assert_eq!(MailApi::SendGrid.accepted(), 202);
    }

    #[test]
    fn a_subject_or_text_over_its_bound_names_the_setting_and_value() {
        let s = over_cap("subject", 250, 200, "ORDO_MAIL_SUBJECT_MAX_BYTES", 998, "x");
        assert!(s.contains("ORDO_MAIL_SUBJECT_MAX_BYTES=250 or more"), "{s}");
        let s = over_cap(
            "subject",
            1200,
            200,
            "ORDO_MAIL_SUBJECT_MAX_BYTES",
            998,
            "x",
        );
        assert!(s.contains("at most 998") && s.contains("shorten it"), "{s}");
        let s = over_cap(
            "text",
            10,
            16384,
            "ORDO_MAIL_TEXT_MAX_BYTES",
            1 << 20,
            "is bad",
        );
        assert_eq!(s, "the text is bad");
        let to = MailAddress::parse("kim@example.org").unwrap();
        let err = Message::new(to, &"s".repeat(1000), "t").err().unwrap();
        assert!(
            format!("{err:?}").contains("ORDO_MAIL_SUBJECT_MAX_BYTES"),
            "{err:?}"
        );
    }

    #[test]
    fn a_job_kind_and_payload_are_bounded() {
        let to = MailAddress::parse("kim@example.org").unwrap();
        assert!(MailJob::new("signin.code", to.clone(), "handle").is_ok());
        assert!(MailJob::new("Signin", to.clone(), "").is_err());
        assert!(MailJob::new("", to.clone(), "").is_err());
        assert!(MailJob::new("x", to.clone(), "a\0b").is_err());
        assert!(MailJob::new("x", to, &"a".repeat(4097)).is_err());
        assert_eq!(
            format!("{:?}", MailAddress::parse("kim@example.org").unwrap()),
            "MailAddress(..)"
        );
    }

    #[test]
    fn one_composer_per_kind() {
        struct Nothing;
        impl Composer for Nothing {
            fn compose(&self, _: &QueuedMail, _: u64) -> Result<Option<Message>, MailError> {
                Ok(None)
            }
        }
        let mut c = Composers::new();
        c.register("a", Arc::new(Nothing)).unwrap();
        assert!(c.register("a", Arc::new(Nothing)).is_err());
        assert_eq!(c.kinds(), vec!["a".to_string()]);
    }

    /// Audit 32 (2026-10-09): a refusal a setting can fix names that setting
    /// and the value to use.
    #[test]
    fn mail_refusals_a_setting_can_fix_name_the_setting_and_the_value() {
        let fix = |r: Refusal, tls| smtp_fix(&r, tls, true).expect("a fix");
        let t = fix(Refusal::TimedOut { after_ms: 10_000 }, SmtpTls::StartTls);
        assert!(t.contains(PORT_ENV) && t.contains("starttls"), "{t}");
        let m = fix(
            Refusal::MalformedResponse {
                detail: "not smtp".into(),
            },
            SmtpTls::Implicit,
        );
        assert!(m.contains("ORDO_MAIL_TLS=starttls"), "{m}");
        for code in [550, 553, 554] {
            let f = fix(Refusal::RelayRefused { code }, SmtpTls::StartTls);
            assert!(f.contains(FROM_ENV), "{code}: {f}");
        }
        for status in [400, 422] {
            let f = api_status_fix(status, "Resend");
            assert!(f.contains("ORDO_MAIL_FROM"), "{status}: {f}");
        }
        assert!(api_status_fix(500, "Resend").is_empty());
    }
}
