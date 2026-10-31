//! `BlobPolicy` — the three numbers the blob tier is bounded by.
//!
//! Shaped like `ordo_exec::cadence::LogPolicy`: a plain struct with an
//! associated [`BlobPolicy::DEFAULT`], authored in the definition language
//! (`definition-language.md` §8, Policies — *"operator decisions, never
//! user consent"*), carried by value. No global, no env read at the point
//! of use, no `Option` that means "ask someone else".

use std::time::Duration;

/// The bounds. Every one of them is a **number**, tunable by policy,
/// rather than a principle — which is the honest shape for all three:
/// each is the point where one cost overtakes another, and where that
/// point falls depends on the deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobPolicy {
    /// The largest `blob inline` write, in bytes.
    ///
    /// Default 256 KiB, from `definition-language.md` §2 ("default 256
    /// KB, policy-tunable"). Above it, four costs overtake the second
    /// system: the bytes land in the thing every backup, every replica and
    /// every `pg_dump` copies; TOAST puts a multi-megabyte value on the
    /// read path of a query that projected other fields, and the
    /// cost-class system has no way to say so; the export bundle's
    /// per-entity JSONL carries megabytes of hex per row and stops being
    /// loadable by ordinary tooling; and a `bytea` column has one read and
    /// it is the whole value, so there are no range reads, ever.
    pub inline_cap: u64,

    /// The largest single-request upload, in bytes, for any tier.
    ///
    /// Default 100 MiB. The door refuses above it with the existing
    /// `bound_violation` class rather than a new one: the standing rule is
    /// that a new refusal class is an oracle until argued, and "too large"
    /// is a bound, which the door already has a word for.
    ///
    /// Multipart and resumable upload are deferred with a stated trigger —
    /// when a real upload exceeds the request timeout — and are an
    /// addition inside `BlobStore`, not a redesign of it.
    pub max_object: u64,

    /// The lifetime a share link gets when nothing has set one.
    ///
    /// Share links are bearer, so every link carries an expiry
    /// (`definition-language.md` §2, Share links [settled]). **This is a
    /// default, not a ceiling** (owner, 2026-09-29: seven days "as a default
    /// setting - when the feature is used but no settings have been defined
    /// for it"). The platform sets no maximum; the lifetime is layered, the
    /// deployment's default, then a tenant's choice when the app developer
    /// exposes it, then the link's own (`plan-sharing.md`). Unused until the
    /// share-link op exists.
    pub share_ttl_default: Duration,
}

impl BlobPolicy {
    pub const DEFAULT: BlobPolicy = BlobPolicy {
        inline_cap: 256 * 1024,
        max_object: 100 * 1024 * 1024,
        share_ttl_default: Duration::from_secs(7 * 24 * 60 * 60),
    };

    /// Is this an inline write the cap admits?
    ///
    /// Checked **before the sealer**, always: an oversize write is refused
    /// before a byte is sealed, so a rejected upload costs no key release
    /// and leaves no payload behind.
    pub fn admits_inline(&self, bytes: u64) -> bool {
        bytes <= self.inline_cap
    }

    pub fn admits_object(&self, bytes: u64) -> bool {
        bytes <= self.max_object
    }
}

impl Default for BlobPolicy {
    fn default() -> Self {
        BlobPolicy::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_cap_is_the_number_the_language_document_states() {
        assert_eq!(BlobPolicy::DEFAULT.inline_cap, 262_144);
        assert!(BlobPolicy::DEFAULT.admits_inline(262_144));
        assert!(!BlobPolicy::DEFAULT.admits_inline(262_145));
    }

    /// The inline cap must be below the object cap, or `inline` would be
    /// the tier with the *larger* limit and the shape summary's pricing
    /// would read backwards.
    #[test]
    fn inline_is_the_smaller_bound() {
        const { assert!(BlobPolicy::DEFAULT.inline_cap < BlobPolicy::DEFAULT.max_object) };
    }
}
