//! Mail submission over TLS (`plan-auth-federated.md` §7.6, §9.3).
//!
//! One connection, one message, one recipient, a closed set of replies.
//! Written out over the same dialler as [`crate::deliver`] for the reason
//! `client.rs` gives for HTTP: what is needed is small, and a general client
//! is built to be accommodating in exactly the ways this crate is not.
//!
//! # What is pinned
//!
//! - **TLS before any credential, unless the app developer says otherwise.**
//!   [`SmtpTls::Implicit`], the default, starts TLS at the first byte
//!   (RFC 8314). [`SmtpTls::StartTls`] reads the greeting, sends `EHLO`, and
//!   sends `STARTTLS` (RFC 3207). Those two commands are the only bytes
//!   written in plaintext. Either way the handshake is the shared
//!   dialler's: certificates are validated against the relay's name and the
//!   floor is TLS 1.2. [`SmtpTls::None`] is plaintext throughout, for a
//!   local mail catcher. It is used only when named, never as a fallback.
//! - **STARTTLS is required, never opportunistic.** A relay whose `EHLO`
//!   does not offer it is refused. So is one that answers `STARTTLS` with
//!   anything but `220`, and one whose handshake fails. Nothing falls back
//!   to plaintext, and no credential is written on any of those paths.
//! - **No plaintext byte survives the upgrade.** A relay that has sent
//!   bytes the client has not asked for, before or right after its `220` to
//!   `STARTTLS`, is refused (the response injection class of CVE-2011-0411).
//!   What the relay said before TLS is discarded, and `EHLO` is sent again
//!   inside TLS.
//! - **The destination pins.** The relay is pinned by
//!   [`crate::resolve_and_pin_host`]: allowlist first, every resolved address
//!   through the deny list, the first one dialled and consumed.
//! - **Nothing is sent that cannot be written exactly.** Both addresses must
//!   match a strict grammar with no control characters, the subject is
//!   printable ASCII, the text is printable ASCII lines (E13). All of it is
//!   checked by [`render`] before a socket opens.
//! - **The relay's words are not kept.** A reply is read for its code and
//!   discarded. Reply text can echo the recipient's address, so a refusal
//!   carries the code alone ([`Refusal::RelayRefused`]).
//! - **The credential is held by the caller.** [`SmtpLogin`] has no `Debug`,
//!   is passed per call, and is written once, base64-encoded, after the
//!   handshake (pin 5), or in plaintext when the app developer chose `None`.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::client::{connect, dial, handshake, Accepted, TlsRoots};
use crate::dest::PinnedDestination;
use crate::refusal::Refusal;

/// Submission over implicit TLS (RFC 8314 §3.3).
pub const SMTPS_PORT: u16 = 465;

/// How the connection to the relay becomes TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// TLS from the first byte (RFC 8314 §3.3). Port 465, usually.
    Implicit,
    /// `EHLO`, then `STARTTLS`, then TLS (RFC 3207). Port 587, usually.
    /// Required: a relay that does not offer it is refused.
    StartTls,
    /// No TLS at all. The credential and the message cross the network
    /// readable. Off unless the app developer names it, for a local mail
    /// catcher such as Mailpit.
    None,
}

impl SmtpTls {
    /// The setting's word: `implicit`, `starttls` or `none`.
    pub fn word(self) -> &'static str {
        match self {
            SmtpTls::Implicit => "implicit",
            SmtpTls::StartTls => "starttls",
            SmtpTls::None => "none",
        }
    }

    /// The inverse of [`SmtpTls::word`], ignoring case.
    pub fn parse(s: &str) -> Option<SmtpTls> {
        match s.to_ascii_lowercase().as_str() {
            "implicit" => Some(SmtpTls::Implicit),
            "starttls" => Some(SmtpTls::StartTls),
            "none" => Some(SmtpTls::None),
            _ => None,
        }
    }
}

/// The longest reply line read. RFC 5321 §4.5.3.1.5 sets 512.
const REPLY_LINE_CAP: usize = 1024;
/// The most lines one multi-line reply may have. An `EHLO` answer is a
/// dozen; a relay sending thousands is a relay making this process spin.
const REPLY_LINES_CAP: usize = 64;
/// The longest subject accepted by default (`ORDO_MAIL_SUBJECT_MAX_BYTES`).
pub const SUBJECT_CAP: usize = 200;
/// The longest text accepted by default (`ORDO_MAIL_TEXT_MAX_BYTES`). One
/// template, one substitution.
pub const TEXT_CAP: usize = 16 * 1024;

static SUBJECT_MAX: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(SUBJECT_CAP);
static TEXT_MAX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(TEXT_CAP);

/// The subject cap now in force, in bytes.
pub fn subject_cap() -> usize {
    SUBJECT_MAX.load(std::sync::atomic::Ordering::Relaxed)
}

/// The text cap now in force, in bytes.
pub fn text_cap() -> usize {
    TEXT_MAX.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set the subject and text caps from the deployment's settings. Called once
/// at boot, before any mail is composed.
pub fn set_caps(subject: usize, text: usize) {
    SUBJECT_MAX.store(subject, std::sync::atomic::Ordering::Relaxed);
    TEXT_MAX.store(text, std::sync::atomic::Ordering::Relaxed);
}
/// The longest line in the text. RFC 5322 §2.1.1 sets 998.
const TEXT_LINE_CAP: usize = 998;

/// An account at the relay. **Not `Debug`, not `Clone`.** Read from the
/// deployment's credentials file and passed to [`submit`] per call.
pub struct SmtpLogin {
    user: String,
    password: String,
}

impl SmtpLogin {
    /// A user and a password. Refused when either is empty or holds a
    /// control character, since `AUTH PLAIN` separates them with NUL. The
    /// refusal names neither value.
    pub fn new(user: &str, password: &str) -> Result<SmtpLogin, String> {
        let ok = |s: &str| !s.is_empty() && !s.chars().any(char::is_control);
        if !ok(user) {
            return Err("the relay user name is empty or holds a control character".into());
        }
        if !ok(password) {
            return Err("the relay password is empty or holds a control character".into());
        }
        Ok(SmtpLogin {
            user: user.to_string(),
            password: password.to_string(),
        })
    }

    /// The `AUTH PLAIN` initial response: base64 of `\0user\0password`.
    fn plain(&self) -> String {
        let mut raw = Vec::with_capacity(self.user.len() + self.password.len() + 2);
        raw.push(0);
        raw.extend_from_slice(self.user.as_bytes());
        raw.push(0);
        raw.extend_from_slice(self.password.as_bytes());
        base64(&raw)
    }
}

/// One message. The addresses are bare (`name@example.com`), never with a
/// display name: a display name is the second place an address could hide.
pub struct Mail<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    /// Lines separated by `\n`. Printable ASCII.
    pub text: &'a str,
    /// Seconds since the epoch, for the `Date:` header.
    pub date: u64,
    /// The `Message-ID:` header value, with its angle brackets, when the
    /// caller fixed one. The same value on every try of one email lets a
    /// mail client fold a duplicate.
    pub message_id: Option<&'a str>,
}

/// Does `s` match the strict address grammar?
///
/// `local@domain`. The local part is a dot-atom of letters, digits and
/// ``!#$%&'*+/=?^_`{|}~-``, at most 64 bytes, with no leading, trailing or
/// doubled dot. The domain is two or more labels of letters, digits and
/// `-`, none starting or ending with `-`, at most 253 bytes; an
/// internationalised domain is accepted in its punycode form. The whole is
/// at most 254 bytes. No quoted local part, no address literal, no
/// comment, no whitespace and no control character: anything outside this is
/// refused, never interpreted.
pub fn address_ok(s: &str) -> bool {
    if s.len() > 254 || !s.is_ascii() {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let atom = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~-".contains(&b);
    let local_ok = !local.is_empty()
        && local.len() <= 64
        && local
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(atom));
    let labels: Vec<&str> = domain.split('.').collect();
    let domain_ok = domain.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    local_ok && domain_ok
}

/// Is `s` a subject this module will write? Printable ASCII, not empty, at
/// most the subject cap ([`SUBJECT_CAP`] unless set).
pub fn subject_ok(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= SUBJECT_MAX.load(std::sync::atomic::Ordering::Relaxed)
        && s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// Is `s` a text this module will write? Printable ASCII and tab in lines
/// split by `\n` (a `\r` directly before a `\n` is accepted), at most
/// [`TEXT_CAP`], each line at most 998 bytes.
pub fn text_ok(s: &str) -> bool {
    s.len() <= TEXT_MAX.load(std::sync::atomic::Ordering::Relaxed)
        && s.split('\n').all(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.len() <= TEXT_LINE_CAP
                && line
                    .bytes()
                    .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
        })
}

/// The message as it goes after `DATA`: headers, a blank line, the
/// dot-stuffed text in CRLF lines, and the terminating `.` line.
///
/// Every check on the message is here, so [`submit`] calls it before it
/// dials: a message that would be refused never costs a connection.
pub fn render(m: &Mail<'_>) -> Result<Vec<u8>, Refusal> {
    let refuse = |d: &str| Refusal::NotRepresentable { detail: d.into() };
    if !address_ok(m.from) {
        return Err(refuse("the sender address"));
    }
    if !address_ok(m.to) {
        return Err(refuse("the recipient address"));
    }
    if !subject_ok(m.subject) {
        return Err(refuse("the subject"));
    }
    if !text_ok(m.text) {
        return Err(refuse("the text"));
    }
    if let Some(id) = m.message_id {
        if id.len() > 200 || !id.bytes().all(|b| (0x21..0x7f).contains(&b)) {
            return Err(refuse("the message id"));
        }
    }
    let mut out = String::with_capacity(m.text.len() + 512);
    // The address travels in exactly one header each (E13).
    out.push_str(&format!("Date: {}\r\n", rfc5322_date(m.date)));
    out.push_str(&format!("From: {}\r\n", m.from));
    out.push_str(&format!("To: {}\r\n", m.to));
    out.push_str(&format!("Subject: {}\r\n", m.subject));
    if let Some(id) = m.message_id {
        out.push_str(&format!("Message-ID: {id}\r\n"));
    }
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str("Content-Type: text/plain; charset=us-ascii\r\n");
    out.push_str("Content-Transfer-Encoding: 7bit\r\n");
    out.push_str("\r\n");
    let text = m.text.strip_suffix('\n').unwrap_or(m.text);
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        // RFC 5321 §4.5.2: a line that starts with a dot gets a second one.
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str(".\r\n");
    Ok(out.into_bytes())
}

/// Submit one message to a pinned relay, over implicit TLS, once.
/// [`submit_over`] with [`SmtpTls::Implicit`].
pub async fn submit(
    dest: PinnedDestination,
    roots: &TlsRoots,
    login: Option<&SmtpLogin>,
    mail: Mail<'_>,
    timeout: Duration,
) -> Result<Accepted, Refusal> {
    submit_over(SmtpTls::Implicit, dest, roots, login, mail, timeout).await
}

/// Submit one message to a pinned relay, once, with TLS started as `mode`
/// says.
///
/// The conversation is fixed. With [`SmtpTls::StartTls`] it opens in
/// plaintext: greeting `220`, `EHLO` `250` offering `STARTTLS`,
/// `STARTTLS` `220`, then the handshake. From there every mode is the
/// same: greeting `220` (implicit and none only), `EHLO` `250`,
/// `AUTH PLAIN` `235` (only when a login is given; without one no `AUTH` is
/// sent), `MAIL FROM` `250`, `RCPT TO` `250` or `251`, `DATA`
/// `354`, the message `250`, then `QUIT`, whose answer is not waited for.
/// Any other code at any step is [`Refusal::RelayRefused`] with that code,
/// and the connection is closed. `EHLO` names the sender's domain.
///
/// With [`SmtpTls::None`] all of it, the credential included, crosses the
/// network in plaintext. The app developer chose that with
/// `ORDO_MAIL_TLS=none`; it is never a fallback.
pub async fn submit_over(
    mode: SmtpTls,
    dest: PinnedDestination,
    roots: &TlsRoots,
    login: Option<&SmtpLogin>,
    mail: Mail<'_>,
    timeout: Duration,
) -> Result<Accepted, Refusal> {
    let data = render(&mail)?;
    let ms = timeout.as_millis() as u64;
    let helo = mail
        .from
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or("localhost");
    let attempt = async {
        match mode {
            SmtpTls::Implicit => {
                let mut tls = dial(&dest, roots).await?;
                let mut r = Replies::default();
                r.expect(&mut tls, &[220]).await?;
                session(tls, r, helo, login, &mail, &data).await
            }
            SmtpTls::None => {
                let mut tcp = connect(&dest).await?;
                let mut r = Replies::default();
                r.expect(&mut tcp, &[220]).await?;
                session(tcp, r, helo, login, &mail, &data).await
            }
            SmtpTls::StartTls => {
                let mut tcp = connect(&dest).await?;
                let mut r = Replies::default();
                r.expect(&mut tcp, &[220]).await?;
                command(&mut tcp, &format!("EHLO {helo}\r\n")).await?;
                if !r.ehlo(&mut tcp).await? {
                    return Err(Refusal::TlsFailed {
                        detail: NO_STARTTLS.into(),
                    });
                }
                r.nothing_unasked()?;
                command(&mut tcp, "STARTTLS\r\n").await?;
                r.expect(&mut tcp, &[220]).await?;
                r.nothing_unasked()?;
                // What the relay said in plaintext is dropped with `r`; the
                // reader inside TLS starts empty.
                drop(r);
                let tls = handshake(&dest, roots, tcp).await?;
                session(tls, Replies::default(), helo, login, &mail, &data).await
            }
        }
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(r) => r,
        Err(_) => Err(Refusal::TimedOut { after_ms: ms }),
    }
}

/// The refusal detail for a relay whose `EHLO` leaves `STARTTLS` out. `pub`
/// so a caller can tell it apart and name the setting to change.
pub const NO_STARTTLS: &str = "the relay does not offer STARTTLS; nothing is sent without TLS";

/// The submission itself, from `EHLO` on, over whatever the stream is.
async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    mut r: Replies,
    helo: &str,
    login: Option<&SmtpLogin>,
    mail: &Mail<'_>,
    data: &[u8],
) -> Result<Accepted, Refusal> {
    command(&mut s, &format!("EHLO {helo}\r\n")).await?;
    r.expect(&mut s, &[250]).await?;
    if let Some(login) = login {
        command(&mut s, &format!("AUTH PLAIN {}\r\n", login.plain())).await?;
        r.expect(&mut s, &[235]).await?;
    }
    command(&mut s, &format!("MAIL FROM:<{}>\r\n", mail.from)).await?;
    r.expect(&mut s, &[250]).await?;
    command(&mut s, &format!("RCPT TO:<{}>\r\n", mail.to)).await?;
    r.expect(&mut s, &[250, 251]).await?;
    command(&mut s, "DATA\r\n").await?;
    r.expect(&mut s, &[354]).await?;
    s.write_all(data).await.map_err(io_refusal)?;
    s.flush().await.map_err(io_refusal)?;
    let code = r.expect(&mut s, &[250]).await?;
    let _ = command(&mut s, "QUIT\r\n").await;
    let _ = s.shutdown().await;
    Ok(Accepted { status: code })
}

async fn command<S: AsyncWrite + Unpin>(s: &mut S, line: &str) -> Result<(), Refusal> {
    s.write_all(line.as_bytes()).await.map_err(io_refusal)?;
    s.flush().await.map_err(io_refusal)
}

fn io_refusal(e: std::io::Error) -> Refusal {
    Refusal::ConnectFailed {
        detail: e.to_string(),
    }
}

/// The reply reader: buffered, bounded, and reading codes only.
#[derive(Default)]
struct Replies {
    buf: Vec<u8>,
}

impl Replies {
    /// Read one whole reply and require its code to be one of `want`.
    async fn expect<S: AsyncRead + Unpin>(
        &mut self,
        s: &mut S,
        want: &[u16],
    ) -> Result<u16, Refusal> {
        let (code, _) = self.reply(s, false).await?;
        if want.contains(&code) {
            Ok(code)
        } else {
            Err(Refusal::RelayRefused { code })
        }
    }

    /// Read the plaintext `EHLO` reply, require `250`, and say whether it
    /// offers `STARTTLS`. The one word is looked for; the rest is dropped.
    async fn ehlo<S: AsyncRead + Unpin>(&mut self, s: &mut S) -> Result<bool, Refusal> {
        let (code, starttls) = self.reply(s, true).await?;
        if code != 250 {
            return Err(Refusal::RelayRefused { code });
        }
        Ok(starttls)
    }

    /// Refuse when the relay has sent bytes nobody asked for. Before
    /// `STARTTLS` and after its `220`, any such byte is plaintext an
    /// attacker could have placed to be read as if it came inside TLS.
    fn nothing_unasked(&self) -> Result<(), Refusal> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(Refusal::TlsFailed {
                detail: "the relay sent bytes ahead of its turn around STARTTLS; \
                         nothing is sent without TLS"
                    .into(),
            })
        }
    }

    /// One reply, which may span lines (`250-…` then `250 …`). With
    /// `want_starttls`, also whether a line's keyword is `STARTTLS`.
    async fn reply<S: AsyncRead + Unpin>(
        &mut self,
        s: &mut S,
        want_starttls: bool,
    ) -> Result<(u16, bool), Refusal> {
        let mut starttls = false;
        for _ in 0..REPLY_LINES_CAP {
            let line = self.line(s).await?;
            let (code, last) = parse_reply_line(&line)?;
            if want_starttls && offers_starttls(&line) {
                starttls = true;
            }
            if last {
                return Ok((code, starttls));
            }
        }
        Err(Refusal::MalformedResponse {
            detail: format!("a reply of more than {REPLY_LINES_CAP} lines"),
        })
    }

    async fn line<S: AsyncRead + Unpin>(&mut self, tls: &mut S) -> Result<Vec<u8>, Refusal> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let rest = self.buf.split_off(i + 2);
                let mut line = std::mem::replace(&mut self.buf, rest);
                line.truncate(i);
                return Ok(line);
            }
            if self.buf.len() > REPLY_LINE_CAP {
                return Err(Refusal::MalformedResponse {
                    detail: "a reply line is too long".into(),
                });
            }
            let mut chunk = [0u8; 1024];
            let n = tls.read(&mut chunk).await.map_err(io_refusal)?;
            if n == 0 {
                return Err(Refusal::MalformedResponse {
                    detail: "the relay closed inside a reply".into(),
                });
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// A reply line's code, and whether it is the last line of its reply.
pub(crate) fn parse_reply_line(line: &[u8]) -> Result<(u16, bool), Refusal> {
    let bad = || Refusal::MalformedResponse {
        detail: "not an SMTP reply line".into(),
    };
    if line.len() < 3 || !line[..3].iter().all(u8::is_ascii_digit) {
        return Err(bad());
    }
    let code =
        (line[0] - b'0') as u16 * 100 + (line[1] - b'0') as u16 * 10 + (line[2] - b'0') as u16;
    if !(200..600).contains(&code) {
        return Err(bad());
    }
    match line.get(3) {
        None | Some(b' ') => Ok((code, true)),
        Some(b'-') => Ok((code, false)),
        Some(_) => Err(bad()),
    }
}

/// Is this `EHLO` reply line the `STARTTLS` extension (RFC 3207 §4)? The
/// keyword is the line's first word after the code, in any case.
pub(crate) fn offers_starttls(line: &[u8]) -> bool {
    line.len() >= 4
        && line.starts_with(b"250")
        && line[4..]
            .split(|b| *b == b' ')
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case(b"STARTTLS"))
}

/// Standard base64 with padding. Twenty lines here instead of a dependency
/// (`tests/holds_nothing.rs` pins this crate's list).
pub(crate) fn base64(raw: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(raw.len().div_ceil(3) * 4);
    for chunk in raw.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ABC[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ABC[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `Thu, 08 Oct 2026 12:00:00 +0000`, from seconds since the epoch.
pub(crate) fn rfc5322_date(secs: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days, for days since 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        DAYS[(days % 7) as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail<'a>(to: &'a str, subject: &'a str, text: &'a str) -> Mail<'a> {
        Mail {
            from: "signin@example.com",
            to,
            subject,
            text,
            date: 0,
            message_id: None,
        }
    }

    #[test]
    fn the_address_grammar_refuses_every_way_to_smuggle_a_header() {
        for good in [
            "nelly@example.com",
            "a.b+tag@mail.example.co.uk",
            "x@xn--bcher-kva.example",
        ] {
            assert!(address_ok(good), "{good}");
        }
        for bad in [
            "nelly@example.com\r\nBcc: all@example.com",
            "nelly@example.com\n",
            "Nelly <nelly@example.com>",
            "nelly@localhost",
            "\"quoted\"@example.com",
            "a..b@example.com",
            ".a@example.com",
            "a@-example.com",
            "a@example.com>",
            "a@[127.0.0.1]",
            "a b@example.com",
            "nélly@example.com",
            "",
            "@example.com",
        ] {
            assert!(!address_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_line_starting_with_a_dot_is_doubled_and_the_message_ends_with_one() {
        let out = render(&mail("kim@example.org", "Your code", ".hidden\nline\n")).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\r\n\r\n..hidden\r\nline\r\n.\r\n"), "{s}");
        assert!(s.starts_with("Date: Thu, 01 Jan 1970 00:00:00 +0000\r\n"));
        assert_eq!(
            s.matches("kim@example.org").count(),
            1,
            "one header carries it"
        );
    }

    #[test]
    fn a_subject_or_text_with_a_control_character_is_not_representable() {
        for (subject, text) in [
            ("Code\r\nBcc: x@example.com", "fine"),
            ("Code", "bell\x07"),
            ("Code", "a lone \r in the middle"),
            ("", "empty subject"),
        ] {
            let e = render(&mail("n@example.com", subject, text)).unwrap_err();
            assert_eq!(e.kind(), "not_representable");
        }
    }

    #[test]
    fn reply_lines_are_read_for_their_code_alone() {
        assert_eq!(
            parse_reply_line(b"250-mx.example.com").unwrap(),
            (250, false)
        );
        assert_eq!(parse_reply_line(b"250 OK").unwrap(), (250, true));
        assert_eq!(parse_reply_line(b"354").unwrap(), (354, true));
        assert!(parse_reply_line(b"25").is_err());
        assert!(parse_reply_line(b"abc def").is_err());
        assert!(parse_reply_line(b"250xOK").is_err());
        assert!(parse_reply_line(b"999 no").is_err());
    }

    #[test]
    fn starttls_is_read_as_a_keyword_and_nowhere_else() {
        for yes in [&b"250-STARTTLS"[..], b"250 starttls", b"250-StartTLS x"] {
            assert!(offers_starttls(yes), "{}", String::from_utf8_lossy(yes));
        }
        for no in [
            &b"250-mx.example.com says STARTTLS"[..],
            b"250-STARTTLSX",
            b"220 STARTTLS",
            b"250",
            b"250-AUTH PLAIN",
        ] {
            assert!(!offers_starttls(no), "{}", String::from_utf8_lossy(no));
        }
        assert_eq!(SmtpTls::parse("STARTTLS"), Some(SmtpTls::StartTls));
        assert_eq!(SmtpTls::parse("implicit"), Some(SmtpTls::Implicit));
        assert_eq!(SmtpTls::parse("plain"), None);
        assert_eq!(SmtpTls::parse("None"), Some(SmtpTls::None));
        assert_eq!(SmtpTls::parse(""), None);
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), enc);
        }
        let login = SmtpLogin::new("user", "pass").unwrap();
        assert_eq!(login.plain(), "AHVzZXIAcGFzcw==");
    }

    #[test]
    fn a_login_with_a_control_character_is_refused_without_naming_it() {
        let e = SmtpLogin::new("user", "zq\0xv").err().unwrap();
        assert!(!e.contains("zq") && !e.contains("xv"), "{e}");
        assert!(SmtpLogin::new("", "x").is_err());
    }

    #[test]
    fn dates_render_in_rfc_5322_form() {
        assert_eq!(rfc5322_date(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        // 2026-10-08T12:34:56Z, a Thursday.
        assert_eq!(
            rfc5322_date(1_791_462_896),
            "Thu, 08 Oct 2026 12:34:56 +0000"
        );
        // 2000-02-29, a leap day, a Tuesday.
        assert_eq!(rfc5322_date(951_782_400), "Tue, 29 Feb 2000 00:00:00 +0000");
    }
}
