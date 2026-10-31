//! Two `ordo-host` processes on one database, writing to one tenant at once.
//!
//! The board task `horizontal-scaling-before-v1`, item 2. Every test boots
//! the real binary twice on the same control database, keys and file store,
//! each with its own port and `ORDO_REPLICA_ID`, and drives both from this
//! process over raw sockets, as a browser would. Nothing here builds a
//! server part by hand.
//!
//! | Test | Proves |
//! |---|---|
//! | [`concurrent_writes_stay_consistent`] | no lost or duplicated writes; conflicting edits resolve by the row's revision rule; the three logs verify; one idempotency key applies once; a feed on B sees A's writes |
//! | [`an_ended_session_is_refused_on_the_other_instance`] | a logout, a logout-everywhere and a revoked share are refused on B within `ORDO_SESSION_REFRESH_SECS` |
//! | [`leader_only_workers_run_on_one_instance`] | one leader; its workers run there only; a second `ordo apply` queues behind a running one |
//! | [`the_guessing_slowdown_counts_tries_on_both`] | wrong tries on either instance count toward one slowdown |
//!
//! `#[ignore]`: each boots two servers and one drives about a hundred
//! concurrent requests. `scripts/two-instances.sh` runs them on a Postgres
//! cluster of their own (`ORDO_PGDATA=$HOME/.ordo-pg/two-instances`).

mod authz_support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use authz_support::actors::{self, Person};
use authz_support::{Answer, Deployment, HOME, PAGE};
use ordo_conformance::Pg;
use serde_json::{json, Value};

/// The session deny list's refresh, in seconds. Low, so the test sees it.
const REFRESH_SECS: u64 = 1;
/// What a refusal may take to cross: the refresh, a worker pass and slack.
const CROSS_BOUND: Duration = Duration::from_secs(REFRESH_SECS + 6);

// ── the pair ────────────────────────────────────────────────────────────

/// Two processes, one control database, one tenant with its owner.
struct Pair {
    a: Deployment,
    b: Deployment,
    /// The deployment owner's device, to sign in to the deployment's space.
    dana: ordo_identity::software_authenticator::TestAuthenticator,
    olga: Person,
    space: String,
}

/// How long B took before it served a tenant A made, and what it said first.
struct Lag {
    first_status: u16,
    took: Duration,
}

impl Pair {
    /// Boot A, make the tenant and run `cast` against A, then boot B.
    ///
    /// B boots last on purpose. A replica reads the key custody once, when it
    /// starts, so a tenant made on A afterwards is not served by B
    /// ([`a_tenant_made_after_the_second_instance_booted_is_served_by_it`]).
    /// Everything the scenarios share exists before B starts.
    async fn boot<T>(db: &str, cast: impl FnOnce(&Deployment, &Person) -> T) -> (Pair, T) {
        let mut a = Deployment::boot_with(
            db,
            &[
                ("ORDO_REPLICA_ID", "rep-a"),
                ("ORDO_SESSION_REFRESH_SECS", &REFRESH_SECS.to_string()),
                (
                    "ORDO_PG_POOL_SIZE",
                    &std::env::var("TWO_POOL").unwrap_or("6".into()),
                ),
            ],
        )
        .await;
        // The deployment owner is named by principal, and a principal is
        // minted when a person registers: register, then restart naming her
        // (the authorization matrix does the same).
        let dana = actors::device(1, "deployment-owner");
        let first = actors::register(&a, &dana, "Dana", json!({}));
        assert_eq!(first.status, 200, "Dana registers: {}", first.text());
        let principal = actors::principal_of(first.json()["token"].as_str().expect("token"));
        a.restart(&[("ORDO_DEPLOYMENT_OWNERS", &principal)]);
        let olga = actors::found(&a, 2, "Olga");
        let space = olga.space.clone();
        let cast = cast(&a, &olga);
        let b = a
            .replica(&[
                ("ORDO_REPLICA_ID", "rep-b"),
                ("ORDO_DEPLOYMENT_OWNERS", &principal),
            ])
            .await;
        (
            Pair {
                a,
                b,
                dana,
                olga,
                space,
            },
            cast,
        )
    }

    /// A session of `p` on B (the one `p.token` is on A).
    fn on_b(&self, p: &Person) -> String {
        sign_in_token(&self.b, p)
    }

    /// Wait up to `within` for B to serve the tenant.
    fn b_serves(&self, token: &str, within: Duration) -> Result<Lag, String> {
        let started = Instant::now();
        let mut first_status = 0;
        loop {
            let a = Seat::new(&self.b, &self.space, token).get("myNotes", &json!({}));
            if first_status == 0 {
                first_status = a.status;
            }
            if a.status == 200 {
                return Ok(Lag {
                    first_status,
                    took: started.elapsed(),
                });
            }
            if started.elapsed() > within {
                return Err(format!(
                    "B did not serve tenant {} in {within:?}: {} {}",
                    self.space,
                    a.status,
                    a.text()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The deployment owner's bearer on `d`.
    fn dana_token(&self, d: &Deployment) -> String {
        let started = Instant::now();
        loop {
            let a = actors::sign_in(d, &self.dana, json!({"space": HOME}));
            if a.status == 200 {
                return a.json()["token"].as_str().expect("token").to_string();
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "Dana cannot sign in: {} {}",
                a.status,
                a.text()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// `/health/deep` on `d`, as the deployment owner.
    fn health(&self, d: &Deployment, token: &str) -> Value {
        let bearer = format!("Bearer {token}");
        let a = d.send(
            "GET",
            "/health/deep",
            &[
                ("Origin", PAGE),
                ("X-Ordo-Space", HOME),
                ("Authorization", &bearer),
            ],
            "",
        );
        assert_eq!(a.status, 200, "/health/deep: {}", a.text());
        a.json()
    }

    fn control(&self) -> Pg {
        Pg {
            db: self.a.db.clone(),
            ..Pg::default()
        }
    }

    fn tenant(&self) -> Pg {
        Pg {
            db: self.space.clone(),
            ..Pg::default()
        }
    }
}

fn sign_in_token(d: &Deployment, p: &Person) -> String {
    let a = actors::sign_in(d, &p.device, json!({"space": p.space}));
    assert_eq!(a.status, 200, "sign-in of {}: {}", p.name, a.text());
    a.json()["token"].as_str().expect("token").to_string()
}

// ── a timeline, for reading a failure ──

static TIMELINE: Mutex<Vec<(Duration, String)>> = Mutex::new(Vec::new());
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Note an event with the time since the first note.
fn mark(what: impl Into<String>) {
    let at = EPOCH.get_or_init(Instant::now).elapsed();
    TIMELINE.lock().expect("timeline").push((at, what.into()));
}

fn print_timeline() {
    for (at, what) in TIMELINE.lock().expect("timeline").iter() {
        eprintln!("  {:>8.3}s  {what}", at.as_secs_f64());
    }
}

// ── one caller on one instance ──────────────────────────────────────────

/// A bearer on one instance, in one tenant.
struct Seat<'a> {
    d: &'a Deployment,
    space: &'a str,
    token: String,
}

impl<'a> Seat<'a> {
    fn new(d: &'a Deployment, space: &'a str, token: &str) -> Seat<'a> {
        Seat {
            d,
            space,
            token: token.to_string(),
        }
    }

    fn call(&self, method: &str, path: &str, key: Option<&str>, body: &str) -> Answer {
        let bearer = format!("Bearer {}", self.token);
        let mut h = vec![
            ("X-Ordo-Space", self.space),
            ("Origin", PAGE),
            ("Accept", "application/json"),
            ("Authorization", bearer.as_str()),
        ];
        if let Some(k) = key {
            h.push(("Idempotency-Key", k));
        }
        if method == "POST" {
            h.push(("Content-Type", "application/json"));
        }
        let a = self.d.send(method, path, &h, body);
        if a.status == 403 && error_code(&a) == "access_denied" {
            mark(format!(
                "403 access_denied on :{} for {method} {path} (token ...{})",
                self.d.port,
                &self.token[self.token.len().saturating_sub(6)..]
            ));
        }
        a
    }

    fn post(&self, op: &str, params: &Value, key: &str) -> Answer {
        self.call("POST", &format!("/op/{op}"), Some(key), &params.to_string())
    }

    fn get(&self, op: &str, params: &Value) -> Answer {
        self.call(
            "GET",
            &format!("/op/{op}{}", actors::query(params)),
            None,
            "",
        )
    }

    fn delete(&self, op: &str, params: &Value, key: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/op/{op}{}", actors::query(params)),
            Some(key),
            "",
        )
    }

    fn create(&self, title: &str, key: &str) -> Answer {
        self.post(
            "createNote",
            &json!({"title": title, "body": "b", "created": "2026-10-01T09:00:00Z"}),
            key,
        )
    }

    fn rev_of(&self, id: &str) -> Option<i64> {
        let a = self.get("noteById", &json!({"id": id}));
        (a.status == 200).then(|| a.json()["rows"][0]["rev"].as_i64())?
    }

    fn edit(&self, id: &str, rev: i64, title: &str, key: &str) -> Answer {
        self.post(
            "editNote",
            &json!({"id": id, "rev": rev, "title": title}),
            key,
        )
    }
}

/// The id a create answered.
fn created_id(a: &Answer) -> Option<String> {
    (a.status == 200).then(|| a.json()["rows"][0]["created"].as_str().map(str::to_string))?
}

fn error_code(a: &Answer) -> String {
    a.json()["error"]["code"].as_str().unwrap_or("").to_string()
}

/// Rows of `sql` against `pg`, each cell as text.
fn rows(pg: &Pg, sql: &str) -> Vec<Vec<String>> {
    pg.query(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|r| r.into_iter().map(Option::unwrap_or_default).collect())
        .collect()
}

fn lit(s: &str) -> String {
    s.replace('\'', "''")
}

// ── the logs ────────────────────────────────────────────────────────────

/// Run `ordo verify` (the crate function the binary calls) and answer its
/// exit code and report.
fn ordo_verify(args: &[&str]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    // The command builds a runtime of its own, so it cannot run on this
    // test's runtime; a thread of its own is a process's main thread enough.
    let out = std::thread::spawn(move || ordo_cli::operator::run("verify", &args))
        .join()
        .expect("verify does not panic");
    (out.code, out.report)
}

/// The same chain, read straight from the table: contiguous positions, each
/// entry's `prev` the hash before it. Answers the problems and the number of
/// entries seen per stream.
fn chains_in_sql(pg: &Pg, table: &str, stream: &str) -> (Vec<String>, BTreeMap<String, i64>) {
    let mut problems = Vec::new();
    let mut seen = BTreeMap::new();
    for r in rows(
        pg,
        &format!(
            "SELECT \"{stream}\", count(*), min(idx), max(idx), count(DISTINCT idx) \
             FROM \"{table}\" GROUP BY \"{stream}\""
        ),
    ) {
        let n: i64 = r[1].parse().unwrap_or(-1);
        let lo: i64 = r[2].parse().unwrap_or(-1);
        let hi: i64 = r[3].parse().unwrap_or(-1);
        let distinct: i64 = r[4].parse().unwrap_or(-1);
        if n != distinct || hi - lo + 1 != n {
            problems.push(format!(
                "{table} `{}`: {n} entries, positions {lo}..{hi}, {distinct} distinct",
                r[0]
            ));
        }
        seen.insert(r[0].clone(), n);
    }
    let broken = rows(
        pg,
        &format!(
            "SELECT c.\"{stream}\", c.idx FROM \"{table}\" c \
             JOIN \"{table}\" p ON p.\"{stream}\" = c.\"{stream}\" AND p.idx = c.idx - 1 \
             WHERE c.prev <> p.hash"
        ),
    );
    for r in broken {
        problems.push(format!(
            "{table} `{}` position {}: prev does not link",
            r[0], r[1]
        ));
    }
    (problems, seen)
}

// ── test 1: the storm ───────────────────────────────────────────────────

const MEMBERS: usize = 3;
/// Creates per member per instance.
const CREATES: usize = 12;
/// Rounds of the same-row edit, per member.
const ROUNDS: usize = 12;
/// Rows made on A and deleted on both, per member.
const DELETES: usize = 5;
/// Idempotency keys sent to both instances at the same instant.
const SIM_KEYS: usize = 10;
/// Idempotency keys sent to A, then to B.
const SEQ_KEYS: usize = 6;

/// (round, instance, base rev, status, code, title, row) of one edit.
type EditRecord = (String, usize, &'static str, i64, u16, String, String);

#[derive(Default)]
struct Ledger {
    /// (id, title) of every create answered 200.
    created: Mutex<Vec<(String, String)>>,
    /// Ids a delete answered 200 for, and the statuses of each double delete.
    deleted: Mutex<Vec<String>>,
    /// Per shared row: (round, instance, base rev, status, code, title).
    edits: Mutex<Vec<EditRecord>>,
    /// (file id, bytes, instance it was uploaded through).
    files: Mutex<Vec<(String, Vec<u8>, &'static str)>>,
    /// What went wrong that no check below names.
    problems: Mutex<Vec<String>>,
}

impl Ledger {
    fn problem(&self, what: String) {
        self.problems.lock().expect("ledger").push(what);
    }
}

/// A member of the tenant and a session on each instance.
struct Member {
    p: Person,
    tb: String,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots two servers; scripts/two-instances.sh"]
async fn concurrent_writes_stay_consistent() {
    let started = Instant::now();
    // The cast: managers who each own their rows, a viewer who signs in and
    // out all the while, and a guest from a tenant of his own.
    let (pair, (people, dee, gus)) = Pair::boot("ordo_two_storm", |a, olga| {
        let people: Vec<Person> = ["Ann", "Ben", "Cat"]
            .iter()
            .enumerate()
            .map(|(i, name)| actors::join(a, olga, 4 + i as u8, name, "manager"))
            .collect();
        let dee = actors::join(a, olga, 9, "Dee", "viewer");
        let gus = actors::found(a, 10, "Gus");
        (people, dee, gus)
    })
    .await;
    let t = pair.space.clone();
    let members: Vec<Member> = people
        .into_iter()
        .map(|p| {
            let tb = pair.on_b(&p);
            Member { p, tb }
        })
        .collect();
    let olga_b = pair.on_b(&pair.olga);
    let lag = pair
        .b_serves(&olga_b, Duration::from_secs(30))
        .expect("B serves a tenant that existed when it booted");
    eprintln!(
        "B served the tenant after {:?} (first answer {})",
        lag.took, lag.first_status
    );

    // Each member's shared row, made before the storm.
    let shared: Vec<String> = members
        .iter()
        .map(|m| {
            let a =
                Seat::new(&pair.a, &t, &m.p.token).create(&format!("shared-{}", m.p.name), &uid());
            created_id(&a).unwrap_or_else(|| panic!("shared row: {} {}", a.status, a.text()))
        })
        .collect();

    // A feed on B, opened before the first write, as Ann.
    let mut feed = pair.b.open_stream(
        "/op/myNotes?live=1",
        &[
            ("Origin".into(), PAGE.into()),
            ("Authorization".into(), format!("Bearer {}", members[0].tb)),
            ("X-Ordo-Space".into(), t.clone()),
        ],
    );
    assert_eq!(feed.status, 200, "the feed opens on B");
    feed.read_for(Duration::from_millis(600));
    // One write through A, then a bounded wait for B's feed to say so.
    let probe = Seat::new(&pair.a, &t, &members[0].p.token).create("feed-probe", &uid());
    let probe_id = created_id(&probe).expect("feed probe write");
    let mut feed_text = String::new();
    let feed_clock = Instant::now();
    while feed_clock.elapsed() < Duration::from_secs(15)
        && !(feed_text.contains("event: invalidate") || feed_text.contains("event: change"))
    {
        feed_text.push_str(&feed.read_for(Duration::from_millis(250)));
    }
    let feed_saw_probe =
        feed_text.contains("event: invalidate") || feed_text.contains("event: change");
    let feed_after_probe = feed_clock.elapsed();

    let ledger = Ledger::default();
    let gates: Vec<Barrier> = (0..MEMBERS).map(|_| Barrier::new(2)).collect();
    let done = AtomicBool::new(false);
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let feed_log = Mutex::new(String::new());
    let sim_answers: Mutex<Vec<(usize, Answer, Answer)>> = Mutex::new(Vec::new());
    let guest_report: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let session_report: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        // Creates, files: every member, both instances.
        for (mi, m) in members.iter().enumerate() {
            for (inst, d, token) in [
                ("a", &pair.a, m.p.token.clone()),
                ("b", &pair.b, m.tb.clone()),
            ] {
                let ledger = &ledger;
                let t = &t;
                let finished = &finished;
                s.spawn(move || {
                    let _f = Finish(finished);
                    let seat = Seat::new(d, t, &token);
                    for n in 0..CREATES {
                        let title = format!("w-{mi}-{inst}-{n}");
                        let key = uid();
                        let a = persist(|| seat.create(&title, &key));
                        match created_id(&a) {
                            Some(id) => {
                                ledger.created.lock().expect("l").push((id.clone(), title));
                                if n % 4 == 0 {
                                    attach_file(&seat, &id, inst, n, ledger);
                                }
                            }
                            None => ledger.problem(format!(
                                "create {title} on {inst}: {} {}",
                                a.status,
                                a.text()
                            )),
                        }
                    }
                });
            }
        }
        // The same row from both sides, in lockstep so they truly collide.
        for (mi, m) in members.iter().enumerate() {
            let gate = &gates[mi];
            for (inst, d, token) in [
                ("a", &pair.a, m.p.token.clone()),
                ("b", &pair.b, m.tb.clone()),
            ] {
                let ledger = &ledger;
                let t = &t;
                let id = shared[mi].clone();
                let finished = &finished;
                s.spawn(move || {
                    let _f = Finish(finished);
                    let seat = Seat::new(d, t, &token);
                    for round in 0..ROUNDS {
                        let rev = seat.rev_of(&id).unwrap_or(-1);
                        // Both read the same revision, then both write it.
                        gate.wait();
                        let title = format!("e-{inst}-{round}");
                        let a = seat.edit(&id, rev, &title, &uid());
                        ledger.edits.lock().expect("l").push((
                            id.clone(),
                            round,
                            inst,
                            rev,
                            a.status,
                            error_code(&a),
                            title,
                        ));
                        gate.wait();
                    }
                });
            }
        }
        // Deletes: made on A, then deleted on both instances at once.
        for (mi, m) in members.iter().enumerate() {
            let ledger = &ledger;
            let t = &t;
            let pair = &pair;
            let finished = &finished;
            s.spawn(move || {
                let _f = Finish(finished);
                let sa = Seat::new(&pair.a, t, &m.p.token);
                let sb = Seat::new(&pair.b, t, &m.tb);
                for n in 0..DELETES {
                    let key = uid();
                    let a = persist(|| sa.create(&format!("del-{mi}-{n}"), &key));
                    let Some(id) = created_id(&a) else {
                        ledger.problem(format!("create del-{mi}-{n}: {} {}", a.status, a.text()));
                        continue;
                    };
                    let gate = Barrier::new(2);
                    let (ra, rb) = std::thread::scope(|s2| {
                        let ha = s2.spawn(|| {
                            gate.wait();
                            let key = uid();
                            persist(|| sa.delete("deleteNote", &json!({"id": id}), &key))
                        });
                        let hb = s2.spawn(|| {
                            gate.wait();
                            let key = uid();
                            persist(|| sb.delete("deleteNote", &json!({"id": id}), &key))
                        });
                        (ha.join().expect("a"), hb.join().expect("b"))
                    });
                    let mut won = 0;
                    for (inst, r) in [("a", &ra), ("b", &rb)] {
                        match r.status {
                            200 => won += 1,
                            404 => {}
                            _ => ledger.problem(format!(
                                "delete {id} on {inst}: {} {}",
                                r.status,
                                r.text()
                            )),
                        }
                    }
                    if won == 0 {
                        ledger.problem(format!("delete {id}: neither instance acknowledged it"));
                    } else {
                        ledger.deleted.lock().expect("l").push(id);
                    }
                }
            });
        }
        // The same idempotency key to both instances at the same instant.
        {
            let sim = &sim_answers;
            let (pair, members, t) = (&pair, &members, &t);
            let finished = &finished;
            s.spawn(move || {
                let _f = Finish(finished);
                let ann = &members[0];
                for k in 0..SIM_KEYS {
                    let key = format!("sim-{}-{k}", uid());
                    let title = format!("idem-sim-{k}");
                    let gate = Barrier::new(2);
                    let (ra, rb) = std::thread::scope(|s2| {
                        let ha = s2.spawn(|| {
                            gate.wait();
                            persist(|| Seat::new(&pair.a, t, &ann.p.token).create(&title, &key))
                        });
                        let hb = s2.spawn(|| {
                            gate.wait();
                            persist(|| Seat::new(&pair.b, t, &ann.tb).create(&title, &key))
                        });
                        (ha.join().expect("a"), hb.join().expect("b"))
                    });
                    sim.lock().expect("sim").push((k, ra, rb));
                }
            });
        }
        // A share, taken up on B and revoked on A, while all of it runs.
        {
            let (pair, members, t, gus, report) = (&pair, &members, &t, &gus, &guest_report);
            let finished = &finished;
            s.spawn(move || {
                let _f = Finish(finished);
                if std::env::var("TWO_SKIP").as_deref() != Ok("share") {
                    share_and_revoke(pair, &members[0], gus, t, report)
                } else {
                    report.lock().unwrap().push("done".into())
                }
            });
        }
        // Sign-in and sign-out, each on the other instance.
        {
            let (pair, dee, t, report) = (&pair, &dee, &t, &session_report);
            let finished = &finished;
            s.spawn(move || {
                let _f = Finish(finished);
                if std::env::var("TWO_SKIP").as_deref() != Ok("churn") {
                    sessions_churn(pair, dee, t, report)
                } else {
                    report.lock().unwrap().push("done".into())
                }
            });
        }
        // The feed on B reads until the writers are done.
        {
            let (feed, done, log) = (&mut feed, &done, &feed_log);
            s.spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let got = feed.read_for(Duration::from_millis(250));
                    log.lock().expect("feed").push_str(&got);
                }
            });
        }
        // The feed reader ends when the writers have, plus a moment for the
        // last frames.
        s.spawn(|| {
            let clock = Instant::now();
            while finished.load(Ordering::SeqCst) < WRITERS
                && clock.elapsed() < Duration::from_secs(300)
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            if finished.load(Ordering::SeqCst) < WRITERS {
                ledger.problem(format!(
                    "only {} of {WRITERS} writer threads finished in 300 s",
                    finished.load(Ordering::SeqCst)
                ));
            }
            std::thread::sleep(Duration::from_secs(2));
            done.store(true, Ordering::SeqCst);
        });
    });
    let storm = started.elapsed();
    eprintln!("the storm took {storm:?} in all");

    // ── check 1: no lost or duplicated writes ──
    let mut failures: Vec<String> = Vec::new();
    for p in ledger.problems.lock().expect("l").iter() {
        failures.push(format!("1 unexpected answer: {p}"));
    }
    for line in guest_report
        .lock()
        .expect("g")
        .iter()
        .filter(|l| *l != "done")
    {
        failures.push(format!("1 share: {line}"));
    }
    for line in session_report
        .lock()
        .expect("s")
        .iter()
        .filter(|l| *l != "done")
    {
        failures.push(format!("1 sessions: {line}"));
    }
    let tenant = pair.tenant();
    let in_db = rows(
        &tenant,
        &format!(
            "SELECT id, title, rev::text FROM \"notes\" WHERE \"$space\" = '{}'",
            lit(&t)
        ),
    );
    let mut by_id: BTreeMap<String, (String, i64)> = BTreeMap::new();
    for r in &in_db {
        if by_id
            .insert(r[0].clone(), (r[1].clone(), r[2].parse().unwrap_or(-1)))
            .is_some()
        {
            failures.push(format!("1 row {} appears twice", r[0]));
        }
    }
    let created = ledger.created.lock().expect("l").clone();
    let mut titles_seen = BTreeSet::new();
    for (id, title) in &created {
        if !titles_seen.insert(title.clone()) {
            failures.push(format!("1 two creates answered for title {title}"));
        }
        match by_id.get(id) {
            None => failures.push(format!("1 acknowledged create {title} ({id}) is missing")),
            Some((t2, _)) if t2 != title => {
                failures.push(format!("1 row {id} holds `{t2}`, create said `{title}`"))
            }
            Some(_) => {}
        }
    }
    let n_w = in_db.iter().filter(|r| r[1].starts_with("w-")).count();
    if n_w != created.len() {
        failures.push(format!(
            "1 {n_w} `w-` rows stored, {} creates acknowledged",
            created.len()
        ));
    }
    for id in ledger.deleted.lock().expect("l").iter() {
        if by_id.contains_key(id) {
            failures.push(format!("1 deleted row {id} is still stored"));
        }
    }
    let n_del = in_db.iter().filter(|r| r[1].starts_with("del-")).count();
    if n_del != 0 {
        failures.push(format!("1 {n_del} `del-` rows survive their deletes"));
    }

    // Conflicting edits. Both sides read the same revision and write it at
    // once, round after round. The row's revision rule: an edit lands only
    // on the revision it names, and moves the row one up. So the accepted
    // edits name revisions 1, 2, 3, ... with no repeat and no gap, the row
    // ends one past the last of them holding the last one's title, and every
    // refused edit is a 409: `cas_miss` (the revision moved) or, when the two
    // overlapped in flight, `serialization_conflict` (send again).
    let edits = ledger.edits.lock().expect("l").clone();
    let (mut cas_miss, mut serialization) = (0, 0);
    for (mi, row_id) in shared.iter().enumerate() {
        let mine: Vec<_> = edits.iter().filter(|e| &e.0 == row_id).collect();
        if mine.len() != 2 * ROUNDS {
            failures.push(format!(
                "1 shared row {mi}: {} edit answers, {} sent",
                mine.len(),
                2 * ROUNDS
            ));
        }
        let mut wins: Vec<(i64, String)> = mine
            .iter()
            .filter(|e| e.4 == 200)
            .map(|e| (e.3, e.6.clone()))
            .collect();
        wins.sort();
        for e in mine.iter().filter(|e| e.4 != 200) {
            match (e.4, e.5.as_str()) {
                (409, "cas_miss") => cas_miss += 1,
                (409, "serialization_conflict") => serialization += 1,
                _ => failures.push(format!(
                    "1 shared row {mi} round {}: an edit on {} was refused {} `{}`, not a 409 conflict",
                    e.1, e.2, e.4, e.5
                )),
            }
        }
        let revs: Vec<i64> = wins.iter().map(|w| w.0).collect();
        let want: Vec<i64> = (1..=wins.len() as i64).collect();
        if revs != want {
            failures.push(format!(
                "1 shared row {mi}: accepted edits named revisions {revs:?}, not {want:?}"
            ));
        }
        match by_id.get(row_id) {
            Some((title, rev)) => {
                if *rev != wins.len() as i64 + 1 {
                    failures.push(format!(
                        "1 shared row {mi}: rev {rev} after {} accepted edits",
                        wins.len()
                    ));
                }
                if let Some(last) = wins.last() {
                    if &last.1 != title {
                        failures.push(format!(
                            "1 shared row {mi}: holds `{title}`, last accepted edit was `{}`",
                            last.1
                        ));
                    }
                }
            }
            None => failures.push(format!("1 shared row {mi} is gone")),
        }
    }
    if cas_miss == 0 {
        failures.push("1 no edit was ever refused `cas_miss`: the two sides never collided".into());
    }
    eprintln!(
        "edits refused: {cas_miss} `cas_miss`, {serialization} `serialization_conflict`; \
         writes sent again after a serialization conflict: {}",
        RETRIED.load(Ordering::SeqCst)
    );

    // Files: bytes uploaded through one instance, read through the other.
    let files = ledger.files.lock().expect("l").clone();
    for (id, bytes, via) in &files {
        let (reader_d, which) = if *via == "a" {
            (&pair.b, 1)
        } else {
            (&pair.a, 0)
        };
        // Whoever uploaded it owns it: try each member until one can read.
        let mut got: Option<Vec<u8>> = None;
        for m in &members {
            let token = if which == 1 { &m.tb } else { &m.p.token };
            let seat = Seat::new(reader_d, &t, token);
            let list = seat.get("myFiles", &json!({}));
            if list.status != 200 {
                continue;
            }
            let handle = list.json()["rows"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["id"] == json!(id)))
                .and_then(|r| r["plain"].as_str().map(str::to_string));
            if let Some(h) = handle {
                let r = seat.call("GET", &format!("/blob/{h}"), None, "");
                if r.status == 200 {
                    got = Some(r.body.clone());
                }
                break;
            }
        }
        match got {
            Some(b) if &b == bytes => {}
            Some(b) => failures.push(format!(
                "1 file {id}: {} bytes read, {} written",
                b.len(),
                bytes.len()
            )),
            None => failures.push(format!(
                "1 file {id} (uploaded via {via}) cannot be read on the other instance"
            )),
        }
    }
    if files.is_empty() {
        failures.push("1 no file was uploaded".into());
    }

    // ── check 3: one key, both instances, one effect ──
    let sim = sim_answers.lock().expect("s").clone();
    for (k, ra, rb) in &sim {
        let title = format!("idem-sim-{k}");
        let n = in_db.iter().filter(|r| r[1] == title).count();
        if n != 1 {
            failures.push(format!("3 key {k} sent to both: {n} rows stored"));
        }
        if ra.status != 200 || rb.status != 200 || ra.json()["rows"] != rb.json()["rows"] {
            failures.push(format!(
                "3 key {k}: A said {} {}, B said {} {}",
                ra.status,
                ra.text(),
                rb.status,
                rb.text()
            ));
        }
        let replays = [ra, rb]
            .iter()
            .filter(|r| r.header("x-ordo-idempotent-replay") == Some("true"))
            .count();
        if replays > 1 {
            failures.push(format!("3 key {k}: both answers were replays"));
        }
    }

    // Sequential keys: A first, then B, then the reverse, and a changed
    // body under a used key.
    {
        let ann = &members[0];
        let sa = Seat::new(&pair.a, &t, &ann.p.token);
        let sb = Seat::new(&pair.b, &t, &ann.tb);
        for k in 0..SEQ_KEYS {
            let (first, second, order) = if k % 2 == 0 {
                (&sa, &sb, "A then B")
            } else {
                (&sb, &sa, "B then A")
            };
            let key = format!("seq-{}-{k}", uid());
            let title = format!("idem-seq-{k}");
            let r1 = first.create(&title, &key);
            let r2 = second.create(&title, &key);
            if r1.status != 200 || r2.status != 200 || r1.json()["rows"] != r2.json()["rows"] {
                failures.push(format!(
                    "3 key {k} {order}: {} {} / {} {}",
                    r1.status,
                    r1.text(),
                    r2.status,
                    r2.text()
                ));
            }
            if r2.header("x-ordo-idempotent-replay") != Some("true") {
                failures.push(format!(
                    "3 key {k} {order}: the second answer was not marked a replay"
                ));
            }
            let r3 = second.create(&format!("{title}-changed"), &key);
            if r3.status != 409 || error_code(&r3) != "idempotency_mismatch" {
                failures.push(format!(
                    "3 key {k}: a changed body under the key got {} {}",
                    r3.status,
                    r3.text()
                ));
            }
            let n = rows(
                &tenant,
                &format!(
                    "SELECT count(*) FROM \"notes\" WHERE \"$space\" = '{}' AND title = '{title}'",
                    lit(&t)
                ),
            )[0][0]
                .clone();
            if n != "1" {
                failures.push(format!("3 key {k} {order}: {n} rows stored"));
            }
        }
    }

    // ── check 4: the feed on B ──
    let whole_feed = format!("{feed_text}{}", feed_log.lock().expect("feed"));
    let frames = whole_feed.matches("event: invalidate").count()
        + whole_feed.matches("event: change").count();
    if !feed_saw_probe {
        failures.push(format!(
            "4 B's feed said nothing in {feed_after_probe:?} after A wrote `feed-probe` ({probe_id}): {feed_text:?}"
        ));
    }
    if frames < 2 {
        failures.push(format!(
            "4 B's feed carried {frames} frames during the storm"
        ));
    }
    eprintln!("B's feed: first frame {feed_after_probe:?} after A's write, {frames} frames in all");

    // ── check 2: the logs ──
    // Security events reach their table through a drain; let it empty.
    {
        let count = |pg: &Pg| rows(pg, "SELECT count(*) FROM \"$secevents\"")[0][0].clone();
        let (mut last, mut calm) = (String::new(), Instant::now());
        let clock = Instant::now();
        while clock.elapsed() < Duration::from_secs(30) && calm.elapsed() < Duration::from_secs(4) {
            let now = format!("{}/{}", count(&tenant), count(&pair.control()));
            if now != last {
                last = now;
                calm = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let pg_ctrl = pair.control();
    let (code, report) = ordo_verify(&["--space", &t, "--db", &t, "--from-genesis"]);
    if code != 0 {
        failures.push(format!("2 `ordo verify --space` exited {code}:\n{report}"));
    }
    eprintln!("{report}");
    let (code, report) = ordo_verify(&["--deployment", "--db", &pair.a.db]);
    if code != 0 {
        failures.push(format!(
            "2 `ordo verify --deployment` exited {code}:\n{report}"
        ));
    }
    eprintln!("{report}");
    let (p1, n1) = chains_in_sql(&tenant, "$commandlog", "space");
    let (p2, n2) = chains_in_sql(&tenant, "$secevents", "scope");
    let (p3, n3) = chains_in_sql(&pg_ctrl, "$secevents", "scope");
    let (p4, n4) = chains_in_sql(&pg_ctrl, "$commandlog", "space");
    for p in p1.into_iter().chain(p2).chain(p3).chain(p4) {
        failures.push(format!("2 {p}"));
    }
    let cmds = n1.get(&t).copied().unwrap_or(0);
    // Every acknowledged write is one entry; at least that many exist.
    let acked = created.len() + ledger.deleted.lock().expect("l").len() + shared.len() + 1;
    if (cmds as usize) < acked {
        failures.push(format!(
            "2 the tenant's command log holds {cmds} entries for at least {acked} writes"
        ));
    }
    eprintln!(
        "chains: tenant commands {n1:?}, tenant security {n2:?}, deployment security {n3:?}, control commands {n4:?}"
    );

    if !failures.is_empty() {
        print_timeline();
    }
    if !failures.is_empty() && std::env::var("TWO_DIAG").is_ok() {
        let show = |label: &str| {
            for m in &members {
                for (n, d, tok) in [("A", &pair.a, &m.p.token), ("B", &pair.b, &m.tb)] {
                    let r = Seat::new(d, &t, tok).get("myNotes", &json!({}));
                    eprintln!("DIAG {label} {} on {n}: {}", m.p.name, r.status);
                }
            }
            eprintln!(
                "DIAG members: {:?}",
                rows(
                    &tenant,
                    &format!(
                        "SELECT id, role, status FROM \"members\" WHERE \"$space\" = '{}'",
                        lit(&t)
                    )
                )
            );
        };
        show("now");
        std::thread::sleep(Duration::from_secs(40));
        show("after 40s");
    }
    if !failures.is_empty() {
        // Keep both server logs for reading the failure.
        for (n, d) in [("a", &pair.a), ("b", &pair.b)] {
            let _ = std::fs::write(
                std::env::temp_dir().join(format!("ordo-two-instances-{n}.log")),
                d.log(),
            );
        }
    }
    drop(pair);
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Counts a writer thread as finished when it ends, however it ends.
struct Finish<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Writer threads the storm starts.
const WRITERS: usize = MEMBERS * 2 + MEMBERS * 2 + MEMBERS + 3;

/// A unique idempotency key.
fn uid() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "two-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

/// Times a write was refused `serialization_conflict` and sent again.
static RETRIED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Send until the answer is not one the API says to send again: a
/// `serialization_conflict` ("other writes to the same rows won, nothing was
/// applied") or an `idempotency_in_flight` (the twin is still running). The
/// caller keeps one idempotency key across the sends, as the SDK does.
fn persist(mut send: impl FnMut() -> Answer) -> Answer {
    for n in 0..40u64 {
        let a = send();
        let again = a.status == 409
            && matches!(
                error_code(&a).as_str(),
                "serialization_conflict" | "idempotency_in_flight"
            );
        if !again {
            return a;
        }
        RETRIED.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20 + 10 * (n % 8)));
    }
    send()
}

/// A file row on `note` and its bytes, put through `seat`'s instance.
fn attach_file(seat: &Seat, note: &str, inst: &'static str, n: usize, ledger: &Ledger) {
    let key = uid();
    let a = persist(|| {
        seat.post(
            "addFile",
            &json!({"note": note, "name": format!("f-{inst}-{n}.txt")}),
            &key,
        )
    });
    let Some(id) = created_id(&a) else {
        ledger.problem(format!("addFile on {inst}: {} {}", a.status, a.text()));
        return;
    };
    let bytes = format!("bytes of {id} through {inst}, {}", "x".repeat(n * 37)).into_bytes();
    let up = persist(|| {
        actors::upload(
            seat.d,
            seat.space,
            &seat.token,
            &format!("/blob/files/plain/{id}"),
            "text/plain",
            &bytes,
        )
    });
    if !(200..300).contains(&up.status) {
        ledger.problem(format!(
            "upload {id} on {inst}: {} {}",
            up.status,
            up.text()
        ));
        return;
    }
    ledger.files.lock().expect("l").push((id, bytes, inst));
}

/// Ann shares a note through A; Gus takes it up on B and reads it on both;
/// Ann revokes through A; Gus is refused on both.
fn share_and_revoke(pair: &Pair, ann: &Member, gus: &Person, t: &str, report: &Mutex<Vec<String>>) {
    let say = |s: String| report.lock().expect("r").push(s);
    let sa = Seat::new(&pair.a, t, &ann.p.token);
    let key = uid();
    let made = persist(|| sa.create("shared-with-gus", &key));
    let Some(note) = created_id(&made) else {
        say(format!("note: {} {}", made.status, made.text()));
        say("done".into());
        return;
    };
    let key = uid();
    let ticket = persist(|| {
        sa.post(
            "std.space.share",
            &json!({
                "ops": "myNotes,noteById,std.notes.get",
                "rows": json!([{"entity": "notes", "id": note, "level": "read"}]).to_string(),
                "expires": "30d",
                "label": "For Gus",
            }),
            &key,
        )
    });
    if ticket.status != 200 {
        say(format!("share: {} {}", ticket.status, ticket.text()));
        say("done".into());
        return;
    }
    let ticket = ticket.json()["rows"][0]["ticket"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let accepted = pair.b.auth(
        "/auth/share/accept",
        &json!({"ticket": ticket, "display_name": "Gus"}),
        Some(&gus.token),
    );
    mark("share: accepted on B");
    if accepted.status != 200 {
        let on_a = pair.a.auth(
            "/auth/share/accept",
            &json!({"ticket": ticket, "display_name": "Gus"}),
            Some(&gus.token),
        );
        say(format!(
            "accept on B: {} {} (the same ticket on A: {} {})",
            accepted.status,
            accepted.text(),
            on_a.status,
            on_a.text()
        ));
        say("done".into());
        return;
    }
    let window = (0..100)
        .find_map(|_| {
            let w = pair
                .b
                .auth("/auth/window", &json!({"space": t}), Some(&gus.token));
            if w.status == 200 {
                return w.json()["token"].as_str().map(str::to_string);
            }
            std::thread::sleep(Duration::from_millis(100));
            None
        })
        .unwrap_or_default();
    if window.is_empty() {
        say("no window opened on B".into());
        say("done".into());
        return;
    }
    for (name, d) in [("A", &pair.a), ("B", &pair.b)] {
        let r = Seat::new(d, t, &window).get("noteById", &json!({"id": note}));
        if r.status != 200 || r.json()["rows"][0]["id"] != json!(note) {
            say(format!(
                "the guest cannot read the shared note on {name}: {} {}",
                r.status,
                r.text()
            ));
        }
    }
    // The seat to revoke.
    let shares = sa.get("std.space.shares", &json!({}));
    let seat = shares.json()["rows"]
        .as_array()
        .and_then(|rs| rs.iter().find(|r| r["displayName"] == json!("Gus")))
        .and_then(|r| r["seat"].as_str().map(str::to_string));
    let Some(seat) = seat else {
        say(format!(
            "no seat for Gus in std.space.shares: {} {}",
            shares.status,
            shares.text()
        ));
        say("done".into());
        return;
    };
    mark("share: revoke sent");
    let revoked = sa.post("std.space.revokeShare", &json!({"seat": seat}), &uid());
    if revoked.status != 200 {
        say(format!("revoke: {} {}", revoked.status, revoked.text()));
    }
    for (name, d) in [("A", &pair.a), ("B", &pair.b)] {
        let clock = Instant::now();
        let refused = loop {
            let r = Seat::new(d, t, &window).get("noteById", &json!({"id": note}));
            if r.status == 401 || r.status == 403 {
                break true;
            }
            if clock.elapsed() > CROSS_BOUND {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if !refused {
            say(format!(
                "the guest still reads the note on {name} {CROSS_BOUND:?} after the revoke"
            ));
        }
    }
    say("done".into());
}

/// Dee signs in on one instance and out on the other, over and over.
fn sessions_churn(pair: &Pair, dee: &Person, t: &str, report: &Mutex<Vec<String>>) {
    let say = |s: String| report.lock().expect("r").push(s);
    for i in 0..8 {
        let (first, second) = if i % 2 == 0 {
            (&pair.a, &pair.b)
        } else {
            (&pair.b, &pair.a)
        };
        mark(format!("churn {i}: sign-in on :{}", first.port));
        let one = actors::sign_in(first, &dee.device, json!({"space": t}));
        let two = actors::sign_in(second, &dee.device, json!({"space": t}));
        if one.status != 200 || two.status != 200 {
            say(format!("sign-in {i}: {} / {}", one.status, two.status));
            continue;
        }
        let tok_one = one.json()["token"].as_str().unwrap_or("").to_string();
        let tok_two = two.json()["token"].as_str().unwrap_or("").to_string();
        // Each session works where it was made.
        for (d, tok) in [(first, &tok_one), (second, &tok_two)] {
            let r = Seat::new(d, t, tok).get("myNotes", &json!({}));
            if r.status != 200 {
                say(format!(
                    "sign-in {i}: a fresh session read {} {}",
                    r.status,
                    r.text()
                ));
            }
        }
        // `tok_one` is ended on the other instance.
        mark(format!("churn {i}: logout on :{}", second.port));
        let out = second.auth("/auth/logout", &json!({}), Some(&tok_one));
        if out.status >= 300 {
            say(format!("logout {i}: {} {}", out.status, out.text()));
        }
        // The other session is untouched.
        let r = Seat::new(second, t, &tok_two).get("myNotes", &json!({}));
        if r.status != 200 {
            say(format!(
                "logout {i} ended another session: {} {}",
                r.status,
                r.text()
            ));
        }
    }
    say("done".into());
}

// ── test 2: an ended session ────────────────────────────────────────────

/// How long `d` takes to refuse `token` (401 or 403) in `t`, up to `within`.
fn refused_after(d: &Deployment, t: &str, token: &str, within: Duration) -> Option<Duration> {
    let started = Instant::now();
    loop {
        let r = Seat::new(d, t, token).get("myNotes", &json!({}));
        if r.status == 401 || r.status == 403 {
            return Some(started.elapsed());
        }
        if started.elapsed() > within {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots two servers; scripts/two-instances.sh"]
async fn an_ended_session_is_refused_on_the_other_instance() {
    let (pair, (pat, quin, ann, gus)) = Pair::boot("ordo_two_sessions", |a, olga| {
        (
            actors::join(a, olga, 4, "Pat", "manager"),
            actors::join(a, olga, 5, "Quin", "manager"),
            actors::join(a, olga, 6, "Ann", "manager"),
            actors::found(a, 7, "Gus"),
        )
    })
    .await;
    let t = pair.space.clone();
    let olga_b = pair.on_b(&pair.olga);
    pair.b_serves(&olga_b, Duration::from_secs(30))
        .expect("B serves the tenant");

    // 1. A logout on A ends the session on B.
    let on_b = Seat::new(&pair.b, &t, &pat.token).get("myNotes", &json!({}));
    assert_eq!(
        on_b.status,
        200,
        "A's session works on B before the logout: {}",
        on_b.text()
    );
    let out = pair.a.auth("/auth/logout", &json!({}), Some(&pat.token));
    assert!(out.status < 300, "logout: {} {}", out.status, out.text());
    let took = refused_after(&pair.b, &t, &pat.token, CROSS_BOUND)
        .unwrap_or_else(|| panic!("B still serves a logged-out session after {CROSS_BOUND:?}"));
    eprintln!("logout crossed to B in {took:?}");
    assert!(
        refused_after(&pair.a, &t, &pat.token, Duration::from_secs(2)).is_some(),
        "A refuses the session it ended"
    );

    // 2. A logout-everywhere on A ends every session, on both.
    let quin_b = pair.on_b(&quin);
    for (name, d, tok) in [("A", &pair.a, &quin.token), ("B", &pair.b, &quin_b)] {
        let r = Seat::new(d, &t, tok).get("myNotes", &json!({}));
        assert_eq!(r.status, 200, "Quin reads on {name}: {}", r.text());
    }
    let out = pair
        .a
        .auth("/auth/logout-all", &json!({}), Some(&quin.token));
    assert!(
        out.status < 300,
        "logout-all: {} {}",
        out.status,
        out.text()
    );
    let took = refused_after(&pair.b, &t, &quin_b, CROSS_BOUND)
        .unwrap_or_else(|| panic!("B still serves a session of Quin's after logout-all"));
    eprintln!("logout-all crossed to B in {took:?}");
    assert!(
        refused_after(&pair.b, &t, &quin.token, CROSS_BOUND).is_some(),
        "B refuses the session A made, too"
    );
    // A sign-in after that second is not touched.
    std::thread::sleep(Duration::from_millis(1_200));
    let again = sign_in_token(&pair.b, &quin);
    let r = Seat::new(&pair.b, &t, &again).get("myNotes", &json!({}));
    assert_eq!(r.status, 200, "a new session works on B: {}", r.text());

    // 3. A revoked share is refused on B (the walk is `share_and_revoke`'s).
    let report = Mutex::new(Vec::new());
    let tb = pair.on_b(&ann);
    let ann = Member { p: ann, tb };
    share_and_revoke(&pair, &ann, &gus, &t, &report);
    let report = report.into_inner().expect("report");
    assert_eq!(
        report,
        vec!["done".to_string()],
        "the share walk: {report:?}"
    );
}

// ── test 3: the leader ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots two servers; scripts/two-instances.sh"]
async fn leader_only_workers_run_on_one_instance() {
    let (pair, ()) = Pair::boot("ordo_two_leader", |_, _| ()).await;
    let (ta, tb) = (pair.dana_token(&pair.a), pair.dana_token(&pair.b));

    // One leader, once the contention settles.
    let started = Instant::now();
    let (ha, hb) = loop {
        let (ha, hb) = (pair.health(&pair.a, &ta), pair.health(&pair.b, &tb));
        let lead = |h: &Value| h["supervisor"]["leadership"]["leader"] == json!(true);
        if lead(&ha) != lead(&hb) {
            break (ha, hb);
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no single leader after 30s: A {} B {}",
            ha["supervisor"]["leadership"],
            hb["supervisor"]["leadership"]
        );
        std::thread::sleep(Duration::from_millis(300));
    };
    let a_leads = ha["supervisor"]["leadership"]["leader"] == json!(true);
    let (lead, follow, lname, fname) = if a_leads {
        (&ha, &hb, "A", "B")
    } else {
        (&hb, &ha, "B", "A")
    };
    assert_eq!(lead["supervisor"]["leadership"]["wanted"], json!(true));
    assert_eq!(follow["supervisor"]["leadership"]["wanted"], json!(true));

    // The leader-only workers carry `leader: true` on the leader and run
    // nowhere else.
    let leader_only: Vec<&str> = lead["workers"]
        .as_object()
        .expect("workers")
        .iter()
        .filter(|(_, w)| w["leader"] == json!(true))
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(
        !leader_only.is_empty(),
        "the leader runs leader-only workers"
    );
    for name in &leader_only {
        let w = &follow["workers"][*name];
        assert_ne!(
            w["leader"],
            json!(true),
            "{name} is marked leader on {fname} too: {w}"
        );
        assert_eq!(
            w["runs"],
            json!(0),
            "{name} ran {} times on the follower {fname}: {w}",
            w["runs"]
        );
    }
    eprintln!("{lname} leads; leader-only workers: {leader_only:?}");

    // An apply queues behind one that is running, across processes. The
    // test holds the apply lock as a running apply would, and sends an
    // apply to the instance that does not lead.
    let pool = ordo_exec::Pool::connect(authz_support::cfg(&pair.a.db))
        .await
        .expect("a pool for the lock");
    let lock = ordo_host::apply_op::ApplyLock::take(&pool)
        .await
        .expect("the lock is taken")
        .expect("the lock is free");
    let hold = Duration::from_secs(3);
    let (target, dana, token) = if a_leads {
        (&pair.b, &pair.dana, &tb)
    } else {
        (&pair.a, &pair.dana, &ta)
    };
    let sent = Instant::now();
    let done = std::thread::scope(|s| {
        let h = s.spawn(|| {
            let present = actors::step_up(target, dana, token);
            let a = actors::op(
                target,
                HOME,
                &present,
                "POST",
                "std.deployment.apply",
                &json!({"program": authz_support::SOURCE}),
            );
            (a, Instant::now())
        });
        std::thread::sleep(hold);
        assert!(!h.is_finished(), "the apply waited for the running one");
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(lock.release()));
        h.join().expect("the apply thread")
    });
    let (done, at) = done;
    assert_eq!(done.status, 200, "the queued apply: {}", done.text());
    assert!(
        at - sent >= hold,
        "the apply finished {:?} after it was sent, inside the {hold:?} hold",
        at - sent
    );
}

// ── test 4: the guessing slowdown ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
#[ignore = "boots two servers; scripts/two-instances.sh"]
async fn the_guessing_slowdown_counts_tries_on_both() {
    let (pair, (secret, other)) = Pair::boot("ordo_two_guess", |a, olga| {
        let note = created_id(&Seat::new(a, &olga.space, &olga.token).create("linked", &uid()))
            .expect("a note");
        let (one, _) = actors::link(a, olga, &note, json!({"password": "correct horse"}));
        let (two, _) = actors::link(a, olga, &note, json!({"password": "correct horse"}));
        (one, two)
    })
    .await;
    let olga_b = pair.on_b(&pair.olga);
    pair.b_serves(&olga_b, Duration::from_secs(30))
        .expect("B serves the tenant");

    // The default slows after 5 wrong tries. 3 on A and 2 on B are 5 in all,
    // and 3 on one instance alone would not slow anything.
    let wrong = |d: &Deployment, link: &str| {
        let r = actors::open_link(d, link, Some("a guess"), None);
        (r.status, error_code(&r))
    };
    for n in 0..3 {
        let (s, c) = wrong(&pair.a, &secret);
        assert_eq!(s, 401, "wrong try {n} on A: {c}");
    }
    for n in 0..2 {
        let (s, c) = wrong(&pair.b, &secret);
        assert_eq!(s, 401, "wrong try {n} on B: {c}");
    }
    // The 6th try, on either instance, is slowed and not counted.
    let (s, c) = wrong(&pair.a, &secret);
    assert_eq!(
        (s, c.as_str()),
        (429, "too_many_attempts"),
        "A slows the link after 3 + 2 tries"
    );
    let (s, c) = wrong(&pair.b, &secret);
    assert_eq!(
        (s, c.as_str()),
        (429, "too_many_attempts"),
        "B slows the link after 3 + 2 tries"
    );
    // The right password is inside the wait too, and is not checked.
    let right = actors::open_link(&pair.b, &secret, Some("correct horse"), None);
    assert_eq!(right.status, 429, "a try in the wait is not checked");

    // Another link is not slowed by it.
    let r = actors::open_link(&pair.b, &other, Some("correct horse"), None);
    assert_eq!(r.status, 200, "another link opens: {}", r.text());

    // After the wait, the right password opens it on the other instance
    // from the one that was guessed against, and resets the count.
    let started = Instant::now();
    let opened = loop {
        let r = actors::open_link(&pair.a, &secret, Some("correct horse"), None);
        if r.status != 429 {
            break r;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "still slowed after 10s"
        );
        std::thread::sleep(Duration::from_millis(300));
    };
    assert_eq!(opened.status, 200, "the right password: {}", opened.text());
    let (s, c) = wrong(&pair.b, &secret);
    assert_eq!(s, 401, "the count started again: {c}");
}
