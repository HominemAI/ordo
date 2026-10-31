//! `ordo-gen` — the one generator, and its two consumers.
//!
//! # The ruling this crate implements
//!
//! Both TypeScript packages in `clients/` that this crate writes are
//! **generated from the implementation's own canonical tables, never
//! hand-maintained**. Language evolution is therefore regeneration, not
//! rewriting: `ordo gen ts` is the whole of it.
//!
//! - `clients/ordo-sdk` (checklist 14d) — the typed client, a pure function
//!   of one deployment's `interface.json`.
//! - `clients/ordo-cdk` (checklist 14f) — the typed builder, whose *language*
//!   layer is a function of the parser's closed tables and whose *schema*
//!   layer is a function of the same `interface.json`.
//!
//! One generator, because the alternative was two: the CDK's honesty rule is
//! that its API "is generated from the same grammar definition the parser
//! uses", and the SDK's is that it is a pure function of the descriptor. Two
//! crates asserting those separately would be two walks over the same tables,
//! which is the defect `ordo_plan::interface` was built to avoid one layer
//! down.
//!
//! # Two ratchets
//!
//! 1. **Exhaustive match** ([`spell`]) — every closed language enum is
//!    matched with no wildcard arm, so a new keyword, scalar type or operator
//!    fails `cargo build` here until someone decides its TypeScript spelling.
//!    `tests/ratchet.rs` scans for the wildcard forms so the rule cannot be
//!    quietly relaxed, and the emitted `vocabulary.ts` carries every decision
//!    into the TypeScript test suite, which checks the offered names resolve.
//! 2. **Isomorphism** ([`emit`], `tests/isomorphism.rs`) — real corpus
//!    programs are parsed, machine-translated into builder calls, synthesized
//!    back to `.ordo`, and reparsed; the two ASTs must be equal.
//!
//! # What this crate may not depend on
//!
//! A schema, a registry, a binder or a database. It reads `ordo-syntax`'s
//! tables and JSON, and it writes text. That is the same rule
//! `ordo_plan::interface` states about itself, for the same reason: a
//! generator that could reach the compiler would be a second walk over it,
//! and two walks are two answers waiting to differ.

pub mod cdk;
pub mod emit;
pub mod sdk;
pub mod spell;
pub mod ts;

pub use ts::GenFile;

/// Where each package's generated half lives, relative to the repository
/// root. Named here because three readers need them: the CLI that writes the
/// files, the golden test that compares them, and the READMEs that document
/// the boundary.
pub const SDK_OUT: &str = "clients/ordo-sdk/src/generated";
pub const CDK_OUT: &str = "clients/ordo-cdk/src/generated";

/// Every file `ordo gen ts` writes, for one descriptor.
///
/// The language half of the CDK does not depend on the descriptor at all —
/// it is the same bytes for every deployment — and is emitted anyway, because
/// a package whose halves came from two commands is a package that ships with
/// one of them stale.
pub fn generate(doc: &serde_json::Value) -> Result<Vec<GenFile>, String> {
    let mut out = sdk::files(doc, SDK_OUT)?;
    out.extend(cdk::language_files(CDK_OUT));
    out.extend(cdk::schema_files(doc, CDK_OUT)?);
    Ok(out)
}
