// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// Re-exports of the generated surface.
//
// Source of truth: the parser's own closed tables in `crates/ordo-syntax` (`Keyword`, `ScalarTy`, `CmpOp`, `TextOp`, `BinOp`, `DurationUnit`, `functions()`, `AGGREGATE_SHAPES`, `ALLOCATE_MODES`)
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

export * from './vocabulary.js';
export * from './schema.js';
// `f` is exported by both halves and they are the same object: the
// language layer builds it, the schema layer re-types it against one
// deployment. The typed one wins here, and an author with no schema
// imports `f` from `./lang.js` directly.
export { f } from './schema.js';
export * from './lang.js';
