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
//! - **Type casts** (built 2026-11-25): a rename whose type differs is a
//!   one-to-one map, a [`FieldMap`] with one old and one new field. Its
//!   conversion is the stored plan's `--fill`, never guessed (`forward-gaps`
//!   asks 15 and 17). `prev_type` is left for a file, whose handle no
//!   expression computes.
//! - **Moves and entity merges** (built 2026-11-25): `prev(other.field)`
//!   moves a field from another entity, an [`EntityMove`]. The rows
//!   correspond by the stored plan's rule (`--correspond <other>.<link>`),
//!   a link of `other` to this entity, `owned unique`. When `other` is gone
//!   and every field of it moves, the move is an entity merge.
//!   `prev_mapping_required` is left for a move with a mapping
//!   (`prev(a.x, b.y)`), a move from this entity itself, a move from one
//!   entity into two, and a move into an entity renamed in the same step.
//! - **Entity splits** (built 2026-11-25): an entity `(prev: whole)` while
//!   `whole` is still declared is a part split off it, an [`EntityPart`].
//!   It takes the fields it declares from `whole`, one row per `whole` row,
//!   by the correspondence rule the stored plan states (`--correspond`).
//!   Two entities `(prev: x)` with `x` gone is a split with no whole,
//!   refused as `entity_split_needs_whole`.
//! - **Splits and merges** (increment 15b): several fields each
//!   `prev(name)` split `name`; one field `prev(first, last)` merges two.
//!   Each resolves to a [`FieldMap`]. Its mapping is the stored plan's
//!   (`--fill` for each new field, `--reverse` for each old one), checked
//!   by the triage, not here.
//! - `prev_sealed`: a sealed field's ciphertext is bound to its field, so
//!   a copy cannot be read under the new name. Re-sealing is the backfill
//!   worker's (increment 10b).
//! - `prev_default_required`: a required field renamed with no default.
//!   While both versions write, each one's insert names only its own column
//!   and the host's dual writes set the other in the same transaction
//!   (increment 11a), so a required column needs a default to take first.
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
    /// `move_renamed`, `prev_sealed`, `prev_default_required`,
    /// `prev_stale`.
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
    /// The splits and merges.
    pub maps: Vec<FieldMap>,
    /// The entity splits: each part, and the entity it is split off.
    pub parts: Vec<EntityPart>,
    /// The moves from another entity, one per pair of entities, and the
    /// entity merges spelled with them.
    pub moves: Vec<EntityMove>,
}

/// **A move from another entity, or an entity merge** (`forward-gaps` asks
/// 7 and 8): fields of `entity` each `prev(from.<field>)`. The fields keep
/// their names. When `from` is no longer declared, every one of its rows
/// merges into the row of `entity` its link names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityMove {
    /// The entity the fields move into.
    pub entity: String,
    /// The entity they move from.
    pub from: String,
    /// The moved fields of `entity`, in declaration order, and the name
    /// each had in `from`.
    pub fields: Vec<(String, String)>,
    /// `from` is gone: an entity merge.
    pub merge: bool,
}

/// **An entity split** (`forward-gaps` ask 8): `part (prev: whole)` while
/// `whole` stays declared. The part's fields, its link to `whole` aside,
/// move from `whole` into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityPart {
    pub part: String,
    pub whole: String,
    /// The part's declared fields, in declaration order.
    pub fields: Vec<String>,
}

/// **A field split or merge** (increment 15b): the old fields of `entity`
/// in the version serving now, and the new fields that take their values.
/// A split has one `from` and several `to`; a merge several `from` and one
/// `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMap {
    pub entity: String,
    /// The old fields, as the version serving now declares them.
    pub from: Vec<crate::Field>,
    /// The new fields, in declaration order.
    pub to: Vec<String>,
}

impl FieldMap {
    pub fn is_split(&self) -> bool {
        self.to.len() > 1
    }

    /// The old fields' names.
    pub fn from_names(&self) -> Vec<String> {
        self.from.iter().map(|f| f.name.clone()).collect()
    }
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

    /// The split or merge `entity.field` is a new field of.
    pub fn map_to(&self, entity: &str, field: &str) -> Option<&FieldMap> {
        self.maps
            .iter()
            .find(|m| m.entity == entity && m.to.iter().any(|t| t == field))
    }

    /// The split or merge `entity.field` is an old field of.
    pub fn map_from(&self, entity: &str, field: &str) -> Option<&FieldMap> {
        self.maps
            .iter()
            .find(|m| m.entity == entity && m.from.iter().any(|f| f.name == field))
    }

    /// The entity `part` is split off, when it is a part.
    pub fn part_of(&self, part: &str) -> Option<&EntityPart> {
        self.parts.iter().find(|p| p.part == part)
    }

    /// The part `whole.field` moves into, when a split moves it.
    pub fn moved_to(&self, whole: &str, field: &str) -> Option<&EntityPart> {
        self.parts
            .iter()
            .find(|p| p.whole == whole && p.fields.iter().any(|f| f == field))
    }

    /// The move `entity.field` is a moved field of.
    pub fn move_to(&self, entity: &str, field: &str) -> Option<&EntityMove> {
        self.moves
            .iter()
            .find(|m| m.entity == entity && m.fields.iter().any(|(f, _)| f == field))
    }

    /// The move that takes `from.field` into another entity.
    pub fn moved_from(&self, from: &str, field: &str) -> Option<&EntityMove> {
        self.moves
            .iter()
            .find(|m| m.from == from && m.fields.iter().any(|(_, old)| old == field))
    }

    /// The entity merge that takes `from` into another entity.
    pub fn merged_into(&self, from: &str) -> Option<&EntityMove> {
        self.moves.iter().find(|m| m.merge && m.from == from)
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
            None => entity_mark(prior, declared, m, &mut out),
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
            // A part split off an entity still declared.
            None if declared.native(&m.prev[0]).is_some() => {
                r.parts.push(EntityPart {
                    part: m.entity.clone(),
                    whole: m.prev[0].clone(),
                    fields: declared
                        .native(&m.entity)
                        .map(|e| e.declared().map(|f| f.name.clone()).collect())
                        .unwrap_or_default(),
                });
            }
            None => {
                r.entities.insert(m.prev[0].clone(), m.entity.clone());
            }
            Some(f) if m.prev[0].contains('.') => {
                let Some((from, old)) = m.prev[0].split_once('.') else {
                    continue;
                };
                let merge = declared.native(from).is_none();
                match r
                    .moves
                    .iter_mut()
                    .find(|x| x.entity == m.entity && x.from == from)
                {
                    Some(x) => x.fields.push((f.clone(), old.to_string())),
                    None => r.moves.push(EntityMove {
                        entity: m.entity.clone(),
                        from: from.to_string(),
                        fields: vec![(f.clone(), old.to_string())],
                        merge,
                    }),
                }
            }
            Some(f) => {
                // A field of a renamed entity is renamed within it: the old
                // name is looked up under the entity's old name.
                let from_entity = entity_in_prior(prior, declared, &m.entity)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| m.entity.clone());
                if from_entity != m.entity {
                    continue;
                }
                let Some(was) = prior.native(&m.entity) else {
                    continue;
                };
                let split = split_of(declared, m);
                let cast = declared
                    .native(&m.entity)
                    .is_some_and(|e| is_cast(was, e, f, m));
                if m.prev.len() > 1 || split.len() > 1 || cast {
                    if r.map_to(&m.entity, f).is_some() {
                        continue;
                    }
                    let to = if split.len() > 1 {
                        split.iter().filter_map(|x| x.field.clone()).collect()
                    } else {
                        vec![f.clone()]
                    };
                    r.maps.push(FieldMap {
                        entity: m.entity.clone(),
                        from: m
                            .prev
                            .iter()
                            .filter_map(|p| was.field(p).cloned())
                            .collect(),
                        to,
                    });
                    continue;
                }
                r.fields
                    .insert((m.entity.clone(), m.prev[0].clone()), f.clone());
            }
        }
    }
    r
}

/// The field marks of `m`'s entity that split the same old name as `m`,
/// `m` among them, in declaration order. One mark is a rename; a merge or a
/// move is never a split.
fn split_of<'a>(declared: &'a Schema, m: &PrevMark) -> Vec<&'a PrevMark> {
    if m.field.is_none() || m.prev.len() != 1 || m.prev[0].contains('.') {
        return Vec::new();
    }
    declared
        .prev
        .iter()
        .filter(|x| x.entity == m.entity && x.field.is_some() && x.prev == m.prev)
        .collect()
}

/// Whether `m`, one old field renamed to `f`, changes its type: a cast,
/// which the plan maps like a split or a merge.
fn is_cast(was: &Entity, e: &Entity, f: &str, m: &PrevMark) -> bool {
    if m.prev.len() != 1 || m.prev[0].contains('.') {
        return false;
    }
    let (Some(from), Some(to)) = (was.field(&m.prev[0]), e.field(f)) else {
        return false;
    };
    let ty = |x: &crate::Field| (crate::ddl::column_ty(x), x.text_max);
    ty(from) != ty(to) || from.ty != to.ty
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

fn entity_mark(prior: &Schema, declared: &Schema, m: &PrevMark, out: &mut PrevReport) {
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
        return;
    }
    // Several entities from one that is gone: a split with no whole. Only
    // a split whose whole keeps its name is built.
    let claims: Vec<&str> = declared
        .prev
        .iter()
        .filter(|x| x.field.is_none() && x.prev == m.prev)
        .map(|x| x.entity.as_str())
        .collect();
    if declared.native(old).is_none() && claims.len() > 1 {
        out.errors.push(PrevIssue {
            class: "entity_split_needs_whole",
            mark: m.clone(),
            message: format!(
                "{} each claim `{old}`, which is gone. A split keeps the entity it splits: \
                 its rows stay where they are, and each part takes one row per row",
                claims
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            fix: format!(
                "keep `{old}` declared as one of the parts (rename it in a version of its own), \
                 and mark only the others `(prev: {old})`"
            ),
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
    if m.prev.iter().any(|p| p.contains('.')) {
        move_mark(prior, declared, was, e, f, m, out);
        return;
    }
    if m.prev.len() > 1 || split_of(declared, m).len() > 1 || is_cast(was, e, f, m) {
        map_mark(was, e, f, m, out);
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
    // While both versions write, each one's insert names only its own
    // column, and the dual writes set the other in the same transaction.
    // A required column the insert does not name needs a default to be
    // taken first: the new one its own, the old one its own or the new
    // field's.
    if to.default.is_none() && (!to.optional || (!from.optional && from.default.is_none())) {
        out.errors.push(PrevIssue {
            class: "prev_default_required",
            mark: m.clone(),
            message: format!(
                "`{}.{f}` renames a required field and declares no default. While both \
                 versions write, an insert by one leaves the other's column to its default \
                 until the dual writes set it in the same write",
                e.name
            ),
            fix: format!(
                "give `{f}` a default: `{f}: {} = <value> prev({old})`",
                type_text(to)
            ),
        });
    }
}

/// A move's mark, `prev(other.field)`: one field of an entity served now,
/// moved into an entity that keeps its name, from an entity whose moves
/// all go to it. The rule and the shape are the stored plan's
/// (`--correspond`), checked by the triage.
fn move_mark(
    prior: &Schema,
    declared: &Schema,
    was: &Entity,
    e: &Entity,
    f: &str,
    m: &PrevMark,
    out: &mut PrevReport,
) {
    let refuse = |out: &mut PrevReport, message: String, fix: String| {
        out.errors.push(PrevIssue {
            class: "prev_mapping_required",
            mark: m.clone(),
            message,
            fix,
        })
    };
    let written = format!("`{}.{f} prev({})`", e.name, m.prev.join(", "));
    let (from, old) = match (m.prev.as_slice(), m.prev[0].split_once('.')) {
        ([_], Some(pair)) => pair,
        _ => {
            return refuse(
                out,
                format!(
                    "{written} computes one field from several fields, some of another entity. \
                     A mapping over two entities is not built"
                ),
                format!(
                    "move the fields into `{}` first (`prev(<entity>.<field>)` each), and merge \
                     them in the next version",
                    e.name
                ),
            )
        }
    };
    if from == e.name {
        return refuse(
            out,
            format!("{written} names its own entity: that is a rename, `prev({old})`"),
            format!("write `prev({old})`"),
        );
    }
    if was.name != e.name {
        return refuse(
            out,
            format!(
                "{written} moves a field into `{}`, which this step also renames from `{}`. A \
                 move into a renamed entity is not built",
                e.name, was.name
            ),
            "rename the entity in one version, and move the field in the next".into(),
        );
    }
    let Some(source) = prior.native(from) else {
        out.errors.push(PrevIssue {
            class: "prev_unresolved",
            mark: m.clone(),
            message: format!(
                "the version serving now has no `{from}`. `prev` reaches one version back only"
            ),
            fix: format!(
                "name an entity of the version serving now, or remove `prev({from}.{old})`"
            ),
        });
        return;
    };
    if source.field(old).is_none() {
        out.errors.push(PrevIssue {
            class: "prev_unresolved",
            mark: m.clone(),
            message: format!(
                "the version serving now has no `{from}.{old}`. `prev` reaches one version back \
                 only"
            ),
            fix: format!("name a field of `{from}` that is served now, or remove the mark"),
        });
        return;
    }
    // The field keeps its name: the dual writes copy a column into the
    // column of the same name.
    if old != f {
        out.errors.push(PrevIssue {
            class: "move_renamed",
            mark: m.clone(),
            message: format!(
                "{written} renames the field as it moves. A move keeps the name, so both \
                 versions write one column name in each entity"
            ),
            fix: format!(
                "write `{old}: ... prev({from}.{old})` in this version, and rename it to `{f}` \
                 in the next"
            ),
        });
        return;
    }
    // Every move out of one entity goes to one entity.
    let targets: std::collections::BTreeSet<&str> = declared
        .prev
        .iter()
        .filter(|x| {
            x.field.is_some()
                && x.prev.len() == 1
                && x.prev[0].split_once('.').map(|(s, _)| s) == Some(from)
        })
        .map(|x| x.entity.as_str())
        .collect();
    if targets.len() > 1 {
        refuse(
            out,
            format!(
                "fields of `{from}` move into {}. Moves out of one entity go to one entity in a \
                 step",
                targets
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            "move them into one entity in this version, and the rest in the next".into(),
        );
    }
}

/// A split's or a merge's mark: each old field is served now, and no old
/// or new field is sealed or a file. Its mapping is the plan's.
fn map_mark(was: &Entity, e: &Entity, f: &str, m: &PrevMark, out: &mut PrevReport) {
    for old in &m.prev {
        let Some(from) = was.field(old) else {
            out.errors.push(PrevIssue {
                class: "prev_unresolved",
                mark: m.clone(),
                message: format!(
                    "the version serving now has no `{}.{old}`. `prev` reaches one version \
                     back only",
                    was.name
                ),
                fix: format!(
                    "name fields of `{}` that are served now, or remove `prev({})`",
                    was.name,
                    m.prev.join(", ")
                ),
            });
            return;
        };
        let to = e.field(f);
        let sealed = from.sealed || to.is_some_and(|t| t.sealed);
        let file = from.is_content() || to.is_some_and(|t| t.is_content());
        if sealed || file {
            out.errors.push(PrevIssue {
                class: if sealed { "prev_sealed" } else { "prev_type" },
                mark: m.clone(),
                message: format!(
                    "`{}.{f} prev({})` maps {}: a split, a merge or a type change computes \
                     values with an expression over the row, and {}",
                    e.name,
                    m.prev.join(", "),
                    if sealed { "a sealed field" } else { "a file" },
                    if sealed {
                        "the database holds only ciphertext"
                    } else {
                        "a file's handle is minted by an upload, never computed"
                    }
                ),
                fix: "keep the fields as they are, or split them in a version before the \
                      one that seals them"
                    .into(),
            });
            return;
        }
    }
}

fn type_text(f: &crate::Field) -> String {
    let base = crate::ddl::column_ty(f);
    match f.text_max {
        Some(n) => format!("text({n})"),
        None => base,
    }
}
