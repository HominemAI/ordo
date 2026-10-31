//! Unicode canonicalization, checklist 9u.
//!
//! `definition-language.md` §2 settles this and does not propose it:
//! *"Text canonicalization [settled] — stored text is Unicode-normalized
//! (NFC) platform-wide"*, on write. Nothing did it. `é` (U+00E9) and
//! `e` + U+0301 are **one text** by that rule, and they sorted apart,
//! compared unequal, and could both occupy a `unique` column.
//!
//! # Why this is the other half of G11
//!
//! G11 ruled that text orders by Unicode codepoint over the stored NFC
//! form, because codepoint order is the only order that is a function of
//! the value alone. That ruling makes the order deterministic **given the
//! bytes**. This is what makes the bytes deterministic **given the text** —
//! without it, `COLLATE "C"` faithfully reproduces an arbitrary choice of
//! encoding, and keyset pagination seeks against a byte sequence the caller
//! could not have predicted from the string they hold.
//!
//! # Where it is applied, and why not in one place
//!
//! Text enters the platform at three doors and there is no single funnel
//! below them:
//!
//! 1. **Literals**, here — the lexer normalizes every string token as it is
//!    built, so a literal is NFC from the moment it exists. That covers
//!    writes, conditions, guards and defaults in one line, and it is why a
//!    guard comparing against a composed literal matches a stored value.
//! 2. **Caller values at the door** (`ordo-api`), so that a *read* filtering
//!    on a decomposed argument matches the composed row. Normalizing on
//!    write alone would store one form and compare another, which fixes
//!    nothing.
//! 3. **Stored values at the write path** (`ordo-conformance`'s write
//!    protocol), which is the boundary the spec's word "on write" names. It
//!    is deliberately not redundant with (2): the conformance harness does
//!    not go through `ordo-api` at all, so a guarantee that lived only at
//!    the door would be untested by the suite that proves the platform's
//!    behaviour, and any future caller path would silently escape it.
//!
//! `unicode-normalization` is a dependency in three crates rather than one
//! because `ordo-api`, `ordo-syntax` and `ordo-crypto` are all leaf crates
//! with no edges between them; inverting that graph for one function, or
//! minting a crate to hold it, would both cost more than the repetition.
//!
//! # What it deliberately does not do
//!
//! NFC, not NFKC. NFKC folds compatibility characters — `ﬁ` to `fi`, `①` to
//! `1`, full-width forms to ASCII — which is a *different* text, not another
//! spelling of the same one. The spec says NFC and NFC is the conservative
//! half: it composes what is canonically equivalent and changes nothing
//! else. Normalization is total (every string has an NFC form) and cannot
//! fail, so this mints no error class.

use unicode_normalization::UnicodeNormalization;

/// The stored form of a text value.
///
/// Cheap on the overwhelmingly common path: `is_nfc_quick` answers `Yes`
/// from a stream check for ASCII and for already-composed text, so a value
/// that is already canonical is returned without reallocating.
pub fn nfc(s: &str) -> String {
    match unicode_normalization::is_nfc_quick(s.chars()) {
        unicode_normalization::IsNormalized::Yes => s.to_string(),
        _ => s.nfc().collect(),
    }
}

/// Is this value already in its stored form?
///
/// Used by assertions and by the migration probe: a deployment predating
/// 9u holds rows this answers `false` for, and normalizing them can merge
/// two rows that a `unique` column currently keeps apart.
pub fn is_nfc(s: &str) -> bool {
    !matches!(
        unicode_normalization::is_nfc_quick(s.chars()),
        unicode_normalization::IsNormalized::No
    ) && s.nfc().eq(s.chars())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact pair the checklist names.
    #[test]
    fn composed_and_decomposed_become_one_text() {
        let composed = "\u{00e9}"; // é
        let decomposed = "e\u{0301}"; // e + combining acute
        assert_ne!(composed, decomposed, "they start as different bytes");
        assert_eq!(nfc(composed), nfc(decomposed));
        assert_eq!(nfc(decomposed), composed, "NFC composes");
        assert!(is_nfc(composed) && !is_nfc(decomposed));
    }

    /// NFC is idempotent, which is what lets the write path assert the
    /// invariant instead of re-normalizing defensively at every layer.
    #[test]
    fn normalizing_twice_changes_nothing() {
        for s in ["e\u{0301}", "abc", "", "\u{fb01}", "\u{ff21}", "가"] {
            assert_eq!(nfc(&nfc(s)), nfc(s));
            assert!(is_nfc(&nfc(s)));
        }
    }

    /// NFC, not NFKC — a compatibility character is a different text, and
    /// folding it would silently rewrite what the author wrote.
    #[test]
    fn compatibility_characters_are_left_alone() {
        assert_eq!(nfc("\u{fb01}"), "\u{fb01}", "ﬁ does not become `fi`");
        assert_eq!(nfc("\u{ff21}"), "\u{ff21}", "full-width A stays");
        assert_eq!(nfc("\u{2460}"), "\u{2460}", "① does not become `1`");
    }

    /// Ordering is the reason this exists: two spellings of one text must
    /// not straddle a third value under the byte order G11 pinned.
    #[test]
    fn one_text_cannot_straddle_another_under_byte_order() {
        let mut raw = vec!["e\u{0301}", "f", "\u{00e9}"];
        raw.sort();
        assert_eq!(raw, vec!["e\u{0301}", "f", "\u{00e9}"], "`f` splits them");

        let mut normalized: Vec<String> = raw.iter().map(|s| nfc(s)).collect();
        normalized.sort();
        assert_eq!(normalized, vec!["f", "\u{00e9}", "\u{00e9}"]);
    }
}
