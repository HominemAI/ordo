//! The deployment's own security record, read through the door:
//! `std.deployment.securityEvents` (G8, `plan-console.md` 4.8 and 7).
//!
//! # What it reads
//!
//! The deployment's stream (`Scope::Deployment`, `$deployment`) in the
//! control database, the one `ordo verify --security --deployment` checks.
//! It holds what has no tenant home: unverified callers, cross-space
//! sweeps, sign-ins, console key acts, deploys, quotas, tenant pauses and
//! creations, bucket custody (`security-logging-design.md` §6.1). A
//! tenant's own stream is not read here: the deployment owner reads one of
//! those through `GET /security/events?space=` where they hold a role, as
//! any tenant owner does (ruled 2026-10-23, ask 5).
//!
//! # Who may read it
//!
//! The deployment owner, in the deployment's own space: the op is
//! owner-only (`platform_ops::owner_only`) and answers `not_found` in any
//! other space. A read, so no touch.
//!
//! # What a row carries
//!
//! The entry as recorded: its full-stream index, kind, act and detail,
//! who, which op, the tenant it is about, when and how many times. Every
//! producer writes no secret into an entry (a ticket is named by a hash
//! prefix, a password not at all, a key by its id), and this reads nothing
//! else: no table beside the stream, no setting.
//!
//! # Paging and filters
//!
//! Newest first by default, `before` an index for the next page; `from` an
//! index reads oldest first instead. `kind`, `tenant`, `since` and `until`
//! narrow it. At most [`PAGE_MAX`] rows a page.

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::secevent::{EntryFilter, Page, PgSecurityStore};
use ordo_exec::Pool;
use ordo_secevent::{EventKind, Scope, StreamEntry};
use serde_json::{json, Value};

/// `std.deployment.securityEvents($kind?, $tenant?, $since?, $until?,
/// $before?, $from?, $limit?)`.
pub const EVENTS: &str = "std.deployment.securityEvents";
/// Every op this module serves.
pub const OPS: [&str; 1] = [EVENTS];

/// The most rows one page answers.
pub const PAGE_MAX: usize = 200;
/// Rows a page answers when `limit` is not given.
pub const PAGE_DEFAULT: usize = 50;

/// Every kind a `kind` filter may name, as the stream spells them.
pub const KINDS: [&str; 25] = [
    "denied",
    "authenticated",
    "authentication-failed",
    "step-up-demanded",
    "step-up-satisfied",
    "key-released",
    "membership-suspended",
    "membership-reactivated",
    "custody-action",
    "authenticator-linked",
    "authenticator-unlinked",
    "principal-onboarded",
    "suppressed",
    "events-dropped",
    "schema-migration",
    "enrolment",
    "sessions-ended",
    "recovery",
    "second-factor",
    "session-handoff",
    "upstream-claim",
    "sealed-unreadable",
    "console-key",
    "deployment-booted",
    "deploy",
];

fn param(name: &str, ty: ParamTy) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty,
        required: false,
        bounds: None,
    }
}

/// The op's spec: a read with optional filters.
pub fn specs() -> Vec<OpSpec> {
    let text = || ParamTy::Text { max_len: Some(128) };
    vec![OpSpec {
        name: EVENTS.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: vec![
            param("kind", text()),
            param("tenant", text()),
            param("since", ParamTy::Int),
            param("until", ParamTy::Int),
            param("before", ParamTy::Int),
            param("from", ParamTy::Int),
            param("limit", ParamTy::Int),
        ],
        output_fields: [
            "index",
            "kind",
            "act",
            "detail",
            "decisions",
            "subject",
            "subjectKind",
            "session",
            "op",
            "tenant",
            "firstAt",
            "lastAt",
            "count",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// What the op needs.
pub struct SecurityRecordOps {
    /// The control database, which holds the deployment's stream.
    pub control: Pool,
    /// The deployment's own space, the only one the op answers in.
    pub home: String,
}

fn refused(msg: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
}

/// A whole number parameter, as a number or the text of one.
fn whole(bound: &BoundParams, name: &str) -> Result<Option<u64>, ExecError> {
    match bound.0.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| refused(format!("`{name}` is a whole number, 0 or more"))),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| refused(format!("`{name}` is a whole number, 0 or more"))),
        Some(_) => Err(refused(format!("`{name}` is a whole number, 0 or more"))),
    }
}

fn word<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// What one entry did, as its act word and its one line of detail. The
/// detail is what the producer recorded, which holds no secret.
pub fn describe(kind: &EventKind) -> (Option<String>, String) {
    match kind {
        EventKind::Denied(set) => (
            None,
            set.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(","),
        ),
        EventKind::AuthenticationFailed { reason } => (None, format!("reason={reason}")),
        EventKind::KeyReleased { purpose } => (None, format!("purpose={purpose}")),
        EventKind::CustodyAction { action } => (None, action.clone()),
        EventKind::AuthenticatorLinked { added, vouched_by } => (
            None,
            format!(
                "added={added} vouched-by={}",
                vouched_by.as_deref().unwrap_or("bootstrap")
            ),
        ),
        EventKind::AuthenticatorUnlinked { removed } => (None, format!("removed={removed}")),
        EventKind::Suppressed { further } => (None, format!("further={further}")),
        EventKind::EventsDropped { lost } => (None, format!("lost={lost}")),
        EventKind::SchemaMigration { migration, phase } => {
            (None, format!("migration={migration} phase={phase}"))
        }
        EventKind::SessionsEnded { reason } => (None, format!("reason={reason}")),
        EventKind::SealedUnreadable {
            entity,
            field,
            epoch,
        } => (None, format!("field={entity}.{field} epoch={epoch}")),
        EventKind::DeploymentBooted { detail } => (None, detail.clone()),
        EventKind::Enrolment { act, detail }
        | EventKind::Recovery { act, detail }
        | EventKind::SecondFactor { act, detail }
        | EventKind::SessionHandoff { act, detail }
        | EventKind::UpstreamClaim { act, detail }
        | EventKind::ConsoleKey { act, detail }
        | EventKind::Deploy { act, detail } => (Some(act.clone()), detail.clone()),
        _ => (None, String::new()),
    }
}

/// One entry as a row.
pub fn row(index: usize, e: &StreamEntry) -> Value {
    let (act, detail) = describe(&e.kind);
    let decisions = match &e.kind {
        EventKind::Denied(set) => json!(set.iter().map(|d| d.as_str()).collect::<Vec<_>>()),
        _ => Value::Null,
    };
    json!({
        "index": index,
        "kind": e.kind.label(),
        "act": act,
        "detail": detail,
        "decisions": decisions,
        "subject": e.subject.render(),
        "subjectKind": e.subject.kind_str(),
        // A deployment token's `kind`, `client` and `sid`, beside `sub`:
        // which session made the call. Never the token itself.
        "session": e.subject.token().map(|t| json!({
            "kind": t.kind,
            "client": t.client,
            "sid": t.sid,
        })),
        "op": e.op,
        "tenant": e.about.as_ref().map(|s| s.0.clone()),
        "firstAt": e.first_at.0,
        "lastAt": e.last_at.0,
        "count": e.count,
    })
}

impl SecurityRecordOps {
    /// `std.deployment.securityEvents`.
    pub async fn execute(&self, space: &str, bound: &BoundParams) -> Result<ExecResult, ExecError> {
        if space != self.home {
            // `not_found`, as every deployment op answers outside its home.
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the deployment's security record is the deployment's: this op answers only in \
                 the deployment's own space. A tenant's own record is GET \
                 /security/events?space=<tenant>",
            ));
        }
        let kind = word(bound, "kind").map(str::to_string);
        if let Some(k) = &kind {
            if !KINDS.contains(&k.as_str()) {
                return Err(refused(format!("`kind` is one of: {}", KINDS.join(", "))));
            }
        }
        let tenant = word(bound, "tenant").map(str::to_string);
        if let Some(t) = &tenant {
            if !ordo_api::well_formed_space(t) {
                return Err(refused(
                    "`tenant` is a tenant id, as std.deployment.tenants lists it",
                ));
            }
        }
        let filter = EntryFilter {
            kind,
            about: tenant,
            since: whole(bound, "since")?,
            until: whole(bound, "until")?,
        };
        let limit = match whole(bound, "limit")? {
            None => PAGE_DEFAULT,
            Some(0) => return Err(refused("`limit` is 1 or more")),
            Some(n) => usize::try_from(n).unwrap_or(PAGE_MAX).min(PAGE_MAX),
        };
        let page = match (whole(bound, "from")?, whole(bound, "before")?) {
            (Some(_), Some(_)) => {
                return Err(refused(
                    "give `from` (oldest first) or `before` (newest first), not both",
                ))
            }
            (Some(f), None) => Page::Forward {
                from: usize::try_from(f).unwrap_or(usize::MAX),
            },
            (None, b) => Page::Back {
                before: b.map(|b| usize::try_from(b).unwrap_or(usize::MAX)),
            },
        };
        let store = PgSecurityStore::new(self.control.clone(), tokio::runtime::Handle::current());
        let entries = store
            .try_entries_where(&Scope::Deployment, &filter, page, limit)
            .await
            .map_err(|e| {
                ExecError::of_fault(
                    e.class(),
                    ErrorCode::SpaceUnavailable,
                    format!(
                        "the deployment's security record could not be read ({e}). Nothing was \
                         read, which is not the same as nothing having happened"
                    ),
                )
            })?;
        Ok(ExecResult {
            position: None,
            rows: entries.iter().map(|(i, c)| row(*i, &c.entry)).collect(),
            next_cursor: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_commandlog::Timestamp;
    use ordo_secevent::{correlation, RecordedDecision, Subject};

    #[test]
    fn every_kind_the_stream_spells_is_a_filter_word() {
        let kinds = [
            EventKind::denied(RecordedDecision::NoMembership),
            EventKind::Authenticated,
            EventKind::Deploy {
                act: "applied".into(),
                detail: String::new(),
            },
            EventKind::ConsoleKey {
                act: "added".into(),
                detail: String::new(),
            },
        ];
        for k in kinds {
            assert!(KINDS.contains(&k.label()), "{}", k.label());
        }
    }

    #[test]
    fn a_row_carries_the_act_and_detail_and_the_tenant_it_is_about() {
        let mut e = StreamEntry::single(
            Scope::Deployment,
            Subject::member("mem_o"),
            "enrol.quota-set",
            EventKind::Enrolment {
                act: "quota-set".into(),
                detail: "space=spc_a limit=5".into(),
            },
            Timestamp(7),
            correlation::mint(),
        );
        e.about = Some(ordo_commandlog::SpaceId("spc_a".into()));
        let r = row(3, &e);
        assert_eq!(r["index"], 3);
        assert_eq!(r["kind"], "enrolment");
        assert_eq!(r["act"], "quota-set");
        assert_eq!(r["detail"], "space=spc_a limit=5");
        assert_eq!(r["tenant"], "spc_a");
        assert_eq!(r["decisions"], Value::Null);
    }
}
