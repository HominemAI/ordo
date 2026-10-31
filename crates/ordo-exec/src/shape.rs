//! F6's ground truth: **empty-shadow shape verification**.
//!
//! The compiler infers each op's result shape statically. Postgres knows
//! the shape for certain. Those two can disagree — a lowering that returns
//! `numeric` where the planner said `int`, a rename that silently changes
//! a column's name, an expression whose type the binder guessed — and a
//! disagreement is a bug that reaches a caller as *wrong data with a
//! plausible shape*, which is the worst kind.
//!
//! So at load, every registered op's planned SQL runs once against an
//! **empty shadow schema**: the same DDL, zero rows. The result carries no
//! data, but the server reports the exact columns and types it would
//! produce. That is the observed shape, and the inferred one must match it
//! or the load fails loudly.
//!
//! Empty is what makes this safe to run on every boot and every hot swap:
//! no row is read, no guard is consulted, nothing can leak, and the cost
//! is a plan-and-describe per op rather than a query.

use crate::{Pool, PoolError};

/// A mismatch between what the compiler said and what the database says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeMismatch {
    pub op: String,
    pub detail: String,
}

impl std::fmt::Display for ShapeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "shape_mismatch in `{}`: {}", self.op, self.detail)
    }
}

/// The shadow schema's name. A scratch namespace, dropped and rebuilt at
/// verification time so it can never accumulate state or drift from the
/// DDL it is supposed to mirror.
///
/// Public because [`Pool::live_drift`]'s caller names both sides of the
/// comparison, and a caller that spelled this one itself could compare the
/// live schema against a namespace nothing built.
pub const SHADOW: &str = "ordo_shadow";

/// A second shadow namespace, for callers that ask the drift question
/// outside the load gate.
///
/// Not a parameter with a default and not a reuse of [`SHADOW`]: the load
/// gate drops and rebuilds its shadow on every verification and drops it
/// again afterwards, so a space export running the same comparison in the
/// same namespace would have its schema deleted mid-comparison by an
/// unrelated boot. Two namespaces is the cheapest way for the two
/// questions to be independent, and the name says which is which in
/// `pg_namespace` when an operator is looking.
pub const EXPORT_SHADOW: &str = "ordo_shadow_export";

impl Pool {
    /// Build the shadow schema from the deployment's own DDL.
    pub async fn build_shadow(&self, ddl: &str) -> Result<(), PoolError> {
        self.build_shadow_in(SHADOW, ddl).await
    }

    /// Build a shadow schema under a caller-chosen name.
    ///
    /// `schema` is interpolated into DDL, so it is **not** caller data:
    /// every call site passes one of this module's own constants. It is a
    /// parameter so the two shadows cannot collide, not so a name can come
    /// from a request.
    pub async fn build_shadow_in(&self, schema: &str, ddl: &str) -> Result<(), PoolError> {
        self.rows(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .await?;
        self.rows(&format!("CREATE SCHEMA {schema}")).await?;
        // The DDL is written unqualified, so a search_path switch is what
        // aims it at the shadow rather than at live data. Belt and braces:
        // the statements run in one connection whose path is set first.
        //
        // **And reset in the same statement, which is the whole point of
        // it being the same statement.** `SET` is session state and this
        // connection goes straight back into the pool; a connection that
        // carried `search_path = ordo_shadow` home would make the *next*
        // unrelated query resolve `orders` to the shadow's empty copy — a
        // read that returns nothing, correctly typed, with no error
        // anywhere. Until this line the only caller dialled a throwaway
        // pool, which hid the hazard rather than removing it; the space
        // export runs on the serving pool and would have found it in
        // production. A failure mid-DDL rolls the implicit transaction
        // back, `SET` included, so the reset is not needed on that path
        // and is harmless on it.
        self.rows(&format!(
            "SET search_path TO {schema}; {ddl}; RESET search_path"
        ))
        .await?;
        Ok(())
    }

    /// The columns and types Postgres reports for `sql`, run against the
    /// shadow schema with a guaranteed-empty result.
    ///
    /// `WHERE false` around the plan is what makes this free of side
    /// effects and free of data: the server still resolves every
    /// expression and reports the row type, but evaluates nothing.
    pub async fn observed_shape(&self, sql: &str) -> Result<Vec<(String, String)>, PoolError> {
        let inner = sql.trim().trim_end_matches(';');
        let probe = format!(
            // DROP then CREATE, never CREATE OR REPLACE: replacing a view
            // may not change its column list, and every op has a different
            // one.
            //
            // `SET LOCAL`, so the path lasts for this probe's own implicit
            // transaction and never stays on a pooled connection. And the
            // view is looked up through `pg_temp`, this session's own temp
            // schema: every concurrent probe has a `$shape`, and matching
            // `relname` alone returned the union of their columns.
            "SET LOCAL search_path TO {SHADOW}; \
             DROP VIEW IF EXISTS pg_temp.\"$shape\"; \
             CREATE TEMP VIEW \"$shape\" AS {inner}; \
             SELECT a.attname, format_type(a.atttypid, a.atttypmod) \
             FROM pg_attribute a \
             WHERE a.attrelid = 'pg_temp.\"$shape\"'::regclass \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum"
        );
        let rows = self.rows(&probe).await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?)))
            .collect())
    }
}

// ── the half the shadow cannot see ───────────────────────────────────────

/// One way the **live** schema differs from the one the artifact asserts.
///
/// See [`Pool::live_drift`] for why this is a separate question from a
/// shape mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub table: String,
    /// `None` when the whole table is missing, or when the drift is about a
    /// constraint rather than a column.
    pub column: Option<String>,
    /// The constraint or index this line is about, when it is about one.
    /// Named rather than described because `ddl_for` mints every constraint
    /// name deterministically, so the name is what an operator greps the
    /// DDL for.
    pub object: Option<String>,
    pub detail: String,
}

impl Drift {
    fn table_missing(table: &str, detail: &str) -> Drift {
        Drift {
            table: table.into(),
            column: None,
            object: None,
            detail: detail.into(),
        }
    }
    fn column(table: &str, column: &str, detail: String) -> Drift {
        Drift {
            table: table.into(),
            column: Some(column.into()),
            object: None,
            detail,
        }
    }
    fn object(table: &str, object: &str, detail: String) -> Drift {
        Drift {
            table: table.into(),
            column: None,
            object: Some(object.into()),
            detail,
        }
    }
}

impl std::fmt::Display for Drift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.column, &self.object) {
            (Some(c), _) => write!(f, "`{}`.`{}`: {}", self.table, c, self.detail),
            (None, Some(o)) => write!(f, "`{}` constraint `{}`: {}", self.table, o, self.detail),
            (None, None) => write!(f, "`{}`: {}", self.table, self.detail),
        }
    }
}

/// One column, as `information_schema` describes it.
type Col = (String, String, String, String, String, String);

/// One constraint or free-standing index, as the catalog describes it.
#[derive(Debug, Clone)]
struct Obj {
    table: String,
    name: String,
    /// `unique constraint`, `check constraint`, `foreign key`, `index` —
    /// printed so a refusal names what kind of thing is missing.
    kind: String,
    /// Postgres' canonical rendering, schema qualification stripped.
    def: String,
}

impl Pool {
    /// Compare the shadow schema against the **live** one, and name every
    /// way the database the ops will actually run against is not the shape
    /// they were verified against.
    ///
    /// # Why shape verification alone cannot answer this
    ///
    /// [`Pool::build_shadow`] builds the shadow from the DDL *the artifact
    /// carries*, and the compiler infers its shapes from the same program.
    /// Both sides of `compare` therefore descend from one source, so what
    /// the gate catches is a **planner-versus-Postgres** disagreement — a
    /// lowering that types a column wrong, a collation the planner did not
    /// account for. It cannot catch a live database that has drifted from
    /// the artifact, because nothing in that loop ever looks at the live
    /// database at all.
    ///
    /// That gap has two names in the specification. It is the hot-swap
    /// hazard — an artifact whose migration has not been applied verifies
    /// perfectly against its own DDL and then fails on the first request.
    /// And it is step 3 of `implementation.md`'s restore drill, *"re-run
    /// empty-shadow shape verification for the whole artifact set against
    /// the restored schema — a restore can land the database on a shape the
    /// registry does not expect, and this is the check that catches it"*,
    /// which the empty shadow on its own does not do.
    ///
    /// # The direction is deliberately one-way
    ///
    /// Every shadow column must exist live, with the same type, length,
    /// precision, nullability and collation. **Extra live columns and extra
    /// live tables are not drift.** The platform's own tables
    /// (`$commandlog`, `$jobs`, `$artifacts`, `$sealedpayload`) are live and
    /// in no artifact's DDL; and `migration.md`'s additive-then-swap
    /// sequence puts the new column in the database one deploy *before* the
    /// artifact that reads it, so a deployment mid-migration must keep
    /// serving. Refusing on extra columns would refuse exactly the state the
    /// migration story is built around.
    ///
    /// Nullability is checked in the same direction and for the same reason
    /// a type is: a column the artifact believes is `NOT NULL` and the
    /// database allows nulls in produces rows the compiler's inferred shape
    /// says cannot exist.
    ///
    /// # Constraints, and why they were the larger half
    ///
    /// Until `plan-schema-evolution.md` §1.1(b) this read
    /// `information_schema.columns` and nothing else, so a missing `UNIQUE`,
    /// `CHECK`, `FOREIGN KEY` or `INDEX` — and the `NULLS NOT DISTINCT`
    /// modifier, which is the whole difference between DL's `null == null`
    /// and SQL's — was **invisible to the gate**. That was tolerable while
    /// the DDL was applied whole or not at all. The moment migrations exist
    /// half the change taxonomy is constraint work, and *"did the migration
    /// run?"* would have had no truthful answer: the constraint half of a
    /// half-applied migration would verify perfectly.
    ///
    /// Constraints are compared **by name against their definition text**.
    /// `ddl_for` mints every name deterministically
    /// (`orders$total$uq`, `orders$status$enum`, `orders$customer$fk`,
    /// `orders$a$b$ix`), so the name is a complete answer to *is this
    /// declaration deployed?*, and `pg_get_constraintdef` /
    /// `pg_get_indexdef` are Postgres' own canonical rendering, which is
    /// what makes `NULLS NOT DISTINCT` visible without a special case for
    /// it. The direction is the same one-way rule: an extra live constraint
    /// is not drift, because the not-yet-dropped constraint of an
    /// expand-contract migration is exactly that.
    pub async fn live_drift(&self, shadow: &str, live: &str) -> Result<Vec<Drift>, PoolError> {
        let want = self.columns_of(shadow).await?;
        let have = self.columns_of(live).await?;
        let mut drift = Vec::new();
        let live_tables: std::collections::BTreeSet<&str> =
            have.iter().map(|c| c.0.as_str()).collect();
        for w in &want {
            if !live_tables.contains(w.0.as_str()) {
                let already = drift.iter().any(|d: &Drift| d.table == w.0);
                if !already {
                    drift.push(Drift::table_missing(
                        &w.0,
                        "the artifact declares this table and the live schema has no such table",
                    ));
                }
                continue;
            }
            let Some(h) = have.iter().find(|h| h.0 == w.0 && h.1 == w.1) else {
                drift.push(Drift::column(
                    &w.0,
                    &w.1,
                    "the artifact declares this column and the live table has no such column"
                        .into(),
                ));
                continue;
            };
            // One line per difference rather than a first-difference wins:
            // an operator repairing a drifted schema wants the whole list,
            // and the query has already paid for it.
            for (what, asserted, actual) in [
                ("type", &w.2, &h.2),
                ("length/precision", &w.3, &h.3),
                ("nullability", &w.4, &h.4),
                ("collation", &w.5, &h.5),
            ] {
                if asserted != actual {
                    drift.push(Drift::column(
                        &h.0,
                        &h.1,
                        format!(
                            "{what}: the artifact says `{asserted}`, the live schema `{actual}`"
                        ),
                    ));
                }
            }
        }

        // ── the constraint half ──
        let want_obj = self.objects_of(shadow).await?;
        let have_obj = self.objects_of(live).await?;
        for w in &want_obj {
            // A constraint on a table the live schema does not have is
            // already reported once, as the missing table. Saying it again
            // per constraint would bury the one line that matters.
            if !live_tables.contains(w.table.as_str()) {
                continue;
            }
            match have_obj
                .iter()
                .find(|h| h.table == w.table && h.name == w.name)
            {
                None => drift.push(Drift::object(
                    &w.table,
                    &w.name,
                    format!(
                        "the artifact declares this {} and the live table has no such \
                         constraint: `{}`",
                        w.kind, w.def
                    ),
                )),
                Some(h) if h.def != w.def => drift.push(Drift::object(
                    &w.table,
                    &w.name,
                    format!(
                        "definition: the artifact says `{}`, the live schema `{}`",
                        w.def, h.def
                    ),
                )),
                Some(_) => {}
            }
        }
        Ok(drift)
    }

    /// Every constraint and every free-standing index of one schema, with
    /// Postgres' own canonical definition text.
    ///
    /// Constraint-backed indexes are excluded from the index half
    /// (`conindid`): a `UNIQUE` constraint and the index implementing it are
    /// one declaration, and reporting both would double every unique drift
    /// and make a `CREATE INDEX` look like two changes.
    ///
    /// The schema name is stripped from the definition text, because
    /// `pg_get_indexdef` qualifies the table (`ON ordo_shadow.orders`) and
    /// the two sides are by construction in different schemas — an
    /// unstripped comparison would report every index as drifted.
    async fn objects_of(&self, schema: &str) -> Result<Vec<Obj>, PoolError> {
        let lit = crate::sql_lit(schema);
        let rows = self
            .rows(&format!(
                "SELECT t.relname, c.conname, \
                        CASE c.contype WHEN 'u' THEN 'unique constraint' \
                                       WHEN 'p' THEN 'primary key' \
                                       WHEN 'f' THEN 'foreign key' \
                                       WHEN 'c' THEN 'check constraint' \
                                       ELSE 'constraint' END, \
                        pg_get_constraintdef(c.oid) \
                 FROM pg_constraint c \
                 JOIN pg_class t ON t.oid = c.conrelid \
                 JOIN pg_namespace n ON n.oid = t.relnamespace \
                 WHERE n.nspname = {lit} AND t.relkind = 'r' \
                 UNION ALL \
                 SELECT t.relname, i.relname, 'index', pg_get_indexdef(x.indexrelid) \
                 FROM pg_index x \
                 JOIN pg_class i ON i.oid = x.indexrelid \
                 JOIN pg_class t ON t.oid = x.indrelid \
                 JOIN pg_namespace n ON n.oid = i.relnamespace \
                 WHERE n.nspname = {lit} AND t.relkind = 'r' \
                   AND NOT EXISTS (SELECT 1 FROM pg_constraint c \
                                   WHERE c.conindid = x.indexrelid) \
                 ORDER BY 1, 2"
            ))
            .await?;
        // Both the bare and the quoted spelling: `pg_get_indexdef` quotes an
        // identifier only when it must, and the platform's own schemas are
        // plain lowercase while a shadow named with capitals would not be.
        let qualifiers = [format!("\"{schema}\"."), format!("{schema}.")];
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                let mut def = g(3);
                for q in &qualifiers {
                    def = def.replace(q.as_str(), "");
                }
                Some(Obj {
                    table: r.first()?.clone()?,
                    name: g(1),
                    kind: g(2),
                    def,
                })
            })
            .collect())
    }

    /// Every column of every ordinary table in one schema.
    ///
    /// Views are excluded: `observed_shape` creates one in the shadow to
    /// read a row type out of, so a shadow described without this filter
    /// would carry the last probed op's result columns as if they were
    /// storage the live schema was missing.
    async fn columns_of(&self, schema: &str) -> Result<Vec<Col>, PoolError> {
        let rows = self
            .rows(&format!(
                "SELECT c.table_name, c.column_name, c.data_type, \
                        coalesce(c.character_maximum_length::text, \
                                 c.numeric_precision::text || ',' || c.numeric_scale::text, ''), \
                        c.is_nullable, coalesce(c.collation_name, '') \
                 FROM information_schema.columns c \
                 JOIN information_schema.tables t \
                   ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
                 WHERE c.table_schema = {} AND t.table_type = 'BASE TABLE' \
                 ORDER BY c.table_name, c.column_name",
                crate::sql_lit(schema)
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                Some((
                    r.first()?.clone()?,
                    r.get(1)?.clone()?,
                    g(2),
                    g(3),
                    g(4),
                    g(5),
                ))
            })
            .collect())
    }
}

/// Does the compiler's declared type accept what Postgres reported?
///
/// Deliberately a mapping rather than string equality: `money` is minor
/// units in `bigint`, an `id` is `text`, and a duration is a count. What
/// this must catch is a REAL divergence — a text column the planner called
/// an int, a timestamp it called a date — not a spelling difference the
/// type system already settled.
pub fn accepts(declared: ordo_syntax::ScalarTy, observed: &str) -> bool {
    use ordo_syntax::ScalarTy as T;
    let o = observed.split('(').next().unwrap_or(observed).trim();
    match declared {
        T::Int | T::Money(_) | T::Duration(_) => {
            matches!(o, "bigint" | "integer" | "smallint" | "numeric")
        }
        // `decimal` is what `/` and `avg` produce, and the planner pins it
        // with `round(…, 12)` over UNCONSTRAINED `numeric` — so `numeric`
        // with no typmod is the only shape the database can report for it.
        // Notably NOT `double precision`: a float here would mean a plan
        // had reintroduced one, which is the failure this gate exists to
        // catch rather than to tolerate.
        T::Decimal(_) => o == "numeric",
        T::Bool => o == "boolean",
        T::Date => o == "date",
        T::Datetime => matches!(
            o,
            "timestamp with time zone" | "timestamp without time zone"
        ),
        T::Text | T::Id => matches!(o, "text" | "character varying" | "character" | "bytea"),
        // An external blob's column is the hex handle in `text`; an inline
        // one and a sealed one are `bytea`. This function is given the
        // declared *type* and not the field, so it cannot tell the tiers
        // apart — and widening to accept both is right rather than lax:
        // what the shape gate exists to catch is a column the planner
        // called an int that the database calls text, and both of these
        // are shapes `ddl::column_ty` genuinely emits for a `blob`.
        T::Blob => matches!(o, "text" | "bytea"),
        // An address is text, or `bytea` when the field is sealed.
        T::Uri => matches!(o, "text" | "character varying" | "bytea"),
        // A document is `jsonb`, or `bytea` when the field is sealed.
        T::Json => matches!(o, "jsonb" | "json" | "bytea"),
    }
}

/// Verify one op: names in order, and a type the declaration accepts.
pub fn compare(
    op: &str,
    inferred: &[(String, ordo_syntax::ScalarTy)],
    observed: &[(String, String)],
) -> Result<(), ShapeMismatch> {
    if inferred.len() != observed.len() {
        return Err(ShapeMismatch {
            op: op.to_string(),
            detail: format!(
                "the compiler expects {} column(s), the database reports {}: {:?} vs {:?}",
                inferred.len(),
                observed.len(),
                inferred.iter().map(|(n, _)| n).collect::<Vec<_>>(),
                observed.iter().map(|(n, _)| n).collect::<Vec<_>>()
            ),
        });
    }
    for (i, ((iname, ity), (oname, oty))) in inferred.iter().zip(observed).enumerate() {
        if iname != oname {
            return Err(ShapeMismatch {
                op: op.to_string(),
                detail: format!(
                    "column {i}: the compiler calls it `{iname}`, the database `{oname}`"
                ),
            });
        }
        if !accepts(*ity, oty) {
            return Err(ShapeMismatch {
                op: op.to_string(),
                detail: format!(
                    "column `{iname}`: the compiler says {}, the database reports `{oty}`",
                    ity.as_str()
                ),
            });
        }
    }
    Ok(())
}
