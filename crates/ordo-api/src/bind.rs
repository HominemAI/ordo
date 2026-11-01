//! Parameter binding: type checks against the closed `ParamTy` inventory and
//! bounds enforcement — all at the door, before anything executes. Errors are
//! loud and name valid alternatives.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::{ApiError, ErrorCode};
use crate::spec::{BoundParams, OpSpec, ParamSpec, ParamTy};

/// Bind a JSON object of bare-keyed parameters against the op's declared
/// specs. Unknown keys are rejected first, so typos surface before "missing"
/// noise. An explicit `null` counts as absent.
pub fn bind_params(op: &OpSpec, supplied: &Map<String, Value>) -> Result<BoundParams, ApiError> {
    for key in supplied.keys() {
        if !op.params.iter().any(|p| p.name == *key) {
            let valid: Vec<&str> = op.params.iter().map(|p| p.name.as_str()).collect();
            let listing = if valid.is_empty() {
                "this op takes none".to_string()
            } else {
                valid.join(", ")
            };
            return Err(ApiError::for_op(
                ErrorCode::UnknownParam,
                op,
                format!("unknown parameter `{key}` — valid parameters: {listing}"),
            )
            .with_param(key.clone()));
        }
    }

    let mut out = Map::new();
    for p in &op.params {
        let value = supplied.get(&p.name).filter(|v| !v.is_null());
        match value {
            None if p.required => {
                return Err(ApiError::for_op(
                    ErrorCode::MissingParam,
                    op,
                    format!("missing required parameter `{}`", p.name),
                )
                .with_param(p.name.clone()));
            }
            None => {}
            Some(v) => {
                // 9u: canonicalize BEFORE validating, not after. DL §2 makes
                // NFC the stored form platform-wide, so a read filtering on a
                // decomposed argument must match the composed row — a value
                // normalized on write and compared as supplied fixes nothing.
                //
                // The order is load-bearing for `text(n)`: NFC changes the
                // character COUNT (decomposed `é` is two chars, composed is
                // one), so validating first would refuse a value that fits,
                // and would refuse it inconsistently depending on how the
                // caller's editor happened to encode it.
                let v = canonicalize(v);
                validate_value(op, p, &v)?;
                out.insert(p.name.clone(), v);
            }
        }
    }
    Ok(BoundParams(out))
}

/// A caller's value in its stored form.
///
/// Only strings carry text, and only `text`/`id` parameters are strings the
/// platform stores or compares — but this normalizes every string rather than
/// switching on the declared type, because the two are the same set and a
/// switch would be one more place to forget a variant when a type is added.
/// Non-strings are returned untouched.
///
/// See `ordo_syntax::text` for why NFC and not NFKC, and for the other two
/// doors text enters by.
fn canonicalize(v: &Value) -> Value {
    match v.as_str() {
        Some(s) => Value::String(unicode_normalization::UnicodeNormalization::nfc(s).collect()),
        None => v.clone(),
    }
}

/// Coerce live-`GET` query strings into the JSON shapes the binder expects:
/// `int` parses, `bool` accepts `true/false/1/0`, everything else stays the
/// string as given (dates, money, ids, durations are strings on the wire
/// anyway). Failed coercions stay strings — the binder rejects them with the
/// same message a `POST` would get.
pub fn coerce_query(op: &OpSpec, query: &HashMap<String, String>) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, raw) in query {
        let ty = op.params.iter().find(|p| p.name == *key).map(|p| &p.ty);
        let value = match ty {
            Some(ParamTy::Int) => raw
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(raw.clone())),
            // A document in a query string is its JSON text; one that
            // does not parse stays the string, which is itself a JSON
            // value, so the caller gets back exactly what they sent.
            Some(ParamTy::Json) => {
                serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
            }
            Some(ParamTy::Bool) => match raw.as_str() {
                "true" | "1" => Value::Bool(true),
                "false" | "0" => Value::Bool(false),
                _ => Value::String(raw.clone()),
            },
            _ => Value::String(raw.clone()),
        };
        out.insert(key.clone(), value);
    }
    out
}

fn validate_value(op: &OpSpec, p: &ParamSpec, v: &Value) -> Result<(), ApiError> {
    let invalid = |expected: &str| {
        Err(ApiError::for_op(
            ErrorCode::InvalidParam,
            op,
            format!("parameter `{}` must be {expected}", p.name),
        )
        .with_param(p.name.clone()))
    };
    match &p.ty {
        ParamTy::Int => {
            let Some(n) = v.as_i64() else {
                return invalid("an integer");
            };
            check_bounds(op, p, &n.to_string(), false)
        }
        ParamTy::Money { places } => {
            let Some(s) = v.as_str() else {
                return invalid("a decimal string (money never rides as a float)");
            };
            if !is_decimal(s) {
                return invalid("a decimal string like \"-12.50\"");
            }
            // `money(2)` holds cents: `"29.999"` would have to round to be
            // stored, and nothing rounds on its own (27ak). Trailing zeros
            // name no value, so `"29.90"` fits a `money(1)`.
            if significant_places(s) > u32::from(*places) {
                return invalid(&format!(
                    "a decimal string with at most {places} places (`money({places})`)"
                ));
            }
            check_bounds(op, p, s, false)
        }
        ParamTy::Decimal { places } => {
            let Some(s) = v.as_str() else {
                return invalid("a decimal string (a decimal never rides as a float)");
            };
            if !is_decimal(s) {
                return invalid("a decimal string like \"0.0825\"");
            }
            if significant_places(s) > u32::from(*places) {
                return invalid(&format!(
                    "a decimal string with at most {places} places (`decimal({places})`)"
                ));
            }
            check_bounds(op, p, s, false)
        }
        ParamTy::Text { max_len } => {
            let Some(s) = v.as_str() else {
                return invalid("a string");
            };
            // Postgres text holds no NUL, so the statement would fail with
            // `string contains embedded null`, which came back as a 500.
            if s.contains('\0') {
                return invalid("a string with no NUL character");
            }
            if let Some(max) = max_len {
                let len = s.chars().count();
                if len > *max as usize {
                    return Err(ApiError::for_op(
                        ErrorCode::BoundViolation,
                        op,
                        format!(
                            "parameter `{}` exceeds text({max}): {len} characters",
                            p.name
                        ),
                    )
                    .with_param(p.name.clone()));
                }
            }
            Ok(())
        }
        ParamTy::Date => {
            let Some(s) = v.as_str() else {
                return invalid("a date string `YYYY-MM-DD`");
            };
            if !is_date(s) {
                return invalid("a date string `YYYY-MM-DD`");
            }
            check_bounds(op, p, s, true)
        }
        ParamTy::Bool => {
            if !v.is_boolean() {
                return invalid("a boolean");
            }
            Ok(())
        }
        ParamTy::Id => {
            let Some(s) = v.as_str() else {
                return invalid("a typed id string like `txn_01J8...`");
            };
            if !is_typed_id(s) {
                return invalid("a typed id string like `txn_01J8...`");
            }
            Ok(())
        }
        ParamTy::Duration => {
            let Some(s) = v.as_str() else {
                return invalid("a duration string like `90d` (units: ms s m h d w mo y)");
            };
            if !is_duration(s) {
                return invalid("a duration string like `90d` (units: ms s m h d w mo y)");
            }
            Ok(())
        }
        // Until this arm existed, a `datetime` parameter bound as
        // unbounded text: `ordo_host::param_ty` had a wildcard and
        // `ParamTy` had no member, so `$t: datetime` — which parses, and
        // which a test in `declparse.rs` writes — reached a plan as
        // whatever string the caller sent.
        //
        // No bounds. `Bounds` is documented as "canonical strings: exact
        // decimals for int/money, YYYY-MM-DD for date. Other types take no
        // bounds in v1", and `param_bound` in the parser admits only int,
        // decimal and date literals, so a datetime bound is unspellable
        // rather than unenforced.
        ParamTy::Datetime => {
            let Some(s) = v.as_str() else {
                return invalid("a datetime string `YYYY-MM-DDTHH:MM:SSZ` (UTC, `Z` required)");
            };
            if !is_datetime(s) {
                return invalid("a datetime string `YYYY-MM-DDTHH:MM:SSZ` (UTC, `Z` required)");
            }
            Ok(())
        }
        // An address. The check is deliberately syntactic and nothing
        // more: the platform does not fetch a `uri`, does not resolve it,
        // and does not judge its host — DL §2 says "referenced, not held",
        // and a door that validated reachability would be holding it.
        //
        // What it does refuse is the shape that is never an address and is
        // often an injection: control characters, whitespace, an empty
        // string, no scheme, or a length nothing legitimate reaches.
        // A document: any JSON value at all, bounded by the deployment's
        // size and depth settings rather than by the type (28b). `null`
        // never reaches here: an explicit `null` is an absent parameter.
        ParamTy::Json => match crate::json::check(v, &crate::json::limits()) {
            Ok(()) => Ok(()),
            Err(why @ crate::json::JsonRefusal::Nul) => {
                invalid(&format!("a JSON document; this one {why}"))
            }
            Err(why) => Err(ApiError::for_op(
                ErrorCode::BoundViolation,
                op,
                format!("parameter `{}` {why}", p.name),
            )
            .with_param(p.name.clone())),
        },
        // A file handle, as the blob door mints and an op answers it. Only
        // its shape is checked here; whether it names a file the caller
        // may read is the row guard's, when the function reads it.
        ParamTy::Blob => {
            let ok = v
                .as_str()
                .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));
            if !ok {
                return invalid(
                    "a file handle: the 32 hex characters an op answers for a `blob` field \
                     after the upload",
                );
            }
            Ok(())
        }
        ParamTy::Uri => {
            let Some(s) = v.as_str() else {
                return invalid("a URI string like `https://example.com/x`");
            };
            if !is_uri(s) {
                return invalid(
                    "a URI string like `https://example.com/x` — a scheme, then a body, \
                     no spaces or control characters, at most 2048 bytes",
                );
            }
            Ok(())
        }
    }
}

/// Bounds enforcement over a canonical string: exact-decimal comparison for
/// numerics, lexicographic (= chronological) for ISO dates.
fn check_bounds(op: &OpSpec, p: &ParamSpec, canonical: &str, date: bool) -> Result<(), ApiError> {
    let Some(bounds) = &p.bounds else {
        return Ok(());
    };
    let cmp = |a: &str, b: &str| {
        if date {
            Some(a.cmp(b))
        } else {
            cmp_decimal(a, b)
        }
    };
    let violation = |relation: &str, limit: &str| {
        Err(ApiError::for_op(
            ErrorCode::BoundViolation,
            op,
            format!(
                "parameter `{}` violates its bound: must be {relation} {limit}",
                p.name
            ),
        )
        .with_param(p.name.clone()))
    };
    if let Some(min) = &bounds.min {
        if cmp(canonical, min) == Some(Ordering::Less) {
            return violation(">=", min);
        }
    }
    if let Some(max) = &bounds.max {
        if cmp(canonical, max) == Some(Ordering::Greater) {
            return violation("<=", max);
        }
    }
    Ok(())
}

// ── wire-shape validators ──

pub(crate) fn is_decimal(s: &str) -> bool {
    let rest = s.strip_prefix('-').unwrap_or(s);
    let mut parts = rest.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// Digits after the point once trailing zeros are dropped: `"29.90"` is 1.
/// Called only on a string [`is_decimal`] accepted.
fn significant_places(s: &str) -> u32 {
    match s.split_once('.') {
        Some((_, frac)) => frac.trim_end_matches('0').len() as u32,
        None => 0,
    }
}

pub(crate) fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(|c| c.is_ascii_digit());
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let y: u16 = s[0..4].parse().unwrap_or(0);
    let m: u8 = s[5..7].parse().unwrap_or(0);
    let d: u8 = s[8..10].parse().unwrap_or(0);
    // A day the month does not have (`2026-02-31`) is a date Postgres
    // refuses, and a refusal there came back as a 500. Year 0 is not a
    // date either: Postgres counts years from 1.
    y >= 1 && d >= 1 && d <= days_in_month(y, m)
}

/// The days of month `m` in year `y`, 0 for a month that is not one.
fn days_in_month(y: u16, m: u8) -> u8 {
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`, and nothing else.
///
/// The `Z` is mandatory and no offset is accepted, matching what the
/// generated clients are told the wire is
/// (`spell::scalar(Datetime).wire`). Accepting `+02:00` would mean the
/// door deciding a timezone conversion, and `implementation.md` puts every
/// instant in UTC precisely so that decision has no place to happen. No
/// fractional seconds, for the same reason: two spellings of one instant
/// is one more than a canonical form allows.
pub(crate) fn is_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z' {
        return false;
    }
    if !is_date(&s[..10]) {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(|c| c.is_ascii_digit());
    if !(digits(11..13) && digits(14..16) && digits(17..19)) {
        return false;
    }
    let hh: u8 = s[11..13].parse().unwrap_or(99);
    let mm: u8 = s[14..16].parse().unwrap_or(99);
    // 60 is allowed: a leap second is a real instant, and refusing one
    // would make the door disagree with the clocks that emit them.
    let ss: u8 = s[17..19].parse().unwrap_or(99);
    hh <= 23 && mm <= 59 && ss <= 60
}

/// A syntactic address: `scheme:body`.
///
/// Deliberately not a full RFC 3986 parser. What the door owes a `uri`
/// parameter is the check that distinguishes an address from a string
/// somebody pasted into the wrong field, plus the refusal of the shapes
/// that are never addresses and often are attacks — control characters
/// (which end up in a log line or a header), whitespace (which splits a
/// header), and unbounded length.
pub(crate) fn is_uri(s: &str) -> bool {
    if s.is_empty() || s.len() > 2048 {
        return false;
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, body)) = s.split_once(':') else {
        return false;
    };
    !body.is_empty()
        && !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

///
/// A typed id is what the platform mints: the entity's key, then `_`, then
/// a body. A body is one segment (`txn_1`) or several joined by `_`, because
/// a serving log mints `<key>_w<log prefix>_<count>` (`note_wf121e1ce_1`).
/// A key derived from a multi-word name is hyphen-joined (`orde-item`), and
/// an explicit key is an identifier, so it may carry digits and capitals.
/// Every segment is non-empty ASCII alphanumerics and hyphens, the first
/// character is a letter, and there is at least one `_`.
fn is_typed_id(s: &str) -> bool {
    if s.len() > 128 || !s.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    let mut segments = 0;
    for seg in s.split('_') {
        if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

fn is_duration(s: &str) -> bool {
    let digits_end = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_end == 0 || digits_end > 12 {
        return false;
    }
    matches!(
        &s[digits_end..],
        "ms" | "s" | "m" | "h" | "d" | "w" | "mo" | "y"
    )
}

// ── exact decimal comparison (no floats, ever) ──

struct Dec {
    neg: bool,
    int: String,
    frac: String,
}

fn parse_decimal(s: &str) -> Option<Dec> {
    if !is_decimal(s) {
        return None;
    }
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    // Normalize zero so -0 == 0.
    let neg = neg && !(int.is_empty() && frac.is_empty());
    Some(Dec {
        neg,
        int: int.to_string(),
        frac: frac.to_string(),
    })
}

/// Exact comparison of two decimal strings; `None` if either fails to parse
/// (bounds come from the trusted registry, so `None` never rejects a caller).
pub(crate) fn cmp_decimal(a: &str, b: &str) -> Option<Ordering> {
    let a = parse_decimal(a)?;
    let b = parse_decimal(b)?;
    let ord = match (a.neg, b.neg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (neg, _) => {
            let mag = cmp_magnitude(&a, &b);
            if neg {
                mag.reverse()
            } else {
                mag
            }
        }
    };
    Some(ord)
}

fn cmp_magnitude(a: &Dec, b: &Dec) -> Ordering {
    // Leading zeros are trimmed, so a longer integer part is a bigger number.
    a.int
        .len()
        .cmp(&b.int.len())
        .then_with(|| a.int.cmp(&b.int))
        .then_with(|| cmp_frac(&a.frac, &b.frac))
}

fn cmp_frac(a: &str, b: &str) -> Ordering {
    let width = a.len().max(b.len());
    let digit = |t: &str, i: usize| t.as_bytes().get(i).copied().unwrap_or(b'0');
    for i in 0..width {
        match digit(a, i).cmp(&digit(b, i)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_comparison_is_exact() {
        assert_eq!(cmp_decimal("10.50", "10.5"), Some(Ordering::Equal));
        assert_eq!(cmp_decimal("-0", "0"), Some(Ordering::Equal));
        assert_eq!(cmp_decimal("2", "10"), Some(Ordering::Less));
        assert_eq!(
            cmp_decimal("0.30000000000000004", "0.3"),
            Some(Ordering::Greater)
        );
        assert_eq!(cmp_decimal("-2", "-10"), Some(Ordering::Greater));
        assert_eq!(cmp_decimal("100", "99.999999"), Some(Ordering::Greater));
    }

    #[test]
    fn wire_shapes() {
        assert!(is_decimal("-12.50") && is_decimal("0") && !is_decimal("1.") && !is_decimal("x"));
        assert!(is_date("2026-07-01") && !is_date("2026-13-01") && !is_date("2026-7-1"));
        // The calendar, not just the shape: Postgres refuses a day the month
        // does not have, and the door must refuse it first.
        assert!(is_date("2024-02-29") && is_date("2000-02-29") && is_date("2026-04-30"));
        assert!(!is_date("2026-02-29") && !is_date("1900-02-29") && !is_date("2026-04-31"));
        assert!(!is_date("2026-02-31") && !is_date("2026-01-00") && !is_date("0000-01-01"));
        assert!(!is_datetime("2026-02-30T00:00:00Z") && is_datetime("2026-01-01T23:59:60Z"));
        assert!(is_typed_id("txn_01J8ZKQ4X7") && !is_typed_id("nounderscore"));
        // Every shape the platform mints, and shapes it never does.
        for minted in [
            "note_w1",
            "note_wf121e1ce63577793_1",
            "orde-item_wf121e1ce63577793_12",
            "Note_w7",
        ] {
            assert!(is_typed_id(minted), "{minted} is minted and must bind");
        }
        for never in [
            "",
            "_",
            "note_",
            "_w1",
            "note__w1",
            "9note_w1",
            "note_w 1",
            "note_w1;x",
            "note-w1",
        ] {
            assert!(!is_typed_id(never), "{never:?} is not a minted shape");
        }
        assert!(is_duration("90d") && is_duration("1mo") && !is_duration("5q"));
    }
}
