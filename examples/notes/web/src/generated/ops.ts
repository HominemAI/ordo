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
  spec_createNote,
  spec_deleteNote,
  spec_editNote,
  spec_grantNotesAccess,
  spec_myNotes,
  spec_notesTagged,
  spec_revokeNotesAccess,
  spec_setNotesAccessLevel,
  spec_std$notes$count,
  spec_std$notes$create,
  spec_std$notes$delete,
  spec_std$notes$get,
  spec_std$notes$list,
  spec_std$notes$update,
  spec_std$notesAccess$count,
  spec_std$notesAccess$create,
  spec_std$notesAccess$delete,
  spec_std$notesAccess$get,
  spec_std$notesAccess$list,
  spec_std$notesAccess$put,
  spec_std$notesAccess$update,
  spec_unshareNote,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';
import type * as Ids from './ids.js';

/**
 * The row shape of `createNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type CreateNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `createNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type CreateNoteParams = {
  /** `text` · fills `notes.title` */
  readonly title: string;
  /** `text` · fills `notes.body` */
  readonly body: string;
  /** `json` · fills `notes.tags` */
  readonly tags: JsonValue;
  /** `datetime` · fills `notes.created` */
  readonly created: string;
};

/**
 * The refusals `createNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CreateNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `createNote` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function createNote(client: OrdoClient, params: CreateNoteParams): WriteCall<CreateNoteRow, CreateNoteRefusal> {
  return new WriteCall(client, spec_createNote, params);
}

/**
 * The row shape of `deleteNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DeleteNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `deleteNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DeleteNoteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
};

/**
 * The refusals `deleteNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DeleteNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `deleteNote` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function deleteNote(client: OrdoClient, params: DeleteNoteParams): WriteCall<DeleteNoteRow, DeleteNoteRefusal> {
  return new WriteCall(client, spec_deleteNote, params);
}

/**
 * The row shape of `editNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type EditNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `editNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type EditNoteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `notes.title` */
  readonly title: string;
  /** `text` · fills `notes.body` */
  readonly body: string;
  /** `json` · fills `notes.tags` */
  readonly tags: JsonValue;
};

/**
 * The refusals `editNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type EditNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `editNote` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function editNote(client: OrdoClient, params: EditNoteParams): WriteCall<EditNoteRow, EditNoteRefusal> {
  return new WriteCall(client, spec_editNote, params);
}

/**
 * The row shape of `grantNotesAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type GrantNotesAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `grantNotesAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type GrantNotesAccessParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `notesAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `grantNotesAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type GrantNotesAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `grantNotesAccess` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function grantNotesAccess(client: OrdoClient, params: GrantNotesAccessParams): WriteCall<GrantNotesAccessRow, GrantNotesAccessRefusal> {
  return new WriteCall(client, spec_grantNotesAccess, params);
}

/**
 * One row of `myNotes`.
 *
 * Row identity: `notes`. `synthetic_rows` is false.
 */
export interface MyNotesRow {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
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
  readonly id: OpenField<'id', Ids.NotesId>;
  readonly rev: OpenField<'rev', number>;
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
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myNotes(client: OrdoClient): ReadCall<MyNotesRow, MyNotesRefusal, MyNotesFields> {
  return new ReadCall(client, spec_myNotes, {});
}

/**
 * One row of `notesTagged`.
 *
 * Row identity: `notes`. `synthetic_rows` is false.
 */
export interface NotesTaggedRow {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
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
 * The parameters of `notesTagged`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type NotesTaggedParams = {
  /** `text` */
  readonly tag: string;
};

/**
 * The fields a `.where` on `notesTagged` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface NotesTaggedFields {
  readonly id: OpenField<'id', Ids.NotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `notesTagged` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type NotesTaggedRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `notesTagged` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function notesTagged(client: OrdoClient, params: NotesTaggedParams): ReadCall<NotesTaggedRow, NotesTaggedRefusal, NotesTaggedFields> {
  return new ReadCall(client, spec_notesTagged, params);
}

/**
 * The row shape of `revokeNotesAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RevokeNotesAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `revokeNotesAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RevokeNotesAccessParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `revokeNotesAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RevokeNotesAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `revokeNotesAccess` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function revokeNotesAccess(client: OrdoClient, params: RevokeNotesAccessParams): WriteCall<RevokeNotesAccessRow, RevokeNotesAccessRefusal> {
  return new WriteCall(client, spec_revokeNotesAccess, params);
}

/**
 * The row shape of `setNotesAccessLevel`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type SetNotesAccessLevelRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `setNotesAccessLevel`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SetNotesAccessLevelParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `notesAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `setNotesAccessLevel` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SetNotesAccessLevelRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `setNotesAccessLevel` — write.
 *
 * Mode: **bulk**.
 * Terminates in `.commit()`.
 */
export function setNotesAccessLevel(client: OrdoClient, params: SetNotesAccessLevelParams): WriteCall<SetNotesAccessLevelRow, SetNotesAccessLevelRefusal> {
  return new WriteCall(client, spec_setNotesAccessLevel, params);
}

/**
 * One row of `std.notes.count`.
 */
export interface StdNotesCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.notes.count` takes no parameters. */
export type StdNotesCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.notes.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdNotesCountFields = Record<never, never>;

/**
 * The refusals `std.notes.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notes$count(client: OrdoClient): SingleReadCall<StdNotesCountRow, StdNotesCountRefusal, StdNotesCountFields> {
  return new SingleReadCall(client, spec_std$notes$count, {});
}

/**
 * The row shape of `std.notes.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notes.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesCreateParams = {
  /** `text` · fills `notes.title` */
  readonly title: string;
  /** `text` · fills `notes.body` · has a default; omit to take it */
  readonly body?: string;
  /** `json` · fills `notes.tags` · has a default; omit to take it */
  readonly tags?: JsonValue;
  /** `datetime` · fills `notes.created` */
  readonly created: string;
};

/**
 * The refusals `std.notes.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$notes$create(client: OrdoClient, params: StdNotesCreateParams): WriteCall<StdNotesCreateRow, StdNotesCreateRefusal> {
  return new WriteCall(client, spec_std$notes$create, params);
}

/**
 * The row shape of `std.notes.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notes.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesDeleteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
};

/**
 * The refusals `std.notes.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$notes$delete(client: OrdoClient, params: StdNotesDeleteParams): WriteCall<StdNotesDeleteRow, StdNotesDeleteRefusal> {
  return new WriteCall(client, spec_std$notes$delete, params);
}

/**
 * One row of `std.notes.get`.
 *
 * Row identity: `notes`. `synthetic_rows` is false.
 */
export interface StdNotesGetRow {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
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
 * The parameters of `std.notes.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesGetParams = {
  /** `id` */
  readonly id: Ids.NotesId;
};

/**
 * The fields a `.where` on `std.notes.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdNotesGetFields {
  readonly id: OpenField<'id', Ids.NotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.notes.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notes$get(client: OrdoClient, params: StdNotesGetParams): ReadCall<StdNotesGetRow, StdNotesGetRefusal, StdNotesGetFields> {
  return new ReadCall(client, spec_std$notes$get, params);
}

/**
 * One row of `std.notes.list`.
 *
 * Row identity: `notes`. `synthetic_rows` is false.
 */
export interface StdNotesListRow {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly body: string;
  /** `json` */
  readonly tags: JsonValue;
  /** `datetime` */
  readonly created: string;
}

/** `std.notes.list` takes no parameters. */
export type StdNotesListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.notes.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdNotesListFields {
  readonly id: OpenField<'id', Ids.NotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
}

/**
 * The refusals `std.notes.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notes$list(client: OrdoClient): ReadCall<StdNotesListRow, StdNotesListRefusal, StdNotesListFields> {
  return new ReadCall(client, spec_std$notes$list, {});
}

/**
 * The row shape of `std.notes.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notes.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesUpdateParams = {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `notes.title` */
  readonly title: string;
  /** `text` · fills `notes.body` */
  readonly body: string;
  /** `json` · fills `notes.tags` */
  readonly tags: JsonValue;
  /** `datetime` · fills `notes.created` */
  readonly created: string;
};

/**
 * The refusals `std.notes.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notes.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$notes$update(client: OrdoClient, params: StdNotesUpdateParams): WriteCall<StdNotesUpdateRow, StdNotesUpdateRefusal> {
  return new WriteCall(client, spec_std$notes$update, params);
}

/**
 * One row of `std.notesAccess.count`.
 */
export interface StdNotesAccessCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.notesAccess.count` takes no parameters. */
export type StdNotesAccessCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.notesAccess.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdNotesAccessCountFields = Record<never, never>;

/**
 * The refusals `std.notesAccess.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notesAccess$count(client: OrdoClient): SingleReadCall<StdNotesAccessCountRow, StdNotesAccessCountRefusal, StdNotesAccessCountFields> {
  return new SingleReadCall(client, spec_std$notesAccess$count, {});
}

/**
 * The row shape of `std.notesAccess.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesAccessCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notesAccess.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesAccessCreateParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `notesAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.notesAccess.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$notesAccess$create(client: OrdoClient, params: StdNotesAccessCreateParams): WriteCall<StdNotesAccessCreateRow, StdNotesAccessCreateRefusal> {
  return new WriteCall(client, spec_std$notesAccess$create, params);
}

/**
 * The row shape of `std.notesAccess.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesAccessDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notesAccess.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesAccessDeleteParams = {
  /** `id` */
  readonly id: Ids.NotesAccessId;
};

/**
 * The refusals `std.notesAccess.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$notesAccess$delete(client: OrdoClient, params: StdNotesAccessDeleteParams): WriteCall<StdNotesAccessDeleteRow, StdNotesAccessDeleteRefusal> {
  return new WriteCall(client, spec_std$notesAccess$delete, params);
}

/**
 * One row of `std.notesAccess.get`.
 *
 * Row identity: `notesAccess`. `synthetic_rows` is false.
 */
export interface StdNotesAccessGetRow {
  /** `id` */
  readonly id: Ids.NotesAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/**
 * The parameters of `std.notesAccess.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesAccessGetParams = {
  /** `id` */
  readonly id: Ids.NotesAccessId;
};

/**
 * The fields a `.where` on `std.notesAccess.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdNotesAccessGetFields {
  readonly id: OpenField<'id', Ids.NotesAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.notesAccess.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notesAccess$get(client: OrdoClient, params: StdNotesAccessGetParams): ReadCall<StdNotesAccessGetRow, StdNotesAccessGetRefusal, StdNotesAccessGetFields> {
  return new ReadCall(client, spec_std$notesAccess$get, params);
}

/**
 * One row of `std.notesAccess.list`.
 *
 * Row identity: `notesAccess`. `synthetic_rows` is false.
 */
export interface StdNotesAccessListRow {
  /** `id` */
  readonly id: Ids.NotesAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/** `std.notesAccess.list` takes no parameters. */
export type StdNotesAccessListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.notesAccess.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdNotesAccessListFields {
  readonly id: OpenField<'id', Ids.NotesAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.notesAccess.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$notesAccess$list(client: OrdoClient): ReadCall<StdNotesAccessListRow, StdNotesAccessListRefusal, StdNotesAccessListFields> {
  return new ReadCall(client, spec_std$notesAccess$list, {});
}

/**
 * The row shape of `std.notesAccess.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesAccessPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notesAccess.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesAccessPutParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `notesAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.notesAccess.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$notesAccess$put(client: OrdoClient, params: StdNotesAccessPutParams): WriteCall<StdNotesAccessPutRow, StdNotesAccessPutRefusal> {
  return new WriteCall(client, spec_std$notesAccess$put, params);
}

/**
 * The row shape of `std.notesAccess.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdNotesAccessUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.notesAccess.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdNotesAccessUpdateParams = {
  /** `id` */
  readonly id: Ids.NotesAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `notesAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.notesAccess.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdNotesAccessUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.notesAccess.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$notesAccess$update(client: OrdoClient, params: StdNotesAccessUpdateParams): WriteCall<StdNotesAccessUpdateRow, StdNotesAccessUpdateRefusal> {
  return new WriteCall(client, spec_std$notesAccess$update, params);
}

/**
 * The row shape of `unshareNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type UnshareNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `unshareNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type UnshareNoteParams = {
  /** `id` · fills `notesAccess.note` */
  readonly note: Ids.NotesId;
  /** `id` · fills `notesAccess.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `unshareNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type UnshareNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `unshareNote` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function unshareNote(client: OrdoClient, params: UnshareNoteParams): WriteCall<UnshareNoteRow, UnshareNoteRefusal> {
  return new WriteCall(client, spec_unshareNote, params);
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  createNote: createNote,
  deleteNote: deleteNote,
  editNote: editNote,
  grantNotesAccess: grantNotesAccess,
  myNotes: myNotes,
  notesTagged: notesTagged,
  revokeNotesAccess: revokeNotesAccess,
  setNotesAccessLevel: setNotesAccessLevel,
  'std.notes.count': std$notes$count,
  'std.notes.create': std$notes$create,
  'std.notes.delete': std$notes$delete,
  'std.notes.get': std$notes$get,
  'std.notes.list': std$notes$list,
  'std.notes.update': std$notes$update,
  'std.notesAccess.count': std$notesAccess$count,
  'std.notesAccess.create': std$notesAccess$create,
  'std.notesAccess.delete': std$notesAccess$delete,
  'std.notesAccess.get': std$notesAccess$get,
  'std.notesAccess.list': std$notesAccess$list,
  'std.notesAccess.put': std$notesAccess$put,
  'std.notesAccess.update': std$notesAccess$update,
  unshareNote: unshareNote,
} as const;
