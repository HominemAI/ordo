// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// Branded id types, one per described entity.
//
// Source of truth: the deployment's `interface.json` (`ordo.interface/2`), itself a projection of `op_artifact`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import type { Branded } from '@hominemai/ordo-sdk/core/brand.js';

/** Every entity id type in this deployment, keyed by entity name. */
/**
 * An id addressing `members`.
 *
 * Typed-id prefix `mem`, so a value looks like `mem_…`. The brand is compile-time only; the wire sees the string.
 */
export type MembersId = Branded<string, 'members'>;

/**
 * An id addressing `tasks`.
 *
 * Typed-id prefix `task`, so a value looks like `task_…`. The brand is compile-time only; the wire sees the string.
 */
export type TasksId = Branded<string, 'tasks'>;

