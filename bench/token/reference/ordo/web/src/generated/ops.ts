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
  spec_addTask,
  spec_deleteTask,
  spec_listMembers,
  spec_listTasks,
  spec_me,
  spec_std$tasks$count,
  spec_std$tasks$create,
  spec_std$tasks$delete,
  spec_std$tasks$get,
  spec_std$tasks$list,
  spec_std$tasks$update,
  spec_updateTask,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type * as Ids from './ids.js';

/**
 * The row shape of `addTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddTaskParams = {
  /** `text` · fills `tasks.title` */
  readonly title: string;
};

/**
 * The refusals `addTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addTask` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addTask(client: OrdoClient, params: AddTaskParams): WriteCall<AddTaskRow, AddTaskRefusal> {
  return new WriteCall(client, spec_addTask, params);
}

/**
 * The row shape of `deleteTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DeleteTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `deleteTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DeleteTaskParams = {
  /** `id` */
  readonly id: Ids.TasksId;
};

/**
 * The refusals `deleteTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DeleteTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `deleteTask` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function deleteTask(client: OrdoClient, params: DeleteTaskParams): WriteCall<DeleteTaskRow, DeleteTaskRefusal> {
  return new WriteCall(client, spec_deleteTask, params);
}

/**
 * One row of `listMembers`.
 *
 * Row identity: `members`. `synthetic_rows` is false.
 */
export interface ListMembersRow {
  /** `id` */
  readonly id: Ids.MembersId;
  /** `text` */
  readonly displayName: string;
  /** `text` */
  readonly role: string;
}

/** `listMembers` takes no parameters. */
export type ListMembersParams = Record<never, never>;

/**
 * The fields a `.where` on `listMembers` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ListMembersFields {
  readonly id: OpenField<'id', Ids.MembersId>;
  readonly displayName: OpenField<'displayName', string>;
  readonly role: OpenField<'role', string>;
}

/**
 * The refusals `listMembers` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ListMembersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `listMembers` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function listMembers(client: OrdoClient): ReadCall<ListMembersRow, ListMembersRefusal, ListMembersFields> {
  return new ReadCall(client, spec_listMembers, {});
}

/**
 * One row of `listTasks`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface ListTasksRow {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
  /** `id` */
  readonly assignee: Ids.MembersId;
}

/** `listTasks` takes no parameters. */
export type ListTasksParams = Record<never, never>;

/**
 * The fields a `.where` on `listTasks` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ListTasksFields {
  readonly id: OpenField<'id', Ids.TasksId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
}

/**
 * The refusals `listTasks` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ListTasksRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `listTasks` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function listTasks(client: OrdoClient): ReadCall<ListTasksRow, ListTasksRefusal, ListTasksFields> {
  return new ReadCall(client, spec_listTasks, {});
}

/**
 * One row of `me`.
 *
 * Row identity: `members`. `synthetic_rows` is false.
 */
export interface MeRow {
  /** `id` */
  readonly id: Ids.MembersId;
  /** `text` */
  readonly displayName: string;
  /** `text` */
  readonly role: string;
}

/** `me` takes no parameters. */
export type MeParams = Record<never, never>;

/**
 * The fields a `.where` on `me` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MeFields {
  readonly id: OpenField<'id', Ids.MembersId>;
  readonly displayName: OpenField<'displayName', string>;
  readonly role: OpenField<'role', string>;
}

/**
 * The refusals `me` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MeRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `me` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function me(client: OrdoClient): ReadCall<MeRow, MeRefusal, MeFields> {
  return new ReadCall(client, spec_me, {});
}

/**
 * One row of `std.tasks.count`.
 */
export interface StdTasksCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.tasks.count` takes no parameters. */
export type StdTasksCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.tasks.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdTasksCountFields = Record<never, never>;

/**
 * The refusals `std.tasks.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$tasks$count(client: OrdoClient): SingleReadCall<StdTasksCountRow, StdTasksCountRefusal, StdTasksCountFields> {
  return new SingleReadCall(client, spec_std$tasks$count, {});
}

/**
 * The row shape of `std.tasks.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTasksCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.tasks.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTasksCreateParams = {
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `bool` · fills `tasks.done` · has a default; omit to take it */
  readonly done?: boolean;
  /** `id` · fills `tasks.assignee` · has a default; omit to take it */
  readonly assignee?: Ids.MembersId;
};

/**
 * The refusals `std.tasks.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$tasks$create(client: OrdoClient, params: StdTasksCreateParams): WriteCall<StdTasksCreateRow, StdTasksCreateRefusal> {
  return new WriteCall(client, spec_std$tasks$create, params);
}

/**
 * The row shape of `std.tasks.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTasksDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.tasks.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTasksDeleteParams = {
  /** `id` */
  readonly id: Ids.TasksId;
};

/**
 * The refusals `std.tasks.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$tasks$delete(client: OrdoClient, params: StdTasksDeleteParams): WriteCall<StdTasksDeleteRow, StdTasksDeleteRefusal> {
  return new WriteCall(client, spec_std$tasks$delete, params);
}

/**
 * One row of `std.tasks.get`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface StdTasksGetRow {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
  /** `id` */
  readonly assignee: Ids.MembersId;
}

/**
 * The parameters of `std.tasks.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTasksGetParams = {
  /** `id` */
  readonly id: Ids.TasksId;
};

/**
 * The fields a `.where` on `std.tasks.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTasksGetFields {
  readonly id: OpenField<'id', Ids.TasksId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
}

/**
 * The refusals `std.tasks.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$tasks$get(client: OrdoClient, params: StdTasksGetParams): ReadCall<StdTasksGetRow, StdTasksGetRefusal, StdTasksGetFields> {
  return new ReadCall(client, spec_std$tasks$get, params);
}

/**
 * One row of `std.tasks.list`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface StdTasksListRow {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
  /** `id` */
  readonly assignee: Ids.MembersId;
}

/** `std.tasks.list` takes no parameters. */
export type StdTasksListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.tasks.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTasksListFields {
  readonly id: OpenField<'id', Ids.TasksId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
}

/**
 * The refusals `std.tasks.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$tasks$list(client: OrdoClient): ReadCall<StdTasksListRow, StdTasksListRefusal, StdTasksListFields> {
  return new ReadCall(client, spec_std$tasks$list, {});
}

/**
 * The row shape of `std.tasks.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTasksUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.tasks.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTasksUpdateParams = {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `bool` · fills `tasks.done` */
  readonly done: boolean;
  /** `id` · fills `tasks.assignee` */
  readonly assignee: Ids.MembersId;
};

/**
 * The refusals `std.tasks.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.tasks.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$tasks$update(client: OrdoClient, params: StdTasksUpdateParams): WriteCall<StdTasksUpdateRow, StdTasksUpdateRefusal> {
  return new WriteCall(client, spec_std$tasks$update, params);
}

/**
 * The row shape of `updateTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type UpdateTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `updateTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type UpdateTaskParams = {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `bool` · fills `tasks.done` */
  readonly done: boolean;
  /** `id` · fills `tasks.assignee` · has a default; omit to take it */
  readonly assignee?: Ids.MembersId;
};

/**
 * The refusals `updateTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type UpdateTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `updateTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function updateTask(client: OrdoClient, params: UpdateTaskParams): WriteCall<UpdateTaskRow, UpdateTaskRefusal> {
  return new WriteCall(client, spec_updateTask, params);
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  addTask: addTask,
  deleteTask: deleteTask,
  listMembers: listMembers,
  listTasks: listTasks,
  me: me,
  'std.tasks.count': std$tasks$count,
  'std.tasks.create': std$tasks$create,
  'std.tasks.delete': std$tasks$delete,
  'std.tasks.get': std$tasks$get,
  'std.tasks.list': std$tasks$list,
  'std.tasks.update': std$tasks$update,
  updateTask: updateTask,
} as const;
