//! The schema diff — increment 1 of `plan-schema-evolution.md`.
//!
//! `ddl_for` emits a whole schema from scratch. This module answers the
//! other question: given a database that already holds rows and a `.ordo`
//! program that has moved on, **what changed, and which class of change is
//! it?**
//!
//! # Why this diffs `Schema` values and not artifact JSON
//!
//! The obvious implementation compares two artifact documents — they are
//! byte-stable and already exist. It would be wrong, and the plan's defect
//! (a) is why: until this increment the artifact document carried no field
//! `default`, no `normalize`, no `enum_values`, no composite `unique_keys`
//! and no declared `indexes`. Those five are exactly the declarations that
//! change the DDL without changing an op's contract, which makes them the
//! ones a deploy diff most needs to see. The document now carries them, and
//! this module still diffs `Schema` values, because a `Schema` is *typed*:
//! a `FieldTy::Ref` knows its target, an `enum_values` is a list rather than
//! a rendered `CHECK`, and a class decision made against `Option<Expr>` is
//! made against the thing `ddl_for` reads rather than against a rendering of
//! it.
//!
//! # The three inputs, and why the middle one is not optional in spirit
//!
//! ```text
//! diff(live, prior, declared)
//!       │      │        └─ the program being deployed
//!       │      └─ the program the deployment is serving now (its artifact's
//!       │         source, recompiled) — `None` when nothing is deployed
//!       └─ what `information_schema` and the catalog say the database holds
//! ```
//!
//! `live` alone cannot decide a **drop**. The live schema holds the
//! platform's own tables (`$commandlog`, `$artifacts`, `$sealedpayload`,
//! `$jobs`), tables from a half-finished expand-contract, and whatever else
//! a database accumulates; `Pool::live_drift` is one-way for precisely that
//! reason, and a diff that read "live has a table the program does not
//! declare" as *drop this table* would propose destroying the audit log on
//! the first run. So a live object is only ever reported as dropped when
//! **something declared it**: `prior` is the record of what did. With no
//! prior artifact there are no drops, and the plan says so rather than
//! guessing.
//!
//! # What a live shape cannot see, stated rather than discovered
//!
//! - **A rename** (§5 B4) is a drop and an add to anything that reads
//!   columns, and it stays that way here. Distinguishing them needs a
//!   declared mapping, which is what B4 requires; guessing from a type match
//!   would silently propose copying one column into another.
//! - **`normalize(lower)`** (§5 B10) touches no column and no constraint —
//!   it changes the plaintext that is HMACed into a blind index. It is
//!   invisible to `information_schema` and is found in the `prior`
//!   comparison, which is possible only because defect (a)'s fix put
//!   `normalize` into the artifact document that carries `prior`'s source.
//! - **The typed-id `key` prefix** (§5 D4) is a schema fact with no physical
//!   column. Same answer, same reason.

use std::collections::{BTreeMap, BTreeSet};

use crate::{ddl, Entity, Field, FieldTy, Schema};

// ── the live side ────────────────────────────────────────────────────────

/// One column, as the database describes it.
///
/// Deliberately three fields and not the six `live_drift` compares.
/// Classification asks different questions from drift detection: a
/// collation difference is drift and is nobody's *migration class*, while
/// `has_default` decides nothing here but tells an operator whether a
/// `DEFAULT` still has to be attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveColumn {
    /// Postgres' own spelling, from `format_type` — `timestamp with time
    /// zone`, not `timestamptz`. [`same_type`] owns the reconciliation.
    pub ty: String,
    pub nullable: bool,
    pub has_default: bool,
}

/// One table: its columns, and the NAMES of its constraints and indexes.
///
/// Names rather than definitions, because `ddl_for` names every object it
/// emits deterministically (`orders$total$uq`, `orders$status$enum`,
/// `orders$customer$fk`, `orders$a$b$ix`). A name is therefore a complete
/// answer to *is this declaration deployed?*, which is the only question
/// classification asks. Whether a deployed object still MEANS what the
/// artifact says is `Pool::live_drift`'s question, and it compares
/// definitions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTable {
    pub columns: BTreeMap<String, LiveColumn>,
    pub constraints: BTreeSet<String>,
    pub indexes: BTreeSet<String>,
}

/// What one schema of a live database holds.
///
/// Built by `ordo_exec::migrate::live_shape`; a plain map here so that
/// `ordo-schema` — which has no database and must not gain one — can own the
/// classification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveShape {
    pub tables: BTreeMap<String, LiveTable>,
    /// The places the database records its stored amounts at, per
    /// `(entity, field)`: the latest `$places` row, written in the same
    /// transaction as the places change it records (25aa). Empty for a
    /// field no places change has touched; the declared places of the
    /// version the data was written under then decide.
    pub places: BTreeMap<(String, String), u8>,
}

impl LiveShape {
    pub fn table(&self, name: &str) -> Option<&LiveTable> {
        self.tables.get(name)
    }

    /// Builder used by the reader in `ordo-exec`.
    pub fn column(&mut self, table: &str, column: &str, c: LiveColumn) {
        self.tables
            .entry(table.to_string())
            .or_default()
            .columns
            .insert(column.to_string(), c);
    }

    /// Builder: record a constraint by name. `constraint` is `true` for a
    /// `pg_constraint` row and `false` for a free-standing index.
    pub fn object(&mut self, table: &str, name: &str, constraint: bool) {
        let t = self.tables.entry(table.to_string()).or_default();
        if constraint {
            t.constraints.insert(name.to_string());
        } else {
            t.indexes.insert(name.to_string());
        }
    }
}

impl LiveTable {
    fn has_object(&self, name: &str) -> bool {
        self.constraints.contains(name) || self.indexes.contains(name)
    }
}

/// Does the type the database reports satisfy the type `ddl_for` would
/// emit?
///
/// A mapping, not string equality, for the same reason `shape::accepts` is
/// one: `timestamptz` and `timestamp with time zone` are one type spelled
/// two ways, and reporting that as a type change would put every deployment
/// permanently in class B.
pub fn same_type(declared: &str, live: &str) -> bool {
    let canon = |t: &str| {
        match t.split('(').next().unwrap_or(t).trim() {
            "timestamptz" | "timestamp with time zone" => "timestamptz",
            "timestamp" | "timestamp without time zone" => "timestamp",
            "int8" | "bigint" => "bigint",
            "int4" | "integer" => "integer",
            "bool" | "boolean" => "boolean",
            "varchar" | "character varying" => "varchar",
            other => other,
        }
        .to_string()
    };
    canon(declared) == canon(live)
}

// ── the taxonomy ─────────────────────────────────────────────────────────

/// `plan-schema-evolution.md` §5's four classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Additive: auto-applies, no barrier, no probe.
    A,
    /// Transforming: declared mapping, write barrier, backfill, verify.
    B,
    /// Constraint: probe first, and a non-empty probe is a refusal.
    C,
    /// Destructive or refused: its own deploy and its own confirmation.
    D,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::A => "A",
            Class::B => "B",
            Class::C => "C",
            Class::D => "D",
        }
    }
}

impl std::fmt::Display for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One change, at the grain the taxonomy rules on.
///
/// A closed enum on purpose. The diff must be **total** over what the two
/// sides can differ by — a change with no variant is a change `ordo plan`
/// prints nothing about, which is defect (a) again in a new place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    // ── A ──
    AddEntity {
        entity: String,
    },
    AddOptionalField {
        entity: String,
        field: String,
    },
    AddConstantField {
        entity: String,
        field: String,
        default: String,
    },
    AddIndex {
        entity: String,
        object: String,
    },
    DropIndex {
        entity: String,
        object: String,
    },
    // ── B ──
    AddExecutionTimeField {
        entity: String,
        field: String,
        default: String,
    },
    AddRequiredField {
        entity: String,
        field: String,
    },
    ChangeType {
        entity: String,
        field: String,
        live: String,
        declared: String,
    },
    RequireField {
        entity: String,
        field: String,
    },
    RelaxField {
        entity: String,
        field: String,
    },
    SealField {
        entity: String,
        field: String,
    },
    IndexSealedField {
        entity: String,
        field: String,
    },
    MakeTree {
        entity: String,
    },
    Normalize {
        entity: String,
        field: String,
        from: Option<String>,
        to: Option<String>,
    },
    /// A stored `money(p)` or `decimal(p)` field whose declared places
    /// differ from the places its stored amounts are held at (25aa).
    ///
    /// Invisible to the column type for money: every `money(p)` is
    /// `bigint` minor units, so `money(0)` to `money(2)` reads every stored
    /// amount 100 times smaller with no DDL at all. Found from the declared
    /// places of the version the data was written under, or from `$places`
    /// once a places change has been applied. Never applied without a
    /// stated [`PlacesChoice`].
    ChangePlaces {
        entity: String,
        field: String,
        /// `money` when true, `decimal` when false.
        money: bool,
        from: u8,
        to: u8,
    },
    // ── C ──
    AddUnique {
        entity: String,
        field: String,
        object: String,
        /// The column the constraint actually lands on: a sealed field's
        /// uniqueness rides its blind index, and a probe over the payload
        /// would compare ciphertext.
        column: String,
    },
    AddCompositeUnique {
        entity: String,
        key: Vec<String>,
        object: String,
    },
    AddEnum {
        entity: String,
        field: String,
        values: Vec<String>,
        object: String,
    },
    AddForeignKey {
        entity: String,
        field: String,
        target: String,
        object: String,
    },
    DropUnique {
        entity: String,
        field: String,
        object: String,
    },
    DropCompositeUnique {
        entity: String,
        object: String,
    },
    DropEnum {
        entity: String,
        field: String,
        object: String,
    },
    /// A row invariant (`check`) the program declares and the live table
    /// does not carry. Class C: the live rows decide, by probe, and the
    /// constraint name carries the condition (`check.rs`).
    AddCheck {
        entity: String,
        object: String,
        /// The condition as the author wrote it, for the report.
        source: String,
        /// The rendered CHECK expression, which is what the probe negates.
        sql: String,
    },
    /// A row invariant the prior program declared, this one does not, and
    /// the live table still carries. Rows that violate the old rule
    /// accumulate the instant it goes, so re-adding it later is a C9 that
    /// will usually fail: the same one-way door as C6.
    DropCheck {
        entity: String,
        object: String,
    },
    // ── D ──
    DropField {
        entity: String,
        field: String,
    },
    DropEntity {
        entity: String,
    },
    ChangeKey {
        entity: String,
        from: String,
        to: String,
    },
    UnsealField {
        entity: String,
        field: String,
    },
    UnindexSealedField {
        entity: String,
        field: String,
    },
    UnmakeTree {
        entity: String,
    },
    DropForeignKey {
        entity: String,
        object: String,
    },
}

impl Change {
    /// The taxonomy row this change is ruled by — `A2`, `B8`, `C1`, `D1`.
    /// Printed beside every line of the plan so an operator can look the
    /// ruling up rather than take the tool's word for it.
    pub fn code(&self) -> &'static str {
        match self {
            Change::AddEntity { .. } => "A1",
            Change::AddOptionalField { .. } => "A2",
            Change::AddConstantField { .. } => "A3",
            Change::AddIndex { .. } => "A4",
            Change::DropIndex { .. } => "A7",
            Change::AddExecutionTimeField { .. } => "B1",
            Change::AddRequiredField { .. } => "B2",
            Change::ChangeType { .. } => "B3",
            Change::RequireField { .. } => "B5",
            Change::RelaxField { .. } => "B6",
            Change::SealField { .. } => "B7",
            Change::IndexSealedField { .. } => "B8",
            Change::MakeTree { .. } => "B9",
            Change::Normalize { .. } => "B10",
            Change::ChangePlaces { .. } => "B11",
            Change::AddUnique { .. } => "C1",
            Change::AddCompositeUnique { .. } => "C2",
            Change::AddEnum { .. } => "C3",
            Change::AddForeignKey { .. } => "C5",
            Change::DropUnique { .. } => "C6",
            Change::DropCompositeUnique { .. } => "C7",
            Change::DropEnum { .. } => "C8",
            Change::AddCheck { .. } => "C9",
            Change::DropCheck { .. } => "C10",
            Change::DropField { .. } => "D1",
            Change::DropEntity { .. } => "D2",
            Change::ChangeKey { .. } => "D4",
            Change::UnsealField { .. } => "D6",
            Change::UnindexSealedField { .. } => "D8",
            Change::UnmakeTree { .. } => "D9",
            Change::DropForeignKey { .. } => "D10",
        }
    }

    pub fn class(&self) -> Class {
        match self.code().as_bytes()[0] {
            b'A' => Class::A,
            b'B' => Class::B,
            b'C' => Class::C,
            _ => Class::D,
        }
    }

    /// The entity every change is about. There is no schema-level change
    /// that is about no entity, which is why this is total.
    pub fn entity(&self) -> &str {
        match self {
            Change::AddEntity { entity }
            | Change::AddOptionalField { entity, .. }
            | Change::AddConstantField { entity, .. }
            | Change::AddIndex { entity, .. }
            | Change::DropIndex { entity, .. }
            | Change::AddExecutionTimeField { entity, .. }
            | Change::AddRequiredField { entity, .. }
            | Change::ChangeType { entity, .. }
            | Change::RequireField { entity, .. }
            | Change::RelaxField { entity, .. }
            | Change::SealField { entity, .. }
            | Change::IndexSealedField { entity, .. }
            | Change::MakeTree { entity }
            | Change::Normalize { entity, .. }
            | Change::ChangePlaces { entity, .. }
            | Change::AddUnique { entity, .. }
            | Change::AddCompositeUnique { entity, .. }
            | Change::AddEnum { entity, .. }
            | Change::AddForeignKey { entity, .. }
            | Change::DropUnique { entity, .. }
            | Change::DropCompositeUnique { entity, .. }
            | Change::DropEnum { entity, .. }
            | Change::AddCheck { entity, .. }
            | Change::DropCheck { entity, .. }
            | Change::DropField { entity, .. }
            | Change::DropEntity { entity }
            | Change::ChangeKey { entity, .. }
            | Change::UnsealField { entity, .. }
            | Change::UnindexSealedField { entity, .. }
            | Change::UnmakeTree { entity }
            | Change::DropForeignKey { entity, .. } => entity,
        }
    }

    /// The field, where the change is about one.
    pub fn field(&self) -> Option<&str> {
        match self {
            Change::AddOptionalField { field, .. }
            | Change::AddConstantField { field, .. }
            | Change::AddExecutionTimeField { field, .. }
            | Change::AddRequiredField { field, .. }
            | Change::ChangeType { field, .. }
            | Change::RequireField { field, .. }
            | Change::RelaxField { field, .. }
            | Change::SealField { field, .. }
            | Change::IndexSealedField { field, .. }
            | Change::Normalize { field, .. }
            | Change::ChangePlaces { field, .. }
            | Change::AddUnique { field, .. }
            | Change::AddEnum { field, .. }
            | Change::AddForeignKey { field, .. }
            | Change::DropUnique { field, .. }
            | Change::DropEnum { field, .. }
            | Change::DropField { field, .. }
            | Change::UnsealField { field, .. }
            | Change::UnindexSealedField { field, .. } => Some(field),
            _ => None,
        }
    }

    /// Refused outright in v1, with the ruling that refuses it.
    ///
    /// A refusal is a class, not prose: `ordo plan` prints the name so the
    /// refusal is greppable and so two spellings of one refusal cannot
    /// arise. See §5 D6 and §5 D5.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            // §5 D6. Unsealing moves values permanently into plaintext
            // columns, WAL and backups; re-sealing does not undo that.
            Change::UnsealField { .. } => Some("unseal_refused"),
            // §5 D4. The key is minted into every id ever issued and stored
            // as text in every ref column that names one.
            Change::ChangeKey { .. } => Some("key_change_refused"),
            _ => None,
        }
    }

    /// Would activating an artifact that declares this change, before the
    /// change has been applied, make the load gate refuse it?
    ///
    /// `Pool::live_drift` is one-way: every column, constraint and index
    /// the artifact asserts must exist live, with the same definition;
    /// extra live objects are not drift. So a change that ADDS something,
    /// or changes a column's definition, blocks activation until the DDL
    /// has run (DDL first, then the artifact), while a DROP is the
    /// expand-contract state the one-way rule exists for and sequences the
    /// other way round (artifact first, then the DDL).
    /// `plan-schema-evolution.md` §4 and §8.1.
    ///
    /// The two declaration-only changes (`normalize`, the typed-id key)
    /// have no physical object for the gate to look for.
    pub fn gate_blocking(&self) -> bool {
        match self {
            Change::AddEntity { .. }
            | Change::AddOptionalField { .. }
            | Change::AddConstantField { .. }
            | Change::AddIndex { .. }
            | Change::AddExecutionTimeField { .. }
            | Change::AddRequiredField { .. }
            | Change::ChangeType { .. }
            | Change::RequireField { .. }
            | Change::RelaxField { .. }
            | Change::SealField { .. }
            | Change::IndexSealedField { .. }
            | Change::MakeTree { .. }
            // The serving program reads the stored amounts at its places;
            // a program at other places must wait until they are moved.
            | Change::ChangePlaces { .. }
            | Change::AddUnique { .. }
            | Change::AddCompositeUnique { .. }
            | Change::AddEnum { .. }
            | Change::AddForeignKey { .. }
            | Change::AddCheck { .. }
            | Change::UnsealField { .. } => true,
            Change::DropIndex { .. }
            | Change::Normalize { .. }
            | Change::DropUnique { .. }
            | Change::DropCompositeUnique { .. }
            | Change::DropEnum { .. }
            | Change::DropCheck { .. }
            | Change::DropField { .. }
            | Change::DropEntity { .. }
            | Change::ChangeKey { .. }
            | Change::UnindexSealedField { .. }
            | Change::UnmakeTree { .. }
            | Change::DropForeignKey { .. } => false,
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let e = self.entity();
        match self {
            Change::AddEntity { .. } => write!(f, "new entity `{e}`"),
            Change::AddOptionalField { field, .. } => {
                write!(f, "new optional field `{e}.{field}`")
            }
            Change::AddConstantField { field, default, .. } => write!(
                f,
                "new required field `{e}.{field}` with the constant default `{default}`"
            ),
            Change::AddIndex { object, .. } => write!(f, "new index `{object}` on `{e}`"),
            Change::DropIndex { object, .. } => write!(f, "index `{object}` on `{e}` dropped"),
            Change::AddExecutionTimeField { field, default, .. } => write!(
                f,
                "new required field `{e}.{field}` defaulted at execution time (`{default}`)"
            ),
            Change::AddRequiredField { field, .. } => {
                write!(f, "new required field `{e}.{field}` with no default")
            }
            Change::ChangeType {
                field,
                live,
                declared,
                ..
            } => write!(
                f,
                "`{e}.{field}` changes type: the database holds `{live}`, the program declares \
                 `{declared}`"
            ),
            Change::RequireField { field, .. } => {
                write!(f, "`{e}.{field}` becomes required")
            }
            Change::RelaxField { field, .. } => {
                write!(f, "`{e}.{field}` becomes optional")
            }
            Change::SealField { field, .. } => write!(f, "`{e}.{field}` becomes sealed"),
            Change::IndexSealedField { field, .. } => {
                write!(f, "sealed `{e}.{field}` gains a blind index")
            }
            Change::MakeTree { .. } => write!(f, "`{e}` becomes a tree"),
            Change::Normalize {
                field, from, to, ..
            } => write!(
                f,
                "`{e}.{field}`'s normalization moves from {} to {}",
                from.as_deref().unwrap_or("none"),
                to.as_deref().unwrap_or("none")
            ),
            Change::ChangePlaces {
                field,
                money,
                from,
                to,
                ..
            } => {
                let ty = if *money { "money" } else { "decimal" };
                write!(
                    f,
                    "`{e}.{field}` changes places: the stored amounts are held at {ty}({from}), \
                     the program declares {ty}({to})"
                )
            }
            Change::AddUnique { field, .. } => write!(f, "`{e}.{field}` becomes unique"),
            Change::AddCompositeUnique { key, .. } => {
                write!(f, "new unique key `{e} ({})`", key.join(", "))
            }
            Change::AddEnum { field, values, .. } => write!(
                f,
                "`{e}.{field}` is pinned to the value set ({})",
                values.join(", ")
            ),
            Change::AddForeignKey { field, target, .. } => {
                write!(f, "`{e}.{field}` gains its reference to `{target}`")
            }
            Change::DropUnique { field, .. } => {
                write!(f, "`{e}.{field}` is no longer unique")
            }
            Change::DropCompositeUnique { object, .. } => {
                write!(f, "unique key `{object}` on `{e}` dropped")
            }
            Change::DropEnum { field, .. } => {
                write!(f, "`{e}.{field}`'s value-set pin dropped")
            }
            Change::AddCheck { source, .. } => {
                write!(f, "new row invariant on `{e}`: check {source}")
            }
            Change::DropCheck { object, .. } => {
                write!(f, "row invariant `{object}` on `{e}` dropped")
            }
            Change::DropField { field, .. } => write!(f, "field `{e}.{field}` dropped"),
            Change::DropEntity { .. } => write!(f, "entity `{e}` dropped"),
            Change::ChangeKey { from, to, .. } => {
                write!(f, "`{e}`'s typed-id key changes from `{from}` to `{to}`")
            }
            Change::UnsealField { field, .. } => write!(f, "`{e}.{field}` is unsealed"),
            Change::UnindexSealedField { field, .. } => {
                write!(f, "sealed `{e}.{field}` loses its blind index")
            }
            Change::UnmakeTree { .. } => write!(f, "`{e}` stops being a tree"),
            Change::DropForeignKey { object, .. } => {
                write!(f, "foreign key `{object}` on `{e}` dropped")
            }
        }
    }
}

// ── object names, exactly as `ddl_for` spells them ───────────────────────

pub fn unique_name(entity: &str, field: &str) -> String {
    format!("{entity}${field}$uq")
}
pub fn composite_unique_name(entity: &str, key: &[String]) -> String {
    format!("{entity}${}$uq", key.join("$"))
}
pub fn enum_name(entity: &str, field: &str) -> String {
    format!("{entity}${field}$enum")
}
pub fn fk_name(entity: &str, field: &str) -> String {
    format!("{entity}${field}$fk")
}
pub fn index_name(entity: &str, key: &[String]) -> String {
    format!("{entity}${}$ix", key.join("$"))
}

/// A field's default, rendered canonically and totally.
///
/// `ddl::literal` refuses anything that is not a literal, because a
/// non-literal default is execution-time behaviour and cannot be a column
/// default. Classification still has to *name* it — `= today` is exactly
/// what separates §5 A3 from §5 B1 — so this renders every `Expr` and never
/// fails. It is also what the artifact document carries, so it must be
/// byte-stable: no spans, no float formatting, no environment.
pub fn render_default(e: &ordo_syntax::Expr) -> String {
    use ordo_syntax::Expr as E;
    match e {
        E::IntLit { value, .. } => value.to_string(),
        E::DecimalLit { digits, .. } => digits.clone(),
        E::StrLit { value, .. } => format!("{value:?}"),
        E::BoolLit { value, .. } => value.to_string(),
        E::NullLit { .. } => "null".into(),
        E::DateLit { y, m, d, .. } => format!("{y:04}-{m:02}-{d:02}"),
        E::DatetimeLit {
            y,
            m,
            d,
            hh,
            mm,
            ss,
            ..
        } => format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z"),
        E::DurationLit { n, unit, .. } => format!("{n}{}", unit.as_str()),
        E::Param { name, .. } => format!("${name}"),
        E::Path { segments, .. } => segments
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join("."),
        E::Neg { expr, .. } => format!("-{}", render_default(expr)),
        E::Binary { op, lhs, rhs, .. } => format!(
            "({} {} {})",
            render_default(lhs),
            op.as_str(),
            render_default(rhs)
        ),
        E::FnCall { name, args, .. } => format!(
            "{}({})",
            name.name,
            args.iter()
                .map(render_default)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // `cond` cannot be a column default and is not a declaration form
        // for one; rendered rather than refused so this stays total.
        E::Cond { arms, else_, .. } => format!(
            "cond({} arm(s), else {})",
            arms.len(),
            render_default(else_)
        ),
    }
}

/// Is this default a literal — the thing Postgres can store as a column
/// default and back-fill for free (§5 A3) — or execution-time behaviour
/// that has to be resolved per row (§5 B1)?
fn is_literal_default(e: &ordo_syntax::Expr) -> bool {
    ddl::literal(e).is_ok()
}

// ── the diff ─────────────────────────────────────────────────────────────

/// Classify every difference between what the database holds, what the
/// deployment declared last, and what the program declares now.
///
/// The result is in a fixed order — entity declaration order, then field
/// declaration order, then constraints, then drops — because a report whose
/// lines move between runs cannot be diffed and cannot be approved twice.
pub fn diff(live: &LiveShape, prior: Option<&Schema>, declared: &Schema) -> Vec<Change> {
    diff_with(live, prior, prior, declared)
}

/// [`diff`], with the places of the stored amounts read from a version of
/// its own (25aa).
///
/// `written` is the program the data was written under. It is usually
/// `prior`, and differs for a tenant's database: it holds no registry, so
/// it has no `prior` and its dropped fields stay where they are, while its
/// stored amounts are still at the places of the version it records.
/// `live.places`, where a field has a row, outranks both.
pub fn diff_with(
    live: &LiveShape,
    prior: Option<&Schema>,
    written: Option<&Schema>,
    declared: &Schema,
) -> Vec<Change> {
    let mut out = Vec::new();

    for e in &declared.entities {
        let Some(t) = live.table(&e.name) else {
            // The whole table is missing: one line, not one per column.
            // An operator reading "new entity `invoices`" does not also
            // need seventeen "new field" lines about a table that does not
            // exist yet.
            out.push(Change::AddEntity {
                entity: e.name.clone(),
            });
            continue;
        };
        let written_as = written.and_then(|w| w.entity(&e.name));
        entity_columns(e, t, written_as, &live.places, &mut out);
        entity_objects(e, t, &mut out);
        if let Some(p) = prior.and_then(|p| p.entity(&e.name)) {
            declaration_only(p, e, &mut out);
        }
    }

    // Drops, and ONLY against a prior declaration — see the module doc.
    if let Some(prior) = prior {
        for p in &prior.entities {
            if declared.entity(&p.name).is_some() {
                continue;
            }
            if live.table(&p.name).is_some() {
                out.push(Change::DropEntity {
                    entity: p.name.clone(),
                });
            }
        }
        for p in &prior.entities {
            let Some(d) = declared.entity(&p.name) else {
                continue;
            };
            let Some(t) = live.table(&p.name) else {
                continue;
            };
            for f in p.declared() {
                match d.field(&f.name) {
                    None => {
                        if t.columns.contains_key(&f.column()) {
                            out.push(Change::DropField {
                                entity: p.name.clone(),
                                field: f.name.clone(),
                            });
                        }
                        // The blind index is a second column, so a dropped
                        // `sealed indexed` field drops two.
                        if f.indexed && t.columns.contains_key(&f.index_column()) {
                            out.push(Change::UnindexSealedField {
                                entity: p.name.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                    // The field stays and its blind index does not. This is
                    // the direction `entity_columns` cannot see: it looks for
                    // an index column the declaration wants and the database
                    // lacks (`B8`), and the reverse — a `$bidx` the database
                    // holds and the declaration has stopped wanting — is a
                    // live column no artifact asserts. Extra live columns are
                    // not drift, by the one-way rule, so nothing else in the
                    // system would ever mention it again.
                    Some(now) => {
                        if f.indexed && !now.indexed && t.columns.contains_key(&f.index_column()) {
                            out.push(Change::UnindexSealedField {
                                entity: p.name.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                }
            }
            if p.is_tree && !d.is_tree && t.columns.contains_key("$path") {
                out.push(Change::UnmakeTree {
                    entity: p.name.clone(),
                });
            }
        }
        // Constraints and indexes the prior program declared, the new one
        // does not, and the database still holds.
        for p in &prior.entities {
            let Some(d) = declared.entity(&p.name) else {
                continue;
            };
            let Some(t) = live.table(&p.name) else {
                continue;
            };
            dropped_objects(p, d, t, &mut out);
        }
    }

    out
}

/// Column-grain classification for one entity that exists on both sides.
fn entity_columns(
    e: &Entity,
    t: &LiveTable,
    written: Option<&Entity>,
    held: &BTreeMap<(String, String), u8>,
    out: &mut Vec<Change>,
) {
    for f in e.declared() {
        match t.columns.get(&f.column()) {
            None => out.push(added_field(e, f)),
            Some(c) => {
                let declared_ty = ddl::column_ty(f);
                if same_type(&declared_ty, &c.ty) {
                    if let Some(ch) = places_change(e, f, c, written, held) {
                        out.push(ch);
                    }
                }
                if !same_type(&declared_ty, &c.ty) {
                    // Sealing and unsealing are type changes at the column
                    // level (`bytea` either way), and they are their own
                    // rulings — B7 has to route through the worker and D6
                    // is refused. Naming them as "type change" would put a
                    // key-custody migration in the class that copies with
                    // SQL.
                    if f.sealed && c.ty != "bytea" {
                        out.push(Change::SealField {
                            entity: e.name.clone(),
                            field: f.name.clone(),
                        });
                    } else if !f.sealed && c.ty == "bytea" {
                        out.push(Change::UnsealField {
                            entity: e.name.clone(),
                            field: f.name.clone(),
                        });
                    } else {
                        out.push(Change::ChangeType {
                            entity: e.name.clone(),
                            field: f.name.clone(),
                            live: c.ty.clone(),
                            declared: declared_ty,
                        });
                    }
                } else if f.optional && !c.nullable {
                    out.push(Change::RelaxField {
                        entity: e.name.clone(),
                        field: f.name.clone(),
                    });
                } else if !f.optional && c.nullable {
                    out.push(Change::RequireField {
                        entity: e.name.clone(),
                        field: f.name.clone(),
                    });
                }
                // The blind index is a SEPARATE column, so gaining one on
                // an already-sealed field is its own change and the
                // hardest in the taxonomy (§7.3).
                if f.indexed && !t.columns.contains_key(&f.index_column()) {
                    out.push(Change::IndexSealedField {
                        entity: e.name.clone(),
                        field: f.name.clone(),
                    });
                }
            }
        }
    }
    if e.is_tree && !t.columns.contains_key("$path") {
        out.push(Change::MakeTree {
            entity: e.name.clone(),
        });
    }
}

/// The places a field declares, and whether it is money: `None` for a
/// field that is neither money nor decimal.
pub fn declared_places(f: &Field) -> Option<(bool, u8)> {
    match f.ty {
        FieldTy::Scalar(crate::ScalarTy::Money(p)) => Some((true, p)),
        FieldTy::Scalar(crate::ScalarTy::Decimal(p)) => Some((false, p)),
        _ => None,
    }
}

/// The scale of a live `numeric(precision,scale)` column, as `format_type`
/// spells it.
fn live_scale(ty: &str) -> Option<u8> {
    let inner = ty.trim().strip_prefix("numeric(")?.strip_suffix(')')?;
    inner.split(',').nth(1)?.trim().parse().ok()
}

/// Does this field's declared places differ from the places its stored
/// amounts are held at? (25aa)
///
/// The held places come from, in order: the `$places` row the last places
/// change wrote; an unsealed decimal's own column scale, which is the
/// database's word for it; and the field's places in the version the data
/// was written under. With none of the three there is nothing to compare,
/// which is a database the program has never been applied to.
fn places_change(
    e: &Entity,
    f: &Field,
    c: &LiveColumn,
    written: Option<&Entity>,
    held: &BTreeMap<(String, String), u8>,
) -> Option<Change> {
    let (money, to) = declared_places(f)?;
    let from = held
        .get(&(e.name.clone(), f.name.clone()))
        .copied()
        .or_else(|| {
            if money || f.sealed {
                None
            } else {
                live_scale(&c.ty)
            }
        })
        .or_else(|| {
            written
                .and_then(|w| w.field(&f.name))
                .and_then(declared_places)
                // A field that changed between money and decimal changed
                // its column type too, and that is its own change.
                .filter(|(m, _)| *m == money)
                .map(|(_, p)| p)
        })?;
    (from != to).then(|| Change::ChangePlaces {
        entity: e.name.clone(),
        field: f.name.clone(),
        money,
        from,
        to,
    })
}

/// What a developer says happens to the stored amounts when a field's
/// places change (25aa, ruled 2026-10-05: refused unless the change says
/// whether to rescale them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlacesChoice {
    /// Every stored amount keeps its value: money is multiplied or divided
    /// by the power of ten, and a decimal column's scale is altered.
    Rescale,
    /// The stored numbers stay as written. For money they now read as
    /// different amounts. A decimal column holds the value itself, so its
    /// scale is altered the same way as for `Rescale`.
    Keep,
}

impl PlacesChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            PlacesChoice::Rescale => "rescale",
            PlacesChoice::Keep => "keep",
        }
    }

    pub fn parse(s: &str) -> Option<PlacesChoice> {
        match s.trim() {
            "rescale" => Some(PlacesChoice::Rescale),
            "keep" => Some(PlacesChoice::Keep),
            _ => None,
        }
    }
}

impl std::fmt::Display for PlacesChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which class a brand-new column lands in: §9's four sub-classes of a
/// defaulted-field addition, decided against the declaration.
fn added_field(e: &Entity, f: &Field) -> Change {
    let entity = e.name.clone();
    let field = f.name.clone();
    if f.optional {
        return Change::AddOptionalField { entity, field };
    }
    match &f.default {
        // A sealed field's default is a plaintext value and `ddl_for`
        // deliberately does not lower it to a column default, so it cannot
        // ride A3's free path however literal it looks.
        Some(d) if is_literal_default(d) && !f.sealed => Change::AddConstantField {
            entity,
            field,
            default: render_default(d),
        },
        Some(d) => Change::AddExecutionTimeField {
            entity,
            field,
            default: render_default(d),
        },
        None => Change::AddRequiredField { entity, field },
    }
}

/// Constraint-grain classification, by the names `ddl_for` mints.
fn entity_objects(e: &Entity, t: &LiveTable, out: &mut Vec<Change>) {
    for f in &e.fields {
        if f.unique && !f.implicit {
            let object = unique_name(&e.name, &f.name);
            if !t.has_object(&object) {
                out.push(Change::AddUnique {
                    entity: e.name.clone(),
                    field: f.name.clone(),
                    column: if f.indexed {
                        f.index_column()
                    } else {
                        f.column()
                    },
                    object,
                });
            }
        }
        if let Some(values) = &f.enum_values {
            if !values.is_empty() {
                let object = enum_name(&e.name, &f.name);
                if !t.has_object(&object) {
                    out.push(Change::AddEnum {
                        entity: e.name.clone(),
                        field: f.name.clone(),
                        values: values.clone(),
                        object,
                    });
                }
            }
        }
        if let FieldTy::Ref { target, .. } = &f.ty {
            let object = fk_name(&e.name, &f.name);
            if !t.has_object(&object) {
                out.push(Change::AddForeignKey {
                    entity: e.name.clone(),
                    field: f.name.clone(),
                    target: target.clone(),
                    object,
                });
            }
        }
    }
    for key in &e.unique_keys {
        if key.is_empty() {
            continue;
        }
        let object = composite_unique_name(&e.name, key);
        if !t.has_object(&object) {
            out.push(Change::AddCompositeUnique {
                entity: e.name.clone(),
                key: key.clone(),
                object,
            });
        }
    }
    // Declared and automatic alike (25y): a ref field added to a table, or
    // one deployed before refs were indexed, reaches the apply here.
    for key in &e.all_indexes() {
        if key.is_empty() {
            continue;
        }
        let object = index_name(&e.name, key);
        if !t.has_object(&object) {
            out.push(Change::AddIndex {
                entity: e.name.clone(),
                object,
            });
        }
    }
    // Row invariants, by the name `check.rs` mints. The name carries the
    // condensed condition, so a changed condition reaches here as a drop
    // and an add rather than as a silent redefinition.
    for chk in &e.checks {
        if !t.has_object(&chk.name) {
            out.push(Change::AddCheck {
                entity: e.name.clone(),
                object: chk.name.clone(),
                source: chk.source.clone(),
                sql: chk.sql.clone(),
            });
        }
    }
}

/// Objects the prior program declared, the new one does not, and the
/// database still holds.
fn dropped_objects(p: &Entity, d: &Entity, t: &LiveTable, out: &mut Vec<Change>) {
    // The database still holds it — which is what makes it a DROP rather
    // than a no-op. Named for what it tests, because the reverse reading
    // ("this object is gone") inverts every condition below.
    let deployed = |name: &str| t.has_object(name);
    for f in &p.fields {
        if f.implicit {
            continue;
        }
        let still = d.field(&f.name);
        if f.unique && !still.is_some_and(|n| n.unique) {
            let object = unique_name(&p.name, &f.name);
            if deployed(&object) {
                out.push(Change::DropUnique {
                    entity: p.name.clone(),
                    field: f.name.clone(),
                    object,
                });
            }
        }
        if f.enum_values.as_ref().is_some_and(|v| !v.is_empty())
            && !still.is_some_and(|n| n.enum_values.as_ref().is_some_and(|v| !v.is_empty()))
        {
            let object = enum_name(&p.name, &f.name);
            if deployed(&object) {
                out.push(Change::DropEnum {
                    entity: p.name.clone(),
                    field: f.name.clone(),
                    object,
                });
            }
        }
        if matches!(f.ty, FieldTy::Ref { .. })
            && !still.is_some_and(|n| matches!(n.ty, FieldTy::Ref { .. }))
        {
            let object = fk_name(&p.name, &f.name);
            if deployed(&object) {
                out.push(Change::DropForeignKey {
                    entity: p.name.clone(),
                    object,
                });
            }
        }
    }
    for key in &p.unique_keys {
        if key.is_empty() || d.unique_keys.contains(key) {
            continue;
        }
        let object = composite_unique_name(&p.name, key);
        if deployed(&object) {
            out.push(Change::DropCompositeUnique {
                entity: p.name.clone(),
                object,
            });
        }
    }
    let wanted = d.all_indexes();
    for key in &p.all_indexes() {
        if key.is_empty() || wanted.contains(key) {
            continue;
        }
        let object = index_name(&p.name, key);
        if deployed(&object) {
            out.push(Change::DropIndex {
                entity: p.name.clone(),
                object,
            });
        }
    }
    for chk in &p.checks {
        if d.checks.iter().any(|c| c.name == chk.name) {
            continue;
        }
        if deployed(&chk.name) {
            out.push(Change::DropCheck {
                entity: p.name.clone(),
                object: chk.name.clone(),
            });
        }
    }
}

/// The two changes no live database can be asked about: `normalize` and the
/// typed-id `key`. Both are schema facts with no physical column, and both
/// are found by comparing the deployed program's declaration with the new
/// one.
fn declaration_only(prior: &Entity, declared: &Entity, out: &mut Vec<Change>) {
    if prior.key != declared.key {
        out.push(Change::ChangeKey {
            entity: declared.name.clone(),
            from: prior.key.clone(),
            to: declared.key.clone(),
        });
    }
    for f in declared.declared() {
        let Some(p) = prior.field(&f.name) else {
            continue;
        };
        if p.normalize != f.normalize {
            out.push(Change::Normalize {
                entity: declared.name.clone(),
                field: f.name.clone(),
                from: p.normalize.clone(),
                to: f.normalize.clone(),
            });
        }
    }
}
