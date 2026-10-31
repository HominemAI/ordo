/**
 * `@hominemai/ordo-cdk` — the typed TypeScript builder.
 *
 * A **frontend, never a second semantics**. `synth()` emits canonical `.ordo`
 * text; that text is the artifact that is reviewed, diffed, committed and
 * deployed, and everything downstream is unchanged. The platform never knows
 * which frontend authored a definition.
 *
 * Two halves:
 *
 * - `src/core/` — the hand-designed **skeleton**: the IR, the chaining shape,
 *   the argument order, the guard algebra, the printer.
 * - `src/generated/` — the **vocabulary**, emitted by `ordo gen ts` from the
 *   parser's own closed tables (and, for `schema.ts`, from one deployment's
 *   `interface.json`).
 *
 * The README documents that boundary and every exclusion.
 */

export { CALLER, OPEN, g } from './core/guard.js';
export type { GuardRhs } from './core/guard.js';
export { MEMBER, rel, release } from './core/release.js';
export { Each, Routine, daily, each, every, routine, when } from './core/routine.js';
export type { RoutineOptions, RoutineTrigger, WhenOptions } from './core/routine.js';
export {
  Body,
  DelStmt,
  Entity,
  Op,
  Q,
  SetStmt,
  Src,
  TypeSpec,
  as_,
  asc,
  desc,
  domain,
  entity,
  minus,
  named,
  op,
  ref,
  src,
  star,
  synth,
} from './core/build.js';
export type {
  EntityOptions,
  ParamsOf,
  PickItem,
  SortArg,
  Sourceable,
  Synthesizable,
  WriteLike,
} from './core/build.js';
export {
  Cond,
  ValueBase,
  installValue,
  makeFields,
  makeParams,
  makePath,
  param,
  pathNode,
  value,
} from './core/value.js';
export type { AnyFields, Value } from './core/value.js';
export { printCond, printDeclaration, printExpr, printPipeline, strLit } from './core/print.js';
export type * from './core/ir.js';

export * from './generated/index.js';
