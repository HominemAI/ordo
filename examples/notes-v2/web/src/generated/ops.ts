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
  spec_addAttachment,
  spec_allNotes,
  spec_archiveNote,
  spec_clearReminder,
  spec_createNote,
  spec_deleteNote,
  spec_editNote,
  spec_grantNotesAccess,
  spec_myFiles,
  spec_myNotes,
  spec_myReminders,
  spec_notesTagged,
  spec_pinNote,
  spec_removeAttachment,
  spec_revokeNotesAccess,
  spec_setNotesAccessLevel,
  spec_setReminder,
  spec_std$attachments$count,
  spec_std$attachments$create,
  spec_std$attachments$delete,
  spec_std$attachments$get,
  spec_std$attachments$list,
  spec_std$attachments$update,
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
  spec_std$reminders$count,
  spec_std$reminders$create,
  spec_std$reminders$delete,
  spec_std$reminders$get,
  spec_std$reminders$list,
  spec_std$reminders$put,
  spec_std$reminders$update,
  spec_unarchiveNote,
  spec_unshareNote,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';
import type * as Ids from './ids.js';

/**
 * The row shape of `addAttachment`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddAttachmentRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addAttachment`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddAttachmentParams = {
  /** `id` · fills `attachments.note` */
  readonly note: Ids.NotesId;
  /** `text` · fills `attachments.name` */
  readonly name: string;
  /** `text` · fills `attachments.kind` */
  readonly kind: string;
  /** `int` · fills `attachments.size` */
  readonly size: number;
  /** `datetime` · fills `attachments.created` */
  readonly created: string;
};

/**
 * The refusals `addAttachment` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddAttachmentRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addAttachment` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addAttachment(client: OrdoClient, params: AddAttachmentParams): WriteCall<AddAttachmentRow, AddAttachmentRefusal> {
  return new WriteCall(client, spec_addAttachment, params);
}

/**
 * One row of `allNotes`.
 *
 * Row identity: `notes`. `synthetic_rows` is false.
 */
export interface AllNotesRow {
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
  /** `bool` */
  readonly pinned: boolean;
  /** `datetime` */
  readonly archived: string;
}

/** `allNotes` takes no parameters. */
export type AllNotesParams = Record<never, never>;

/**
 * The fields a `.where` on `allNotes` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface AllNotesFields {
  readonly id: OpenField<'id', Ids.NotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly body: OpenField<'body', string>;
  readonly created: OpenField<'created', string>;
  readonly pinned: OpenField<'pinned', boolean>;
  readonly archived: OpenField<'archived', string>;
}

/**
 * The refusals `allNotes` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AllNotesRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `allNotes` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function allNotes(client: OrdoClient): ReadCall<AllNotesRow, AllNotesRefusal, AllNotesFields> {
  return new ReadCall(client, spec_allNotes, {});
}

/**
 * The row shape of `archiveNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ArchiveNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `archiveNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ArchiveNoteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
  /** `datetime` · fills `notes.archived` */
  readonly at: string;
};

/**
 * The refusals `archiveNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ArchiveNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `archiveNote` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function archiveNote(client: OrdoClient, params: ArchiveNoteParams): WriteCall<ArchiveNoteRow, ArchiveNoteRefusal> {
  return new WriteCall(client, spec_archiveNote, params);
}

/**
 * The row shape of `clearReminder`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ClearReminderRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `clearReminder`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ClearReminderParams = {
  /** `id` · fills `reminders.note` */
  readonly note: Ids.NotesId;
};

/**
 * The refusals `clearReminder` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ClearReminderRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `clearReminder` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function clearReminder(client: OrdoClient, params: ClearReminderParams): WriteCall<ClearReminderRow, ClearReminderRefusal> {
  return new WriteCall(client, spec_clearReminder, params);
}

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
 * One row of `myFiles`.
 *
 * Row identity: `attachments`. `synthetic_rows` is false.
 */
export interface MyFilesRow {
  /** `id` */
  readonly id: Ids.AttachmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly kind: string;
  /** `int` */
  readonly size: number;
  /** `datetime` */
  readonly created: string;
  /** `blob` */
  readonly file: string;
}

/** `myFiles` takes no parameters. */
export type MyFilesParams = Record<never, never>;

/**
 * The fields a `.where` on `myFiles` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyFilesFields {
  readonly id: OpenField<'id', Ids.AttachmentsId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly name: OpenField<'name', string>;
  readonly kind: OpenField<'kind', string>;
  readonly size: OpenField<'size', number>;
  readonly created: OpenField<'created', string>;
  readonly file: OpenField<'file', string>;
}

/**
 * The refusals `myFiles` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyFilesRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myFiles` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myFiles(client: OrdoClient): ReadCall<MyFilesRow, MyFilesRefusal, MyFilesFields> {
  return new ReadCall(client, spec_myFiles, {});
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
 * One row of `myReminders`.
 *
 * Row identity: `reminders`. `synthetic_rows` is false.
 */
export interface MyRemindersRow {
  /** `id` */
  readonly id: Ids.RemindersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `datetime` */
  readonly due: string;
  /** `datetime` */
  readonly fired: string;
}

/** `myReminders` takes no parameters. */
export type MyRemindersParams = Record<never, never>;

/**
 * The fields a `.where` on `myReminders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyRemindersFields {
  readonly id: OpenField<'id', Ids.RemindersId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly due: OpenField<'due', string>;
  readonly fired: OpenField<'fired', string>;
}

/**
 * The refusals `myReminders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyRemindersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myReminders` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myReminders(client: OrdoClient): ReadCall<MyRemindersRow, MyRemindersRefusal, MyRemindersFields> {
  return new ReadCall(client, spec_myReminders, {});
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
  /** `bool` */
  readonly pinned: boolean;
  /** `datetime` */
  readonly archived: string;
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
  readonly pinned: OpenField<'pinned', boolean>;
  readonly archived: OpenField<'archived', string>;
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
 * The row shape of `pinNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type PinNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `pinNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type PinNoteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
  /** `bool` · fills `notes.pinned` */
  readonly pinned: boolean;
};

/**
 * The refusals `pinNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PinNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `pinNote` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function pinNote(client: OrdoClient, params: PinNoteParams): WriteCall<PinNoteRow, PinNoteRefusal> {
  return new WriteCall(client, spec_pinNote, params);
}

/**
 * The row shape of `removeAttachment`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RemoveAttachmentRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `removeAttachment`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RemoveAttachmentParams = {
  /** `id` */
  readonly id: Ids.AttachmentsId;
};

/**
 * The refusals `removeAttachment` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RemoveAttachmentRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `removeAttachment` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function removeAttachment(client: OrdoClient, params: RemoveAttachmentParams): WriteCall<RemoveAttachmentRow, RemoveAttachmentRefusal> {
  return new WriteCall(client, spec_removeAttachment, params);
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
 * The row shape of `setReminder`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type SetReminderRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `setReminder`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SetReminderParams = {
  /** `id` · fills `reminders.note` */
  readonly note: Ids.NotesId;
  /** `datetime` · fills `reminders.due` */
  readonly due: string;
};

/**
 * The refusals `setReminder` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SetReminderRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `setReminder` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function setReminder(client: OrdoClient, params: SetReminderParams): WriteCall<SetReminderRow, SetReminderRefusal> {
  return new WriteCall(client, spec_setReminder, params);
}

/**
 * One row of `std.attachments.count`.
 */
export interface StdAttachmentsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.attachments.count` takes no parameters. */
export type StdAttachmentsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.attachments.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdAttachmentsCountFields = Record<never, never>;

/**
 * The refusals `std.attachments.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$attachments$count(client: OrdoClient): SingleReadCall<StdAttachmentsCountRow, StdAttachmentsCountRefusal, StdAttachmentsCountFields> {
  return new SingleReadCall(client, spec_std$attachments$count, {});
}

/**
 * The row shape of `std.attachments.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdAttachmentsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.attachments.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdAttachmentsCreateParams = {
  /** `id` · fills `attachments.note` */
  readonly note: Ids.NotesId;
  /** `text` · fills `attachments.name` */
  readonly name: string;
  /** `text` · fills `attachments.kind` · has a default; omit to take it */
  readonly kind?: string;
  /** `int` · fills `attachments.size` · has a default; omit to take it */
  readonly size?: number;
  /** `datetime` · fills `attachments.created` */
  readonly created: string;
};

/**
 * The refusals `std.attachments.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$attachments$create(client: OrdoClient, params: StdAttachmentsCreateParams): WriteCall<StdAttachmentsCreateRow, StdAttachmentsCreateRefusal> {
  return new WriteCall(client, spec_std$attachments$create, params);
}

/**
 * The row shape of `std.attachments.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdAttachmentsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.attachments.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdAttachmentsDeleteParams = {
  /** `id` */
  readonly id: Ids.AttachmentsId;
};

/**
 * The refusals `std.attachments.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$attachments$delete(client: OrdoClient, params: StdAttachmentsDeleteParams): WriteCall<StdAttachmentsDeleteRow, StdAttachmentsDeleteRefusal> {
  return new WriteCall(client, spec_std$attachments$delete, params);
}

/**
 * One row of `std.attachments.get`.
 *
 * Row identity: `attachments`. `synthetic_rows` is false.
 */
export interface StdAttachmentsGetRow {
  /** `id` */
  readonly id: Ids.AttachmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly kind: string;
  /** `int` */
  readonly size: number;
  /** `datetime` */
  readonly created: string;
  /** `blob` */
  readonly file: string;
}

/**
 * The parameters of `std.attachments.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdAttachmentsGetParams = {
  /** `id` */
  readonly id: Ids.AttachmentsId;
};

/**
 * The fields a `.where` on `std.attachments.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdAttachmentsGetFields {
  readonly id: OpenField<'id', Ids.AttachmentsId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly name: OpenField<'name', string>;
  readonly kind: OpenField<'kind', string>;
  readonly size: OpenField<'size', number>;
  readonly created: OpenField<'created', string>;
  readonly file: OpenField<'file', string>;
}

/**
 * The refusals `std.attachments.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$attachments$get(client: OrdoClient, params: StdAttachmentsGetParams): ReadCall<StdAttachmentsGetRow, StdAttachmentsGetRefusal, StdAttachmentsGetFields> {
  return new ReadCall(client, spec_std$attachments$get, params);
}

/**
 * One row of `std.attachments.list`.
 *
 * Row identity: `attachments`. `synthetic_rows` is false.
 */
export interface StdAttachmentsListRow {
  /** `id` */
  readonly id: Ids.AttachmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly kind: string;
  /** `int` */
  readonly size: number;
  /** `datetime` */
  readonly created: string;
  /** `blob` */
  readonly file: string;
}

/** `std.attachments.list` takes no parameters. */
export type StdAttachmentsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.attachments.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdAttachmentsListFields {
  readonly id: OpenField<'id', Ids.AttachmentsId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly name: OpenField<'name', string>;
  readonly kind: OpenField<'kind', string>;
  readonly size: OpenField<'size', number>;
  readonly created: OpenField<'created', string>;
  readonly file: OpenField<'file', string>;
}

/**
 * The refusals `std.attachments.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$attachments$list(client: OrdoClient): ReadCall<StdAttachmentsListRow, StdAttachmentsListRefusal, StdAttachmentsListFields> {
  return new ReadCall(client, spec_std$attachments$list, {});
}

/**
 * The row shape of `std.attachments.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdAttachmentsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.attachments.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdAttachmentsUpdateParams = {
  /** `id` */
  readonly id: Ids.AttachmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `attachments.note` */
  readonly note: Ids.NotesId;
  /** `text` · fills `attachments.name` */
  readonly name: string;
  /** `text` · fills `attachments.kind` */
  readonly kind: string;
  /** `int` · fills `attachments.size` */
  readonly size: number;
  /** `datetime` · fills `attachments.created` */
  readonly created: string;
};

/**
 * The refusals `std.attachments.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdAttachmentsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.attachments.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$attachments$update(client: OrdoClient, params: StdAttachmentsUpdateParams): WriteCall<StdAttachmentsUpdateRow, StdAttachmentsUpdateRefusal> {
  return new WriteCall(client, spec_std$attachments$update, params);
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
  /** `bool` · fills `notes.pinned` · has a default; omit to take it */
  readonly pinned?: boolean;
  /** `datetime` · fills `notes.archived` · has a default; omit it or pass null to take it */
  readonly archived?: string | null;
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
  /** `bool` */
  readonly pinned: boolean;
  /** `datetime` */
  readonly archived: string;
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
  readonly pinned: OpenField<'pinned', boolean>;
  readonly archived: OpenField<'archived', string>;
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
  /** `bool` */
  readonly pinned: boolean;
  /** `datetime` */
  readonly archived: string;
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
  readonly pinned: OpenField<'pinned', boolean>;
  readonly archived: OpenField<'archived', string>;
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
  /** `bool` · fills `notes.pinned` */
  readonly pinned: boolean;
  /** `datetime` · fills `notes.archived` */
  readonly archived: string;
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
 * One row of `std.reminders.count`.
 */
export interface StdRemindersCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.reminders.count` takes no parameters. */
export type StdRemindersCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.reminders.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdRemindersCountFields = Record<never, never>;

/**
 * The refusals `std.reminders.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$reminders$count(client: OrdoClient): SingleReadCall<StdRemindersCountRow, StdRemindersCountRefusal, StdRemindersCountFields> {
  return new SingleReadCall(client, spec_std$reminders$count, {});
}

/**
 * The row shape of `std.reminders.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdRemindersCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.reminders.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdRemindersCreateParams = {
  /** `id` · fills `reminders.note` */
  readonly note: Ids.NotesId;
  /** `datetime` · fills `reminders.due` */
  readonly due: string;
  /** `datetime` · fills `reminders.fired` · has a default; omit it or pass null to take it */
  readonly fired?: string | null;
};

/**
 * The refusals `std.reminders.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$reminders$create(client: OrdoClient, params: StdRemindersCreateParams): WriteCall<StdRemindersCreateRow, StdRemindersCreateRefusal> {
  return new WriteCall(client, spec_std$reminders$create, params);
}

/**
 * The row shape of `std.reminders.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdRemindersDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.reminders.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdRemindersDeleteParams = {
  /** `id` */
  readonly id: Ids.RemindersId;
};

/**
 * The refusals `std.reminders.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$reminders$delete(client: OrdoClient, params: StdRemindersDeleteParams): WriteCall<StdRemindersDeleteRow, StdRemindersDeleteRefusal> {
  return new WriteCall(client, spec_std$reminders$delete, params);
}

/**
 * One row of `std.reminders.get`.
 *
 * Row identity: `reminders`. `synthetic_rows` is false.
 */
export interface StdRemindersGetRow {
  /** `id` */
  readonly id: Ids.RemindersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `datetime` */
  readonly due: string;
  /** `datetime` */
  readonly fired: string;
}

/**
 * The parameters of `std.reminders.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdRemindersGetParams = {
  /** `id` */
  readonly id: Ids.RemindersId;
};

/**
 * The fields a `.where` on `std.reminders.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdRemindersGetFields {
  readonly id: OpenField<'id', Ids.RemindersId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly due: OpenField<'due', string>;
  readonly fired: OpenField<'fired', string>;
}

/**
 * The refusals `std.reminders.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$reminders$get(client: OrdoClient, params: StdRemindersGetParams): ReadCall<StdRemindersGetRow, StdRemindersGetRefusal, StdRemindersGetFields> {
  return new ReadCall(client, spec_std$reminders$get, params);
}

/**
 * One row of `std.reminders.list`.
 *
 * Row identity: `reminders`. `synthetic_rows` is false.
 */
export interface StdRemindersListRow {
  /** `id` */
  readonly id: Ids.RemindersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly note: Ids.NotesId;
  /** `datetime` */
  readonly due: string;
  /** `datetime` */
  readonly fired: string;
}

/** `std.reminders.list` takes no parameters. */
export type StdRemindersListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.reminders.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdRemindersListFields {
  readonly id: OpenField<'id', Ids.RemindersId>;
  readonly rev: OpenField<'rev', number>;
  readonly note: OpenField<'note', Ids.NotesId>;
  readonly due: OpenField<'due', string>;
  readonly fired: OpenField<'fired', string>;
}

/**
 * The refusals `std.reminders.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$reminders$list(client: OrdoClient): ReadCall<StdRemindersListRow, StdRemindersListRefusal, StdRemindersListFields> {
  return new ReadCall(client, spec_std$reminders$list, {});
}

/**
 * The row shape of `std.reminders.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdRemindersPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.reminders.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdRemindersPutParams = {
  /** `id` · fills `reminders.note` */
  readonly note: Ids.NotesId;
  /** `datetime` · fills `reminders.due` */
  readonly due: string;
  /** `datetime` · fills `reminders.fired` · always written: pass the current value to keep it, or null to write the default (null clears it when the default is null) */
  readonly fired: string | null;
};

/**
 * The refusals `std.reminders.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$reminders$put(client: OrdoClient, params: StdRemindersPutParams): WriteCall<StdRemindersPutRow, StdRemindersPutRefusal> {
  return new WriteCall(client, spec_std$reminders$put, params);
}

/**
 * The row shape of `std.reminders.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdRemindersUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.reminders.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdRemindersUpdateParams = {
  /** `id` */
  readonly id: Ids.RemindersId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `reminders.note` */
  readonly note: Ids.NotesId;
  /** `datetime` · fills `reminders.due` */
  readonly due: string;
  /** `datetime` · fills `reminders.fired` */
  readonly fired: string;
};

/**
 * The refusals `std.reminders.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdRemindersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.reminders.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$reminders$update(client: OrdoClient, params: StdRemindersUpdateParams): WriteCall<StdRemindersUpdateRow, StdRemindersUpdateRefusal> {
  return new WriteCall(client, spec_std$reminders$update, params);
}

/**
 * The row shape of `unarchiveNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type UnarchiveNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `unarchiveNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type UnarchiveNoteParams = {
  /** `id` */
  readonly id: Ids.NotesId;
  /** `int` */
  readonly rev: number;
};

/**
 * The refusals `unarchiveNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type UnarchiveNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `unarchiveNote` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function unarchiveNote(client: OrdoClient, params: UnarchiveNoteParams): WriteCall<UnarchiveNoteRow, UnarchiveNoteRefusal> {
  return new WriteCall(client, spec_unarchiveNote, params);
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
  addAttachment: addAttachment,
  allNotes: allNotes,
  archiveNote: archiveNote,
  clearReminder: clearReminder,
  createNote: createNote,
  deleteNote: deleteNote,
  editNote: editNote,
  grantNotesAccess: grantNotesAccess,
  myFiles: myFiles,
  myNotes: myNotes,
  myReminders: myReminders,
  notesTagged: notesTagged,
  pinNote: pinNote,
  removeAttachment: removeAttachment,
  revokeNotesAccess: revokeNotesAccess,
  setNotesAccessLevel: setNotesAccessLevel,
  setReminder: setReminder,
  'std.attachments.count': std$attachments$count,
  'std.attachments.create': std$attachments$create,
  'std.attachments.delete': std$attachments$delete,
  'std.attachments.get': std$attachments$get,
  'std.attachments.list': std$attachments$list,
  'std.attachments.update': std$attachments$update,
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
  'std.reminders.count': std$reminders$count,
  'std.reminders.create': std$reminders$create,
  'std.reminders.delete': std$reminders$delete,
  'std.reminders.get': std$reminders$get,
  'std.reminders.list': std$reminders$list,
  'std.reminders.put': std$reminders$put,
  'std.reminders.update': std$reminders$update,
  unarchiveNote: unarchiveNote,
  unshareNote: unshareNote,
} as const;
