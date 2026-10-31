//! What enrolment records (`plan-enrolment.md` §4.4).
//!
//! Every act that gives somebody a way in, or changes the way they have,
//! writes one [`ordo_secevent::EventKind::Enrolment`]. The credential is the
//! deployment's, so the act is recorded at **deployment** scope. When a
//! tenant's administrator did it, the same act is **mirrored** to that
//! tenant's stream, so the tenant's owner can read it without being shown
//! anybody else's.

use ordo_secevent::{EventKind, Observation, Recorder, Relationship, Subject};

/// One act, and who did it.
pub struct Act<'a> {
    /// A short fixed word: `ticket-issued`, `member-admitted`, ...
    pub act: &'a str,
    /// `key=value` pairs naming who, to whom and by which authority. Never
    /// a secret: a ticket is named by the first eight characters of its
    /// hash, and a password is not named at all.
    pub detail: String,
    /// The principal or member who did it. `None` for the platform itself.
    pub by: Option<&'a str>,
    /// The tenant whose administrator did it, if one did. The act is then
    /// mirrored into that tenant's stream.
    pub tenant: Option<&'a str>,
    /// Whether the deployment's own stream also carries it. True for an act
    /// on a credential or an identity, which the deployment owns. False for
    /// an act on a membership, which is the tenant's business alone.
    pub deployment: bool,
    pub at: u64,
}

/// Record one act. Never fails and never blocks: the recorder is an outbox
/// (`security-logging-design.md` §6.1).
pub fn record(stream: &Recorder, a: &Act<'_>) {
    let subject = match a.by {
        Some(who) => Subject::member(who),
        None => Subject::platform("enrolment"),
    };
    let op = format!("enrol.{}", a.act);
    let correlation = ordo_secevent::correlation::mint();
    let kind = || EventKind::Enrolment {
        act: a.act.to_string(),
        detail: a.detail.clone(),
    };
    let at = ordo_commandlog::Timestamp(a.at);
    // The deployment's own record, for acts on what the deployment owns.
    if a.deployment {
        stream.observe(
            Observation {
                space: a.tenant.map(|t| ordo_commandlog::SpaceId(t.to_string())),
                subject: subject.clone(),
                op: op.clone(),
                kind: kind(),
                at,
                correlation: correlation.clone(),
            },
            Relationship::None,
        );
    }
    // The mirror, for the tenant that acted.
    if let Some(t) = a.tenant {
        stream.observe(
            Observation {
                space: Some(ordo_commandlog::SpaceId(t.to_string())),
                subject,
                op,
                kind: kind(),
                at,
                correlation,
            },
            Relationship::Member,
        );
    }
}
