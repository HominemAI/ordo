//! **Enrolment E5: first come, first served.** A tenant's `firstComer`
//! setting (`std.space.setAdmission`), off by default, lets the first
//! person to arrive at an empty tenant own it. The claim code that once
//! guarded it was removed (owner, 2026-10-30), and the deployment's own
//! space never admits a first-comer: deployment people come from the
//! identity service (`plan-identity-service.md` 6).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::{Fixture, Pg};
use ordo_host::admission::{
    Admission, Arrival, Ceiling, Outcome, PolicyChange, PolicyStore, Way, SET_ADMISSION,
};
use ordo_host::member_ops::MemberOps;
use serde_json::json;

const T0: u64 = 1_000_000;
/// The deployment's own space, which never admits a first-comer.
const HOME: &str = "ordo_home_e5";
/// A space nobody has ever been admitted to.
const EMPTY: &str = "empty_tenant";

struct Harness {
    admission: Arc<Admission>,
    pool: ordo_exec::Pool,
}

async fn harness(db: &str, ceiling: Ceiling) -> Harness {
    let f = Fixture::load().expect("fixture");
    f.reset(&Pg::named(db)).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 4,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("tables");
    for t in ["$admissionpolicy", "$firstcomer", "$deploymentsetting"] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(Arc::default()),
        refresh: None,
        now: Arc::new(|| T0),
        managed: None,
    });
    Harness {
        admission: Arc::new(Admission {
            members,
            store: PolicyStore::new(pool.clone()),
            tickets: None,
            ceiling,
            home: Some(HOME.to_string()),
        }),
        pool,
    }
}

impl Harness {
    async fn arrive(&self, who: &str, space: &str, _old_code: Option<&str>) -> Outcome {
        self.admission
            .on_arrival(who, space, &Arrival::default())
            .await
            .expect("decided")
    }

    async fn first_comer_on(&self, space: &str) {
        self.admission
            .set_policy(
                space,
                &PolicyChange {
                    first_comer: Some(true),
                    ..Default::default()
                },
                "test",
                T0,
            )
            .await
            .expect("within the ceiling");
    }

    async fn role_of(&self, who: &str, space: &str) -> Option<String> {
        self.pool
            .rows(&format!(
                "SELECT \"role\" FROM \"members\" WHERE \"$space\" = '{space}' AND \"principal\" = '{who}'"
            ))
            .await
            .expect("read")
            .first()
            .and_then(|r| r[0].clone())
    }
}

/// **The first sign-in to an empty tenant owns it; the second does not.**
#[tokio::test(flavor = "multi_thread")]
async fn the_first_sign_in_to_an_empty_tenant_owns_it_and_the_second_does_not() {
    let h = harness("ordo_first_comer_basic", Ceiling::everything()).await;
    h.first_comer_on(EMPTY).await;

    let got = h.arrive("usr_first", EMPTY, None).await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::FirstComer, .. } if role == "owner"),
        "{got:?}"
    );
    assert_eq!(
        h.role_of("usr_first", EMPTY).await.as_deref(),
        Some("owner")
    );

    assert_eq!(h.arrive("usr_second", EMPTY, None).await, Outcome::Refused);
    assert_eq!(h.role_of("usr_second", EMPTY).await, None);
}

/// Two people arriving at once: one owns it. The latch is the database's, so
/// this holds across replicas as much as across tasks.
#[tokio::test(flavor = "multi_thread")]
async fn two_arriving_at_once_do_not_both_own_it() {
    let h = harness("ordo_first_comer_race", Ceiling::everything()).await;
    h.first_comer_on(EMPTY).await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let admission = h.admission.clone();
        tasks.push(tokio::spawn(async move {
            admission
                .on_arrival(&format!("usr_racer_{i}"), EMPTY, &Arrival::default())
                .await
                .expect("decided")
        }));
    }
    let mut owners = 0;
    for t in tasks {
        if matches!(
            t.await.unwrap(),
            Outcome::Admitted {
                way: Way::FirstComer,
                ..
            }
        ) {
            owners += 1;
        }
    }
    assert_eq!(owners, 1, "exactly one arrival claims an empty tenant");
}

/// **It closes itself permanently.** Even a tenant emptied by hand is not
/// open to the next arrival, because the latch is a fact and not a count.
#[tokio::test(flavor = "multi_thread")]
async fn it_closes_itself_permanently_the_moment_it_is_used() {
    let h = harness("ordo_first_comer_latch", Ceiling::everything()).await;
    h.first_comer_on(EMPTY).await;
    assert!(matches!(
        h.arrive("usr_first", EMPTY, None).await,
        Outcome::Admitted { .. }
    ));
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{EMPTY}'"
        ))
        .await
        .expect("emptied by hand");
    assert_eq!(
        h.arrive("usr_opportunist", EMPTY, None).await,
        Outcome::Refused
    );
}

/// **First-comer is off when nothing turns it on**, and on in an app
/// tenant only where its owner turned it on. A sign-in that still carries
/// a claim code is admitted only by the setting, and when refused it is
/// told the code is no longer read and which setting admits a first-comer.
#[tokio::test(flavor = "multi_thread")]
async fn an_app_tenant_refuses_a_first_comer_with_the_setting_named_until_it_is_on() {
    let h = harness("ordo_first_comer_app", Ceiling::everything()).await;
    // Off: refused, code or no code.
    assert_eq!(h.arrive("usr_other", EMPTY, None).await, Outcome::Refused);
    assert_eq!(
        h.arrive("usr_coder", EMPTY, Some("1a2b-3c4d-5e6f-7a8b"))
            .await,
        Outcome::Refused
    );
    let said = ordo_host::auth::admission_answer(
        EMPTY,
        "refused",
        &json!({"space": EMPTY, "claim_code": "1a2b-3c4d-5e6f-7a8b"}),
    );
    let message = said["message"].as_str().expect("a message");
    assert!(message.contains("claim codes are no longer read"), "{said}");
    assert!(message.contains("`firstComer=true`"), "{said}");
    assert!(
        message.contains("ORDO_AUTH_PROVISIONING=auto-onboard"),
        "{said}"
    );
    assert_eq!(said["setting"], ordo_host::admission::FIRST_COMER_SETTING);
    // Without a code, or admitted, nothing more is said.
    let plain = ordo_host::auth::admission_answer(EMPTY, "refused", &json!({"space": EMPTY}));
    assert!(plain.get("message").is_none(), "{plain}");
    let admitted = ordo_host::auth::admission_answer(
        EMPTY,
        "admitted",
        &json!({"space": EMPTY, "claim_code": "x"}),
    );
    assert!(admitted.get("message").is_none(), "{admitted}");

    // On: the first arrival owns it, with or without an old code.
    h.first_comer_on(EMPTY).await;
    let got = h
        .arrive("usr_first", EMPTY, Some("1a2b-3c4d-5e6f-7a8b"))
        .await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::FirstComer, .. } if role == "owner"),
        "{got:?}"
    );

    // And a ceiling that forbids it cannot be talked round.
    let narrow = harness("ordo_first_comer_app_narrow", Ceiling::invitation_only()).await;
    assert!(
        narrow
            .admission
            .set_policy(
                EMPTY,
                &PolicyChange {
                    first_comer: Some(true),
                    ..Default::default()
                },
                "test",
                T0,
            )
            .await
            .is_err(),
        "the ceiling refuses to switch it on"
    );
}

/// **The deployment's own space never admits a first-comer** (owner,
/// 2026-10-30): switching it on there is refused by name, and a policy row
/// written before still admits nobody.
#[tokio::test(flavor = "multi_thread")]
async fn the_home_space_refuses_a_first_comer() {
    let h = harness("ordo_first_comer_home", Ceiling::everything()).await;
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());
    let err = h
        .admission
        .execute(
            SET_ADMISSION,
            HOME,
            &bound(json!({"firstComer": true})),
            "mem_o",
        )
        .await
        .expect_err("refused at home");
    assert_eq!(err.code, ErrorCode::TenantRefused);
    assert!(
        err.message.contains("never admits a first-comer"),
        "{err:?}"
    );
    assert!(
        err.message.contains("ordo-identity-service invite"),
        "{err:?}"
    );

    // A row from before the removal, with first-comer on at home.
    let mut p = h.admission.store.get(HOME).await.expect("policy");
    p.first_comer = true;
    h.admission
        .store
        .put(HOME, &p, "old", T0)
        .await
        .expect("written");
    assert_eq!(h.arrive("usr_first", HOME, None).await, Outcome::Refused);
    assert_eq!(h.role_of("usr_first", HOME).await, None);

    // Switching it off at home is still allowed.
    h.admission
        .execute(
            SET_ADMISSION,
            HOME,
            &bound(json!({"firstComer": false})),
            "mem_o",
        )
        .await
        .expect("off is fine");
}

/// A deployment that still sets `ORDO_AUTH_CLAIM_CODE` boots; the log says
/// it is ignored and names the first-comer setting. Any value, even one the
/// old door refused, and the `_FILE` form.
#[test]
fn a_claim_code_still_set_is_named_as_ignored_and_never_refuses() {
    use ordo_host::admission::ignored_claim_code_note;
    for (k, v) in [
        ("ORDO_AUTH_CLAIM_CODE", "required"),
        ("ORDO_AUTH_CLAIM_CODE", "sometimes"),
        ("ORDO_AUTH_CLAIM_CODE_FILE", "/run/secrets/claim"),
    ] {
        let note = ignored_claim_code_note(&|n: &str| (n == k).then(|| v.to_string()))
            .expect("a log line");
        assert!(
            note.starts_with("ORDO_AUTH_CLAIM_CODE is set and ignored"),
            "{note}"
        );
        assert!(note.contains("`firstComer=true`"), "{note}");
    }
    assert_eq!(ignored_claim_code_note(&|_| None), None);
    assert_eq!(ignored_claim_code_note(&|_| Some(" ".into())), None);
}
