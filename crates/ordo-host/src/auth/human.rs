//! The optional human check at `begin` (`plan-auth-federated.md` §7.4 E15).
//!
//! `ORDO_AUTH_HUMAN_CHECK=turnstile` makes `POST /auth/begin` require a
//! Cloudflare Turnstile token in the body's `human` field, and verifies it at
//! Cloudflare's siteverify through [`ordo_egress::fetch`]. Off by default.
//!
//! **One check, called from every `begin`.** [`AuthState::check_human`] is the
//! whole surface. The emailed-code `begin` and a first guest sign-in call it
//! the same way `/auth/begin` does:
//!
//! ```text
//! if let Err(refusal) = auth.check_human(&body, now).await { return *refusal; }
//! ```
//!
//! Three things the check keeps:
//!
//! - **It runs first.** A `begin` without a token, or with a bad one, has
//!   sent nothing to a provider, a mailbox or a database by the time it is
//!   refused.
//! - **The visitor's address is never sent.** siteverify's `remoteip` is
//!   optional, and Ordo relies on no client address (owner, 2026-10-08).
//! - **One answer.** A missing token, a rejected token and a siteverify that
//!   cannot be reached all answer the sign-in's one refusal. The reason goes
//!   to the security stream only. The secret is read from a file and is held
//!   by a type with no `Debug`.

use std::time::Duration;

use axum::response::Response;
use ordo_egress::{
    Allowlist, DenyList, FetchMethod, FetchRequest, HttpsUrl, Refusal as EgressRefusal, TlsRoots,
};
use ordo_identity::federation::{form_encode, ClientSecret};
use ordo_identity::Timestamp;
use serde_json::Value;

use super::{refused, AuthState};

/// Cloudflare's siteverify, the default for `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL`.
pub const TURNSTILE_VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

/// How long siteverify may take by default, connect to last byte
/// (`ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS`).
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest siteverify answer read. A real one is a few hundred bytes.
const VERIFY_CAP: usize = 16 * 1024;
/// Turnstile documents tokens of at most 2048 characters.
const TOKEN_MAX: usize = 2048;

/// A configured check. Holds the secret, so it has no `Debug`.
pub struct HumanCheck {
    secret: ClientSecret,
    url: HttpsUrl,
    allow: Allowlist,
    deny: DenyList,
    roots: TlsRoots,
    timeout: Duration,
    /// The public site key (`ORDO_AUTH_HUMAN_CHECK_SITE_KEY`), shown to
    /// pages by `GET /auth/methods`. Public by design: it is in every page
    /// that renders the widget.
    site_key: Option<String>,
}

/// Why a `begin` did not pass. Goes to the security stream, never to the
/// caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HumanFailure {
    Missing,
    Rejected,
    Unavailable,
}

impl HumanFailure {
    fn reason(self) -> &'static str {
        match self {
            HumanFailure::Missing => "human-check-missing",
            HumanFailure::Rejected => "human-check-rejected",
            HumanFailure::Unavailable => "human-check-unavailable",
        }
    }
}

impl HumanCheck {
    /// A check against `verify_url`, which must be an `https` address. The
    /// allowlist is exactly that address's host.
    pub fn new(
        secret: String,
        verify_url: &str,
        roots: TlsRoots,
        deny: DenyList,
    ) -> Result<HumanCheck, String> {
        let secret = ClientSecret::new(secret);
        if secret.is_empty() {
            return Err("the secret file is empty".into());
        }
        let url = HttpsUrl::parse(verify_url).map_err(|e| e.disclosable())?;
        let allow = Allowlist::exactly(url.host());
        Ok(HumanCheck {
            secret,
            url,
            allow,
            deny,
            roots,
            timeout: VERIFY_TIMEOUT,
            site_key: None,
        })
    }

    /// Record the widget's public site key, for `GET /auth/methods`.
    pub fn with_site_key(mut self, key: Option<String>) -> HumanCheck {
        self.site_key = key;
        self
    }

    /// The public site key, when one is configured.
    pub fn site_key(&self) -> Option<&str> {
        self.site_key.as_deref()
    }

    /// Replace the default siteverify timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> HumanCheck {
        self.timeout = timeout;
        self
    }

    /// Ask siteverify whether `token` came from a person's browser.
    pub async fn verify(&self, token: Option<&str>) -> Result<(), HumanFailure> {
        let token = match token {
            Some(t) if t.len() > TOKEN_MAX => return Err(HumanFailure::Rejected),
            Some(t) if !t.is_empty() => t,
            _ => return Err(HumanFailure::Missing),
        };
        let unavailable = |_: EgressRefusal| HumanFailure::Unavailable;
        let dest = ordo_egress::resolve_and_pin(&self.url, &self.allow, &self.deny)
            .await
            .map_err(unavailable)?;
        // No `remoteip`: Ordo reads and sends no client address.
        let body = form_encode(&[
            ("secret", self.secret.reveal_to_wire()),
            ("response", token),
        ]);
        let fetched = ordo_egress::fetch(
            dest,
            &self.roots,
            FetchRequest {
                method: FetchMethod::PostForm,
                body: body.as_bytes(),
                accept: "application/json",
                headers: &[],
            },
            VERIFY_CAP,
            self.timeout,
        )
        .await
        .map_err(unavailable)?;
        let answer: Value = serde_json::from_slice(&fetched.into_verifier_input())
            .map_err(|_| HumanFailure::Unavailable)?;
        match answer.get("success").and_then(Value::as_bool) {
            Some(true) => Ok(()),
            Some(false) => Err(HumanFailure::Rejected),
            None => Err(HumanFailure::Unavailable),
        }
    }
}

/// Read `ORDO_AUTH_HUMAN_CHECK` and its companions. `Ok(None)` is off, which
/// is the default.
///
/// | Variable | Meaning |
/// |---|---|
/// | `ORDO_AUTH_HUMAN_CHECK` | `turnstile`, or unset (also `off`) |
/// | `ORDO_AUTH_HUMAN_CHECK_SECRET_FILE` | required when on: the Turnstile secret key |
/// | `ORDO_AUTH_HUMAN_CHECK_SITE_KEY` | optional: the widget's public site key, shown by `GET /auth/methods` |
/// | `ORDO_AUTH_HUMAN_CHECK_VERIFY_URL` | siteverify, default Cloudflare's |
///
/// A secret in `ORDO_AUTH_HUMAN_CHECK_SECRET` is refused, and no message here
/// carries a secret.
pub fn from_vars(
    var: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&str) -> Result<String, String>,
    deny: DenyList,
) -> Result<Option<HumanCheck>, (&'static str, String)> {
    const MODE: &str = "ORDO_AUTH_HUMAN_CHECK";
    const FILE: &str = "ORDO_AUTH_HUMAN_CHECK_SECRET_FILE";
    const URL: &str = "ORDO_AUTH_HUMAN_CHECK_VERIFY_URL";
    const SECS: &str = "ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS";
    const SITE: &str = "ORDO_AUTH_HUMAN_CHECK_SITE_KEY";
    match var(MODE).as_deref().map(str::trim) {
        None | Some("") | Some("off") => return Ok(None),
        Some("turnstile") => {}
        Some(other) => {
            return Err((
                MODE,
                format!("`{other}` is not a human check (turnstile | off; default off)"),
            ))
        }
    }
    if var("ORDO_AUTH_HUMAN_CHECK_SECRET").is_some() {
        return Err((
            "ORDO_AUTH_HUMAN_CHECK_SECRET",
            format!(
                "not read: a secret in a variable's value shows in a process listing; \
                 put it in a file and name the file in {FILE}"
            ),
        ));
    }
    let path = var(FILE)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .ok_or((FILE, "is required when the human check is on".to_string()))?;
    let secret = read_file(&path).map_err(|_| (FILE, "could not be read".to_string()))?;
    let url = var(URL)
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| TURNSTILE_VERIFY_URL.to_string());
    let secs = match var(SECS)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => VERIFY_TIMEOUT.as_secs(),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=30).contains(n))
            .ok_or((
                SECS,
                format!(
                    "`{v}` is not a number of seconds from 1 to 30 (default {})",
                    VERIFY_TIMEOUT.as_secs()
                ),
            ))?,
    };
    let site_key = match var(SITE)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => None,
        Some(k)
            if k.len() <= 128
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
        {
            Some(k)
        }
        Some(_) => {
            return Err((
                SITE,
                "is not a site key: letters, digits, `_` and `-`, at most 128".to_string(),
            ))
        }
    };
    HumanCheck::new(secret, &url, TlsRoots::webpki(), deny)
        .map(|c| c.with_timeout(Duration::from_secs(secs)))
        .map(|c| c.with_site_key(site_key))
        .map(Some)
        .map_err(|detail| (URL, detail))
}

impl AuthState {
    /// The human check, for any `begin`. With the check off this is `Ok` and
    /// does nothing. With it on, the body's `human` string must verify, or
    /// the sign-in's one refusal is returned and the reason is recorded.
    ///
    /// Call it before the route does any other work.
    pub async fn check_human(&self, body: &Value, now: Timestamp) -> Result<(), Box<Response>> {
        self.check_human_for("auth.begin", body, now).await
    }

    /// [`AuthState::check_human`] for a route that is not a `begin`: the
    /// refusal is recorded under `op`.
    pub async fn check_human_for(
        &self,
        op: &str,
        body: &Value,
        now: Timestamp,
    ) -> Result<(), Box<Response>> {
        let Some(check) = &self.human else {
            return Ok(());
        };
        match check
            .verify(body.get("human").and_then(Value::as_str))
            .await
        {
            Ok(()) => Ok(()),
            Err(why) => {
                self.record_refusal(op, why.reason(), now);
                Err(Box::new(refused()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let m: Vec<_> = pairs.to_vec();
        move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    fn deny() -> DenyList {
        DenyList::categories_only()
    }

    #[test]
    fn the_verify_timeout_is_a_bounded_setting() {
        let read = |_: &str| Ok("s".to_string());
        let on = ("ORDO_AUTH_HUMAN_CHECK", "turnstile");
        let file = ("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE", "/s");
        let c = from_vars(&vars(&[on, file]), &read, deny())
            .unwrap()
            .unwrap();
        assert_eq!(c.timeout, VERIFY_TIMEOUT);
        let ok = vars(&[on, file, ("ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS", "12")]);
        assert_eq!(
            from_vars(&ok, &read, deny()).unwrap().unwrap().timeout,
            Duration::from_secs(12)
        );
        for bad in ["0", "31", "five"] {
            let v = vars(&[on, file, ("ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS", bad)]);
            let (var, detail) = from_vars(&v, &read, deny()).err().unwrap();
            assert_eq!(var, "ORDO_AUTH_HUMAN_CHECK_VERIFY_SECS");
            assert!(detail.contains("default 5"), "{detail}");
        }
    }

    #[test]
    fn unset_and_off_are_off() {
        let read = |_: &str| Ok("s".to_string());
        assert!(from_vars(&vars(&[]), &read, deny()).unwrap().is_none());
        let off = vars(&[("ORDO_AUTH_HUMAN_CHECK", "off")]);
        assert!(from_vars(&off, &read, deny()).unwrap().is_none());
    }

    #[test]
    fn on_needs_a_secret_file_and_refuses_a_secret_value() {
        let read = |_: &str| Ok("hush".to_string());
        let on = ("ORDO_AUTH_HUMAN_CHECK", "turnstile");
        let e = from_vars(&vars(&[on]), &read, deny()).err().unwrap();
        assert_eq!(e.0, "ORDO_AUTH_HUMAN_CHECK_SECRET_FILE");
        let with_value = vars(&[on, ("ORDO_AUTH_HUMAN_CHECK_SECRET", "hush")]);
        let e = from_vars(&with_value, &read, deny()).err().unwrap();
        assert!(!e.1.contains("hush"));
        let file = ("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE", "/f");
        let unreadable = |_: &str| Err("/f: some path text".to_string());
        let e = from_vars(&vars(&[on, file]), &unreadable, deny())
            .err()
            .unwrap();
        assert_eq!(e.1, "could not be read");
        assert!(from_vars(&vars(&[on, file]), &read, deny())
            .unwrap()
            .is_some());
        let insecure = (
            "ORDO_AUTH_HUMAN_CHECK_VERIFY_URL",
            "http://insecure.example/",
        );
        assert!(from_vars(&vars(&[on, file, insecure]), &read, deny()).is_err());
    }
}
