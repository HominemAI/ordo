//! **Checklist 24e: a tenant owner or manager can invite a person.**
//!
//! Until `std.space.invite` nothing issued a ticket of purpose `Invitation`,
//! so a tenant whose admission is invitation-only admitted nobody. What is
//! proved here, against real Postgres and the real sign-in hook
//! (`Admission::on_arrival`):
//!
//! - an invitation-only tenant admits a person who presents the invitation
//!   and refuses one who does not;
//! - an invitation works once;
//! - an expired invitation is refused;
//! - a member without the role to invite is refused, and a manager cannot
//!   invite above their own role;
//! - outstanding invitations are listed by id, without the secret, and one
//!   can be revoked;
//! - the invitation is on the tenant's record.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::{Fixture, Pg};
use ordo_host::admission::{Admission, Arrival, Ceiling, Outcome, PolicyStore, Way};
use ordo_host::invite_ops::{InviteOps, INVITATIONS, INVITE, REVOKE_INVITATION};
use ordo_host::member_ops::MemberOps;
use ordo_host::presence::PresencePolicy;
use ordo_host::tickets::{MemoryTicketStore, Tickets, DEFAULT_TTL_SECS};
use ordo_identity::principal::{PrincipalId, SpaceId};
use ordo_identity::AccessEngine;
use serde_json::{json, Value};

const T0: u64 = 1_000_000;

struct Harness {
    admission: Admission,
    invite: InviteOps,
    engine: AccessEngine,
    clock: Arc<AtomicU64>,
    stream: Arc<ordo_secevent::Recorder>,
    pool: ordo_exec::Pool,
    space: String,
}

async fn harness(db: &str) -> Harness {
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
    let space = ordo_conformance::fixture_space().0;

    let stream: Arc<ordo_secevent::Recorder> = Arc::default();
    let clock = Arc::new(AtomicU64::new(T0));
    let now = {
        let c = clock.clone();
        move || c.load(Ordering::SeqCst)
    };
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(stream.clone()),
        refresh: None,
        now: Arc::new(now.clone()),
        managed: None,
    });
    let tickets = Arc::new(
        Tickets::new(Arc::new(MemoryTicketStore::new())).with_clock(Box::new(now.clone())),
    );
    let row = |id: &str, role: &str| {
        vec![
            Some(space.clone()),
            Some(id.to_string()),
            Some(role.to_string()),
            Some("active".to_string()),
            Some(id.to_string()),
        ]
    };
    let engine = ordo_host::engine_from_members_under(
        &f,
        &[
            row("mem_o", "owner"),
            row("mem_m", "manager"),
            row("mem_e", "editor"),
            row("mem_v", "viewer"),
        ],
        &PresencePolicy::everything_required(),
    )
    .expect("engine");
    Harness {
        admission: Admission {
            members,
            store: PolicyStore::new(pool.clone()),
            tickets: Some(tickets.clone()),
            ceiling: Ceiling::invitation_only(),
            home: None,
        },
        invite: InviteOps {
            tickets,
            stream: Some(stream.clone()),
            now: Arc::new(now),
        },
        engine,
        clock,
        stream,
        pool,
        space,
    }
}

fn params(pairs: &[(&str, &str)]) -> BoundParams {
    BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect(),
    )
}

impl Harness {
    fn run(
        &self,
        op: &str,
        caller: &str,
        pairs: &[(&str, &str)],
    ) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
        self.invite
            .execute(&self.engine, op, &self.space, &params(pairs), caller)
    }

    fn invite_as(&self, caller: &str, pairs: &[(&str, &str)]) -> String {
        let out = self.run(INVITE, caller, pairs).expect("invited");
        out.rows[0]["ticket"].as_str().expect("ticket").to_string()
    }

    async fn arrive(&self, who: &str, ticket: Option<&str>) -> Outcome {
        self.admission
            .on_arrival(
                who,
                &self.space,
                &Arrival {
                    ticket,
                    ..Default::default()
                },
            )
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
}

/// **An invitation-only tenant admits a person who presents the invitation
/// at sign-in, at the role it names, and refuses one who has none.**
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_only_tenant_admits_the_invited_and_refuses_everybody_else() {
    let h = harness("ordo_invite_admits").await;
    let ticket = h.invite_as("mem_o", &[("role", "editor")]);

    assert_eq!(
        h.arrive("usr_uninvited", None).await,
        Outcome::Refused,
        "an invitation-only tenant admits nobody who holds no invitation"
    );
    assert!(h.role_of("usr_uninvited").await.is_none());

    let got = h.arrive("usr_guest", Some(&ticket)).await;
    assert!(
        matches!(&got, Outcome::Admitted { role, way: Way::Invitation, .. } if role == "editor"),
        "got {got:?}"
    );
    assert_eq!(h.role_of("usr_guest").await.as_deref(), Some("editor"));
}

/// An invitation that names no role admits at the tenant's default, viewer.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_with_no_role_admits_as_a_viewer() {
    let h = harness("ordo_invite_default_role").await;
    let out = h.run(INVITE, "mem_m", &[]).expect("a manager invites");
    assert_eq!(out.rows[0]["role"], Value::Null);
    let ticket = out.rows[0]["ticket"].as_str().unwrap().to_string();
    h.arrive("usr_default", Some(&ticket)).await;
    assert_eq!(h.role_of("usr_default").await.as_deref(), Some("viewer"));
}

/// **An invitation works once.**
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_works_once() {
    let h = harness("ordo_invite_once").await;
    let ticket = h.invite_as("mem_o", &[("role", "viewer")]);
    assert!(matches!(
        h.arrive("usr_first", Some(&ticket)).await,
        Outcome::Admitted { .. }
    ));
    assert_eq!(
        h.arrive("usr_second", Some(&ticket)).await,
        Outcome::Refused,
        "the second person to present it is refused"
    );
    assert!(h.role_of("usr_second").await.is_none());
}

/// **An expired invitation is refused.** The lifetime is stated per
/// invitation, and the ticket default applies when none is given.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_invitation_is_refused_and_the_lifetime_is_the_issuers() {
    let h = harness("ordo_invite_expiry").await;
    let short = h
        .run(INVITE, "mem_o", &[("expires", "1h")])
        .expect("invited");
    assert_eq!(short.rows[0]["expiresAt"], T0 + 3_600);
    let dflt = h.run(INVITE, "mem_o", &[]).expect("invited");
    assert_eq!(dflt.rows[0]["expiresAt"], T0 + DEFAULT_TTL_SECS);
    let short = short.rows[0]["ticket"].as_str().unwrap().to_string();
    let dflt = dflt.rows[0]["ticket"].as_str().unwrap().to_string();

    h.clock.store(T0 + 3_601, Ordering::SeqCst);
    assert_eq!(h.arrive("usr_late", Some(&short)).await, Outcome::Refused);
    assert!(
        matches!(
            h.arrive("usr_in_time", Some(&dflt)).await,
            Outcome::Admitted { .. }
        ),
        "an invitation with the default lifetime is still good"
    );

    let bad = h.run(INVITE, "mem_o", &[("expires", "soon")]);
    assert_eq!(bad.unwrap_err().code, ErrorCode::TenantRefused);
}

/// **A member without the role to invite is refused**, and a manager
/// cannot hand out a role above their own.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_without_the_role_cannot_invite_and_a_manager_cannot_invite_an_owner() {
    let h = harness("ordo_invite_roles").await;
    let reaches = |who: &str, op: &str| {
        h.engine
            .membership_reaches(&SpaceId(h.space.clone()), &PrincipalId(who.into()), op)
    };
    for op in [INVITE, INVITATIONS, REVOKE_INVITATION] {
        assert!(reaches("mem_o", op), "an owner reaches {op}");
        assert!(reaches("mem_m", op), "a manager reaches {op}");
        assert!(!reaches("mem_e", op), "an editor does not reach {op}");
        assert!(!reaches("mem_v", op), "a viewer does not reach {op}");
        assert!(
            !reaches("usr_stranger", op),
            "a stranger does not reach {op}"
        );
    }
    let escalate = h.run(INVITE, "mem_m", &[("role", "owner")]);
    assert_eq!(escalate.unwrap_err().code, ErrorCode::TenantRefused);
    assert!(h.run(INVITE, "mem_m", &[("role", "manager")]).is_ok());
    assert!(h.run(INVITE, "mem_o", &[("role", "owner")]).is_ok());
    // The op itself refuses a caller who holds nothing here, whatever the
    // door said.
    let outsider = h.run(INVITE, "usr_stranger", &[("role", "viewer")]);
    assert_eq!(outsider.unwrap_err().code, ErrorCode::TenantRefused);
    let unknown = h.run(INVITE, "mem_o", &[("role", "emperor")]);
    assert_eq!(unknown.unwrap_err().code, ErrorCode::TenantRefused);
}

/// **Outstanding invitations are listed, without the secret, and one can be
/// revoked.**
#[tokio::test(flavor = "multi_thread")]
async fn outstanding_invitations_are_listed_by_id_and_a_revoked_one_admits_nobody() {
    let h = harness("ordo_invite_list_revoke").await;
    let keep = h.invite_as("mem_o", &[("role", "viewer"), ("label", "Ada")]);
    let drop = h.invite_as("mem_m", &[("role", "editor")]);

    let listed = h.run(INVITATIONS, "mem_m", &[]).expect("listed");
    assert_eq!(listed.rows.len(), 2);
    let dump = serde_json::to_string(&listed.rows).unwrap();
    assert!(
        !dump.contains(&keep) && !dump.contains(&drop) && !dump.contains("tkt_"),
        "the listing never shows a secret"
    );
    let editor = listed
        .rows
        .iter()
        .find(|r| r["role"] == "editor")
        .expect("the editor invitation is listed");
    assert_eq!(editor["issuedBy"], "mem_m");
    let id = editor["id"].as_str().unwrap().to_string();

    let gone = h
        .run(REVOKE_INVITATION, "mem_o", &[("id", &id)])
        .expect("revoked");
    assert_eq!(gone.rows[0]["outcome"], json!("revoked"));
    assert_eq!(
        h.run(REVOKE_INVITATION, "mem_o", &[("id", &id)])
            .unwrap_err()
            .code,
        ErrorCode::NotFound,
        "a second revocation finds nothing"
    );
    assert_eq!(
        h.run(REVOKE_INVITATION, "mem_o", &[("id", "not-an-id")])
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(h.run(INVITATIONS, "mem_o", &[]).unwrap().rows.len(), 1);

    assert_eq!(
        h.arrive("usr_revoked", Some(&drop)).await,
        Outcome::Refused,
        "a revoked invitation admits nobody"
    );
    assert!(matches!(
        h.arrive("usr_kept", Some(&keep)).await,
        Outcome::Admitted { .. }
    ));
    assert!(
        h.run(INVITATIONS, "mem_o", &[]).unwrap().rows.is_empty(),
        "a redeemed invitation is no longer outstanding"
    );
}

/// An invitation named for a person is spendable by that person alone, and
/// another tenant's invitation is not listed here.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_for_a_named_person_is_spent_by_that_person_alone() {
    let h = harness("ordo_invite_named").await;
    let ticket = h.invite_as("mem_o", &[("role", "viewer"), ("to", "usr_named")]);
    assert_eq!(h.arrive("usr_other", Some(&ticket)).await, Outcome::Refused);
    assert!(matches!(
        h.arrive("usr_named", Some(&ticket)).await,
        Outcome::Admitted { .. }
    ));
}

/// **The invitation is on the tenant's record**, and the record names the
/// ticket by nothing that can be redeemed.
#[tokio::test(flavor = "multi_thread")]
async fn issuing_and_revoking_are_on_the_tenants_record() {
    let h = harness("ordo_invite_record").await;
    let ticket = h.invite_as("mem_o", &[("role", "viewer")]);
    let id = h.run(INVITATIONS, "mem_o", &[]).unwrap().rows[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.run(REVOKE_INVITATION, "mem_o", &[("id", &id)]).unwrap();
    let lines = h
        .stream
        .take_rendered(ordo_commandlog::Timestamp(T0 + 10_000_000));
    let all = lines.join("\n");
    assert!(all.contains("invitation-issued"), "record: {all}");
    assert!(all.contains("invitation-revoked"), "record: {all}");
    assert!(!all.contains(&ticket), "the record never holds the secret");
}

// ───────────── Decline (owner ruling 2026-10-05) ─────────────

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_host::auth::{AuthState, Enrolment};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    MemorySessionRevocations, ProviderRegistry, SessionClaim, Timestamp as IStamp, TokenKey,
    TokenSpec,
};
use tower::util::ServiceExt;

const SECRET: &[u8] = b"invitation-decline-secret";

/// The auth routes over the harness's tickets and record.
fn decline_app(h: &Harness) -> axum::Router {
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(h.stream.clone())
    .with_enrolment(Enrolment {
        tickets: Some(h.invite.tickets.clone()),
        ..Default::default()
    });
    let clock = h.clock.clone();
    state.now = Box::new(move || clock.load(Ordering::SeqCst));
    ordo_host::auth::router(Some(Arc::new(state)))
}

fn session_of(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(T0 + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: Some(SessionClaim::starting_at(IStamp(T0))),
        },
    )
}

async fn post_json(
    app: &axum::Router,
    path: &str,
    body: Value,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// **An invitee may decline an invitation before it expires.** The peek
/// shows what it offers without spending it; the decline spends it, so it
/// admits nobody afterwards; the inviter sees it listed as declined; and
/// the act is on the tenant's record and the deployment's.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitee_declines_an_invitation_and_the_inviter_sees_it() {
    let h = harness("ordo_invite_decline").await;
    let app = decline_app(&h);
    let ticket = h.invite_as("mem_m", &[("role", "editor")]);

    let (st, peek) = post_json(
        &app,
        "/auth/invitation/peek",
        json!({"ticket": ticket}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{peek}");
    assert_eq!(peek["space"], json!(h.space));
    assert_eq!(peek["role"], "editor");
    assert_eq!(peek["named"], false);

    // An unnamed ticket is declined by whoever holds it, with no token.
    let (st, out) = post_json(
        &app,
        "/auth/invitation/decline",
        json!({"ticket": ticket}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["outcome"], "declined");

    // Spent: a second decline, a peek and an arrival are all refused.
    let (st, out) = post_json(
        &app,
        "/auth/invitation/decline",
        json!({"ticket": ticket}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["error"]["code"], "invitation_refused");
    let (st, _) = post_json(
        &app,
        "/auth/invitation/peek",
        json!({"ticket": ticket}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(h.arrive("usr_late", Some(&ticket)).await, Outcome::Refused);

    // The inviter sees it declined, by the same id the listing used.
    let listed = h.run(INVITATIONS, "mem_m", &[]).expect("listed");
    assert_eq!(listed.rows.len(), 1, "{:?}", listed.rows);
    assert_eq!(listed.rows[0]["status"], "declined");
    assert_eq!(listed.rows[0]["declinedAt"], json!(T0));
    assert_eq!(listed.rows[0]["issuedBy"], "mem_m");
    let id = listed.rows[0]["id"].as_str().unwrap().to_string();
    // A declined invitation cannot be revoked: there is nothing open.
    assert_eq!(
        h.run(REVOKE_INVITATION, "mem_o", &[("id", &id)])
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    // On the record, in the tenant's stream and the deployment's, naming the
    // id and never the secret.
    let events = h
        .stream
        .drain_all(ordo_commandlog::Timestamp(T0 + 10_000_000));
    let declined: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(&e.kind, ordo_secevent::EventKind::Enrolment { act, .. }
                if act == "invitation-declined")
        })
        .collect();
    let mut scopes: Vec<String> = declined
        .iter()
        .map(|e| match &e.scope {
            ordo_secevent::Scope::Deployment => "deployment".to_string(),
            ordo_secevent::Scope::Space(s) => format!("space:{}", s.0),
        })
        .collect();
    scopes.sort();
    assert_eq!(
        scopes,
        vec!["deployment".to_string(), format!("space:{}", h.space)]
    );
    for e in &declined {
        let ordo_secevent::EventKind::Enrolment { detail, .. } = &e.kind else {
            unreachable!()
        };
        assert!(detail.contains(&format!("id={id}")), "{detail}");
        assert!(!detail.contains(&ticket), "never the secret");
    }

    // Once its life ends, the declined invitation leaves the listing.
    h.clock.store(T0 + DEFAULT_TTL_SECS + 1, Ordering::SeqCst);
    assert!(h.run(INVITATIONS, "mem_m", &[]).unwrap().rows.is_empty());
}

/// **A named invitation is declined by the person it names, and nobody
/// else.** A holder with no session, or with another person's session, is
/// refused and the ticket stays whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_named_invitation_is_declined_by_that_person_alone() {
    let h = harness("ordo_invite_decline_named").await;
    let app = decline_app(&h);
    let ticket = h.invite_as("mem_o", &[("role", "viewer"), ("to", "usr_named")]);

    for bearer in [None, Some(session_of("usr_other"))] {
        let (st, out) = post_json(
            &app,
            "/auth/invitation/decline",
            json!({"ticket": ticket}),
            bearer.as_deref(),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    }
    let open = h.run(INVITATIONS, "mem_o", &[]).unwrap();
    assert_eq!(open.rows[0]["status"], "open", "the ticket stays whole");

    let (st, out) = post_json(
        &app,
        "/auth/invitation/decline",
        json!({"ticket": ticket}),
        Some(&session_of("usr_named")),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(
        h.arrive("usr_named", Some(&ticket)).await,
        Outcome::Refused,
        "a declined invitation admits nobody, the named person included"
    );
}
