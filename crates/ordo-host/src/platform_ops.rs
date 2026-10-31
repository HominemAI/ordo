//! 8r — the platform-op path, and `std.space.callersOf` on it.
//!
//! # The decision this module is, stated
//!
//! Every [`ordo_api::OpSpec`] this deployment serves is derived from the
//! **compiled registry**: `ordo_schema` walks a program's declarations,
//! `ordo_plan` bounds them, and `crate::build_specs` projects the result.
//! `std.space.*` cannot come from there. Its ops read the access engine's
//! **runtime state** — memberships, the space's profile, the grant registry
//! — which no program declares, no planner can bound, and no schema
//! describes. `ordo_schema::derive_std` says so in terms and declines to
//! invent a mechanism, which is why 8r stayed open after its blocker was
//! removed.
//!
//! The decision here is: **a platform op is a second *source* of specs, not
//! a second kind of op.** One `OpSpec`, hand-written because there is
//! nothing to compile it from, registered into the same registry the door
//! dispatches on, and executed through the same [`ordo_api::OpExecutor`]
//! trait. Everything downstream is untouched and gets these ops for free —
//! the access check, the `$space` bind, the parameter binder, the version
//! headers, the uniform refusal envelope, the scoped catalog, idempotency,
//! the metered executor. Nothing here re-implements any of it, and nothing
//! here may.
//!
//! Two mechanical choices follow from that and are worth naming, because
//! both were the alternative that looked cheaper:
//!
//! - **Not a second router.** `crate::serve` mounts a second router for the
//!   deployment's own surfaces (`/health/deep`, `/metrics`, the catalog),
//!   and `callersOf` could have been a route there. It must not be: those
//!   are surfaces *about* the deployment, reached by reserved names outside
//!   the op namespace. `std.space.callersOf` is an **op** — it is named in
//!   the std library, it takes a parameter, it returns rows, it belongs in
//!   a caller's catalog, and a capability grant must be able to name it in
//!   order to be refused. A route would have none of that and would need
//!   its own copy of every check.
//! - **Not a second registry.** [`specs`] is merged into the one
//!   [`crate::deploy::Deployment`] holds, so the catalog publishes these
//!   ops, `/health/ready`'s canary sees them, and a hot swap cannot leave
//!   the door and the executor disagreeing about them. They live on the
//!   `Deployment` rather than inside a `Loaded`, which says the true thing:
//!   **a platform op is not part of a program version and does not change
//!   when one is swapped.**
//!
//! # The door this opens for the rest of `std.space.*`
//!
//! `std-library.md` §3 lists more of them — the membership and profile
//! administration commands. Adding one is: a spec in [`specs`], an arm in
//! [`PlatformOps::execute`], and a grant in `crate::deployment_engine`.
//! The collision refusal, the manifest, the catalog entry and the wiring
//! are already here. Write ops will additionally need a command group and a
//! chain append, which is the part this module deliberately does not
//! pretend to have solved: `callersOf` is a read, and a read is the honest
//! first member.
//!
//! # `callersOf`'s refusal rule, and the argument for it
//!
//! It returns **principals**. That makes it an enumeration surface, and two
//! enumeration oracles have already been found in this codebase, so the
//! rule is argued rather than assumed.
//!
//! What one call discloses, for one op: every member who can call it, every
//! group and override that gets them there, and **every capability grant
//! over it — whose it is, and which client it was consented to**. That last
//! part is not the space's data about itself. It is a list of the
//! third-party apps individual people have connected, which
//! `GrantRegistry::grants_of` describes as *"what a person is shown when
//! they ask what they have consented to"*. Showing Alice her own is a
//! feature; showing Bob hers is a different act.
//!
//! **The floor is manager, and it is enforced by the profile** —
//! `crate::deployment_engine` lists the op for [`Role::Manager`] and
//! [`Role::Owner`] and for nobody else.
//!
//! - *Not editor.* An editor can change data and cannot change who may call
//!   anything. The answer informs no act available to them, so what they
//!   would gain is a complete map of the space's privilege structure — the
//!   reconnaissance step, with the escalation left as an exercise. The same
//!   argument `deployment_engine` already makes for `/health/deep`: *"a
//!   manager who may edit data has no business reading either"*, one role
//!   down.
//! - *Not owner-only.* The manager is the role that **administers the
//!   answer** — §6's *"managers tighten and the engine obeys"*. An audit
//!   surface the administrator cannot read is one that pushes deployments
//!   to hand out owner, and a control that is too tight is a control people
//!   route around. Rule 4's stated value — *"a group grant that does not
//!   show up there silently breaks the model's best property"* — is a value
//!   to whoever maintains the grants, which is the manager.
//!
//! **And it is step-up gated**, which is where the token question is
//! answered. `security-logging-design.md` §5 pins that *"capability tokens
//! can never satisfy step-up, so step-up ops are structurally human-only"*,
//! and `AccessEngine::require_step_up`'s own doc names **full export** as
//! one of the things registered there. A dump of a space's entire access
//! control state, including everyone's connected apps, is export-class by
//! any reading. So:
//!
//! - a capability token is refused **whatever its allowlist says** — Alice
//!   may delegate Alice's authority, and Bob's consent record is not hers
//!   to delegate;
//! - the refusal is **not delegable away**: no client can lower an op's
//!   intrinsic step-up minimum (Z-003), and no profile edit can either,
//!   because step-up is checked after inclusion and separately from it.
//!
//! Reusing step-up rather than inventing a "human-only" flag matters: the
//! platform has exactly one mechanism for *not delegable*, and a second one
//! would be a second thing to get right.
//!
//! # The fifth via, and what it adds to the disclosure
//!
//! The answer now also carries **delegations**: which principals are acting
//! on somebody else's behalf, and whose. That strengthens the manager floor
//! rather than weakening it — it is the most sensitive column, because a
//! delegation is a person's own consent to be acted for and reads as *"Ada
//! let this thing be her"*. It is also the column an audit most needs: a
//! delegated agent is invisible in the role, group and override columns by
//! construction, and would have been indexed under `token` if the four
//! words had been held to, which would send a manager to revoke a grant and
//! find the caller still there.
//!
//! It is **conjunctive** with the token column —
//! [`AccessEngine::callers_of`] emits it only for an agent that also holds a
//! live grant listing the op — so the surface never names a caller the door
//! would refuse.
//!
//! # The number of new refusal classes is zero, on purpose
//!
//! Every refusal above is the door's, and the door has one answer:
//! `access_denied`, identical for an op you may not call and an op that does
//! not exist (`api-engine-design.md` §4.1). A new class minted for *"you are
//! not a manager"* or *"tokens may not read this"* would itself be the
//! oracle — it would tell a prober which wall they hit. So there is no new
//! class, and that is the design rather than an omission.
//!
//! The one refusal this module raises itself is about the **argument**:
//! `callersOf($op)` where `$op` is not in the asker's own scoped catalog
//! answers [`ErrorCode::NotFound`], whose documented meaning is already
//! exactly this — *"does not exist — or exists outside the caller's read
//! view, which answers the same way, because you learn a row exists only if
//! you may read it"*. A name nobody registered and a name the asker may not
//! call are one answer, so the parameter enumerates nothing. It is
//! deliberately **not** an empty result: an empty answer for an op that
//! does have callers is the lie-by-omission that kept 8r closed.

use std::sync::Arc;

use futures::future::BoxFuture;
use ordo_api::{
    BoundParams, ExecError, ExecOutcome, ExecResult, FieldSpec, NotSyncable, OpCost, OpExecutor,
    OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, ReplayLease, Syncability,
};
use ordo_identity::delegation::DelegationRegistry;
use ordo_identity::grants::GrantRegistry;
use ordo_identity::principal::{PrincipalId, SpaceId, Timestamp};
use ordo_identity::AccessEngine;
use serde_json::json;

/// `std.space.callersOf($op: text)` — every principal currently able to call
/// `$op` in the bound space, with the *via* that gets them there.
pub const CALLERS_OF: &str = "std.space.callersOf";

/// `std.person.tenants`: the tenants the caller is a member of, with the
/// role held in each (owner ruling, 2026-09-29: *a person may see the list
/// of their own tenants*).
///
/// **A platform op scoped to the caller, not an auth route.** The door has
/// one pipeline (access check, catalog, metering, the single refusal) and a
/// route beside it would need its own copy of each, in `auth.rs` or
/// `serve.rs`. The op takes no parameter, so nothing can name another
/// person: the principal is the one the door proved. It runs in any tenant
/// the caller belongs to and the answer is about none of them in particular;
/// the tenant it ran in learns only that the person called an op, as it
/// learns of any call, and never the other tenants named in the answer.
/// Every role may call it and it asks for no presence: it is the person's
/// own information, and a password session may read it.
pub const MY_TENANTS: &str = "std.person.tenants";

/// Ops that answer about the caller and act on no space. The engine gives
/// them to every role and does not put them behind step-up.
pub fn person_scoped(name: &str) -> bool {
    name == MY_TENANTS
}

/// The version hash platform ops carry.
///
/// Not a content hash of anything, because there is no artifact to hash: a
/// platform op's behaviour is this binary's, not a deployed program's. The
/// word is deliberately not a version *number* — a caller comparing two
/// deployments' `callersOf` is comparing binaries, and a fake semver would
/// invite them to think otherwise.
pub const PLATFORM_VERSION: &str = "platform";

/// The platform ops this build serves.
///
/// A function rather than a constant so the specs are owned values the
/// registry can take, and so adding the rest of `std.space.*` is one entry
/// here rather than a change of shape.
pub fn specs() -> Vec<OpSpec> {
    vec![
        OpSpec {
            name: CALLERS_OF.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Read,
            params: vec![ParamSpec {
                name: "op".to_string(),
                ty: ParamTy::Text { max_len: Some(256) },
                required: true,
                bounds: None,
            }],
            output_fields: vec![
                FieldSpec::new("principal"),
                FieldSpec::new("via"),
                FieldSpec::new("detail"),
            ],
            // A live subscription would have to invalidate on membership,
            // profile and grant writes — three sources, one of which
            // (`$capabilitygrant`) is not a domain table and publishes no
            // staleness signal. DL §5's "named ⇒ subscribable" is about
            // *compiled* reads over entities; this is neither, and claiming
            // live-capability the invalidation path cannot honour would make a
            // subscriber believe a stale answer was current.
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
            requires_idempotency_key: false,
            public: Default::default(),
            // No refinement surface. `$refine` is validated against
            // `output_fields`, and a condition over `principal` would be a
            // filtered enumeration — the shape every oracle in the security
            // review takes.
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            // The honest registration for an op no planner classified. It is
            // read from the access engine's in-memory state plus one indexed
            // registry lookup keyed on `(space, op)`, which is bounded by the
            // space's membership — not by anything a caller supplies. `Sql` is
            // the class; `refine_candidate_bounded: false` is what refuses a
            // refinement outright rather than serving one on a fact nobody
            // established.
            cost: OpCost::default(),
        },
        // ── the exit ops (`definition-language.md` §8) ──
        //
        // `Write` rather than `Read` for the export, and the choice is not
        // cosmetic: `OpKind` fixes the HTTP verb, and an export produces a
        // durable artifact on the deployment's disk and a disclosure record in
        // the owner's security stream. A `GET` that does both would be a `GET`
        // with side effects, which every cache and every prefetcher in the
        // world is entitled to repeat.
        OpSpec {
            name: crate::exit_ops::EXPORT.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![ParamSpec {
                // A **bool**, not a `mode: text` with two legal spellings,
                // and the difference is which layer refuses a typo. Text
                // would put an enum check inside the handler — and an
                // executor may not say `invalid_param`, which is the
                // door's word, so a typo would have reached the caller as
                // `internal`. A bool has no invalid value: `bind_params`
                // type-checks it at the door, in the door's own
                // vocabulary, before the op is reached.
                //
                // Absent (or false) is *opened*, which is `migration.md`
                // §6's disclosure and what an owner leaving the platform
                // wants. The one thing this must never do is fall back to
                // total disclosure on a value it did not understand.
                name: "ciphertext".to_string(),
                ty: ParamTy::Bool,
                required: false,
                bounds: None,
            }],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("version"),
                FieldSpec::new("sealed"),
                FieldSpec::new("bundle"),
                FieldSpec::new("members"),
                FieldSpec::new("entities"),
                FieldSpec::new("rows"),
                FieldSpec::new("logGroups"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // `std.space.import` (11-10a), and it is a `Write` for a stronger
        // reason than the export's: it writes every row in the bundle
        // through the write protocol. The verb is the honest one.
        OpSpec {
            name: crate::exit_ops::IMPORT.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![
                ParamSpec {
                    // A **staged bundle's name**, never a path. The door
                    // type-checks that it is text and the handler refuses
                    // anything that is not a plain name, so `..` never
                    // reaches the filesystem: see `ExitOps::staged` for why
                    // an illegal name and an absent bundle are one refusal.
                    name: "bundle".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // The import's name, and it is **required** because it
                    // is the resume key. Two runs under one name resume
                    // each other and two names are two imports, so a
                    // default would silently decide which of those an
                    // operator meant on the one call where it matters: the
                    // retry after a crash.
                    name: "name".to_string(),
                    ty: ParamTy::Text { max_len: Some(64) },
                    required: true,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("name"),
                FieldSpec::new("bundle"),
                FieldSpec::new("entities"),
                FieldSpec::new("rows"),
                FieldSpec::new("groups"),
                FieldSpec::new("batchesWritten"),
                FieldSpec::new("batchesSkipped"),
                FieldSpec::new("objectsAttached"),
                FieldSpec::new("objectsSkipped"),
                FieldSpec::new("peopleMapped"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            // **Not required, and the import's own name is why.** Item 44
            // put an `Idempotency-Key` on `std.space.create` because a
            // retried create with no key mints a second tenant. A retried
            // import under the same name resumes the first one: the batch
            // leases in `$replay` are keyed by the import name and
            // fingerprinted by their source ids. The parameter is the
            // idempotency key.
            //
            // **It is weaker than this comment used to claim.** It said a
            // changed bundle under a used name "refuses rather than
            // skipping". The fingerprint covers ids and not values, so a
            // bundle with the same ids and different values resumes and
            // reports success over a mixture of old and new rows
            // (`security-review-6.md` S6-006). Whether a header would have
            // been stronger is therefore open again, and is not decided
            // here.
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        OpSpec {
            name: crate::exit_ops::DELETE.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Delete,
            params: vec![ParamSpec {
                // The confirmation is the space's own id typed back. A
                // `required` boolean would be supplied for free by any retry,
                // and a token the platform issued would make the platform the
                // party that decided.
                name: "confirm".to_string(),
                ty: ParamTy::Text { max_len: Some(128) },
                required: true,
                bounds: None,
            }],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("rowsDeleted"),
                FieldSpec::new("deleted"),
                FieldSpec::new("retained"),
                FieldSpec::new("logGroups"),
                FieldSpec::new("insideUndoHorizon"),
                FieldSpec::new("sealedPayloads"),
                FieldSpec::new("strandedMirrors"),
                FieldSpec::new("objectsDeleted"),
                FieldSpec::new("objectsLeft"),
                // E7: what deleting the tenant did to the identities it made.
                FieldSpec::new("identities"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // -- attestation (`security-logging-design.md` section 7, layers 2 and 4) --
        //
        // `Write`, on the export's argument: it mints a per-sink secret,
        // stores a destination, and writes a `CustodyAction` into the owner's
        // security stream. A `GET` that does those things is a `GET` with
        // side effects, which every cache and every prefetcher in the world
        // is entitled to repeat -- and repeating this one rotates the secret
        // out from under a receiver who is already using it.
        OpSpec {
            name: crate::attest_ops::ATTEST_TO.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![
                ParamSpec {
                    // 2048 rather than 256: a webhook URL with a signed path
                    // segment is routinely longer than a hostname, and the
                    // one thing this must not do is refuse a legitimate
                    // destination for a length nobody chose on purpose. The
                    // door measures it, so an over-long value is
                    // `bound_violation` in the door's own vocabulary.
                    //
                    // **Optional** since the owner ruling of 2026-10-05: no
                    // destination removes the one that is set. The three
                    // below are optional with it, and the op refuses a
                    // destination that comes without them.
                    name: "destination".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(2048),
                    },
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    name: "ownerKeyId".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    // 64 hex characters of Ed25519 public key. Typed as text
                    // with a length the door checks; the *content* check is
                    // the signature verification, which is the only check
                    // that means anything.
                    name: "ownerKey".to_string(),
                    ty: ParamTy::Text { max_len: Some(64) },
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    // 128 hex characters of Ed25519 signature over
                    // `ordo_exec::webhook::consent_bytes`.
                    name: "signature".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("destination"),
                FieldSpec::new("destinationKeyId"),
                // Shown once at grant, section 7b's word.
                // `std.space.attestation` reads the destination back and
                // never this.
                FieldSpec::new("secret"),
                FieldSpec::new("watchingFrom"),
                FieldSpec::new("reachable"),
                FieldSpec::new("schema"),
                // `set`, or `removed` / `unchanged` for no destination.
                FieldSpec::new("outcome"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // `std.space.attestation` and `std.space.attestationDeadLetters`
        // (owner ruling 2026-10-05): read-backs, owner-only like the
        // `attestTo` they read. `Read`: neither writes or records anything.
        OpSpec {
            name: crate::attest_ops::ATTESTATION.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Read,
            params: Vec::new(),
            output_fields: [
                "space",
                "destination",
                "destinationKeyId",
                "setAt",
                "watchingFrom",
                "deadLetters",
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
        },
        OpSpec {
            name: crate::attest_ops::ATTESTATION_DEAD_LETTERS.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Read,
            params: Vec::new(),
            output_fields: ["upto", "destination", "attempts", "at", "kind"]
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
        },
        // `Read`, and the export's argument runs the other way here. A chain
        // proof writes nothing, mints nothing, and records nothing -- it
        // discloses no space content, only digests and signatures, so there
        // is no disclosure whose record is owed. It is a pure function of
        // (state, from, to), which is exactly the property a cache and a
        // prefetcher are entitled to assume, so a `GET` is honest.
        OpSpec {
            name: crate::attest_ops::CHAIN_PROOF.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Read,
            params: vec![
                ParamSpec {
                    name: "from".to_string(),
                    ty: ParamTy::Int,
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    name: "to".to_string(),
                    ty: ParamTy::Int,
                    required: true,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("from"),
                FieldSpec::new("to"),
                FieldSpec::new("links"),
                FieldSpec::new("anchors"),
                FieldSpec::new("head"),
                // The artifact, as a **string**. Structured JSON here would
                // be re-serialised by whatever renders the response, and a
                // verifier whose input depends on a client library's key
                // ordering is a verifier that fails for reasons nobody can
                // reproduce. The bytes built are the bytes checked.
                FieldSpec::new("proof"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // ── BYO bucket (`definition-language.md` §2, pin 1) ──
        //
        // `Write`, not `Read`, for the exit ops' reason one step further:
        // `OpKind` fixes the HTTP verb, and this one changes storage
        // custody and writes a record to the owner's security stream. A
        // `GET` that did either would be a `GET` with side effects.
        //
        // Every parameter is a **name**, never a secret. `$credential` is
        // a label the operator installed out of band; see
        // `ordo_exec::bucket`'s module doc for why an access key here
        // would be an access key in the plaintext command log.
        OpSpec {
            name: crate::bucket_ops::ATTACH_BUCKET.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![
                ParamSpec {
                    name: "endpoint".to_string(),
                    ty: ParamTy::Uri,
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    name: "region".to_string(),
                    ty: ParamTy::Text { max_len: Some(64) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    name: "bucket".to_string(),
                    ty: ParamTy::Text { max_len: Some(63) },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    name: "credential".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: true,
                    bounds: None,
                },
                // **A bool, and required.** `exit_ops`' `ciphertext`
                // makes the same choice for the same stated reason: text
                // with two legal spellings puts an enum check inside the
                // handler, and an executor may not say `invalid_param` —
                // so a typo would reach the caller as `internal`. A bool
                // has no invalid value, and `bind_params` refuses a
                // non-boolean at the door in the door's own vocabulary.
                //
                // Required with no default, because the two positions
                // answer differently to "what does deleting this space do
                // to these objects" — defaulting would be the platform
                // deciding a disclosure on the owner's behalf. `true` is
                // platform-sealed (their bucket holds ciphertext they
                // cannot read alone); `false` is their own perimeter.
                ParamSpec {
                    name: "platformSealed".to_string(),
                    ty: ParamTy::Bool,
                    required: true,
                    bounds: None,
                },
                // U1 (`docs/plan-tenant-storage.md` §2 B-C2), both optional
                // and both `NULL`/absent meaning today's behaviour, so an
                // attachment that never mentions either is indistinguishable
                // from one made before these parameters existed.
                //
                // `pathStyle`: `S3Config::new`'s derivation is right for the
                // deployment's own store and wrong for a self-hosted gateway
                // behind TLS on a real hostname, which needs path style and
                // gets virtual-host style instead — a DNS lookup for
                // `<bucket>.<host>` that does not resolve. This is the
                // tenant's only way to say which one their gateway needs.
                ParamSpec {
                    name: "pathStyle".to_string(),
                    ty: ParamTy::Bool,
                    required: false,
                    bounds: None,
                },
                // `keyPrefix`: a prefix ahead of the space segment in every
                // key this attachment's objects live under, for a tenant
                // whose own bucket holds other things besides this
                // platform's objects. Charset-checked in the handler on
                // `ordo_blob::space_prefix`'s own rule — refused rather than
                // sanitised — because no `ParamTy` expresses "usable as a
                // bucket key segment".
                ParamSpec {
                    name: "keyPrefix".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("bucket"),
                FieldSpec::new("endpoint"),
                FieldSpec::new("region"),
                // **Never `credential`.** It is a label the operator
                // installed, not a secret, but it is deployment-side naming
                // material a space owner has no use for — returnable to a
                // deployment administrator, never to a tenant. See
                // `docs/plan-tenant-storage.md` §10 B-X4.2.
                FieldSpec::new("custody"),
                FieldSpec::new("pathStyle"),
                FieldSpec::new("keyPrefix"),
                FieldSpec::new("outcome"),
                FieldSpec::new("disclosure"),
                FieldSpec::new("existingObjects"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // `std.space.detachBucket` (owner ruling 2026-10-05): the attach run
        // back. `Write` for the attach's reason, and no parameter: the
        // space it is bound to is the only thing it acts on.
        OpSpec {
            name: crate::bucket_ops::DETACH_BUCKET.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: Vec::new(),
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("bucket"),
                FieldSpec::new("endpoint"),
                FieldSpec::new("custody"),
                FieldSpec::new("outcome"),
                FieldSpec::new("disclosure"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // ── the deploy door (18l) ──
        //
        // `std.deployment.*`, and the namespace is the gate restated in
        // the name: every other op here acts on the space it is bound to,
        // and this one acts on the deployment that holds every space. See
        // `crate::apply_op`'s module doc for the three checks that stack
        // behind it, and in particular for the one neither the profile nor
        // step-up can express — the bound space must be the platform's
        // own, because the profile is deployment-wide policy installed in
        // every space and a tenant owner passes it.
        //
        // `Write`, and not arguably: it runs DDL, stores an artifact,
        // re-points the deployment and writes a `$migrations` row and a
        // security event. A `GET` that did any of those is a `GET` with
        // side effects.
        OpSpec {
            name: crate::apply_op::APPLY.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![
                ParamSpec {
                    // The program **source**, not a compiled artifact.
                    // The service compiles it, through the same pipeline
                    // the load gate uses, which is what keeps "what was
                    // planned against this database" and "what a node
                    // would load" one thing. Accepting a client-compiled
                    // artifact would put the compiler on the caller's side
                    // of the trust boundary and make the deployment's
                    // schema a function of whichever `ordo` binary was
                    // nearest.
                    //
                    // A megabyte, measured at the door in the door's own
                    // vocabulary (`bound_violation`). Every `.ordo` file
                    // in this repository's corpus fits inside a few tens
                    // of kilobytes; a program past this is not a program.
                    name: "program".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(1024 * 1024),
                    },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // `--no-activate`, inverted. A bool for `ciphertext`'s
                    // stated reason: text with two legal spellings puts an
                    // enum check inside the handler, and an executor may
                    // not say `invalid_param`.
                    //
                    // Optional, defaulting to **activate**, because that is
                    // what `ordo apply` does without a flag. A door that
                    // defaulted the other way would make a deploy over the
                    // wire a different act from a deploy on the box, which
                    // is the one difference 17u exists to remove.
                    name: "activate".to_string(),
                    ty: ParamTy::Bool,
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    // Backend functions' code (`plan-backend-functions.md`
                    // §4.3): a JSON object from each `code "..."` path to
                    // `{"hash", "text"}`. The door hashes each text itself,
                    // refuses one that does not match the hash the caller
                    // read, and stores it by hash before the version that
                    // names it ([`crate::apply_op::CodeFiles`]). Text and
                    // not `json`, because `ORDO_JSON_MAX_BYTES` is the app's
                    // field limit and a bundle is not app data. Required
                    // only for a program that declares functions.
                    name: "code".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(crate::apply_op::MAX_DOOR_CODE as u32),
                    },
                    required: false,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("version"),
                FieldSpec::new("notes"),
                FieldSpec::new("schema"),
                FieldSpec::new("stored"),
                FieldSpec::new("activation"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        // `std.space.create` (17aa piece 3). Named for the space it
        // creates and not for the deployment it is bound to, which is the
        // opposite convention from `std.deployment.apply` above and is
        // `bootstrap.md` §1's name, kept because that document has
        // specified this surface since before the code existed.
        //
        // `Write`, plainly: it mints custody, writes a founding member
        // row, writes a registry row and wakes every replica's door.
        // `std.space.reap` — release what a failed provision left behind
        // (`bootstrap.md` §4.5). **Deliberately not `std.space.delete`**:
        // that one is the owner's and destroys a live space through
        // crypto-erasure with a confirmation window. This one is the
        // deployment operator's and refuses anything `active`, because a
        // failed provision has no owner to ask and "clean up a mess" must
        // never be the same button as "destroy a customer's world".
        OpSpec {
            name: crate::reap_op::REAP.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![ParamSpec {
                // The tenant's minted id. Named rather than bound as
                // `$space`, because the op is bound to the **deployment's**
                // space: it is the deployment acting on one of its tenants.
                name: "tenant".to_string(),
                ty: ParamTy::Text { max_len: Some(128) },
                required: true,
                bounds: None,
            }],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("outcome"),
                FieldSpec::new("released"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            requires_idempotency_key: false,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
        OpSpec {
            name: crate::tenant_ops::CREATE.to_string(),
            version_hash: PLATFORM_VERSION.to_string(),
            kind: OpKind::Write,
            params: vec![
                ParamSpec {
                    // The human reference, and **not** an identity. There
                    // is no uniqueness rule on it and there will not be
                    // one: two tenants may be called the same thing, and
                    // the id is what tells them apart.
                    name: "name".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(crate::tenant_ops::MAX_NAME_LEN),
                    },
                    required: true,
                    bounds: None,
                },
                ParamSpec {
                    // The principal who will hold the space. Required and
                    // not defaulted to the caller: a deployment owner
                    // minting a tenant for a customer is the ordinary
                    // case, and a default that made the operator the owner
                    // of every tenant would be a default nobody notices
                    // until they try to hand one over.
                    name: "owner".to_string(),
                    ty: ParamTy::Text { max_len: Some(256) },
                    required: true,
                    bounds: None,
                },
                // ── Which cluster the tenant's database goes in ─────────
                //
                // **Not whether it gets one.** Ruled 2026-09-25: every
                // tenant has a database and its name is minted from the
                // space id, so there is no `$db` and never was a choice
                // about it. These five name a **cluster**, and `$dbHost` is
                // what decides: absent, the deployment's default tenant
                // cluster, which itself defaults to the deployment's own
                // server; present, a cluster this deployment was not
                // configured with, which has to be described in full.
                //
                // `TenantOps::target_for` refuses a half-stated address
                // rather than completing it from this deployment's own
                // connection, because a caller who named a user and forgot
                // the host asked for somewhere else.
                //
                // **The system chooses, not the caller's customer.** Ruled
                // 2026-09-13 (`db-attach-gate`): at provisioning there is
                // no owner yet to consent and no space without a place to
                // store it, so this is a parameter of creation rather than
                // an owner-consented attach op. Changing it afterwards is a
                // two-party act and is 18e.
                //
                // Every parameter is a **name**, never a secret.
                // `$dbCredential` is a label the operator installed out of
                // band, for `ordo_exec::spacedb`'s stated reason: the
                // command log is plaintext by design and a password here
                // would be a password in it.
                ParamSpec {
                    name: "dbHost".to_string(),
                    ty: ParamTy::Text { max_len: Some(255) },
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    name: "dbPort".to_string(),
                    ty: ParamTy::Int,
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    name: "dbUser".to_string(),
                    ty: ParamTy::Text { max_len: Some(63) },
                    required: false,
                    bounds: None,
                },
                ParamSpec {
                    name: "dbCredential".to_string(),
                    ty: ParamTy::Text { max_len: Some(128) },
                    required: false,
                    bounds: None,
                },
                // **A bool, for `platformSealed`'s reason.** Custody as
                // text would put an enum check inside the executor, and an
                // executor may not say `invalid_param` (§4.1) — so a typo
                // would reach the caller as a 500. A bool has no invalid
                // value. Optional only because `$db` is; given `$db` it is
                // required, because the two positions disclose different
                // things about what deleting the space does and defaulting
                // would be the platform choosing that for the owner.
                ParamSpec {
                    name: "dbOwnPerimeter".to_string(),
                    ty: ParamTy::Bool,
                    required: false,
                    bounds: None,
                },
                // **`dryRun`** (`bootstrap.md` §2 item 45). Runs phase 0 —
                // every decidable check, including a dial of the target
                // cluster — and returns the resolved plan without
                // reserving an id or touching a resource. *"This is the
                // surface agents and operators should call first."*
                //
                // A bool for `$dbOwnPerimeter`'s reason: an executor may
                // not raise `invalid_param` (api-engine-design §4.1), so
                // a parameter whose wrong values are the executor's
                // problem is a parameter whose typos arrive as 500s. A
                // bool has no wrong value.
                //
                // Optional and false by default, which is the only safe
                // direction: a caller who forgets it gets the create they
                // asked for, where a caller who forgets a default-true
                // would get silence and no tenant.
                ParamSpec {
                    name: "dryRun".to_string(),
                    ty: ParamTy::Bool,
                    required: false,
                    bounds: None,
                },
            ],
            output_fields: vec![
                FieldSpec::new("space"),
                FieldSpec::new("name"),
                FieldSpec::new("owner"),
                FieldSpec::new("reachable"),
                // Only on the `dryRun` answer, and null otherwise: a plan
                // rendered beside a tenant that was actually created would
                // read as a description of what is standing rather than of
                // what was intended.
                FieldSpec::new("plan"),
            ],
            live_capable: false,
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            // **The one op where the header is required** (`bootstrap.md`
            // §2 item 44). Every other write can be retried at worst into a
            // duplicate row, which the row's key, a `rev` CAS or a
            // `converge` mode bounds. This one mints a KMS key and creates
            // a database *before* it activates the registry row, so a retry
            // after a network failure makes a **second tenant** — a second
            // database, a second master key — and answers with the second
            // one's id. The caller never learns the first exists, both rows
            // are `active` and correct, and nothing reconciles them: the
            // orphan is visible only to somebody reading the cluster.
            requires_idempotency_key: true,
            public: Default::default(),
            refinable: RefineTier::None,
            aggregate_only: false,
            mode_label: None,
            cost: OpCost::default(),
        },
    ]
    .into_iter()
    .chain(member_specs())
    .chain(admission_specs())
    .chain(origins_specs())
    .chain(credential_specs())
    .chain(identity_specs())
    .chain(person_specs())
    .chain(share_specs())
    .chain(invite_specs())
    .chain(self_serve_specs())
    .chain(guest_specs())
    .chain(session_policy_specs())
    .chain(crate::guess_limits::specs())
    .chain(email_policy_specs())
    .chain(crate::share_links::specs())
    .chain(crate::pause::specs())
    .chain(crate::tenant_providers::specs())
    .chain(crate::consent::specs())
    .chain(crate::profile_ops::specs())
    .chain(crate::observed_cost::specs())
    .chain(crate::console_keys::specs())
    .chain(crate::secrets::specs())
    .chain(crate::deploy_ops::specs())
    .chain(crate::fleet::specs())
    .chain(crate::settings::specs())
    .chain(crate::quotas::specs())
    .chain(crate::people_ops::specs())
    .chain(crate::security_record::specs())
    .collect()
}

/// `std.deployment.setSelfServe` (`plan-provisioning-on-signup.md` P2). Every
/// parameter is optional text: a mode word, and two limits that are a number
/// or `unlimited`.
fn self_serve_specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required: false,
        bounds: None,
    };
    vec![OpSpec {
        name: crate::self_serve::SET_SELF_SERVE.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![text("mode", 16), text("signupsPerHour", 16)],
        output_fields: ["mode", "signupsPerHour"]
            .iter()
            .map(|f| FieldSpec::new(*f))
            .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// `std.deployment.setEmailPolicy` ([`crate::contact_email`]): one optional
/// text parameter, `required`: `off`, `all`, a comma list of ops, or
/// `default`, and `changeTouch`: `on`, `off` or `default`. Without either the
/// op reads what is in force.
fn email_policy_specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: crate::contact_email::SET_POLICY.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![
            ParamSpec {
                name: "required".to_string(),
                ty: ParamTy::Text {
                    max_len: Some(4096),
                },
                required: false,
                bounds: None,
            },
            ParamSpec {
                name: "changeTouch".to_string(),
                ty: ParamTy::Text { max_len: Some(16) },
                required: false,
                bounds: None,
            },
        ],
        output_fields: crate::contact_email::FIELDS
            .iter()
            .map(|f| FieldSpec::new(*f))
            .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// `std.deployment.setGuest` (`plan-auth-federated.md` §7A): one optional
/// text parameter, `on` or `off`.
fn guest_specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: crate::auth::guest::SET_GUEST.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![ParamSpec {
            name: "mode".to_string(),
            ty: ParamTy::Text { max_len: Some(8) },
            required: false,
            bounds: None,
        }],
        output_fields: vec![FieldSpec::new("mode")],
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
}

/// The session policy ops ([`crate::session_policy`]): the app developer's
/// layer, a tenant's own values and their read. Every parameter is optional
/// text: a number of seconds, `on` or `off`, a `min-max` range, or
/// `default` to clear that layer's value.
fn session_policy_specs() -> Vec<OpSpec> {
    use crate::session_policy::{APP_FIELDS, READ_TENANT, SET_APP, SET_TENANT, TENANT_FIELDS};
    let text = |name: &str| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(32) },
        required: false,
        bounds: None,
    };
    let values = || {
        vec![
            text("tokenSecs"),
            text("maxSecs"),
            text("remember"),
            text("rememberSecs"),
            text("rememberMaxSecs"),
        ]
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    let mut app = values();
    app.extend([
        text("tokenSecsRange"),
        text("maxSecsRange"),
        text("rememberSecsRange"),
        text("rememberMaxSecsRange"),
    ]);
    vec![
        spec(SET_APP, OpKind::Write, app, &APP_FIELDS),
        spec(SET_TENANT, OpKind::Write, values(), &TENANT_FIELDS),
        OpSpec {
            sync: Syncability::NotSyncable(NotSyncable::NotARead),
            ..spec(READ_TENANT, OpKind::Read, Vec::new(), &TENANT_FIELDS)
        },
    ]
}

/// `std.space.invite`, `std.space.invitations` and
/// `std.space.revokeInvitation` (24e).
fn invite_specs() -> Vec<OpSpec> {
    use crate::invite_ops::{INVITATIONS, INVITE, REVOKE_INVITATION};
    let param = |name: &str, ty: ParamTy, required: bool| ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    };
    let text = |name: &str, max: u32, required: bool| {
        param(name, ParamTy::Text { max_len: Some(max) }, required)
    };
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        sync: if kind == OpKind::Read {
            Syncability::NotSyncable(NotSyncable::NoAnchor)
        } else {
            Syncability::NotSyncable(NotSyncable::NotARead)
        },
        kind,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            INVITE,
            OpKind::Write,
            vec![
                text("role", 32, false),
                param("expires", ParamTy::Duration, false),
                text("to", 256, false),
                text("label", 256, false),
            ],
            &["ticket", "expiresAt", "role", "outcome"],
        ),
        spec(
            INVITATIONS,
            OpKind::Read,
            Vec::new(),
            &[
                "id",
                "role",
                "for",
                "label",
                "issuedBy",
                "issuedAt",
                "expiresAt",
                "status",
                "declinedAt",
            ],
        ),
        spec(
            REVOKE_INVITATION,
            OpKind::Write,
            vec![text("id", 64, true)],
            &["id", "outcome"],
        ),
    ]
}

/// `std.space.share` (`plan-sharing.md` G2). Text parameters carry the op
/// list (comma-separated names) and the rows (a JSON array), because a
/// platform op's parameters are scalars.
fn share_specs() -> Vec<OpSpec> {
    let param = |name: &str, ty: ParamTy, required: bool| ParamSpec {
        name: name.to_string(),
        ty,
        required,
        bounds: None,
    };
    let text = |name: &str, max: u32, required: bool| {
        param(name, ParamTy::Text { max_len: Some(max) }, required)
    };
    let revoke = OpSpec {
        name: crate::share_ops::REVOKE_SHARE.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![text("seat", 256, true)],
        output_fields: [
            "seat",
            "principal",
            "grants",
            "aclRows",
            "suspended",
            "outcome",
            "door",
        ]
        .iter()
        .map(|f| FieldSpec::new(*f))
        .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![OpSpec {
        name: crate::share_ops::SHARE.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params: vec![
            text("ops", 4096, true),
            text("rows", 16384, true),
            param("expires", ParamTy::Duration, true),
            text("to", 256, false),
            text("label", 256, false),
        ],
        output_fields: ["ticket", "expiresAt", "windowEndsAt", "ops", "outcome"]
            .iter()
            .map(|f| FieldSpec::new(*f))
            .collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }]
    .into_iter()
    .chain([revoke])
    .chain([OpSpec {
        // `std.space.shares` (owner ruling 2026-10-05). `Read`; the
        // manager's and the owner's, as `revokeShare`, `links` and
        // `invitations` are.
        name: crate::share_ops::SHARES.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: Vec::new(),
        output_fields: [
            "seat",
            "displayName",
            "guest",
            "ops",
            "grantedAt",
            "expiresAt",
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
    }])
    .collect()
}

/// `std.identity.recover`, `std.identity.recoveryTicket`,
/// `std.identity.cancelRecovery` and `std.space.succeedOwner` (recovery R5).
fn identity_specs() -> Vec<OpSpec> {
    use crate::identity_ops::{
        APPROVE_MEMBER_RECOVERY, APPROVE_RECOVERY, CANCEL_RECOVERY, CLOSE_CONTEST, OBJECT_TO_CASE,
        OPEN_CASES, RECOVER, RECOVERY_TICKET, RECOVER_MEMBER, RESOLVE_OBJECTION,
        SET_RECOVERY_POLICY, SUCCEED_OWNER,
    };
    let optional = |name: &str, ty: ParamTy| ParamSpec {
        name: name.to_string(),
        ty,
        required: false,
        bounds: None,
    };
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            RECOVER,
            vec![
                text("principal", 256, true),
                text("evidence", 64, true),
                text("reference", 128, false),
            ],
            &[
                "case",
                "principal",
                "state",
                "maturesAt",
                "approvalsRequired",
                "ticket",
                "ticketExpiresAt",
                "outcome",
            ],
        ),
        spec(
            APPROVE_RECOVERY,
            vec![text("case", 256, true)],
            &["case", "approvals", "approvalsRequired", "outcome"],
        ),
        spec(
            APPROVE_MEMBER_RECOVERY,
            vec![text("member", 256, true), text("case", 256, true)],
            &[
                "case",
                "member",
                "approvals",
                "approvalsRequired",
                "outcome",
            ],
        ),
        spec(
            RECOVERY_TICKET,
            vec![text("case", 256, true)],
            &["case", "ticket", "ticketExpiresAt", "outcome"],
        ),
        spec(
            CANCEL_RECOVERY,
            vec![text("case", 256, true), text("reason", 64, false)],
            &["case", "outcome"],
        ),
        spec(
            RECOVER_MEMBER,
            vec![text("member", 256, true), text("case", 256, false)],
            &[
                "case",
                "member",
                "principal",
                "state",
                "maturesAt",
                "approvalsRequired",
                "ticket",
                "ticketExpiresAt",
                "outcome",
            ],
        ),
        spec(
            SET_RECOVERY_POLICY,
            vec![
                optional("hold", ParamTy::Bool),
                optional("delaySecs", ParamTy::Int),
                optional("approvals", ParamTy::Int),
                optional("pathEnabled", ParamTy::Bool),
                optional("codesEnabled", ParamTy::Bool),
                optional("managersMayRecover", ParamTy::Bool),
            ],
            &[
                "space",
                "hold",
                "delaySecs",
                "approvals",
                "pathEnabled",
                "codesEnabled",
                "managersMayRecover",
                "outcome",
            ],
        ),
        spec(
            SUCCEED_OWNER,
            vec![
                text("tenant", 256, true),
                text("principal", 256, false),
                text("evidence", 64, false),
                text("reference", 128, false),
                text("case", 256, false),
                optional("departed", ParamTy::Json),
                optional("endGrants", ParamTy::Bool),
            ],
            &[
                "case",
                "tenant",
                "principal",
                "member",
                "from",
                "role",
                "maturesAt",
                "departed",
                "endsGrants",
                "grantsEnded",
                "delegationsEnded",
                "notEnded",
                "outcome",
                "door",
            ],
        ),
        spec(
            OBJECT_TO_CASE,
            vec![text("case", 256, true)],
            &["case", "tenant", "path", "state", "outcome"],
        ),
        spec(
            RESOLVE_OBJECTION,
            vec![
                text("case", 256, true),
                text("decision", 16, true),
                text("reason", 64, true),
            ],
            &[
                "case",
                "decision",
                "reason",
                "objections",
                "state",
                "outcome",
            ],
        ),
        spec(
            CLOSE_CONTEST,
            vec![text("case", 256, true), text("reason", 64, true)],
            &["case", "principal", "reason", "retired", "state", "outcome"],
        ),
        OpSpec {
            kind: OpKind::Read,
            sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
            ..spec(
                OPEN_CASES,
                vec![text("tenant", 256, false)],
                &[
                    "case",
                    "path",
                    "tenant",
                    "principal",
                    "evidence",
                    "reference",
                    "openedAt",
                    "maturesAt",
                    "state",
                    "mature",
                    "paused",
                    "approvals",
                    "approvalsRequired",
                    "departed",
                    "objections",
                ],
            )
        },
    ]
}

/// `std.person.tenants`: one read, no parameter.
fn person_specs() -> Vec<OpSpec> {
    vec![OpSpec {
        name: MY_TENANTS.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Read,
        params: Vec::new(),
        output_fields: vec![
            FieldSpec::new("space"),
            FieldSpec::new("name"),
            FieldSpec::new("role"),
            FieldSpec::new("status"),
        ],
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

/// `std.space.createPerson`, `std.space.resetCredential` and
/// `std.space.setTemporaryPassword` (E6).
fn credential_specs() -> Vec<OpSpec> {
    use crate::credential_ops::{CREATE_PERSON, RESET_CREDENTIAL, SET_TEMPORARY_PASSWORD};
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, params: Vec<ParamSpec>, out: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params,
        output_fields: out.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            CREATE_PERSON,
            vec![
                text("display", 256, true),
                text("role", 32, false),
                text("purpose", 16, false),
            ],
            &[
                "principal",
                "member",
                "role",
                "ticket",
                "expiresAt",
                "outcome",
                "door",
            ],
        ),
        spec(
            RESET_CREDENTIAL,
            vec![text("member", 256, true)],
            &["principal", "member", "ticket", "expiresAt", "outcome"],
        ),
        spec(
            SET_TEMPORARY_PASSWORD,
            vec![
                text("member", 256, true),
                text("identifier", 256, true),
                text("password", 1024, true),
            ],
            &["principal", "member", "outcome"],
        ),
    ]
}

/// `std.space.setAdmission` and `std.space.admission` (E4, E5). The claim
/// code op was removed (owner, 2026-10-30).
fn admission_specs() -> Vec<OpSpec> {
    use crate::admission::{ADMISSION, SET_ADMISSION};
    let param = |name: &str, ty: ParamTy| ParamSpec {
        name: name.to_string(),
        ty,
        required: false,
        bounds: None,
    };
    let base = |name: &str, kind: OpKind, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: vec![
            FieldSpec::new("open"),
            FieldSpec::new("role"),
            FieldSpec::new("providers"),
            FieldSpec::new("firstComer"),
            FieldSpec::new("secondFactor"),
            FieldSpec::new("presenceFrom"),
            FieldSpec::new("ceiling"),
        ],
        live_capable: false,
        sync: Syncability::NotSyncable(match kind {
            OpKind::Read => NotSyncable::NoAnchor,
            _ => NotSyncable::NotARead,
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        base(
            SET_ADMISSION,
            OpKind::Write,
            vec![
                param("open", ParamTy::Bool),
                param("role", ParamTy::Text { max_len: Some(32) }),
                param(
                    "providers",
                    ParamTy::Text {
                        max_len: Some(2048),
                    },
                ),
                param("firstComer", ParamTy::Bool),
                param("secondFactor", ParamTy::Text { max_len: Some(32) }),
                param(
                    "presenceFrom",
                    ParamTy::Text {
                        max_len: Some(1024),
                    },
                ),
            ],
        ),
        base(ADMISSION, OpKind::Read, Vec::new()),
    ]
}

/// `std.space.setOrigins` and `std.space.origins`
/// (`crate::tenant_origins`). The admission ops' shape, renamed.
fn origins_specs() -> Vec<OpSpec> {
    use crate::tenant_origins::{ORIGINS, SET_ORIGINS};
    admission_specs()
        .into_iter()
        .take(2)
        .zip([SET_ORIGINS, ORIGINS])
        .map(|(mut spec, name)| {
            spec.name = name.to_string();
            spec.output_fields = vec![FieldSpec::new("origins"), FieldSpec::new("max")];
            spec.params = if name == SET_ORIGINS {
                vec![ParamSpec {
                    name: "origins".to_string(),
                    ty: ParamTy::Text {
                        max_len: Some(8192),
                    },
                    required: true,
                    bounds: None,
                }]
            } else {
                Vec::new()
            };
            spec
        })
        .collect()
}

/// The membership commands' specs (`plan-enrolment.md` E3). Their own
/// function so the five share one shape and a sixth is one line.
fn member_specs() -> Vec<OpSpec> {
    use crate::member_ops::{ADMIT, REINSTATE, REMOVE, SET_ROLE, SUSPEND};
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let spec = |name: &str, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: PLATFORM_VERSION.to_string(),
        kind: OpKind::Write,
        params,
        output_fields: vec![
            FieldSpec::new("member"),
            FieldSpec::new("principal"),
            FieldSpec::new("role"),
            FieldSpec::new("status"),
            FieldSpec::new("outcome"),
            FieldSpec::new("door"),
        ],
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NotARead),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            ADMIT,
            vec![
                text("principal", 256, true),
                text("role", 32, true),
                text("display", 256, false),
            ],
        ),
        spec(
            SET_ROLE,
            vec![text("member", 256, true), text("role", 32, true)],
        ),
        spec(SUSPEND, vec![text("member", 256, true)]),
        spec(REINSTATE, vec![text("member", 256, true)]),
        spec(REMOVE, vec![text("member", 256, true)]),
        // 27aq. No parameter: the seat is the caller's own, so nobody can
        // name another person's seat to leave.
        OpSpec {
            output_fields: ["member", "status", "rule", "effects", "door"]
                .iter()
                .map(|f| FieldSpec::new(*f))
                .collect(),
            ..spec(crate::release_ops::LEAVE, Vec::new())
        },
    ]
}

/// Which platform ops are **owner-only**.
///
/// The policy lives beside the ops rather than in the registration loop,
/// because a role list assembled where the engine is built is a role list
/// that gets loosened by whoever is editing the engine. `callersOf` is
/// manager-and-owner (an audit surface its own administrator cannot read
/// pushes deployments to hand out owner); the exit ops are the owner's
/// alone, because ending a space and disclosing all of it are not
/// administration.
pub fn owner_only(name: &str) -> bool {
    name == crate::exit_ops::EXPORT
        || name == crate::exit_ops::IMPORT
        || name == crate::exit_ops::DELETE
        // Nominating where a space's history is published is a custody
        // decision of the same class as export: it names a third party who
        // will hold signed statements about this chain forever. The owner's.
        //
        // `std.space.chainProof` is deliberately **absent** from this list.
        // The artifact it produces is positions, 32-byte digests and
        // signatures -- no rows, no images, no actors, no correlation ids --
        // so it discloses strictly less than `callersOf`, which names
        // principals by id. Refusing it to the manager would mean the role
        // that administers a space cannot check whether its history is
        // intact, which is `callersOf`'s own argument for a manager floor.
        || name == crate::attest_ops::ATTEST_TO
        // Reading the destination back, and the deliveries to it that gave
        // up, are the owner's who set it (owner ruling 2026-10-05).
        || name == crate::attest_ops::ATTESTATION
        || name == crate::attest_ops::ATTESTATION_DEAD_LETTERS
        // Attaching a bucket decides where a space's files live and who
        // can read them — which decides what leaving the platform means.
        // A manager administers who may do what; this is not that.
        || name == crate::bucket_ops::ATTACH_BUCKET
        // Detaching is the same decision run back (owner, 2026-10-05).
        || name == crate::bucket_ops::DETACH_BUCKET
        // 18l. Owner, and then narrower still: `ApplyOps::apply` refuses
        // unless the bound space is the platform's own, because this list
        // decides a ROLE and the question here is also WHOSE deployment.
        // A manager administers who may do what inside a space; replacing
        // the program every space runs is not that.
        || name == crate::apply_op::APPLY
        // G1 to G3: the program's history, its rollback and its preview
        // are the deployment's, on `APPLY`'s terms.
        || crate::deploy_ops::OPS.contains(&name)
        // 17aa. Owner, and then narrower still for `APPLY`'s exact reason:
        // `TenantOps::create` refuses unless the bound space is the
        // platform's own. Creating the next tenant decides who the
        // deployment serves and who pays for it, which is not the
        // administration of a space that already exists.
        || name == crate::tenant_ops::CREATE
        || name == crate::reap_op::REAP
        // E3. Who is in a space, and at what level, is the owner's to
        // decide. See `crate::member_ops` for why a manager may not.
        || crate::member_ops::OPS.contains(&name)
        // E4. How open a space is, is the owner's to decide, and to read.
        || name == crate::admission::SET_ADMISSION
        || name == crate::admission::ADMISSION
        // Which other sites' pages may read the tenant's answers is the
        // owner's to decide, and to read (`design-public-path.md` §8).
        || crate::tenant_origins::OPS.contains(&name)
        // E6. Making a person, and taking a credential from a person, are
        // the owner's.
        || crate::credential_ops::OPS.contains(&name)
        // Recovery R5. Restoring a person's way in, and making a tenant's
        // owner, are the deployment owner's, and answer in its own space.
        || crate::identity_ops::OPS.contains(&name)
        // P2. Whether a sign-in gives a person a tenant, and how many
        // sign-ups an hour, is the deployment's and answers in its own space.
        || crate::self_serve::OPS.contains(&name)
        // Observed op cost (`plan-tiers.md`, ruled 2026-10-23) is the
        // deployment owner's to read, and answers in its own space.
        || crate::observed_cost::OPS.contains(&name)
        // G14. Which consoles may sign in as the deployment owner is the
        // owner's, and the ops answer only in the deployment's own space.
        || crate::console_keys::OPS.contains(&name)
        // Backend functions. The app secrets are the deployment owner's, and
        // the ops answer only in the deployment's own space.
        || crate::secrets::OPS.contains(&name)
        // G4. The list of every tenant is the deployment owner's, and the
        // op answers only in the deployment's own space.
        || crate::fleet::OPS.contains(&name)
        // G7. Every setting the deployment took, and from where, is the
        // deployment owner's, in the deployment's own space.
        || crate::settings::OPS.contains(&name)
        // G9. How much a tenant may use is the deployment owner's to read and
        // set, and the ops answer only in the deployment's own space.
        || crate::quotas::OPS.contains(&name)
        // G8. The deployment's own security record names suspects across
        // every tenant: the deployment owner's, in its own space.
        || crate::security_record::OPS.contains(&name)
        // §7A. Whether a new guest may be made is the deployment's.
        || crate::auth::guest::OPS.contains(&name)
        // Whether a person must have a verified email address is the app
        // developer's, and the op answers only in the deployment's space.
        || crate::contact_email::OPS.contains(&name)
        // How long sessions last: the app developer's layer answers in the
        // deployment's own space; a tenant's own values are its owner's,
        // unless the app developer's access template says otherwise.
        || name == crate::session_policy::SET_APP
        || name == crate::session_policy::SET_TENANT
        // L1. A tenant's own choice of how long its links live is the tenant
        // owner's. Minting, revoking and listing are the manager's and the
        // owner's, like `share`.
        || name == crate::share_links::SET_LINK_LIFETIME
        // A tenant's link password limits are its owner's. The app
        // developer's answer only in the deployment's own space.
        || name == crate::share_links::SET_PASSWORD_LIMITS
        || name == crate::share_links::SET_APP_PASSWORD_LIMITS
        // The password slowdown: the app developer's answers only in the
        // deployment's own space; a tenant's own values are its owner's,
        // unless the app developer's access template says otherwise.
        || crate::guess_limits::OPS.contains(&name)
        // 27ar. Pausing a tenant is a deployment operation (owner,
        // 2026-10-05), and the two answer only in its own space.
        || crate::pause::deployment_scoped(name)
        // L1 (owner, 2026-10-09). A tenant's own sign-in providers are its
        // owner's to add, change, retire and read.
        || crate::tenant_providers::OPS.contains(&name)
}

/// The roles that reach a platform op when nothing else names it: no access
/// template entry, no pause policy and no tenant setter rule. Every member
/// for leaving and the consent ceremony, the owner alone for
/// [`owner_only`] ops a manager cannot reach, and manager and owner for the
/// rest. The engine's registration loop and `ordo ops` both read it, so the
/// roles the CLI prints are the roles the door grants by default.
pub fn default_roles(name: &str) -> &'static [ordo_identity::principal::Role] {
    use ordo_identity::principal::Role;
    if person_scoped(name)
        || crate::release_ops::any_member(name)
        || crate::consent::any_member(name)
    {
        &[Role::Viewer, Role::Editor, Role::Manager, Role::Owner]
    } else if owner_only(name) && !manager_reachable(name) {
        &[Role::Owner]
    } else {
        &[Role::Manager, Role::Owner]
    }
}

/// Recovery ops a tenant's **manager** may reach at the door, besides its
/// owners: a tenant's recovery of a member and the approval of one. Whether a
/// manager may in fact is the app developer's setting
/// (`ORDO_RECOVERY_MANAGERS`, off unless set) and the tenant's own, and the
/// op checks both, so the door grant alone opens nothing.
pub fn manager_reachable(name: &str) -> bool {
    name == crate::identity_ops::RECOVER_MEMBER
        || name == crate::identity_ops::APPROVE_MEMBER_RECOVERY
        // A manager objects to a case about the tenant's owner (owner,
        // 2026-10-24). The op refuses anyone but an active manager.
        || name == crate::identity_ops::OBJECT_TO_CASE
}

/// Is `name` one of this build's platform ops?
pub fn is_platform_op(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

/// A platform op's catalog manifest.
///
/// Hand-built, because `crate::manifest` reads an `ordo_plan` artifact and
/// there is no plan. Every field a compiled manifest carries is present —
/// the key set is asserted against a real one in `tests/callers_of.rs`, so
/// a catalog consumer never meets a manifest missing a key it reads — and
/// the ones that have no meaning here say so rather than being omitted:
///
/// - `entities` and `scope` are **empty**, and `platform` is true beside
///   them. An empty scope with no stated reason reads as *"touches
///   nothing"*, which for this op would be the opposite of the truth.
/// - `guards` is empty and `unguarded` is **false**. `unguarded` is a
///   disclosure meaning *"a declared guard was bypassed"*; there is no
///   guard here to bypass, and marking it would put a bypass warning on a
///   consent page for a bypass that did not happen.
/// - `release` is empty: it reads no sealed field, so it needs no key.
pub fn manifest(spec: &OpSpec) -> serde_json::Value {
    json!({
        "op": spec.name,
        "version": spec.version_hash,
        "kind": match spec.kind {
            OpKind::Read => "read",
            OpKind::Write => "write",
            OpKind::Delete => "delete",
        },
        "mode": serde_json::Value::Null,
        "statements": [],
        "cost": { "class": "sql", "rows": "bounded by the space's membership" },
        "params": spec.params.iter().map(|p| json!({
            "name": p.name,
            // Named rather than wildcarded, so a platform op that takes a
            // type nobody has spelled here is a compile error rather than a
            // manifest that calls an int a text.
            "type": match p.ty {
                ParamTy::Text { .. } => "text",
                ParamTy::Int => "int",
                ParamTy::Money { .. } => "money",
                ParamTy::Decimal { .. } => "decimal",
                ParamTy::Date => "date",
                ParamTy::Bool => "bool",
                ParamTy::Id => "id",
                ParamTy::Duration => "duration",
                ParamTy::Datetime => "datetime",
                ParamTy::Uri => "uri",
                ParamTy::Json => "json",
            },
            "required": p.required,
            // The one bound a platform op has, and the document used to say
            // `null` here regardless.
            //
            // A platform op declares no `<=` value bound (`ParamSpec::bounds`
            // is `None` on all three). But `ParamTy::Text { max_len }` is a
            // LENGTH bound, `ordo_api::bind` refuses a longer value with
            // `bound_violation`, and two of these ops carry one — so `null`
            // was a descriptor claiming unbounded about a parameter the door
            // measures. It is the same key an authored `text(n)` would fill
            // if the grammar lexed one (it does not: `ScalarTy::Text` carries
            // no length, which is recorded as owed), and it is unambiguous
            // because `param_bound` refuses `<=` on `text` — so a `max` on a
            // `text` parameter can only ever be a character count.
            "max": match p.ty {
                ParamTy::Text { max_len: Some(n) } => serde_json::Value::from(n.to_string()),
                _ => serde_json::Value::Null,
            },
            // No `take $n` — a platform op has no compiled pipeline at all —
            // so no derived floor.
            "min": serde_json::Value::Null,
            // A platform op's `id` parameters are principal and space ids —
            // not an entity's typed id — so there is no entity for action
            // discovery to point a row action at, and no declared field for a
            // form to read a value set off. Both are `null` rather than
            // absent, so the key set a catalog consumer reads is one key set.
            "id_entity": serde_json::Value::Null,
            "field_ref": serde_json::Value::Null,
        })).collect::<Vec<_>>(),
        "outputs": spec.output_fields.iter().map(|f| json!({
            "name": f.name, "sealed": f.sealed, "refinable": f.refinable,
            // **`null`, and it is a real gap rather than a shrug.**
            // `ordo_api::FieldSpec` — the wire's output manifest — carries
            // no type at all, so a platform op is the one kind of op whose
            // result columns nothing in the tree can name a type for. A
            // compiled op's types come off the bound query, which is why
            // this is only ever null here. A descriptor consumer must treat
            // an untyped column as opaque text; giving platform specs
            // declared output types is owed work, recorded rather than
            // papered over with a guess.
            "type": serde_json::Value::Null,
            "id_entity": serde_json::Value::Null,
        })).collect::<Vec<_>>(),
        // No plan, so no anchor table, no grouping and no proof of a single
        // row — the all-negative form a write op and a failed bind also
        // take, which is what keeps one key set across every kind.
        //
        // `shape` is `rows` for ALL THREE, write and delete included, and
        // that is not the compiled default: a platform op does not go
        // through `run_write_body`, so `std.space.export` and
        // `std.space.delete` answer with a real row over their declared
        // `output_fields` rather than with a write summary. It is exactly
        // why the shape is a producer's fact and not something the
        // descriptor derives from `kind`.
        "result": {
            "anchor": serde_json::Value::Null,
            "grouped": false,
            "single_row": false,
            "shape": "rows",
        },
        "entities": [],
        "scope": [],
        // The pinning work added `row_scope` to every compiled manifest.
        // A platform op reads runtime state, not entity rows, so its row
        // scope is honestly empty — same shape as `scope`, present so a
        // consent page renders one schema for both kinds of op.
        "row_scope": [],
        "release": [],
        "guards": [],
        "unguarded": false,
        "unbounded": false,
        "bypassed_guards": [],
        "error": serde_json::Value::Null,
        // The one key a compiled manifest does not have. A reader that
        // renders a catalog must be able to tell "the compiler proved this"
        // from "the platform asserts this", because the evidence behind the
        // two is not the same kind of evidence.
        "platform": true,
    })
}

/// Every platform op's manifest, op-name sorted — what
/// [`crate::deploy::Deployment::manifests`] appends to the program's.
pub fn manifests() -> Vec<(String, serde_json::Value)> {
    let mut out: Vec<(String, serde_json::Value)> = specs()
        .iter()
        .map(|s| (s.name.clone(), manifest(s)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The runtime state platform ops read, and the handler over it.
///
/// # Why the grant registry is not optional
///
/// §6 rule 4 requires `callersOf` to be **exhaustive**, and 8r's whole
/// history is that a surface answering confidently while omitting the token
/// column is worse than an absent one. A `None` here would make that state
/// reachable at runtime, so it is not representable: a deployment either
/// hands over a registry and serves the op, or hands over nothing and does
/// not register it, in which case the door answers as it does for every
/// name nobody registered. There is no third state and therefore no partial
/// answer to guard against.
pub struct PlatformOps {
    /// The same engine the door decides with. Shared rather than rebuilt:
    /// an audit surface computed from a second copy of the access state
    /// would answer about a deployment that is not the one serving.
    pub engine: Arc<AccessEngine>,
    pub grants: Arc<dyn GrantRegistry>,
    /// The delegation column's source. `None` is a deployment that records
    /// no delegations, and the column is then empty **because there is
    /// nothing in it** — not because it went unread. A deployment that
    /// records delegations and does not hand the registry over here would
    /// have `callersOf` omit exactly the callers acting as other people,
    /// which is the exhaustiveness failure 8r existed to close; the door's
    /// registry and this one are the same `Arc` in `IdentityAccess`'s
    /// deployment, so the two cannot come apart by accident.
    pub delegations: Option<Arc<dyn DelegationRegistry>>,
    /// Wall clock, injectable so a test can pin expiry and so the op reads
    /// the same clock the door does.
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// The exit ops' state, when this deployment wired them.
    ///
    /// `Option`, and the same argument the grant registry makes above
    /// applies once more: a deployment either hands this over **and**
    /// registers the two specs, or hands over neither and the door answers
    /// for `std.space.export` exactly as it answers for any name nobody
    /// registered. The refusal below exists for the assembly that registers
    /// the specs and forgets the state, which is a bug in the deployment
    /// rather than a state a caller reaches on a correct one.
    pub exit: Option<Arc<crate::exit_ops::ExitOps>>,
    /// The attestation ops' state, on the same terms as `exit`: a deployment
    /// either wires this **and** registers the two specs, or wires neither.
    pub attest: Option<Arc<crate::attest_ops::AttestOps>>,
    /// The bucket op's state. Same shape and same reason as `exit`: a
    /// deployment that registers `std.space.attachBucket` and wires
    /// nothing for it is a bug in the deployment, not a state a caller
    /// reaches on a correct one.
    pub buckets: Option<Arc<crate::bucket_ops::BucketOps>>,
    /// 11-15: `std.space.reap`'s state. `None` is a deployment that
    /// registered the op and wired nothing, which `reap_ops` refuses.
    pub reap: Option<Arc<crate::reap_op::ReapOps>>,
    /// The membership commands' state (E3). `None` is a deployment that
    /// registered them and wired nothing, which `member_ops` refuses.
    pub members: Option<Arc<crate::member_ops::MemberOps>>,
    /// Leaving a tenant (27aq). `None` is a deployment that registered
    /// `std.space.leave` and wired nothing, which the op refuses.
    pub release: Option<Arc<crate::release_ops::ReleaseOps>>,
    /// The password give and the reset's state (E6). `None` is a deployment
    /// with no authentication, and the ops then refuse by saying so.
    pub credentials: Option<Arc<crate::credential_ops::CredentialOps>>,
    /// Admission policy's state (E4).
    pub admission: Option<Arc<crate::admission::Admission>>,
    /// The tenants' own CORS origins (`design-public-path.md` §8).
    pub origins: Option<Arc<crate::tenant_origins::TenantOriginMap>>,
    /// Sharing's state (G2). `None` is a deployment with no authentication,
    /// where an invitation has nobody to be redeemed by.
    pub share: Option<Arc<crate::share_ops::ShareOps>>,
    /// Open share links' state (L1 to L3). `None` where sharing is not wired.
    pub links: Option<Arc<crate::share_links::ShareLinks>>,
    /// Invitations' state (24e). `None` is a deployment with no
    /// authentication, where an invitation has nobody to be redeemed by.
    pub invite: Option<Arc<crate::invite_ops::InviteOps>>,
    /// The deployment owner's recovery and succession ops (R5). `None` is an
    /// assembly that registered them and wired nothing, which they refuse.
    pub identity: Option<Arc<crate::identity_ops::IdentityOps>>,
    /// The self-serve provisioning setting's state (P2).
    pub self_serve: Option<Arc<crate::self_serve::SelfServeOps>>,
    /// The guest sign-in setting's state (`plan-auth-federated.md` §7A).
    pub guest: Option<Arc<crate::auth::guest::GuestOps>>,
    /// The session policy layers ([`crate::session_policy`]).
    pub session_policy: Option<Arc<crate::session_policy::SessionPolicies>>,
    /// The password slowdown's layers ([`crate::guess_limits`]).
    pub guess_limits: Option<Arc<crate::guess_limits::GuessLimits>>,
    /// The email requirement's state ([`crate::contact_email`]).
    pub contact: Option<Arc<crate::contact_email::ContactEmails>>,
    /// What a tenant's deletion does to identities (E7). `None` leaves them
    /// as they were, which is a deployment with no `$managed` table.
    pub identities: Option<Arc<crate::tenant_identities::TenantIdentities>>,
    /// The deploy door's state (18l). Same shape and same reason as
    /// `exit`: a deployment that registers `std.deployment.apply` and
    /// wires nothing for it is a bug in the deployment, not a state a
    /// caller reaches on a correct one.
    pub apply: Option<Arc<crate::apply_op::ApplyOps>>,
    /// `std.space.create`'s state. Separate from [`Self::apply`] and not
    /// folded into it, though both are deployment-scoped and both are
    /// owner-only: applying a program needs a pool, and minting a tenant
    /// needs a pool **and the `KeyProvider`**, so one struct would hand
    /// the deploy path a custody handle it has no business holding.
    pub tenants: Option<Arc<crate::tenant_ops::TenantOps>>,
    /// Pausing a tenant (27ar): the four ops' state, and the register the
    /// pause gate reads. `None` is a deployment with no gate, where the
    /// four ops refuse by saying so.
    pub pause: Option<Arc<crate::pause::PauseOps>>,
    /// Where the live engine is read from, when the door can replace its
    /// engine. `None` reads `engine` above, which is right for every
    /// assembly whose engine is built once.
    ///
    /// The binary rebuilds its door on a program swap
    /// (`crate::runtime::SwappableAccess`), and the rebuilt door carries a
    /// **new** `AccessEngine`. An `Arc` captured at boot would then be the
    /// previous version's engine: `callersOf` would answer from manifest pins
    /// and memberships the door no longer decides with. A source read on
    /// every call keeps the audit surface on the engine that is serving.
    pub engine_source: Option<Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>>,
    /// The tenants' own sign-in providers (`plan-tenant-providers.md`). Set
    /// once, after the authentication state that shares them is built.
    /// Absent, the four ops refuse naming the setting.
    pub tenant_connections: std::sync::OnceLock<Arc<crate::tenant_providers::TenantConnections>>,
    /// The consent ceremony (11-4b): review, approve, decline, list and
    /// revoke. `None` is an assembly that registered the five ops and wired
    /// nothing, which they refuse.
    pub consent: Option<Arc<crate::consent::Ceremony>>,
    /// A tenant's own access profile and groups (27ah). `None` is an
    /// assembly that registered the ops and wired nothing, which they
    /// refuse.
    pub profiles: Option<Arc<crate::profile_ops::ProfileOps>>,
    /// Observed op cost's state ([`crate::observed_cost`]). `None` is an
    /// assembly that registered the op and wired nothing, which it refuses.
    pub observed_cost: Option<Arc<crate::observed_cost::ObservedCostOps>>,
    /// Console keys' state ([`crate::console_keys`], G14). `None` is an
    /// assembly with no sign-in, which the three ops refuse naming the
    /// setting.
    pub console_keys: Option<Arc<crate::console_keys::ConsoleKeyOps>>,
    /// The app secrets ([`crate::secrets`]). `None` is an assembly that
    /// registered the ops and wired nothing, which they refuse.
    pub secrets: Option<Arc<crate::secrets::SecretOps>>,
    /// The list of tenants' state ([`crate::fleet`], G4). `None` is an
    /// assembly that registered the op and wired nothing, which it says.
    pub tenant_list: Option<Arc<crate::fleet::TenantListOps>>,
    /// The deployment's settings ([`crate::settings`], G7). `None` is an
    /// assembly that registered the op and wired nothing, which it says.
    pub settings: Option<Arc<crate::settings::SettingsOps>>,
    /// The quotas ([`crate::quotas::QuotaOps`], G9). `None` is an assembly
    /// that registered the ops and wired nothing, which they say.
    pub quotas: Option<Arc<crate::quotas::QuotaOps>>,
    /// The deployment's security record ([`crate::security_record`], G8).
    /// `None` is an assembly that registered the op and wired nothing.
    pub security_record: Option<Arc<crate::security_record::SecurityRecordOps>>,
}

impl PlatformOps {
    /// Read the engine the door decides with on every call, rather than the
    /// one handed to [`PlatformOps::new`]. For an assembly that swaps its
    /// door; see the field's doc.
    pub fn with_engine_source(
        mut self,
        source: Arc<dyn Fn() -> Arc<AccessEngine> + Send + Sync>,
    ) -> PlatformOps {
        self.engine_source = Some(source);
        self
    }

    /// The engine to answer from: the live one when a source is wired.
    pub(crate) fn engine(&self) -> Arc<AccessEngine> {
        match &self.engine_source {
            Some(source) => source(),
            None => self.engine.clone(),
        }
    }

    pub fn new(
        engine: Arc<AccessEngine>,
        grants: Arc<dyn GrantRegistry>,
        now: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> PlatformOps {
        PlatformOps {
            engine,
            grants,
            now,
            delegations: None,
            exit: None,
            attest: None,
            buckets: None,
            reap: None,
            members: None,
            release: None,
            admission: None,
            origins: None,
            share: None,
            links: None,
            invite: None,
            identity: None,
            self_serve: None,
            guest: None,
            session_policy: None,
            guess_limits: None,
            contact: None,
            identities: None,
            credentials: None,
            apply: None,
            tenants: None,
            pause: None,
            engine_source: None,
            tenant_connections: std::sync::OnceLock::new(),
            consent: None,
            profiles: None,
            observed_cost: None,
            console_keys: None,
            secrets: None,
            tenant_list: None,
            settings: None,
            quotas: None,
            security_record: None,
        }
    }

    /// Wire a tenant's own profile and groups (27ah).
    pub fn with_profiles(mut self, profiles: Arc<crate::profile_ops::ProfileOps>) -> PlatformOps {
        self.profiles = Some(profiles);
        self
    }

    /// Wire the consent ceremony (11-4b).
    pub fn with_consent(mut self, consent: Arc<crate::consent::Ceremony>) -> PlatformOps {
        self.consent = Some(consent);
        self
    }

    /// Wire pausing (27ar): the four ops, and the gate every op call passes
    /// through in [`PlatformExecutor`].
    pub fn with_pause(mut self, pause: Arc<crate::pause::PauseOps>) -> PlatformOps {
        self.pause = Some(pause);
        self
    }

    fn pause_ops(&self) -> Result<&crate::pause::PauseOps, ExecError> {
        self.pause.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered the pause ops and wired no state for them",
            )
        })
    }

    /// Give the audit surface the delegation registry the door consults, so
    /// the fifth via can be answered from the same rows the door decides on.
    pub fn with_delegations(mut self, registry: Arc<dyn DelegationRegistry>) -> PlatformOps {
        self.delegations = Some(registry);
        self
    }

    /// Wire the exit ops. A builder rather than a fourth argument, so a
    /// deployment that serves only `callersOf` needs no change.
    pub fn with_exit(mut self, exit: Arc<crate::exit_ops::ExitOps>) -> PlatformOps {
        self.exit = Some(exit);
        self
    }

    /// Wire the attestation ops.
    pub fn with_attest(mut self, attest: Arc<crate::attest_ops::AttestOps>) -> PlatformOps {
        self.attest = Some(attest);
        self
    }

    fn attest(&self) -> Result<&crate::attest_ops::AttestOps, ExecError> {
        self.attest.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered an attestation op and wired no state for it; \
                 the two go together, because an attestation surface that answers \
                 partially is worse than one that is not there",
            )
        })
    }

    /// Wire the bucket op. Same builder shape and same reason: a
    /// deployment that serves no blobs needs no change.
    pub fn with_buckets(mut self, buckets: Arc<crate::bucket_ops::BucketOps>) -> PlatformOps {
        self.buckets = Some(buckets);
        self
    }

    fn buckets(&self) -> Result<&crate::bucket_ops::BucketOps, ExecError> {
        self.buckets.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered `std.space.attachBucket` and wired no state                  for it — a storage-custody op that answers partially is worse than one                  that is not there",
            )
        })
    }

    /// Wire the deploy door (18l). Same builder shape: a deployment that
    /// serves no control plane needs no change.
    pub fn with_apply(mut self, apply: Arc<crate::apply_op::ApplyOps>) -> PlatformOps {
        self.apply = Some(apply);
        self
    }

    pub fn with_tenants(mut self, tenants: Arc<crate::tenant_ops::TenantOps>) -> PlatformOps {
        self.tenants = Some(tenants);
        self
    }

    pub fn with_release(mut self, release: Arc<crate::release_ops::ReleaseOps>) -> PlatformOps {
        self.release = Some(release);
        self
    }

    fn release_ops(&self) -> Result<&crate::release_ops::ReleaseOps, ExecError> {
        self.release.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered std.space.leave and wired no state for it",
            )
        })
    }

    pub fn with_members(mut self, members: Arc<crate::member_ops::MemberOps>) -> PlatformOps {
        self.members = Some(members);
        self
    }

    fn member_ops(&self) -> Result<&crate::member_ops::MemberOps, ExecError> {
        self.members.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered the membership commands and wired no state for \
                 them, and a door that accepts a member and admits nobody is worse than one \
                 that is not there",
            )
        })
    }

    pub fn with_share(mut self, share: Arc<crate::share_ops::ShareOps>) -> PlatformOps {
        self.share = Some(share);
        self
    }

    pub fn with_links(mut self, links: Arc<crate::share_links::ShareLinks>) -> PlatformOps {
        self.links = Some(links);
        self
    }

    fn link_ops(&self) -> Result<&crate::share_links::ShareLinks, ExecError> {
        self.links.as_deref().ok_or_else(|| {
            ExecError::new(
                ordo_api::ErrorCode::TenantRefused,
                "this deployment has no authentication configured (ORDO_AUTH_METHODS), so a \
                 share link has nothing to open it with",
            )
        })
    }

    pub fn with_observed_cost(
        mut self,
        ops: Arc<crate::observed_cost::ObservedCostOps>,
    ) -> PlatformOps {
        self.observed_cost = Some(ops);
        self
    }

    /// Wire `std.deployment.settings` (G7).
    pub fn with_settings(mut self, ops: Arc<crate::settings::SettingsOps>) -> PlatformOps {
        self.settings = Some(ops);
        self
    }

    /// Wire `std.deployment.tenants` (G4).
    pub fn with_tenant_list(mut self, ops: Arc<crate::fleet::TenantListOps>) -> PlatformOps {
        self.tenant_list = Some(ops);
        self
    }

    /// Wire `std.deployment.securityEvents` (G8).
    pub fn with_security_record(
        mut self,
        ops: Arc<crate::security_record::SecurityRecordOps>,
    ) -> PlatformOps {
        self.security_record = Some(ops);
        self
    }

    /// Wire `std.deployment.quotas` and `setQuota` (G9).
    pub fn with_quotas(mut self, ops: Arc<crate::quotas::QuotaOps>) -> PlatformOps {
        self.quotas = Some(ops);
        self
    }

    pub fn with_console_keys(
        mut self,
        ops: Arc<crate::console_keys::ConsoleKeyOps>,
    ) -> PlatformOps {
        self.console_keys = Some(ops);
        self
    }

    pub fn with_secrets(mut self, ops: Arc<crate::secrets::SecretOps>) -> PlatformOps {
        self.secrets = Some(ops);
        self
    }

    pub fn with_self_serve(mut self, ops: Arc<crate::self_serve::SelfServeOps>) -> PlatformOps {
        self.self_serve = Some(ops);
        self
    }

    pub fn with_guest(mut self, ops: Arc<crate::auth::guest::GuestOps>) -> PlatformOps {
        self.guest = Some(ops);
        self
    }

    pub fn with_contact(mut self, ops: Arc<crate::contact_email::ContactEmails>) -> PlatformOps {
        self.contact = Some(ops);
        self
    }

    pub fn with_session_policy(
        mut self,
        ops: Arc<crate::session_policy::SessionPolicies>,
    ) -> PlatformOps {
        self.session_policy = Some(ops);
        self
    }

    pub fn with_guess_limits(mut self, ops: Arc<crate::guess_limits::GuessLimits>) -> PlatformOps {
        self.guess_limits = Some(ops);
        self
    }

    pub fn with_invite(mut self, invite: Arc<crate::invite_ops::InviteOps>) -> PlatformOps {
        self.invite = Some(invite);
        self
    }

    fn invite_ops(&self) -> Result<&crate::invite_ops::InviteOps, ExecError> {
        self.invite.as_deref().ok_or_else(|| {
            ExecError::new(
                ordo_api::ErrorCode::TenantRefused,
                "this deployment has no authentication configured (ORDO_AUTH_METHODS), so an \
                 invitation has nobody to be redeemed by",
            )
        })
    }

    fn share_ops(&self) -> Result<&crate::share_ops::ShareOps, ExecError> {
        self.share.as_deref().ok_or_else(|| {
            ExecError::new(
                ordo_api::ErrorCode::TenantRefused,
                "this deployment has no authentication configured (ORDO_AUTH_METHODS), so a \
                 share has nobody to be redeemed by",
            )
        })
    }

    pub fn with_identities(
        mut self,
        identities: Arc<crate::tenant_identities::TenantIdentities>,
    ) -> PlatformOps {
        self.identities = Some(identities);
        self
    }

    pub fn with_identity(mut self, identity: Arc<crate::identity_ops::IdentityOps>) -> PlatformOps {
        self.identity = Some(identity);
        self
    }

    fn identity_ops(&self) -> Result<&crate::identity_ops::IdentityOps, ExecError> {
        self.identity.as_deref().ok_or_else(|| {
            ExecError::new(
                ordo_api::ErrorCode::TenantRefused,
                "this deployment has no credential recovery configured (it needs passkey \
                 authentication, ORDO_AUTH_METHODS)",
            )
        })
    }

    pub fn with_admission(mut self, admission: Arc<crate::admission::Admission>) -> PlatformOps {
        self.admission = Some(admission);
        self
    }

    pub fn with_origins(
        mut self,
        origins: Arc<crate::tenant_origins::TenantOriginMap>,
    ) -> PlatformOps {
        self.origins = Some(origins);
        self
    }

    fn admission_ops(&self) -> Result<&crate::admission::Admission, ExecError> {
        self.admission.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered the admission ops and wired no state for them",
            )
        })
    }

    pub fn with_credentials(
        mut self,
        credentials: Arc<crate::credential_ops::CredentialOps>,
    ) -> PlatformOps {
        self.credentials = Some(credentials);
        self
    }

    fn credential_ops(&self) -> Result<&crate::credential_ops::CredentialOps, ExecError> {
        self.credentials.as_deref().ok_or_else(|| {
            ExecError::new(
                ordo_api::ErrorCode::TenantRefused,
                "this deployment has no authentication configured (ORDO_AUTH_METHODS), so it \
                 holds no credentials for a tenant to give or reset",
            )
        })
    }

    pub fn with_reap(mut self, reap: Arc<crate::reap_op::ReapOps>) -> PlatformOps {
        self.reap = Some(reap);
        self
    }

    fn reap_ops(&self) -> Result<&crate::reap_op::ReapOps, ExecError> {
        self.reap.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered `std.space.reap` and wired no state for it — an \
                 op that releases resources and answers partially is worse than one that is \
                 not there",
            )
        })
    }

    fn tenant_ops(&self) -> Result<&crate::tenant_ops::TenantOps, ExecError> {
        self.tenants.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered `std.space.create` and wired no state for it — a \
                 door that accepts a tenant and creates nothing is worse than one that is not \
                 there",
            )
        })
    }

    fn apply_ops(&self) -> Result<&crate::apply_op::ApplyOps, ExecError> {
        self.apply.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered `std.deployment.apply` and wired no state for \
                 it — a deploy door that answers partially is worse than one that is not \
                 there",
            )
        })
    }

    fn exit(&self) -> Result<&crate::exit_ops::ExitOps, ExecError> {
        self.exit.as_deref().ok_or_else(|| {
            ExecError::internal(
                "this deployment registered an exit op and wired no state for it — the two \
                 go together, because an exit op that answers partially is worse than one \
                 that is not there",
            )
        })
    }

    /// Run a platform op. `caller` is the principal the door proved.
    pub async fn execute(
        &self,
        op: &OpSpec,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op.name.as_str() {
            CALLERS_OF => self.callers_of(space, bound, caller).await,
            MY_TENANTS => self.my_tenants(space, caller).await,
            crate::exit_ops::EXPORT => self.exit()?.export(space, bound, caller).await,
            crate::exit_ops::IMPORT => self.exit()?.import(space, bound, caller).await,
            // E7: with the identity settlement wired, a deletion is followed
            // by what it does to the people the tenant made. The erasure has
            // already happened by then, so a settlement that fails is
            // reported in the answer and never turns a completed erasure into
            // a refusal.
            crate::exit_ops::DELETE if self.identities.is_some() => {
                let mut out = self.exit()?.delete(space, bound, caller).await?;
                let settled = match &self.identities {
                    Some(ids) => match ids.after_delete(space, caller).await {
                        Ok(o) => json!({
                            "released": o.released,
                            "removed": o.removed,
                            "retired": o.retired,
                        }),
                        Err(e) => json!({ "error": e }),
                    },
                    None => serde_json::Value::Null,
                };
                if let Some(row) = out.rows.first_mut() {
                    row["identities"] = settled;
                }
                Ok(out)
            }
            crate::exit_ops::DELETE => self.exit()?.delete(space, bound, caller).await,
            crate::attest_ops::ATTEST_TO => self.attest()?.attest_to(space, bound, caller).await,
            crate::attest_ops::ATTESTATION => self.attest()?.attestation(space).await,
            crate::attest_ops::ATTESTATION_DEAD_LETTERS => self.attest()?.dead_letters(space).await,
            crate::attest_ops::CHAIN_PROOF => {
                self.attest()?.chain_proof(space, bound, caller).await
            }
            crate::bucket_ops::ATTACH_BUCKET => self.buckets()?.attach(space, bound, caller).await,
            crate::bucket_ops::DETACH_BUCKET => self.buckets()?.detach(space, caller).await,
            crate::apply_op::APPLY => self.apply_ops()?.apply(space, bound, caller).await,
            name if crate::deploy_ops::OPS.contains(&name) => {
                crate::deploy_ops::execute(self.apply_ops()?, name, space, bound, caller).await
            }
            crate::tenant_ops::CREATE => self.tenant_ops()?.create(space, bound, caller).await,
            crate::reap_op::REAP => self.reap_ops()?.reap(space, bound, caller).await,
            name if crate::pause::OPS.contains(&name) => {
                self.pause_ops()?.execute(name, space, bound, caller).await
            }
            // G5: who is in a tenant and what they may do.
            name if crate::people_ops::OPS.contains(&name) => {
                crate::people_ops::execute(self, name, space, bound, caller).await
            }
            name if crate::profile_ops::OPS.contains(&name) => {
                let ops = self.profiles.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered the profile ops and wired no state for them",
                    )
                })?;
                ops.execute(name, space, bound, caller).await
            }
            name if crate::release_ops::OPS.contains(&name) => {
                // A tenant keeps an owner who signs in without its own
                // providers (`plan-tenant-providers.md` §8).
                // The check and the write share the tenant's lock.
                let ops = self.release_ops()?;
                match self.tenant_connections.get().filter(|t| t.enabled) {
                    Some(tc) => {
                        let without = crate::tenant_providers::Without::Principal(caller);
                        tc.guarded(space, without, false, || {
                            ops.execute(name, space, bound, caller)
                        })
                        .await
                    }
                    None => ops.execute(name, space, bound, caller).await,
                }
            }
            name if crate::member_ops::OPS.contains(&name) => {
                let ops = self.member_ops()?;
                let tc = self.tenant_connections.get().filter(|t| t.enabled);
                let change = crate::tenant_providers::TenantConnections::member_change(name, bound);
                match (tc, change) {
                    (Some(tc), Some(without)) => {
                        tc.guarded(space, without, false, || {
                            ops.execute(name, space, bound, caller)
                        })
                        .await
                    }
                    _ => ops.execute(name, space, bound, caller).await,
                }
            }
            // A credential reset takes a member's passwords away.
            crate::credential_ops::RESET_CREDENTIAL
                if self.tenant_connections.get().is_some_and(|t| t.enabled) =>
            {
                let ops = self.credential_ops()?;
                let tc = self.tenant_connections.get().expect("checked just above");
                let member = bound
                    .0
                    .get("member")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .unwrap_or_default();
                let without = crate::tenant_providers::Without::MemberPasswords(member);
                tc.guarded(space, without, false, || {
                    ops.execute(op.name.as_str(), space, bound, caller)
                })
                .await
            }
            name if crate::tenant_providers::OPS.contains(&name) => {
                match self.tenant_connections.get() {
                    Some(tc) => tc.execute(name, space, bound, caller).await,
                    None => Err(ExecError::new(
                        ordo_api::ErrorCode::TenantRefused,
                        crate::tenant_providers::off_message(),
                    )),
                }
            }
            name if crate::consent::OPS.contains(&name) => {
                let ops = self.consent.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered the delegation ops and wired no state for them",
                    )
                })?;
                ops.execute(name, space, bound, caller).await
            }
            name if crate::share_ops::OPS.contains(&name) => {
                let engine = self.engine();
                self.share_ops()?
                    .execute(&engine, name, space, bound, caller)
                    .await
            }
            name if crate::share_links::OPS.contains(&name) => {
                let engine = self.engine();
                if name == crate::share_links::SET_PASSWORD_LIMITS {
                    // The app developer may have closed this to tenants.
                    let ispace = ordo_identity::principal::SpaceId(space.to_string());
                    let who = engine
                        .principal_of_member(&ispace, caller)
                        .cloned()
                        .unwrap_or_else(|| ordo_identity::PrincipalId(caller.to_string()));
                    let role = engine.standing(&ispace, &who).and_then(|(r, _)| r);
                    if let Some(why) = crate::session_policy::closed_by_template_for(
                        name,
                        &crate::access_template::current(),
                        role,
                    ) {
                        return Err(ExecError::new(ordo_api::ErrorCode::TenantRefused, why));
                    }
                }
                self.link_ops()?
                    .execute(&engine, name, space, bound, caller)
                    .await
            }
            name if crate::auth::guest::OPS.contains(&name) => {
                let ops = self.guest.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.setGuest and wired no state \
                         for it",
                    )
                })?;
                ops.execute(space, bound, caller).await
            }
            name if crate::contact_email::OPS.contains(&name) => {
                let ops = self.contact.as_deref().ok_or_else(|| {
                    ExecError::new(
                        ordo_api::ErrorCode::TenantRefused,
                        "an email requirement needs sign-in: this deployment has no \
                         ORDO_AUTH_METHODS",
                    )
                })?;
                ops.execute(name, space, bound, caller).await
            }
            name if crate::session_policy::OPS.contains(&name) => {
                let ops = self.session_policy.as_deref().ok_or_else(|| {
                    ExecError::new(
                        ordo_api::ErrorCode::TenantRefused,
                        "session settings need sign-in: this deployment has no ORDO_AUTH_METHODS",
                    )
                })?;
                if name == crate::session_policy::SET_TENANT {
                    // The app developer may have closed this to tenants.
                    let engine = self.engine();
                    let ispace = ordo_identity::principal::SpaceId(space.to_string());
                    let who = engine
                        .principal_of_member(&ispace, caller)
                        .cloned()
                        .unwrap_or_else(|| ordo_identity::PrincipalId(caller.to_string()));
                    let role = engine.standing(&ispace, &who).and_then(|(r, _)| r);
                    if let Some(why) = crate::session_policy::closed_by_template(
                        &crate::access_template::current(),
                        role,
                    ) {
                        return Err(ExecError::new(ordo_api::ErrorCode::TenantRefused, why));
                    }
                }
                ops.execute(name, space, bound, caller).await
            }
            name if crate::guess_limits::OPS.contains(&name) => {
                let ops = self.guess_limits.as_deref().ok_or_else(|| {
                    ExecError::new(
                        ordo_api::ErrorCode::TenantRefused,
                        "the password slowdown needs sign-in: this deployment has no \
                         ORDO_AUTH_METHODS",
                    )
                })?;
                if name == crate::guess_limits::SET_TENANT {
                    // The app developer may have closed this to tenants.
                    let engine = self.engine();
                    let ispace = ordo_identity::principal::SpaceId(space.to_string());
                    let who = engine
                        .principal_of_member(&ispace, caller)
                        .cloned()
                        .unwrap_or_else(|| ordo_identity::PrincipalId(caller.to_string()));
                    let role = engine.standing(&ispace, &who).and_then(|(r, _)| r);
                    if let Some(why) = crate::session_policy::closed_by_template_for(
                        name,
                        &crate::access_template::current(),
                        role,
                    ) {
                        return Err(ExecError::new(ordo_api::ErrorCode::TenantRefused, why));
                    }
                }
                ops.execute(name, space, bound, caller).await
            }
            name if crate::observed_cost::OPS.contains(&name) => {
                let ops = self.observed_cost.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.observedCost and wired no \
                         state for it",
                    )
                })?;
                ops.execute(space, bound)
            }
            name if crate::settings::OPS.contains(&name) => {
                let ops = self.settings.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.settings and wired no state \
                         for it",
                    )
                })?;
                ops.execute(space, bound).await
            }
            name if crate::security_record::OPS.contains(&name) => {
                let ops = self.security_record.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.securityEvents and wired no \
                         state for it",
                    )
                })?;
                ops.execute(space, bound).await
            }
            name if crate::fleet::OPS.contains(&name) => {
                let ops = self.tenant_list.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.tenants and wired no state \
                         for it",
                    )
                })?;
                let engine = self.engine();
                ops.execute(&engine, space, bound, caller).await
            }
            name if crate::quotas::OPS.contains(&name) => {
                let ops = self.quotas.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered the quota ops and wired no state for them",
                    )
                })?;
                ops.execute(name, space, bound, caller).await
            }
            name if crate::console_keys::OPS.contains(&name) => {
                let ops = self.console_keys.as_deref().ok_or_else(|| {
                    ExecError::new(
                        ordo_api::ErrorCode::TenantRefused,
                        "console keys need sign-in: this deployment has no ORDO_AUTH_METHODS",
                    )
                })?;
                // The door hands `$caller` as the member id in this space.
                // A key's owner is the principal: the console session it
                // mints names the principal, and every op and step-up
                // matches on that.
                let engine = self.engine();
                let principal = engine
                    .principal_of_member(&SpaceId(space.to_string()), caller)
                    .map(|p| p.0.clone())
                    .unwrap_or_else(|| caller.to_string());
                ops.execute(name, space, bound, &principal).await
            }
            name if crate::secrets::OPS.contains(&name) => {
                let ops = self.secrets.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered the secret ops and wired no state for them",
                    )
                })?;
                // `$caller` is the member id in this space; the record
                // names the principal, as console keys do.
                let engine = self.engine();
                let principal = engine
                    .principal_of_member(&SpaceId(space.to_string()), caller)
                    .map(|p| p.0.clone())
                    .unwrap_or_else(|| caller.to_string());
                ops.execute(name, space, bound, &principal).await
            }
            name if crate::self_serve::OPS.contains(&name) => {
                let ops = self.self_serve.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered std.deployment.setSelfServe and wired no state \
                         for it",
                    )
                })?;
                ops.execute(space, bound, caller).await
            }
            name if crate::invite_ops::OPS.contains(&name) => {
                let engine = self.engine();
                self.invite_ops()?
                    .execute(&engine, name, space, bound, caller)
            }
            name if crate::credential_ops::OPS.contains(&name) => {
                self.credential_ops()?
                    .execute(name, space, bound, caller)
                    .await
            }
            name if crate::identity_ops::OPS.contains(&name) => {
                self.identity_ops()?
                    .execute(name, space, bound, caller)
                    .await
            }
            name if name == crate::admission::SET_ADMISSION
                || name == crate::admission::ADMISSION =>
            {
                self.admission_ops()?
                    .execute(name, space, bound, caller)
                    .await
            }
            name if crate::tenant_origins::OPS.contains(&name) => {
                let origins = self.origins.as_deref().ok_or_else(|| {
                    ExecError::internal(
                        "this deployment registered the origins ops and wired no state for them",
                    )
                })?;
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                origins.execute(name, space, bound, caller, at).await
            }
            other => Err(ExecError::internal(format!(
                "`{other}` is registered as a platform op and has no handler"
            ))),
        }
    }

    /// `std.person.tenants`. The person is the caller the door proved; there
    /// is no parameter, so no other person can be asked about. Read from the
    /// engine the door decides with, so a suspended or removed membership is
    /// as fresh here as it is at the door.
    async fn my_tenants(&self, space: &str, caller: &str) -> Result<ExecResult, ExecError> {
        let engine = self.engine();
        let asked_in = SpaceId(space.to_string());
        let person = engine
            .principal_of_member(&asked_in, caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));
        let mut rows = Vec::new();
        for (member_of, role, status) in engine.spaces_of(&person) {
            // A tenant's name is the creator's label and lives in the
            // control database. A deployment wired without it, and a space
            // the deployment did not mint (its own home), answer null.
            let tenant = match &self.tenants {
                Some(t) => crate::tenants::lookup(&t.pool, &member_of.0)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let role = match role {
                Some(ordo_identity::principal::Role::Owner) => Some("owner"),
                Some(ordo_identity::principal::Role::Manager) => Some("manager"),
                Some(ordo_identity::principal::Role::Editor) => Some("editor"),
                Some(ordo_identity::principal::Role::Viewer) => Some("viewer"),
                // A guest seat: no role (`plan-sharing.md` §3.1, G6).
                None => None,
            };
            let status = match status {
                ordo_identity::principal::MembershipStatus::Active => "active",
                ordo_identity::principal::MembershipStatus::Suspended => "suspended",
            };
            let mut row = json!({
                "space": member_of.0,
                "name": tenant.as_ref().map(|t| t.name.clone()),
                "role": role,
                "status": status,
            });
            // 27ar: whether the tenant itself is paused, and how. The layer
            // outside a paused tenant is where a person learns it.
            crate::pause::list_state(&mut row, tenant.as_ref());
            rows.push(row);
        }
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }

    async fn callers_of(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let target = bound
            .0
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::internal("`$op` did not reach the platform op"))?;
        let space = SpaceId(space.to_string());
        // `caller` is the asker's member id in this space (E1); the engine
        // is asked about the person. A caller with no membership here (a
        // capability token) is asked about as it arrived.
        // One snapshot for the resolution, the gate and the answer, so a
        // swap landing between them cannot have them read different engines.
        let engine = self.engine();
        let asker = engine
            .principal_of_member(&space, caller)
            .cloned()
            .unwrap_or_else(|| PrincipalId(caller.to_string()));

        // The enumeration rule. An op outside the asker's own scoped catalog
        // and an op nobody registered are ONE answer, so the parameter
        // enumerates nothing — the door's non-enumerability rule, applied to
        // an argument instead of a path. `membership_reaches` is the same
        // predicate `callers_of` reports with, so the gate and the answer
        // cannot come to disagree about who is included.
        if !engine.membership_reaches(&space, &asker, target) {
            return Err(ExecError::new(
                ordo_api::ErrorCode::NotFound,
                "no such op, or one you may not call — which answer this is, \
                 is deliberately not disclosed",
            ));
        }

        let now = Timestamp((self.now)());
        // The token column. A registry that cannot answer is a REFUSAL, not
        // an empty column: rule 4's exhaustiveness is the property, and an
        // answer missing its tokens while looking complete is what kept this
        // op unbuilt.
        let records = self
            .grants
            .callers_of(&space, target, now)
            .await
            .map_err(|e| {
                ExecError::internal(format!(
                    "the grant registry could not answer, so `callersOf` would have been \
                     exhaustive only in the role columns; refusing rather than answering \
                     partially: {e}"
                ))
            })?;

        // The delegation column, on the same terms as the token one: a
        // registry that cannot answer is a REFUSAL, not an empty column. An
        // answer that quietly omitted everyone acting as somebody else while
        // looking complete is the exact failure rule 4 forbids, and it would
        // omit the callers hardest to notice by any other means.
        let delegations = match &self.delegations {
            Some(reg) => reg.live_in(&space, now).await.map_err(|e| {
                ExecError::internal(format!(
                    "the delegation registry could not answer, so `callersOf` would have \
                     omitted every caller acting on somebody else's behalf; refusing rather \
                     than answering partially: {e}"
                ))
            })?,
            None => Vec::new(),
        };

        let rows = engine
            .callers_of(&space, target, &records, &delegations, now)
            .into_iter()
            .map(|c| {
                json!({
                    "principal": c.principal.0,
                    "via": c.via.kind(),
                    "detail": c.via.detail(),
                })
            })
            .collect();
        Ok(ExecResult {
            position: None,
            rows,
            next_cursor: None,
        })
    }
}

/// The executor decorator that serves platform ops and delegates the rest.
///
/// The same shape `crate::metrics::MeteredExecutor` and `MeteredKeys` take,
/// for the same reason: the door's boundary is a small set of traits, and a
/// new capability that fits one of them should wrap it rather than widen it.
/// A deployment assembles `MeteredExecutor::new(PlatformExecutor::new(real))`.
/// The order does not matter to correctness; it decides whether a platform
/// op's own duration is timed, which it should be, so the meter goes
/// outside. Inside, it would only ever see the ops this executor delegates.
pub struct PlatformExecutor {
    pub inner: Arc<dyn OpExecutor>,
    pub ops: Arc<PlatformOps>,
}

impl PlatformExecutor {
    pub fn new(inner: Arc<dyn OpExecutor>, ops: Arc<PlatformOps>) -> PlatformExecutor {
        PlatformExecutor { inner, ops }
    }
}

impl OpExecutor for PlatformExecutor {
    fn execute<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ReplayLease>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        self.execute_sized(op, space, bound, refine, page, lease, None)
    }

    fn execute_sized<'a>(
        &'a self,
        op: &'a OpSpec,
        space: &'a str,
        bound: BoundParams,
        refine: Option<ordo_api::Refinement>,
        page: Option<ordo_api::PageCursor>,
        lease: Option<ReplayLease>,
        page_rows: Option<usize>,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecError>> {
        // 27ar: the pause gate, for every op, app and platform alike. Here
        // rather than at the door so the access check has already answered
        // `access_denied` to anyone with no membership, and a pause tells
        // them nothing.
        if let Some(pause) = &self.ops.pause {
            if let Err(e) = crate::pause::gate(&pause.spaces, &op.name, space) {
                return Box::pin(async move { Err(e) });
            }
        }
        if !is_platform_op(&op.name) {
            return self
                .inner
                .execute_sized(op, space, bound, refine, page, lease, page_rows);
        }
        Box::pin(async move {
            // `$caller` is router-inserted from the access decision — a
            // reserved key no op parameter can shadow (R-G13). Its absence
            // is a bug in the door, never a caller's doing.
            let caller = bound
                .0
                .get("$caller")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ExecError::internal("no principal reached the platform op"))?
                .to_string();
            self.ops
                .execute(op, space, &bound, &caller)
                .await
                .map(ExecOutcome::Ran)
        })
    }
}
