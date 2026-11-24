//! **`prev`: renames across versions** (`plan-syntax-additions.md` §5,
//! `plan-versions.md` increment 15).
//!
//! A `prev` declares that a field or an entity is the same thing as a name
//! in the version serving now, so the plan renders a rename and never a
//! drop and an add. The compiler checks the shape (`prev_self`,
//! `prev_still_declared`, `prev_duplicate`, in the parser). This module is
//! the plan's half: it reads the version serving now (`prior`) beside the
//! program (`declared`) and answers which renames the step makes, which
//! marks cannot be made, and which have already landed.
//!
//! - `prev_unresolved`: the named thing is not in the version serving now.
//!   `prev` reaches one version back only.
//! - `prev_type`: a rename whose type differs. No cast option exists yet, so
//!   it is refused, naming the two types.
//! - `prev_mapping_required`: a merge (`prev(first, last)`) or a move
//!   (`prev(orders.note)`). Each needs a mapping the plan does not offer
//!   yet.
//! - `prev_sealed`: a sealed field's ciphertext is bound to its field, so
//!   a copy cannot be read under the new name. Re-sealing is the backfill
//!   worker's (increment 10b).
//! - `prev_stale`, a warning: the rename has already landed in the version
//!   serving now. The mark can go.
//!
//! With no version serving (a fresh database), nothing is checked: there is
//! nothing to rename from, and the program is created as declared.

use std::collections::BTreeMap;

use crate::{Entity, Schema};

/// One `prev` mark, as the program declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrevMark {
    pub entity: String,
    /// `None` for an entity's `(prev: old)`.
    pub field: Option<String>,
    /// The names as written: one for a rename, two for a merge,
    /// `entity.field` for a move.
    pub prev: Vec<String>,
    /// Where the mark is, `line:col`, for the offer to remove it.
    pub line: u32,
}

impl PrevMark {
    /// As written in the source: `notes.heading prev(title)` or
    /// `customers (prev: clients)`.
    pub fn text(&self) -> String {
        match &self.field {
            Some(f) => format!("{}.{f} prev({})", self.entity, self.prev.join(", ")),
            None => format!("{} (prev: {})", self.entity, self.prev.join(", ")),
        }
    }
}

/// A `prev` mark the step cannot make, or one that has landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrevIssue {
    /// `prev_unresolved`, `prev_type`, `prev_mapping_required`,
    /// `prev_sealed`, `prev_stale`.
    pub class: &'static str,
    pub mark: PrevMark,
    pub message: String,
    /// What the developer can do.
    pub fix: String,
}

/// What [`check`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrevReport {
    /// Marks the step cannot make. A plan with any is refused.
    pub errors: Vec<PrevIssue>,
    /// Marks that have landed: warnings, never refusals.
    pub stale: Vec<PrevIssue>,
}

/// The renames a step makes: `(entity, old field) -> new field`, and
/// `old entity -> new entity`, each one resolved by [`check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Renames {
    pub fields: BTreeMap<(String, String), String>,
    pub entities: BTreeMap<String, String>,
}

impl Renames {
    /// The new name of `entity.field`, when the step renames it.
    pub fn field_to(&self, entity: &str, field: &str) -> Option<&str> {
        self.fields
            .get(&(entity.to_string(), field.to_string()))
            .map(String::as_str)
    }

    /// The old name of `entity.field`, when the step renamed it from one.
    pub fn field_from(&self, entity: &str, field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|((e, _), to)| e == entity && to.as_str() == field)
            .map(|((_, from), _)| from.as_str())
    }

    /// The old name of `entity`, when the step renames it from one.
    pub fn entity_from(&self, entity: &str) -> Option<&str> {
        self.entities
            .iter()
            .find(|(_, to)| to.as_str() == entity)
            .map(|(from, _)| from.as_str())
    }
}

/// The plan's checks of every `prev` mark in `declared` against the version
/// serving now. `None` for `prior` checks nothing.
pub fn check(prior: Option<&Schema>, declared: &Schema) -> PrevReport {
    let mut out = PrevReport::default();
    let Some(prior) = prior else {
        return out;
    };
    for m in &declared.prev {
        let Some(e) = declared.native(&m.entity) else {
            continue;
        };
        match &m.field {
            None => entity_mark(prior, m, &mut out),
            Some(f) => field_mark(prior, declared, e, f, m, &mut out),
        }
    }
    out
}

/// The renames `declared` makes from `prior`: only the marks [`check`]
/// passes. Used by the diff, so an unresolved or stale mark falls back to
/// what the diff would see without it.
pub fn renames(prior: Option<&Schema>, declared: &Schema) -> Renames {
    let mut r = Renames::default();
    let Some(prior) = prior else {
        return r;
    };
    let report = check(Some(prior), declared);
    let bad = |m: &PrevMark| {
        report
            .errors
            .iter()
            .chain(report.stale.iter())
            .any(|i| i.mark == *m)
    };
    for m in &declared.prev {
        if bad(m) {
            continue;
        }
        match &m.field {
            None => {
                r.entities.insert(m.prev[0].clone(), m.entity.clone());
            }
            Some(f) => {
                // A field of a renamed entity is renamed within it: the old
                // name is looked up under the entity's old name.
                let from_entity = entity_in_prior(prior, declared, &m.entity)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| m.entity.clone());
                if from_entity == m.entity {
                    r.fields
                        .insert((m.entity.clone(), m.prev[0].clone()), f.clone());
                }
            }
        }
    }
    r
}

/// The entity in `prior` that `declared`'s `name` was: itself, or its
/// `prev`.
fn entity_in_prior<'a>(prior: &'a Schema, declared: &Schema, name: &str) -> Option<&'a Entity> {
    prior.native(name).or_else(|| {
        declared
            .prev
            .iter()
            .find(|m| m.entity == name && m.field.is_none())
            .and_then(|m| prior.native(&m.prev[0]))
    })
}

fn entity_mark(prior: &Schema, m: &PrevMark, out: &mut PrevReport) {
    let old = &m.prev[0];
    if prior.native(&m.entity).is_some() {
        out.stale.push(PrevIssue {
            class: "prev_stale",
            mark: m.clone(),
            message: format!(
                "`{} (prev: {old})` is done: the version serving now already has `{}`",
                m.entity, m.entity
            ),
            fix: "remove the mark".into(),
        });
        return;
    }
    if prior.native(old).is_none() {
        out.errors.push(PrevIssue {
            class: "prev_unresolved",
            mark: m.clone(),
            message: format!(
                "the version serving now has no `{old}`. `prev` reaches one version back only"
            ),
            fix: format!("name an entity of the version serving now, or remove `prev: {old}`"),
        });
    }
}

fn field_mark(
    prior: &Schema,
    declared: &Schema,
    e: &Entity,
    f: &str,
    m: &PrevMark,
    out: &mut PrevReport,
) {
    let Some(was) = entity_in_prior(prior, declared, &e.name) else {
        // The entity is new, or its own `prev` is unresolved, which is
        // reported on the entity. A new entity has no field to rename.
        if declared
            .prev
            .iter()
            .any(|x| x.entity == e.name && x.field.is_none())
        {
            return;
        }
        out.errors.push(PrevIssue {
            class: "prev_unresolved",
            mark: m.clone(),
            message: format!(
                "the version serving now has no `{}`, so `{f}` renames nothing. `prev` \
                 reaches one version back only",
                e.name
            ),
            fix: format!("remove `prev({})`", m.prev.join(", ")),
        });
        return;
    };
    if was.field(f).is_some() {
        out.stale.push(PrevIssue {
            class: "prev_stale",
            mark: m.clone(),
            message: format!(
                "`{}.{f} prev({})` is done: the version serving now already has `{f}`",
                e.name,
                m.prev.join(", ")
            ),
            fix: "remove the mark".into(),
        });
        return;
    }
    if m.prev.len() > 1 || m.prev[0].contains('.') {
        let what = if m.prev.len() > 1 {
            "a merge"
        } else {
            "a move from another entity"
        };
        out.errors.push(PrevIssue {
            class: "prev_mapping_required",
            mark: m.clone(),
            message: format!(
                "`{}.{f} prev({})` is {what}, which needs a mapping the plan does not offer \
                 yet",
                e.name,
                m.prev.join(", ")
            ),
            fix: format!(
                "add `{f}` as a new field and fill it with an op, then remove the old \
                 field(s) in a later version"
            ),
        });
        return;
    }
    let old = &m.prev[0];
    let Some(from) = was.field(old) else {
        out.errors.push(PrevIssue {
            class: "prev_unresolved",
            mark: m.clone(),
            message: format!(
                "the version serving now has no `{}.{old}`. `prev` reaches one version back \
                 only",
                was.name
            ),
            fix: format!(
                "name a field of `{}` that is served now, or remove `prev({old})`",
                was.name
            ),
        });
        return;
    };
    let Some(to) = e.field(f) else {
        return;
    };
    if from.sealed || to.sealed {
        out.errors.push(PrevIssue {
            class: "prev_sealed",
            mark: m.clone(),
            message: format!(
                "`{}.{old}` is sealed, and its ciphertext is bound to the field, so a copy \
                 cannot be read as `{f}`",
                was.name
            ),
            fix: "keep the name for now: renaming a sealed field re-seals every row, which \
                  is the backfill worker's (increment 10b)"
                .into(),
        });
        return;
    }
    let ty = |x: &crate::Field| (crate::ddl::column_ty(x), x.text_max);
    if ty(from) != ty(to) || from.ty != to.ty {
        out.errors.push(PrevIssue {
            class: "prev_type",
            mark: m.clone(),
            message: format!(
                "`{}.{old}` was `{}` and `{}.{f}` is `{}`; a rename keeps the type",
                was.name,
                type_text(from),
                e.name,
                type_text(to)
            ),
            fix: format!(
                "rename it in one version with the type `{}`, and change the type in the next",
                type_text(from)
            ),
        });
    }
}

fn type_text(f: &crate::Field) -> String {
    let base = crate::ddl::column_ty(f);
    match f.text_max {
        Some(n) => format!("text({n})"),
        None => base,
    }
}
