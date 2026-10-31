/**
 * Refusals: the closed inventory, and what a surface is supposed to do with
 * one.
 *
 * The inventory, the throwable and the decoder are `@hominemai/ordo-sdk`'s
 * (`src/core/refusal.ts`), re-exported as they are: one error shape across
 * both packages (owner ruling, 2026-10-23; audit-service-interfaces.md, fix
 * order step 7). An `OrdoRefusal` carries its detail as `.refusal` (this
 * package's name) and as `.detail` (the SDK's), the same object, with the
 * wire `code`, the `status`, the door's `retryAfterMs`, its `disposition`
 * and whether it is `retryable`.
 *
 * A refusal is never a string in this package. `ui-gen.md` §1: *typed
 * rejections render typed, never stringly.*
 */

import type { RefusalClass } from '@hominemai/ordo-sdk/core/refusal.js';

export {
  EDGE_CODES,
  REFUSAL_CLASSES,
  OrdoLocalRefusal,
  OrdoRefusal,
  decodeRefusal,
  dispositionOf,
  isDoorCode,
  isEdgeCode,
  isErrorEnvelope,
  isRefusalClass,
  type Disposition,
  type DoorCode,
  type EdgeCode,
  type ErrorEnvelope,
  type RefusalClass,
  /** The refusal a hook exposes: the SDK's `RefusalDetail`, under this package's name. */
  type RefusalDetail as Refusal,
} from '@hominemai/ordo-sdk/core/refusal.js';

/**
 * Type-level projection of one op's declared refusal set, for a consumer who
 * imports their `interface.json` with `resolveJsonModule` and `as const`:
 *
 * ```ts
 * import doc from './interface.json';
 * type R = RefusalOf<typeof doc, 'std.tasks.update'>;
 * //   ^? 'access_denied' | 'cas_miss' | … — the op's set, not the whole table
 * ```
 *
 * Without `as const` the JSON widens to `string[]` and this degrades to
 * `RefusalClass`, which is still sound — it is the closed set — just less
 * sharp. `test/types.test-d.ts` pins both behaviours.
 */
export type RefusalOf<
  Doc extends { readonly ops: readonly { readonly op: string; readonly refusals: readonly string[] }[] },
  Name extends Doc['ops'][number]['op'],
> = Extract<Doc['ops'][number], { readonly op: Name }>['refusals'][number] extends infer R
  ? R extends RefusalClass
    ? R
    : RefusalClass
  : never;
