//! What a capability grant pins, and how two pins are compared.
//!
//! # The rule this implements, and the one it replaces
//!
//! `security-logging-design.md` §5 and `api-engine-design.md` §10 both
//! specified an **ordered field-set** comparison per op: hash equal ⇒
//! unchanged; hash differs ⇒ compare field sets, subset = narrowing (applies
//! silently), superset or disjoint = expansion (grant dead until re-consent).
//!
//! `definition-language.md` §8 calls that form *"decidable and **wrong**"*,
//! and the counterexample is one line of ordinary editing:
//!
//! > Delete `member == $caller` from an op body, or add `unguarded` to its
//! > header, and the op's **output field set is byte-identical**: same
//! > columns, same types, same order. A field-set comparison reads that as
//! > "not an expansion", classifies it as narrowing, and **applies it
//! > silently**. A grant a person consented to as *"this app may read my
//! > expenses"* becomes *"this app may read everyone's expenses"* with no
//! > re-consent prompt, no audit flag, and no visible change anywhere. Field
//! > sets answer **which columns**; they never answered **which rows**, and
//! > rows are what the consent was about.
//!
//! DL wins (`corpus-rulings.md`, "Grant pinning: DL §8 wins, and the losing
//! form is a silent escalation"). So a pin carries the field set **and** the
//! row-scope shape, and:
//!
//! > **Narrowing = field-set subset AND row scope no weaker.** Both
//! > conditions, or it is not narrowing.
//!
//! # Why this crate holds only the data
//!
//! `ordo-identity` is a leaf: it depends on no sibling crate, so a door
//! decision never drags a compiler in. The pin is therefore **plain data**
//! that the compiler fills — `ordo_plan::artifact::op_scope` and
//! `op_row_scope` produce it, `ordo_host` converts, and this module compares.
//! One producer of what an op reaches; one comparator of what changed.
//!
//! # Why the whole shape rides in the pin, not a digest of it
//!
//! A digest can answer "same or different" and nothing else, and this
//! comparison has to separate *narrowing* (applies silently) from *expansion*
//! (kills the grant) — which is exactly what §5 discovered when it retired
//! the single combined hash: *"narrowing changes the hash too"*. Strength is
//! a subset test, and a subset test needs the elements. So the token carries
//! the consented shape, and the door decides from the token alone, with no
//! registry history — which is what keeps a registry outage from becoming an
//! authorization outage.
//!
//! The cost is token size, and it is real: a grant over many ops touching
//! many entities produces a long bearer string. That is a stated cost of
//! consent being legible, not an oversight.

use std::collections::{BTreeMap, BTreeSet};

use crate::principal::ManifestHash;

/// How an op's reach over one entity is bounded **in rows**, for one verb.
///
/// `source` provenance (`own` / `inherited from x` / `acl-derived`) is
/// deliberately **not** carried. It belongs on the consent page, which reads
/// the manifest; it is not part of the row bound, and comparing it would kill
/// a grant over a refactor that moved a clause between declarations while
/// admitting exactly the same rows.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum GuardPin {
    /// Every row: the entity declares no guard for this verb, or declares
    /// `access <verb> open`.
    Every,
    /// The rows this walk admits — the effective walk, rebase applied, as
    /// `ordo_schema::Guards::render` writes it.
    Walk(String),
}

impl GuardPin {
    /// Is `self` (the consented bound) still honoured by `now`?
    ///
    /// A pinned walk must be **the same walk**. A *different* walk may well
    /// be narrower, and deciding that is query containment — undecidable in
    /// the general case and not something an access check gets to guess. DL
    /// says "is the same walk", and the failure direction of the strict
    /// reading is a re-consent prompt rather than a silent widening.
    fn honoured_by(&self, now: &GuardPin) -> bool {
        match self {
            // Nothing to weaken: a pin that bounded no rows cannot be
            // widened. Gaining a guard is strengthening and applies silently.
            GuardPin::Every => true,
            GuardPin::Walk(w) => matches!(now, GuardPin::Walk(n) if n == w),
        }
    }
}

/// One entity's line in a pinned manifest: **which columns** and **which
/// rows**, together, because separating them is how the escalation happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityPin {
    pub entity: String,
    /// Base fields the op reads — `ordo_plan::artifact::EntityScope::read`,
    /// which is the field-grain manifest 9l derived, not the output column
    /// list. A field read in a filter and never projected is still disclosed:
    /// the rows that come back are the ones it selected.
    pub read: BTreeSet<String>,
    /// Base fields a write statement assigns.
    pub written: BTreeSet<String>,
    /// A tree traversal read the materialized path: structure, no values.
    pub structure: bool,
    pub read_guard: GuardPin,
    pub write_guard: GuardPin,
    /// The op's own body restricts this entity's rows to the caller's.
    pub caller_restricted: bool,
}

impl EntityPin {
    /// A minimal pin for an entity with no guards and no restriction — the
    /// starting point a builder fills in.
    pub fn new(entity: &str) -> EntityPin {
        EntityPin {
            entity: entity.to_string(),
            read: BTreeSet::new(),
            written: BTreeSet::new(),
            structure: false,
            read_guard: GuardPin::Every,
            write_guard: GuardPin::Every,
            caller_restricted: false,
        }
    }
}

/// One op's pinned manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpPin {
    /// The digest of the op's whole manifest. An equality fast path, and the
    /// only thing that notices a change to a part of the manifest the shape
    /// below does not cover — see [`PinDrift::Narrowed`].
    pub manifest_hash: ManifestHash,
    /// The op's `unguarded` acknowledgment. Pinned at the op level because DL
    /// states it as its own clause, and because folding it into each guard
    /// would make a bypassed guard indistinguishable from an absent one — so
    /// dropping a guard from an already-`unguarded` op would compare equal.
    pub unguarded: bool,
    /// Entity-sorted. Sorted rather than a map so the encoding has one order
    /// and the comparison has no iteration-order question.
    pub entities: Vec<EntityPin>,
}

impl OpPin {
    fn entity(&self, name: &str) -> Option<&EntityPin> {
        self.entities.iter().find(|e| e.entity == name)
    }
}

/// What changed between the manifest a person consented to and the one
/// deployed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinDrift {
    /// Byte-identical manifests.
    Unchanged,
    /// Strictly less reach: fewer columns, or a tighter row bound, or both.
    /// **Applies silently and is audit-noted** — the behaviour §5 wanted and
    /// the combined hash denied. The cost of getting this wrong is not
    /// theoretical: a consent screen people are made to click through after
    /// every deploy is a consent screen they stop reading.
    Narrowed { notes: Vec<String> },
    /// More reach, in columns or in rows. **The grant is dead until
    /// re-consent** — for the whole grant, not just this op: consent was one
    /// act over a set of ops, and DL says "dead for that space until
    /// re-consent".
    Expanded { reasons: Vec<String> },
    /// The op is not deployed. Expansion-equivalent: the grant no longer
    /// means what was consented to, and an absent manifest is not a smaller
    /// one.
    Undeployed,
}

impl PinDrift {
    /// Does this drift kill the grant?
    pub fn is_fatal(&self) -> bool {
        matches!(self, PinDrift::Expanded { .. } | PinDrift::Undeployed)
    }

    /// One line for an audit record.
    pub fn note(&self) -> String {
        match self {
            PinDrift::Unchanged => "unchanged".into(),
            PinDrift::Narrowed { notes } => format!("narrowed: {}", notes.join("; ")),
            PinDrift::Expanded { reasons } => format!("expanded: {}", reasons.join("; ")),
            PinDrift::Undeployed => "the op is no longer deployed".into(),
        }
    }
}

/// Compare a consented pin against the deployed one.
///
/// Decidable from the two manifests alone, with no registry history — which
/// the pre-round-1 formulation quietly required, since it promised that
/// narrowing applied silently *and* that any hash change killed the grant,
/// and narrowing changes the hash.
///
/// # The order of the tests is the ruling
///
/// **Every** row-scope weakening is checked, and a weakening kills the grant
/// *"even when the field set is unchanged or strictly smaller"* (DL §8).
/// There is no combination of column narrowing that buys a row-scope
/// widening, so the two halves are not weighed against each other: expansion
/// on either side is expansion.
pub fn compare(consented: &OpPin, current: &OpPin) -> PinDrift {
    if consented == current {
        return PinDrift::Unchanged;
    }
    let mut reasons: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // ── row scope: any weakening is fatal ────────────────────────────────
    if current.unguarded && !consented.unguarded {
        reasons.push(
            "`unguarded` is newly set — every guard on this op's own sources is now bypassed"
                .into(),
        );
    }
    if !current.unguarded && consented.unguarded {
        notes.push("`unguarded` was cleared".into());
    }

    for then in &consented.entities {
        let Some(now) = current.entity(&then.entity) else {
            // The op no longer reaches this entity at all. Fewer rows and
            // fewer columns; its guards went with it.
            notes.push(format!("`{}` is no longer reached", then.entity));
            continue;
        };
        for (verb, t, n) in [
            ("read", &then.read_guard, &now.read_guard),
            ("write", &then.write_guard, &now.write_guard),
        ] {
            if !t.honoured_by(n) {
                let GuardPin::Walk(w) = t else {
                    unreachable!("Every is honoured by anything")
                };
                reasons.push(match n {
                    GuardPin::Every => format!(
                        "`{}` lost its {verb} guard — `{w}` no longer bounds which rows this op reaches",
                        then.entity
                    ),
                    GuardPin::Walk(now_walk) => format!(
                        "`{}`'s {verb} guard changed from `{w}` to `{now_walk}` — a different walk is a different set of rows",
                        then.entity
                    ),
                });
            } else if t != n {
                notes.push(format!("`{}` gained a {verb} guard", then.entity));
            }
        }
        if then.caller_restricted && !now.caller_restricted {
            reasons.push(format!(
                "`{}` lost its `$caller` restriction — the op's rows are no longer the caller's own",
                then.entity
            ));
        }
        if !then.caller_restricted && now.caller_restricted {
            notes.push(format!("`{}` gained a `$caller` restriction", then.entity));
        }

        // ── field scope: superset or disjoint is expansion ───────────────
        let new_read: Vec<&String> = now.read.difference(&then.read).collect();
        if !new_read.is_empty() {
            reasons.push(format!("`{}` now reads {}", then.entity, join(&new_read)));
        }
        let new_written: Vec<&String> = now.written.difference(&then.written).collect();
        if !new_written.is_empty() {
            reasons.push(format!(
                "`{}` now writes {}",
                then.entity,
                join(&new_written)
            ));
        }
        if now.structure && !then.structure {
            reasons.push(format!(
                "`{}` is now walked as a tree — rows reached through an ancestor the caller never named",
                then.entity
            ));
        }
        if then.read.difference(&now.read).next().is_some()
            || then.written.difference(&now.written).next().is_some()
            || (then.structure && !now.structure)
        {
            notes.push(format!("`{}` reaches fewer fields", then.entity));
        }
    }

    // An entity the consent never mentioned is reach the consent never gave.
    for now in &current.entities {
        if consented.entity(&now.entity).is_none() {
            reasons.push(format!("`{}` is newly reached", now.entity));
        }
    }

    if !reasons.is_empty() {
        return PinDrift::Expanded { reasons };
    }
    if notes.is_empty() {
        // The manifests differ and neither the columns nor the rows moved:
        // something the pinned shape does not cover changed — a parameter's
        // `<=` bound, a cost class, a write's mode label. DL's rule makes
        // that narrowing (equal is a subset, and the row scope is no
        // weaker), so it applies; the note is here so an operator reading
        // the audit trail is not told "narrowed" with nothing narrowed.
        //
        // **What this does not cover is volume.** A `take $n` whose declared
        // bound was loosened reaches more rows of the same shape, and neither
        // half of the pinned manifest can see it. Recorded rather than
        // silently folded in: adding it would be inventing a clause DL does
        // not have.
        notes
            .push("the manifest changed outside the pinned shape (same columns, same rows)".into());
    }
    PinDrift::Narrowed { notes }
}

fn join(items: &[&String]) -> String {
    items
        .iter()
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── the canonical encoding ───────────────────────────────────────────────
//
// Netstrings (`<len>:<bytes>`), read back positionally. Two reasons over a
// delimiter-separated record: a guard walk contains spaces, dots, `$`, `(`
// and `==`, so there is no delimiter that is safe by inspection; and a
// length-prefixed encoding is unambiguous by construction, which is the same
// property `combined_manifest_hash` needed when it length-prefixed its
// preimage (SH-27). The inputs here are op names and guard text, both of
// which a package author controls, and the output decides whether a consented
// grant stays alive.
//
// Not JSON: a token payload is parsed on every request, and a hand-rolled
// positional read cannot be tricked by a duplicate key the way a lenient JSON
// parser can — the same reason `token.rs` gives for its own format.

const VERSION: &str = "pins.v1";

fn put(out: &mut String, s: &str) {
    out.push_str(&s.len().to_string());
    out.push(':');
    out.push_str(s);
}

/// Encode the per-op pins a grant carries. Deterministic: `BTreeMap` and
/// `BTreeSet` iteration order, entity-sorted lists, and a fixed field order.
pub fn encode_pins(pins: &BTreeMap<String, OpPin>) -> String {
    let mut out = String::new();
    put(&mut out, VERSION);
    put(&mut out, &pins.len().to_string());
    for (op, pin) in pins {
        put(&mut out, op);
        put(&mut out, &hex(&pin.manifest_hash.0));
        put(&mut out, if pin.unguarded { "1" } else { "0" });
        put(&mut out, &pin.entities.len().to_string());
        for e in &pin.entities {
            put(&mut out, &e.entity);
            put(&mut out, &set_str(&e.read));
            put(&mut out, &set_str(&e.written));
            put(&mut out, if e.structure { "1" } else { "0" });
            put(&mut out, &guard_str(&e.read_guard));
            put(&mut out, &guard_str(&e.write_guard));
            put(&mut out, if e.caller_restricted { "1" } else { "0" });
        }
    }
    out
}

/// The inverse. `None` for anything that is not exactly what [`encode_pins`]
/// writes — a trailing byte, a short count, a bad hex digit. A pin that does
/// not decode is not a pin, and the caller's only sound response is to refuse
/// the token.
pub fn decode_pins(text: &str) -> Option<BTreeMap<String, OpPin>> {
    let mut r = Reader { s: text, i: 0 };
    if r.take()? != VERSION {
        return None;
    }
    let n: usize = r.take()?.parse().ok()?;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let op = r.take()?.to_string();
        let manifest_hash = ManifestHash(unhex(r.take()?)?);
        let unguarded = flag(r.take()?)?;
        let en: usize = r.take()?.parse().ok()?;
        let mut entities = Vec::with_capacity(en);
        for _ in 0..en {
            entities.push(EntityPin {
                entity: r.take()?.to_string(),
                read: parse_set(r.take()?),
                written: parse_set(r.take()?),
                structure: flag(r.take()?)?,
                read_guard: parse_guard(r.take()?)?,
                write_guard: parse_guard(r.take()?)?,
                caller_restricted: flag(r.take()?)?,
            });
        }
        // The encoder writes entity-sorted lists; a token that arrives with
        // them shuffled would compare the same but hash differently, so the
        // one order is enforced on the way in rather than assumed.
        if entities.windows(2).any(|w| w[0].entity >= w[1].entity) && entities.len() > 1 {
            return None;
        }
        out.insert(
            op,
            OpPin {
                manifest_hash,
                unguarded,
                entities,
            },
        );
    }
    // Trailing bytes are a second message hiding behind the first.
    if r.i != r.s.len() {
        return None;
    }
    Some(out)
}

struct Reader<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self) -> Option<&'a str> {
        let rest = self.s.get(self.i..)?;
        let colon = rest.find(':')?;
        let len: usize = rest.get(..colon)?.parse().ok()?;
        let start = self.i + colon + 1;
        let end = start.checked_add(len)?;
        let out = self.s.get(start..end)?;
        self.i = end;
        Some(out)
    }
}

fn flag(s: &str) -> Option<bool> {
    match s {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

/// Field names are identifiers, so a comma cannot occur in one.
fn set_str(s: &BTreeSet<String>) -> String {
    s.iter().cloned().collect::<Vec<_>>().join(",")
}

fn parse_set(s: &str) -> BTreeSet<String> {
    if s.is_empty() {
        BTreeSet::new()
    } else {
        s.split(',').map(String::from).collect()
    }
}

/// `""` is `Every`; anything else is the walk itself. A walk is never empty
/// (it renders at least one atom), so the two cannot collide.
fn guard_str(g: &GuardPin) -> String {
    match g {
        GuardPin::Every => String::new(),
        GuardPin::Walk(w) => w.clone(),
    }
}

fn parse_guard(s: &str) -> Option<GuardPin> {
    Some(if s.is_empty() {
        GuardPin::Every
    } else {
        GuardPin::Walk(s.to_string())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
