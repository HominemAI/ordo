// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// One typed function per op: params in, terminal-verb builder out.
//
// Source of truth: the deployment's `interface.json` (`ordo.interface/2`), itself a projection of `op_artifact`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import type { RefusalClass } from '@hominemai/ordo-sdk/core/refusal.js';
import {
  spec_myNotes,
  spec_publicNotes,
  spec_std$entries$count,
  spec_std$entries$get,
  spec_std$entries$list,
  spec_std$published$count,
  spec_std$published$get,
  spec_std$published$list,
} from './specs.js';
import { ReadCall, SingleReadCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';
import type * as Ids from './ids.js';

/**
 * One row of `myNotes`.
 *
 * Row identity: `entries`. `synthetic_rows` is false.
 */
export interface MyNotesRow {
  /** `id` */
  readonly id: Ids.EntriesId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `json` */
  readonly tags: JsonValue;
  /** `datetime` */
  readonly created: string;
}

/** `myNotes` takes no parameters. */
export type MyNotesParams = Record<never, never>;

/**
 * The fields a `.where` on `myNotes` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyNotesFields {
  readonly id: OpenField<'id', Ids.EntriesId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `myNotes` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyNotesRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myNotes` — read.
 * Cost class `sql`, rows: <= 100.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myNotes(client: OrdoClient): ReadCall<MyNotesRow, MyNotesRefusal, MyNotesFields> {
  return new ReadCall(client, spec_myNotes, {});
}

/**
 * One row of `publicNotes`.
 *
 * Row identity: `published`. `synthetic_rows` is false.
 */
export interface PublicNotesRow {
  /** `id` */
  readonly id: Ids.PublishedId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `datetime` */
  readonly created: string;
}

/** `publicNotes` takes no parameters. */
export type PublicNotesParams = Record<never, never>;

/**
 * The fields a `.where` on `publicNotes` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface PublicNotesFields {
  readonly id: OpenField<'id', Ids.PublishedId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `publicNotes` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PublicNotesRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `publicNotes` — read.
 * Cost class `sql`, rows: <= 100.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function publicNotes(client: OrdoClient): ReadCall<PublicNotesRow, PublicNotesRefusal, PublicNotesFields> {
  return new ReadCall(client, spec_publicNotes, {});
}

/**
 * One row of `std.entries.count`.
 */
export interface StdEntriesCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.entries.count` takes no parameters. */
export type StdEntriesCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.entries.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdEntriesCountFields = Record<never, never>;

/**
 * The refusals `std.entries.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdEntriesCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.entries.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$entries$count(client: OrdoClient): SingleReadCall<StdEntriesCountRow, StdEntriesCountRefusal, StdEntriesCountFields> {
  return new SingleReadCall(client, spec_std$entries$count, {});
}

/**
 * One row of `std.entries.get`.
 *
 * Row identity: `entries`. `synthetic_rows` is false.
 */
export interface StdEntriesGetRow {
  /** `id` */
  readonly id: Ids.EntriesId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `json` */
  readonly tags: JsonValue;
  /** `datetime` */
  readonly created: string;
}

/**
 * The parameters of `std.entries.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdEntriesGetParams = {
  /** `id` */
  readonly id: Ids.EntriesId;
};

/**
 * The fields a `.where` on `std.entries.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdEntriesGetFields {
  readonly id: OpenField<'id', Ids.EntriesId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.entries.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdEntriesGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.entries.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$entries$get(client: OrdoClient, params: StdEntriesGetParams): ReadCall<StdEntriesGetRow, StdEntriesGetRefusal, StdEntriesGetFields> {
  return new ReadCall(client, spec_std$entries$get, params);
}

/**
 * One row of `std.entries.list`.
 *
 * Row identity: `entries`. `synthetic_rows` is false.
 */
export interface StdEntriesListRow {
  /** `id` */
  readonly id: Ids.EntriesId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `json` */
  readonly tags: JsonValue;
  /** `datetime` */
  readonly created: string;
}

/** `std.entries.list` takes no parameters. */
export type StdEntriesListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.entries.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdEntriesListFields {
  readonly id: OpenField<'id', Ids.EntriesId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.entries.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdEntriesListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.entries.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$entries$list(client: OrdoClient): ReadCall<StdEntriesListRow, StdEntriesListRefusal, StdEntriesListFields> {
  return new ReadCall(client, spec_std$entries$list, {});
}

/**
 * One row of `std.published.count`.
 */
export interface StdPublishedCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.published.count` takes no parameters. */
export type StdPublishedCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.published.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdPublishedCountFields = Record<never, never>;

/**
 * The refusals `std.published.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdPublishedCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.published.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$published$count(client: OrdoClient): SingleReadCall<StdPublishedCountRow, StdPublishedCountRefusal, StdPublishedCountFields> {
  return new SingleReadCall(client, spec_std$published$count, {});
}

/**
 * One row of `std.published.get`.
 *
 * Row identity: `published`. `synthetic_rows` is false.
 */
export interface StdPublishedGetRow {
  /** `id` */
  readonly id: Ids.PublishedId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `datetime` */
  readonly created: string;
}

/**
 * The parameters of `std.published.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdPublishedGetParams = {
  /** `id` */
  readonly id: Ids.PublishedId;
};

/**
 * The fields a `.where` on `std.published.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdPublishedGetFields {
  readonly id: OpenField<'id', Ids.PublishedId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.published.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdPublishedGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.published.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$published$get(client: OrdoClient, params: StdPublishedGetParams): ReadCall<StdPublishedGetRow, StdPublishedGetRefusal, StdPublishedGetFields> {
  return new ReadCall(client, spec_std$published$get, params);
}

/**
 * One row of `std.published.list`.
 *
 * Row identity: `published`. `synthetic_rows` is false.
 */
export interface StdPublishedListRow {
  /** `id` */
  readonly id: Ids.PublishedId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `datetime` */
  readonly created: string;
}

/** `std.published.list` takes no parameters. */
export type StdPublishedListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.published.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdPublishedListFields {
  readonly id: OpenField<'id', Ids.PublishedId>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.published.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdPublishedListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.published.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$published$list(client: OrdoClient): ReadCall<StdPublishedListRow, StdPublishedListRefusal, StdPublishedListFields> {
  return new ReadCall(client, spec_std$published$list, {});
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  myNotes: myNotes,
  publicNotes: publicNotes,
  'std.entries.count': std$entries$count,
  'std.entries.get': std$entries$get,
  'std.entries.list': std$entries$list,
  'std.published.count': std$published$count,
  'std.published.get': std$published$get,
  'std.published.list': std$published$list,
} as const;
