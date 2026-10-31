//! The security round's attack catalog, replayed as rows of the matrix.
//!
//! `security/attack-catalog/*.jsonl` holds every attack the round of
//! 2026-10-24 ran. Each `http` entry is one [`Spec`] here, carrying its
//! catalog id in the row's name (`FDATA-017 ...`), its request mapped onto
//! the world's actors and objects, and the answer that is safe **today**.
//! Where a catalog entry was a defect since fixed, the row holds the fixed
//! behaviour (and the ruling in `docs/corpus-rulings.md` where one decides).
//!
//! The catalog's own victims map as follows. Kim and Alice are `member_a`
//! (Ann, a manager in T1); Lee and Bob are `t2_owner` (Carl, who owns T2).
//! Their ops map onto the fixture program: `myNotes`, `noteById`,
//! `createNote`, `editNote`, `deleteNote`, `addFile`, `notesMatching`.
//!
//! Entries the world cannot express are in [`SKIPPED`], each with a reason.
//! The guard test [`every_catalog_http_id_has_a_row_or_a_reason`] fails when
//! a catalog id is in neither, and `security/attack-catalog/COVERAGE.md`
//! lists every id (all 335) with where it is guarded.
//!
//! Rows run in order, so a few share state (`FDATA-053`, `FID-SESSION-002`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::actors::{self, World};
use super::policy::{Expect, Mark};
use super::{Answer, Deployment, FOREIGN, HOME, PAGE};
use std::time::Duration;

/// A property of the answer a status code alone cannot say (a header, a
/// body that must not hold something). A row with a check passes when the
/// check does.
pub type Check = fn(&Answer) -> bool;
/// A row that is a short script, not one request.
pub type Script = fn(&Deployment, &World, &Overlay) -> Answer;

/// Objects this module makes itself, by placeholder name.
pub type Overlay = BTreeMap<String, String>;

/// Entries the world cannot express, with the reason.
pub const SKIPPED: &[(&str, &str)] = &[
    // Guessing limits: the other agent's rows.
    ("DEPLOY-016", "covered by the guessing-limits rows"),
    ("DATA-032", "covered by the guessing-limits rows"),
    // A differently configured boot.
    (
        "DEPLOY-013",
        "needs a boot with only the required settings (no sign-in method on)",
    ),
    (
        "DEPLOY-015",
        "needs a boot with the password method on and a closed door",
    ),
    ("DEPLOY-017", "needs a boot with the password method on"),
    (
        "DEPLOY-025",
        "needs Postgres stopped under the running door",
    ),
    // A second server.
    // Replayed against a real console in
    // `crates/ordo-console/tests/catalog_attacks.rs`, one test per id.
    (
        "FID-CONSOLE-001",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "FID-CONSOLE-002",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "FID-CONSOLE-003",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "FID-CONSOLE-004",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "FID-CONSOLE-005",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "FID-CONSOLE-010",
        "file modes on the console's data directory, not an HTTP answer",
    ),
    (
        "ID-040",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-041",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-042",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-043",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-044",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-045",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-046",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-047",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-048",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-049",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    (
        "ID-050",
        "console test: crates/ordo-console/tests/catalog_attacks.rs",
    ),
    ("SYNC-013", "needs pool-pressure timing across paced feeds"),
    // Not an outcome.
    ("DATA-046", "a timing comparison, not an outcome"),
];

/// Known defects found by replaying the catalog: id to a line.
pub const DEFECTS: &[(&str, &str)] = &[];

// ── a row ───────────────────────────────────────────────────────────────

enum SpaceHdr {
    /// The actor's own space.
    Actors,
    None,
    Is(String),
}

pub struct Spec {
    pub id: &'static str,
    what: &'static str,
    method: &'static str,
    path: String,
    actor: &'static str,
    space: SpaceHdr,
    headers: Vec<(String, String)>,
    body: String,
    expect: Expect,
    mark: Mark,
    check: Option<Check>,
    script: Option<Script>,
}

fn r(
    id: &'static str,
    what: &'static str,
    method: &'static str,
    path: &str,
    actor: &'static str,
    expect: Expect,
) -> Spec {
    Spec {
        id,
        what,
        method,
        path: path.to_string(),
        actor,
        space: SpaceHdr::Actors,
        headers: Vec::new(),
        body: String::new(),
        expect,
        mark: Mark::Holds,
        check: None,
        script: None,
    }
}

/// A row that is a script; `expect` is what the script's answer must be.
fn script(id: &'static str, what: &'static str, f: Script, expect: Expect) -> Spec {
    let mut s = r(id, what, "SCRIPT", "", "anonymous", expect);
    s.script = Some(f);
    s
}

impl Spec {
    fn space(mut self, t: &str) -> Self {
        self.space = SpaceHdr::Is(t.to_string());
        self
    }
    fn no_space(mut self) -> Self {
        self.space = SpaceHdr::None;
        self
    }
    /// Set a header; an empty value removes it.
    fn hdr(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    fn body(mut self, b: &str) -> Self {
        self.body = b.to_string();
        self
    }
    fn json(self, v: Value) -> Self {
        self.body(&v.to_string())
    }
    fn check(mut self, f: Check) -> Self {
        self.check = Some(f);
        self.expect = Expect::Code(200, "");
        self
    }
    fn known_defect(mut self, id: &'static str) -> Self {
        self.mark = Mark::KnownDefect(id);
        self
    }
}

fn fill(w: &World, o: &Overlay, s: &str) -> String {
    let mut t = s.to_string();
    for (k, v) in o {
        t = t.replace(&format!("{{{k}}}"), v);
    }
    w.fill(&t)
}

fn put_header(h: &mut Vec<(String, String)>, k: &str, v: String) {
    h.retain(|(n, _)| !n.eq_ignore_ascii_case(k));
    if !v.is_empty() {
        h.push((k.to_string(), v));
    }
}

impl Spec {
    fn request(&self, w: &World, o: &Overlay) -> (String, Vec<(String, String)>, Vec<u8>) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let a = w.actor(self.actor);
        let mut h: Vec<(String, String)> = vec![
            ("Origin".into(), PAGE.into()),
            ("Accept".into(), "application/json".into()),
        ];
        if let Some(t) = &a.token {
            h.push(("Authorization".into(), format!("Bearer {t}")));
        }
        match &self.space {
            SpaceHdr::Actors => h.push(("X-Ordo-Space".into(), a.space.clone())),
            SpaceHdr::None => {}
            SpaceHdr::Is(t) => h.push(("X-Ordo-Space".into(), fill(w, o, t))),
        }
        if !matches!(self.method, "GET" | "OPTIONS" | "HEAD") {
            h.push((
                "Idempotency-Key".into(),
                format!("catalog-{}", KEY.fetch_add(1, Ordering::SeqCst)),
            ));
        }
        if !self.body.is_empty() {
            h.push(("Content-Type".into(), "application/json".into()));
        }
        for (k, v) in &self.headers {
            put_header(&mut h, k, fill(w, o, v));
        }
        (
            fill(w, o, &self.path),
            h,
            fill(w, o, &self.body).into_bytes(),
        )
    }
}

// ── the checks ──────────────────────────────────────────────────────────

fn acao(a: &Answer) -> Option<&str> {
    a.header("access-control-allow-origin")
}
/// No page of another origin is handed the answer.
fn no_acao(a: &Answer) -> bool {
    acao(a).is_none()
}
fn is_403_without_acao(a: &Answer) -> bool {
    a.status == 403 && no_acao(a)
}
fn preflight_not_credentialed(a: &Answer) -> bool {
    let wildcard = acao(a) == Some("*");
    a.header("access-control-allow-credentials").is_none()
        && (acao(a).is_none()
            || (wildcard
                && !a
                    .header("access-control-allow-headers")
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains("authorization")))
}
fn ok_without_acao(a: &Answer) -> bool {
    a.status == 200 && no_acao(a)
}
/// A stranger's health answer is the status alone.
fn health_status_alone(a: &Answer) -> bool {
    let j = a.json();
    let Some(o) = j.as_object() else { return false };
    ![
        "checks",
        "failed",
        "quarantined",
        "credential_labels",
        "program_active",
        "nodes",
        "node",
        "version",
        "pending",
    ]
    .iter()
    .any(|k| o.contains_key(*k))
        && (a.status == 200 || a.status == 503)
}
fn hardened(a: &Answer) -> bool {
    a.header("x-content-type-options") == Some("nosniff")
        && a.header("x-frame-options") == Some("DENY")
        && a.header("referrer-policy") == Some("no-referrer")
}
/// Active content comes down as an attachment under the sandbox policy.
fn neutralised(a: &Answer) -> bool {
    a.status == 200
        && a.header("x-content-type-options") == Some("nosniff")
        && a.header("content-type")
            .unwrap_or("")
            .starts_with("application/octet-stream")
        && a.header("content-disposition")
            .unwrap_or("")
            .starts_with("attachment")
        && a.header("content-security-policy")
            .unwrap_or("")
            .contains("sandbox")
}
/// A type the door may show inline (a PDF, a raster image) still carries
/// the policy and `nosniff`.
fn shown_safely(a: &Answer) -> bool {
    let ct = a.header("content-type").unwrap_or("");
    a.status == 200
        && a.header("x-content-type-options") == Some("nosniff")
        && a.header("content-security-policy").is_some()
        && (neutralised(a) || ct.starts_with("application/pdf") || ct.starts_with("image/png"))
}
fn four_xx(a: &Answer) -> bool {
    (400..500).contains(&a.status)
}
/// Never a server fault, and never another person's rows.
fn only_own_rows(a: &Answer) -> bool {
    a.status < 500 && !a.text().contains("Ben's note") && !a.text().contains("Carl's note")
}
fn not_param_refusal_nor_fault(a: &Answer) -> bool {
    a.status < 500 && a.code() != "invalid_param"
}
fn full_plain(a: &Answer) -> bool {
    a.status == 200 && a.text() == PLAIN
}
fn range_first_or_full(a: &Answer) -> bool {
    (a.status == 206 && a.text() == "012") || full_plain(a)
}
fn has_token(a: &Answer) -> bool {
    a.status == 200 && a.json()["token"].as_str().is_some()
}
fn only_linked_note(a: &Answer) -> bool {
    a.status == 200
        && a.text().contains("Behind a link")
        && !a.text().contains("Ann's note")
        && !a.text().contains("Ben's note")
}
fn some_rows(a: &Answer) -> bool {
    a.status == 200 && a.json()["rows"].as_array().is_some_and(|r| !r.is_empty())
}
fn no_rows(a: &Answer) -> bool {
    a.status == 200 && a.json()["rows"].as_array().is_some_and(Vec::is_empty)
}
fn rotated_cookie(a: &Answer) -> bool {
    a.status == 200 && cookie_of(a).is_some()
}

static IDEM_FIRST: Mutex<String> = Mutex::new(String::new());
fn idem_first(a: &Answer) -> bool {
    *IDEM_FIRST.lock().unwrap() = a.text();
    a.status == 200
}
fn idem_second(a: &Answer) -> bool {
    a.status == 200 && a.text() != *IDEM_FIRST.lock().unwrap()
}

const PLAIN: &str = "0123456789abcdef";

fn cookie_of(a: &Answer) -> Option<String> {
    a.cookies().into_iter().find_map(|c| {
        let first = c.split(';').next()?.trim().to_string();
        let (name, value) = first.split_once('=')?;
        (name.ends_with("ordo_renew") && !value.is_empty()).then_some(first)
    })
}

// ── scripts ─────────────────────────────────────────────────────────────

fn synthetic(ok: bool, real: &Answer) -> Answer {
    Answer {
        status: if ok { 200 } else { 0 },
        headers: real.headers.clone(),
        body: real.body.clone(),
    }
}

// ── emailed codes and the outside provider ──────────────────────────────

/// Sessions the relay has had for `to`.
fn mail_count(d: &Deployment, to: &str) -> usize {
    mailed_to(d, to).len()
}

/// The DATA of each relay session addressed to `to`, oldest first.
fn mailed_to(d: &Deployment, to: &str) -> Vec<String> {
    let want = to.to_lowercase();
    d.relay
        .sessions()
        .into_iter()
        .filter(|s| {
            s.commands.iter().any(|c| {
                c.strip_prefix("RCPT TO:")
                    .is_some_and(|a| a.trim().trim_matches(['<', '>']).to_lowercase() == want)
            })
        })
        .filter_map(|s| s.data)
        .collect()
}

/// The digits after `marker` in the newest message to `to`, once there are
/// more than `before` of them.
fn mailed_code(d: &Deployment, to: &str, before: usize, marker: &str) -> String {
    let data = super::eventually("the mail worker sends the code", || {
        let all = mailed_to(d, to);
        (all.len() > before).then(|| all.last().cloned().unwrap_or_default())
    });
    let start = data
        .find(marker)
        .unwrap_or_else(|| panic!("`{marker}` in {data}"))
        + marker.len();
    data[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// `begin` for an emailed code. The answer holds the challenge and binder.
fn email_begin(d: &Deployment, address: &str) -> (Answer, usize) {
    let before = mail_count(d, address);
    let a = d.auth(
        "/auth/begin",
        &json!({"method": "email", "identifier": address}),
        None,
    );
    (a, before)
}

fn email_complete(d: &Deployment, begun: &Value, code: &str, binder: &Value) -> Answer {
    d.auth(
        "/auth/complete",
        &json!({
            "method": "email", "kind": "email_code",
            "challenge": begun["challenge"], "binder": binder, "code": code,
        }),
        None,
    )
}

/// Sign in with an emailed code; the completed answer.
fn email_sign_in(d: &Deployment, address: &str) -> Answer {
    let (b, before) = email_begin(d, address);
    assert_eq!(b.status, 200, "begin: {}", b.text());
    let begun = b.json();
    let code = mailed_code(d, address, before, "code is ");
    let done = email_complete(d, &begun, &code, &begun["binder"]);
    assert_eq!(
        done.status,
        200,
        "an emailed code signs in: {}",
        done.text()
    );
    done
}

fn token_of(a: &Answer) -> String {
    a.json()["token"].as_str().expect("a token").to_string()
}

/// The principal a token names.
fn principal_of(token: &str) -> String {
    let payload = token.split('.').next().expect("a payload");
    let raw = ordo_identity::webauthn::b64url_decode(payload).expect("base64url");
    String::from_utf8(raw)
        .expect("text")
        .split('|')
        .nth(1)
        .expect("a principal")
        .to_string()
}

fn keys_of(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    k.sort();
    k
}

/// `begin` answers alike for an address with an account and one without:
/// the same status, the same fields, the same field lengths.
fn begin_alike(d: &Deployment, known: &str, unknown: &str) -> Answer {
    email_sign_in(d, known);
    let (k, _) = email_begin(d, known);
    let (u, _) = email_begin(d, unknown);
    let (kj, uj) = (k.json(), u.json());
    let len = |v: &Value, f: &str| v[f].as_str().map(str::len);
    let ok = k.status == 200
        && u.status == 200
        && keys_of(&kj) == keys_of(&uj)
        && len(&kj, "challenge") == len(&uj, "challenge")
        && len(&kj, "binder") == len(&uj, "binder");
    synthetic(ok, &u)
}

fn email_enumeration_a(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    begin_alike(d, "known.a@example.org", "unknown.a@example.org")
}

fn email_enumeration_b(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    begin_alike(d, "known.b@example.org", "unknown.b@example.org")
}

/// Seven wrong codes, then the real one. The fifth wrong try ended the
/// ceremony, so every answer is the one refusal, the real code included.
fn email_code_brute_force(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let addr = "brute.force@example.org";
    let (b, before) = email_begin(d, addr);
    assert_eq!(b.status, 200, "{}", b.text());
    let begun = b.json();
    let real = mailed_code(d, addr, before, "code is ");
    let wrong = |n: u32| {
        let w = format!("{:08}", n);
        if w == real {
            format!("{:08}", n + 1)
        } else {
            w
        }
    };
    let mut all_refused = true;
    for n in 0..7 {
        let a = email_complete(d, &begun, &wrong(n), &begun["binder"]);
        all_refused &= a.status == 401;
    }
    let last = email_complete(d, &begun, &real, &begun["binder"]);
    if all_refused {
        last
    } else {
        synthetic(true, &last)
    }
}

/// Another ceremony's binder does not complete this ceremony, with the
/// attacker's own code or with the victim's.
fn email_binder_swap(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let (victim, v_before) = email_begin(d, "victim.swap@example.org");
    let (attacker, a_before) = email_begin(d, "attacker.swap@example.org");
    let (vj, aj) = (victim.json(), attacker.json());
    let v_code = mailed_code(d, "victim.swap@example.org", v_before, "code is ");
    let a_code = mailed_code(d, "attacker.swap@example.org", a_before, "code is ");
    let own = email_complete(d, &vj, &a_code, &aj["binder"]);
    let theirs = email_complete(d, &vj, &v_code, &aj["binder"]);
    if own.status == 401 {
        theirs
    } else {
        synthetic(true, &own)
    }
}

/// A person signed in by email who has no contact address yet: the token.
fn email_person(d: &Deployment, address: &str) -> String {
    token_of(&email_sign_in(d, address))
}

/// Ask for a code for the contact `address` as `token` and answer with it.
fn contact_begin(d: &Deployment, token: &str, address: &str) -> (Answer, String) {
    let before = mail_count(d, address);
    let b = d.auth(
        "/auth/email/begin",
        &json!({"address": address}),
        Some(token),
    );
    assert_eq!(b.status, 200, "contact begin: {}", b.text());
    let code = mailed_code(d, address, before, "address is ");
    (b, code)
}

fn contact_complete(d: &Deployment, token: &str, b: &Answer, code: &str) -> Answer {
    let j = b.json();
    d.auth(
        "/auth/email/complete",
        &json!({"challenge": j["challenge"], "binder": j["binder"], "code": code}),
        Some(token),
    )
}

/// A first contact address needs no touch (finding F1: see the board).
fn contact_first_needs_no_touch(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let token = email_person(d, "grace.one@example.org");
    let (b, code) = contact_begin(d, &token, "attacker1.contact@example.org");
    let done = contact_complete(d, &token, &b, &code);
    let ok = done.status == 200 && done.json()["replaced"] == false;
    if ok {
        done
    } else {
        synthetic(false, &done)
    }
}

/// Changing a contact address without a fresh touch is refused, naming the
/// setting, and the address stays.
fn contact_change_needs_touch(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let token = email_person(d, "grace.two@example.org");
    let (b, code) = contact_begin(d, &token, "first.contact@example.org");
    let first = contact_complete(d, &token, &b, &code);
    assert_eq!(first.status, 200, "{}", first.text());
    let (b, code) = contact_begin(d, &token, "attacker2.contact@example.org");
    let change = contact_complete(d, &token, &b, &code);
    let read = d.send(
        "GET",
        "/auth/email",
        &[
            ("Origin", PAGE),
            ("Authorization", &format!("Bearer {token}")),
        ],
        "",
    );
    if read.json()["address"] == "first.contact@example.org" {
        change
    } else {
        synthetic(true, &change)
    }
}

/// A contact address is not a sign-in: signing in as it makes another
/// person, and the victim's own address still reaches the victim.
fn contact_is_not_a_sign_in(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let victim = "grace.three@example.org";
    let attacker = "attacker3.contact@example.org";
    let v1 = email_sign_in(d, victim);
    let token = token_of(&v1);
    let (b, code) = contact_begin(d, &token, attacker);
    let added = contact_complete(d, &token, &b, &code);
    assert_eq!(added.status, 200, "{}", added.text());
    let via_attacker = email_sign_in(d, attacker);
    let v2 = email_sign_in(d, victim);
    let ok = principal_of(&token_of(&via_attacker)) != principal_of(&token)
        && principal_of(&token_of(&v2)) == principal_of(&token);
    synthetic(ok, &via_attacker)
}

/// The outside provider's `begin`: the exact return origin gets a redirect.
fn outside_begin(d: &Deployment, return_to: &str) -> Answer {
    d.auth(
        "/auth/begin",
        &json!({"method": "discord", "return_to": return_to}),
        None,
    )
}

fn outside_redirect_to_provider(a: &Answer) -> bool {
    a.status == 200
        && a.json()["redirect"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://discord.com/"))
}

/// Eight look-alike return addresses are refused with no redirect, and the
/// exact origin still passes.
fn outside_return_bypasses(d: &Deployment, _: &World, _: &Overlay) -> Answer {
    let control = outside_begin(d, &format!("{}/x", super::OUTSIDE_RETURN));
    let mut last = control.clone();
    let mut ok = outside_redirect_to_provider(&control);
    for p in [
        "http://localhost:4981.evil.com/",
        "http://localhost:4981@evil.com/",
        "http://localhost:4981\\@evil.com/",
        "http://localhost:4981%2f@evil.com/",
        "//evil.com/",
        "javascript:alert(1)",
        "https://localhost:4981/",
        "http://localhost:4982/",
    ] {
        last = outside_begin(d, p);
        ok &= last.status == 401 && last.json().get("redirect").is_none();
    }
    if ok {
        last
    } else {
        synthetic(true, &last)
    }
}

fn ann_signs_in(d: &Deployment, w: &World) -> (String, String) {
    let ann = &w.people["ann"];
    let a = actors::sign_in(d, &ann.device, json!({"space": w.var("T1")}));
    assert_eq!(a.status, 200, "Ann signs in: {}", a.text());
    (
        a.json()["token"].as_str().expect("token").to_string(),
        cookie_of(&a).expect("renewal cookie"),
    )
}

fn refresh(d: &Deployment, cookie: &str) -> Answer {
    d.send(
        "POST",
        "/auth/refresh",
        &[("Origin", PAGE), ("Cookie", cookie)],
        "",
    )
}

/// The renewal cookie is HttpOnly, SameSite=Strict and scoped to its path;
/// over http on loopback it is not `Secure`.
fn cookie_flags(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let ann = &w.people["ann"];
    let a = actors::sign_in(d, &ann.device, json!({"space": w.var("T1")}));
    let c = a
        .cookies()
        .into_iter()
        .find(|c| c.contains("ordo_renew"))
        .unwrap_or_default();
    let ok = c.contains("HttpOnly")
        && c.contains("SameSite=Strict")
        && c.contains("Path=/auth/refresh")
        && !c.contains("Secure")
        && !c.starts_with("__Secure-");
    synthetic(ok, &a)
}

/// A logout ends the whole family: the cookie before and after a renewal.
fn logout_ends_family(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let (token, c0) = ann_signs_in(d, w);
    let renewed = refresh(d, &c0);
    let c1 = cookie_of(&renewed).unwrap_or_else(|| c0.clone());
    let out = d.send(
        "POST",
        "/auth/logout",
        &[
            ("Origin", PAGE),
            ("Authorization", &format!("Bearer {token}")),
            ("Cookie", &c1),
        ],
        "",
    );
    let cleared = out.cookies().iter().any(|c| c.contains("Max-Age=0"));
    let dead = refresh(d, &c0).status == 401 && refresh(d, &c1).status == 401;
    let mut a = synthetic(out.status == 204 && cleared && dead, &out);
    if a.status == 200 {
        a.status = 204;
    }
    a
}

/// A cookie whose family a logout ended is refused.
fn logged_out_cookie(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let (token, c0) = ann_signs_in(d, w);
    let out = d.send(
        "POST",
        "/auth/logout",
        &[
            ("Origin", PAGE),
            ("Authorization", &format!("Bearer {token}")),
            ("Cookie", &c0),
        ],
        "",
    );
    assert!(out.status < 300, "logout: {}", out.text());
    refresh(d, &c0)
}

// ── held-open streams ───────────────────────────────────────────────────
//
// A row opens a stream as an actor, does one thing (revokes, logs out,
// removes a member, writes a row), then reads for a short wait. The door
// asks again once a second here (`ORDO_LIVE_RECHECK_SECS=1`, set in
// `authz_support/mod.rs`), so a refusal arrives within the wait with no
// write at all.

/// Settle time after a stream opens, and the wait after the act.
const STREAM_OPEN: Duration = Duration::from_millis(500);
const STREAM_WAIT: Duration = Duration::from_secs(3);

fn verdict(ok: bool, seen: &str) -> Answer {
    Answer {
        status: if ok { 200 } else { 0 },
        headers: Vec::new(),
        body: seen.as_bytes().to_vec(),
    }
}

fn feed_headers(token: &str, space: &str) -> Vec<(String, String)> {
    vec![
        ("Origin".into(), PAGE.into()),
        ("Authorization".into(), format!("Bearer {token}")),
        ("X-Ordo-Space".into(), space.into()),
    ]
}

fn no_act() {}

/// Ann writes a row, so a feed that still lived would send a frame.
fn ann_writes(d: &Deployment, w: &World) {
    actors::must(
        d,
        &w.people["ann"],
        "POST",
        "createNote",
        json!({"title": "after-the-act", "body": "", "created": actors::stamp()}),
    );
}

/// Open `path` as `token`, run `act`, read. Safe when the feed's last word
/// is an `error` frame, no `change` or `invalidate` follows the act, and it
/// closes.
fn feed_refused_after(
    d: &Deployment,
    path: &str,
    token: &str,
    space: &str,
    act: &dyn Fn(),
) -> Answer {
    let mut s = d.open_stream(path, &feed_headers(token, space));
    if s.status != 200 {
        return verdict(false, &format!("the feed did not open: {}", s.status));
    }
    s.read_for(STREAM_OPEN);
    act();
    let after = s.read_for(STREAM_WAIT);
    let ok = after.contains("event: error")
        && !after.contains("event: change")
        && !after.contains("event: invalidate")
        && s.closed;
    verdict(ok, &format!("closed={} sent:\n{after}", s.closed))
}

/// A share link revoked while its holder's feed is open.
fn link_feed(d: &Deployment, w: &World, path: &str) -> Answer {
    let ann = &w.people["ann"];
    let (secret, id) = actors::link(d, ann, w.var("NOTE_LINKED"), json!({}));
    let opened = actors::open_link(d, &secret, None, Some("stream-device"));
    assert_eq!(opened.status, 200, "open link: {}", opened.text());
    let token = opened.json()["token"].as_str().expect("token").to_string();
    feed_refused_after(d, path, &token, &ann.space, &|| {
        actors::must(d, ann, "POST", "std.space.revokeLink", json!({"link": id}));
        ann_writes(d, w);
    })
}

fn link_sync_feed(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    link_feed(d, w, "/op/myNotes?live=1&sync=1")
}

fn link_invalidate_feed(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    link_feed(d, w, "/op/myNotes?live=1")
}

/// A delegation Ann approves for the matrix's agent, withdrawn while the
/// agent's sync feed is open.
fn withdrawn_delegation_feed(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let ann = &w.people["ann"];
    let t1 = w.var("T1");
    let req = d.send(
        "POST",
        "/auth/delegation/request",
        &[("Content-Type", "application/json")],
        &json!({
            "space": t1, "agent": super::AGENT, "person": ann.member,
            "ops": ["myNotes"], "hours": 2,
        })
        .to_string(),
    );
    assert_eq!(req.status, 201, "delegation request: {}", req.text());
    let req = req.json();
    let request = req["request"].as_str().expect("request").to_string();
    let binder = req["binder"].as_str().expect("binder").to_string();
    let present = actors::step_up(d, &ann.device, &ann.token);
    let approved = actors::op(
        d,
        t1,
        &present,
        "POST",
        "std.delegation.approve",
        &json!({"request": request}),
    );
    assert_eq!(approved.status, 200, "approve: {}", approved.text());
    let redeemed = d.send(
        "POST",
        "/auth/delegation/redeem",
        &[("Content-Type", "application/json")],
        &json!({"request": request, "binder": binder}).to_string(),
    );
    assert_eq!(redeemed.status, 200, "redeem: {}", redeemed.text());
    let redeemed = redeemed.json();
    let token = redeemed["token"].as_str().expect("token").to_string();
    let id = redeemed["delegation"]
        .as_str()
        .expect("delegation")
        .to_string();
    feed_refused_after(d, "/op/myNotes?live=1&sync=1", &token, t1, &|| {
        actors::must(
            d,
            ann,
            "POST",
            "std.delegation.revoke",
            json!({"delegation": id}),
        );
        ann_writes(d, w);
    })
}

/// A session ended by logout while its feed is open, with no write after.
fn logout_feed(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let (token, _) = ann_signs_in(d, w);
    feed_refused_after(d, "/op/myNotes?live=1&sync=1", &token, w.var("T1"), &|| {
        let out = d.auth("/auth/logout", &json!({}), Some(&token));
        assert!(out.status < 300, "logout: {}", out.text());
    })
}

/// The stream id in a `/live` ready frame.
fn stream_id(text: &str) -> Option<String> {
    text.split("\"stream\":\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .map(str::to_string)
}

fn add_read(d: &Deployment, sid: &str, token: &str, space: &str) -> Answer {
    let mut h = feed_headers(token, space);
    h.push(("Content-Type".into(), "application/json".into()));
    h.push(("Idempotency-Key".into(), format!("stream-add-{sid}")));
    d.raw(
        "POST",
        &format!("/live/{sid}"),
        &h,
        br#"{"key":"k1","op":"myNotes","params":{}}"#,
    )
}

/// A member removed while their `/live` stream holds a read.
fn removed_member_live(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let olga = &w.people["olga"];
    let zed = actors::join(d, olga, 20, "Zed", "viewer");
    let mut s = d.open_stream("/live", &feed_headers(&zed.token, &zed.space));
    if s.status != 200 {
        return verdict(false, &format!("/live did not open: {}", s.status));
    }
    s.read_for(STREAM_OPEN);
    let Some(sid) = stream_id(&s.text) else {
        return verdict(false, &format!("no ready frame: {}", s.text));
    };
    let added = add_read(d, &sid, &zed.token, &zed.space);
    if added.status != 200 {
        return verdict(false, &format!("add: {} {}", added.status, added.text()));
    }
    s.read_for(STREAM_OPEN);
    actors::must(
        d,
        olga,
        "POST",
        "std.space.remove",
        json!({"member": zed.member}),
    );
    ann_writes(d, w);
    let after = s.read_for(STREAM_WAIT);
    let ok = after.contains("event: error")
        && !after.contains("event: change")
        && !after.contains("event: invalidate")
        && s.closed;
    verdict(ok, &format!("closed={} sent:\n{after}", s.closed))
}

/// A member's `?live=1` feed hears of another member's write at most as a
/// signal: no row, no title, reaches the feed (the signal is per op by
/// design, `live.rs`).
fn invalidate_carries_no_row(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let ben = w.actor("member_b");
    let token = ben.token.clone().expect("Ben's token");
    let mut s = d.open_stream("/op/myNotes?live=1", &feed_headers(&token, &ben.space));
    if s.status != 200 {
        return verdict(false, &format!("the feed did not open: {}", s.status));
    }
    s.read_for(STREAM_OPEN);
    actors::must(
        d,
        &w.people["ann"],
        "POST",
        "createNote",
        json!({"title": "hidden-from-ben-7f3a", "body": "", "created": actors::stamp()}),
    );
    let after = s.read_for(Duration::from_millis(1500));
    verdict(!after.contains("hidden-from-ben-7f3a"), &after)
}

/// A forged `x-ordo-resume` hash gets the normal load: Ann's rows, and
/// none of Ben's or the other tenant's.
fn forged_resume(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let ann = w.actor("member_a");
    let token = ann.token.clone().expect("Ann's token");
    let mut h = feed_headers(&token, &ann.space);
    h.push((
        "x-ordo-resume".into(),
        "0000000000000000000000000000000000000000000000000000000000000000".into(),
    ));
    let mut s = d.open_stream("/op/myNotes?live=1&sync=1", &h);
    if s.status != 200 {
        return verdict(false, &format!("the feed did not open: {}", s.status));
    }
    let seen = s.read_for(Duration::from_millis(1500));
    let ok = seen.contains(w.var("NOTE_A"))
        && !seen.contains(w.var("NOTE_B"))
        && !seen.contains(w.var("NOTE_T2"));
    verdict(ok, &seen)
}

/// Bob opens `/live` against Alice's space: the stream is his, and adding
/// a read there is refused.
fn live_add_in_foreign_space(d: &Deployment, w: &World, _: &Overlay) -> Answer {
    let bob = w.actor("t2_owner");
    let token = bob.token.clone().expect("Carl's token");
    let space = w.var("T1");
    let mut s = d.open_stream("/live", &feed_headers(&token, space));
    if s.status != 200 {
        return verdict(false, &format!("/live did not open: {}", s.status));
    }
    s.read_for(STREAM_OPEN);
    let Some(sid) = stream_id(&s.text) else {
        return verdict(false, &format!("no ready frame: {}", s.text));
    };
    let added = add_read(d, &sid, &token, space);
    let ok = added.status == 403 && !added.text().contains("Ann's note");
    verdict(ok, &format!("{} {}", added.status, added.text()))
}

/// An upload that declares 2 bytes and sends 10 stores 2.
fn upload_short_length(d: &Deployment, w: &World, o: &Overlay) -> Answer {
    let ann = &w.people["ann"];
    let file = &o["CAT_FILE_UP"];
    let put = d.raw(
        "PUT",
        &format!("/blob/files/plain/{file}"),
        &[
            ("Origin".into(), PAGE.into()),
            ("X-Ordo-Space".into(), ann.space.clone()),
            ("Authorization".into(), format!("Bearer {}", ann.token)),
            ("Content-Type".into(), "text/plain".into()),
            ("Content-Length".into(), "2".into()),
        ],
        b"AAAAAAAAAA",
    );
    let rows = actors::must(d, ann, "GET", "myFiles", json!({}));
    let handle = rows
        .iter()
        .find(|r| r["id"] == json!(file))
        .and_then(|r| r["plain"].as_str())
        .unwrap_or("")
        .to_string();
    let read = d.raw(
        "GET",
        &format!("/blob/{handle}"),
        &[
            ("Origin".into(), PAGE.into()),
            ("X-Ordo-Space".into(), ann.space.clone()),
            ("Authorization".into(), format!("Bearer {}", ann.token)),
        ],
        b"",
    );
    let ok = (200..300).contains(&put.status) && read.status == 200 && read.text() == "AA";
    synthetic(ok, &put)
}

/// Fifty-one items by Ann, then the first page of `allItems`: its `next`
/// cursor. A page holds 50 rows, so 51 rows make a second page.
fn first_cursor(d: &Deployment, w: &World) -> String {
    let ann = &w.people["ann"];
    for i in 0..51 {
        let a = actors::op(
            d,
            &ann.space,
            &ann.token,
            "POST",
            "createItem",
            &json!({"label": format!("page-{i:02}"), "price": "1.00", "rate": "0.0001", "tags": {}}),
        );
        assert_eq!(a.status, 200, "createItem: {}", a.text());
    }
    let page = actors::op(d, &ann.space, &ann.token, "GET", "allItems", &json!({}));
    assert_eq!(page.status, 200, "allItems: {}", page.text());
    page.json()["next"]
        .as_str()
        .unwrap_or_else(|| panic!("allItems made no next cursor: {}", page.text()))
        .to_string()
}

fn page_with(d: &Deployment, w: &World, cursor: &str) -> Answer {
    let ann = &w.people["ann"];
    d.raw(
        "GET",
        "/op/allItems",
        &[
            ("Origin".into(), PAGE.into()),
            ("X-Ordo-Space".into(), ann.space.clone()),
            ("Authorization".into(), format!("Bearer {}", ann.token)),
            ("X-Ordo-Page".into(), cursor.to_string()),
        ],
        b"",
    )
}

/// A cursor Ann was given, shown back to Ann: the next page.
fn cursor_replayed(d: &Deployment, w: &World, _o: &Overlay) -> Answer {
    let page = page_with(d, w, &first_cursor(d, w));
    let rows = page.json()["rows"].as_array().map_or(0, Vec::len);
    synthetic(page.status == 200 && rows >= 1, &page)
}

/// A cursor whose payload was edited and whose MAC was kept.
fn cursor_tampered(d: &Deployment, w: &World, _o: &Overlay) -> Answer {
    let cursor = first_cursor(d, w);
    let (p, m) = cursor.split_once('.').unwrap_or((&cursor, ""));
    let mut chars: Vec<char> = p.chars().collect();
    let mid = chars.len() / 2;
    chars[mid] = if chars[mid] == 'A' { 'B' } else { 'A' };
    let forged = format!("{}.{m}", chars.into_iter().collect::<String>());
    let page = page_with(d, w, &forged);
    let j = page.json();
    let ok = page.status == 400 && j["error"]["code"] == json!("invalid_cursor")
        || page.status == 400 && j["code"] == json!("invalid_cursor");
    synthetic(ok, &page)
}

// ── the world this module adds ──────────────────────────────────────────

fn flip_last(s: &str) -> String {
    let mut c: Vec<char> = s.chars().collect();
    if let Some(l) = c.last_mut() {
        *l = if *l == 'A' { 'B' } else { 'A' };
    }
    c.into_iter().collect()
}

/// A file row on Ann's note holding `bytes`; answers (row id, handle).
fn file_with(d: &Deployment, w: &World, name: &str, mime: &str, bytes: &[u8]) -> (String, String) {
    let ann = &w.people["ann"];
    let rows = actors::must(
        d,
        ann,
        "POST",
        "addFile",
        json!({"note": w.var("NOTE_A"), "name": name}),
    );
    let id = rows[0]["created"]
        .as_str()
        .or_else(|| rows[0]["id"].as_str())
        .expect("a file id")
        .to_string();
    if !bytes.is_empty() {
        let up = actors::upload(
            d,
            &ann.space,
            &ann.token,
            &format!("/blob/files/plain/{id}"),
            mime,
            bytes,
        );
        assert!(
            (200..300).contains(&up.status),
            "upload {name}: {}",
            up.text()
        );
    }
    let rows = actors::must(d, ann, "GET", "myFiles", json!({}));
    let handle = rows
        .iter()
        .find(|r| r["id"] == json!(id))
        .and_then(|r| r["plain"].as_str())
        .unwrap_or("")
        .to_string();
    (id, handle)
}

/// Make what the rows name that the world does not hold.
pub fn overlay(d: &Deployment, w: &World) -> Overlay {
    let mut o = Overlay::new();
    o.insert("DOOR".into(), format!("http://127.0.0.1:{}", d.port));
    o.insert("PLAIN".into(), PLAIN.into());
    o.insert("FOREIGN".into(), FOREIGN.into());
    o.insert("HOME".into(), HOME.into());
    o.insert("PAGE_LOOKALIKE".into(), format!("{PAGE}.evil.test"));
    o.insert("T1_UPPER".into(), w.var("T1").to_ascii_uppercase());
    o.insert(
        "HANDLE_A_PLAIN_UPPER".into(),
        w.var("HANDLE_A_PLAIN").to_ascii_uppercase(),
    );
    // A well-formed note id for a row that does not exist.
    let note = w.var("NOTE_A");
    let ghost = match note.rfind('_') {
        Some(i) => format!("{}_000000000099", &note[..i]),
        None => note.to_string(),
    };
    o.insert("NOTE_GHOST".into(), ghost);

    // Ann's token, in pieces.
    let token = w.actor("member_a").token.clone().unwrap_or_default();
    let (p, m) = token.split_once('.').unwrap_or((&token, ""));
    o.insert("TOKEN_A_PAYLOAD".into(), p.to_string());
    o.insert("TOKEN_A_TAG".into(), m.to_string());
    o.insert("TOKEN_A_TAG_FLIPPED".into(), flip_last(m));
    let mid = p.len() / 2;
    let mut tampered: Vec<char> = p.chars().collect();
    tampered[mid] = if tampered[mid] == 'A' { 'B' } else { 'A' };
    o.insert(
        "TOKEN_A_PAYLOAD_TAMPERED".into(),
        tampered.into_iter().collect(),
    );
    // The expired token with its expiry rewritten to the far future and its
    // original tag kept.
    let expired = w.actor("expired").token.clone().unwrap_or_default();
    let (ep, em) = expired.split_once('.').unwrap_or((&expired, ""));
    let text =
        String::from_utf8_lossy(&ordo_identity::webauthn::b64url_decode(ep).unwrap_or_default())
            .into_owned();
    let mut f: Vec<String> = text.split('|').map(str::to_string).collect();
    if f.len() > 2 {
        f[2] = "9999999999".into();
    }
    o.insert(
        "TOKEN_A_EXP_EXTENDED".into(),
        format!(
            "{}.{em}",
            ordo_identity::webauthn::b64url_encode(f.join("|").as_bytes())
        ),
    );

    // Renewal cookies of fresh sign-ins, one per row that spends one.
    let (_, fresh) = ann_signs_in(d, w);
    o.insert("COOKIE_FRESH".into(), fresh);
    let (_, grace) = ann_signs_in(d, w);
    o.insert("COOKIE_GRACE".into(), grace);

    // Files: active types to be served, and a file row to upload to.
    let (_, h) = file_with(d, w, "c.pdf", "application/pdf", b"%PDF-1.4\n% catalog");
    o.insert("CAT_HANDLE_PDF".into(), h);
    let (_, h) = file_with(
        d,
        w,
        "c.png",
        "image/png",
        b"\x89PNG\r\n\x1a\n<script>alert(1)</script>",
    );
    o.insert("CAT_HANDLE_PNG".into(), h);
    let (_, h) = file_with(d, w, "c.js", "text/javascript", b"alert(document.cookie)");
    o.insert("CAT_HANDLE_JS".into(), h);
    let (id, _) = file_with(d, w, "up.txt", "text/plain", b"");
    o.insert("CAT_FILE_UP".into(), id);
    o
}

// ── the rows ────────────────────────────────────────────────────────────

const A: &str = "member_a";
const X: &str = "t2_owner";
const ANON: &str = "anonymous";

fn note(title: &str) -> Value {
    json!({"title": title, "body": "", "created": "2026-10-25T09:00:00Z"})
}

fn edit(rev: &str) -> String {
    format!(r#"{{"id":"{{NOTE_A}}","rev":{rev},"title":"x"}}"#)
}

fn denied(id: &'static str, what: &'static str, path: &str, actor: &'static str) -> Spec {
    r(id, what, "GET", path, actor, Expect::Denied)
}

const MY: &str = "/op/myNotes";
const SCHEME_REFINE: &str = "/op/myNotes?$refine=";

/// One request per `http` catalog entry the world can express.
pub fn specs() -> Vec<Spec> {
    let t1 = "{SPACE_member_a}";
    let t2 = "{SPACE_t2_owner}";
    let mut v: Vec<Spec> = Vec::new();

    // ── the deployment audit ──
    v.push(
        r(
            "DEPLOY-003",
            "preflight from a foreign page: no credentialed allowance",
            "OPTIONS",
            "/op/x",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .hdr("Origin", FOREIGN)
        .hdr("Access-Control-Request-Method", "POST")
        .check(preflight_not_credentialed),
    );
    v.push(
        r(
            "DEPLOY-004",
            "a foreign page's call without a bearer: 403, no ACAO",
            "POST",
            "/op/x",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .hdr("Origin", FOREIGN)
        .body("{}")
        .check(is_403_without_acao),
    );
    v.push(denied("DEPLOY-005", "/metrics to a stranger", "/metrics", ANON).no_space());
    v.push(
        denied(
            "DEPLOY-006",
            "/health/deep to a stranger",
            "/health/deep",
            ANON,
        )
        .no_space(),
    );
    v.push(denied("DEPLOY-007", "/usage to a stranger", "/usage", ANON).no_space());
    v.push(
        denied(
            "DEPLOY-008",
            "/security/alerts to a stranger",
            "/security/alerts",
            ANON,
        )
        .no_space(),
    );
    v.push(
        denied(
            "DEPLOY-009",
            "/security/events to a stranger",
            "/security/events?space=system",
            ANON,
        )
        .no_space(),
    );
    v.push(
        r(
            "DEPLOY-010",
            "/health/ready to a stranger: the status alone (ruled 2026-10-25)",
            "GET",
            "/health/ready",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .check(health_status_alone),
    );
    v.push(
        r(
            "DEPLOY-011",
            "/health/deploy to a stranger: the status alone",
            "GET",
            "/health/deploy",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .check(health_status_alone),
    );
    v.push(denied("DEPLOY-012", "/live without a bearer", "/live", ANON).no_space());
    v.push(
        r(
            "DEPLOY-014",
            "/auth/spaces to a stranger",
            "GET",
            "/auth/spaces",
            ANON,
            Expect::Unauth,
        )
        .no_space(),
    );

    // ── data, pen test 2 (Kim and Lee) ──
    v.push(denied("FDATA-001", "another tenant's token on a read in T1", MY, X).space(t1));
    v.push(
        r(
            "FDATA-002",
            "edit another tenant's note id from my own tenant",
            "POST",
            "/op/editNote",
            X,
            Expect::Code(404, "not_found"),
        )
        .body(&edit("1")),
    );
    v.push(
        r(
            "FDATA-003",
            "edit with T1 named by a T2 token",
            "POST",
            "/op/editNote",
            X,
            Expect::Denied,
        )
        .space(t1)
        .body(&edit("1")),
    );
    v.push(
        r(
            "FDATA-004",
            "delete another tenant's note, id in the body",
            "DELETE",
            "/op/deleteNote",
            X,
            Expect::Code(400, ""),
        )
        .body(r#"{"id":"{NOTE_A}"}"#)
        .check(four_xx),
    );
    v.push(denied("FDATA-005", "a read of T1 with no bearer", MY, ANON));
    v.push(r(
        "FDATA-006",
        "delete another tenant's note id from my own tenant",
        "DELETE",
        "/op/deleteNote?id={NOTE_A}",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "FDATA-007",
            "delete with T1 named by a T2 token",
            "DELETE",
            "/op/deleteNote?id={NOTE_A}",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(
        r(
            "FDATA-008",
            "quote-and-or in a text parameter is a literal",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(200, ""),
        )
        .json(note("x' OR '1'='1")),
    );
    v.push(
        r(
            "FDATA-009",
            "a dropped table in a text parameter is a literal",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(200, ""),
        )
        .json(note("x'); DROP TABLE notes;--")),
    );
    v.push(
        r(
            "FDATA-010",
            "a double-quote tautology in a text parameter is a literal",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(200, ""),
        )
        .json(note("\" OR 1=1 --")),
    );
    v.push(
        r(
            "FDATA-011",
            "a tautology in an id parameter is refused as a bad id",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .body(r#"{"id":"note_x' OR '1'='1","rev":1,"title":"x"}"#),
    );
    v.push(r(
        "FDATA-012",
        "my own download",
        "GET",
        "/download",
        A,
        Expect::Code(200, ""),
    ));
    v.push(
        r(
            "FDATA-013",
            "a download of T1 named by a T2 token",
            "GET",
            "/download",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(
        r(
            "FDATA-014",
            "an uploaded HTML file is neutralised",
            "GET",
            "/blob/{HANDLE_A_HTML}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "FDATA-015",
            "an uploaded SVG file is neutralised",
            "GET",
            "/blob/{HANDLE_A_SVG}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "FDATA-016",
            "an uploaded PDF is served safely",
            "GET",
            "/blob/{CAT_HANDLE_PDF}",
            A,
            Expect::Code(200, ""),
        )
        .check(shown_safely),
    );
    v.push(
        r(
            "FDATA-017",
            "a PNG with a script in it is served safely",
            "GET",
            "/blob/{CAT_HANDLE_PNG}",
            A,
            Expect::Code(200, ""),
        )
        .check(shown_safely),
    );
    v.push(r(
        "FDATA-018",
        "a handle read from my own tenant",
        "GET",
        "/blob/{HANDLE_A_HTML}",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "FDATA-019",
            "a handle read with T1 named by a T2 token",
            "GET",
            "/blob/{HANDLE_A_HTML}",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(
        r(
            "FDATA-020",
            "a handle read with no credentials",
            "GET",
            "/blob/{HANDLE_A_HTML}",
            ANON,
            Expect::Code(404, "not_found"),
        )
        .no_space(),
    );
    v.push(r(
        "FDATA-021",
        "a made-up handle",
        "GET",
        "/blob/cbb502dbed5eb0",
        A,
        Expect::Code(404, "not_found"),
    ));
    v.push(r(
        "FDATA-022",
        "a handle's meta from my own tenant",
        "GET",
        "/blob/{HANDLE_A_HTML}/meta",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "FDATA-023",
            "attach a file to another tenant's note",
            "POST",
            "/op/addFile",
            X,
            Expect::Code(404, "not_found"),
        )
        .body(r#"{"note":"{NOTE_A}","name":"x"}"#),
    );
    v.push(
        r(
            "FDATA-024",
            "upload to another tenant's file row",
            "PUT",
            "/blob/files/plain/{FILE_A}",
            X,
            Expect::Code(404, "not_found"),
        )
        .hdr("Content-Type", "text/plain")
        .body("x"),
    );
    v.push(
        r(
            "FDATA-025",
            "HTML served as an attachment under the sandbox policy",
            "GET",
            "/blob/{HANDLE_A_HTML}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "FDATA-026",
            "SVG served as an attachment under the sandbox policy",
            "GET",
            "/blob/{HANDLE_A_SVG}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "FDATA-027",
            "PDF served with nosniff and a policy",
            "GET",
            "/blob/{CAT_HANDLE_PDF}",
            A,
            Expect::Code(200, ""),
        )
        .check(shown_safely),
    );
    v.push(
        r(
            "FDATA-028",
            "PNG served with nosniff and a policy",
            "GET",
            "/blob/{CAT_HANDLE_PNG}",
            A,
            Expect::Code(200, ""),
        )
        .check(shown_safely),
    );
    v.push(
        denied(
            "FDATA-035",
            "/metrics with no credentials",
            "/metrics",
            ANON,
        )
        .no_space(),
    );
    v.push(denied(
        "FDATA-036",
        "/metrics with a tenant token",
        "/metrics",
        A,
    ));
    v.push(
        denied(
            "FDATA-037",
            "/security/alerts with no credentials",
            "/security/alerts",
            ANON,
        )
        .no_space(),
    );
    v.push(denied(
        "FDATA-038",
        "/security/alerts with a tenant token",
        "/security/alerts",
        A,
    ));
    v.push(denied(
        "FDATA-039",
        "another tenant's /security/events",
        "/security/events?space={T1}",
        X,
    ));
    v.push(
        denied(
            "FDATA-040",
            "/security/events with no credentials",
            "/security/events?space={T1}",
            ANON,
        )
        .no_space(),
    );
    v.push(r(
        "FDATA-041",
        "my own tenant's /security/events",
        "GET",
        "/security/events?space={T2}",
        X,
        Expect::Code(200, ""),
    ));
    v.push(denied(
        "FDATA-042",
        "another tenant's /usage",
        "/usage?space={T1}&from=0&to=9999999999",
        X,
    ));
    v.push(
        denied(
            "FDATA-043",
            "/usage with no credentials",
            "/usage?space={T1}&from=0&to=9999999999",
            ANON,
        )
        .no_space(),
    );
    v.push(denied(
        "FDATA-044",
        "/usage with a quote in the space",
        "/usage?space=sys'--",
        X,
    ));
    v.push(
        denied(
            "FDATA-045",
            "a live feed of T1 with a T2 token",
            "/op/myNotes?live=1&sync=1",
            X,
        )
        .space(t1),
    );
    v.push(denied(
        "FDATA-046",
        "a live feed with no bearer",
        "/op/myNotes?live=1",
        ANON,
    ));
    v.push(r(
        "FDATA-047",
        "a cut-off refinement is a typed refusal",
        "GET",
        &format!("{SCHEME_REFINE}%7B%22title%22%3A%7B%22eq%22%3A%22x%22"),
        A,
        Expect::Code(400, "invalid_refinement"),
    ));
    for (id, what, enc) in [
        (
            "FDATA-048",
            "a refinement on an unknown field",
            "%7B%22nonexistent%22%3A%7B%22eq%22%3A%22x%22%7D%7D",
        ),
        (
            "FDATA-049",
            "a quote-or refinement value",
            "%7B%22title%22%3A%7B%22eq%22%3A%22x%5Cu0027%20OR%201%3D1%22%7D%7D",
        ),
        (
            "FDATA-050",
            "a greater-than refinement",
            "%7B%22body%22%3A%7B%22gt%22%3A%22a%22%7D%7D",
        ),
        (
            "FDATA-051",
            "an or-refinement",
            "%7B%22%24or%22%3A%5B%7B%22title%22%3A%7B%22eq%22%3A%22x%22%7D%7D%5D%7D",
        ),
    ] {
        v.push(
            r(
                id,
                what,
                "GET",
                &format!("{SCHEME_REFINE}{enc}"),
                A,
                Expect::Code(200, ""),
            )
            .check(only_own_rows),
        );
    }
    v.push(r(
        "FDATA-052",
        "a forged page token",
        "GET",
        "/op/myNotes?$page=forged.page.token",
        A,
        Expect::Code(400, "invalid_cursor"),
    ));
    v.push(
        r(
            "FDATA-053",
            "an idempotency key used first by A",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(200, ""),
        )
        .hdr("Idempotency-Key", "catalog-shared-key")
        .json(note("idem-a"))
        .check(idem_first),
    );
    v.push(
        r(
            "FDATA-053",
            "the same key replayed by another tenant gets its own answer",
            "POST",
            "/op/createNote",
            X,
            Expect::Code(200, ""),
        )
        .hdr("Idempotency-Key", "catalog-shared-key")
        .json(note("idem-x"))
        .check(idem_second),
    );
    v.push(r(
        "FDATA-054",
        "my own download holds my rows only",
        "GET",
        "/download",
        A,
        Expect::Code(200, ""),
    ));
    v.push(
        r(
            "FDATA-055",
            "another tenant's access list",
            "GET",
            "/op/std.notesAccess.list",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(
        r(
            "FDATA-056",
            "revoke a share in T1 with a T2 token",
            "DELETE",
            "/op/revokeNotesAccess?note={NOTE_A}&member={MEMBER_BEN}",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(
        r(
            "FDATA-057",
            "grant admin in T1 with a T2 token",
            "POST",
            "/op/grantNotesAccess",
            X,
            Expect::Denied,
        )
        .space(t1)
        .body(r#"{"note":"{NOTE_A}","level":"admin"}"#),
    );
    for (id, link) in [
        ("FDATA-058", "lnk_deadbeef"),
        ("FDATA-059", "../etc"),
        ("FDATA-060", "00000000000000000000"),
        ("FDATA-061", ""),
    ] {
        v.push(
            r(
                id,
                "a made-up link is refused the same way as any other",
                "POST",
                "/auth/link",
                ANON,
                Expect::Code(403, "link_refused"),
            )
            .no_space()
            .json(json!({ "link": link })),
        );
    }

    // ── identity, Fable ──
    v.push(r(
        "FID-TOKEN-001",
        "a live bearer renews",
        "POST",
        "/auth/refresh",
        A,
        Expect::Code(200, ""),
    ));
    v.push(
        r(
            "FID-TOKEN-002",
            "a real payload with a made-up tag",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Unauth,
        )
        .hdr(
            "Authorization",
            "Bearer {TOKEN_A_PAYLOAD}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ),
    );
    v.push(
        r(
            "FID-CSRF-001",
            "renewal from a foreign Origin leaves the cookie whole",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, "remember_off"),
        )
        .hdr("Origin", FOREIGN)
        .hdr("Cookie", "{COOKIE_OLGA}"),
    );
    v.push(
        r(
            "FID-CSRF-002",
            "renewal with no Origin is refused",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, ""),
        )
        .hdr("Origin", "")
        .hdr("Cookie", "{COOKIE_OLGA}"),
    );
    v.push(
        r(
            "FID-SESSION-001",
            "a fresh cookie from the app's page renews and rotates",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Cookie", "{COOKIE_FRESH}")
        .check(rotated_cookie),
    );
    v.push(
        r(
            "FID-SESSION-002",
            "a cookie spent once is honoured within the grace",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Cookie", "{COOKIE_GRACE}"),
    );
    v.push(
        r(
            "FID-SESSION-002",
            "and again within the grace",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Cookie", "{COOKIE_GRACE}"),
    );
    v.push(
        r(
            "FID-SESSION-003",
            "a spent cookie replayed past the grace is refused",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, ""),
        )
        .hdr("Cookie", "{COOKIE_SPENT}"),
    );
    v.push(denied(
        "FID-SESSION-004",
        "a logged-out token on a read",
        MY,
        "logged_out",
    ));
    v.push(r(
        "FID-SESSION-005",
        "a logged-out token renewing",
        "POST",
        "/auth/refresh",
        "logged_out",
        Expect::Unauth,
    ));
    v.push(script(
        "FID-SESSION-006",
        "a cookie whose family a logout ended",
        logged_out_cookie,
        Expect::Code(401, ""),
    ));
    v.push(denied("FID-BOLA-001", "A's token naming B's tenant", MY, X).space(t1));
    v.push(
        denied(
            "FID-BOLA-002",
            "A reads B's secret note: never",
            "/op/noteById?id={NOTE_T2}",
            X,
        )
        .space(t1),
    );
    v.push(
        r(
            "FID-AUTHN-003",
            "an op that does not exist is denied like any other",
            "POST",
            "/op/addNote",
            A,
            Expect::Denied,
        )
        .body("{}"),
    );
    v.push(
        r(
            "FID-HEADERS-001",
            "a JSON route carries the hardening headers",
            "GET",
            "/auth/methods",
            ANON,
            Expect::Code(200, ""),
        )
        .check(hardened),
    );
    v.push(script(
        "FID-COOKIE-001",
        "the renewal cookie's attributes on http",
        cookie_flags,
        Expect::Code(200, ""),
    ));

    // ── data, pen test 1 (Alice and Bob) ──
    v.push(denied("DATA-001", "B's token reads A's tenant", MY, X).space(t1));
    v.push(
        r(
            "DATA-002",
            "B edits A's note in A's tenant",
            "POST",
            "/op/editNote",
            X,
            Expect::Denied,
        )
        .space(t1)
        .body(&edit("1")),
    );
    v.push(
        r(
            "DATA-003",
            "B deletes A's note in A's tenant",
            "DELETE",
            "/op/deleteNote",
            X,
            Expect::Denied,
        )
        .space(t1)
        .body(r#"{"id":"{NOTE_A}"}"#),
    );
    v.push(
        r(
            "DATA-004",
            "A's note id used in B's own tenant",
            "POST",
            "/op/editNote",
            X,
            Expect::Code(404, "not_found"),
        )
        .body(&edit("1")),
    );
    v.push(r(
        "DATA-005",
        "A's note id read in B's tenant: no rows",
        "GET",
        "/op/std.notes.get?id={NOTE_A}",
        X,
        Expect::Rows(0),
    ));
    v.push(
        r(
            "DATA-006",
            "A's note read in A's tenant with B's token",
            "GET",
            "/op/std.notes.get?id={NOTE_A}",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(r(
        "DATA-007",
        "A's handle in B's tenant is as a missing one",
        "GET",
        "/blob/{HANDLE_A_PLAIN}",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "DATA-008",
            "A's handle in A's tenant with B's token",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            X,
            Expect::Denied,
        )
        .space(t1),
    );
    v.push(r(
        "DATA-009",
        "A's handle's meta in B's tenant",
        "GET",
        "/blob/{HANDLE_A_PLAIN}/meta",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "DATA-010",
            "a handle with no credentials",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            ANON,
            Expect::Code(404, "not_found"),
        )
        .no_space(),
    );
    v.push(denied(
        "DATA-011",
        "a handle with A's space and no token",
        "/blob/{HANDLE_A_PLAIN}",
        ANON,
    ));
    v.push(r(
        "DATA-012",
        "the all-zero handle",
        "GET",
        "/blob/00000000000000000000000000000000",
        X,
        Expect::Code(404, "not_found"),
    ));
    v.push(
        r(
            "DATA-013",
            "HTML upload is neutralised",
            "GET",
            "/blob/{HANDLE_A_HTML}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "DATA-014",
            "SVG upload is neutralised",
            "GET",
            "/blob/{HANDLE_A_SVG}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "DATA-015",
            "a JavaScript upload is neutralised",
            "GET",
            "/blob/{CAT_HANDLE_JS}",
            A,
            Expect::Code(200, ""),
        )
        .check(neutralised),
    );
    v.push(
        r(
            "DATA-016",
            "a range past the end",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            A,
            Expect::Code(400, ""),
        )
        .hdr("Range", "bytes=100-200"),
    );
    v.push(
        r(
            "DATA-017",
            "a range starting at the end",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            A,
            Expect::Code(400, ""),
        )
        .hdr("Range", "bytes=16-"),
    );
    v.push(
        r(
            "DATA-018",
            "a suffix range reads the whole object, no more",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            A,
            Expect::Code(200, ""),
        )
        .hdr("Range", "bytes=-999999")
        .check(full_plain),
    );
    v.push(
        r(
            "DATA-019",
            "a backwards range reads the whole object",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            A,
            Expect::Code(200, ""),
        )
        .hdr("Range", "bytes=10-2")
        .check(full_plain),
    );
    v.push(
        r(
            "DATA-020",
            "two ranges serve the first only",
            "GET",
            "/blob/{HANDLE_A_PLAIN}",
            A,
            Expect::Code(200, ""),
        )
        .hdr("Range", "bytes=0-2,5-7")
        .check(range_first_or_full),
    );
    v.push(
        r(
            "DATA-021",
            "an upload declaring a huge length is refused unread",
            "PUT",
            "/blob/files/plain/{CAT_FILE_UP}",
            A,
            Expect::Code(400, ""),
        )
        .hdr("Content-Type", "text/plain")
        .hdr("Content-Length", "999999999999")
        .body("A"),
    );
    v.push(
        r(
            "DATA-022",
            "a chunked upload with no length is refused",
            "PUT",
            "/blob/files/plain/{CAT_FILE_UP}",
            A,
            Expect::Code(400, ""),
        )
        .hdr("Content-Type", "text/plain")
        .hdr("Transfer-Encoding", "chunked")
        .body("3\r\nABC\r\n0\r\n\r\n"),
    );
    v.push(script(
        "DATA-023",
        "an upload stores exactly the declared length",
        upload_short_length,
        Expect::Code(200, ""),
    ));
    v.push(r(
        "DATA-024",
        "a traversal in a handle",
        "GET",
        "/blob/..%2f..%2fetc%2fpasswd",
        A,
        Expect::Code(404, ""),
    ));
    v.push(r(
        "DATA-025",
        "a traversal in an op name is an op name",
        "GET",
        "/op/..%2f..%2fhealth%2fdeploy",
        A,
        Expect::Denied,
    ));
    v.push(
        r(
            "DATA-026",
            "an upload to a text field",
            "PUT",
            "/blob/files/name/{CAT_FILE_UP}",
            A,
            Expect::Code(404, ""),
        )
        .hdr("Content-Type", "text/plain")
        .body("x"),
    );
    v.push(
        r(
            "DATA-027",
            "B's note id used in A's tenant",
            "POST",
            "/op/addFile",
            A,
            Expect::Code(404, "not_found"),
        )
        .body(r#"{"note":"{NOTE_T2}","name":"x"}"#),
    );
    v.push(
        r(
            "DATA-028",
            "an upper-case handle is the same object",
            "GET",
            "/blob/{HANDLE_A_PLAIN_UPPER}",
            A,
            Expect::Code(200, ""),
        )
        .check(full_plain),
    );
    v.push(
        r(
            "DATA-029",
            "a made-up link secret",
            "POST",
            "/auth/link",
            ANON,
            Expect::Code(403, "link_refused"),
        )
        .no_space()
        .body(r#"{"link":"lks_00000000000000000000"}"#),
    );
    v.push(
        r(
            "DATA-030",
            "a live link opens",
            "POST",
            "/auth/link",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .body(r#"{"link":"{LINK_OK}"}"#)
        .check(has_token),
    );
    v.push(
        r(
            "DATA-031",
            "a password link without its password",
            "POST",
            "/auth/link",
            ANON,
            Expect::Code(401, "link_password_required"),
        )
        .no_space()
        .body(r#"{"link":"{LINK_PWD}"}"#),
    );
    v.push(
        r(
            "DATA-033",
            "a link guest sees the linked note only",
            "GET",
            MY,
            "link",
            Expect::Code(200, ""),
        )
        .check(only_linked_note),
    );
    v.push(r(
        "DATA-034",
        "a link guest asks for a note outside the link",
        "GET",
        "/op/std.notes.get?id={NOTE_A}",
        "link",
        Expect::Nothing,
    ));
    v.push(denied(
        "DATA-035",
        "a link guest calls an op outside the link",
        "/op/myFiles",
        "link",
    ));
    v.push(
        r(
            "DATA-036",
            "a link guest writes",
            "POST",
            "/op/createNote",
            "link",
            Expect::Denied,
        )
        .json(note("guest")),
    );
    v.push(denied(
        "DATA-037",
        "a link guest downloads",
        "/download",
        "link",
    ));
    v.push(
        denied(
            "DATA-039",
            "a live feed of T1 with a T2 token",
            "/op/myNotes?live=1&sync=1",
            X,
        )
        .space(t1),
    );
    v.push(
        r(
            "DATA-041",
            "a guessed stream id",
            "POST",
            "/live/ls_00000000000000000000000000000000",
            X,
            Expect::Code(404, "not_found"),
        )
        .body(r#"{"key":"k3","op":"myNotes"}"#),
    );
    v.push(
        denied(
            "DATA-042",
            "a tenant token naming the deployment's space",
            MY,
            X,
        )
        .space("{HOME}"),
    );
    v.push(denied(
        "DATA-043",
        "a tenant token on /metrics",
        "/metrics",
        A,
    ));
    v.push(r(
        "DATA-044",
        "a quote-or in an id parameter",
        "GET",
        "/op/noteById?id=x%27%20OR%201%3D1--",
        A,
        Expect::Code(400, "invalid_param"),
    ));
    v.push(
        r(
            "DATA-045",
            "a body that is not JSON",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, ""),
        )
        .body("{bad json"),
    );

    // ── identity, pen test 3 ──
    v.push(denied("ID-001", "a flipped tag", MY, ANON).hdr(
        "Authorization",
        "Bearer {TOKEN_A_PAYLOAD}.{TOKEN_A_TAG_FLIPPED}",
    ));
    v.push(
        denied("ID-002", "a payload with no tag", MY, ANON)
            .hdr("Authorization", "Bearer {TOKEN_A_PAYLOAD}"),
    );
    v.push(
        denied("ID-003", "a payload with an empty tag", MY, ANON)
            .hdr("Authorization", "Bearer {TOKEN_A_PAYLOAD}."),
    );
    v.push(
        denied("ID-004", "an edited payload under the old tag", MY, ANON).hdr(
            "Authorization",
            "Bearer {TOKEN_A_PAYLOAD_TAMPERED}.{TOKEN_A_TAG}",
        ),
    );
    v.push(
        denied(
            "ID-005",
            "an expired token with its expiry rewritten",
            MY,
            ANON,
        )
        .hdr("Authorization", "Bearer {TOKEN_A_EXP_EXTENDED}"),
    );
    v.push(
        denied("ID-006", "a bearer that is not a token", MY, ANON)
            .hdr("Authorization", "Bearer not.a.token"),
    );
    v.push(denied("ID-007", "no bearer", MY, ANON));
    v.push(denied("ID-008", "A's token naming B's tenant", MY, A).space(t2));
    v.push(
        denied(
            "ID-009",
            "A's token naming a tenant that does not exist",
            MY,
            A,
        )
        .space("spc_00000000000000000000000000"),
    );
    v.push(denied("ID-010", "A's token naming the deployment's space", MY, A).space("{HOME}"));
    v.push(
        r(
            "ID-011",
            "no space header",
            "GET",
            MY,
            A,
            Expect::Code(400, "missing_space"),
        )
        .no_space(),
    );
    v.push(
        r(
            "ID-017",
            "renewal from a foreign page",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, "remember_off"),
        )
        .hdr("Origin", FOREIGN)
        .hdr("Cookie", "{COOKIE_OLGA}"),
    );
    v.push(
        r(
            "ID-018",
            "renewal from the door's own origin",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, "remember_off"),
        )
        .hdr("Origin", "{DOOR}")
        .hdr("Cookie", "{COOKIE_OLGA}"),
    );
    v.push(
        r(
            "ID-019",
            "renewal with no Origin",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, "remember_off"),
        )
        .hdr("Origin", "")
        .hdr("Cookie", "{COOKIE_OLGA}"),
    );
    v.push(
        r(
            "ID-020",
            "an old cookie replayed past the grace ends the family",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(401, ""),
        )
        .hdr("Cookie", "{COOKIE_SPENT}"),
    );
    v.push(
        r(
            "ID-021",
            "a double-submit inside the grace is honoured",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Cookie", "{COOKIE_GRACE}"),
    );
    v.push(script(
        "ID-022",
        "a logout ends the family and clears the cookie",
        logout_ends_family,
        Expect::Code(204, ""),
    ));
    v.push(
        r(
            "ID-023",
            "renewal with nothing",
            "POST",
            "/auth/refresh",
            ANON,
            Expect::Unauth,
        )
        .no_space(),
    );
    v.push(
        r(
            "ID-024",
            "a preflight from a foreign page for a members' op",
            "OPTIONS",
            MY,
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Origin", FOREIGN)
        .hdr("Access-Control-Request-Method", "GET")
        .hdr(
            "Access-Control-Request-Headers",
            "authorization,x-ordo-space",
        )
        .check(is_403_without_acao),
    );
    v.push(
        r(
            "ID-025",
            "a look-alike origin is refused its preflight",
            "OPTIONS",
            MY,
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Origin", "{PAGE_LOOKALIKE}")
        .hdr("Access-Control-Request-Method", "GET")
        .hdr(
            "Access-Control-Request-Headers",
            "authorization,x-ordo-space",
        )
        .check(is_403_without_acao),
    );
    v.push(
        r(
            "ID-026",
            "a look-alike origin is handed no ACAO",
            "GET",
            MY,
            A,
            Expect::Code(200, ""),
        )
        .hdr("Origin", "{PAGE_LOOKALIKE}")
        .check(ok_without_acao),
    );
    v.push(
        r(
            "ID-027",
            "a foreign preflight is given no ACAO",
            "OPTIONS",
            MY,
            ANON,
            Expect::Code(200, ""),
        )
        .hdr("Origin", FOREIGN)
        .hdr("Access-Control-Request-Method", "GET")
        .check(preflight_not_credentialed),
    );
    v.push(
        r(
            "ID-028",
            "a quote-or in a sign-in identifier",
            "POST",
            "/auth/begin",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .body(r#"{"method":"email","identifier":"a' OR '1'='1@x.org"}"#),
    );
    v.push(
        r(
            "ID-029",
            "a 5 KB sign-in identifier",
            "POST",
            "/auth/begin",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .json(json!({"method":"email","identifier": format!("{}@x.org", "a".repeat(5000))})),
    );
    v.push(
        r(
            "ID-030",
            "a sign-in body that is not JSON",
            "POST",
            "/auth/begin",
            ANON,
            Expect::Code(400, ""),
        )
        .no_space()
        .body("not json{"),
    );
    v.push(
        r(
            "ID-031",
            "a sign-in method that does not exist",
            "POST",
            "/auth/begin",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .body(r#"{"method":"telepathy"}"#),
    );
    v.push(
        r(
            "ID-032",
            "step-up with nothing",
            "POST",
            "/auth/step-up/begin",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .body("{}"),
    );
    v.push(
        r(
            "ID-033",
            "account delete with nothing",
            "POST",
            "/auth/account/delete",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .body("{}"),
    );
    v.push(
        r(
            "ID-034",
            "a forged delegation redeem",
            "POST",
            "/auth/delegation/redeem",
            ANON,
            Expect::Code(400, "invalid_request"),
        )
        .no_space()
        .body(r#"{"token":"forged"}"#),
    );
    v.push(
        r(
            "ID-035",
            "recovery for a stranger",
            "POST",
            "/auth/recover/begin",
            ANON,
            Expect::Unauth,
        )
        .no_space()
        .body(r#"{"identifier":"x@x.org"}"#),
    );
    v.push(
        r(
            "ID-036",
            "a cross-site logout is a no-op",
            "POST",
            "/auth/logout",
            ANON,
            Expect::Code(204, ""),
        )
        .no_space(),
    );
    v.push(
        r(
            "ID-037",
            "a member without a fresh touch shares",
            "POST",
            "/op/std.space.share",
            "manager_stale",
            Expect::Denied,
        )
        .body(r#"{"ops":"myNotes","to":"x@x.org"}"#),
    );
    v.push(denied(
        "ID-038",
        "a tenant member reads the deployment's settings",
        "/op/std.deployment.settings",
        A,
    ));
    v.push(
        r(
            "ID-039",
            "the health route carries the hardening headers",
            "GET",
            "/health/live",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .check(hardened),
    );

    // ── injection ──
    for (id, what, t) in [
        (
            "INJ-SESSION-001",
            "a quote-or in text is a literal",
            "' OR '1'='1",
        ),
        (
            "INJ-SESSION-002",
            "a dropped table in text is a literal",
            "x'; DROP TABLE items;--",
        ),
        (
            "INJ-SESSION-003",
            "a union in text is a literal",
            "x' UNION SELECT null--",
        ),
        ("INJ-SESSION-004", "a comment in text is a literal", "x'--"),
        (
            "INJ-SESSION-005",
            "an escaped quote in text is a literal",
            "x\\' OR 1=1--",
        ),
        (
            "INJ-SESSION-006",
            "a like-wildcard in text is a literal",
            "100%_x",
        ),
        (
            "INJ-SESSION-007",
            "an apostrophe is stored verbatim",
            "O'Reilly",
        ),
        (
            "INJ-SESSION-014",
            "a newline and a comment in text",
            "a\nb;--",
        ),
        (
            "INJ-SESSION-015",
            "a combining character in text",
            "e\u{301}",
        ),
        (
            "INJ-SESSION-029",
            "Bobby Tables is stored as text",
            "Robert'); DROP TABLE items;--",
        ),
    ] {
        v.push(r(id, what, "POST", "/op/createNote", A, Expect::Code(200, "")).json(note(t)));
    }
    v.push(
        r(
            "INJ-SESSION-008",
            "a regex bound as a parameter matches",
            "GET",
            "/op/notesMatching?p=%5EAnn",
            A,
            Expect::Code(200, ""),
        )
        .check(some_rows),
    );
    v.push(r(
        "INJ-SESSION-009",
        "a broken regex is the caller's fault, typed",
        "GET",
        "/op/notesMatching?p=.*)%7C(select",
        A,
        Expect::Code(400, "invalid_param"),
    ));
    v.push(r(
        "INJ-SESSION-010",
        "a nested-quantifier regex runs in bounded time",
        "GET",
        "/op/notesMatching?p=(a%2B)%2B%24",
        A,
        Expect::Code(200, ""),
    ));
    v.push(r(
        "INJ-SESSION-011",
        "an alternation regex runs in bounded time",
        "GET",
        &format!("/op/notesMatching?p={}b", "(a%7Caa)".repeat(40)),
        A,
        Expect::Code(200, ""),
    ));
    v.push(r(
        "INJ-SESSION-012",
        "a huge repetition count is the caller's fault, typed",
        "GET",
        "/op/notesMatching?p=a%7B1000000000%7D",
        A,
        Expect::Code(400, "invalid_param"),
    ));
    v.push(
        r(
            "INJ-SESSION-013",
            "a NUL in text is refused",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(note("a\u{0}b")),
    );
    v.push(r(
        "INJ-SESSION-016",
        "a 500 KB query string is refused",
        "GET",
        &format!("/op/noteById?id={}", "A".repeat(500_000)),
        A,
        Expect::Refused,
    ));
    v.push(
        r(
            "INJ-SESSION-017",
            "SQL in an integer parameter",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .body(&edit(r#""5 OR 1=1""#)),
    );
    v.push(
        r(
            "INJ-SESSION-018",
            "an integer past 64 bits",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .body(&edit("99999999999999999999")),
    );
    v.push(
        r(
            "INJ-SESSION-019",
            "a negative integer is an integer",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(200, ""),
        )
        .body(&edit("-1"))
        .check(not_param_refusal_nor_fault),
    );
    v.push(
        r(
            "INJ-SESSION-022",
            "an array where text is declared",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":["a"],"body":"","created":"2026-10-25T09:00:00Z"})),
    );
    v.push(
        r(
            "INJ-SESSION-023",
            "an operator object where text is declared",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":{"$gt":""},"body":"","created":"2026-10-25T09:00:00Z"})),
    );
    v.push(
        r(
            "INJ-SESSION-024",
            "a number where text is declared",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":123,"body":"","created":"2026-10-25T09:00:00Z"})),
    );
    v.push(
        r(
            "INJ-SESSION-025",
            "an array where an integer is declared",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .body(&edit("[1]")),
    );
    v.push(
        r(
            "INJ-SESSION-026",
            "a fraction where an integer is declared",
            "POST",
            "/op/editNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .body(&edit("1.5")),
    );
    v.push(
        r(
            "INJ-SESSION-031",
            "300 KB of text rides the body cap",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(200, ""),
        )
        .json(note(&"A".repeat(300_000))),
    );
    v.push(
        r(
            "INJ-SESSION-038",
            "SQL in a date",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":"x","body":"","created":"2026-10-01'--"})),
    );
    v.push(
        r(
            "INJ-SESSION-039",
            "a date that is not a date",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":"x","body":"","created":"2026-02-31T00:00:00Z"})),
    );
    v.push(
        r(
            "INJ-SESSION-040",
            "SQL after a datetime",
            "POST",
            "/op/createNote",
            A,
            Expect::Code(400, "invalid_param"),
        )
        .json(json!({"title":"x","body":"","created":"2026-10-01T00:00:00Z'--"})),
    );
    v.push(r(
        "INJ-SESSION-042",
        "SQL in an id",
        "GET",
        "/op/noteById?id=item_x%27%20OR%201%3D1--",
        A,
        Expect::Code(400, "invalid_param"),
    ));
    v.push(
        r(
            "INJ-SESSION-043",
            "a well-formed id of no row: no rows",
            "GET",
            "/op/noteById?id={NOTE_GHOST}",
            A,
            Expect::Code(200, ""),
        )
        .check(no_rows),
    );
    v.push(r(
        "INJ-SESSION-044",
        "a caller supplied for the platform",
        "GET",
        "/op/myNotes?$caller=mem_admin",
        A,
        Expect::Code(403, "caller_supplied"),
    ));
    v.push(r(
        "INJ-SESSION-045",
        "an unknown parameter",
        "GET",
        "/op/myNotes?principal=mem_admin",
        A,
        Expect::Code(400, "unknown_param"),
    ));
    v.push(r(
        "INJ-SESSION-046",
        "a dot-dot in the op path",
        "GET",
        "/op/../op/myNotes",
        A,
        Expect::Code(404, ""),
    ));
    v.push(denied(
        "INJ-SESSION-047",
        "an encoded dot-dot op name",
        "/op/%2e%2e%2fmyNotes",
        A,
    ));
    v.push(denied(
        "INJ-SESSION-048",
        "a NUL in the op name",
        "/op/myNotes%00",
        A,
    ));
    v.push(denied(
        "INJ-SESSION-049",
        "an op name in the wrong case",
        "/op/MYNOTES",
        A,
    ));
    v.push(denied(
        "INJ-SESSION-050",
        "an op that does not exist",
        "/op/noSuchOp",
        A,
    ));
    v.push(denied("INJ-SESSION-051", "a space the caller is not in", MY, A).space("other"));
    v.push(
        r(
            "INJ-SESSION-052",
            "SQL in the space header",
            "GET",
            MY,
            A,
            Expect::Code(400, "missing_space"),
        )
        .space("dev'--"),
    );
    v.push(
        r(
            "INJ-SESSION-053",
            "a 500-character space header",
            "GET",
            MY,
            A,
            Expect::Code(400, "missing_space"),
        )
        .space(&"a".repeat(500)),
    );
    v.push(
        r(
            "INJ-SESSION-054",
            "a space smuggled in the body of a read",
            "POST",
            MY,
            A,
            Expect::Code(405, "method_mismatch"),
        )
        .body(r#"{"text":"x","$space":"other"}"#),
    );
    v.push(
        r(
            "INJ-SESSION-055",
            "no space header",
            "GET",
            MY,
            A,
            Expect::Code(400, "missing_space"),
        )
        .no_space(),
    );
    v.push(denied("INJ-SESSION-056", "no credentials", MY, ANON));
    v.push(denied("INJ-SESSION-057", "the space in the wrong case", MY, A).space("{T1_UPPER}"));
    for (id, what, enc) in [
        ("INJ-SESSION-058", "SQL in a refinement field", "%7B%22where%22%3A%5B%7B%22field%22%3A%22rev%29%20OR%20%281%3D1--%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A1%7D%5D%7D"),
        ("INJ-SESSION-059", "a refinement field that is not there", "%7B%22where%22%3A%5B%7B%22field%22%3A%22nonesuch%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A1%7D%5D%7D"),
        ("INJ-SESSION-061", "SQL in a refinement value", "%7B%22where%22%3A%5B%7B%22field%22%3A%22rev%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A%225%20OR%201%3D1%22%7D%5D%7D"),
        ("INJ-SESSION-062", "SQL in a picked field", "%7B%22pick%22%3A%5B%22rev%29%20%2C%20%28select%22%5D%7D"),
        ("INJ-SESSION-065", "a cut-off refinement", "%7B"),
    ] {
        v.push(r(id, what, "GET", &format!("{SCHEME_REFINE}{enc}"), A, Expect::Code(400, "invalid_refinement")));
    }
    let many: Vec<String> = (0..20_000).map(|i| i.to_string()).collect();
    v.push(r(
        "INJ-SESSION-063",
        "a 20,000-value in-list is refused",
        "GET",
        &format!("{SCHEME_REFINE}%7B%22where%22%3A%5B%7B%22field%22%3A%22rev%22%2C%22op%22%3A%22in%22%2C%22value%22%3A%5B{}%5D%7D%5D%7D", many.join("%2C")),
        A,
        Expect::Refused,
    ));
    let conds: Vec<String> = (0..5000)
        .map(|_| {
            "%7B%22field%22%3A%22rev%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A1%7D".to_string()
        })
        .collect();
    v.push(r(
        "INJ-SESSION-064",
        "5,000 conditions are refused",
        "GET",
        &format!(
            "{SCHEME_REFINE}%7B%22where%22%3A%5B{}%5D%7D",
            conds.join("%2C")
        ),
        A,
        Expect::Refused,
    ));
    v.push(
        r(
            "INJ-SESSION-067",
            "a forged page header",
            "GET",
            MY,
            A,
            Expect::Code(400, "invalid_cursor"),
        )
        .hdr("X-Ordo-Page", "AAAA.BBBB"),
    );
    v.push(
        r(
            "INJ-SESSION-074",
            "a write verb on a read op",
            "POST",
            MY,
            A,
            Expect::Code(405, "method_mismatch"),
        )
        .body(r#"{"text":"x"}"#),
    );

    // ── typed fields: money, decimal, json, a bounded take, a page ──
    let item = |price: Value, rate: Value, tags: Value| json!({"label": "i", "price": price, "rate": rate, "tags": tags});
    let create = |id: &'static str, what: &'static str, body: Value, e: Expect| {
        r(id, what, "POST", "/op/createItem", A, e).json(body)
    };
    let bad = Expect::Code(400, "invalid_param");
    v.push(r(
        "INJ-SESSION-020",
        "a take bound of 999999 is over the declared bound",
        "GET",
        "/op/topN?n=999999",
        A,
        Expect::Code(400, "bound_violation"),
    ));
    v.push(r(
        "INJ-SESSION-021",
        "a negative take is under the declared bound",
        "GET",
        "/op/topN?n=-1",
        A,
        Expect::Code(400, "bound_violation"),
    ));
    v.push(create(
        "INJ-SESSION-027",
        "money sent as a float",
        item(json!(1.0), json!("0.0001"), json!({})),
        bad,
    ));
    v.push(create(
        "INJ-SESSION-028",
        "money sent as an array",
        item(json!(["1.00"]), json!("0.0001"), json!({})),
        bad,
    ));
    v.push(create(
        "INJ-SESSION-030",
        "a bare string is a valid document",
        item(json!("1.00"), json!("0.0001"), json!("x")),
        Expect::Code(200, ""),
    ));
    v.push(create(
        "INJ-SESSION-032",
        "an operator object is stored as data",
        item(json!("1.00"), json!("0.0001"), json!({"$ne": null})),
        Expect::Code(200, ""),
    ));
    v.push(create(
        "INJ-SESSION-033",
        "a NUL in a document is refused",
        item(json!("1.00"), json!("0.0001"), json!(["a\u{0}"])),
        bad,
    ));
    let mut deep = json!(1);
    for _ in 0..400 {
        deep = json!([deep]);
    }
    v.push(create(
        "INJ-SESSION-034",
        "a document 400 levels deep is refused",
        item(json!("1.00"), json!("0.0001"), deep),
        Expect::Code(400, ""),
    ));
    let huge: Vec<u32> = (0..100_000).collect();
    v.push(create(
        "INJ-SESSION-035",
        "a document over the json byte cap is refused",
        item(json!("1.00"), json!("0.0001"), json!(huge)),
        Expect::Code(400, ""),
    ));
    v.push(r(
        "INJ-SESSION-036",
        "SQL in a money string",
        "GET",
        "/op/priceAtMost?cap=12.50%27%20OR%201%3D1--",
        A,
        bad,
    ));
    v.push(r(
        "INJ-SESSION-037",
        "money over its declared places",
        "GET",
        "/op/priceAtMost?cap=12.509",
        A,
        bad,
    ));
    v.push(create(
        "INJ-SESSION-041",
        "SQL in a decimal",
        item(json!("1.00"), json!("0.0001'--"), json!({})),
        bad,
    ));
    v.push(r(
        "INJ-SESSION-060",
        "a refinement on a json field",
        "GET",
        &format!(
            "/op/allItems?$refine={}",
            "%7B%22where%22%3A%5B%7B%22field%22%3A%22tags%22%2C%22op%22%3A%22eq%22%2C%22value%22%3A%5B%5D%7D%5D%7D"
        ),
        A,
        Expect::Code(400, "invalid_refinement"),
    ));
    v.push(script(
        "INJ-SESSION-066",
        "a cursor replayed by the same person serves the next page",
        cursor_replayed,
        Expect::Code(200, ""),
    ));
    v.push(script(
        "INJ-SESSION-068",
        "a cursor with an edited payload and its old MAC is refused",
        cursor_tampered,
        Expect::Code(200, ""),
    ));

    // ── sync ──
    v.push(denied(
        "SYNC-004",
        "a revoked session opens a feed",
        "/op/myNotes?live=1&sync=1",
        "revoked",
    ));
    v.push(r(
        "SYNC-009",
        "a page token on a feed",
        "GET",
        "/op/myNotes?live=1&sync=1&$page=x",
        A,
        Expect::Code(400, "invalid_cursor"),
    ));
    v.push(
        r(
            "SYNC-010",
            "a new device on a link that has used its opens",
            "POST",
            "/auth/link",
            ANON,
            Expect::Code(403, "link_refused"),
        )
        .no_space()
        .body(r#"{"link":"{LINK_ONCE}","device":"attacker-device"}"#),
    );

    // ── emailed codes and the outside provider ──
    v.push(
        r(
            "FID-OAUTH-001",
            "an outside sign-in begun with the exact return origin redirects to the provider",
            "POST",
            "/auth/begin",
            ANON,
            Expect::Code(200, ""),
        )
        .no_space()
        .body(r#"{"method":"discord","return_to":"http://localhost:4981/"}"#)
        .check(outside_redirect_to_provider),
    );
    v.push(script(
        "FID-OAUTH-002",
        "eight look-alike return addresses are refused with no redirect",
        outside_return_bypasses,
        Expect::Unauth,
    ));
    v.push(script(
        "FID-AUTHN-001",
        "an emailed-code begin answers alike for a known and an unknown address",
        email_enumeration_a,
        Expect::Code(200, ""),
    ));
    v.push(script(
        "ID-015",
        "an emailed-code begin answers alike for an account and a fresh address",
        email_enumeration_b,
        Expect::Code(200, ""),
    ));
    v.push(script(
        "FID-AUTHN-002",
        "seven wrong emailed codes, then the real one: the one refusal",
        email_code_brute_force,
        Expect::Unauth,
    ));
    v.push(script(
        "FID-AUTHN-004",
        "another ceremony's binder does not complete an emailed code",
        email_binder_swap,
        Expect::Unauth,
    ));
    v.push(script(
        "FID-EMAIL-001",
        "a first contact address is added without a touch",
        contact_first_needs_no_touch,
        Expect::Code(200, ""),
    ));
    v.push(script(
        "FID-EMAIL-002",
        "changing a contact address without a touch is refused",
        contact_change_needs_touch,
        Expect::Code(403, "presence_required"),
    ));
    v.push(script(
        "FID-EMAIL-003",
        "a contact address does not sign in as the account",
        contact_is_not_a_sign_in,
        Expect::Code(200, ""),
    ));

    // ── held-open streams ──
    for (id, what, f) in [
        (
            "SYNC-001",
            "a revoked link ends the holder's sync feed with an error and no change",
            link_sync_feed as Script,
        ),
        (
            "SYNC-002",
            "a revoked link ends the holder's ?live=1 feed with an error and no invalidate",
            link_invalidate_feed,
        ),
        (
            "SYNC-003",
            "a withdrawn delegation ends the agent's sync feed",
            withdrawn_delegation_feed,
        ),
        (
            "SYNC-005",
            "a logout ends the session's feed within one tick, with no write",
            logout_feed,
        ),
        (
            "SYNC-006",
            "a removed member's /live stream ends with an error",
            removed_member_live,
        ),
        (
            "SYNC-007",
            "a ?live=1 feed hears another member's write as a signal, never a row",
            invalidate_carries_no_row,
        ),
        (
            "SYNC-008",
            "a forged resume hash on a sync feed gets the normal load, no extra rows",
            forged_resume,
        ),
        (
            "DATA-040",
            "a read added to a /live stream opened in a foreign space is refused",
            live_add_in_foreign_space,
        ),
    ] {
        v.push(script(id, what, f, Expect::Code(200, "")));
    }

    // ── after the destructive rows: what they aimed at is whole ──
    v.push(r(
        "CATALOG-INTACT",
        "A's note survived every attack above",
        "GET",
        "/op/noteById?id={NOTE_A}",
        A,
        Expect::Rows(1),
    ));
    v
}

/// Run every row. Answers in the order of [`specs`].
pub struct Outcome {
    pub what: String,
    pub expect: Expect,
    pub mark: Mark,
    pub answer: Answer,
}

pub fn run(d: &Deployment, w: &World) -> Vec<Outcome> {
    let o = overlay(d, w);
    let trace = std::env::var("CATALOG_TRACE").is_ok_and(|v| v == "1");
    let mut out = Vec::new();
    for s in specs() {
        let (answer, shown) = match s.script {
            Some(f) => (f(d, w, &o), "script".to_string()),
            None => {
                let (path, headers, body) = s.request(w, &o);
                let a = d.raw(s.method, &path, &headers, &body);
                (
                    a,
                    format!("{} {}", s.method, path.chars().take(60).collect::<String>()),
                )
            }
        };
        let (expect, answer, tail) = match s.check {
            Some(ok) => {
                let pass = ok(&answer);
                let tail = if pass {
                    String::new()
                } else {
                    format!(" [check failed, saw {}/{}]", answer.status, answer.code())
                };
                (s.expect, synthetic(pass, &answer), tail)
            }
            None => (s.expect, answer, String::new()),
        };
        if trace {
            eprintln!(
                "  {} -> {}/{} {}",
                s.id,
                answer.status,
                answer.code(),
                shown
            );
        }
        out.push(Outcome {
            what: format!("{} {}{}", s.id, s.what, tail),
            expect,
            mark: s.mark,
            answer,
        });
    }
    out
}

// ── the guard ───────────────────────────────────────────────────────────

fn catalog_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the workspace")
        .join("security/attack-catalog")
}

/// Every catalog entry: (id, replayable).
fn catalog() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(catalog_dir())
        .expect("security/attack-catalog")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    for f in files {
        for line in std::fs::read_to_string(&f).expect("read").lines() {
            if line.trim().is_empty() {
                continue;
            }
            let j: Value = serde_json::from_str(line).expect("a catalog line is JSON");
            out.push((
                j["id"].as_str().expect("id").to_string(),
                j["replayable"].as_str().expect("replayable").to_string(),
            ));
        }
    }
    out
}

/// A new catalog entry cannot be silently dropped: every `http` id has a
/// row or a reason, no row or reason names an id the catalog lacks, and
/// COVERAGE.md lists every id.
#[test]
fn every_catalog_http_id_has_a_row_or_a_reason() {
    let all = catalog();
    let ids: std::collections::BTreeSet<&str> = all.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(ids.len(), all.len(), "a catalog id is used twice");
    let rows: std::collections::BTreeSet<&str> = specs().iter().map(|s| s.id).collect();
    let skipped: std::collections::BTreeSet<&str> = SKIPPED.iter().map(|(i, _)| *i).collect();
    let mut problems = Vec::new();
    for (id, how) in &all {
        if how == "http" && !rows.contains(id.as_str()) && !skipped.contains(id.as_str()) {
            problems.push(format!(
                "{id}: an http entry with no row and no skip reason"
            ));
        }
        if how == "http" && rows.contains(id.as_str()) && skipped.contains(id.as_str()) {
            problems.push(format!("{id}: both a row and skipped"));
        }
    }
    for id in rows.iter().chain(skipped.iter()) {
        if !ids.contains(id) && *id != "CATALOG-INTACT" {
            problems.push(format!("{id}: not in the catalog"));
        }
    }
    for (id, why) in SKIPPED {
        if why.is_empty() {
            problems.push(format!("{id}: a skip with no reason"));
        }
    }
    let coverage = std::fs::read_to_string(catalog_dir().join("COVERAGE.md")).unwrap_or_default();
    // A skip that names a console test must have that test (named by the
    // id) in the console crate, and COVERAGE.md must point at it.
    let console_tests = std::fs::read_to_string(
        catalog_dir().join("../../crates/ordo-console/tests/catalog_attacks.rs"),
    )
    .unwrap_or_default();
    for (id, why) in SKIPPED {
        if why.starts_with("console test:") {
            let name = id.replace('-', "_");
            if !console_tests.contains(&format!("async fn {name}()")) {
                problems.push(format!("{id}: no console test `{name}`"));
            }
            let row = coverage
                .lines()
                .find(|l| l.contains(&format!("`{id}`")))
                .unwrap_or_default();
            if !row.contains("catalog_attacks.rs") {
                problems.push(format!("{id}: COVERAGE.md does not name the console test"));
            }
        }
    }
    for id in &ids {
        if !coverage.contains(&format!("`{id}`")) {
            problems.push(format!("{id}: not listed in COVERAGE.md"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
