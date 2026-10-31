//! The policy table: what every cell of the matrix expects, in one place.
//!
//! Three parts.
//!
//! 1. **Who reaches which op** ([`op_class`], [`expect_op`]): every op the
//!    door may serve, sorted into a class, and what each kind of actor gets
//!    from the door for each class. The classes follow `howto-roles.md` §1,
//!    `std-library.md` §5, `howto-sharing.md`, `howto-delegation.md`,
//!    `plan-console.md` §8 and `howto-pause.md`.
//! 2. **What each mounted route answers** ([`route_rule`]): every route
//!    `runtime::door` serves, other than the op door, with the answer a
//!    probe gets from it.
//! 3. **Rows** (`authz_matrix.rs`, `ROWS`): one request each, with an id,
//!    built from actors and objects, and the safe answer. Empty for now: the
//!    extension point for replayed catalog entries.
//!
//! A cell whose real answer is a defect is marked [`Mark::KnownDefect`] with
//! the defect's id ([`DEFECTS`]), so the suite still passes and the list of
//! defects is the report. A cell no document decides is
//! [`Mark::Unspecified`] with the question ([`QUESTIONS`]). Either mark on a
//! cell whose answer is the expected one is itself a surprise: the defect
//! is fixed, or the question answered, and the table must say so.

use super::{routes, Answer, HOME};

/// The answer a cell expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Past every check the cell asks about: a 2xx, or a refusal about the
    /// request's own parameters or body (the probe's unknown parameter),
    /// which the door gives only after it let the caller in.
    Pass,
    /// The door's one refusal: `403 access_denied`.
    Denied,
    /// `401 authentication_failed`, the sign-in routes' one refusal.
    Unauth,
    /// This status and code exactly. An empty code matches any.
    Code(u16, &'static str),
    /// `200` with exactly this many rows.
    Rows(usize),
    /// Nothing about the object: `200` with no rows, or `404 not_found`, or
    /// `403 access_guard`.
    Nothing,
    /// Refused, by any 4xx other than a parameter refusal, or the
    /// connection closed (status 0). Never a 2xx and never a 5xx.
    Refused,
    /// A presence-gated route reached by a person who was let in: either
    /// served (2xx), stopped at its own confirm or input check (400), or
    /// asked for a fresh touch (403 `presence_required` /
    /// `reauthentication_required`). Never `access_denied`, `401` or `5xx`.
    /// Which of the three it is depends on how fresh the sign-in presence
    /// is when the cell runs, so all three are safe.
    Admitted,
}

/// Parameter and body refusals: the caller got in, the request was wrong.
const PARAM_CODES: &[&str] = &[
    "unknown_param",
    "missing_param",
    "invalid_param",
    "invalid_request",
];

impl Expect {
    pub fn holds(self, a: &Answer) -> bool {
        let code = a.code();
        match self {
            Expect::Pass => {
                (200..300).contains(&a.status)
                    || (a.status == 400 && PARAM_CODES.contains(&code.as_str()))
            }
            Expect::Denied => a.status == 403 && code == "access_denied",
            Expect::Unauth => a.status == 401 && code == "authentication_failed",
            Expect::Code(s, c) => a.status == s && (c.is_empty() || code == c),
            Expect::Rows(n) => {
                a.status == 200 && a.json()["rows"].as_array().map(Vec::len) == Some(n)
            }
            Expect::Nothing => {
                (a.status == 200 && a.json()["rows"].as_array().is_some_and(Vec::is_empty))
                    || (a.status == 404 && code == "not_found")
                    || (a.status == 403 && code == "access_guard")
            }
            Expect::Refused => {
                a.status == 0
                    || ((400..500).contains(&a.status)
                        && !(a.status == 400 && PARAM_CODES.contains(&code.as_str())))
            }
            Expect::Admitted => {
                (200..300).contains(&a.status)
                    || a.status == 400
                    || (a.status == 403
                        && ["presence_required", "reauthentication_required"]
                            .contains(&code.as_str()))
            }
        }
    }
}

/// How a cell's answer is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The expectation is the documented behaviour, and the door keeps it.
    Holds,
    /// The door does not keep it: a defect, by id in [`DEFECTS`]. The cell
    /// passes while the defect is there and is a surprise once it is fixed.
    KnownDefect(&'static str),
    /// No document decides; the question is in [`QUESTIONS`]. Any answer
    /// passes and is listed.
    Unspecified(&'static str),
}

/// Every defect the matrix knows, by id: what is wrong, in one line.
pub const DEFECTS: &[(&str, &str)] = &[];

/// Every question the matrix could not settle from a document.
pub const QUESTIONS: &[(&str, &str)] = &[];

/// The actor kinds the op rules speak of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No token, or one the door must refuse: expired, forged, garbled,
    /// signed out, revoked, replayed, an expired or revoked link.
    Dead,
    /// A person in a role, in their own tenant. `fresh` is a passkey touch
    /// within the step-up window.
    Role { role: Role, fresh: bool },
    /// A share window or a share link: the shared ops only.
    Grant,
    /// The delegated agent for member A.
    Agent,
    /// A person the identity service signed in, in the home space or a
    /// tenant (`plan-identity-service.md` 4.3). `fresh` is a touch inside
    /// the window.
    Deployment { fresh: bool },
    /// A machine key's deployment token: no touch, no tenant.
    Machine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Editor,
    Manager,
    Owner,
}

/// Every actor `actors::build` makes, and its kind.
pub const ACTORS: &[(&str, Kind)] = &[
    ("anonymous", Kind::Dead),
    ("expired", Kind::Dead),
    ("foreign_key", Kind::Dead),
    ("garbled", Kind::Dead),
    ("logged_out", Kind::Dead),
    ("revoked", Kind::Dead),
    ("replayed", Kind::Dead),
    ("link_expired", Kind::Dead),
    ("link_revoked", Kind::Dead),
    ("guest", Kind::Grant),
    ("link", Kind::Grant),
    ("link_password", Kind::Grant),
    ("link_once", Kind::Grant),
    ("agent", Kind::Agent),
    (
        "viewer",
        Kind::Role {
            role: Role::Viewer,
            fresh: true,
        },
    ),
    (
        "editor",
        Kind::Role {
            role: Role::Editor,
            fresh: true,
        },
    ),
    (
        "member_a",
        Kind::Role {
            role: Role::Manager,
            fresh: true,
        },
    ),
    (
        "member_b",
        Kind::Role {
            role: Role::Manager,
            fresh: true,
        },
    ),
    (
        "manager",
        Kind::Role {
            role: Role::Manager,
            fresh: true,
        },
    ),
    (
        "manager_stale",
        Kind::Role {
            role: Role::Manager,
            fresh: false,
        },
    ),
    (
        "owner",
        Kind::Role {
            role: Role::Owner,
            fresh: true,
        },
    ),
    (
        "owner_stale",
        Kind::Role {
            role: Role::Owner,
            fresh: false,
        },
    ),
    (
        "t2_owner",
        Kind::Role {
            role: Role::Owner,
            fresh: true,
        },
    ),
    (
        "deployment_owner",
        Kind::Role {
            role: Role::Owner,
            fresh: true,
        },
    ),
    // No console key actors: the matrix's door verifies the identity
    // service's tokens (`ORDO_IDENTITY_URL` set), so a console key does
    // not sign in (`plan-identity-service.md` 6; `actors.rs` proves it).
    // The identity service's tokens (`authz_support/identity.rs`).
    ("deployment_admin", Kind::Deployment { fresh: true }),
    ("deployment_admin_stale", Kind::Deployment { fresh: false }),
    ("machine_key", Kind::Machine),
    ("deployment_expired", Kind::Dead),
    ("deployment_wrong_aud", Kind::Dead),
    ("deployment_wrong_iss", Kind::Dead),
    ("deployment_unknown_kid", Kind::Dead),
    ("deployment_foreign_signer", Kind::Dead),
];

/// The actors whose op cells run in the home space and in a tenant, since
/// the realm table answers differently in each.
pub fn in_both_spaces(actor: &str) -> bool {
    actor.starts_with("deployment_") && actor != "deployment_owner" || actor == "machine_key"
}

pub fn kind_of(actor: &str) -> Kind {
    ACTORS
        .iter()
        .find(|(a, _)| *a == actor)
        .map(|(_, k)| *k)
        .unwrap_or_else(|| panic!("the policy has no kind for actor `{actor}`"))
}

/// The ops the share and every share link list (`actors::build`).
pub const SHARED_OPS: &[&str] = &["myNotes", "noteById", "std.notes.get"];
/// The ops the agent's delegation grants. `deleteNote` was asked for and is
/// not granted: a delete is not delegable while `ORDO_DELEGABLE_OPS` is
/// unset (`howto-delegation.md` §2).
pub const DELEGATED_OPS: &[&str] = &["myNotes", "noteById", "createNote", "editNote"];

/// What kind of op, for the door.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpClass {
    /// An authored read: every role (`howto-roles.md` §1).
    Read,
    /// An authored write or delete: editor and above.
    Write,
    /// The derived `std.<entity>.*`: manager and owner (`std-library.md` §5).
    Std,
    /// A person's own answer: every role, no touch.
    Person,
    /// Any member, every role (leaving, the consent ceremony). `presence`:
    /// a write that asks for a touch.
    AnyMember { presence: bool },
    /// Administration of a tenant: manager and owner.
    Admin,
    /// The tenant owner alone.
    OwnerOnly,
    /// The deployment's own: the door lets an owner of any space through
    /// (the role profile is deployment-wide, `lib.rs` §8r) and the op
    /// answers only in the deployment's own space. The rows in `rows.rs`
    /// call the reads for real in a tenant.
    Deployment,
    /// Served to nobody here: not wired in this configuration (exit,
    /// import, attestation, buckets) or closed to every role until the app
    /// developer names one (`ORDO_TENANT_PAUSE_ROLES`, `howto-pause.md`).
    Nobody,
}

/// Every op, by class. An op the door may serve that is not here fails the
/// guard (`op_class` answers `None`).
pub fn op_class(name: &str) -> Option<OpClass> {
    use OpClass::*;
    if let Some(entity_op) = name.strip_prefix("std.") {
        let entity = entity_op.split('.').next().unwrap_or("");
        if ["notes", "files", "posters", "items", "notesAccess"].contains(&entity) {
            return Some(Std);
        }
    }
    let class = match name {
        "myNotes" | "noteById" | "notesMatching" | "myFiles" | "posterList" | "priceAtMost"
        | "topN" | "allItems" => Read,
        "createNote"
        | "editNote"
        | "deleteNote"
        | "addFile"
        | "createItem"
        | "addPoster"
        | "grantNotesAccess"
        | "revokeNotesAccess"
        | "setNotesAccessLevel" => Write,
        "std.person.tenants" => Person,
        "std.space.leave" => AnyMember { presence: true },
        "std.delegation.review" | "std.delegation.mine" => AnyMember { presence: false },
        "std.delegation.approve" => AnyMember { presence: true },
        "std.delegation.decline" | "std.delegation.revoke" => AnyMember { presence: false },
        // Owner only.
        "std.space.addConnection"
        | "std.space.updateConnection"
        | "std.space.retireConnection"
        | "std.space.connections"
        | "std.space.admission"
        | "std.space.setAdmission"
        | "std.space.admit"
        | "std.space.setRole"
        | "std.space.suspend"
        | "std.space.reinstate"
        | "std.space.remove"
        | "std.space.createPerson"
        | "std.space.resetCredential"
        | "std.space.setTemporaryPassword"
        | "std.space.origins"
        | "std.space.setOrigins"
        | "std.space.setRecoveryPolicy"
        | "std.space.succeedOwner"
        | "std.space.setSessionPolicy"
        | "std.space.setShareLinkLifetime"
        | "std.space.setLinkPasswordLimits"
        | "std.space.setPasswordSlowdown"
        | "std.space.create"
        | "std.space.reap" => OwnerOnly,
        // Manager and owner.
        "std.space.addToGroup"
        | "std.space.removeFromGroup"
        | "std.space.createGroup"
        | "std.space.deleteGroup"
        | "std.space.groups"
        | "std.space.groupMembers"
        | "std.space.profile"
        | "std.space.setOpAccess"
        | "std.space.resetOpAccess"
        | "std.space.setMemberAccess"
        | "std.space.callableBy"
        | "std.space.callersOf"
        | "std.space.grants"
        | "std.space.members"
        | "std.space.invite"
        | "std.space.invitations"
        | "std.space.revokeInvitation"
        | "std.space.share"
        | "std.space.shares"
        | "std.space.revokeShare"
        | "std.space.shareLink"
        | "std.space.links"
        | "std.space.revokeLink"
        | "std.space.setLinkPassword"
        | "std.space.setLinkMaxOpens"
        | "std.space.sessionPolicy"
        | "std.space.approveMemberRecovery"
        | "std.space.recoverMember"
        | "std.space.objectToCase" => Admin,
        // Not wired here, or closed by default.
        "std.space.attachBucket"
        | "std.space.detachBucket"
        | "std.space.attestTo"
        | "std.space.attestation"
        | "std.space.attestationDeadLetters"
        | "std.space.chainProof"
        | "std.space.export"
        | "std.space.import"
        | "std.space.delete"
        | "std.space.download"
        | "std.space.pause"
        | "std.space.resume" => Nobody,
        n if n.starts_with("std.deployment.") || n.starts_with("std.identity.") => Deployment,
        _ => return None,
    };
    Some(class)
}

/// A platform write that asks for a fresh touch (`presence.rs`: every
/// platform op that changes something, unless `ORDO_STEP_UP_EXEMPT_OPS`
/// names it, and it is unset here).
fn needs_presence(class: OpClass, kind: ordo_api::OpKind) -> bool {
    let write = kind != ordo_api::OpKind::Read;
    match class {
        OpClass::AnyMember { presence } => presence,
        OpClass::Admin | OpClass::OwnerOnly | OpClass::Deployment => write,
        _ => false,
    }
}

fn role_reaches(class: OpClass, role: Role) -> bool {
    match class {
        OpClass::Read | OpClass::Person | OpClass::AnyMember { .. } => true,
        OpClass::Write => role >= Role::Editor,
        OpClass::Std | OpClass::Admin => role >= Role::Manager,
        OpClass::OwnerOnly | OpClass::Deployment => role == Role::Owner,
        OpClass::Nobody => false,
    }
}

/// The door's answer to a probe of `op` (an unknown parameter, so nothing
/// runs) by `actor` in its own space.
pub fn expect_op(name: &str, kind: ordo_api::OpKind, actor: &str) -> (Expect, Mark) {
    expect_op_in(name, kind, actor, HOME)
}

/// The ops no caller reaches in this configuration because they are not
/// registered: the op door's uniform denial comes before any access check.
const NOT_SERVED_HERE: &[&str] = &[
    "std.space.attachBucket",
    "std.space.detachBucket",
    "std.space.attestTo",
    "std.space.attestation",
    "std.space.attestationDeadLetters",
    "std.space.chainProof",
    "std.space.export",
    "std.space.import",
    "std.space.delete",
];

/// The realm table (`plan-identity-service.md` 4.3) for a deployment
/// token on `name`, probed in the home space or in a tenant.
///
/// The tenant ops come from the product's one list,
/// `console_keys::TENANT_OPS_OPEN`, so narrowing it changes this table
/// with it. A platform op is one the build's `platform_ops::specs()` lists.
fn deployment_op(name: &str, kind: ordo_api::OpKind, who: Kind, home: bool) -> Expect {
    // A platform surface outside the catalog (`platform.security.events`)
    // has no class; it is a read, so no class rule applies to it.
    let class = op_class(name).unwrap_or(OpClass::Deployment);
    if !routes::platform_op_names().contains(name) {
        return Expect::Denied;
    }
    // Registered nowhere in this configuration (exit, import, attestation,
    // buckets): the op door refuses every caller before any access check,
    // so the realm is never asked. A stale person gets `access_denied`,
    // not `presence_required`, which proves it.
    if NOT_SERVED_HERE.contains(&name) {
        return Expect::Denied;
    }
    let (person, fresh) = match who {
        Kind::Deployment { fresh } => (true, fresh),
        _ => (false, false),
    };
    let presence = Expect::Code(403, "presence_required");
    if home {
        // A write the touch gate names asks a stale person for a touch.
        let gated = kind != ordo_api::OpKind::Read
            && !matches!(class, OpClass::AnyMember { presence: false });
        return if person && !fresh && gated {
            presence
        } else {
            Expect::Pass
        };
    }
    if !person || !ordo_host::console_keys::TENANT_OPS_OPEN.contains(&name) {
        return Expect::Denied;
    }
    if fresh {
        Expect::Pass
    } else {
        presence
    }
}

/// [`expect_op`] for a probe sent in `space`.
pub fn expect_op_in(
    name: &str,
    kind: ordo_api::OpKind,
    actor: &str,
    space: &str,
) -> (Expect, Mark) {
    let class = op_class(name).expect("the guard checked every op has a class");
    let holds = |e| (e, Mark::Holds);
    match kind_of(actor) {
        k @ (Kind::Deployment { .. } | Kind::Machine) => {
            holds(deployment_op(name, kind, k, space == HOME))
        }
        Kind::Dead => holds(Expect::Denied),
        Kind::Grant => holds(if SHARED_OPS.contains(&name) {
            Expect::Pass
        } else {
            Expect::Denied
        }),
        Kind::Agent => holds(if DELEGATED_OPS.contains(&name) {
            Expect::Pass
        } else {
            Expect::Denied
        }),
        Kind::Role { role, fresh } => {
            if !role_reaches(class, role) {
                return holds(Expect::Denied);
            }
            if !fresh && needs_presence(class, kind) {
                return holds(Expect::Denied);
            }
            holds(Expect::Pass)
        }
    }
}

/// What a non-op route answers a probe: by actor kind.
#[derive(Clone, Copy, Debug)]
pub enum RouteRule {
    /// The same answer to every caller, token or none. `/auth/*` routes sit
    /// under the console gate, so a console session is `Unauth` there.
    Open(Expect),
    /// A person's session gets `pass`; every other caller `Unauth`. The
    /// console gate refuses a console session too.
    Session(Expect),
    /// A changing route that needs a fresh passkey touch: a person without
    /// one is refused (any 4xx; the code is `presence_required` or
    /// `reauthentication_required` by route), and a non-person is `Unauth`.
    /// The matrix's live persons carry sign-in presence, which the op path
    /// accepts and these routes do not, so this is the stable answer.
    NeedsPresence,
    /// The deployment owner in the deployment's own space gets `pass`;
    /// everybody else `Denied` (`lib.rs`: the deployment's own surfaces).
    DeploymentOwner(Expect),
    /// A door route; the probe's expectation is computed per actor in
    /// [`expect_route`].
    Door,
}

/// Every route `runtime::door` mounts other than `/op/{name}`, with what a
/// probe of it expects. A mounted route missing here fails the guard.
pub fn route_rule(method: &str, path: &str) -> Option<RouteRule> {
    use RouteRule::*;
    let u = Expect::Unauth;
    let ok = Expect::Code(200, "");
    Some(match (method, path) {
        ("GET", "/health/live" | "/health/ready" | "/health/deploy") => Open(ok),
        ("GET", "/.well-known/ordo-catalog" | "/catalog") => Open(ok),
        ("GET", "/.well-known/ordo-interface" | "/interface") => Open(ok),
        // No related origins are set, so there is no document to serve.
        ("GET", "/.well-known/webauthn") => Open(Expect::Code(404, "")),
        ("GET", "/auth/methods") => Open(ok),
        ("GET", "/auth/callback/{connection}" | "/auth/callback/t/{tenant}/{connection}") => {
            Open(u)
        }
        // A probe body is no ceremony: the one sign-in refusal, to anyone.
        (
            "POST",
            "/auth/begin"
            | "/auth/complete"
            | "/auth/register"
            | "/auth/password/redeem"
            | "/auth/password/change"
            | "/auth/step-up"
            | "/auth/recover/begin"
            | "/auth/recover/complete"
            | "/auth/recover/code"
            | "/auth/recover/contest"
            | "/auth/authenticators/link"
            | "/auth/authenticators/unlink"
            | "/auth/claim/begin"
            | "/auth/claim/complete"
            | "/auth/claim/passkey"
            | "/auth/second-step"
            | "/auth/second-step/begin"
            | "/auth/factors/key/complete"
            | "/auth/factors/totp/confirm"
            | "/auth/factors/policy"
            | "/auth/handoff"
            | "/auth/handoff/redeem",
        ) => Open(u),
        // A fresh touch is required; a signed-in person without one is
        // refused, not served.
        (
            "POST",
            "/auth/account/delete"
            | "/auth/email/remove"
            | "/auth/recovery-codes"
            | "/auth/factors/totp/begin"
            | "/auth/factors/key/begin",
        ) => NeedsPresence,
        // A bogus factor id or recovery acknowledgement: refused for
        // everyone, by 401 or, for a freshly present person, a presence or
        // confirm refusal. Never served, never `access_denied`.
        ("POST", "/auth/recover/ack" | "/auth/factors/{id}/remove") => Open(Expect::Refused),
        ("POST", "/auth/delegation/request" | "/auth/delegation/redeem") => {
            Open(Expect::Code(400, "invalid_request"))
        }
        ("POST", "/auth/share/peek" | "/auth/share/decline") => {
            Open(Expect::Code(403, "share_refused"))
        }
        ("POST", "/auth/invitation/peek" | "/auth/invitation/decline") => {
            Open(Expect::Code(403, "invitation_refused"))
        }
        ("POST", "/auth/link") => Open(Expect::Code(403, "link_refused")),
        // A person's own.
        (
            "GET",
            "/auth/authenticators"
            | "/auth/email"
            | "/auth/factors"
            | "/auth/identities"
            | "/auth/provisioning"
            | "/auth/spaces",
        ) => Session(ok),
        (
            "POST",
            "/auth/authenticators/link/begin"
            | "/auth/provisioning"
            | "/auth/refresh"
            | "/auth/step-up/begin",
        ) => Session(ok),
        // Mail is on in the world, so the contact-address routes are
        // mounted for real. The probe's body is not an address or a code,
        // and the sign-in routes' one refusal answers it.
        ("POST", "/auth/email/begin" | "/auth/email/complete") => Session(Expect::Unauth),
        ("POST", "/auth/share/accept" | "/auth/share/leave" | "/auth/window") => {
            Session(Expect::Code(403, "share_refused"))
        }
        // Spent by the probe; run by the dead actors only, and as rows.
        ("POST", "/auth/logout" | "/auth/logout-all") => Session(ok),
        ("GET", "/health/deep" | "/metrics" | "/security/alerts") => DeploymentOwner(ok),
        ("GET", "/usage") => DeploymentOwner(Expect::Code(400, "invalid_param")),
        ("GET", "/security/events") => Door,
        ("GET", "/download") => Door,
        ("GET", "/live") => Door,
        ("POST", "/live/{stream}") => Door,
        ("DELETE", "/live/{stream}/{key}") => Door,
        ("GET", "/blob/{handle}" | "/blob/{handle}/meta") => Door,
        ("PUT", "/blob/{entity}/{field}/{id}") => Door,
        ("ANY", "/op/{name}") => Door,
        // A function name the program does not declare (the matrix's
        // program declares none): the door's uniform denial to every
        // caller, as an unknown op gets, before any access check
        // (`router.rs`, `fn_dispatch`). A declared function's own gate is
        // the op door's, proved in `tests/functions.rs`.
        ("ANY", "/fn/{name}") => Open(Expect::Denied),
        _ => return None,
    })
}

/// A probe's expectation for `actor` on a route.
pub fn expect_route(method: &str, path: &str, actor: &str) -> (Expect, Mark) {
    let rule = route_rule(method, path).expect("the guard checked every route has a rule");
    let kind = kind_of(actor);
    let auth = path.starts_with("/auth/") || path == "/.well-known/webauthn";
    let person = matches!(kind, Kind::Role { .. });
    let holds = |e| (e, Mark::Holds);
    // The door's `/auth/*` routes refuse a deployment token: it has no app
    // session to manage (plan 4.3).
    if auth && in_both_spaces(actor) {
        return holds(Expect::Unauth);
    }
    match rule {
        RouteRule::Open(e) => holds(e),
        RouteRule::Session(e) => {
            // Only a person's own session lists their tenants (owner,
            // 2026-10-25): a link, a share window, a guest or the agent is
            // refused as an op refuses that token kind.
            if path == "/auth/spaces"
                && (matches!(kind, Kind::Grant | Kind::Agent) || actor == "link_revoked")
            {
                return holds(Expect::Denied);
            }
            holds(if person { e } else { Expect::Unauth })
        }
        RouteRule::NeedsPresence => holds(if person {
            Expect::Admitted
        } else {
            Expect::Unauth
        }),
        RouteRule::DeploymentOwner(e) => holds(
            if actor == "deployment_owner"
                || matches!(kind, Kind::Deployment { .. } | Kind::Machine)
            {
                e
            } else {
                Expect::Denied
            },
        ),
        RouteRule::Door => expect_door_route(method, path, actor, kind),
    }
}

/// The probes `authz_matrix.rs` sends the door routes: `/security/events`
/// for T1, `/download`, `/live`, a live add with a probe body, a live drop
/// of an unknown stream, A's plain file and its meta, an upload to an
/// unknown row.
fn expect_door_route(method: &str, path: &str, actor: &str, kind: Kind) -> (Expect, Mark) {
    let holds = |e| (e, Mark::Holds);
    let role = match kind {
        Kind::Role { role, .. } => Some(role),
        _ => None,
    };
    let in_t1 = !["t2_owner", "deployment_owner"].contains(&actor);
    // A deployment token reaches no app route (plan 4.3: any app op, any
    // space, denied). `/security/events` is a platform surface on the
    // open tenant list: the realm table decides it.
    if matches!(kind, Kind::Deployment { .. } | Kind::Machine) {
        return match (method, path) {
            ("GET", "/security/events") => holds(deployment_op(
                ordo_host::serve::SECEVENTS_OP,
                ordo_api::OpKind::Read,
                kind,
                false,
            )),
            // The probe body is no read: refused for the body, to anyone.
            ("POST", "/live/{stream}") => holds(Expect::Code(400, "invalid_param")),
            _ => holds(Expect::Denied),
        };
    }
    match (method, path) {
        // A tenant's own record: its owner, in it (`lib.rs`, SECEVENTS_OP).
        ("GET", "/security/events") => holds(if role == Some(Role::Owner) && in_t1 {
            Expect::Code(200, "")
        } else {
            Expect::Denied
        }),
        // A person's own download; not a grant or an agent's
        // (`download.rs`: delegated tokens only when ORDO_DELEGABLE_OPS
        // names it).
        ("GET", "/download") => holds(if role.is_some() {
            Expect::Code(200, "")
        } else {
            Expect::Denied
        }),
        ("GET", "/live") => match kind {
            Kind::Dead => holds(Expect::Denied),
            _ => holds(Expect::Code(200, "")),
        },
        // The probe body is no read: refused for the body, to anyone.
        ("POST", "/live/{stream}") => holds(Expect::Code(400, "invalid_param")),
        ("DELETE", "/live/{stream}/{key}") => {
            if actor == "anonymous" {
                holds(Expect::Denied)
            } else if matches!(kind, Kind::Dead) {
                // The token first, then the stream (owner, 2026-10-25).
                holds(Expect::Denied)
            } else {
                holds(Expect::Code(404, "not_found"))
            }
        }
        // A's plain file: A reads it. A manager or owner of T1 passes the
        // door (`std.$blob.get` is manager and owner) and meets the row
        // guard; a grant holder too. A viewer or editor stops at the door.
        ("GET", "/blob/{handle}" | "/blob/{handle}/meta") => match kind {
            _ if actor == "member_a" => holds(Expect::Code(200, "")),
            Kind::Dead | Kind::Agent => holds(Expect::Denied),
            Kind::Role { role, .. } if role < Role::Manager => holds(Expect::Denied),
            _ if !in_t1 => holds(Expect::Code(404, "not_found")),
            // Identical to a handle that is not there (owner, 2026-10-25).
            _ => holds(Expect::Code(404, "not_found")),
        },
        // An upload to a row that does not exist. A viewer, editor or
        // non-member is stopped at the door (`std.files.update` is manager
        // and owner). A manager or owner passes the door and the write is
        // refused because the row is not there; the door answers its own
        // `access_denied` for a row it cannot show exists, so the exact code
        // is not pinned here, only that it is refused.
        ("PUT", "/blob/{entity}/{field}/{id}") => holds(match kind {
            Kind::Role { role, .. } if role >= Role::Manager => Expect::Refused,
            _ => Expect::Denied,
        }),
        _ => panic!("no door probe for {method} {path}"),
    }
}
