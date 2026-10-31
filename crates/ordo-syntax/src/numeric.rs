//! Exact decimal arithmetic on literal spellings, for money and `decimal`.
//!
//! Ruled 2026-10-04 (27ak): `money` is stored as whole minor units and
//! written in major units. Source says `29.99` for a `money(2)` field and
//! the compiler stores `2999`; the wire says `"29.99"` and the door does
//! the same. Every one of those conversions is a string of digits moved
//! by a power of ten, so they share this module rather than each doing its
//! own arithmetic. There is no float anywhere here.

use crate::ast::Expr;

/// Why a literal does not become whole minor units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// The expression is not a numeric literal.
    NotNumeric,
    /// The literal has more places than the slot holds: `29.999` for a
    /// `money(2)`. Storing it would round, and nothing rounds implicitly.
    TooFine { written: u32 },
    /// The value is outside what a `bigint` holds once scaled.
    Overflow,
}

/// `"-12.50"` as `(-1250, 2)`: the digits without the point, and how many
/// followed it. `None` for anything that is not an optional sign, digits,
/// and at most one point with digits on both sides.
pub fn decimal_parts(digits: &str) -> Option<(i128, u32)> {
    let (neg, body) = match digits.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, digits),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || (body.contains('.') && frac.is_empty())
    {
        return None;
    }
    let mut v: i128 = 0;
    for b in int.bytes().chain(frac.bytes()) {
        v = v.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    Some((if neg { -v } else { v }, frac.len() as u32))
}

/// A number written in major units, as whole minor units at `places`.
///
/// `"29.99"` at 2 is `2999`; `"30"` at 2 is `3000`; `"29.90"` at 1 is
/// `299`, because the trailing zero names no value. `"29.999"` at 2 is
/// [`Fit::TooFine`]: the slot would have to round it.
pub fn minor_units(digits: &str, places: u8) -> Result<i64, Fit> {
    let (mut v, mut scale) = decimal_parts(digits).ok_or(Fit::NotNumeric)?;
    let written = scale;
    // A trailing zero after the point names no value, so `29.90` fits a
    // `money(1)`. Only digits that carry value decide whether it fits.
    while scale > u32::from(places) && v % 10 == 0 {
        v /= 10;
        scale -= 1;
    }
    if scale > u32::from(places) {
        return Err(Fit::TooFine { written });
    }
    let lift = 10i128
        .checked_pow(u32::from(places) - scale)
        .ok_or(Fit::Overflow)?;
    let minor = v.checked_mul(lift).ok_or(Fit::Overflow)?;
    i64::try_from(minor).map_err(|_| Fit::Overflow)
}

/// A numeric literal expression (`IntLit` or `DecimalLit`) as whole minor
/// units at `places`. See [`minor_units`].
pub fn literal_minor_units(e: &Expr, places: u8) -> Result<i64, Fit> {
    match e {
        Expr::IntLit { value, .. } => minor_units(&value.to_string(), places),
        Expr::DecimalLit { digits, .. } => minor_units(digits, places),
        _ => Err(Fit::NotNumeric),
    }
}

/// Whole minor units spelled in major units with exactly `places` digits
/// after the point: `2999` at 2 is `"29.99"`, `-5` at 2 is `"-0.05"`,
/// `2999` at 0 is `"2999"`. This is money's one wire spelling.
pub fn major_spelling(minor: i128, places: u8) -> String {
    let neg = minor < 0;
    let digits = minor.unsigned_abs().to_string();
    let p = usize::from(places);
    let body = if p == 0 {
        digits
    } else {
        let padded = format!("{digits:0>width$}", width = p + 1);
        let (int, frac) = padded.split_at(padded.len() - p);
        format!("{int}.{frac}")
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// An exact decimal re-spelled with exactly `places` digits after the
/// point, when that loses nothing: `"0.08"` at 4 is `"0.0800"`, `"-1"` at 2
/// is `"-1.00"`. `None` when the value is finer than `places`, or is not a
/// decimal at all.
///
/// This is a `decimal(p)`'s one wire spelling, which is also what Postgres
/// prints for a `numeric` with scale `p`.
pub fn at_places(digits: &str, places: u8) -> Option<String> {
    minor_units(digits, places)
        .ok()
        .map(|m| major_spelling(i128::from(m), places))
}

/// How many places a decimal spelling carries once trailing zeros after
/// the point are dropped: `"29.90"` carries 1, `"30"` and `"30.00"` carry 0.
pub fn significant_places(digits: &str) -> Option<u32> {
    let (mut v, mut scale) = decimal_parts(digits)?;
    while scale > 0 && v % 10 == 0 {
        v /= 10;
        scale -= 1;
    }
    Some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_units_become_minor_units_exactly() {
        assert_eq!(minor_units("29.99", 2), Ok(2999));
        assert_eq!(minor_units("30", 2), Ok(3000));
        assert_eq!(minor_units("-12.5", 2), Ok(-1250));
        assert_eq!(minor_units("29.90", 1), Ok(299));
        assert_eq!(minor_units("2999", 0), Ok(2999));
        assert_eq!(minor_units("29.00", 0), Ok(29));
        assert_eq!(minor_units("0.001", 3), Ok(1));
        assert_eq!(
            minor_units("29.999", 2),
            Err(Fit::TooFine { written: 3 }),
            "a value finer than the places would have to round"
        );
        assert_eq!(minor_units("29.99", 0), Err(Fit::TooFine { written: 2 }));
        assert_eq!(minor_units("99999999999999999999", 2), Err(Fit::Overflow));
        assert_eq!(minor_units("x", 2), Err(Fit::NotNumeric));
        assert_eq!(minor_units("1.", 2), Err(Fit::NotNumeric));
    }

    #[test]
    fn minor_units_spell_back_in_major_units() {
        assert_eq!(major_spelling(2999, 2), "29.99");
        assert_eq!(major_spelling(-5, 2), "-0.05");
        assert_eq!(major_spelling(0, 2), "0.00");
        assert_eq!(major_spelling(2999, 0), "2999");
        assert_eq!(major_spelling(-2999, 0), "-2999");
        assert_eq!(at_places("0.08", 4).as_deref(), Some("0.0800"));
        assert_eq!(at_places("-1", 2).as_deref(), Some("-1.00"));
        assert_eq!(at_places("0.00001", 4), None);
        assert_eq!(significant_places("29.90"), Some(1));
        assert_eq!(significant_places("30.00"), Some(0));
    }
}
