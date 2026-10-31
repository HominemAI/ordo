//! The Tier-0 refinement envelope (tooling plan §B2): structured data, never
//! query text; narrowing only; every field validated against the op's
//! output-field manifest.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bind::{is_date, is_datetime, is_decimal};
use crate::error::{ApiError, ErrorCode};
use crate::spec::{CostClass, FieldSpec, FieldTy, OpSpec, RefineTier};

/// The shipped default for [`RefineLimits::max_conditions`].
pub const DEFAULT_MAX_CONDITIONS: usize = 16;
/// The shipped default for [`RefineLimits::max_in_values`].
pub const DEFAULT_MAX_IN_VALUES: usize = 200;

/// How large one refinement may be (owner, 2026-10-05, `refinement-default`
/// ask 6). Settings with shipped defaults, never fixed values: an unbounded
/// `in` is a scan the cost gate cannot see, and a deployment with a screen
/// that needs more raises the limit.
///
/// The refusal names the limit and the variable, so a caller who hits one
/// knows what to change and who can change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefineLimits {
    /// The most `where` conditions one refinement carries.
    /// `ORDO_REFINE_MAX_CONDITIONS`, default 16.
    pub max_conditions: usize,
    /// The most values one `in` condition carries.
    /// `ORDO_REFINE_MAX_IN_VALUES`, default 200.
    pub max_in_values: usize,
}

impl Default for RefineLimits {
    fn default() -> Self {
        RefineLimits {
            max_conditions: DEFAULT_MAX_CONDITIONS,
            max_in_values: DEFAULT_MAX_IN_VALUES,
        }
    }
}

impl RefineLimits {
    /// The environment's values, the defaults for what it leaves unset.
    ///
    /// A value that is not a number, or zero, reads as unset here. The
    /// host's `RuntimeConfig::from_env` refuses the boot on the same
    /// variables, so a deployment never runs on a default it did not
    /// choose, while a tool or test that sets nothing still works. Read
    /// once, where the door is built.
    pub fn from_env() -> RefineLimits {
        let var = |k: &str| std::env::var(k).ok();
        RefineLimits::parse(
            var("ORDO_REFINE_MAX_CONDITIONS").as_deref(),
            var("ORDO_REFINE_MAX_IN_VALUES").as_deref(),
        )
    }

    /// [`RefineLimits::from_env`] over given values, testable without
    /// touching the process environment.
    pub fn parse(max_conditions: Option<&str>, max_in_values: Option<&str>) -> RefineLimits {
        let num = |v: Option<&str>, d: usize| {
            v.and_then(|s| s.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(d)
        };
        RefineLimits {
            max_conditions: num(max_conditions, DEFAULT_MAX_CONDITIONS),
            max_in_values: num(max_in_values, DEFAULT_MAX_IN_VALUES),
        }
    }
}

/// The Tier-0 operator set, closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    IsNull,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefineCond {
    pub field: String,
    pub op: RefineOp,
    #[serde(default)]
    pub value: Value,
}

/// `{ where: [conds…], pick: [fields…] }` — conditions combine as AND;
/// `pick` (when present) selects a non-empty subset of output fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Refinement {
    #[serde(rename = "where", default)]
    pub where_: Vec<RefineCond>,
    #[serde(default)]
    pub pick: Option<Vec<String>>,
}

/// Parse the `$refine` body value. Shape errors are `invalid_refinement`
/// with serde's diagnosis in the message.
///
/// Every string value is normalized to NFC here, the stored form of all
/// text (`definition-language.md` §2), exactly as `bind.rs` does for a
/// parameter. A decomposed `e\u{301}` then matches the composed row, which
/// is what the device already assumes the door does.
pub fn parse(op: &OpSpec, raw: Value) -> Result<Refinement, ApiError> {
    let mut refinement: Refinement = serde_json::from_value(raw).map_err(|e| {
        ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            format!("malformed $refine envelope: {e}"),
        )
    })?;
    for cond in &mut refinement.where_ {
        nfc_in_place(&mut cond.value);
    }
    Ok(refinement)
}

fn nfc_in_place(v: &mut Value) {
    match v {
        Value::String(s) => {
            *s = unicode_normalization::UnicodeNormalization::nfc(s.as_str()).collect();
        }
        Value::Array(items) => items.iter_mut().for_each(nfc_in_place),
        _ => {}
    }
}

/// Validate a refinement against the op: tier, field manifest, **sealed-field
/// rules**, value shapes. The validated structure passes to the executor
/// untouched.
///
/// The sealed rules are the door's half of `std-library.md` §2.1 (SH-4 /
/// Y-033), applied in the order they are written there:
///
/// 1. **Not refinable by default.** A sealed field accepts no condition
///    unless its declaration opted in ([`FieldSpec::sealed_refinable`]).
/// 2. **Where enabled, equality only.** Sealed refinement rides the blind
///    index, which answers "same digest?" and nothing else. `eq` and `in` (a
///    set of equalities) are the whole vocabulary; `ne`, `lt`, `le`, `gt`,
///    `ge`, and `is_null` are `invalid_refinement`, because caller-controlled
///    ordering over decrypted values is the oracle rule 2 exists to close.
/// 3. **Aggregate-only ops accept no sealed condition at all** — if you may
///    not read the rows, you may not measure them.
///
/// Rules 1–3 are the declaration-time gate. Rule 4 — the request-time cost
/// gate — is [`check_cost`], and the two are run together by the router:
/// §7.1 is explicit that "implementing only one half leaves the hole open in
/// one direction", so neither is optional and neither subsumes the other.
///
/// Then the checks that keep a bad value from ever reaching the binder,
/// where it would be a 500 (`plan-refinement.md` section 4): the size
/// limits ([`RefineLimits`]), and each value's shape against the field's
/// type ([`FieldTy`]).
pub fn validate(
    op: &OpSpec,
    refinement: &Refinement,
    limits: &RefineLimits,
) -> Result<(), ApiError> {
    if op.refinable == RefineTier::None {
        return Err(ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            format!("op `{}` is not refinable", op.name),
        ));
    }

    if refinement.where_.len() > limits.max_conditions {
        return Err(ApiError::for_op(
            ErrorCode::InvalidRefinement,
            op,
            format!(
                "this refinement has {} conditions and the limit is {}. The limit is the \
                 deployment setting `ORDO_REFINE_MAX_CONDITIONS` (default {}). Fold values of \
                 one field into a single `in`, or ask the deployment owner to raise it",
                refinement.where_.len(),
                limits.max_conditions,
                DEFAULT_MAX_CONDITIONS,
            ),
        ));
    }

    for cond in &refinement.where_ {
        let field = check_condition_field(op, &cond.field)?;
        let shape_err = |msg: String| {
            Err(ApiError::for_op(ErrorCode::InvalidRefinement, op, msg)
                .with_param(cond.field.clone()))
        };
        if field.ty == FieldTy::Json {
            return shape_err(format!(
                "`{}` is a json field, and no condition may name a json field: the language \
                 has no operators over a document's insides yet. Pick it and read it whole",
                cond.field
            ));
        }
        if !field.refinable {
            // Sealed fields are the reason this flag exists, and they get the
            // reason spelled out; a field the author simply did not open to
            // refinement gets the plain answer.
            return shape_err(if field.sealed {
                format!(
                    "`{}` is a sealed field and is not refinable — refining a sealed field is \
                     opt-in at its declaration, because the capability is a disclosure and \
                     belongs on the consent page",
                    cond.field
                )
            } else {
                format!("field `{}` is not refinable", cond.field)
            });
        }
        if field.sealed {
            if op.aggregate_only {
                return shape_err(format!(
                    "`{}` is a sealed field and `{}` is an aggregate-only op — if you may not \
                     read the rows, you may not measure them",
                    cond.field, op.name
                ));
            }
            if cond.op != RefineOp::Eq && cond.op != RefineOp::In {
                return shape_err(format!(
                    "sealed field `{}` accepts equality only (`eq`, `in`) — a sealed condition \
                     rides the blind index, and ordering comparisons over sealed values are an \
                     oracle",
                    cond.field
                ));
            }
        }
        if field.ty == FieldTy::Blob {
            return shape_err(format!(
                "`{}` is a blob, and a blob has no order and no meaningful equality, so no \
                 condition may name it",
                cond.field
            ));
        }
        match cond.op {
            RefineOp::In => {
                let Some(items) = cond.value.as_array() else {
                    return shape_err(format!("`in` on `{}` takes an array value", cond.field));
                };
                if items.len() > limits.max_in_values {
                    return shape_err(format!(
                        "`in` on `{}` has {} values and the limit is {}. The limit is the \
                         deployment setting `ORDO_REFINE_MAX_IN_VALUES` (default {}); ask the \
                         deployment owner to raise it, or narrow the set with another condition",
                        cond.field,
                        items.len(),
                        limits.max_in_values,
                        DEFAULT_MAX_IN_VALUES,
                    ));
                }
                if items.is_empty() && field.sealed {
                    // On an open field `in []` matches nothing. A sealed
                    // field's equality rides the blind index one digest at
                    // a time, and there is no digest to compare with none.
                    return shape_err(format!(
                        "`in` on sealed field `{}` takes at least one value",
                        cond.field
                    ));
                }
                for item in items {
                    if item.is_null() {
                        // A null member matches a null value: `in` is a
                        // chain of `==`, and `null == null` is true.
                        continue;
                    }
                    if let Err(expected) = check_value(&field.ty, item) {
                        return shape_err(format!(
                            "every value of `in` on `{}` must be {expected}, or null",
                            cond.field
                        ));
                    }
                }
            }
            RefineOp::IsNull => {
                if !cond.value.is_null() {
                    return shape_err(format!("`is_null` on `{}` takes no value", cond.field));
                }
            }
            _ => {
                if cond.value.is_null() || cond.value.is_array() || cond.value.is_object() {
                    return shape_err(format!(
                        "`{}` needs one non-null scalar value (null tests are `is_null`)",
                        cond.field
                    ));
                }
                if let Err(expected) = check_value(&field.ty, &cond.value) {
                    return shape_err(format!("the value of `{}` must be {expected}", cond.field));
                }
            }
        }
    }

    if let Some(pick) = &refinement.pick {
        if pick.is_empty() {
            return Err(ApiError::for_op(
                ErrorCode::InvalidRefinement,
                op,
                "pick must name at least one field (omit it to keep all fields)",
            ));
        }
        for (i, field) in pick.iter().enumerate() {
            check_output_field(op, field)?;
            if pick[..i].contains(field) {
                // The projection would carry the column twice, and the
                // statement failed on the ambiguous name, which came back
                // as a 500. The device's `pick` is a set too.
                return Err(ApiError::for_op(
                    ErrorCode::InvalidRefinement,
                    op,
                    format!("`pick` names `{field}` twice"),
                )
                .with_param(field.clone()));
            }
        }
    }
    Ok(())
}

/// The **effective cost class** of this request: `api-engine-design.md`
/// §7.1's
///
/// ```text
/// effective = registered ⊔ class_of(refinement fields, refinement ops)
/// ```
///
/// The registered class and every per-field class were computed by
/// `ordo_plan::cost` against the op's own plan and handed over on the
/// [`OpSpec`]; the join is the lattice order and nothing here classifies.
/// That is deliberate — a door with its own cost model is a second cost
/// model, and two cost models that can disagree would be worse than none.
///
/// Only `where` conditions are joined. A `pick` names fields too, but it
/// *removes* columns from a result the op already produces: a sealed field
/// in the manifest is one the plan already opens, so narrowing to it costs
/// nothing new. Refinement narrows results; only a condition changes what
/// must be examined to produce them.
///
/// Total by construction, and callable before [`validate`] — an unknown
/// field simply contributes nothing, because an unknown field is
/// `invalid_refinement` and never reaches an executor.
pub fn effective_class(op: &OpSpec, refinement: &Refinement) -> CostClass {
    refinement
        .where_
        .iter()
        .filter_map(|cond| {
            op.output_fields
                .iter()
                .find(|f| f.filter && f.name == cond.field)
        })
        .fold(op.cost.class, |acc, field| acc.join(field.refine_class))
}

/// §2.1 rule 4 / §7.1's rejection: **the effective class is recomputed per
/// request, and a refinement that pushes the op to `unbounded-host` without
/// the op's `unbounded` acknowledgment is refused** — `refinement_unbounded`
/// (400), naming the field.
///
/// # What this is protecting, in one shape
///
/// `std.E.count` is registered bounded: it returns one row, so the row-bound
/// prover proves `<= 1` and the op needs no acknowledgment. Its refinement
/// conditions apply **before** the reduce, over the whole set the reduce
/// examines — so a condition on a sealed field with no blind index turns one
/// cheap number into a host-side decrypt of every row in the space, under an
/// op nobody registered as unbounded. The registration gates
/// (`unacknowledged_unbounded`, `unacknowledged_unguarded`, the venue gate)
/// exist to stop unacknowledged work; without this one, a refinement walks
/// around them at request time.
///
/// # Why the acknowledgment is an exemption rather than a stronger rule
///
/// Because it is a human's sentence: *this op's work grows with the size of
/// the space, and I mean it* (`definition-language.md`, cost classes — the
/// same word covers unbounded-`sql` and `unbounded-host`). §7.1 rejects only
/// the op that "quietly became unbounded at request time". An op that said
/// so at registration is serving what it advertised.
///
/// Returns the effective class on success, because that class is what gets
/// served and is what an observability layer should carry beside the
/// registered one.
pub fn check_cost(op: &OpSpec, refinement: &Refinement) -> Result<CostClass, ApiError> {
    let effective = effective_class(op, refinement);
    if effective != CostClass::UnboundedHost || op.cost.unbounded_ack {
        return Ok(effective);
    }
    // Name the field that did it. The first condition whose own class
    // reaches unbounded-host is the honest culprit; if none does, the
    // registered class was already there (impossible for an unacknowledged
    // op, which the venue gate refuses at registration) and the message
    // says so rather than inventing a field.
    let culprit = refinement
        .where_
        .iter()
        .find(|cond| {
            op.output_fields.iter().any(|f| {
                f.filter && f.name == cond.field && f.refine_class == CostClass::UnboundedHost
            })
        })
        .map(|cond| cond.field.clone());
    let message = match &culprit {
        Some(field) => format!(
            "refining `{}` on `{}` raises this request's effective cost class to \
             `{}` (registered `{}`) — the condition cannot be answered in the database, so it \
             opens sealed payloads for an unbounded number of rows. `{}` was not registered with \
             the `unbounded` acknowledgment, and a refinement cannot acknowledge on its author's \
             behalf",
            field,
            op.name,
            effective.as_str(),
            op.cost.class.as_str(),
            op.name,
        ),
        None => format!(
            "this request's effective cost class is `{}` and `{}` was not registered with the \
             `unbounded` acknowledgment",
            effective.as_str(),
            op.name,
        ),
    };
    let err = ApiError::for_op(ErrorCode::RefinementUnbounded, op, message);
    Err(match culprit {
        Some(field) => err.with_param(field),
        None => err,
    })
}

/// §7.1's other refusal, the venue (`refinement-default` ask 3, ruled
/// 2026-10-05): **a condition that only the host can answer is refused at the
/// door, naming the field, until host-side filtering is built.**
///
/// A sealed field with no blind index is answered by opening every
/// candidate payload. [`check_cost`] refuses that when it is unbounded and
/// unacknowledged; what it lets through (a bounded candidate set, or an op
/// that said `unbounded`) the executor cannot run, and it used to fail
/// there with a message that named no field and no fix. Run after
/// [`check_cost`], so an unacknowledged unbounded condition keeps its own,
/// more specific refusal.
pub fn check_venue(op: &OpSpec, refinement: &Refinement) -> Result<(), ApiError> {
    for cond in &refinement.where_ {
        let Some(field) = op
            .output_fields
            .iter()
            .find(|f| f.filter && f.name == cond.field)
        else {
            continue;
        };
        if matches!(
            field.refine_class,
            CostClass::BoundedHost | CostClass::UnboundedHost
        ) {
            return Err(ApiError::for_op(
                ErrorCode::SealedUnsupported,
                op,
                format!(
                    "a condition on `{}` is answered by opening sealed payloads on the host, \
                     and host-side filtering is not built yet. Give the field a blind index \
                     (`sealed indexed refinable`) so `eq` and `in` run in the database, or \
                     filter on an open field",
                    cond.field
                ),
            )
            .with_param(cond.field.clone()));
        }
    }
    Ok(())
}

/// A value against a field's type. `Err` carries the words for what was
/// expected. The shapes are the wire's: what the op returns for the field
/// is what a condition on it takes.
fn check_value(ty: &FieldTy, v: &Value) -> Result<(), &'static str> {
    // Postgres text holds no NUL. A value with one fails the statement with
    // `string contains embedded null`, which came back as a 500, and it can
    // match no stored value anyway.
    let clean = |v: &Value| v.as_str().is_some_and(|s| !s.contains('\0'));
    let ok = match ty {
        FieldTy::Any => !v.is_array() && !v.is_object() && (!v.is_string() || clean(v)),
        FieldTy::Text | FieldTy::Id | FieldTy::Uri => clean(v),
        FieldTy::Int => v.as_i64().is_some(),
        FieldTy::Money { places } | FieldTy::Decimal { places } => {
            let digits = match v {
                Value::String(s) if is_decimal(s) => s.clone(),
                Value::Number(n) if n.is_i64() => n.to_string(),
                _ => return Err(numeric_shape(ty)),
            };
            if !numeric_fits(ty, &digits, *places) {
                return Err(match ty {
                    FieldTy::Money { .. } => {
                        "a money value of at most 18 digits (the column holds whole minor \
                         units in a 64-bit integer)"
                    }
                    _ => "a decimal of at most 38 digits",
                });
            }
            true
        }
        FieldTy::Bool => v.is_boolean(),
        FieldTy::Date => v.as_str().is_some_and(is_date),
        FieldTy::Datetime => v.as_str().is_some_and(is_datetime),
        FieldTy::Duration { .. } => v.as_u64().is_some(),
        FieldTy::Blob | FieldTy::Json => false,
    };
    if ok {
        return Ok(());
    }
    Err(match ty {
        FieldTy::Any => "one scalar, with no NUL character in a string",
        FieldTy::Text | FieldTy::Uri => "a string with no NUL character",
        FieldTy::Id => "an id string with no NUL character",
        FieldTy::Int => "an integer",
        FieldTy::Money { .. } | FieldTy::Decimal { .. } => numeric_shape(ty),
        FieldTy::Bool => "a boolean",
        FieldTy::Date => "a date string `YYYY-MM-DD`",
        FieldTy::Datetime => "a datetime string `YYYY-MM-DDTHH:MM:SSZ` (UTC, `Z` required)",
        FieldTy::Duration { .. } => {
            "a non-negative integer in the field's unit, as the op returns it"
        }
        FieldTy::Blob => "nothing: a blob takes no condition",
        FieldTy::Json => "nothing: a json field takes no condition",
    })
}

fn numeric_shape(ty: &FieldTy) -> &'static str {
    match ty {
        FieldTy::Money { .. } => {
            "a decimal string like \"-12.50\" or an integer (money never rides as a float)"
        }
        _ => "a decimal string like \"0.0825\" or an integer (a decimal never rides as a float)",
    }
}

/// Whether a money or decimal value, already of a decimal's shape, is one
/// the executor can build a literal for. A money column holds whole minor
/// units in a 64-bit integer, so a value is bounded at 18 digits once
/// written at the field's places; a decimal literal is read into a 128-bit
/// integer with its scale, so 38 digits in all. Past either, the binder
/// cannot fit the value and the request came back as a 500 (R8). The bound
/// is on the digits that carry value: leading zeros, and trailing zeros
/// after the point, count for nothing.
fn numeric_fits(ty: &FieldTy, digits: &str, places: u8) -> bool {
    let unsigned = digits.strip_prefix('-').unwrap_or(digits);
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    match ty {
        FieldTy::Money { .. } => {
            // Minor units: the integer digits plus `places` digits of the
            // fraction. A finer fraction compares exactly and adds nothing
            // to what the column must hold.
            int.len() + frac.len().min(usize::from(places)) <= 18
        }
        _ => int.len() + frac.len() <= 38,
    }
}

/// The field a `where` condition names. On most ops that is any output; on
/// `std.E.count` it is a field of `std.E.list`'s manifest (§2.1).
fn check_condition_field<'a>(op: &'a OpSpec, field: &str) -> Result<&'a FieldSpec, ApiError> {
    find_field(op, field, |f| f.filter, "fields a condition may name")
}

/// The field a `pick` names: an output of this op.
fn check_output_field<'a>(op: &'a OpSpec, field: &str) -> Result<&'a FieldSpec, ApiError> {
    find_field(op, field, |f| f.output, "output fields")
}

fn find_field<'a>(
    op: &'a OpSpec,
    field: &str,
    eligible: impl Fn(&FieldSpec) -> bool,
    listing: &str,
) -> Result<&'a FieldSpec, ApiError> {
    if let Some(spec) = op
        .output_fields
        .iter()
        .find(|f| eligible(f) && f.name == field)
    {
        return Ok(spec);
    }
    let manifest: Vec<&str> = op
        .output_fields
        .iter()
        .filter(|f| eligible(f))
        .map(|f| f.name.as_str())
        .collect();
    Err(ApiError::for_op(
        ErrorCode::InvalidRefinement,
        op,
        format!(
            "unknown field `{field}` — {listing}: {}",
            manifest.join(", ")
        ),
    )
    .with_param(field.to_string()))
}
