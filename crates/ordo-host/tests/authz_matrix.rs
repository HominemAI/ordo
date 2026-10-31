//! The authorization matrix, against a real deployment.
//!
//! The `ordo-host` binary boots on real Postgres with the fixture program
//! `authz_support/authz.ordo`. `authz_support::actors::build` makes every
//! actor through the real sign-in paths and every object through the real
//! ops. Then the suite runs, as cells:
//!
//! - **Ops × actors** ([`op_cells`]): every op the door may serve (the
//!   program's registry and `platform_ops::specs()`), times every actor, as
//!   a probe — the op's own HTTP method with one unknown parameter, so the
//!   door's decision shows (a parameter refusal means the caller got in) and
//!   nothing runs. The policy for each cell is [`policy::expect_op`].
//! - **Routes × actors** ([`route_cells`]): every route `runtime::door`
//!   mounts other than the op door, times every actor, by
//!   [`policy::expect_route`].
//! - **Header variations** ([`header_cells`]): one op, with `X-Ordo-Space`
//!   naming the other tenant, a foreign `Origin`, a missing bearer, a
//!   garbled bearer, and a bearer from another deployment's key.
//! - **Rows** ([`ROWS`]): hand-written attack requests built from actors and
//!   objects, each with a safe answer. The extension point for a pen
//!   tester's catalog; empty today, and the format is in `rows.rs` once
//!   there is one to replay.
//!
//! Every cell names its expected outcome ([`policy::Expect`]) and a mark
//! ([`policy::Mark`]): `Holds` for documented behaviour, `KnownDefect` for a
//! cell whose real answer is a security defect (the suite still passes and
//! the defect is listed), `Unspecified` for one no document decides. A
//! `Holds` cell whose answer is not the expected one is a surprise and
//! fails. A marked cell whose answer IS the expected one is also a surprise:
//! the defect is fixed or the question answered, and the table must say so.
//!
//! **The route guard** ([`guard_every_route_and_op_is_in_the_policy`]): the
//! policy is read against the routes `runtime::door` actually mounts and the
//! ops the program and platform serve. A route or op with no policy fails
//! the suite, so a new one cannot ship untested.
//!
//! It boots one deployment and builds the world once (~20 s) and runs in
//! about 40 s, so it is `#[ignore]` and CI runs it by name
//! (`scripts/authz-matrix.sh`). Needs its own Postgres cluster
//! (`ORDO_PGDATA=$HOME/.ordo-pg/authz-matrix`, see the script).

mod authz_support;

use authz_support::actors::{self, World};
use authz_support::policy::{self, Expect, Mark};
use authz_support::{routes, Answer, Deployment, FOREIGN, PAGE};
use serde_json::json;

/// One cell's judgement.
struct Cell {
    what: String,
    expect: Expect,
    mark: Mark,
    answer: Answer,
}

impl Cell {
    /// Did the door do what the policy said?
    fn matches(&self) -> bool {
        self.expect.holds(&self.answer)
    }

    /// A line for the diff table: what the door actually answered.
    fn saw(&self) -> String {
        let code = self.answer.code();
        if code.is_empty() {
            format!("{}", self.answer.status)
        } else {
            format!("{}/{}", self.answer.status, code)
        }
    }
}

/// The verdict over every cell: the surprises, and the marks that held.
#[derive(Default)]
struct Verdict {
    cells: usize,
    /// A `Holds` cell whose answer was not expected, or a marked cell whose
    /// answer WAS expected (the mark is stale).
    surprises: Vec<String>,
    defects: std::collections::BTreeMap<String, Vec<String>>,
    questions: std::collections::BTreeMap<String, Vec<String>>,
}

impl Verdict {
    fn judge(&mut self, cell: Cell) {
        self.cells += 1;
        let ok = cell.matches();
        match cell.mark {
            Mark::Holds => {
                if !ok {
                    self.surprises.push(format!(
                        "{}: expected {:?}, saw {}",
                        cell.what,
                        cell.expect,
                        cell.saw()
                    ));
                }
            }
            Mark::KnownDefect(id) => {
                if ok {
                    self.defects
                        .entry(id.to_string())
                        .or_default()
                        .push(format!("{} (saw {})", cell.what, cell.saw()));
                } else {
                    self.surprises.push(format!(
                        "{}: marked KNOWN_DEFECT {id} but the defect is gone (expected {:?}, saw {})",
                        cell.what,
                        cell.expect,
                        cell.saw()
                    ));
                }
            }
            Mark::Unspecified(id) => {
                self.questions
                    .entry(id.to_string())
                    .or_default()
                    .push(format!("{} (saw {})", cell.what, cell.saw()));
            }
        }
    }
}

/// Every op the door may serve, probed by every actor in its own space.
fn op_cells(d: &Deployment, w: &World, v: &mut Verdict) {
    let program = ordo_conformance::Fixture::from_source(authz_support::SOURCE, "").unwrap();
    for (op, kind) in routes::ops(&program) {
        let (_, method) = ordo_api::router::method_for(kind);
        for (actor, _) in policy::ACTORS {
            let a = w.actor(actor);
            let token = a.token.clone().unwrap_or_default();
            // The identity service's tokens meet a different answer in the
            // home space and in a tenant (plan 4.3), so each is probed in both.
            let spaces: Vec<&str> = if policy::in_both_spaces(actor) {
                vec![authz_support::HOME, w.var("T1")]
            } else {
                vec![a.space.as_str()]
            };
            for space in spaces {
                let answer =
                    actors::op(d, space, &token, method, &op, &json!({"zzAuthzProbe": "1"}));
                let (expect, mark) = policy::expect_op_in(&op, kind, actor, space);
                v.judge(Cell {
                    what: format!("op {op} [{actor}] in {space}"),
                    expect,
                    mark,
                    answer,
                });
            }
        }
    }
}

/// Every mounted route other than the op door, probed by every actor.
fn route_cells(d: &Deployment, w: &World, v: &mut Verdict) {
    for r in routes::mounted() {
        if r.path == "/op/{name}" {
            continue;
        }
        let method = if r.method == "ANY" {
            "GET".to_string()
        } else {
            r.method.clone()
        };
        // A spent `/auth/logout` would end a live actor's session mid-run;
        // the dead actors run it as rows instead.
        if r.path == "/auth/logout" || r.path == "/auth/logout-all" {
            continue;
        }
        let path = fill_route(&r.path, w);
        for (actor, _) in policy::ACTORS {
            let a = w.actor(actor);
            let mut headers: Vec<(String, String)> = vec![
                ("Origin".into(), PAGE.into()),
                ("Content-Type".into(), "application/json".into()),
                ("X-Ordo-Space".into(), a.space.clone()),
            ];
            if let Some(t) = &a.token {
                headers.push(("Authorization".into(), format!("Bearer {t}")));
            }
            let body: &[u8] = if method == "GET" || method == "DELETE" {
                b""
            } else {
                br#"{"zzAuthzProbe":1}"#
            };
            let answer = d.raw(&method, &path, &headers, body);
            let (expect, mark) = policy::expect_route(&r.method, &r.path, actor);
            v.judge(Cell {
                what: format!("{} {} [{actor}]", r.method, r.path),
                expect,
                mark,
                answer,
            });
        }
    }
}

fn fill_route(path: &str, w: &World) -> String {
    let p = path
        .replace("{connection}", "x")
        .replace("{tenant}", w.var("T1"))
        .replace("{id}", "x")
        .replace("{stream}", "x")
        .replace("{key}", "x")
        .replace("{entity}/{field}/{id}", "files/plain/x")
        .replace("{handle}", w.var("HANDLE_A_PLAIN"));
    // `/security/events` and `/usage` need a `space` query to get past
    // their request check to the access check the matrix is probing.
    if p.starts_with("/security/events") || p.starts_with("/usage") {
        format!("{p}?space={}", w.var("T1"))
    } else {
        p
    }
}

/// Header variations on one op, read by every actor (`core/client.ts`'s
/// `myNotes` is the vehicle; a read with a row guard).
fn header_cells(d: &Deployment, w: &World, v: &mut Verdict) {
    let t1 = w.var("T1").to_string();
    let t2 = w.var("T2").to_string();
    for (actor, _) in policy::ACTORS {
        let a = w.actor(actor);
        let token = a.token.clone().unwrap_or_default();
        // A tenant the actor has no seat in: T2 for everyone, except
        // t2_owner (whose own tenant is T2), who is sent T1.
        let other = if *actor == "t2_owner" { &t1 } else { &t2 };
        let other = d.raw("GET", "/op/myNotes", &with(&token, other, PAGE), b"");
        v.judge(Cell {
            what: format!("header X-Ordo-Space=other [{actor}]"),
            expect: Expect::Denied,
            mark: Mark::Holds,
            answer: other,
        });
    }
    // A foreign Origin on a real caller's read: the door answers, and the
    // CORS layer does not hand the foreign page the answer.
    let a = w.actor("member_a");
    let token = a.token.clone().unwrap();
    let foreign = d.raw("GET", "/op/myNotes", &with(&token, &a.space, FOREIGN), b"");
    // The foreign page must not be handed the answer: the CORS layer gives
    // it no `Access-Control-Allow-Origin` of its own (`design-public-path`
    // §8). A browser then refuses the read.
    let acao = foreign.header("access-control-allow-origin");
    let leaked = acao == Some(FOREIGN) || acao == Some("*");
    v.judge(Cell {
        what: "header Origin=foreign carries no ACAO, member_a".into(),
        expect: Expect::Code(200, ""),
        mark: Mark::Holds,
        answer: Answer {
            status: if leaked { 0 } else { 200 },
            headers: vec![],
            body: vec![],
        },
    });
}

fn with(token: &str, space: &str, origin: &str) -> Vec<(String, String)> {
    let mut h = vec![
        ("Origin".to_string(), origin.to_string()),
        ("X-Ordo-Space".to_string(), space.to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if !token.is_empty() {
        h.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
    h
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots a deployment; run with scripts/authz-matrix.sh"]
async fn authorization_matrix() {
    let t0 = std::time::Instant::now();
    // The identity service's keys, ahead of the boot: the door fetches them
    // when it starts.
    let issuer = authz_support::identity::Issuer::start();
    let settings = issuer.settings();
    let extra: Vec<(&str, &str)> = settings.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut d = Deployment::boot_with("ordo_authz_matrix", &extra).await;
    let mut w = actors::build(&mut d);
    actors::add_deployment_actors(&mut w, &issuer);
    eprintln!("world built in {:?}", t0.elapsed());

    guard_every_route_and_op_is_in_the_policy(&w);

    let mut v = Verdict::default();
    op_cells(&d, &w, &mut v);
    route_cells(&d, &w, &mut v);
    header_cells(&d, &w, &mut v);

    // Rows: the extension point for replayed catalog attacks. Each row's
    // request is filled from the world and judged by its own `Expect`.
    for rowf in ROWS {
        let (what, method, path, headers, body, expect, mark) = rowf(&w);
        let h: Vec<(String, String)> = headers;
        let answer = d.raw(&method, &path, &h, &body);
        v.judge(Cell {
            what,
            expect,
            mark,
            answer,
        });
    }

    for o in authz_support::catalog::run(&d, &w) {
        v.judge(Cell {
            what: o.what,
            expect: o.expect,
            mark: o.mark,
            answer: o.answer,
        });
    }

    report(&v);
    eprintln!("matrix: {} cells in {:?}", v.cells, t0.elapsed());
    assert!(
        v.surprises.is_empty(),
        "{} surprise(s):\n{}",
        v.surprises.len(),
        v.surprises.join("\n")
    );
}

fn report(v: &Verdict) {
    eprintln!("\n=== {} cells ===", v.cells);
    if !v.defects.is_empty() {
        eprintln!("\nKNOWN_DEFECT:");
        for (id, cells) in &v.defects {
            let desc = policy::DEFECTS
                .iter()
                .find(|(d, _)| d == id)
                .map(|(_, t)| *t)
                .unwrap_or("");
            eprintln!("  [{id}] {desc}");
            for c in cells {
                eprintln!("      {c}");
            }
        }
    }
    if !v.questions.is_empty() {
        eprintln!("\nUNSPECIFIED:");
        for (id, cells) in &v.questions {
            let desc = policy::QUESTIONS
                .iter()
                .find(|(d, _)| d == id)
                .map(|(_, t)| *t)
                .unwrap_or("");
            eprintln!("  [{id}] {desc}");
            for c in cells {
                eprintln!("      {c}");
            }
        }
    }
    if !v.surprises.is_empty() {
        eprintln!("\nSURPRISES:");
        for s in &v.surprises {
            eprintln!("  {s}");
        }
    }
}

/// Every route the door mounts and every op it serves must have a policy.
/// A new route or op fails this, so it cannot ship untested.
fn guard_every_route_and_op_is_in_the_policy(w: &World) {
    let mut missing = Vec::new();
    for r in routes::mounted() {
        if r.path == "/op/{name}" {
            continue;
        }
        if policy::route_rule(&r.method, &r.path).is_none() {
            missing.push(format!("route {} {} ({})", r.method, r.path, r.file));
        }
    }
    let program = ordo_conformance::Fixture::from_source(authz_support::SOURCE, "").unwrap();
    for (op, _) in routes::ops(&program) {
        if policy::op_class(&op).is_none() {
            missing.push(format!("op {op}"));
        }
    }
    let _ = w;
    assert!(
        missing.is_empty(),
        "the door serves these, and the policy does not list them \
         (add them to authz_support/policy.rs):\n{}",
        missing.join("\n")
    );
}

/// A row builds one request from the world. Empty today: the place a later
/// session adds a pen tester's catalog entries (see this file's head and
/// `rows.rs` once one exists). Each returns what `raw` needs plus its
/// policy.
/// What a row returns: its name, method, path, headers, body, expected
/// answer and mark.
type Row = (
    String,
    String,
    String,
    Vec<(String, String)>,
    Vec<u8>,
    Expect,
    Mark,
);

type RowFn = fn(&World) -> Row;

/// How many tries each slowdown burst sends.
const GUESSES: usize = 12;

/// What try `n` of a guessing burst must answer (owner, 2026-10-25): the
/// first 5 are judged as wrong; the last waits. Between them a try is
/// refused either way: the first wait is 1 s on a whole-second clock, so a
/// try that lands just past a second boundary may be checked (and is then
/// counted, so the next wait is 2 s and the last try is surely refused).
fn guess_expect(n: usize, wrong: Expect) -> Expect {
    match n {
        1..=5 => wrong,
        GUESSES => Expect::Code(429, "too_many_attempts"),
        _ => Expect::Refused,
    }
}

/// Wrong password try `N` against one account. Before 2026-10-25, 200 went
/// through unslowed. An identifier nobody holds is slowed the same as a
/// real one, so the row tells a stranger nothing about accounts.
fn password_guess<const N: usize>(_: &World) -> Row {
    let body = json!({
        "method": "password",
        "kind": "password",
        "challenge": "pwd-ch-00000000000000000000000000000000",
        "identifier": "slow-target@example.test",
        "password": "a guess",
        "display": "Guess",
    });
    (
        format!("ROW guess {N}/{GUESSES} wrong password on one account"),
        "POST".into(),
        "/auth/complete".into(),
        vec![
            ("Origin".into(), PAGE.into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body.to_string().into_bytes(),
        guess_expect(N, Expect::Unauth),
        Mark::Holds,
    )
}

/// Wrong password try `N` against one share link. Before 2026-10-25, 60
/// went through in 13 s. The lock (`LINK_TRIES`) sits above the slowdown.
fn link_guess<const N: usize>(w: &World) -> Row {
    let body = json!({"link": w.var("LINK_PWD"), "password": "a guess"});
    (
        format!("ROW guess {N}/{GUESSES} wrong password on one share link"),
        "POST".into(),
        "/auth/link".into(),
        vec![
            ("Origin".into(), PAGE.into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body.to_string().into_bytes(),
        guess_expect(N, Expect::Code(401, "link_password_wrong")),
        Mark::Holds,
    )
}

const ROWS: &[RowFn] = &[
    // ── the guessing slowdown: one account, then one link ──
    password_guess::<1>,
    password_guess::<2>,
    password_guess::<3>,
    password_guess::<4>,
    password_guess::<5>,
    password_guess::<6>,
    password_guess::<7>,
    password_guess::<8>,
    password_guess::<9>,
    password_guess::<10>,
    password_guess::<11>,
    password_guess::<GUESSES>,
    link_guess::<1>,
    link_guess::<2>,
    link_guess::<3>,
    link_guess::<4>,
    link_guess::<5>,
    link_guess::<6>,
    link_guess::<7>,
    link_guess::<8>,
    link_guess::<9>,
    link_guess::<10>,
    link_guess::<11>,
    link_guess::<GUESSES>,
    // ── a forged bearer: another deployment's key over A's claims ──
    |w: &World| {
        let a = w.actor("foreign_key");
        (
            "ROW IDOR forged bearer from another deployment key".into(),
            "GET".into(),
            "/op/myNotes".into(),
            with(a.token.as_deref().unwrap_or(""), &a.space, PAGE),
            Vec::new(),
            Expect::Denied,
            Mark::Holds,
        )
    },
    // ── /auth/refresh with a foreign Origin: refused, cookie left whole ──
    |w: &World| {
        let cookie = w.var("COOKIE_OLGA").to_string();
        (
            "ROW CSRF /auth/refresh from a foreign Origin".into(),
            "POST".into(),
            "/auth/refresh".into(),
            vec![("Origin".into(), FOREIGN.into()), ("Cookie".into(), cookie)],
            Vec::new(),
            // A foreign origin is refused before the secret is touched
            // (`auth.rs::resume`): the `remember_off` edge code, and the
            // cookie is left whole so no page can sign a person out.
            Expect::Code(401, "remember_off"),
            Mark::Holds,
        )
    },
    // ── a forged deployment-key bearer carries no deployment reach ──
    |w: &World| {
        let a = w.actor("foreign_key");
        (
            "ROW forged bearer cannot reach a deployment op".into(),
            "GET".into(),
            "/op/std.deployment.tenants".into(),
            with(a.token.as_deref().unwrap_or(""), authz_support::HOME, PAGE),
            Vec::new(),
            Expect::Denied,
            Mark::Holds,
        )
    },
];
