//! 17u — the CLI talking to a deployment instead of to its database.
//!
//! The owner: *"CLI should interact with the endpoint, should be adjusted
//! with a flag."* The operator surface was written when there was no
//! server, so every command reaches into Postgres directly, which is why
//! deploying to the cluster needs database credentials in whatever runs the
//! CLI. With a server, the ordinary path is the door.
//!
//! # Which commands move, and which must not
//!
//! `apply` moves, because 18l gave it a door. The ruling names what stays
//! on the database and why: **custody init, chain verification, revert**
//! must work when the door does not. An operator reaching for `ordo verify`
//! is very often reaching for it *because* the engine is refusing requests,
//! and a verification that can only be performed by the thing under
//! suspicion is not a verification. `ordo keys init` is sharper still: it
//! exists precisely because a deployment will not mint custody for a space
//! it was merely told about, and a running server is the thing that cannot
//! boot without the custody this command creates.
//!
//! `ordo space create` (17aa piece 3, `std.space.create`) never had a
//! database mode to fall back to, so there is no `--host`-decides-the-mode
//! branch the way `apply` has one: `tenant_ops.rs`'s module doc states the
//! ruling. Minting a tenant is the deployment's act, gated the way every
//! platform op is, and the CLI reaching a database directly to write a
//! founding member row would be a second way to do what the door already
//! does under a lock. This module's `--host`/`--token`/`--space`
//! resolution, the bearer-token story and the refusal rendering are the
//! same ones `apply --host` uses; only the body and the op path differ.
//!
//! # The host, and the scheme
//!
//! `--host`, or `ORDO_HOST`. There is no default: with neither, a command
//! that needs a host refuses and names both. A **bare host gets `https`**, and that is a deliberate
//! departure from `ordo_blob::http::Endpoint::parse`, whose own doc refuses
//! a schemeless endpoint rather than defaulting it. The two are answering
//! different questions and the difference is who typed the string:
//!
//! - A blob endpoint is *configuration*, set once by whoever stood the
//!   deployment up, and the case that matters there is a config that meant
//!   `http` for a local gateway — silently upgrading it produces a TLS
//!   handshake against a plaintext port, the least legible error in the set.
//! - A `--host` is typed by an operator at a shell, and the default it
//!   overrides is a bare public hostname. Refusing `--host ordo.example.com`
//!   for want of a scheme would be a refusal with nothing to learn from,
//!   and plaintext by default is the one wrong answer here: this request
//!   carries a bearer token that can replace the deployment's program.
//!
//! So `http` must be **spelled**, which is exactly what addressing a local
//! `ordo dev` looks like: `--host http://127.0.0.1:4000`. `ordo dev`
//! serves plaintext on `ORDO_DEV_ADDR` (default `127.0.0.1:4000`) and has
//! no certificate, so there is nothing to default to.
//!
//! # The HTTP client, and why there is no new dependency
//!
//! None was added. The workspace already owns a minimal HTTP/1.1 client
//! over `tokio-rustls` — `ordo_blob::http`, hand-rolled for the S3 surface
//! — and this is its second caller. The alternatives were all worse in the
//! same direction: `reqwest` brings connection pooling, redirect following,
//! cookie jars, proxy discovery and a middleware stack into the `ordo`
//! binary and into three Docker images, for one POST; `hyper` plus
//! `hyper-util`, `http-body-util` and `hyper-rustls` is three new manifest
//! entries to send one request on a connection that is closed immediately
//! afterwards. `plan-ci-images.md` §1.5 asks that any TLS added be rustls,
//! and this is rustls because `ordo_blob::http` already is.
//!
//! What it does not do is stated there rather than here: one connection per
//! request, no redirects, no HTTP/2, no decompression.
//!
//! # The token
//!
//! Since identity step 5, `--token` and `ORDO_TOKEN` are the first two of
//! four sources: a machine key and the session `ordo login` stored come
//! after them ([`crate::signin`]). The deployment person's token from the
//! identity service is the ordinary case now. The text below is the raw
//! token's story, which still holds.
//!
//! `--token`, or `ORDO_TOKEN`. It is an ordinary **bearer token from the
//! deployment's own sign-in**, which is the whole point of "use what the
//! door already accepts": there is no operator credential, no API key and
//! no second notion of an administrator with its own secret. An operator
//! gets one by signing in as a principal who is an owner of the
//! deployment's own space:
//!
//! ```text
//! curl -s https://deploy.example.com/auth/begin \
//!   -H 'content-type: application/json' \
//!   -d '{"method":"password","identifier":"you@example.com"}'
//! curl -s https://deploy.example.com/auth/complete \
//!   -H 'content-type: application/json' \
//!   -d '{"challenge":"…","password":"…"}'      # → {"token":"…"}
//! export ORDO_TOKEN=…
//! ```
//!
//! Two properties of that token decide what this mode can do, and both are
//! the door's rather than this module's:
//!
//! - **It carries presence.** `std.deployment.apply` is step-up gated like
//!   every platform op, so a token minted without a fresh presence claim is
//!   refused. A sign-in produces one; `/auth/refresh` continues it.
//! - **It cannot be a capability token.** §5 pins that a capability token
//!   can never satisfy step-up, so no third-party client and no delegated
//!   agent can apply a schema however its allowlist reads. That is the
//!   mechanism, not a policy this module implements.
//!
//! # What is asserted, and what is not
//!
//! `--as` is refused in this mode. On the database path it is an assertion
//! an operator holding credentials makes about themselves; over the door
//! the actor written into `$migrations` is the principal
//! `IdentityAccess::check` proved. Taking the flag and ignoring it would
//! put a string in a command line that does not reach the audit record it
//! looks like it is writing.

use ordo_blob::http::{self, Endpoint, Request};
use serde_json::{json, Value};

use crate::operator::{Flags, Outcome};

/// The host an empty `--host` selects: the address `ordo dev` serves on.
/// Local until the project has a public domain (owner, 2026-10-31).
pub const DEFAULT_HOST: &str = "http://127.0.0.1:4000";

/// The environment variable `--host` overrides, spelled like the rest of
/// the operator surface's (`ORDO_PG_HOST`, `ORDO_MASTER_KEY`,
/// `ORDO_CHECKPOINT_KEY_ID`).
pub const HOST_ENV: &str = "ORDO_HOST";
/// The bearer token `--token` overrides.
pub const TOKEN_ENV: &str = "ORDO_TOKEN";
/// The space the op is bound to, when `--space` is not given.
pub const SPACE_ENV: &str = "ORDO_SPACE";

/// Where the deployment is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub endpoint: Endpoint,
}

impl Host {
    /// `https://h[:p]`, `http://h[:p]`, or a bare `h[:p]`, which gets
    /// `https`. See the module doc for why a bare host defaults here and is
    /// refused in `ordo_blob`.
    pub fn parse(s: &str) -> Result<Host, String> {
        let s = s.trim();
        let with_scheme = if s.contains("://") {
            s.to_string()
        } else {
            format!("https://{s}")
        };
        Endpoint::parse(&with_scheme)
            .map(|endpoint| Host { endpoint })
            .map_err(|e| format!("--host: {e}"))
    }

    pub fn url(&self, path: &str) -> String {
        let scheme = if self.endpoint.tls { "https" } else { "http" };
        format!("{scheme}://{}{path}", self.endpoint.authority())
    }
}

/// Did this invocation ask for the door?
///
/// `Some` when `--host` was given or `ORDO_HOST` is set. `None` is the
/// database path, which stays the default: an operator who has always run
/// `ordo apply --db …` must not start talking to a public hostname because
/// a flag grew a default. An empty `--host` selects [`DEFAULT_HOST`].
pub fn requested(flags: &Flags) -> Option<Result<Host, String>> {
    let raw = flags
        .named_value("--host")
        .map(str::to_string)
        .or_else(|| {
            std::env::var(HOST_ENV)
                .ok()
                .filter(|v| !v.trim().is_empty())
        })?;
    if raw.trim().is_empty() {
        return Some(Host::parse(DEFAULT_HOST));
    }
    Some(Host::parse(&raw))
}

/// The bearer token, or the refusal that says how to get one.
///
/// `--token`, then `ORDO_TOKEN`, then a machine key (`ORDO_MACHINE_KEY` or
/// `ORDO_MACHINE_KEY_FILE`, with `ORDO_IDENTITY_URL`), then the session
/// `ordo login` stored. See [`crate::signin`].
pub(crate) async fn token(flags: &Flags) -> Result<String, String> {
    let ctx = crate::signin::Ctx::from_process();
    crate::signin::credential(&ctx, flags.named_value("--token"), None)
        .await
        .map(|c| c.token)
        .map_err(|e| e.render())
}

/// The space the op binds. The deployment's own, the only space
/// `std.deployment.apply` and `std.space.create` answer in, shared by
/// every door command this module has, because both are `owner_only` and
/// narrower still: check 3 of each op's own gate refuses any space but the
/// deployment's own.
fn space(flags: &Flags) -> Result<String, String> {
    flags
        .named_value("--space")
        .map(str::to_string)
        .or_else(|| {
            std::env::var(SPACE_ENV)
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .ok_or_else(|| {
            format!(
                "talking to the door names the deployment's OWN space: `--space <s>` or \
                 {SPACE_ENV}.\nIt is the space the binary was configured with, not a tenant's \
                 — the op refuses any other,\nbecause owning a space you were given is not \
                 owning the deployment that serves it.\n"
            )
        })
}

/// `ordo apply --host …`: POST the program, render what comes back.
pub async fn apply_over_the_wire(
    host: &Host,
    flags: &Flags,
    source: &str,
    activate: bool,
    path: &str,
    db: &str,
    wait: Option<std::time::Duration>,
) -> Outcome {
    let token = match token(flags).await {
        Ok(t) => t,
        Err(e) => return Outcome::usage(e),
    };
    let space = match space(flags) {
        Ok(s) => s,
        Err(e) => return Outcome::usage(e),
    };
    // The envelope the door reads: the op's parameters at the top level,
    // `$space` beside them. `activate` is sent explicitly rather than
    // omitted when true, so the request says what it means and a default
    // that ever moved would not silently change what a stored script does.
    let body = json!({
        "$space": space,
        "program": source,
        "activate": activate,
    })
    .to_string();

    let (status, value) = match post_touching(
        host,
        &format!("/op/{}", ordo_host::apply_op::APPLY),
        flags,
        &token,
        &space,
        body,
        // `std.deployment.apply` does not require the header, and this does
        // not start sending one: an apply is idempotent by its own content
        // (the artifact is stored under its version hash and activation is
        // a re-point), so a key would add a replay cache to a path that
        // already converges, and changing that is not this increment's.
        None,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return Outcome::failed(format!(
                "REFUSED  {}: {e}\n",
                host.url(&format!("/op/{}", ordo_host::apply_op::APPLY))
            ))
        }
    };

    if status >= 400 {
        return door_refusal(status, &value);
    }

    let row = match value["rows"].as_array().and_then(|r| r.first()) {
        Some(row) => row,
        None => {
            return Outcome::failed(format!(
                "REFUSED  the deployment answered {status} with no result row: {value}\n"
            ))
        }
    };
    match ordo_host::apply_op::Report::from_json(row) {
        // The same renderer the direct path uses, over the same value. See
        // `operator::render_apply`.
        Ok(report) => {
            let out = crate::operator::render_apply(&report, path, db);
            match (wait, crate::operator::served_version(&report)) {
                (Some(limit), Some(version)) => {
                    let ask = format!("/health/deploy?version={version}");
                    // A door that has no `/health/deploy` (one built before it,
                    // or a test door without it) cannot say when it serves the
                    // version, so there is nothing to wait on.
                    if let Ok((404, _)) = get(host, &ask).await {
                        return Outcome::ok(
                            out + "waited   not at all: this door has no /health/deploy, so it cannot say when it serves the version\n",
                        );
                    }
                    match crate::deploy_wait::until_served(&version, limit, || {
                        let ask = ask.clone();
                        async move {
                            let (status, body) = get(host, &ask).await?;
                            if status >= 400 {
                                return Err(format!("{} answered {status}", host.url(&ask)));
                            }
                            Ok(crate::deploy_wait::seen_from_status(&body))
                        }
                    })
                    .await
                    {
                        Ok(lines) => Outcome::ok(out + &lines),
                        Err(lines) => Outcome::failed(out + &lines),
                    }
                }
                _ => Outcome::ok(out),
            }
        }
        Err(e) => Outcome::failed(format!("REFUSED  {e}\n")),
    }
}

/// A plain GET with no credential, for `/health/deploy`.
async fn get(host: &Host, path: &str) -> Result<(u16, Value), String> {
    use futures::StreamExt;

    let response = http::send(
        &host.endpoint,
        Request {
            method: "GET",
            path: path.to_string(),
            headers: vec![("accept".into(), "application/json".into())],
            body: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let status = response.status;
    let mut out = Vec::new();
    let mut stream = response.body;
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
    }
    let value = serde_json::from_slice(&out).unwrap_or(Value::Null);
    Ok((status, value))
}

/// The door's uniform refusal envelope, rendered as the class and the
/// message it carries.
///
/// Shared by every op this module calls over the wire, because the
/// envelope is the router's, not the op's: `{error: {code, message}}` for
/// whichever op refused. The class is the SAME word the op's own error
/// type prints on the direct path where one exists (`ApplyRefused`'s
/// `Display`/`code()` for `apply`; `tenant_ops::refusal_to_error`'s message
/// for `space create`, which has no direct path to agree with, only its
/// own words). A refusal the client cannot read is printed whole rather
/// than summarised, because a summary of something unrecognised is a
/// guess.
fn door_refusal(status: u16, value: &Value) -> Outcome {
    let code = value["error"]["code"].as_str();
    let message = value["error"]["message"].as_str();
    Outcome::failed(match (code, message) {
        (Some(code), Some(message)) => {
            let mut out = format!("REFUSED  [{code}] {message}\n");
            if let Some(next) = door_next(code) {
                out.push_str(&format!("         next: {next}\n"));
            }
            out
        }
        _ => format!("REFUSED  {status}: {value}\n"),
    })
}

/// What to do about a door refusal, for the classes a person signed in
/// with `ordo login` fixes alone. Every class here is in the wire
/// inventory (`api-engine-design.md`); a test holds that.
pub(crate) const DOOR_HINTS: &[(&str, &str)] = &[
    (
        "presence_required",
        "run `ordo login --touch`, then run this again",
    ),
    (
        "access_denied",
        "if your sign-in ended, run `ordo login`; otherwise ask an admin of this deployment",
    ),
];

fn door_next(code: &str) -> Option<&'static str> {
    DOOR_HINTS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// `ordo space create --host …`: POST `$name`/`$owner` to `std.space.create`
/// and print the minted tenant.
///
/// # Why this has no `apply_over_the_wire`-style database twin
///
/// `apply` has two modes because it had one first: a direct path that
/// opened Postgres, and 18l gave it a door later. `std.space.create` was
/// born with only the door, per `tenant_ops.rs`'s module doc ("a tenant
/// comes into being through a call at the door"), so there is nothing for
/// this function to fall back to and no `requested()`-is-`None` case that
/// means "use the database instead". `None` here is a usage error.
///
/// # What the caller supplies, and what comes back
///
/// `--name` and `--owner` are the op's whole input; `--host`, `--token`
/// and `--space` are the same three every door command in this module
/// takes, resolved the same way. The op mints the space id, so there is
/// nothing to mint client-side and nothing to retry with a different id on
/// a network failure. A caller who is unsure whether a create landed
/// re-runs `ordo space create` and gets a second tenant, which is why the
/// printed id is the one thing worth reading twice.
pub async fn create_space_over_the_wire(flags: &Flags) -> Outcome {
    let host = match requested(flags) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return Outcome::usage(e),
        None => {
            return Outcome::usage(format!(
                "`ordo space create` has no database mode: `--host <h>` or {HOST_ENV} is \
                 required.\nMinting a tenant mints custody, writes a founding owner, and \
                 tells every replica to rebuild\nits access engine. A tool holding a \
                 database password could do the first two and\nnot the third, so this one \
                 is the deployment's act and there is no local path to it.\n"
            ));
        }
    };
    let token = match token(flags).await {
        Ok(t) => t,
        Err(e) => return Outcome::usage(e),
    };
    let space = match space(flags) {
        Ok(s) => s,
        Err(e) => return Outcome::usage(e),
    };
    let name = match flags.require("--name") {
        Ok(n) => n.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let owner = match flags.require("--owner") {
        Ok(o) => o.to_string(),
        Err(e) => return Outcome::usage(e),
    };

    let body = json!({
        "$space": space,
        "name": name,
        "owner": owner,
    })
    .to_string();

    // **`std.space.create` requires an `Idempotency-Key`** (`bootstrap.md`
    // §2 item 44), because its effect reaches outside the database it writes
    // to: a repeat mints a second KMS key and a second database, answers with
    // the second tenant's id, and leaves the first for somebody to find by
    // reading the cluster.
    //
    // Minted per invocation unless `--idempotency-key` pins it, and the key
    // used is **printed**, which is the point of the flag: a run that dies
    // after the door accepted it leaves an operator unable to tell "no tenant
    // was made" from "a tenant was made and I did not see the answer". Re-run
    // with the same key and the door replays the first answer instead of
    // building a second tenant.
    let key = match flags.named.get("--idempotency-key") {
        Some(k) => k.clone(),
        None => format!(
            "ordo-space-create-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ),
    };

    let op_path = format!("/op/{}", ordo_host::tenant_ops::CREATE);
    let (status, value) =
        match post_touching(&host, &op_path, flags, &token, &space, body, Some(&key)).await {
            Ok(v) => v,
            // **The key is printed on the failure**, which is the case it is for.
            // A transport error leaves an operator unable to tell "the door never
            // saw it" from "the door built a tenant and the answer was lost", and
            // those need opposite next steps. Re-running with this key settles it
            // without risking a second tenant.
            Err(e) => {
                return Outcome::failed(format!(
                    "REFUSED  {}: {e}\n         re-run with --idempotency-key {key} to \
                 learn whether this one landed, rather than building a second tenant\n",
                    host.url(&op_path)
                ))
            }
        };

    if status >= 400 {
        return door_refusal(status, &value);
    }

    let row = match value["rows"].as_array().and_then(|r| r.first()) {
        Some(row) => row,
        None => {
            return Outcome::failed(format!(
                "REFUSED  the deployment answered {status} with no result row: {value}\n"
            ))
        }
    };
    render_created(row)
}

/// The created tenant, as an operator reads it.
///
/// `reachable` carries the one thing a caller cannot otherwise tell.
/// `tenant_ops.rs`'s module doc says why the op has to be honest about it,
/// so this is the one field with two shapes rather than a string: `true`
/// once every replica has rebuilt its door, or `{at, why}` when the
/// announce did not land and the tenant waits for a restart.
fn render_created(row: &Value) -> Outcome {
    let space = row["space"].as_str().unwrap_or("?");
    let name = row["name"].as_str().unwrap_or("?");
    let owner = row["owner"].as_str().unwrap_or("?");
    let mut out = format!("space      {space}\nname       {name}\nowner      {owner}\n");
    match &row["reachable"] {
        Value::Bool(true) => {
            out.push_str("reachable  now        every replica has rebuilt its door\n")
        }
        Value::Object(o) => {
            let why = o
                .get("why")
                .and_then(Value::as_str)
                .unwrap_or("the announce did not say");
            out.push_str(&format!(
                "reachable  at next restart  (the door was not rebuilt live: {why})\n"
            ));
        }
        other => out.push_str(&format!("reachable  {other}\n")),
    }
    Outcome::ok(out)
}

/// One POST, one connection, the body read whole.
///
/// The body is collected rather than streamed because it is one result row.
/// **Bounded**, at a megabyte: a deployment that answers an apply with more
/// than that is not answering this call, and reading an unbounded body into
/// memory on an operator's laptop because the far end is confused is how a
/// CLI becomes the incident.
/// `idempotency` is `Some` for an op that requires the header
/// (`OpSpec::requires_idempotency_key` — `std.space.create` is the only one)
/// and for any caller that wants a retry to replay rather than repeat.
/// `GET path` with the bearer token, the answer read as JSON. Used by
/// `ordo triggers`, which asks a running deployment what it serves.
pub(crate) async fn get_json(host: &Host, path: &str, token: &str) -> Result<(u16, Value), String> {
    get_json_in(host, path, token, None).await
}

/// [`get_json`], bound to `space` when one is given: a read op through the
/// door, which a read answers only as a `GET`.
async fn get_json_in(
    host: &Host,
    path: &str,
    token: &str,
    space: Option<&str>,
) -> Result<(u16, Value), String> {
    use futures::StreamExt;

    let mut headers = vec![
        ("authorization".into(), format!("Bearer {token}")),
        ("accept".into(), "application/json".into()),
    ];
    if let Some(space) = space {
        headers.push((ordo_api::router::SPACE_HEADER.into(), space.to_string()));
    }
    let response = http::send(
        &host.endpoint,
        Request {
            method: "GET",
            path: path.to_string(),
            headers,
            body: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let status = response.status;
    let mut out = Vec::new();
    let mut stream = response.body;
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        if out.len() > 1024 * 1024 {
            return Err("the deployment's answer ran past a megabyte".into());
        }
    }
    let value: Value = serde_json::from_slice(&out).map_err(|e| {
        format!(
            "the deployment answered {status} with something that is not JSON ({e}): {}",
            String::from_utf8_lossy(&out[..out.len().min(512)])
        )
    })?;
    Ok((status, value))
}

/// [`post`], and on `presence_required` at a terminal with a session
/// token, the `--touch` flow and the same call once more
/// ([`crate::signin::touch_if_interactive`]). Anywhere else the refusal is
/// returned as it came, and its hint says what to run.
#[allow(clippy::too_many_arguments)]
async fn post_touching(
    host: &Host,
    path: &str,
    flags: &Flags,
    token: &str,
    space: &str,
    body: String,
    idempotency: Option<&str>,
) -> Result<(u16, Value), String> {
    let first = post(host, path, token, space, body.clone(), idempotency).await?;
    if first.0 != 403 || first.1["error"]["code"].as_str() != Some("presence_required") {
        return Ok(first);
    }
    let ctx = crate::signin::Ctx::from_process();
    match crate::signin::touch_if_interactive(&ctx, flags.named_value("--token"), None).await {
        Ok(true) => {
            let token = self::token(flags).await?;
            post(host, path, &token, space, body, idempotency).await
        }
        Ok(false) => Ok(first),
        Err(e) => Err(e.render()),
    }
}

async fn post(
    host: &Host,
    path: &str,
    token: &str,
    space: &str,
    body: String,
    idempotency: Option<&str>,
) -> Result<(u16, Value), String> {
    use futures::StreamExt;

    let len = body.len() as u64;
    let bytes = body.into_bytes();
    let stream: ordo_blob::ByteStream = Box::pin(futures::stream::once(async move { Ok(bytes) }));
    let response = http::send(
        &host.endpoint,
        Request {
            method: "POST",
            path: path.to_string(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), format!("Bearer {token}")),
                // Sent as well as carried in the envelope. The door refuses
                // the two disagreeing (`missing_space`), which is the check
                // that catches a client that built one of them wrong.
                (ordo_api::router::SPACE_HEADER.into(), space.to_string()),
                ("accept".into(), "application/json".into()),
            ]
            .into_iter()
            .chain(idempotency.map(|k| ("idempotency-key".to_string(), k.to_string())))
            .collect(),
            body: Some((len, stream)),
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    let status = response.status;
    let mut out = Vec::new();
    let mut stream = response.body;
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        if out.len() > 1024 * 1024 {
            return Err("the deployment's answer ran past a megabyte".into());
        }
    }
    let value: Value = serde_json::from_slice(&out).map_err(|e| {
        format!(
            "the deployment answered {status} with something that is not JSON ({e}): {}",
            String::from_utf8_lossy(&out[..out.len().min(512)])
        )
    })?;
    Ok((status, value))
}

/// `ordo import --bundle <b> --name <n> --host <h> --token <t> --space <s>`:
/// load a staged bundle through `std.space.import`.
///
/// **No database mode, and here the reason is custody rather than
/// announcement.** `space create`'s doc says a tool holding a database
/// password could mint the keys and not tell the replicas. An import's
/// version of that is sharper: every sealed field in the bundle has to be
/// re-sealed under the destination space's key, and a CLI has no way to hold
/// one. That is 17y finding 1 in one sentence, and it is why the engine sat
/// unreachable from the day it was built: *an import that cannot seal cannot
/// write a sealed field*. The deployment holds custody, so the deployment
/// runs the import and this command asks it to.
pub async fn import_over_the_wire(flags: &Flags) -> Outcome {
    let host = match requested(flags) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return Outcome::usage(e),
        None => {
            return Outcome::usage(format!(
                "`ordo import` has no database mode: `--host <h>` or {HOST_ENV} is \
                 required.\nEvery sealed field in a bundle is re-sealed under the \
                 destination space's key, and this\ntool has no way to hold one. The \
                 deployment does, so the import is the deployment's act.\n"
            ));
        }
    };
    let token = match token(flags).await {
        Ok(t) => t,
        Err(e) => return Outcome::usage(e),
    };
    let space = match space(flags) {
        Ok(s) => s,
        Err(e) => return Outcome::usage(e),
    };
    let bundle = match flags.require("--bundle") {
        Ok(b) => b.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    // Defaulted to the bundle's own name, which is the common case and is
    // still explicit in the report. Two runs naming one bundle resume each
    // other, which is what an operator re-running after a lost connection
    // wants; two different `--name`s over one bundle are two imports, which
    // is what somebody loading the same bundle into a second space wants.
    let name = flags
        .named
        .get("--name")
        .cloned()
        .unwrap_or_else(|| bundle.clone());

    let body = json!({
        "$space": space,
        "bundle": bundle,
        "name": name,
    })
    .to_string();

    let op_path = format!("/op/{}", ordo_host::exit_ops::IMPORT);
    // **No `Idempotency-Key`, and the import's name is why.** The op does not
    // require one (`platform_ops`'s spec says so and gives the argument): the
    // batch leases in `$replay` are keyed by this import's name and
    // fingerprinted by their source ids, so a re-run under the same name
    // resumes. It does NOT refuse a changed bundle whose ids are unchanged:
    // the fingerprint covers source ids only (security-review-6.md S6-006). A header would
    // add a second, weaker answer to a question the parameter already settles.
    let (status, value) =
        match post_touching(&host, &op_path, flags, &token, &space, body, None).await {
            Ok(v) => v,
            // Re-running is the right move and is safe, which is the opposite of
            // `space create`'s advice and for the opposite reason: the name makes
            // the retry a resume rather than a second act.
            Err(e) => {
                return Outcome::failed(format!(
                    "REFUSED  {}: {e}\n         re-run with the same --name to resume \
                 whatever landed; it will not import twice\n",
                    host.url(&op_path)
                ))
            }
        };

    if status >= 400 {
        return door_refusal(status, &value);
    }

    let row = match value["rows"].as_array().and_then(|r| r.first()) {
        Some(row) => row,
        None => {
            return Outcome::failed(format!(
                "REFUSED  the deployment answered {status} with no result row: {value}\n"
            ))
        }
    };
    render_imported(row)
}

/// What an import did, as an operator reads it.
///
/// **The skipped counts are printed even when they are zero.** A resume is
/// the case this command exists to make safe, and an operator who cannot see
/// that a run skipped 900 batches cannot tell a resume from a fresh import
/// that found nothing to do.
fn render_imported(row: &Value) -> Outcome {
    let n = |k: &str| row[k].as_u64().unwrap_or(0);
    let mut out = format!(
        "IMPORTED {} rows into {}\n",
        n("rows"),
        row["space"].as_str().unwrap_or("?"),
    );
    out.push_str(&format!(
        "         bundle {}  as `{}`\n",
        row["bundle"].as_str().unwrap_or("?"),
        row["name"].as_str().unwrap_or("?"),
    ));
    if let Some(entities) = row["entities"].as_array() {
        for e in entities {
            out.push_str(&format!(
                "           {:<24} {} rows\n",
                e["entity"].as_str().unwrap_or("?"),
                e["rows"].as_u64().unwrap_or(0),
            ));
        }
    }
    out.push_str(&format!(
        "         {} command groups · {} batches written, {} already there\n",
        n("groups"),
        n("batchesWritten"),
        n("batchesSkipped"),
    ));
    out.push_str(&format!(
        "         {} people mapped from the staged `.people` file\n",
        n("peopleMapped"),
    ));
    out.push_str(&format!(
        "         {} objects attached, {} already there\n",
        n("objectsAttached"),
        n("objectsSkipped"),
    ));
    Outcome::ok(out)
}

/// `ordo console-key revoke --id <ck-…> --host <h> --token <t> --space <s>`:
/// revoke a console's key at the deployment (`plan-console.md` 8.2), with
/// `std.deployment.revokeConsoleKey`. The deployment ends every session the
/// key signed in at once. The token is the deployment owner's own sign-in
/// with a passkey touch: this op can never be exempted from step-up.
pub async fn revoke_console_key_over_the_wire(flags: &Flags) -> Outcome {
    let host = match requested(flags) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return Outcome::usage(e),
        None => {
            return Outcome::usage(format!(
                "`ordo console-key revoke` talks to the deployment's door: `--host <h>` or \
                 {HOST_ENV} is required\n"
            ))
        }
    };
    let token = match token(flags).await {
        Ok(t) => t,
        Err(e) => return Outcome::usage(e),
    };
    let space = match space(flags) {
        Ok(s) => s,
        Err(e) => return Outcome::usage(e),
    };
    let id = match flags.require("--id") {
        Ok(i) => i.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let op_path = format!("/op/{}", ordo_host::console_keys::REVOKE);
    let body = json!({"$space": space, "id": id}).to_string();
    let (status, value) =
        match post_touching(&host, &op_path, flags, &token, &space, body, None).await {
            Ok(v) => v,
            Err(e) => return Outcome::failed(format!("REFUSED  {}: {e}\n", host.url(&op_path))),
        };
    if status >= 400 {
        return door_refusal(status, &value);
    }
    let row = &value["rows"][0];
    Outcome::ok(format!(
        "REVOKED  console key {} ({}); {} open session(s) ended\n",
        row["id"].as_str().unwrap_or(&id),
        row["name"].as_str().unwrap_or("unnamed"),
        row["sessionsEnded"].as_u64().unwrap_or(0)
    ))
}

/// The host, the token and the space a secret command talks to the door
/// with, or the refusal that says which is missing.
async fn secret_door(flags: &Flags) -> Result<(Host, String, String), Outcome> {
    let host = match requested(flags) {
        Some(Ok(h)) => h,
        Some(Err(e)) => return Err(Outcome::usage(e)),
        None => {
            return Err(Outcome::usage(format!(
                "`ordo secret` talks to the deployment's door: `--host <h>` or {HOST_ENV} is \
                 required\n"
            )))
        }
    };
    let token = token(flags).await.map_err(Outcome::usage)?;
    let space = space(flags).map_err(Outcome::usage)?;
    Ok((host, token, space))
}

/// The hosts a listed secret is bound to, in words.
fn bound_words(row: &Value) -> String {
    let hosts: Vec<&str> = row["hosts"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if hosts.is_empty() {
        "bound to no host: the program declares no `secret` by this name".into()
    } else {
        format!("bound to {}", hosts.join(", "))
    }
}

/// `ordo secret set <name>`: seal `value` as the secret's next version
/// (`std.deployment.setSecret`). The value came from standard input; one
/// trailing line end is dropped, as a file form drops it.
pub async fn set_secret_over_the_wire(flags: &Flags, name: &str, mut value: Vec<u8>) -> Outcome {
    if value.ends_with(b"\n") {
        value.pop();
        if value.ends_with(b"\r") {
            value.pop();
        }
    }
    if value.is_empty() {
        return Outcome::usage(
            "standard input held no value; pipe one in: `ordo secret set <name> < key.txt`\n"
                .into(),
        );
    }
    let Ok(text) = String::from_utf8(value) else {
        return Outcome::usage("a secret's value is UTF-8 text; standard input was not\n".into());
    };
    let (host, token, space) = match secret_door(flags).await {
        Ok(d) => d,
        Err(o) => return o,
    };
    let op_path = format!("/op/{}", ordo_host::secrets::SET);
    let body = json!({"$space": space, "name": name, "value": text}).to_string();
    drop(text);
    let (status, value) =
        match post_touching(&host, &op_path, flags, &token, &space, body, None).await {
            Ok(v) => v,
            Err(e) => return Outcome::failed(format!("REFUSED  {}: {e}\n", host.url(&op_path))),
        };
    if status >= 400 {
        return door_refusal(status, &value);
    }
    let row = &value["rows"][0];
    Outcome::ok(format!(
        "SET      secret {} version {}; {}\n",
        row["name"].as_str().unwrap_or(name),
        row["version"].as_u64().unwrap_or(0),
        bound_words(row)
    ))
}

/// `ordo secret ls`: every secret with a value, its bound hosts, version,
/// and who set it when (`std.deployment.secrets`). Never a value.
pub async fn list_secrets_over_the_wire(flags: &Flags) -> Outcome {
    let (host, token, space) = match secret_door(flags).await {
        Ok(d) => d,
        Err(o) => return o,
    };
    let op_path = format!("/op/{}", ordo_host::secrets::LIST);
    let (status, value) = match get_json_in(&host, &op_path, &token, Some(&space)).await {
        Ok(v) => v,
        Err(e) => return Outcome::failed(format!("REFUSED  {}: {e}\n", host.url(&op_path))),
    };
    if status >= 400 {
        return door_refusal(status, &value);
    }
    let rows = value["rows"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return Outcome::ok(
            "no secrets are set; `ordo secret set <name> < key.txt` sets one\n".into(),
        );
    }
    let mut out = String::new();
    for row in &rows {
        out.push_str(&format!(
            "{}  version {}  set by {} at {}  {}\n",
            row["name"].as_str().unwrap_or("?"),
            row["version"].as_u64().unwrap_or(0),
            row["setBy"].as_str().unwrap_or("?"),
            row["setAt"].as_u64().unwrap_or(0),
            bound_words(row)
        ));
    }
    Outcome::ok(out)
}

/// `ordo secret rm <name>`: remove the secret's value
/// (`std.deployment.removeSecret`). A function that uses it is then refused
/// with `secret_not_set` until it is set again.
pub async fn remove_secret_over_the_wire(flags: &Flags, name: &str) -> Outcome {
    let (host, token, space) = match secret_door(flags).await {
        Ok(d) => d,
        Err(o) => return o,
    };
    let op_path = format!("/op/{}", ordo_host::secrets::REMOVE);
    let body = json!({"$space": space, "name": name}).to_string();
    let (status, value) =
        match post_touching(&host, &op_path, flags, &token, &space, body, None).await {
            Ok(v) => v,
            Err(e) => return Outcome::failed(format!("REFUSED  {}: {e}\n", host.url(&op_path))),
        };
    if status >= 400 {
        return door_refusal(status, &value);
    }
    let row = &value["rows"][0];
    Outcome::ok(format!(
        "REMOVED  secret {} (it was version {})\n",
        row["name"].as_str().unwrap_or(name),
        row["version"].as_u64().unwrap_or(0)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_door_class_given_a_hint_is_in_the_wire_inventory() {
        const DOC: &str = include_str!("../../../docs/api-engine-design.md");
        for (code, _) in DOOR_HINTS {
            assert!(
                DOC.lines()
                    .any(|l| l.trim().starts_with(&format!("| `{code}` |"))),
                "`{code}` has a hint here and is in no wire inventory"
            );
        }
        let out = door_refusal(
            403,
            &json!({"error": {"code": "presence_required", "message": "m"}}),
        );
        assert!(out.report.contains("ordo login --touch"), "{}", out.report);
    }

    #[test]
    fn a_bare_host_gets_https_and_a_spelled_scheme_is_kept() {
        assert_eq!(
            Host::parse("deploy.example.com").unwrap().endpoint,
            Endpoint {
                host: "deploy.example.com".into(),
                port: 443,
                tls: true
            }
        );
        // The local `ordo dev` case: plaintext, spelled, and not upgraded.
        assert_eq!(
            Host::parse("http://127.0.0.1:4000").unwrap().endpoint,
            Endpoint {
                host: "127.0.0.1".into(),
                port: 4000,
                tls: false
            }
        );
        assert_eq!(
            Host::parse("https://ordo.example.com:8443")
                .unwrap()
                .endpoint,
            Endpoint {
                host: "ordo.example.com".into(),
                port: 8443,
                tls: true
            }
        );
        // A bare host with a port is still https.
        assert!(Host::parse("ordo.example.com:8443").unwrap().endpoint.tls);
    }

    #[test]
    fn a_host_with_a_path_is_refused_rather_than_truncated() {
        // `ordo_blob`'s parser's rule, inherited: the op path is the
        // client's, so a host that carries one is a mistake rather than a
        // prefix to honour.
        assert!(Host::parse("https://ordo.example.com/api").is_err());
        assert!(Host::parse("https://").is_err());
        assert!(Host::parse("https://h:not-a-port").is_err());
    }

    #[test]
    fn an_empty_host_selects_the_local_dev_address() {
        let mut flags = crate::operator::Flags::default();
        flags.named.insert("--host".into(), "".into());
        let host = requested(&flags).unwrap().unwrap();
        assert_eq!(host.url("/op/x"), "http://127.0.0.1:4000/op/x");
    }

    #[test]
    fn the_url_round_trips_the_scheme_and_omits_a_default_port() {
        assert_eq!(
            Host::parse("deploy.example.com").unwrap().url("/op/x"),
            "https://deploy.example.com/op/x"
        );
        assert_eq!(
            Host::parse("http://127.0.0.1:4000").unwrap().url("/op/x"),
            "http://127.0.0.1:4000/op/x"
        );
    }
}
