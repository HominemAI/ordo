//! Commands, groups, and actor attribution — the units of history.

use crate::chain::ChainHash;
use crate::value::Image;

/// Seconds since the Unix epoch, UTC. Server-observed, never client-claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

/// Typed space id (`spc_…`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpaceId(pub String);

impl std::fmt::Display for SpaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Typed command id (`cmd_…`) — server-minted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommandId(pub String);

/// Correlation id shared by every command of one invocation (`grp_…`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CorrelationId(pub String);

/// Actor attribution, the spec's closed taxonomy (§8). The `Display`
/// rendering is the **canonical form** — it is what appears in audit
/// surfaces and what the hash chain encodes. Worker pods attribute as
/// `System` with the deciding component's name (design §7 [pinned]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// `user:usr_…` — a human acting directly.
    User(String),
    /// `routine:spend-watch` — a platform-born routine.
    Routine(String),
    /// `agent:ellanor on behalf of user:usr_…` — delegation, both named.
    AgentOnBehalfOf { agent: String, user: String },
    /// `source:camera-hub` — a write-only outsider (mail slot).
    Source(String),
    /// `system:retention` — the platform itself, by component.
    System(String),
}

impl Actor {
    /// Read back a canonical attribution string — the inverse of
    /// [`Display`](std::fmt::Display), needed because the chain encodes the
    /// *rendering* and a durable store therefore stores the rendering.
    ///
    /// The delegation form is parsed longest-marker-first: an agent name
    /// cannot contain `" on behalf of user:"` without the split being
    /// ambiguous, so the **last** occurrence is the separator and everything
    /// before it is the agent. Anything that is not one of the five forms is
    /// refused rather than guessed — an unattributable entry in the audit
    /// spine is worse than a loud decode failure.
    pub fn parse(s: &str) -> Result<Actor, crate::chain::DecodeError> {
        let bad = || crate::chain::DecodeError::BadActor(s.to_string());
        if let Some(rest) = s.strip_prefix("agent:") {
            let (agent, user) = rest.rsplit_once(" on behalf of user:").ok_or_else(bad)?;
            if agent.is_empty() || user.is_empty() {
                return Err(bad());
            }
            return Ok(Actor::AgentOnBehalfOf {
                agent: agent.to_string(),
                user: user.to_string(),
            });
        }
        let (tag, rest) = s.split_once(':').ok_or_else(bad)?;
        if rest.is_empty() {
            return Err(bad());
        }
        match tag {
            "user" => Ok(Actor::User(rest.to_string())),
            "routine" => Ok(Actor::Routine(rest.to_string())),
            "source" => Ok(Actor::Source(rest.to_string())),
            "system" => Ok(Actor::System(rest.to_string())),
            _ => Err(bad()),
        }
    }
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Actor::User(id) => write!(f, "user:{id}"),
            Actor::Routine(name) => write!(f, "routine:{name}"),
            Actor::AgentOnBehalfOf { agent, user } => {
                write!(f, "agent:{agent} on behalf of user:{user}")
            }
            Actor::Source(name) => write!(f, "source:{name}"),
            Actor::System(component) => write!(f, "system:{component}"),
        }
    }
}

/// What a command did to its row. The write language's whole verb family
/// compiles down to these three (spec §6): every `set` is a create or a
/// converge, every `del` a delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Create,
    Converge,
    Delete,
}

/// One per-row, attributed command. Image shape is an invariant of the kind:
///
/// | kind     | before | after |
/// |----------|--------|-------|
/// | Create   | none   | full  |
/// | Converge | full   | full  |
/// | Delete   | full   | none  |
///
/// Violations are typed errors at append ([`GroupShapeError`]) — malformed
/// history never enters the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub space: SpaceId,
    pub entity: String,
    pub row_id: String,
    pub kind: CommandKind,
    pub before: Option<Image>,
    pub after: Option<Image>,
    pub at: Timestamp,
}

impl Command {
    /// Check the kind/image invariant.
    pub fn validate(&self) -> Result<(), GroupShapeError> {
        let ok = match self.kind {
            CommandKind::Create => self.before.is_none() && self.after.is_some(),
            CommandKind::Converge => self.before.is_some() && self.after.is_some(),
            CommandKind::Delete => self.before.is_some() && self.after.is_none(),
        };
        if ok {
            Ok(())
        } else {
            Err(GroupShapeError::BadImageShape {
                command: self.id.clone(),
                kind: self.kind,
                has_before: self.before.is_some(),
                has_after: self.after.is_some(),
            })
        }
    }
}

/// All commands from one invocation: one correlation id, one actor, one
/// database transaction, one revert unit. Single-space by construction —
/// the chain is per-space and BYO-DB makes the space the transaction
/// boundary (design §7 [pinned]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandGroup {
    pub correlation: CorrelationId,
    pub actor: Actor,
    pub commands: Vec<Command>,
    /// **Rule 6's second half.** DL §3 bootstrap rule 6 `[settled]` says an
    /// owner administering acl rows does so *"logged with the owner as actor
    /// and flagged as owner administration"*. The actor half was always
    /// right; this is the flag.
    ///
    /// It exists for exactly one reader — an auditor — and for exactly one
    /// question: did the owner use their **rule-6 capability**, or did they
    /// happen to hold an access row like any other member? Both spellings
    /// produce the identical command, on the identical entity, with the
    /// identical actor. Nothing in the images tells them apart, and the
    /// distinction is the whole reason rule 6 is a named capability rather
    /// than an implementation detail.
    ///
    /// **It is not an authorization input and nothing may read it as one.**
    /// The exemption is decided at the door, once per call, and the call
    /// then runs against a schema view whose acl entities carry no guards —
    /// so nothing downstream consults this field to decide anything. It is a
    /// record of a decision already made, hashed into the chain so that the
    /// record cannot be edited later, which is the only property an audit
    /// flag actually needs.
    ///
    /// `false` for every other group, including a platform command
    /// ([`crate`]'s membership surface) and including an owner's ordinary
    /// write. There is exactly one producer of `true`; see
    /// `ordo_conformance::write`'s door step.
    pub owner_administration: bool,
    /// **The second trailer bit** (`plan-opslog.md` D7, `plan-blob-chain.md`
    /// §4.3/§4.4). Set when a revert this group performed restored at least
    /// one blob handle whose before-image carried no recorded digest: an
    /// image written before C5 put one there, so there was nothing to
    /// compare the object against.
    ///
    /// It exists for exactly one reader, the space owner, in the same
    /// command-log explorer that already renders a group's before/after
    /// images, and for exactly one question: did this revert have
    /// anything to verify. That is deliberately not a security question:
    /// the owner ruled 2026-09-09 that an image with no digest is a fact
    /// about the encoding's age rather than evidence of wrongdoing, so this
    /// bit carries no `EventKind` and appears in no denial explorer.
    ///
    /// **Group-level, not field-level, and that is a stated loss of
    /// detail rather than an oversight.** The flag says "at least one
    /// restore in this group was unverified" and not which entity, field,
    /// or handle. The detail is derivable without a second bit: the group
    /// already carries the before-image each restore wrote back, and the
    /// absent `"{field}$digest"` key in that image is exactly what this
    /// flag is about, so one bit plus the image the group already stores
    /// is a complete answer to "which one".
    ///
    /// `false` for every group that is not a revert, every revert whose
    /// restored objects all verified, and every revert that restored no
    /// blob handle at all. There is exactly one producer of `true`; see
    /// `ordo_exec::undo::apply`.
    pub restored_unverified: bool,
    /// **The restore marker** (`plan-production-ops.md` §2.3, R2a): this
    /// entry records that the space's database was restored and its chain
    /// resumed from an earlier point. Encoded as trailer `0x03`, the first
    /// trailer with a payload; see "The trailer" in [`crate::chain`].
    ///
    /// A marker group carries **no commands**: it records that history
    /// moved, and a group that also wrote rows would be two facts in one
    /// entry ([`GroupShapeError::MarkerWithCommands`]). It is the one group
    /// [`GroupShapeError::EmptyGroup`] does not refuse, and only because the
    /// marker is present. Its actor is `system:restore`.
    ///
    /// `None` for every other group, and a group with `None` renders
    /// exactly the bytes it rendered before this field existed.
    pub restore: Option<RestoreMarker>,
}

/// The component a restore marker is attributed to: its actor is
/// `Actor::System(RESTORE_COMPONENT)`, rendered `system:restore`.
pub const RESTORE_COMPONENT: &str = "restore";

/// What a restore marker attests (`plan-production-ops.md` §2.4). Every
/// field is hashed into the chain with the group that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreMarker {
    /// Which chain this marker belongs to. [`CommandGroup::validate`] reads
    /// an ordinary group's space from its first command; a marker has none,
    /// so it carries its own.
    pub space: SpaceId,
    /// The last entry that survived the restore: `(index, hash)`, read
    /// from the restored database when the marker is recorded. It can
    /// differ from the marker's own `prev` when writes landed between the
    /// restore and the record; both are then true.
    pub restored_from: (u64, ChainHash),
    /// What the chain said before the restore, from the last signed
    /// checkpoint or an owner-held pin. `None` is a legal, disclosed answer
    /// after a total loss: it means "unattested", never "nothing was lost".
    pub pre_restore_head: Option<(u64, ChainHash)>,
    /// What the database was restored from. Never absent: a restore from
    /// an unidentifiable source is refused before a marker is built.
    pub source: RestoreSource,
    /// The human who ran the restore, as the operator CLI's `--as` names
    /// them. A field and not the actor: the actor is `system:restore`,
    /// and the tool that records a marker attributes, it does not
    /// authenticate.
    pub operator: String,
}

/// Where a restored database came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreSource {
    /// Postgres point-in-time recovery to the target time `at`, in the
    /// chain's one time unit: [`Timestamp`], seconds since the epoch.
    Pitr { at: Timestamp },
    /// An export bundle, named by `sha256(manifest.json)`: the manifest
    /// digests every other member, so its own digest fixes the bundle.
    Bundle { manifest_sha256: [u8; 32] },
}

impl CommandGroup {
    /// Structural validity: single-space, every command's image shape
    /// matches its kind, and non-empty unless the group is a restore
    /// marker, which must be empty, attributed to `system:restore`, and
    /// whose space is the marker's.
    pub fn validate(&self) -> Result<&SpaceId, GroupShapeError> {
        if let Some(marker) = &self.restore {
            if !self.commands.is_empty() {
                return Err(GroupShapeError::MarkerWithCommands {
                    correlation: self.correlation.clone(),
                });
            }
            if self.actor != Actor::System(RESTORE_COMPONENT.into()) {
                return Err(GroupShapeError::MarkerActor {
                    correlation: self.correlation.clone(),
                    actor: self.actor.to_string(),
                });
            }
            return Ok(&marker.space);
        }
        let first = self
            .commands
            .first()
            .ok_or_else(|| GroupShapeError::EmptyGroup {
                correlation: self.correlation.clone(),
            })?;
        for cmd in &self.commands {
            cmd.validate()?;
            if cmd.space != first.space {
                return Err(GroupShapeError::MixedSpaces {
                    correlation: self.correlation.clone(),
                    expected: first.space.clone(),
                    found: cmd.space.clone(),
                });
            }
        }
        Ok(&first.space)
    }
}

/// Structural rejections at the append gate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GroupShapeError {
    EmptyGroup {
        correlation: CorrelationId,
    },
    MixedSpaces {
        correlation: CorrelationId,
        expected: SpaceId,
        found: SpaceId,
    },
    BadImageShape {
        command: CommandId,
        kind: CommandKind,
        has_before: bool,
        has_after: bool,
    },
    /// A restore marker that also carries commands. The marker records
    /// that history moved; rows written in the same entry would be a
    /// second fact sharing its hash.
    MarkerWithCommands {
        correlation: CorrelationId,
    },
    /// A restore marker attributed to anything but `system:restore`. The
    /// verifier locates markers by the plaintext actor column
    /// (`plan-production-ops.md` R2b), so a marker under another actor
    /// would be a seam the anchor rule never sees.
    MarkerActor {
        correlation: CorrelationId,
        actor: String,
    },
}

impl std::fmt::Display for GroupShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupShapeError::EmptyGroup { correlation } => {
                write!(f, "group `{}` contains no commands", correlation.0)
            }
            GroupShapeError::MixedSpaces {
                correlation,
                expected,
                found,
            } => write!(
                f,
                "group `{}` spans spaces `{expected}` and `{found}` — a group is single-space",
                correlation.0
            ),
            GroupShapeError::BadImageShape {
                command,
                kind,
                has_before,
                has_after,
            } => write!(
                f,
                "command `{}` is {kind:?} but has before={has_before}, after={has_after}",
                command.0
            ),
            GroupShapeError::MarkerWithCommands { correlation } => write!(
                f,
                "group `{}` is a restore marker and carries commands; a marker records \
                 that history moved and writes nothing",
                correlation.0
            ),
            GroupShapeError::MarkerActor { correlation, actor } => write!(
                f,
                "group `{}` is a restore marker attributed to `{actor}`; a marker's actor \
                 is `system:{RESTORE_COMPONENT}`",
                correlation.0
            ),
        }
    }
}

impl std::error::Error for GroupShapeError {}
