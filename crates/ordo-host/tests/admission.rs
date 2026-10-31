//! **Enrolment E4: admission policy per tenant, under the deployment's
//! ceiling.** Against real Postgres.
//!
//! What is proved here:
//!
//! - a tenant set to `open` admits a stranger as a viewer, and as another
//!   role when the app developer set one;
//! - the same tenant under an invitation-only ceiling cannot be set to
//!   `open`, and a policy written under a wider ceiling is not honoured under
//!   a narrower one;
//! - invitation and provider rules admit, at the roles they name, and the
//!   refusal is one refusal;
//! - a tenant that switched nothing on admits nobody.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::{Fixture, Pg};
use ordo_host::admission::{
    parse_providers, Admission, Arrival, Ceiling, Outcome, PolicyChange, PolicyStore, Via, Way,
    SET_ADMISSION,
};
use ordo_host::member_ops::MemberOps;
use ordo_host::tickets::{MemoryTicketStore, Purpose, TicketSpec, Tickets};
use ordo_identity::principal::PrincipalId;
use serde_json::json;

const T0: u64 = 1_000_000;

struct Harness {
    admission: Admission,
    tickets: Arc<Tickets>,
    pool: ordo_exec::Pool,
    space: String,
    /// The stream the members' notes go to.
    recorder: Arc<ordo_secevent::Recorder>,
    /// Held for the harness's life. Every harness uses the fixture's one
    /// space, and `setAdmission` writes that space's entry in the
    /// process-global upstream acceptance, so two harnesses running at once
    /// would erase each other's `presenceFrom` between a write and its read.
    _serial: tokio::sync::OwnedMutexGuard<()>,
}

static GLOBAL_ACCEPTANCE: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> =
    std::sync::OnceLock::new();

async fn harness(db: &str, ceiling: Ceiling) -> Harness {
    harness_in(db, ceiling, None).await
}

async fn harness_in(
    db: &str,
    ceiling: Ceiling,
    directory: Option<Arc<dyn ordo_identity::directory::DirectoryStore>>,
) -> Harness {
    let serial = GLOBAL_ACCEPTANCE
        .get_or_init(Default::default)
        .clone()
        .lock_owned()
        .await;
    let f = Fixture::load().expect("fixture");
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("policy table");
    let _ = pool.rows("DELETE FROM \"$admissionpolicy\"").await;

    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory,
        stream: Some(recorder.clone()),
        refresh: None,
        now: Arc::new(|| T0),
        managed: None,
    });
    let clock = Arc::new(AtomicU64::new(T0));
    let tickets = Arc::new(
        Tickets::new(Arc::new(MemoryTicketStore::new()))
            .with_clock(Box::new(move || clock.load(Ordering::SeqCst))),
    );
    Harness {
        admission: Admission {
            members,
            store: PolicyStore::new(pool.clone()),
            tickets: Some(tickets.clone()),
            ceiling,
            home: None,
        },
        tickets,
        pool,
        space: ordo_conformance::fixture_space().0,
        recorder,
        _serial: serial,
    }
}

impl Harness {
    async fn arrive(&self, who: &str, arrival: Arrival<'_>) -> Outcome {
        self.admission
            .on_arrival(who, &self.space, &arrival)
            .await
            .expect("the arrival is decided")
    }

    async fn role_of(&self, who: &str) -> Option<String> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"role\" FROM \"members\" WHERE \"$space\" = '{}' AND \"principal\" = '{who}'",
                self.space
            ))
            .await
            .expect("read");
        rows.first().and_then(|r| r[0].clone())
    }

    async fn display_of(&self, who: &str) -> Option<String> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"displayName\" FROM \"members\" WHERE \"$space\" = '{}' AND \"principal\" = '{who}'",
                self.space
            ))
            .await
            .expect("read");
        rows.first().and_then(|r| r[0].clone())
    }

    async fn set(&self, change: PolicyChange) -> Result<(), ordo_api::ExecError> {
        self.admission
            .set_policy(&self.space, &change, "mem_o", T0)
            .await
            .map(|_| ())
    }

    fn invitation(&self, space: &str, role: &str, principal: Option<&str>) -> String {
        self.tickets
            .issue(TicketSpec {
                purpose: Purpose::Invitation,
                principal: principal.map(|p| PrincipalId(p.into())),
                display: "Guest".into(),
                space: Some(space.into()),
                role: Some(role.into()),
                issued_by: "mem_o".into(),
                ttl_secs: 0,
            })
            .expect("issued")
            .ticket
    }
}

fn open() -> PolicyChange {
    PolicyChange {
        open: Some(true),
        ..Default::default()
    }
}

/// **A tenant set to `open` admits a stranger's first sign-in as a viewer.**
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_set_to_open_admits_a_strangers_first_sign_in_as_a_viewer() {
    let h = harness("ordo_admission_open", Ceiling::everything()).await;
    assert_eq!(
        h.arrive("usr_stranger", Arrival::default()).await,
        Outcome::Refused,
        "the control: a tenant that chose nothing admits nobody"
    );
    assert_eq!(h.role_of("usr_stranger").await, None);

    h.set(open()).await.expect("open is within the ceiling");
    let got = h.arrive("usr_stranger", Arrival::default()).await;
    let Outcome::Admitted { member, role, way } = got else {
        panic!("expected an admission, got {got:?}");
    };
    assert_eq!(role, "viewer");
    assert_eq!(way, Way::Open);
    assert!(member.starts_with("mem_"));
    assert_eq!(h.role_of("usr_stranger").await.as_deref(), Some("viewer"));

    // The second sign-in is the same member, not a second row.
    assert_eq!(
        h.arrive("usr_stranger", Arrival::default()).await,
        Outcome::AlreadyMember { member }
    );
}

/// The app developer may change the default (owner, 2026-09-29).
#[tokio::test(flavor = "multi_thread")]
async fn the_app_developer_may_change_the_default_role() {
    let h = harness("ordo_admission_role", Ceiling::everything()).await;
    h.set(PolicyChange {
        open: Some(true),
        default_role: Some("editor".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    let got = h.arrive("usr_a", Arrival::default()).await;
    assert!(
        matches!(&got, Outcome::Admitted { role, .. } if role == "editor"),
        "{got:?}"
    );
    assert_eq!(h.role_of("usr_a").await.as_deref(), Some("editor"));
}

/// **The same tenant under an invitation-only ceiling cannot be set to
/// `open`.** And a policy written under a wider ceiling is not honoured once
/// the ceiling narrows.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_tenant_under_an_invitation_only_ceiling_cannot_be_set_to_open() {
    let narrow = harness("ordo_admission_ceiling", Ceiling::invitation_only()).await;
    let err = narrow
        .set(open())
        .await
        .expect_err("open is wider than the ceiling");
    assert_eq!(err.code, ErrorCode::TenantRefused);
    assert!(err.message.contains("ceiling"), "{}", err.message);
    for change in [
        PolicyChange {
            first_comer: Some(true),
            ..Default::default()
        },
        PolicyChange {
            providers: Some(parse_providers("oidc:acme=viewer").unwrap()),
            ..Default::default()
        },
    ] {
        assert!(narrow.set(change).await.is_err());
    }
    assert_eq!(
        narrow.arrive("usr_stranger", Arrival::default()).await,
        Outcome::Refused
    );

    // One harness at a time holds the global acceptance's lock.
    drop(narrow);

    // A policy written while the ceiling was wider, and read under a
    // narrower one on the next boot, admits nobody through the way that was
    // taken away.
    let wide = harness("ordo_admission_ceiling_lowered", Ceiling::everything()).await;
    wide.set(open()).await.unwrap();
    let lowered = Admission {
        members: wide.admission.members.clone(),
        store: wide.admission.store.clone(),
        tickets: None,
        ceiling: Ceiling::invitation_only(),
        home: None,
    };
    let out = lowered
        .on_arrival("usr_late", &wide.space, &Arrival::default())
        .await
        .unwrap();
    assert_eq!(out, Outcome::Refused);
}

/// An invitation admits at the role it names, once, for the tenant it names,
/// and for the person it names when it names one. Under any ceiling.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_admits_once_at_its_role_and_every_other_case_is_the_one_refusal() {
    let h = harness("ordo_admission_invite", Ceiling::invitation_only()).await;

    let good = h.invitation(&h.space, "editor", None);
    let got = h
        .arrive(
            "usr_guest",
            Arrival {
                ticket: Some(&good),
                ..Default::default()
            },
        )
        .await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::Invitation, .. } if role == "editor"),
        "{got:?}"
    );

    // Spent.
    assert_eq!(
        h.arrive(
            "usr_other",
            Arrival {
                ticket: Some(&good),
                ..Default::default()
            }
        )
        .await,
        Outcome::Refused
    );
    // For another tenant.
    let elsewhere = h.invitation("some_other_space", "editor", None);
    assert_eq!(
        h.arrive(
            "usr_x",
            Arrival {
                ticket: Some(&elsewhere),
                ..Default::default()
            }
        )
        .await,
        Outcome::Refused
    );
    // Naming somebody else.
    let named = h.invitation(&h.space, "viewer", Some("usr_named"));
    assert_eq!(
        h.arrive(
            "usr_thief",
            Arrival {
                ticket: Some(&named),
                ..Default::default()
            }
        )
        .await,
        Outcome::Refused
    );
    assert_eq!(h.role_of("usr_thief").await, None);
    // Not a ticket at all.
    assert_eq!(
        h.arrive(
            "usr_y",
            Arrival {
                ticket: Some("tkt_nope"),
                ..Default::default()
            }
        )
        .await,
        Outcome::Refused
    );
    // And the named person, who can use it.
    let got = h
        .arrive(
            "usr_named",
            Arrival {
                ticket: Some(&named),
                ..Default::default()
            },
        )
        .await;
    assert!(
        matches!(
            got,
            Outcome::Admitted {
                way: Way::Invitation,
                ..
            }
        ),
        "{got:?}"
    );
}

/// A provider rule admits a person who arrived through that provider, from
/// that domain, at the rule's role, and nobody else.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_rule_admits_its_provider_and_its_domain_at_its_role() {
    let h = harness("ordo_admission_provider", Ceiling::everything()).await;
    h.set(PolicyChange {
        providers: Some(parse_providers("oidc:acme@Acme.com=editor, saml:corp=viewer").unwrap()),
        ..Default::default()
    })
    .await
    .unwrap();
    let via = |provider, domain| Arrival {
        provider: Some(Via { provider, domain }),
        ..Default::default()
    };

    assert_eq!(
        h.arrive("usr_wrong_domain", via("oidc:acme", Some("evil.com")))
            .await,
        Outcome::Refused
    );
    assert_eq!(
        h.arrive("usr_no_domain", via("oidc:acme", None)).await,
        Outcome::Refused
    );
    assert_eq!(
        h.arrive("usr_other_provider", via("oidc:other", Some("acme.com")))
            .await,
        Outcome::Refused
    );
    let got = h.arrive("usr_in", via("oidc:acme", Some("ACME.com"))).await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::Provider, .. } if role == "editor"),
        "{got:?}"
    );
    let got = h.arrive("usr_corp", via("saml:corp", None)).await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::Provider, .. } if role == "viewer"),
        "{got:?}"
    );
}

/// The two ops, through their own entry point: a change is merged, refused
/// by name over the ceiling, and read back.
#[tokio::test(flavor = "multi_thread")]
async fn the_admission_ops_merge_a_change_and_refuse_one_over_the_ceiling() {
    let h = harness("ordo_admission_ops", Ceiling::from_auto_onboard(true)).await;
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());

    let out = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"open": true})),
            "mem_o",
        )
        .await
        .unwrap();
    assert_eq!(out.rows[0]["open"], true);
    assert_eq!(out.rows[0]["role"], "viewer");
    // G12: the change is recorded in the tenant's stream, naming who made it.
    let acts: Vec<String> = h
        .recorder
        .drain_all(ordo_commandlog::Timestamp(T0 + 1))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Enrolment { act, detail } => Some(format!("{act} {detail}")),
            _ => None,
        })
        .collect();
    assert!(
        acts.iter()
            .any(|a| a.starts_with("admission-policy-set") && a.contains("by=mem_o")),
        "{acts:?}"
    );

    // A second change leaves the first alone.
    let out = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"role": "editor"})),
            "mem_o",
        )
        .await
        .unwrap();
    assert_eq!(out.rows[0]["open"], true);
    assert_eq!(out.rows[0]["role"], "editor");

    let err = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"role": "emperor"})),
            "mem_o",
        )
        .await
        .expect_err("not a role");
    assert_eq!(err.code, ErrorCode::TenantRefused);

    let narrow = Admission {
        members: h.admission.members.clone(),
        store: h.admission.store.clone(),
        tickets: None,
        ceiling: Ceiling::invitation_only(),
        home: None,
    };
    let err = narrow
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"firstComer": true})),
            "mem_o",
        )
        .await
        .expect_err("over the ceiling");
    assert_eq!(err.code, ErrorCode::TenantRefused);
}

#[test]
fn provider_rules_parse_and_refuse_what_is_not_a_rule() {
    let rules = parse_providers("oidc:acme@acme.com=editor,saml:corp=viewer").unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].domain.as_deref(), Some("acme.com"));
    assert_eq!(rules[1].domain, None);
    assert!(parse_providers("").unwrap().is_empty());
    assert!(parse_providers("oidc:acme").is_err());
    assert!(parse_providers("oidc:acme=emperor").is_err());
    assert!(parse_providers("=viewer").is_err());
}

/// **Step 10, as ask 10 left it: a tenant accepts a provider's two-step
/// claim for its own data from any connection of the deployment**, one the
/// app developer left `off` included. A name that is no connection is
/// refused, naming the ones there are. The choice reaches the shared
/// acceptance at once, and another replica reads it back from the row.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_accepts_presence_from_any_connection_of_the_deployment() {
    use ordo_identity::{PresenceMode, SpaceId, UpstreamAcceptance};
    let h = harness("ordo_admission_presence", Ceiling::from_auto_onboard(true)).await;
    ordo_host::presence::install_upstream(Arc::new(
        UpstreamAcceptance::new()
            .with_connection("team", PresenceMode::Off)
            .with_connection("corp", PresenceMode::On),
    ));
    let acceptance = ordo_host::presence::upstream().expect("installed");
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());

    let err = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"presenceFrom": "nobody"})),
            "mem_o",
        )
        .await
        .expect_err("no such connection");
    assert_eq!(err.code, ErrorCode::TenantRefused);
    assert!(
        err.message.contains("corp, team"),
        "names the connections there are: {}",
        err.message
    );
    let err = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"presenceFrom": "Not A Name"})),
            "mem_o",
        )
        .await
        .expect_err("not a connection name");
    assert_eq!(err.code, ErrorCode::TenantRefused);

    let out = h
        .admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"presenceFrom": "team"})),
            "mem_o",
        )
        .await
        .unwrap();
    assert_eq!(out.rows[0]["presenceFrom"], "team");
    let space = SpaceId(h.space.clone());
    assert!(acceptance.tenant(&space).contains("team"), "at once");

    // Another replica learns it from the row.
    acceptance.replace_tenants(Default::default());
    assert!(acceptance.tenant(&space).is_empty());
    let n = ordo_host::admission::load_presence_from(&h.pool, &acceptance)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(acceptance.tenant(&space).contains("team"));

    // Cleared, it accepts nothing.
    h.admission
        .execute(
            SET_ADMISSION,
            &h.space,
            &bound(json!({"presenceFrom": ""})),
            "mem_o",
        )
        .await
        .unwrap();
    assert!(acceptance.tenant(&space).is_empty());
}

/// **25ba: a member admitted at sign-in is named by the person's name, by
/// every way in.** The directory holds the name given at sign-up (or the
/// provider's profile name). The id stands in only when there is none.
#[tokio::test(flavor = "multi_thread")]
async fn every_way_in_names_the_member_by_the_name_the_directory_holds() {
    use ordo_identity::directory::{DirectoryStore, MemoryDirectoryStore};
    use ordo_identity::principal::{PrincipalKind, Timestamp};

    let dir = Arc::new(MemoryDirectoryStore::new());
    let mint = |name: &str| {
        dir.create_principal(PrincipalKind::Human, name, Timestamp(1))
            .expect("minted")
            .0
    };
    let (open_p, invited, provider, first, nameless) = (
        mint("Olive Open"),
        mint("Ines Invited"),
        mint("Pia Provider"),
        mint("Fay First"),
        mint(""),
    );
    let h = harness_in("ordo_admission_names", Ceiling::everything(), Some(dir)).await;

    // First-comer: needs a tenant with no members, so a space of its own.
    let empty = "names_first";
    let _ = h.pool.rows("DELETE FROM \"$firstcomer\"").await;
    h.admission
        .set_policy(
            empty,
            &PolicyChange {
                first_comer: Some(true),
                ..Default::default()
            },
            "mem_o",
            T0,
        )
        .await
        .expect("first-comer is within the ceiling");
    let got = h
        .admission
        .on_arrival(&first, empty, &Arrival::default())
        .await
        .expect("decided");
    assert!(
        matches!(
            &got,
            Outcome::Admitted {
                way: Way::FirstComer,
                ..
            }
        ),
        "{got:?}"
    );
    let rows = h
        .pool
        .rows(&format!(
            "SELECT \"displayName\" FROM \"members\" WHERE \"$space\" = '{empty}' AND \"principal\" = '{first}'"
        ))
        .await
        .expect("read");
    assert_eq!(rows[0][0].as_deref(), Some("Fay First"));

    // Invitation.
    let ticket = h.invitation(&h.space, "editor", None);
    let got = h
        .arrive(
            &invited,
            Arrival {
                ticket: Some(&ticket),
                ..Default::default()
            },
        )
        .await;
    assert!(
        matches!(
            &got,
            Outcome::Admitted {
                way: Way::Invitation,
                ..
            }
        ),
        "{got:?}"
    );
    assert_eq!(
        h.display_of(&invited).await.as_deref(),
        Some("Ines Invited")
    );

    // Provider.
    let got = h
        .admission
        .admit_by_connection(&provider, &h.space, "viewer")
        .await
        .expect("decided");
    assert!(
        matches!(
            &got,
            Outcome::Admitted {
                way: Way::Provider,
                ..
            }
        ),
        "{got:?}"
    );
    assert_eq!(
        h.display_of(&provider).await.as_deref(),
        Some("Pia Provider")
    );

    // Open, and the fallback to the id when the directory holds no name.
    h.set(open()).await.expect("open");
    let got = h.arrive(&open_p, Arrival::default()).await;
    assert!(
        matches!(&got, Outcome::Admitted { way: Way::Open, .. }),
        "{got:?}"
    );
    assert_eq!(h.display_of(&open_p).await.as_deref(), Some("Olive Open"));
    h.arrive(&nameless, Arrival::default()).await;
    assert_eq!(
        h.display_of(&nameless).await.as_deref(),
        Some(nameless.as_str())
    );
    h.arrive("usr_unknown", Arrival::default()).await;
    assert_eq!(
        h.display_of("usr_unknown").await.as_deref(),
        Some("usr_unknown")
    );
}
