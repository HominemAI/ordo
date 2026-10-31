//! DDL printing. One `CREATE TABLE` per entity, then constraints, in a fixed
//! order so the output is byte-stable and diffable — a golden file that
//! reorders on every run tests nothing.
//!
//! Identifiers are always double-quoted. The language is case-sensitive and
//! `orderItems` is a real entity name; unquoted Postgres would fold it.

use ordo_syntax::decl::ScalarTy;
use ordo_syntax::token::DurationUnit;
use ordo_syntax::Expr;

use crate::{Entity, Field, FieldTy, Schema, SchemaError};

/// Milliseconds per unit, for the units that have an exact one.
///
/// `mo` and `y` deliberately have none: a month is not a fixed number of
/// milliseconds, and quietly picking 30 days would make `duration(ms) = 1mo`
/// wrong in a way nothing downstream could detect.
pub fn unit_ms(u: DurationUnit) -> Option<i128> {
    Some(match u {
        DurationUnit::Ms => 1,
        DurationUnit::S => 1_000,
        DurationUnit::M => 60_000,
        DurationUnit::H => 3_600_000,
        DurationUnit::D => 86_400_000,
        DurationUnit::W => 604_800_000,
        DurationUnit::Mo | DurationUnit::Y => return None,
    })
}

/// Convert `n` from one unit to another, exactly or not at all.
///
/// A duration column stores its **declared** unit, so `cadence: duration(ms)
/// = 7d` must land as 604800000 and not as 7. Dropping the literal's unit was
/// a real bug: seven days became seven milliseconds, and the DDL looked fine.
pub fn convert_duration(n: i128, from: DurationUnit, to: DurationUnit) -> Option<i128> {
    let (f, t) = (unit_ms(from)?, unit_ms(to)?);
    let ms = n.checked_mul(f)?;
    if ms % t != 0 {
        return None; // not representable in the declared unit without loss
    }
    Some(ms / t)
}

/// Quote an identifier for Postgres.
pub fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The tenancy column every table carries.
///
/// **Physical only, never language-visible.** `id`, `rev` and `depth` are
/// implicit *fields*: undeclarable but addressable, projectable, sortable.
/// `$space` is not a field at all — it is the same kind of thing as
/// `$path`, a column the platform maintains and the language cannot name.
/// The `$` is what makes that structural rather than conventional: the
/// lexer cannot produce `$space` as an identifier, so there is no spelling
/// by which an author declares it, projects it, filters on it, or assigns
/// it. A caller-writable space column is a tenancy escape, and the cheapest
/// way to have none is for the word to be unpronounceable.
///
/// It is also not a language-visible field for a reason beyond safety: the
/// space is not a property of the row that varies within a query. It is
/// bound once, at the door, for the whole request
/// (`engine-plan.md`: "the space is bound before the check and carried into
/// the executor as its own argument"). A row attribute that is constant
/// across every row a statement can see is a *predicate*, not a column
/// anyone should be reading.
pub const SPACE_COLUMN: &str = "$space";

/// The physical column type for a field.
///
/// `money` is minor units and therefore integral — there is no float anywhere
/// in this mapping, by construction rather than by convention. A sealed field
/// is `bytea` because its payload is ciphertext, and it stays `bytea` even
/// though nothing encrypts yet: a `text` column here would be a schema that
/// invites plaintext.
///
/// # One field, one column — including a blob
///
/// A `blob` field is **one** column holding a **reference**, never bytes and
/// never a family of siblings. Four columns for `(handle, mime, size, hash)`
/// were considered and declined for two reasons that are not aesthetics:
/// [`crate::diff::Change::AddOptionalField`] names a *field*, and `live_drift`
/// compares columns one-for-one, so a single blob field added to a live schema
/// would read as four changes on the class-A additive path; and a plaintext
/// `size` column beside a sealed blob answers *"is this the standard NDA?"*
/// without a key. The metadata lives in `$blobref`, sealed when the field is.
pub fn column_ty(f: &Field) -> String {
    if f.sealed {
        return "bytea".into();
    }
    match &f.ty {
        FieldTy::Ref { .. } => "text".into(),
        FieldTy::Scalar(s) => match s {
            ScalarTy::Text => "text".into(),
            ScalarTy::Id => "text".into(),
            ScalarTy::Int => "bigint".into(),
            ScalarTy::Money(_) => "bigint".into(),
            ScalarTy::Bool => "boolean".into(),
            ScalarTy::Date => "date".into(),
            ScalarTy::Datetime => "timestamptz".into(),
            ScalarTy::Duration(_) => "bigint".into(),
            // `decimal(4)`: an exact `numeric` at the declared scale (27ak).
            // The precision is Postgres's largest, so the scale is the only
            // bound, as the ruling says ("any precision"). Postgres rounds a
            // finer value on the way in; nothing reaches it that way, since
            // the compiler and the door refuse a value with more places.
            // Never `double precision`: there is no float here.
            ScalarTy::Decimal(p) => format!("numeric(1000, {p})"),
            // An external blob's column holds the **hex handle**: text,
            // because that is what a keyed-random 128-bit reference is
            // once it is written down, and because a `bytea` here would
            // make an unsealed blob field indistinguishable from a sealed
            // one in the DDL, which is the one distinction an operator
            // reading a schema most needs.
            //
            // An inline blob's column holds the raw 16-byte payload
            // handle, so it is `bytea` even unsealed — the same shape
            // `$sealedpayload."ref"` carries, so the join is bytes to
            // bytes with no encode/decode step to get wrong.
            ScalarTy::Blob => if f.inline { "bytea" } else { "text" }.into(),
            // An address is text. `uri` has no custody and no handle, so
            // there is nothing opaque about it and nothing to store but
            // the string the author was given.
            ScalarTy::Uri => "text".into(),
            // A JSON document is `jsonb` (28b): Postgres checks it is JSON
            // on the way in and stores it parsed. A sealed one never gets
            // here; it is `bytea` like every sealed field.
            ScalarTy::Json => "jsonb".into(),
        },
    }
}

/// Render a literal default. Only literals are accepted: a default that is a
/// call or a path (`= today`) is execution-time behaviour, which belongs to
/// the writer in P5, not to a column default.
pub fn literal(e: &Expr) -> Result<String, SchemaError> {
    Ok(match e {
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::DecimalLit { digits, .. } => digits.clone(),
        Expr::StrLit { value, .. } => format!("'{}'", value.replace('\'', "''")),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::NullLit { .. } => "NULL".into(),
        Expr::DateLit { y, m, d, .. } => format!("DATE '{y:04}-{m:02}-{d:02}'"),
        Expr::DatetimeLit {
            y,
            m,
            d,
            hh,
            mm,
            ss,
            ..
        } => {
            format!("TIMESTAMPTZ '{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z'")
        }
        Expr::DurationLit { n, .. } => n.to_string(),
        Expr::Neg { expr, .. } => format!("-{}", literal(expr)?),
        other => {
            return Err(SchemaError {
                code: "unsupported_default",
                message: "only literal defaults are lowered to a column default. `now`, \
                          `today` and `newid` are in the language's default set \
                          (definition-language.md, defaults) and are not built yet: give the \
                          field its value on the statement that creates the row"
                    .into(),
                span: other.span(),
            })
        }
    })
}

/// A field's default, rendered in the field's own units.
fn default_literal(f: &Field, e: &Expr) -> Result<String, SchemaError> {
    if let (FieldTy::Scalar(ScalarTy::Duration(decl)), Expr::DurationLit { n, unit, span }) =
        (&f.ty, e)
    {
        let n = i128::from(*n);
        return match convert_duration(n, *unit, *decl) {
            Some(v) => Ok(v.to_string()),
            None => Err(SchemaError {
                code: "duration_unit_mismatch",
                message: format!(
                    "`{}` is declared `duration({})` and its default is `{n}{}`, which has no exact value in that unit",
                    f.name,
                    decl.as_str(),
                    unit.as_str()
                ),
                span: *span,
            }),
        };
    }
    // `= 10.50` on a `money(2)` is 1050 in the column (27ak). The schema
    // build has already refused a default that does not fit.
    if let FieldTy::Scalar(s) = &f.ty {
        if let Ok(Some(stored)) = crate::stored_numeric(*s, e) {
            return literal(&stored);
        }
    }
    literal(e)
}

/// What a physical column *is*, so a consumer can act on the kind rather
/// than on a naming convention.
///
/// A reader that told `memo` from `memo$bidx` by looking for a `$bidx`
/// suffix would be reimplementing [`Field::index_column`] at a distance,
/// and would silently mis-classify a declared field somebody named
/// `memo$bidx`. The role travels with the column instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnRole<'a> {
    /// [`SPACE_COLUMN`] — the tenancy predicate, never a field.
    Space,
    /// A declared or implicit field's value.
    Field(&'a Field),
    /// The keyed digest of a `sealed indexed` field. Derived from the
    /// field's plaintext **and the space's key**, so it is reproducible
    /// nowhere else.
    BlindIndex(&'a Field),
    /// A tree entity's stored materialized path.
    Path,
}

/// One column as the database holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalColumn<'a> {
    pub name: String,
    /// The Postgres type, exactly as [`ddl_for`] writes it.
    pub ty: String,
    pub not_null: bool,
    pub role: ColumnRole<'a>,
}

/// **The** column list of an entity's table, in `CREATE TABLE` order.
///
/// This exists because two things need the same answer and must never
/// derive it twice: [`ddl_for`], which prints the table, and the space
/// export, which reads it back. An export with its own idea of which
/// columns a table has is one entity away from being wrong, and the class
/// of bug — *export silently omits the field somebody just added* — is
/// invisible until the day it matters, because a bundle that is missing a
/// column still parses.
///
/// So the printer and the reader share this function, and a declaration
/// that reaches one reaches the other by construction. `tests::` below
/// asserts the two halves cannot drift: every name here appears in the
/// generated DDL and every column name in the generated DDL appears here.
pub fn physical_columns(e: &Entity) -> Vec<PhysicalColumn<'_>> {
    let mut cols = Vec::with_capacity(e.fields.len() + 2);

    // The tenancy column, first on every table. Physical only: not a
    // language-visible field (see `SPACE_COLUMN`).
    cols.push(PhysicalColumn {
        name: SPACE_COLUMN.to_string(),
        ty: "text".into(),
        not_null: true,
        role: ColumnRole::Space,
    });

    for f in &e.fields {
        cols.push(PhysicalColumn {
            name: f.column(),
            ty: column_ty(f),
            not_null: !f.optional,
            role: ColumnRole::Field(f),
        });
        if f.indexed {
            // ALWAYS `NOT NULL`, even when the base field is optional.
            // The blind index of a null is not null: a nullable
            // `sealed indexed` field HMACs the reserved null sentinel
            // (`ordo_crypto::NULL_SENTINEL`; DL, uniqueness —
            // settled), so the digest is total and the column can say
            // so. This is the constraint doing the work: while the
            // column admitted NULL, `sealed indexed unique` did not
            // constrain nulls at all — Postgres `UNIQUE` permits
            // unlimited NULLs — so two rows could hold a null in a
            // unique sealed field and neither the database nor any
            // test would notice. Now a writer that forgets the
            // sentinel is refused by the schema rather than silently
            // widening the constraint.
            cols.push(PhysicalColumn {
                name: f.index_column(),
                ty: "bytea".into(),
                not_null: true,
                role: ColumnRole::BlindIndex(f),
            });
        }
    }

    if e.is_tree {
        // The stored materialized path (ruling 5): id chain, slash-
        // joined, TRAILING delimiter — 'cat_1/' never prefix-matches
        // 'cat_10/…', which is what makes the containment test exact.
        // Physical only: not a language-visible field.
        cols.push(PhysicalColumn {
            name: "$path".to_string(),
            ty: "text".into(),
            not_null: true,
            role: ColumnRole::Path,
        });
    }

    cols
}

/// The whole schema as one script. Tables first, then foreign keys, then
/// unique constraints — so no statement depends on a later one.
///
/// Every statement comes from one of the per-entity emitters below
/// ([`create_table`], [`foreign_key`], [`unique`], [`composite_unique`],
/// [`index`]), and the additive apply path (`plan-schema-evolution.md`
/// increment 2) emits its `ALTER TABLE … ADD` statements from the same
/// emitters. That is what keeps a table created whole and a table grown a
/// column at a time byte-identical in the catalog: there is one spelling of
/// every column, constraint and index, and both paths print it.
pub fn ddl_for(schema: &Schema) -> Result<String, SchemaError> {
    let mut out = String::new();

    for e in &schema.entities {
        out.push_str(&create_table(e)?);
    }

    for e in &schema.entities {
        for f in &e.fields {
            if let Some(stmt) = foreign_key(e, f) {
                out.push_str(&stmt);
            }
        }
    }

    for e in &schema.entities {
        for f in &e.fields {
            if let Some(stmt) = unique(e, f) {
                out.push_str(&stmt);
            }
        }
    }

    // Multi-column unique keys (`unique (board, member)`), last, so a
    // reader meets the single-column keys first and the composite ones as
    // the exception they are.
    for e in &schema.entities {
        for key in &e.unique_keys {
            if let Some(stmt) = composite_unique(e, key)? {
                out.push_str(&stmt);
            }
        }
    }

    // `index (a, b)` — the extra secondary indexes DL §3 declares.
    //
    // Last, and separately, because an index is the only entity-level
    // declaration that constrains **nothing**: it cannot fail a write, it
    // has no error class, and dropping it changes cost and not meaning. A
    // reader who has understood the constraints above has understood the
    // shape; this is the part that only makes it fast.
    //
    // Then one per ref field, which DL indexes automatically
    // ([`Entity::all_indexes`], reported fix 25y).
    for e in &schema.entities {
        for key in &e.all_indexes() {
            if let Some(stmt) = index(e, key, false)? {
                out.push_str(&stmt);
            }
        }
    }

    Ok(out)
}

// ── the per-entity emitters ──────────────────────────────────────────────

/// One physical column's definition: name, type, nullability, default.
///
/// Only the DEFAULT clause is decided here, because rendering a literal
/// can fail and a column list cannot. A sealed field's default is a
/// plaintext value; sealing it is F0's job, so it cannot become a column
/// default.
fn column_def(pc: &PhysicalColumn<'_>) -> Result<String, SchemaError> {
    let mut col = format!("{} {}", q(&pc.name), pc.ty);
    if pc.not_null {
        col.push_str(" NOT NULL");
    }
    if let ColumnRole::Field(f) = pc.role {
        if f.name == "rev" {
            col.push_str(" DEFAULT 1");
        } else if let Some(d) = &f.default {
            if !f.sealed {
                col.push_str(&format!(" DEFAULT {}", default_literal(f, d)?));
            }
        }
    }
    Ok(col)
}

/// `ALTER TABLE … ADD COLUMN` for one declared field: its value column,
/// and its blind-index column when the field is `sealed indexed`.
///
/// The definition is [`column_def`]'s, so a column added to a live table
/// is spelled exactly as `CREATE TABLE` would have spelled it. The caller
/// decides whether the addition is legal on a populated table: a
/// `<f>$bidx bytea NOT NULL` column has no default and cannot land on one
/// that holds rows (`plan-schema-evolution.md` §7.3).
pub fn add_column(e: &Entity, f: &Field) -> Result<Vec<String>, SchemaError> {
    physical_columns(e)
        .iter()
        .filter(|pc| match pc.role {
            ColumnRole::Field(x) | ColumnRole::BlindIndex(x) => x.name == f.name,
            ColumnRole::Space | ColumnRole::Path => false,
        })
        .map(|pc| {
            Ok(format!(
                "ALTER TABLE {} ADD COLUMN {};\n",
                q(&e.name),
                column_def(pc)?
            ))
        })
        .collect()
}

/// The `CONSTRAINT … CHECK (… IN (…))` clause pinning a field to its
/// closed value set, or `None` when the field declares none.
///
/// A table constraint rather than an inline column one so it carries a
/// NAME: a violation should say which ladder was left, not `23514 on
/// "boardsAccess"`. This is the database's half of the pin, and it is the
/// half that holds through a restore, a hand-written `INSERT`, or a bug in
/// the write path — `level: "root"` is not a level, it is a value whose
/// position in the ordering is undefined, and a guard that enumerates the
/// qualifying levels would silently exclude it rather than refuse it.
pub fn enum_constraint(e: &Entity, f: &Field) -> Option<String> {
    let values = f.enum_values.as_ref()?;
    if values.is_empty() {
        return None;
    }
    let list: Vec<String> = values
        .iter()
        .map(|v| format!("'{}'", v.replace('\'', "''")))
        .collect();
    Some(format!(
        "CONSTRAINT {} CHECK ({} IN ({}))",
        q(&format!("{}${}$enum", e.name, f.name)),
        q(&f.column()),
        list.join(", "),
    ))
}

/// The `CONSTRAINT … CHECK (…)` clause for one row invariant (`check`).
///
/// Enforced by the database inside every write's transaction, so a
/// violation aborts the whole atomic group (DL §3). The name IS the
/// diagnostic: 23514 reports it, and it carries the condition.
pub fn check_constraint(chk: &crate::check::CheckRule) -> String {
    format!("CONSTRAINT {} CHECK ({})", q(&chk.name), chk.sql)
}

/// `CREATE TABLE` for one entity: every physical column, the enum pins,
/// the row invariants and the primary key.
pub fn create_table(e: &Entity) -> Result<String, SchemaError> {
    let mut out = format!("CREATE TABLE {} (\n", q(&e.name));
    let mut cols: Vec<String> = Vec::new();

    // Every column comes from `physical_columns` — the one list the
    // export reads too.
    for pc in physical_columns(e) {
        cols.push(format!("  {}", column_def(&pc)?));
    }

    // A field pinned to a closed value set gets a `CHECK` naming the
    // set, in declaration order.
    for f in &e.fields {
        if let Some(clause) = enum_constraint(e, f) {
            cols.push(format!("  {clause}"));
        }
    }

    // Row invariants (`check`), each as a real CHECK constraint. A
    // `text(n)` field's length bound is one of them
    // (`check::text_bound_checks`), after the authored ones.
    for chk in &e.checks {
        cols.push(format!("  {}", check_constraint(chk)));
    }

    // **The primary key is `(space, id)`, not `id`.**
    //
    // Typed ids are globally unique by minting, so `id` alone would
    // key the table correctly today. It is the wrong key anyway,
    // because a primary key is what foreign keys reference, and a
    // foreign key that can cross spaces is the whole failure this
    // column exists to prevent. With `(space, id)` as the target, a
    // child's ref carries its OWN space into the reference (below),
    // so a row in one space cannot point at a row in another — the
    // database refuses it, rather than nobody happening to try.
    //
    // The space leads, which is also the order every query wants: a
    // statement is always an equality on the space and a lookup
    // inside it.
    //
    // Note what is deliberately NOT emitted: a deployment-wide
    // `UNIQUE (id)`. Id uniqueness across spaces is a property of
    // minting, and asserting it in the schema would turn a mint
    // collision in one tenant into a failed write in another.
    cols.push(format!(
        "  CONSTRAINT {} PRIMARY KEY ({}, {})",
        q(&format!("{}$pk", e.name)),
        q(SPACE_COLUMN),
        q("id")
    ));
    out.push_str(&cols.join(",\n"));
    out.push_str("\n);\n");
    Ok(out)
}

/// The composite foreign key for one ref field, or `None` for a field
/// that is not a ref.
///
/// **Every ref is a same-space ref, structurally.** The referencing side
/// is `(this row's space, the ref column)` and the referenced side is the
/// target's `(space, id)` primary key, so the only rows a ref can name
/// are rows in the ref holder's own space. There is no spelling — no bug,
/// no hand-written statement, no restore, no import — that puts a
/// cross-space edge in this database: Postgres rejects it.
///
/// This is why the composite primary key is worth its extra column. A
/// single-column `REFERENCES t(id)` would happily accept another tenant's
/// id.
pub fn foreign_key(e: &Entity, f: &Field) -> Option<String> {
    let FieldTy::Ref { target, .. } = &f.ty else {
        return None;
    };
    Some(format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}, {}) REFERENCES {} ({}, {});\n",
        q(&e.name),
        q(&format!("{}${}$fk", e.name, f.name)),
        q(SPACE_COLUMN),
        q(&f.column()),
        q(target),
        q(SPACE_COLUMN),
        q("id"),
    ))
}

/// The single-column unique constraint for one field, or `None` for a
/// field that is not `unique` (or is implicit: `id` is keyed by the
/// primary key).
///
/// Uniqueness over a sealed field is uniqueness over its blind index —
/// the payload is not comparable.
///
/// **The space is the leading column of every generated unique index,
/// without exception and without syntax** (DL, uniqueness — settled).
/// There is no deployment-wide uniqueness and no way to ask for one: a
/// domain cannot claim a global name across spaces it does not own, and
/// two tenants must be able to hold the same `externalId`.
///
/// A `sealed indexed unique` field lands on `UNIQUE (space, hmac)` — the
/// form `engine-plan.md` P2 names in its deliverables. The blind-index
/// key is already derived per space (`BIK = KDF(SMK(space), …)`), so two
/// spaces' digests of one value already differ; the column makes the
/// claim structural rather than key-dependent, which is what keeps it
/// true through a key rotation, a restore, or a provider that ever stops
/// salting per space.
pub fn unique(e: &Entity, f: &Field) -> Option<String> {
    if !f.unique || f.implicit {
        return None;
    }
    let col = if f.indexed {
        f.index_column()
    } else {
        f.column()
    };
    Some(format!(
        "ALTER TABLE {} ADD CONSTRAINT {} UNIQUE{} ({}, {});\n",
        q(&e.name),
        q(&format!("{}${}$uq", e.name, f.name)),
        nulls_not_distinct(!f.indexed),
        q(SPACE_COLUMN),
        q(&col),
    ))
}

/// One multi-column unique key (`unique (board, member)`), or `None` for
/// an empty key.
///
/// **The space leads here too**, for the same reason it leads every
/// other unique index: two tenants must be able to hold the same pair,
/// and there is no spelling that asks for a deployment-wide key. The
/// constraint NAME carries every column, so a violation says which key
/// was claimed twice rather than which table.
pub fn composite_unique(e: &Entity, key: &[String]) -> Result<Option<String>, SchemaError> {
    if key.is_empty() {
        return Ok(None);
    }
    let mut cols = vec![q(SPACE_COLUMN)];
    for name in key {
        match e.field(name) {
            // A composite key over a sealed field would have to
            // address through the blind index, exactly as the
            // single-column form does. Nothing derives one yet, so
            // this is a refusal rather than a silent payload
            // comparison that could never match.
            Some(f) if f.sealed => {
                return Err(SchemaError {
                    code: "unsupported_unique_key",
                    message: format!(
                        "`{}` has a composite unique key over sealed `{name}`, which \
                         compares no two values equal",
                        e.name
                    ),
                    span: f.span,
                })
            }
            Some(f) => cols.push(q(&f.column())),
            None => {
                return Err(SchemaError {
                    code: "unknown_unique_key_field",
                    message: format!(
                        "`{}`'s unique key names `{name}`, which is not a field on it",
                        e.name
                    ),
                    span: e.span,
                })
            }
        }
    }
    Ok(Some(format!(
        "ALTER TABLE {} ADD CONSTRAINT {} UNIQUE{} ({});\n",
        q(&e.name),
        q(&format!("{}${}$uq", e.name, key.join("$"))),
        nulls_not_distinct(true),
        cols.join(", "),
    )))
}

/// One declared `index (a, b)`, or `None` for an empty key.
///
/// `concurrently` is the additive apply path's form: `CREATE INDEX
/// CONCURRENTLY` takes no lock that blocks writes, cannot run inside a
/// transaction, and can leave an INVALID index behind on failure — the
/// caller owns both of those facts. A fresh `CREATE TABLE` script wants
/// the plain form.
///
/// **The space column leads here too, and that is a pin this engine
/// takes rather than one DL spells.** DL fixes the leading space column
/// for *unique* indexes, where it is a correctness rule (two tenants
/// must be able to claim the same pair). For a plain index it is a
/// usefulness rule with the same shape: every statement this compiler
/// emits filters on `$space` — there is no cross-space read — so an
/// index that did not lead with it would be unusable for the only
/// queries that exist. Stating it here so it is chosen rather than
/// discovered from an unused index in production.
pub fn index(
    e: &Entity,
    key: &[String],
    concurrently: bool,
) -> Result<Option<String>, SchemaError> {
    if key.is_empty() {
        return Ok(None);
    }
    let mut cols = vec![q(SPACE_COLUMN)];
    for name in key {
        match e.field(name) {
            // A sealed payload is ciphertext: an index over it
            // orders and groups nothing an author could predict,
            // and would read as a working index. `sealed indexed`
            // already has its blind-index column and its own
            // equality story; naming the payload here is a
            // mistake, not a request for that.
            Some(f) if f.sealed => {
                return Err(SchemaError {
                    code: "unsupported_index",
                    message: format!(
                        "`{}`'s `index` names sealed `{name}` — a sealed payload is \
                         ciphertext, so an index over it answers no query",
                        e.name
                    ),
                    span: f.span,
                })
            }
            Some(f) => cols.push(q(&f.column())),
            None => {
                return Err(SchemaError {
                    code: "unknown_index_field",
                    message: format!(
                        "`{}`'s `index` names `{name}`, which is not a field on it",
                        e.name
                    ),
                    span: e.span,
                })
            }
        }
    }
    Ok(Some(format!(
        "CREATE INDEX{} {} ON {} ({});\n",
        if concurrently { " CONCURRENTLY" } else { "" },
        q(&format!("{}${}$ix", e.name, key.join("$"))),
        q(&e.name),
        cols.join(", "),
    )))
}

/// `NULLS NOT DISTINCT`, or nothing.
///
/// DL, "Uniqueness uses language equality [settled]": the language has one
/// equality and `null == null` is TRUE in it, so a nullable unique field
/// admits **at most one null row** and "Generated DDL is
/// `UNIQUE NULLS NOT DISTINCT`". Postgres defaults to the opposite
/// (`NULLS DISTINCT`, unlimited nulls), which is the SQL-native reading DL
/// calls "the deliberate departure from every SQL system" — so the modifier
/// is not decoration, it is the whole difference between the constraint
/// meaning what the language says and meaning what SQL says.
///
/// Not emitted on a blind index: DL spells that one `UNIQUE (space, hmac)`
/// exactly, and the digest column is declared `NOT NULL` (a null value
/// HMACs the reserved null sentinel), so there is no null for the modifier
/// to rule on and adding it would only make the two spellings differ.
fn nulls_not_distinct(yes: bool) -> &'static str {
    if yes {
        " NULLS NOT DISTINCT"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse the quoted column names out of one `CREATE TABLE` body — the
    /// lines that are a quoted identifier followed by a type, which is
    /// every line except the `CONSTRAINT …` ones.
    fn ddl_column_names(ddl: &str, table: &str) -> Vec<String> {
        let head = format!("CREATE TABLE \"{table}\" (\n");
        let body = ddl
            .split(&head)
            .nth(1)
            .unwrap_or_else(|| panic!("no CREATE TABLE for `{table}` in:\n{ddl}"));
        let body = body.split("\n);").next().expect("a terminated table");
        body.lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("CONSTRAINT"))
            .filter_map(|l| l.strip_prefix('"'))
            .filter_map(|l| l.split_once('"').map(|(n, _)| n.replace("\"\"", "\"")))
            .collect()
    }

    fn schema_of(src: &str) -> Schema {
        let program = ordo_syntax::parse_program(src).expect("the fixture parses");
        crate::build(&program).expect("the fixture builds")
    }

    /// The guarantee the space export rests on: **the printer and the
    /// reader share one column list.**
    ///
    /// This is the test a future hand-rolled column list fails. The export
    /// derives what it selects from [`physical_columns`]; if that ever
    /// stops being what `ddl_for` prints — a declaration wired into one and
    /// not the other, a `$bidx` column emitted inline again — an export
    /// would quietly hand over a different shape from the `schema.sql`
    /// shipped beside it, and the bundle would still parse.
    ///
    /// Checked in **both** directions on purpose. Names-in-DDL-not-in-list
    /// is the omission (the export drops a column). Names-in-list-not-in-DDL
    /// is the invention (the export selects a column the database has no
    /// reason to hold, and the query then fails against a database built
    /// from the very DDL shipped beside it).
    #[test]
    fn the_ddl_and_the_physical_column_list_cannot_drift() {
        let schema = schema_of(
            "domain m\n\
             entity tags { name: text }\n\
             entity notes (key: note) {\n\
               tag -> tags\n\
               title: text\n\
               body:  text?\n\
               memo:  text sealed indexed\n\
               count: int = 0\n\
               live:  bool = true\n\
               due:   date?\n\
             }\n\
             entity folders { name: text\n parent -> folders? tree }\n",
        );
        let ddl = ddl_for(&schema).expect("the fixture renders");

        // The comparison below is only interesting if the fixture really
        // has the awkward columns: a sealed-indexed digest and a tree path.
        let all: Vec<String> = schema
            .entities
            .iter()
            .flat_map(|e| physical_columns(e).into_iter().map(|c| c.name))
            .collect();
        assert!(
            all.iter().any(|n| n == "memo$bidx") && all.iter().any(|n| n == "$path"),
            "the fixture stopped exercising the derived columns: {all:?}"
        );

        for e in &schema.entities {
            let listed: Vec<String> = physical_columns(e).into_iter().map(|c| c.name).collect();
            let printed = ddl_column_names(&ddl, &e.name);
            assert_eq!(
                listed, printed,
                "`{}`: `physical_columns` and `ddl_for` disagree. Whatever \
                 declaration was added reached one of them and not the other, \
                 and a space export of this entity would hand over a shape its \
                 own schema.sql does not describe.",
                e.name
            );
        }
    }

    /// `text(64)` lowers to a named `CHECK` on the character count of an
    /// OPEN column, and to nothing on a sealed one: ciphertext has no
    /// character count the database could compare, so the write path is
    /// the only enforcer there and the DDL must not pretend otherwise.
    #[test]
    fn a_text_bound_is_a_check_on_open_columns_only() {
        let schema = schema_of(
            "domain m\n\
             entity notes { title: text(64)\n body: text\n memo: text(8) sealed\n n: int }\n",
        );
        let ddl = ddl_for(&schema).expect("renders");
        assert!(
            ddl.contains(
                r#"CONSTRAINT "notes$len$title<=64" CHECK (("title" IS NULL OR char_length("title") <= 64))"#
            ),
            "{ddl}"
        );
        assert!(
            !ddl.contains("notes$len$memo"),
            "a sealed column's length is the ciphertext's, not the value's:\n{ddl}"
        );
        assert!(!ddl.contains("notes$len$body") && !ddl.contains("notes$len$n"));
        let notes = schema.entity("notes").expect("the shape");
        // The bound is one of the entity's row invariants, so the diff,
        // the apply path and the artifact carry it with no code of their
        // own; the number is in the name, so a changed bound is a drop
        // and an add.
        let names: Vec<&str> = notes.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["notes$len$title<=64"]);
        assert_eq!(notes.checks[0].source, "length(title) <= 64");
        assert_eq!(notes.field("title").unwrap().text_max, Some(64));
        assert_eq!(notes.field("memo").unwrap().text_max, Some(8));
        assert_eq!(notes.field("body").unwrap().text_max, None);
    }

    /// Every column carries a role, and the role is not guessed from the
    /// name. The export reads `BlindIndex` to know what to leave out, so a
    /// digest column mis-labelled `Field` would leave the bundle carrying a
    /// keyed digest as if it were data somebody could re-import.
    #[test]
    fn a_derived_column_is_labelled_derived_rather_than_recognised_by_suffix() {
        let schema = schema_of("domain m\nentity notes { memo: text sealed indexed }\n");
        let notes = schema.entity("notes").expect("the shape is there");
        let cols = physical_columns(notes);

        let bidx = cols
            .iter()
            .find(|c| c.name == "memo$bidx")
            .expect("the digest column");
        assert!(matches!(bidx.role, ColumnRole::BlindIndex(f) if f.name == "memo"));
        assert_eq!(bidx.ty, "bytea");
        assert!(bidx.not_null, "a digest is total — the null sentinel too");

        let memo = cols
            .iter()
            .find(|c| c.name == "memo")
            .expect("the value column");
        assert!(matches!(memo.role, ColumnRole::Field(f) if f.sealed));
        assert_eq!(memo.ty, "bytea", "ciphertext, and never text");

        assert!(matches!(cols[0].role, ColumnRole::Space));
    }
}
