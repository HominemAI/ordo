//! `whole entity($id)` — the anchored composition read (DL §`whole`).
//!
//! One member plus, downward, its full owned closure; upward, its own refs
//! as one-hop shallow signposts; incoming non-owned associations never
//! expand. Emitted as a single SELECT producing one nested JSON document —
//! the shape is static and derived entirely from the schema, so it
//! registers like any op.
//!
//! Sealed fields are excluded from every object (open scalars only), and
//! physical columns ($path) never appear.

use ordo_schema::ddl::{q, SPACE_COLUMN};
use ordo_schema::{FieldTy, Schema};

use crate::{PlanError, Space};

/// `whole` builds its SELECT by hand rather than through the binder, so it
/// carries its own scoping. Every nested select — the signposts, each owned
/// child's `json_agg`, and the anchor itself — reads a base table and
/// therefore takes a predicate; the count is not "one per statement" but
/// "one per source", which for a deep owned closure is a dozen or more.
///
/// This is the shape of the whole-document read that made it worth
/// centralising: the signpost and child helpers are the only two places
/// that name a table, so both emit the predicate with the source, and
/// `plan_whole_for` verifies the finished statement.
struct Scope {
    /// The space itself, kept beside its rendering because a guard is
    /// rendered by the pipeline planner, which takes the space and not its
    /// SQL text.
    space: Space,
    /// The rendered space value (a literal, or the reserved slot).
    sql: String,
    /// Source aliases, for verification.
    aliases: Vec<String>,
}

impl Scope {
    fn new(space: &Space) -> Scope {
        Scope {
            space: space.clone(),
            sql: match space {
                Space::Bound(s) => format!("'{}'", s.replace('\'', "''")),
                // `whole`'s statement is executed as written; a prepared
                // form would need a bind order this entry point does not
                // produce.
                Space::Slot => "$1".to_string(),
            },
            aliases: Vec::new(),
        }
    }

    /// The one way `whole` names a base table: source and predicate, minted
    /// together.
    fn table(&mut self, entity: &str, alias: &str) -> (String, String) {
        self.aliases.push(alias.to_string());
        (
            format!("{} AS {}", q(entity), q(alias)),
            format!("{}.{} = {}", q(alias), q(SPACE_COLUMN), self.sql),
        )
    }
}

/// The owned children of an entity: (child entity, the owned ref's column).
///
/// A `tree owned` ref is a self-edge — `parent -> projects? tree owned` on
/// `projects` — and expanding it here recurses forever on a static shape
/// that has no depth to stop at. (It aborted the process: an earlier
/// comment claimed one-owner-max made this terminate, which is true of
/// distinct entities and false of an entity that owns itself.)
///
/// A tree's subtree is a RUNTIME depth, not a schema shape, so it cannot
/// become a nested `json_build_object` chain at plan time. `whole` on a
/// tree entity yields the node and its non-tree owned children; the
/// subtree belongs to `under`, which is bounded by the data rather than
/// by the schema.
fn owned_children<'a>(schema: &'a Schema, parent: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    for e in &schema.entities {
        for f in &e.fields {
            if let FieldTy::Ref {
                target,
                owned,
                tree,
            } = &f.ty
            {
                if *owned && target == parent && !(*tree && e.name == parent) {
                    out.push((e.name.as_str(), f.name.as_str()));
                }
            }
        }
    }
    out
}

/// json_build_object pairs for a row's OPEN scalar fields.
fn scalar_pairs(schema: &Schema, entity: &str, alias: &str) -> Vec<String> {
    let e = schema.entity(entity).expect("resolved");
    let mut pairs = vec![format!("'id', {}.{}", q(alias), q("id"))];
    for f in e.declared() {
        if f.sealed {
            continue;
        }
        if matches!(f.ty, FieldTy::Ref { .. }) {
            continue;
        }
        pairs.push(format!("'{}', {}.{}", f.name, q(alias), q(&f.column())));
    }
    pairs
}

/// A one-hop shallow signpost object for a ref field, null-safe.
fn signpost(
    schema: &Schema,
    target: &str,
    from_alias: &str,
    from_col: &str,
    n: &mut usize,
    sc: &mut Scope,
    reader: Reader<'_>,
) -> Result<String, PlanError> {
    *n += 1;
    let a = format!("w{n}");
    let pairs = scalar_pairs(schema, target, &a).join(", ");
    let (src, pred) = sc.table(target, &a);
    // A scalar sub-select that matches no row is NULL, so a target the
    // reader may not see renders exactly as a ref that is null.
    let guard = row_guard(schema, target, &a, *n, sc, reader)?;
    Ok(format!(
        "(SELECT json_build_object({pairs}) FROM {src} WHERE {}.{} = {}.{} AND {pred}{guard})",
        q(&a),
        q("id"),
        q(from_alias),
        q(from_col),
    ))
}

/// Who a whole-read is for.
///
/// `Unguarded` is the pre-authorization planner the composition corpus
/// still exercises: no guard anywhere. `Caller` guards **every row the
/// statement reads**, which is the anchor, each signpost's target and each
/// owned child; `Caller(None)` is anonymous, for whom every guard is false.
#[derive(Clone, Copy)]
pub enum Reader<'a> {
    Unguarded,
    Caller(Option<&'a str>),
}

/// The read guard of `entity` at `alias`, as ` AND (<sql>)`, or nothing.
///
/// **One function for the anchor, the signposts and the children**, because
/// the defect this closes was that only the first of the three had one. The
/// module's own doc said a whole-read "discloses exactly what an ordinary
/// read of that row would", and it disclosed the scalars of every row the
/// anchor pointed at, guarded or not (`security-review-6.md` S6-001; found
/// by auditing for other instances of the dot-hop and bridge defect).
fn row_guard(
    schema: &Schema,
    entity: &str,
    alias: &str,
    n: usize,
    sc: &mut Scope,
    reader: Reader<'_>,
) -> Result<String, PlanError> {
    let Reader::Caller(caller) = reader else {
        return Ok(String::new());
    };
    // Each guard's binder mints aliases from a base of its own, clear of
    // the planner's `w<n>` and of every other guard in the statement.
    let base = 1000 * (n + 1);
    let cond =
        ordo_sema::whole_guard_cond_at(schema, entity, alias, caller, base).map_err(|e| {
            // The binder's own class passes through. A guard that does
            // not compile is a fact the binder already has a name for, and
            // a second name minted here would be a refusal class that
            // appears in no specification (`class_inventory.rs` refuses
            // exactly that, and caught this).
            PlanError {
                code: e.code,
                message: format!(
                    "the read guard of `{entity}` does not compile: {}",
                    e.message
                ),
            }
        })?;
    match cond {
        None => Ok(String::new()),
        Some(c) => {
            let (sql, sourced) = crate::render_cond(&c, &sc.space.clone())?;
            sc.aliases.extend(sourced.physical_aliases());
            Ok(format!(" AND ({sql})"))
        }
    }
}

/// The object for one row of `entity` at `alias`: open scalars, signposts
/// for its refs, and a recursively built array per owned child entity.
fn row_object(
    schema: &Schema,
    entity: &str,
    alias: &str,
    n: &mut usize,
    sc: &mut Scope,
    reader: Reader<'_>,
) -> Result<String, PlanError> {
    let e = schema.entity(entity).ok_or_else(|| PlanError {
        code: "unknown_source",
        message: format!("`{entity}` is not an entity"),
    })?;

    let mut pairs = scalar_pairs(schema, entity, alias);

    // Upward / sideways: every ref on this row embeds as a shallow
    // signpost — the owner, and a link row's far side (the "one shallow
    // hop"; never further, because signposts contain no refs).
    for f in e.declared() {
        if let FieldTy::Ref { target, .. } = &f.ty {
            pairs.push(format!(
                "'{}', {}",
                f.name,
                signpost(schema, target, alias, &f.column(), n, sc, reader)?
            ));
        }
    }

    // Downward: the owned closure, one array per owned child entity,
    // recursing. Ownership is one-owner-max, so this terminates.
    for (child, via) in owned_children(schema, entity) {
        *n += 1;
        let ca = format!("w{n}");
        let child_n = *n;
        let obj = row_object(schema, child, &ca, n, sc, reader)?;
        let (src, pred) = sc.table(child, &ca);
        // An owned child usually inherits its owner's guard, and then this
        // repeats what the anchor already proved. It is applied anyway: a
        // child may declare a guard of its own, and "usually inherited" is
        // the reasoning that left the signpost open.
        let child_guard = row_guard(schema, child, &ca, child_n, sc, reader)?;
        // `id` is a text column, so this ORDER BY is a TEXT ordering and
        // takes the engine's pinned collation like every other one
        // (`crate::TEXT_COLLATION`): the owned closure must serialise in
        // the same sequence on every deployment, or two nodes render the
        // same document with the children in different orders.
        pairs.push(format!(
            "'{child}', (SELECT coalesce(json_agg({obj} ORDER BY {}.{} COLLATE \"{}\"), '[]'::json) \
             FROM {src} WHERE {}.{} = {}.{} AND {pred}{child_guard})",
            q(&ca),
            q("id"),
            crate::TEXT_COLLATION,
            q(&ca),
            q(via),
            q(alias),
            q("id"),
        ));
    }

    Ok(format!("json_build_object({})", pairs.join(", ")))
}

/// The full statement: one row, one column `whole`, or zero rows when the
/// anchor does not exist.
pub fn plan_whole(
    schema: &Schema,
    entity: &str,
    anchor_sql: &str,
    space: &Space,
) -> Result<String, PlanError> {
    plan_whole_for(schema, entity, anchor_sql, Reader::Unguarded, space)
}

/// `whole`, with the anchor's read guard inlined.
///
/// This is the op that returns an entire object graph, and it was the one
/// op with no authorization: the plan took no caller, so a guarded
/// entity's whole-document read handed the graph to anybody, signposts
/// included. Guards are not optional anywhere else and are not optional
/// here.
///
/// `guard` is the anchor's read guard as bound IR over the alias `w0`,
/// compiled by `ordo_sema::whole_guard_cond` (which owns `$caller`).
/// `None` means the anchor is genuinely unguarded — an `access read open`
/// entity, or one with no guards — and never "we could not work it out".
pub fn plan_whole_as(
    schema: &Schema,
    entity: &str,
    anchor_sql: &str,
    caller: Option<&str>,
    space: &Space,
) -> Result<String, PlanError> {
    plan_whole_for(schema, entity, anchor_sql, Reader::Caller(caller), space)
}

fn plan_whole_for(
    schema: &Schema,
    entity: &str,
    anchor_sql: &str,
    reader: Reader<'_>,
    space: &Space,
) -> Result<String, PlanError> {
    let mut n = 0usize;
    let mut sc = Scope::new(space);
    let alias = "w0";
    let obj = row_object(schema, entity, alias, &mut n, &mut sc, reader)?;
    let (src, pred) = sc.table(entity, alias);
    // The anchor's guard, from the same function as every other row's. It
    // used to be compiled by the caller and handed in as IR, which is how a
    // statement came to have one guard and many rows.
    let guard = row_guard(schema, entity, alias, 0, &mut sc, reader)?;
    let sql = format!(
        "SELECT {obj} AS {} FROM {src} WHERE {}.{} = {anchor_sql} AND {pred}{guard};",
        q("whole"),
        q(alias),
        q("id"),
    );
    crate::verify_whole(&sql, &sc.aliases)?;
    Ok(sql)
}
