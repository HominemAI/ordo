//! Resolving blind-index digests — the step between the access check and
//! the query.
//!
//! The binder emits digest SITES (`BExpr::BlindDigest`) and no key
//! material, so a compiled artifact can be inspected, diffed and deployed
//! without carrying anything derived from a key. This module turns those
//! sites into bytes, and it runs where the pinned order says it must:
//! after the door has resolved a principal and checked access, behind a
//! key release naming the space.
//!
//! Two bugs died here. Digesting at bind time put key-derived bytes in the
//! artifact before any access check; and the parameter branch never
//! digested at all, comparing a raw plaintext parameter against a digest
//! column — which matched nothing, silently, for a lookup that should have
//! worked.

use ordo_crypto::{KeyProvider, SpaceId};
use ordo_sema::ir::{BCond, BExpr, BlindSource, From, Query, Sort};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlindError(pub String);

impl std::fmt::Display for BlindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Replace every digest site in `q` with its computed digest.
///
/// `params` supplies values for `BlindSource::Param`. A parameter with no
/// value is a typed refusal, never a silent mismatch: an unresolved blind
/// comparison can only ever return zero rows, and returning zero rows
/// because of a platform gap is indistinguishable to a caller from "there
/// is no such customer".
pub fn resolve(
    q: &mut Query,
    provider: &dyn KeyProvider,
    space: &SpaceId,
    params: &[(String, String)],
) -> Result<(), BlindError> {
    for c in q.filters.iter_mut().chain(q.having.iter_mut()) {
        cond(c, provider, space, params)?;
    }
    for o in q.outputs.iter_mut() {
        expr(&mut o.expr, provider, space, params)?;
    }
    for s in q.sort.iter_mut() {
        expr(&mut s.expr, provider, space, params)?;
    }
    for t in q.tiebreak.iter_mut() {
        expr(t, provider, space, params)?;
    }
    if let Some(g) = q.group.as_mut() {
        for k in g.keys.iter_mut() {
            expr(k, provider, space, params)?;
        }
    }
    for c in q.ctes.iter_mut() {
        resolve(&mut c.query, provider, space, params)?;
    }
    for j in q.source_joins.iter_mut() {
        if let From::Sub { query, .. } = &mut j.from {
            resolve(query, provider, space, params)?;
        }
    }
    match &mut q.from {
        From::Sub { query, .. } => resolve(query, provider, space, params)?,
        From::Union { arms, .. } => {
            for a in arms.iter_mut() {
                resolve(a, provider, space, params)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn cond(
    c: &mut BCond,
    p: &dyn KeyProvider,
    space: &SpaceId,
    params: &[(String, String)],
) -> Result<(), BlindError> {
    match c {
        BCond::Cmp { lhs, rhs, .. } => {
            expr(lhs, p, space, params)?;
            expr(rhs, p, space, params)
        }
        BCond::In { expr: e, list } => {
            expr(e, p, space, params)?;
            for i in list.iter_mut() {
                expr(i, p, space, params)?;
            }
            Ok(())
        }
        BCond::Text { lhs, rhs, .. } => {
            expr(lhs, p, space, params)?;
            expr(rhs, p, space, params)
        }
        BCond::IsNull { expr: e, .. } | BCond::Bare(e) => expr(e, p, space, params),
        BCond::And(v) | BCond::Or(v) => {
            for i in v.iter_mut() {
                cond(i, p, space, params)?;
            }
            Ok(())
        }
        BCond::Not(i) => cond(i, p, space, params),
        BCond::Exists { query, .. } => resolve(query, p, space, params),
        BCond::TreeWalk {
            expr: e, anchor, ..
        } => {
            expr(e, p, space, params)?;
            expr(anchor, p, space, params)
        }
    }
}

fn expr(
    e: &mut BExpr,
    p: &dyn KeyProvider,
    space: &SpaceId,
    params: &[(String, String)],
) -> Result<(), BlindError> {
    if let BExpr::BlindDigest {
        source,
        entity,
        field,
        normalize,
        ty,
    } = e
    {
        let raw = match source {
            // A null is a VALUE here, not a missing one: it digests the
            // reserved sentinel, which is what makes `null == null` true
            // in the digest column too.
            BlindSource::Null => None,
            BlindSource::Literal(v) => Some(v.clone()),
            BlindSource::Param(name) => Some(
                params
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| {
                        BlindError(format!(
                            "unbound_required_param: `${name}` is compared against a blind index \
                         and has no value; an unresolved digest can only match nothing"
                        ))
                    })?,
            ),
        };
        // Normalization is the schema's, and runs before the HMAC. A null
        // has nothing to normalize — `lower(null)` is null — so it goes
        // straight to the sentinel.
        let normalized = raw.map(|v| match normalize.as_deref() {
            Some("lower") => v.to_lowercase(),
            _ => v,
        });
        let digest = p
            .blind_index(space, entity, field, normalized.as_deref().into())
            .map_err(|err| BlindError(format!("key release: {err}")))?;
        *e = BExpr::ByteaLit {
            hex: ordo_crypto::hex(&digest),
            ty: *ty,
        };
        return Ok(());
    }
    match e {
        BExpr::Neg { expr: i, .. } => expr(i, p, space, params),
        BExpr::Binary { lhs, rhs, .. } => {
            expr(lhs, p, space, params)?;
            expr(rhs, p, space, params)
        }
        BExpr::Fn { args, .. } => {
            for a in args.iter_mut() {
                expr(a, p, space, params)?;
            }
            Ok(())
        }
        BExpr::Cond { arms, else_, .. } => {
            for (c, v) in arms.iter_mut() {
                cond(c, p, space, params)?;
                expr(v, p, space, params)?;
            }
            expr(else_, p, space, params)
        }
        BExpr::Agg { arg, mask, .. } => {
            if let Some(a) = arg.as_deref_mut() {
                expr(a, p, space, params)?;
            }
            if let Some(m) = mask.as_deref_mut() {
                cond(m, p, space, params)?;
            }
            Ok(())
        }
        BExpr::RowNumber {
            partition, order, ..
        } => {
            for k in partition.iter_mut() {
                expr(k, p, space, params)?;
            }
            for s in order.iter_mut() {
                let Sort { expr: se, .. } = s;
                expr(se, p, space, params)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
