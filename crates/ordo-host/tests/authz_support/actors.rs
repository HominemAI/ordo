//! The actors and the objects, each made through the real routes.
//!
//! Every credential here comes from the sign-in path a real caller uses:
//! a passkey registration or assertion, an invitation, a share accepted and
//! its window opened, a share link opened, a delegation approved and
//! redeemed, a console key signed in. Two are made by hand, and say so: an
//! expired token (the same claims as a live one, re-signed with an expiry
//! in the past, since no setting makes a session last under a minute) and a
//! token signed with another deployment's key.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::CompleteRequest;
use serde_json::{json, Value};

use super::{eventually, Answer, Deployment, AGENT, FOREIGN_TOKEN_KEY, HOME, PAGE, TOKEN_KEY};

/// A person: their device and what the page holds once they are signed in.
pub struct Person {
    pub name: String,
    pub device: TestAuthenticator,
    pub token: String,
    /// The tenant they act in here.
    pub space: String,
    /// Their seat there, `members.id`.
    pub member: String,
    /// The renewal cookie the sign-in set, `name=value`.
    pub cookie: Option<String>,
}

/// One caller of the matrix: a bearer (or none) and the space it names.
#[derive(Clone, Debug)]
pub struct Actor {
    pub token: Option<String>,
    pub space: String,
}

/// Everything the matrix calls with and calls on.
pub struct World {
    pub actors: BTreeMap<&'static str, Actor>,
    /// Object ids, handles and secrets, by placeholder name (`NOTE_A`,
    /// `HANDLE_A_PLAIN`, `LINK_PWD`, ...). A row's request names them in
    /// braces.
    pub vars: BTreeMap<String, String>,
    pub people: BTreeMap<&'static str, Person>,
}

impl World {
    pub fn actor(&self, key: &str) -> &Actor {
        self.actors
            .get(key)
            .unwrap_or_else(|| panic!("no actor `{key}`"))
    }

    pub fn var(&self, key: &str) -> &str {
        self.vars
            .get(key)
            .unwrap_or_else(|| panic!("no object `{key}`"))
    }

    /// `{NAME}` replaced by the object or token it names. `{TOKEN_<actor>}`
    /// and `{SPACE_<actor>}` name an actor's bearer and space.
    pub fn fill(&self, template: &str) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[i..]);
                return out;
            };
            let key = &after[..end];
            let value = if let Some(a) = key.strip_prefix("TOKEN_") {
                self.actors
                    .get(a)
                    .and_then(|x| x.token.clone())
                    .unwrap_or_default()
            } else if let Some(a) = key.strip_prefix("SPACE_") {
                self.actors
                    .get(a)
                    .map(|x| x.space.clone())
                    .unwrap_or_default()
            } else if let Some(v) = self.vars.get(key) {
                v.clone()
            } else if key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                && !key.is_empty()
            {
                panic!("the template names `{{{key}}}` and the world has no such thing")
            } else {
                // Not a placeholder: JSON braces.
                out.push('{');
                rest = after;
                continue;
            };
            out.push_str(&value);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

pub fn device(seed: u8, name: &str) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: name.as_bytes().to_vec(),
        rp_id: "localhost".to_string(),
        origin: PAGE.to_string(),
        ..TestAuthenticator::es256()
    }
}

pub fn assertion_body(req: &CompleteRequest) -> Value {
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = req
    else {
        panic!("not an assertion");
    };
    json!({
        "method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
        "credential_id": b64url_encode(credential_id),
        "client_data_json": b64url_encode(client_data_json),
        "authenticator_data": b64url_encode(authenticator_data),
        "signature": b64url_encode(signature),
    })
}

fn renewal_cookie(a: &Answer) -> Option<String> {
    a.cookies().into_iter().find_map(|c| {
        let first = c.split(';').next()?.trim().to_string();
        let (name, value) = first.split_once('=')?;
        (name.ends_with("ordo_renew") && !value.is_empty()).then_some(first)
    })
}

/// `registerPasskey`, with `extra` merged into `/auth/complete` (a page
/// holding an invitation adds `space` and `invitation`).
pub fn register(d: &Deployment, device: &TestAuthenticator, display: &str, extra: Value) -> Answer {
    let options = d.auth(
        "/auth/register",
        &json!({"method": "passkey", "display": display}),
        None,
    );
    assert_eq!(options.status, 200, "{}", options.text());
    let options = options.json();
    let challenge = options["challenge"].as_str().expect("challenge");
    let CompleteRequest::PasskeyRegistration {
        challenge: c,
        client_data_json,
        attestation_object,
    } = device.register(challenge)
    else {
        panic!("not a registration");
    };
    let mut body = json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": c.0,
        "client_data_json": b64url_encode(&client_data_json),
        "attestation_object": b64url_encode(&attestation_object),
        "display": display,
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    d.auth("/auth/complete", &body, None)
}

/// `signInWithPasskey`: an assertion at `/auth/begin` and `/auth/complete`.
pub fn sign_in(d: &Deployment, device: &TestAuthenticator, extra: Value) -> Answer {
    let options = d.auth("/auth/begin", &json!({"method": "passkey"}), None);
    assert_eq!(options.status, 200, "{}", options.text());
    let challenge = options.json()["challenge"]
        .as_str()
        .expect("challenge")
        .to_string();
    let mut body = assertion_body(&device.assert(&challenge));
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    d.auth("/auth/complete", &body, None)
}

/// The sign-in's answer once the person's own tenant is made.
fn tenant_ready(d: &Deployment, mut done: Value) -> Value {
    if done["provisioning"] != "in progress" {
        return done;
    }
    let bearer = format!("Bearer {}", done["token"].as_str().expect("a token"));
    let headers = [("Origin", PAGE), ("Authorization", bearer.as_str())];
    let mut last = Value::Null;
    for _ in 0..600 {
        let said = d.send("GET", "/auth/provisioning", &headers, "");
        assert_eq!(said.status, 200, "{}", said.text());
        last = said.json();
        match last["state"].as_str() {
            Some("ready") => {
                done["space"] = last["space"].clone();
                done["provisioning"] = Value::Null;
                return done;
            }
            Some("provisioning") => std::thread::sleep(Duration::from_millis(100)),
            _ => panic!("the tenant was not made: {last}"),
        }
    }
    panic!("the tenant was not ready in 60 seconds: {last}");
}

/// A call to `/op/<name>` as `core/client.ts` sends it.
pub fn op(
    d: &Deployment,
    space: &str,
    token: &str,
    method: &str,
    name: &str,
    params: &Value,
) -> Answer {
    static KEY: AtomicU64 = AtomicU64::new(1);
    let bearer = format!("Bearer {token}");
    let key = format!("authz-{}", KEY.fetch_add(1, Ordering::SeqCst));
    let mut headers = vec![
        ("X-Ordo-Space", space),
        ("Origin", PAGE),
        ("Accept", "application/json"),
    ];
    if !token.is_empty() {
        headers.push(("Authorization", bearer.as_str()));
    }
    if method != "GET" {
        headers.push(("Idempotency-Key", key.as_str()));
    }
    if method == "POST" {
        headers.push(("Content-Type", "application/json"));
        d.send(
            method,
            &format!("/op/{name}"),
            &headers,
            &params.to_string(),
        )
    } else {
        d.send(
            method,
            &format!("/op/{name}{}", query(params)),
            &headers,
            "",
        )
    }
}

pub fn query(params: &Value) -> String {
    let q: Vec<String> = params
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let v = v.as_str().map(str::to_string).unwrap_or(v.to_string());
                    format!("{k}={}", urlencode(&v))
                })
                .collect()
        })
        .unwrap_or_default();
    if q.is_empty() {
        String::new()
    } else {
        format!("?{}", q.join("&"))
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A write or read that must land; answers its rows.
pub fn must(d: &Deployment, p: &Person, method: &str, name: &str, params: Value) -> Vec<Value> {
    let a = op(d, &p.space, &p.token, method, name, &params);
    assert_eq!(a.status, 200, "{name} as {}: {}", p.name, a.text());
    a.json()["rows"].as_array().cloned().unwrap_or_default()
}

/// Wait until the door serves the person in their tenant.
fn settle(d: &Deployment, space: &str, token: &str) {
    eventually(&format!("the door serves {space}"), || {
        (op(d, space, token, "GET", "myNotes", &json!({})).status == 200).then_some(())
    });
}

/// The person's seat, read from the directory the door builds from. No op
/// answers a person their own member id, and this is a read.
fn seat_of(d: &Deployment, p: &Person) -> String {
    let principal = principal_of(&p.token);
    // A tenant's rows are in a database of its own, named by its id; the
    // deployment's own space is in the control database.
    for db in [p.space.clone(), d.db.clone()] {
        let pg = ordo_conformance::Pg {
            db,
            ..ordo_conformance::Pg::default()
        };
        let rows = pg
            .query(&format!(
                r#"SELECT "id" FROM "members" WHERE "$space" = '{}' AND "principal" = '{}'"#,
                p.space.replace('\'', "''"),
                principal.replace('\'', "''")
            ))
            .unwrap_or_default();
        if let Some(id) = rows.first().and_then(|r| r.first().cloned().flatten()) {
            return id;
        }
    }
    panic!("{} holds no seat in {}", p.name, p.space)
}

pub fn principal_of(token: &str) -> String {
    ordo_identity::verify(&token_key(), token, ordo_identity::Timestamp(now_secs()))
        .map(|c| c.principal.0)
        .unwrap_or_default()
}

fn person_of(
    d: &Deployment,
    name: &str,
    device: TestAuthenticator,
    done: &Answer,
    space: Option<&str>,
) -> Person {
    assert_eq!(done.status, 200, "{name}: {}", done.text());
    let cookie = renewal_cookie(done);
    let j = tenant_ready(d, done.json());
    let token = j["token"].as_str().expect("a token").to_string();
    let space = space
        .map(str::to_string)
        .or_else(|| j["space"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("{name} has no tenant: {j}"));
    settle(d, &space, &token);
    let mut p = Person {
        name: name.to_string(),
        device,
        token,
        space,
        member: String::new(),
        cookie,
    };
    p.member = seat_of(d, &p);
    p
}

/// A founder signs up and lands in a tenant of their own, as its owner.
pub fn found(d: &Deployment, seed: u8, name: &str) -> Person {
    let dev = device(seed, name);
    let done = register(d, &dev, name, json!({}));
    person_of(d, name, dev, &done, None)
}

/// An owner or manager invites someone at `role`; they sign up with it.
pub fn join(d: &Deployment, admin: &Person, seed: u8, name: &str, role: &str) -> Person {
    let row = must(
        d,
        admin,
        "POST",
        "std.space.invite",
        json!({"role": role, "expires": "7d", "label": "Join"}),
    );
    let ticket = row[0]["ticket"].as_str().expect("a ticket").to_string();
    let dev = device(seed, name);
    let done = register(
        d,
        &dev,
        name,
        json!({"space": admin.space, "invitation": ticket}),
    );
    assert_eq!(
        done.json()["admission"]["outcome"],
        "admitted",
        "{}",
        done.text()
    );
    person_of(d, name, dev, &done, Some(&admin.space))
}

/// A fresh passkey touch: `/auth/step-up/begin`, the assertion, `/auth/step-up`.
/// Answers the present token.
pub fn step_up(d: &Deployment, device: &TestAuthenticator, token: &str) -> String {
    try_step_up(d, device, token).unwrap_or_else(|a| panic!("step-up: {} {}", a.status, a.text()))
}

/// [`step_up`], answering the refusal instead of failing.
pub fn try_step_up(
    d: &Deployment,
    device: &TestAuthenticator,
    token: &str,
) -> Result<String, Answer> {
    let options = d.auth("/auth/step-up/begin", &json!({}), Some(token));
    assert_eq!(options.status, 200, "step-up begin: {}", options.text());
    let challenge = options.json()["challenge"]
        .as_str()
        .expect("challenge")
        .to_string();
    let a = d.auth(
        "/auth/step-up",
        &assertion_body(&device.assert(&challenge)),
        Some(token),
    );
    if a.status != 200 {
        return Err(a);
    }
    Ok(a.json()["token"].as_str().expect("token").to_string())
}

pub fn token_key() -> ordo_identity::TokenKey {
    key_of(TOKEN_KEY)
}

fn key_of(hex: &str) -> ordo_identity::TokenKey {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    ordo_identity::TokenKey::from_secret(&out)
}

/// The same claims as `token`, signed with `key` and ending at `expires`.
pub fn remint(token: &str, foreign: bool, expires: u64) -> String {
    let now = ordo_identity::Timestamp(now_secs());
    let c = ordo_identity::verify(&token_key(), token, now).expect("a live token");
    let spec = ordo_identity::TokenSpec {
        principal: c.principal,
        expires: ordo_identity::Timestamp(expires),
        client: c.client,
        user_presence_at: c.user_presence_at,
        capability: c.capability,
        delegation: c.delegation,
        session: c.session,
    };
    let key = if foreign {
        key_of(FOREIGN_TOKEN_KEY)
    } else {
        token_key()
    };
    ordo_identity::mint(&key, &spec)
}

/// The same claims as `token` with no passkey touch: a session whose
/// presence has gone stale. The door decides presence from this claim alone.
pub fn without_presence(token: &str) -> String {
    let now = ordo_identity::Timestamp(now_secs());
    let c = ordo_identity::verify(&token_key(), token, now).expect("a live token");
    let spec = ordo_identity::TokenSpec {
        principal: c.principal,
        expires: ordo_identity::Timestamp(now_secs() + 3600),
        client: c.client,
        user_presence_at: None,
        capability: c.capability,
        delegation: c.delegation,
        session: c.session,
    };
    ordo_identity::mint(&token_key(), &spec)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

pub fn stamp() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    format!("2026-10-01T09:{:02}:{:02}Z", (n / 60) % 60, n % 60)
}

fn note(d: &Deployment, p: &Person, title: &str) -> String {
    let rows = must(
        d,
        p,
        "POST",
        "createNote",
        json!({"title": title, "body": format!("{title} body"), "created": stamp()}),
    );
    rows[0]["created"]
        .as_str()
        .or_else(|| rows[0]["id"].as_str())
        .unwrap_or_else(|| panic!("createNote answered {rows:?}"))
        .to_string()
}

/// `PUT /blob/{entity}/{field}/{id}` with `bytes` as `mime`.
pub fn upload(
    d: &Deployment,
    space: &str,
    token: &str,
    path: &str,
    mime: &str,
    bytes: &[u8],
) -> Answer {
    let headers = vec![
        ("X-Ordo-Space".to_string(), space.to_string()),
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Origin".to_string(), PAGE.to_string()),
        ("Content-Type".to_string(), mime.to_string()),
    ];
    d.raw("PUT", path, &headers, bytes)
}

fn handle_of(d: &Deployment, p: &Person, file: &str, field: &str) -> String {
    let rows = must(d, p, "GET", "myFiles", json!({}));
    rows.iter()
        .find(|r| r["id"] == json!(file))
        .and_then(|r| r[field].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("{file}.{field} has no handle: {rows:?}"))
}

fn add_file(d: &Deployment, p: &Person, note: &str, name: &str) -> String {
    let rows = must(d, p, "POST", "addFile", json!({"note": note, "name": name}));
    rows[0]["created"]
        .as_str()
        .or_else(|| rows[0]["id"].as_str())
        .expect("a file id")
        .to_string()
}

/// A file on `note` holding `bytes` as `mime`; answers (row id, handle).
fn file_with(
    d: &Deployment,
    p: &Person,
    note: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> (String, String) {
    let id = add_file(d, p, note, name);
    let a = upload(
        d,
        &p.space,
        &p.token,
        &format!("/blob/files/plain/{id}"),
        mime,
        bytes,
    );
    assert!(
        (200..300).contains(&a.status),
        "upload {name}: {} {}",
        a.status,
        a.text()
    );
    let h = handle_of(d, p, &id, "plain");
    (id, h)
}

/// A share link on `note`, minted by `p` with `extra` (password, maxOpens,
/// expires). Answers `(secret, id)`.
pub fn link(d: &Deployment, p: &Person, note: &str, extra: Value) -> (String, String) {
    let mut params = json!({
        "ops": "myNotes,noteById,std.notes.get",
        "rows": json!([{"entity": "notes", "id": note, "level": "read"}]).to_string(),
        "label": "matrix",
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        params[k] = v;
    }
    let rows = must(d, p, "POST", "std.space.shareLink", params);
    (
        rows[0]["link"].as_str().expect("a secret").to_string(),
        rows[0]["id"].as_str().expect("a link id").to_string(),
    )
}

pub fn open_link(
    d: &Deployment,
    secret: &str,
    password: Option<&str>,
    device: Option<&str>,
) -> Answer {
    let mut body = json!({"link": secret});
    if let Some(p) = password {
        body["password"] = json!(p);
    }
    if let Some(dev) = device {
        body["device"] = json!(dev);
    }
    d.send(
        "POST",
        "/auth/link",
        &[("Content-Type", "application/json"), ("Origin", PAGE)],
        &body.to_string(),
    )
}

fn link_token(d: &Deployment, secret: &str, password: Option<&str>) -> String {
    let a = open_link(d, secret, password, Some("matrix-device"));
    assert_eq!(a.status, 200, "open link: {}", a.text());
    a.json()["token"].as_str().expect("token").to_string()
}

fn actor(token: &str, space: &str) -> Actor {
    Actor {
        token: Some(token.to_string()),
        space: space.to_string(),
    }
}

/// Make every actor and object. Order matters only where one thing needs
/// another; nothing here is spent by the matrix that runs after.
pub fn build(d: &mut Deployment) -> World {
    // ── the deployment owner ──
    // `ORDO_DEPLOYMENT_OWNERS` names a principal, and a principal is minted
    // when a person registers (`deploying.md`): Dana registers on the app's
    // page, the deployment is restarted naming her, and she signs in to the
    // deployment's own space.
    let ddev = device(1, "deployment-owner");
    let first = register(d, &ddev, "Dana", json!({}));
    assert_eq!(first.status, 200, "Dana registers: {}", first.text());
    let first = tenant_ready(d, first.json());
    let principal = principal_of(first["token"].as_str().expect("token"));
    // A door with the identity service ignores the setting (step 8), so
    // she is named on a boot without it, as before the cut-over; the
    // next boot, with it, keeps her row as an owner of the home space.
    d.restart(&[
        ("ORDO_DEPLOYMENT_OWNERS", &principal),
        ("ORDO_IDENTITY_URL", ""),
    ]);
    // The removed claim code, with a value the old door refused: the
    // door boots and names it as ignored (owner, 2026-10-30).
    d.restart(&[
        ("ORDO_DEPLOYMENT_OWNERS", &principal),
        ("ORDO_AUTH_CLAIM_CODE", "sometimes"),
    ]);
    assert!(
        d.log()
            .contains("ORDO_DEPLOYMENT_OWNERS is set and ignored"),
        "the boot with the identity service does not name the ignored setting"
    );
    assert!(
        d.log().contains("ORDO_AUTH_CLAIM_CODE is set and ignored"),
        "the boot does not name the ignored claim code"
    );
    let d: &Deployment = d;
    let done = sign_in(d, &ddev, json!({"space": HOME}));
    let dana = person_of(d, "Dana", ddev, &done, Some(HOME));
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut actors: BTreeMap<&'static str, Actor> = BTreeMap::new();
    let mut people: BTreeMap<&'static str, Person> = BTreeMap::new();

    // ── tenants ──
    let olga = found(d, 2, "Olga"); // owner of T1
    let carl = found(d, 3, "Carl"); // owner of T2
    let t1 = olga.space.clone();
    let t2 = carl.space.clone();
    vars.insert("T1".into(), t1.clone());
    vars.insert("T2".into(), t2.clone());
    vars.insert("HOME".into(), HOME.into());

    let ann = join(d, &olga, 4, "Ann", "manager");
    let ben = join(d, &olga, 5, "Ben", "manager");
    let vic = join(d, &olga, 6, "Vic", "viewer");
    let eve = join(d, &olga, 7, "Eve", "editor");
    let max = join(d, &olga, 8, "Max", "manager");
    let ron = join(d, &olga, 9, "Ron", "editor");
    let gus = found(d, 10, "Gus"); // a guest, from a tenant of his own

    // ── objects ──
    let note_a = note(d, &ann, "Ann's note");
    let note_b = note(d, &ben, "Ben's note");
    let note_t2 = note(d, &carl, "Carl's note");
    let note_shared = note(d, &ann, "Shared with Gus");
    let note_linked = note(d, &ann, "Behind a link");
    let note_victim = note(d, &ann, "Ann's note for B to delete");
    let note_victim2 = note(d, &ann, "Ann's note for the agent to delete");
    vars.insert("NOTE_A".into(), note_a.clone());
    vars.insert("NOTE_B".into(), note_b.clone());
    vars.insert("NOTE_T2".into(), note_t2.clone());
    vars.insert("NOTE_SHARED".into(), note_shared.clone());
    vars.insert("NOTE_LINKED".into(), note_linked.clone());
    vars.insert("NOTE_VICTIM".into(), note_victim);
    vars.insert("NOTE_VICTIM_AGENT".into(), note_victim2);

    // Files. A's plain file; HTML and SVG uploads to see how they are served;
    // a file row with no bytes for the upload rows; B's file; T2's file.
    let (file_a, h_a) = file_with(d, &ann, &note_a, "a.txt", "text/plain", b"0123456789abcdef");
    let (_, h_html) = file_with(
        d,
        &ann,
        &note_a,
        "a.html",
        "text/html",
        b"<script>fetch('/auth/refresh',{method:'POST'})</script>",
    );
    let (_, h_svg) = file_with(
        d,
        &ann,
        &note_a,
        "a.svg",
        "image/svg+xml",
        b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>",
    );
    let (_, h_linked) = file_with(d, &ann, &note_linked, "l.txt", "text/plain", b"linked file");
    let (_, h_shared) = file_with(d, &ann, &note_shared, "s.txt", "text/plain", b"shared file");
    let file_a_empty = add_file(d, &ann, &note_a, "empty");
    let (file_b, h_b) = file_with(d, &ben, &note_b, "b.txt", "text/plain", b"ben's bytes");
    let (_, h_t2) = file_with(d, &carl, &note_t2, "c.txt", "text/plain", b"carl's bytes");
    vars.insert("FILE_A".into(), file_a);
    vars.insert("FILE_A_EMPTY".into(), file_a_empty);
    vars.insert("FILE_B".into(), file_b);
    vars.insert("HANDLE_A_PLAIN".into(), h_a.clone());
    vars.insert("HANDLE_A_HTML".into(), h_html);
    vars.insert("HANDLE_A_SVG".into(), h_svg);
    vars.insert("HANDLE_LINKED".into(), h_linked);
    vars.insert("HANDLE_SHARED".into(), h_shared);
    vars.insert("HANDLE_B".into(), h_b);
    vars.insert("HANDLE_T2".into(), h_t2);

    // A public poster with a file, in the deployment's own space.
    let poster = must(d, &dana, "POST", "addPoster", json!({"title": "Open day"}));
    let poster = poster[0]["created"]
        .as_str()
        .or_else(|| poster[0]["id"].as_str())
        .expect("poster id")
        .to_string();
    let up = upload(
        d,
        HOME,
        &dana.token,
        &format!("/blob/posters/art/{poster}"),
        "image/png",
        b"\x89PNG\r\n\x1a\nposter",
    );
    assert!(
        (200..300).contains(&up.status),
        "poster upload: {}",
        up.text()
    );
    let rows = op(d, HOME, "", "GET", "posterList", &json!({}));
    let h_public = rows.json()["rows"][0]["art"]
        .as_str()
        .unwrap_or_else(|| panic!("posterList: {}", rows.text()))
        .to_string();
    vars.insert("POSTER".into(), poster);
    vars.insert("HANDLE_PUBLIC".into(), h_public);

    // ── a share: Ann shares one note with Gus, who accepts and opens it ──
    let ticket = must(
        d,
        &ann,
        "POST",
        "std.space.share",
        json!({
            "ops": "myNotes,noteById,std.notes.get",
            "rows": json!([{"entity": "notes", "id": note_shared, "level": "read"}]).to_string(),
            "expires": "30d",
            "label": "For Gus",
        }),
    )[0]["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();
    let accepted = d.auth(
        "/auth/share/accept",
        &json!({"ticket": ticket, "display_name": "Gus"}),
        Some(&gus.token),
    );
    assert_eq!(accepted.status, 200, "accept: {}", accepted.text());
    let window = eventually("the window opens", || {
        let w = d.auth("/auth/window", &json!({"space": t1}), Some(&gus.token));
        (w.status == 200).then(|| w.json()["token"].as_str().expect("w").to_string())
    });
    vars.insert("SHARE_TICKET_SPENT".into(), ticket);

    // ── share links on one note ──
    let (link_ok, _) = link(d, &ann, &note_linked, json!({}));
    let (link_pwd, _) = link(d, &ann, &note_linked, json!({"password": "correct horse"}));
    let (link_lock, _) = link(d, &ann, &note_linked, json!({"password": "battery staple"}));
    let (link_once, _) = link(d, &ann, &note_linked, json!({"maxOpens": "1"}));
    let (link_short, _) = link(d, &ann, &note_linked, json!({"expires": "5s"}));
    // Opened now, while it is live, so the token is real; its grant and the
    // token both lapse in 5 s, well before the matrix runs (a GRACE sleep
    // and more setup follow). The actor is then a dead one, refused.
    let tok_link_short = link_token(d, &link_short, None);
    let (link_revoked, link_revoked_id) = link(d, &ann, &note_linked, json!({}));
    let tok_link = link_token(d, &link_ok, None);
    let tok_link_pwd = link_token(d, &link_pwd, Some("correct horse"));
    let tok_link_revoked = link_token(d, &link_revoked, None);
    // The one open `link_once` allows, by its own device.
    let tok_link_once = link_token(d, &link_once, None);
    must(
        d,
        &ann,
        "POST",
        "std.space.revokeLink",
        json!({"link": link_revoked_id}),
    );
    vars.insert("LINK_OK".into(), link_ok);
    vars.insert("LINK_PWD".into(), link_pwd);
    vars.insert("LINK_LOCK".into(), link_lock);
    vars.insert("LINK_ONCE".into(), link_once);
    vars.insert("LINK_SHORT".into(), link_short);
    vars.insert("LINK_REVOKED".into(), link_revoked);

    // ── a delegation: the agent asks for Ann, Ann approves with a touch ──
    let req = d.send(
        "POST",
        "/auth/delegation/request",
        &[("Content-Type", "application/json")],
        &json!({
            "space": t1, "agent": AGENT, "person": ann.member,
            "ops": ["myNotes", "noteById", "createNote", "editNote", "deleteNote"],
            "hours": 2,
        })
        .to_string(),
    );
    assert_eq!(req.status, 201, "delegation request: {}", req.text());
    let req = req.json();
    let request = req["request"].as_str().expect("request").to_string();
    let binder = req["binder"].as_str().expect("binder").to_string();
    let present = step_up(d, &ann.device, &ann.token);
    let approved = op(
        d,
        &t1,
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
    let tok_agent = redeemed.json()["token"]
        .as_str()
        .expect("agent token")
        .to_string();
    vars.insert("DELEGATION_REQUEST".into(), request);
    vars.insert("DELEGATION_BINDER".into(), binder);

    // ── the console key: none is made ──
    // The door verifies the identity service's tokens here
    // (`ORDO_IDENTITY_URL` set), so no console key is made, even with a
    // touch, and the refusal names the setting (`plan-identity-service.md`
    // step 8). A key from before does not sign in (`console_key.rs`).
    let sk = SigningKey::from_bytes(&[42u8; 32]);
    let pk = b64url_encode(sk.verifying_key().as_bytes());
    let dana_present = step_up(d, &dana.device, &dana.token);
    let added = op(
        d,
        HOME,
        &dana_present,
        "POST",
        ordo_host::console_keys::ADD,
        &json!({"publicKey": pk, "name": "matrix console"}),
    );
    assert_ne!(added.status, 200, "addConsoleKey: {}", added.text());
    assert!(
        added.text().contains("ORDO_IDENTITY_URL"),
        "addConsoleKey: {}",
        added.text()
    );
    vars.insert("DANA_PRINCIPAL".into(), principal_of(&dana.token));

    // ── dead tokens ──
    // Expired: Ann's own claims, ending an hour ago.
    let tok_expired = remint(&ann.token, false, now_secs() - 3600);
    // Another deployment's key: Ann's claims, signed by somebody else.
    let tok_foreign = remint(&ann.token, true, now_secs() + 3600);
    // Logged out: a second sign-in of Ann's, ended at `/auth/logout`.
    let second = sign_in(d, &ann.device, json!({"space": t1}));
    assert_eq!(second.status, 200, "{}", second.text());
    let tok_logged_out = second.json()["token"].as_str().expect("t").to_string();
    let out = d.auth("/auth/logout", &json!({}), Some(&tok_logged_out));
    assert!(out.status < 300, "logout: {}", out.text());
    // Revoked: Ron's session, ended by his sign-out everywhere from another.
    let ron2 = sign_in(d, &ron.device, json!({"space": t1}));
    let tok_ron2 = ron2.json()["token"].as_str().expect("t").to_string();
    let all = d.auth("/auth/logout-all", &json!({}), Some(&tok_ron2));
    assert!(all.status < 300, "logout-all: {}", all.text());
    let tok_revoked = ron.token.clone();
    // Replayed: a renewal cookie spent, then presented again past the grace,
    // ends its family; the token the first renewal gave is the dead one.
    let fresh = sign_in(d, &eve.device, json!({"space": t1}));
    let cookie = renewal_cookie(&fresh).expect("a sign-in sets the renewal cookie");
    let renewed = d.send(
        "POST",
        "/auth/refresh",
        &[("Origin", PAGE), ("Cookie", &cookie)],
        "",
    );
    assert_eq!(renewed.status, 200, "renewal: {}", renewed.text());
    let tok_replayed = renewed.json()["token"].as_str().expect("t").to_string();
    vars.insert("COOKIE_SPENT".into(), cookie);
    vars.insert("COOKIE_ANN".into(), ann.cookie.clone().unwrap_or_default());
    // A live cookie of Olga's, for the foreign-origin rows: they must leave
    // it whole.
    vars.insert(
        "COOKIE_OLGA".into(),
        olga.cookie.clone().unwrap_or_default(),
    );

    actors.insert(
        "anonymous",
        Actor {
            token: None,
            space: t1.clone(),
        },
    );
    actors.insert("guest", actor(&window, &t1));
    actors.insert("link", actor(&tok_link, &t1));
    actors.insert("link_password", actor(&tok_link_pwd, &t1));
    actors.insert("link_once", actor(&tok_link_once, &t1));
    actors.insert("link_expired", actor(&tok_link_short, &t1));
    actors.insert("link_revoked", actor(&tok_link_revoked, &t1));
    actors.insert("member_a", actor(&ann.token, &t1));
    actors.insert("member_b", actor(&ben.token, &t1));
    actors.insert("viewer", actor(&vic.token, &t1));
    actors.insert("editor", actor(&eve.token, &t1));
    actors.insert("manager", actor(&max.token, &t1));
    actors.insert("owner", actor(&olga.token, &t1));
    actors.insert("owner_stale", actor(&without_presence(&olga.token), &t1));
    actors.insert("manager_stale", actor(&without_presence(&max.token), &t1));
    actors.insert("t2_owner", actor(&carl.token, &t2));
    actors.insert("deployment_owner", actor(&dana.token, HOME));
    actors.insert("agent", actor(&tok_agent, &t1));
    actors.insert("expired", actor(&tok_expired, &t1));
    actors.insert("foreign_key", actor(&tok_foreign, &t1));
    actors.insert("logged_out", actor(&tok_logged_out, &t1));
    actors.insert("revoked", actor(&tok_revoked, &t1));
    actors.insert("replayed", actor(&tok_replayed, &t1));
    actors.insert("garbled", actor("not.a-token", &t1));

    for (k, p) in [
        ("dana", dana),
        ("olga", olga),
        ("carl", carl),
        ("ann", ann),
        ("ben", ben),
        ("vic", vic),
        ("eve", eve),
        ("max", max),
        ("ron", ron),
        ("gus", gus),
    ] {
        vars.insert(
            format!("MEMBER_{}", k.to_ascii_uppercase()),
            p.member.clone(),
        );
        people.insert(k, p);
    }

    let mut w = World {
        actors,
        vars,
        people,
    };
    // The replay itself, past the grace, so the family is ended before the
    // matrix runs. The short link has expired by then too.
    std::thread::sleep(Duration::from_secs(ordo_host::renewal::GRACE_SECS + 1));
    let replay = d.send(
        "POST",
        "/auth/refresh",
        &[("Origin", PAGE), ("Cookie", w.var("COOKIE_SPENT"))],
        "",
    );
    w.vars
        .insert("REPLAY_STATUS".into(), replay.status.to_string());
    w
}

/// The identity service's callers (`plan-identity-service.md` 4.3): a
/// person with a fresh touch and with a stale one, a machine key, and five
/// tokens the door must refuse. All in the home space; the matrix also
/// probes them in a tenant.
pub fn add_deployment_actors(w: &mut World, issuer: &super::identity::Issuer) {
    for (name, token) in [
        ("deployment_admin", issuer.person_fresh()),
        ("deployment_admin_stale", issuer.person_stale()),
        ("machine_key", issuer.machine()),
        ("deployment_expired", issuer.expired()),
        ("deployment_wrong_aud", issuer.wrong_aud()),
        ("deployment_wrong_iss", issuer.wrong_iss()),
        ("deployment_unknown_kid", issuer.unknown_kid()),
        ("deployment_foreign_signer", issuer.foreign_signer()),
    ] {
        w.actors.insert(name, actor(&token, HOME));
    }
}
