//! Field values as the command log records them. A closed inventory, like
//! everything else — no serde, no JSON: the canonical byte encoding in
//! [`crate::chain`] is the one serialization, and it is deterministic.

use std::collections::BTreeMap;

/// A recorded field value.
///
/// The redaction rule (design §7) lives in the type: a sealed field's image
/// is **always** [`Value::SealedRef`] — an **opaque random handle** into the
/// sealed payload — never plaintext, and never a digest of the ciphertext.
///
/// The handle is minted fresh per write and carries no information. It was
/// once specified as SHA-256 of the ciphertext, which is wrong in a way worth
/// recording: with deterministic sealing or a deduplicating payload store,
/// equal plaintexts produce equal digests, so the *plaintext* command log
/// becomes an unsalted permanent equality index over every sealed field —
/// one that answers "did these two rows ever hold the same value?" forever,
/// and **survives crypto-erasure**, because destroying the key does not
/// destroy the digests. A random handle cannot answer that question at all.
/// (`security-logging-design.md` §7, correcting the earlier ruling.)
///
/// `money` rides [`Value::Int`] (minor-units integer semantics, spec §2);
/// dates/datetimes ride [`Value::Int`] as epoch values resolved at write
/// time (resolved values, never "default").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    /// A typed id (`txn_…`, `acct_…`) — part of the plaintext skeleton.
    Id(String),
    /// An opaque 128-bit handle to the sealed payload holding this field's
    /// value. Random per write; never derived from the value or its
    /// ciphertext, so it leaks nothing — not even equality.
    SealedRef([u8; 16]),
    /// A `decimal(p)` field's value (27ak), in its one canonical spelling.
    /// See [`Decimal`].
    Decimal(Decimal),
}

/// An exact decimal in the **one spelling** the chain hashes.
///
/// A decimal has many spellings of one value: `0.08`, `0.0800`, `+0.08`,
/// `00.08`. A hash over the spelling would let one value hash two ways, so
/// the chain fixes one, and this type is the only way to make a
/// [`Value::Decimal`]:
///
/// - an optional `-`, never `+`, and never on zero (`-0` is `0`);
/// - the integer digits with no leading zero, and `0` when there are none;
/// - a point and the fraction digits only when the fraction is not zero,
///   with **trailing zeros stripped**.
///
/// So `0.0800` at a `decimal(4)` column and `0.08` at a `decimal(2)` one
/// are the same value and hash the same, and a column's scale can widen
/// without moving a hash. The column's scale is a schema fact, recorded in
/// the artifact; the chain records the value. Equality of values is
/// equality of bytes.
///
/// The decoder refuses a stored spelling that is not canonical, so the map
/// from value to bytes stays a bijection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decimal(String);

impl Decimal {
    /// The canonical spelling of `digits`, or `None` when it is not a
    /// decimal: an optional sign, digits, and at most one point with
    /// digits after it.
    pub fn parse(digits: &str) -> Option<Decimal> {
        let (neg, body) = match digits.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, digits.strip_prefix('+').unwrap_or(digits)),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty() || !all_digits(int) || !all_digits(frac) {
            return None;
        }
        if body.contains('.') && frac.is_empty() {
            return None;
        }
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        let int = if int.is_empty() { "0" } else { int };
        let zero = int == "0" && frac.is_empty();
        let mut s = String::new();
        if neg && !zero {
            s.push('-');
        }
        s.push_str(int);
        if !frac.is_empty() {
            s.push('.');
            s.push_str(frac);
        }
        Some(Decimal(s))
    }

    /// The canonical spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Value {
    /// Encoding tag byte — part of the canonical layout, frozen.
    pub(crate) fn tag(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) => 2,
            Value::Text(_) => 3,
            Value::Id(_) => 4,
            Value::SealedRef(_) => 5,
            // Added 2026-10-04 (27ak) by presence: only a decimal renders
            // this byte, so every value written before it renders as it
            // always did (`tests.rs`, `PRE_DECIMAL_TAG_GOLDEN`).
            Value::Decimal(_) => 6,
        }
    }
}

/// A before/after image: field name → recorded value. `BTreeMap` makes the
/// canonical field order (byte order of NFC-normalized UTF-8 keys) a
/// property of the type, not a convention.
pub type Image = BTreeMap<String, Value>;
