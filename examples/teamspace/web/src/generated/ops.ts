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
  spec_board,
  spec_createTask,
  spec_editTask,
  spec_me,
  spec_moveTask,
  spec_myProjects,
  spec_people,
  spec_projectTeam,
  spec_reportByPerson,
  spec_reportByProject,
  spec_std$projects$count,
  spec_std$projects$create,
  spec_std$projects$delete,
  spec_std$projects$get,
  spec_std$projects$list,
  spec_std$projects$update,
  spec_std$tasks$count,
  spec_std$tasks$create,
  spec_std$tasks$delete,
  spec_std$tasks$get,
  spec_std$tasks$list,
  spec_std$tasks$update,
  spec_std$team$count,
  spec_std$team$create,
  spec_std$team$delete,
  spec_std$team$get,
  spec_std$team$list,
  spec_std$team$put,
  spec_std$team$update,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type * as Ids from './ids.js';

/**
 * One row of `board`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface BoardRow {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
  /** `id` */
  readonly assignee: Ids.MembersId;
  /** `text` */
  readonly assigneeName: string;
  /** `text` */
  readonly authorName: string;
  /** `date` */
  readonly due: string;
  /** `datetime` */
  readonly added: string;
}

/**
 * The parameters of `board`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type BoardParams = {
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
};

/**
 * The fields a `.where` on `board` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface BoardFields {
  readonly id: OpenField<'id', Ids.TasksId>;
  readonly rev: OpenField<'rev', number>;
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
  readonly assigneeName: OpenField<'assigneeName', string>;
  readonly authorName: OpenField<'authorName', string>;
  readonly due: OpenField<'due', string>;
  readonly added: OpenField<'added', string>;
}

/**
 * The refusals `board` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type BoardRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `board` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function board(client: OrdoClient, params: BoardParams): ReadCall<BoardRow, BoardRefusal, BoardFields> {
  return new ReadCall(client, spec_board, params);
}

/**
 * The row shape of `createTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type CreateTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `createTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type CreateTaskParams = {
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `id` · fills `tasks.assignee` · has a default; omit it or pass null to take it */
  readonly assignee?: Ids.MembersId | null;
  /** `date` · fills `tasks.due` · has a default; omit it or pass null to take it */
  readonly due?: string | null;
  /** `datetime` · fills `tasks.added` · filled from the caller's clock when omitted */
  readonly added?: string;
};

/**
 * The refusals `createTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CreateTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `createTask` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function createTask(client: OrdoClient, params: CreateTaskParams): WriteCall<CreateTaskRow, CreateTaskRefusal> {
  return new WriteCall(client, spec_createTask, { ...params, added: params.added ?? filledDatetime() });
}

/**
 * The row shape of `editTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type EditTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `editTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type EditTaskParams = {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `id` · fills `tasks.assignee` · always written: pass the current value to keep it, or null to write the default (null clears it when the default is null) */
  readonly assignee: Ids.MembersId | null;
  /** `date` · fills `tasks.due` · always written: pass the current value to keep it, or null to write the default (null clears it when the default is null) */
  readonly due: string | null;
};

/**
 * The refusals `editTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type EditTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `editTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function editTask(client: OrdoClient, params: EditTaskParams): WriteCall<EditTaskRow, EditTaskRefusal> {
  return new WriteCall(client, spec_editTask, params);
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
 * The row shape of `moveTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type MoveTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `moveTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MoveTaskParams = {
  /** `id` */
  readonly id: Ids.TasksId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `tasks.status` */
  readonly status: string;
};

/**
 * The refusals `moveTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MoveTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `moveTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function moveTask(client: OrdoClient, params: MoveTaskParams): WriteCall<MoveTaskRow, MoveTaskRefusal> {
  return new WriteCall(client, spec_moveTask, params);
}

/**
 * One row of `myProjects`.
 *
 * Row identity: `team`. `synthetic_rows` is false.
 */
export interface MyProjectsRow {
  /** `id` */
  readonly id: Ids.ProjectsId;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly summary: string;
  /** `id` */
  readonly $id: Ids.TeamId;
}

/** `myProjects` takes no parameters. */
export type MyProjectsParams = Record<never, never>;

/**
 * The fields a `.where` on `myProjects` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyProjectsFields {
  readonly id: OpenField<'id', Ids.ProjectsId>;
  readonly name: OpenField<'name', string>;
  readonly summary: OpenField<'summary', string>;
  readonly $id: OpenField<'$id', Ids.TeamId>;
}

/**
 * The refusals `myProjects` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyProjectsRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myProjects` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myProjects(client: OrdoClient): ReadCall<MyProjectsRow, MyProjectsRefusal, MyProjectsFields> {
  return new ReadCall(client, spec_myProjects, {});
}

/**
 * One row of `people`.
 *
 * Row identity: `members`. `synthetic_rows` is false.
 */
export interface PeopleRow {
  /** `id` */
  readonly id: Ids.MembersId;
  /** `text` */
  readonly displayName: string;
  /** `text` */
  readonly role: string;
  /** `text` */
  readonly status: string;
  /** `date` */
  readonly joined: string;
}

/** `people` takes no parameters. */
export type PeopleParams = Record<never, never>;

/**
 * The fields a `.where` on `people` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface PeopleFields {
  readonly id: OpenField<'id', Ids.MembersId>;
  readonly displayName: OpenField<'displayName', string>;
  readonly role: OpenField<'role', string>;
  readonly status: OpenField<'status', string>;
  readonly joined: OpenField<'joined', string>;
}

/**
 * The refusals `people` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PeopleRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `people` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function people(client: OrdoClient): ReadCall<PeopleRow, PeopleRefusal, PeopleFields> {
  return new ReadCall(client, spec_people, {});
}

/**
 * One row of `projectTeam`.
 *
 * Row identity: `team`. `synthetic_rows` is false.
 */
export interface ProjectTeamRow {
  /** `id` */
  readonly id: Ids.TeamId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly memberName: string;
  /** `text` */
  readonly memberRole: string;
}

/**
 * The parameters of `projectTeam`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ProjectTeamParams = {
  /** `id` · fills `team.project` */
  readonly project: Ids.ProjectsId;
};

/**
 * The fields a `.where` on `projectTeam` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ProjectTeamFields {
  readonly id: OpenField<'id', Ids.TeamId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly memberName: OpenField<'memberName', string>;
  readonly memberRole: OpenField<'memberRole', string>;
}

/**
 * The refusals `projectTeam` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ProjectTeamRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `projectTeam` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function projectTeam(client: OrdoClient, params: ProjectTeamParams): ReadCall<ProjectTeamRow, ProjectTeamRefusal, ProjectTeamFields> {
  return new ReadCall(client, spec_projectTeam, params);
}

/**
 * One row of `reportByPerson`.
 *
 * Row identity: `members`. `synthetic_rows` is false.
 */
export interface ReportByPersonRow {
  /** `id` */
  readonly id: Ids.MembersId;
  /** `text` */
  readonly displayName: string;
  /** `text` */
  readonly role: string;
  /** `int` */
  readonly total: number;
  /** `int` */
  readonly open: number;
  /** `int` */
  readonly done: number;
  /** `int` */
  readonly overdue: number;
}

/**
 * The parameters of `reportByPerson`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ReportByPersonParams = {
  /** `date` · filled from the caller's clock when omitted */
  readonly today?: string;
};

/**
 * The fields a `.where` on `reportByPerson` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ReportByPersonFields {
  readonly id: OpenField<'id', Ids.MembersId>;
  readonly displayName: OpenField<'displayName', string>;
  readonly role: OpenField<'role', string>;
  readonly total: OpenField<'total', number>;
  readonly open: OpenField<'open', number>;
  readonly done: OpenField<'done', number>;
  readonly overdue: OpenField<'overdue', number>;
}

/**
 * The refusals `reportByPerson` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ReportByPersonRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `reportByPerson` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * **`unguarded`** — this op bypasses row guards.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function reportByPerson(client: OrdoClient, params: ReportByPersonParams): ReadCall<ReportByPersonRow, ReportByPersonRefusal, ReportByPersonFields> {
  return new ReadCall(client, spec_reportByPerson, { ...params, today: params.today ?? filledDate() });
}

/**
 * One row of `reportByProject`.
 *
 * Row identity: `projects`. `synthetic_rows` is false.
 */
export interface ReportByProjectRow {
  /** `id` */
  readonly id: Ids.ProjectsId;
  /** `text` */
  readonly name: string;
  /** `int` */
  readonly total: number;
  /** `int` */
  readonly todo: number;
  /** `int` */
  readonly doing: number;
  /** `int` */
  readonly review: number;
  /** `int` */
  readonly done: number;
  /** `int` */
  readonly overdue: number;
}

/**
 * The parameters of `reportByProject`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ReportByProjectParams = {
  /** `date` · filled from the caller's clock when omitted */
  readonly today?: string;
};

/**
 * The fields a `.where` on `reportByProject` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ReportByProjectFields {
  readonly id: OpenField<'id', Ids.ProjectsId>;
  readonly name: OpenField<'name', string>;
  readonly total: OpenField<'total', number>;
  readonly todo: OpenField<'todo', number>;
  readonly doing: OpenField<'doing', number>;
  readonly review: OpenField<'review', number>;
  readonly done: OpenField<'done', number>;
  readonly overdue: OpenField<'overdue', number>;
}

/**
 * The refusals `reportByProject` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ReportByProjectRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `reportByProject` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * **`unguarded`** — this op bypasses row guards.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function reportByProject(client: OrdoClient, params: ReportByProjectParams): ReadCall<ReportByProjectRow, ReportByProjectRefusal, ReportByProjectFields> {
  return new ReadCall(client, spec_reportByProject, { ...params, today: params.today ?? filledDate() });
}

/**
 * One row of `std.projects.count`.
 */
export interface StdProjectsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.projects.count` takes no parameters. */
export type StdProjectsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.projects.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdProjectsCountFields = Record<never, never>;

/**
 * The refusals `std.projects.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projects$count(client: OrdoClient): SingleReadCall<StdProjectsCountRow, StdProjectsCountRefusal, StdProjectsCountFields> {
  return new SingleReadCall(client, spec_std$projects$count, {});
}

/**
 * The row shape of `std.projects.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projects.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectsCreateParams = {
  /** `text` · fills `projects.name` */
  readonly name: string;
  /** `text` · fills `projects.summary` · has a default; omit to take it */
  readonly summary?: string;
  /** `datetime` · fills `projects.added` */
  readonly added: string;
};

/**
 * The refusals `std.projects.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$projects$create(client: OrdoClient, params: StdProjectsCreateParams): WriteCall<StdProjectsCreateRow, StdProjectsCreateRefusal> {
  return new WriteCall(client, spec_std$projects$create, params);
}

/**
 * The row shape of `std.projects.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projects.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectsDeleteParams = {
  /** `id` */
  readonly id: Ids.ProjectsId;
};

/**
 * The refusals `std.projects.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$projects$delete(client: OrdoClient, params: StdProjectsDeleteParams): WriteCall<StdProjectsDeleteRow, StdProjectsDeleteRefusal> {
  return new WriteCall(client, spec_std$projects$delete, params);
}

/**
 * One row of `std.projects.get`.
 *
 * Row identity: `projects`. `synthetic_rows` is false.
 */
export interface StdProjectsGetRow {
  /** `id` */
  readonly id: Ids.ProjectsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly summary: string;
  /** `datetime` */
  readonly added: string;
}

/**
 * The parameters of `std.projects.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectsGetParams = {
  /** `id` */
  readonly id: Ids.ProjectsId;
};

/**
 * The fields a `.where` on `std.projects.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProjectsGetFields {
  readonly id: OpenField<'id', Ids.ProjectsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly summary: OpenField<'summary', string>;
  readonly added: OpenField<'added', string>;
}

/**
 * The refusals `std.projects.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projects$get(client: OrdoClient, params: StdProjectsGetParams): ReadCall<StdProjectsGetRow, StdProjectsGetRefusal, StdProjectsGetFields> {
  return new ReadCall(client, spec_std$projects$get, params);
}

/**
 * One row of `std.projects.list`.
 *
 * Row identity: `projects`. `synthetic_rows` is false.
 */
export interface StdProjectsListRow {
  /** `id` */
  readonly id: Ids.ProjectsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly summary: string;
  /** `datetime` */
  readonly added: string;
}

/** `std.projects.list` takes no parameters. */
export type StdProjectsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.projects.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProjectsListFields {
  readonly id: OpenField<'id', Ids.ProjectsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly summary: OpenField<'summary', string>;
  readonly added: OpenField<'added', string>;
}

/**
 * The refusals `std.projects.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projects$list(client: OrdoClient): ReadCall<StdProjectsListRow, StdProjectsListRefusal, StdProjectsListFields> {
  return new ReadCall(client, spec_std$projects$list, {});
}

/**
 * The row shape of `std.projects.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projects.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectsUpdateParams = {
  /** `id` */
  readonly id: Ids.ProjectsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `projects.name` */
  readonly name: string;
  /** `text` · fills `projects.summary` */
  readonly summary: string;
  /** `datetime` · fills `projects.added` */
  readonly added: string;
};

/**
 * The refusals `std.projects.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projects.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$projects$update(client: OrdoClient, params: StdProjectsUpdateParams): WriteCall<StdProjectsUpdateRow, StdProjectsUpdateRefusal> {
  return new WriteCall(client, spec_std$projects$update, params);
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
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `text` · fills `tasks.status` · has a default; omit to take it */
  readonly status?: string;
  /** `id` · fills `tasks.assignee` · has a default; omit it or pass null to take it */
  readonly assignee?: Ids.MembersId | null;
  /** `id` · fills `tasks.author` */
  readonly author: Ids.MembersId;
  /** `date` · fills `tasks.due` · has a default; omit it or pass null to take it */
  readonly due?: string | null;
  /** `datetime` · fills `tasks.added` */
  readonly added: string;
};

/**
 * The refusals `std.tasks.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
export type StdTasksDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
  /** `id` */
  readonly assignee: Ids.MembersId;
  /** `id` */
  readonly author: Ids.MembersId;
  /** `date` */
  readonly due: string;
  /** `datetime` */
  readonly added: string;
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
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
  readonly author: OpenField<'author', Ids.MembersId>;
  readonly due: OpenField<'due', string>;
  readonly added: OpenField<'added', string>;
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
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
  /** `id` */
  readonly assignee: Ids.MembersId;
  /** `id` */
  readonly author: Ids.MembersId;
  /** `date` */
  readonly due: string;
  /** `datetime` */
  readonly added: string;
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
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
  readonly author: OpenField<'author', Ids.MembersId>;
  readonly due: OpenField<'due', string>;
  readonly added: OpenField<'added', string>;
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
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `text` · fills `tasks.status` */
  readonly status: string;
  /** `id` · fills `tasks.assignee` */
  readonly assignee: Ids.MembersId;
  /** `id` · fills `tasks.author` */
  readonly author: Ids.MembersId;
  /** `date` · fills `tasks.due` */
  readonly due: string;
  /** `datetime` · fills `tasks.added` */
  readonly added: string;
};

/**
 * The refusals `std.tasks.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTasksUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
 * One row of `std.team.count`.
 */
export interface StdTeamCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.team.count` takes no parameters. */
export type StdTeamCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.team.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdTeamCountFields = Record<never, never>;

/**
 * The refusals `std.team.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$team$count(client: OrdoClient): SingleReadCall<StdTeamCountRow, StdTeamCountRefusal, StdTeamCountFields> {
  return new SingleReadCall(client, spec_std$team$count, {});
}

/**
 * The row shape of `std.team.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTeamCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.team.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTeamCreateParams = {
  /** `id` · fills `team.project` */
  readonly project: Ids.ProjectsId;
  /** `id` · fills `team.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `std.team.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$team$create(client: OrdoClient, params: StdTeamCreateParams): WriteCall<StdTeamCreateRow, StdTeamCreateRefusal> {
  return new WriteCall(client, spec_std$team$create, params);
}

/**
 * The row shape of `std.team.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTeamDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.team.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTeamDeleteParams = {
  /** `id` */
  readonly id: Ids.TeamId;
};

/**
 * The refusals `std.team.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$team$delete(client: OrdoClient, params: StdTeamDeleteParams): WriteCall<StdTeamDeleteRow, StdTeamDeleteRefusal> {
  return new WriteCall(client, spec_std$team$delete, params);
}

/**
 * One row of `std.team.get`.
 *
 * Row identity: `team`. `synthetic_rows` is false.
 */
export interface StdTeamGetRow {
  /** `id` */
  readonly id: Ids.TeamId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly member: Ids.MembersId;
}

/**
 * The parameters of `std.team.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTeamGetParams = {
  /** `id` */
  readonly id: Ids.TeamId;
};

/**
 * The fields a `.where` on `std.team.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTeamGetFields {
  readonly id: OpenField<'id', Ids.TeamId>;
  readonly rev: OpenField<'rev', number>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
}

/**
 * The refusals `std.team.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$team$get(client: OrdoClient, params: StdTeamGetParams): ReadCall<StdTeamGetRow, StdTeamGetRefusal, StdTeamGetFields> {
  return new ReadCall(client, spec_std$team$get, params);
}

/**
 * One row of `std.team.list`.
 *
 * Row identity: `team`. `synthetic_rows` is false.
 */
export interface StdTeamListRow {
  /** `id` */
  readonly id: Ids.TeamId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly member: Ids.MembersId;
}

/** `std.team.list` takes no parameters. */
export type StdTeamListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.team.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTeamListFields {
  readonly id: OpenField<'id', Ids.TeamId>;
  readonly rev: OpenField<'rev', number>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
}

/**
 * The refusals `std.team.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$team$list(client: OrdoClient): ReadCall<StdTeamListRow, StdTeamListRefusal, StdTeamListFields> {
  return new ReadCall(client, spec_std$team$list, {});
}

/**
 * The row shape of `std.team.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTeamPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.team.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTeamPutParams = {
  /** `id` · fills `team.project` */
  readonly project: Ids.ProjectsId;
  /** `id` · fills `team.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `std.team.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$team$put(client: OrdoClient, params: StdTeamPutParams): WriteCall<StdTeamPutRow, StdTeamPutRefusal> {
  return new WriteCall(client, spec_std$team$put, params);
}

/**
 * The row shape of `std.team.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTeamUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.team.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTeamUpdateParams = {
  /** `id` */
  readonly id: Ids.TeamId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `team.project` */
  readonly project: Ids.ProjectsId;
  /** `id` · fills `team.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `std.team.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTeamUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.team.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$team$update(client: OrdoClient, params: StdTeamUpdateParams): WriteCall<StdTeamUpdateRow, StdTeamUpdateRefusal> {
  return new WriteCall(client, spec_std$team$update, params);
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  board: board,
  createTask: createTask,
  editTask: editTask,
  me: me,
  moveTask: moveTask,
  myProjects: myProjects,
  people: people,
  projectTeam: projectTeam,
  reportByPerson: reportByPerson,
  reportByProject: reportByProject,
  'std.projects.count': std$projects$count,
  'std.projects.create': std$projects$create,
  'std.projects.delete': std$projects$delete,
  'std.projects.get': std$projects$get,
  'std.projects.list': std$projects$list,
  'std.projects.update': std$projects$update,
  'std.tasks.count': std$tasks$count,
  'std.tasks.create': std$tasks$create,
  'std.tasks.delete': std$tasks$delete,
  'std.tasks.get': std$tasks$get,
  'std.tasks.list': std$tasks$list,
  'std.tasks.update': std$tasks$update,
  'std.team.count': std$team$count,
  'std.team.create': std$team$create,
  'std.team.delete': std$team$delete,
  'std.team.get': std$team$get,
  'std.team.list': std$team$list,
  'std.team.put': std$team$put,
  'std.team.update': std$team$update,
} as const;

/** Today on the caller's clock, `YYYY-MM-DD`: the value of an omitted `filled` date. */
function filledDate(): string {
  const d = new Date();
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Now on the caller's clock, as an instant: the value of an omitted `filled` datetime. */
function filledDatetime(): string {
  return new Date().toISOString();
}
