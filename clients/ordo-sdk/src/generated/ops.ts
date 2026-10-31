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

import type { OrdoClient } from '../core/client.js';
import type { RefusalClass } from '../core/refusal.js';
import {
  spec_activeProducts,
  spec_addAnonCustomer,
  spec_addBoard,
  spec_addCard,
  spec_addCategory,
  spec_addCustomer,
  spec_addNote,
  spec_addRootCategory,
  spec_addTask,
  spec_allTasks,
  spec_anonCustomers,
  spec_boardList,
  spec_cardList,
  spec_catInfo,
  spec_cheapActive,
  spec_claimCoupon,
  spec_claimTask,
  spec_clearCustomerEmail,
  spec_closeTask,
  spec_customerByEmail,
  spec_doneOrders,
  spec_dropCategory,
  spec_dropCategoryTree,
  spec_dropFolder,
  spec_dropFolderTree,
  spec_entryList,
  spec_finishCard,
  spec_firstTask,
  spec_giftOrders,
  spec_grantBoardsAccess,
  spec_memoDigest,
  spec_monthlyRevenue,
  spec_moveBA,
  spec_moveCategory,
  spec_movePM,
  spec_myAssignedTasks,
  spec_myProjects,
  spec_noteFeed,
  spec_openTasks,
  spec_orderMemos,
  spec_productRevenue,
  spec_purgeDone,
  spec_purgeNotes,
  spec_reassign,
  spec_recentOrders,
  spec_recordEntry,
  spec_recordEntryFor,
  spec_recordEntryOnce,
  spec_renameTask,
  spec_renameTaskIf,
  spec_renumberEntry,
  spec_renumberEntryOnce,
  spec_reviseEntry,
  spec_revokeBoardsAccess,
  spec_setBoardsAccessLevel,
  spec_setMemo,
  spec_specCount,
  spec_specTasks,
  spec_spendByCustomer,
  spec_std$boards$count,
  spec_std$boards$create,
  spec_std$boards$delete,
  spec_std$boards$get,
  spec_std$boards$list,
  spec_std$boards$update,
  spec_std$boardsAccess$count,
  spec_std$boardsAccess$create,
  spec_std$boardsAccess$delete,
  spec_std$boardsAccess$get,
  spec_std$boardsAccess$list,
  spec_std$boardsAccess$put,
  spec_std$boardsAccess$update,
  spec_std$cards$count,
  spec_std$cards$create,
  spec_std$cards$delete,
  spec_std$cards$get,
  spec_std$cards$list,
  spec_std$cards$update,
  spec_std$categories$count,
  spec_std$categories$create,
  spec_std$categories$delete,
  spec_std$categories$deleteSubtree,
  spec_std$categories$get,
  spec_std$categories$list,
  spec_std$categories$put,
  spec_std$categories$update,
  spec_std$coupons$count,
  spec_std$coupons$delete,
  spec_std$coupons$get,
  spec_std$coupons$list,
  spec_std$coupons$update,
  spec_std$customers$count,
  spec_std$customers$create,
  spec_std$customers$delete,
  spec_std$customers$get,
  spec_std$customers$list,
  spec_std$customers$put,
  spec_std$customers$update,
  spec_std$folders$count,
  spec_std$folders$create,
  spec_std$folders$delete,
  spec_std$folders$deleteSubtree,
  spec_std$folders$get,
  spec_std$folders$list,
  spec_std$folders$update,
  spec_std$ledgerEntries$count,
  spec_std$ledgerEntries$create,
  spec_std$ledgerEntries$delete,
  spec_std$ledgerEntries$get,
  spec_std$ledgerEntries$list,
  spec_std$ledgerEntries$put,
  spec_std$ledgerEntries$update,
  spec_std$orderItems$count,
  spec_std$orderItems$create,
  spec_std$orderItems$delete,
  spec_std$orderItems$get,
  spec_std$orderItems$list,
  spec_std$orderItems$update,
  spec_std$orders$count,
  spec_std$orders$create,
  spec_std$orders$delete,
  spec_std$orders$get,
  spec_std$orders$list,
  spec_std$orders$update,
  spec_std$products$count,
  spec_std$products$create,
  spec_std$products$delete,
  spec_std$products$get,
  spec_std$products$list,
  spec_std$products$update,
  spec_std$projectMembers$count,
  spec_std$projectMembers$create,
  spec_std$projectMembers$delete,
  spec_std$projectMembers$get,
  spec_std$projectMembers$list,
  spec_std$projectMembers$update,
  spec_std$projects$count,
  spec_std$projects$create,
  spec_std$projects$delete,
  spec_std$projects$get,
  spec_std$projects$list,
  spec_std$projects$update,
  spec_std$taskNotes$count,
  spec_std$taskNotes$create,
  spec_std$taskNotes$delete,
  spec_std$taskNotes$get,
  spec_std$taskNotes$list,
  spec_std$taskNotes$update,
  spec_std$tasks$count,
  spec_std$tasks$create,
  spec_std$tasks$delete,
  spec_std$tasks$get,
  spec_std$tasks$list,
  spec_std$tasks$update,
  spec_std$vendors$count,
  spec_std$vendors$create,
  spec_std$vendors$delete,
  spec_std$vendors$get,
  spec_std$vendors$list,
  spec_std$vendors$update,
  spec_taskBoard,
  spec_teamRoster,
  spec_toolDeals,
  spec_top3Orders,
  spec_upsertCustomer,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '../core/call.js';
import type { OpenField } from '../core/refine.js';
import type * as Ids from './ids.js';

/**
 * One row of `activeProducts`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface ActiveProductsRow {
  /** `text` */
  readonly name: string;
  /** `money` */
  readonly price: string;
  /** `id` */
  readonly vendor: Ids.VendorsId;
  /** `id` */
  readonly category: Ids.CategoriesId;
  /** `id` */
  readonly $id: Ids.ProductsId;
}

/** `activeProducts` takes no parameters. */
export type ActiveProductsParams = Record<never, never>;

/**
 * The fields a `.where` on `activeProducts` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ActiveProductsFields {
  readonly name: OpenField<'name', string>;
  readonly price: OpenField<'price', string>;
  readonly vendor: OpenField<'vendor', Ids.VendorsId>;
  readonly category: OpenField<'category', Ids.CategoriesId>;
  readonly $id: OpenField<'$id', Ids.ProductsId>;
}

/**
 * The refusals `activeProducts` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ActiveProductsRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `activeProducts` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function activeProducts(client: OrdoClient): ReadCall<ActiveProductsRow, ActiveProductsRefusal, ActiveProductsFields> {
  return new ReadCall(client, spec_activeProducts, {});
}

/**
 * The row shape of `addAnonCustomer`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddAnonCustomerRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addAnonCustomer`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddAnonCustomerParams = {
  /** `text` · fills `customers.name` */
  readonly name: string;
};

/**
 * The refusals `addAnonCustomer` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddAnonCustomerRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addAnonCustomer` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addAnonCustomer(client: OrdoClient, params: AddAnonCustomerParams): WriteCall<AddAnonCustomerRow, AddAnonCustomerRefusal> {
  return new WriteCall(client, spec_addAnonCustomer, params);
}

/**
 * The row shape of `addBoard`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddBoardRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addBoard`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddBoardParams = {
  /** `text` · fills `boards.name` */
  readonly name: string;
};

/**
 * The refusals `addBoard` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddBoardRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addBoard` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addBoard(client: OrdoClient, params: AddBoardParams): WriteCall<AddBoardRow, AddBoardRefusal> {
  return new WriteCall(client, spec_addBoard, params);
}

/**
 * The row shape of `addCard`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddCardRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addCard`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddCardParams = {
  /** `id` · fills `cards.board` */
  readonly board: Ids.BoardsId;
  /** `text` · fills `cards.title` */
  readonly title: string;
};

/**
 * The refusals `addCard` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddCardRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addCard` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addCard(client: OrdoClient, params: AddCardParams): WriteCall<AddCardRow, AddCardRefusal> {
  return new WriteCall(client, spec_addCard, params);
}

/**
 * The row shape of `addCategory`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddCategoryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addCategory`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddCategoryParams = {
  /** `text` · fills `categories.name` */
  readonly name: string;
  /** `id` · fills `categories.parent` */
  readonly parent: Ids.CategoriesId;
};

/**
 * The refusals `addCategory` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddCategoryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `addCategory` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function addCategory(client: OrdoClient, params: AddCategoryParams): WriteCall<AddCategoryRow, AddCategoryRefusal> {
  return new WriteCall(client, spec_addCategory, params);
}

/**
 * The row shape of `addCustomer`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddCustomerRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addCustomer`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddCustomerParams = {
  /** `text` · fills `customers.name` */
  readonly name: string;
  /** `text` · fills `customers.email` */
  readonly email: string;
};

/**
 * The refusals `addCustomer` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddCustomerRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addCustomer` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function addCustomer(client: OrdoClient, params: AddCustomerParams): WriteCall<AddCustomerRow, AddCustomerRefusal> {
  return new WriteCall(client, spec_addCustomer, params);
}

/**
 * The row shape of `addNote`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddNoteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addNote`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddNoteParams = {
  /** `id` · fills `taskNotes.task` */
  readonly task: Ids.TasksId;
  /** `text` · fills `taskNotes.body` */
  readonly body: string;
};

/**
 * The refusals `addNote` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddNoteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addNote` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function addNote(client: OrdoClient, params: AddNoteParams): WriteCall<AddNoteRow, AddNoteRefusal> {
  return new WriteCall(client, spec_addNote, params);
}

/**
 * The row shape of `addRootCategory`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddRootCategoryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addRootCategory`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddRootCategoryParams = {
  /** `text` · fills `categories.name` */
  readonly name: string;
};

/**
 * The refusals `addRootCategory` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddRootCategoryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `addRootCategory` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function addRootCategory(client: OrdoClient, params: AddRootCategoryParams): WriteCall<AddRootCategoryRow, AddRootCategoryRefusal> {
  return new WriteCall(client, spec_addRootCategory, params);
}

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
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
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
 * One row of `allTasks`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface AllTasksRow {
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `allTasks` takes no parameters. */
export type AllTasksParams = Record<never, never>;

/**
 * The fields a `.where` on `allTasks` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface AllTasksFields {
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `allTasks` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AllTasksRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `allTasks` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * **`unguarded`** — this op bypasses row guards.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function allTasks(client: OrdoClient): ReadCall<AllTasksRow, AllTasksRefusal, AllTasksFields> {
  return new ReadCall(client, spec_allTasks, {});
}

/**
 * One row of `anonCustomers`.
 *
 * Row identity: `customers`. `synthetic_rows` is false.
 */
export interface AnonCustomersRow {
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly $id: Ids.CustomersId;
}

/** `anonCustomers` takes no parameters. */
export type AnonCustomersParams = Record<never, never>;

/**
 * The fields a `.where` on `anonCustomers` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface AnonCustomersFields {
  readonly name: OpenField<'name', string>;
  readonly $id: OpenField<'$id', Ids.CustomersId>;
}

/**
 * The refusals `anonCustomers` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AnonCustomersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'sealed_open_failed' | 'sealed_unreadable' | 'unknown_param' | 'version_skew'>;

/**
 * `anonCustomers` — read.
 * Cost class `blind-index`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function anonCustomers(client: OrdoClient): ReadCall<AnonCustomersRow, AnonCustomersRefusal, AnonCustomersFields> {
  return new ReadCall(client, spec_anonCustomers, {});
}

/**
 * One row of `boardList`.
 *
 * Row identity: `boards`. `synthetic_rows` is false.
 */
export interface BoardListRow {
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly $id: Ids.BoardsId;
}

/** `boardList` takes no parameters. */
export type BoardListParams = Record<never, never>;

/**
 * The fields a `.where` on `boardList` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface BoardListFields {
  readonly name: OpenField<'name', string>;
  readonly $id: OpenField<'$id', Ids.BoardsId>;
}

/**
 * The refusals `boardList` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type BoardListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `boardList` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function boardList(client: OrdoClient): ReadCall<BoardListRow, BoardListRefusal, BoardListFields> {
  return new ReadCall(client, spec_boardList, {});
}

/**
 * One row of `cardList`.
 *
 * Row identity: `cards`. `synthetic_rows` is false.
 */
export interface CardListRow {
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
  /** `id` */
  readonly board: Ids.BoardsId;
  /** `id` */
  readonly $id: Ids.CardsId;
}

/** `cardList` takes no parameters. */
export type CardListParams = Record<never, never>;

/**
 * The fields a `.where` on `cardList` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface CardListFields {
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
  readonly board: OpenField<'board', Ids.BoardsId>;
  readonly $id: OpenField<'$id', Ids.CardsId>;
}

/**
 * The refusals `cardList` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CardListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `cardList` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function cardList(client: OrdoClient): ReadCall<CardListRow, CardListRefusal, CardListFields> {
  return new ReadCall(client, spec_cardList, {});
}

/**
 * One row of `catInfo`.
 *
 * Row identity: `categories`. `synthetic_rows` is false.
 */
export interface CatInfoRow {
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly parent: Ids.CategoriesId;
  /** `int` */
  readonly depth: number;
  /** `id` */
  readonly $id: Ids.CategoriesId;
}

/** `catInfo` takes no parameters. */
export type CatInfoParams = Record<never, never>;

/**
 * The fields a `.where` on `catInfo` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface CatInfoFields {
  readonly name: OpenField<'name', string>;
  readonly parent: OpenField<'parent', Ids.CategoriesId>;
  readonly depth: OpenField<'depth', number>;
  readonly $id: OpenField<'$id', Ids.CategoriesId>;
}

/**
 * The refusals `catInfo` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CatInfoRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `catInfo` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function catInfo(client: OrdoClient): ReadCall<CatInfoRow, CatInfoRefusal, CatInfoFields> {
  return new ReadCall(client, spec_catInfo, {});
}

/**
 * One row of `cheapActive`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface CheapActiveRow {
  /** `text` */
  readonly name: string;
  /** `money` */
  readonly price: string;
  /** `id` */
  readonly category: Ids.CategoriesId;
  /** `id` */
  readonly $id: Ids.ProductsId;
}

/**
 * The parameters of `cheapActive`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type CheapActiveParams = {
  /** `money` */
  readonly cap: string;
};

/**
 * The fields a `.where` on `cheapActive` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface CheapActiveFields {
  readonly name: OpenField<'name', string>;
  readonly price: OpenField<'price', string>;
  readonly category: OpenField<'category', Ids.CategoriesId>;
  readonly $id: OpenField<'$id', Ids.ProductsId>;
}

/**
 * The refusals `cheapActive` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CheapActiveRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `cheapActive` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function cheapActive(client: OrdoClient, params: CheapActiveParams): ReadCall<CheapActiveRow, CheapActiveRefusal, CheapActiveFields> {
  return new ReadCall(client, spec_cheapActive, params);
}

/**
 * The row shape of `claimCoupon`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ClaimCouponRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `claimCoupon`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ClaimCouponParams = {
  /** `text` · fills `coupons.code` */
  readonly code: string;
  /** `text` · fills `coupons.slug` */
  readonly slug: string;
  /** `money` · fills `coupons.value` */
  readonly value: string;
};

/**
 * The refusals `claimCoupon` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ClaimCouponRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `claimCoupon` — write.
 *
 * Mode: **ambiguous-identity**.
 * Terminates in `.commit()`.
 */
export function claimCoupon(client: OrdoClient, params: ClaimCouponParams): WriteCall<ClaimCouponRow, ClaimCouponRefusal> {
  return new WriteCall(client, spec_claimCoupon, params);
}

/**
 * The row shape of `claimTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ClaimTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `claimTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ClaimTaskParams = {
  /** `id` */
  readonly task: Ids.TasksId;
};

/**
 * The refusals `claimTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ClaimTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `claimTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function claimTask(client: OrdoClient, params: ClaimTaskParams): WriteCall<ClaimTaskRow, ClaimTaskRefusal> {
  return new WriteCall(client, spec_claimTask, params);
}

/**
 * The row shape of `clearCustomerEmail`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ClearCustomerEmailRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `clearCustomerEmail`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ClearCustomerEmailParams = {
  /** `id` */
  readonly c: Ids.CustomersId;
};

/**
 * The refusals `clearCustomerEmail` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ClearCustomerEmailRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `clearCustomerEmail` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function clearCustomerEmail(client: OrdoClient, params: ClearCustomerEmailParams): WriteCall<ClearCustomerEmailRow, ClearCustomerEmailRefusal> {
  return new WriteCall(client, spec_clearCustomerEmail, params);
}

/**
 * The row shape of `closeTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type CloseTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `closeTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type CloseTaskParams = {
  /** `id` */
  readonly task: Ids.TasksId;
};

/**
 * The refusals `closeTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CloseTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `closeTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function closeTask(client: OrdoClient, params: CloseTaskParams): WriteCall<CloseTaskRow, CloseTaskRefusal> {
  return new WriteCall(client, spec_closeTask, params);
}

/**
 * One row of `customerByEmail`.
 *
 * Row identity: `customers`. `synthetic_rows` is false.
 */
export interface CustomerByEmailRow {
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly tier: string;
  /** `id` */
  readonly $id: Ids.CustomersId;
}

/**
 * The parameters of `customerByEmail`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type CustomerByEmailParams = {
  /** `text` · fills `customers.email` */
  readonly e: string;
};

/**
 * The fields a `.where` on `customerByEmail` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface CustomerByEmailFields {
  readonly name: OpenField<'name', string>;
  readonly tier: OpenField<'tier', string>;
  readonly $id: OpenField<'$id', Ids.CustomersId>;
}

/**
 * The refusals `customerByEmail` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CustomerByEmailRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'sealed_open_failed' | 'sealed_unreadable' | 'unknown_param' | 'version_skew'>;

/**
 * `customerByEmail` — read.
 * Cost class `blind-index`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function customerByEmail(client: OrdoClient, params: CustomerByEmailParams): ReadCall<CustomerByEmailRow, CustomerByEmailRefusal, CustomerByEmailFields> {
  return new ReadCall(client, spec_customerByEmail, params);
}

/**
 * One row of `doneOrders`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface DoneOrdersRow {
  /** `date` */
  readonly placed: string;
  /** `money` */
  readonly total: string;
  /** `id` */
  readonly customer: Ids.CustomersId;
  /** `id` */
  readonly $id: Ids.OrdersId;
}

/**
 * The parameters of `doneOrders`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DoneOrdersParams = {
  /** `date` · fills `orders.placed` */
  readonly from: string;
};

/**
 * The fields a `.where` on `doneOrders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface DoneOrdersFields {
  readonly placed: OpenField<'placed', string>;
  readonly total: OpenField<'total', string>;
  readonly customer: OpenField<'customer', Ids.CustomersId>;
  readonly $id: OpenField<'$id', Ids.OrdersId>;
}

/**
 * The refusals `doneOrders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DoneOrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `doneOrders` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function doneOrders(client: OrdoClient, params: DoneOrdersParams): ReadCall<DoneOrdersRow, DoneOrdersRefusal, DoneOrdersFields> {
  return new ReadCall(client, spec_doneOrders, params);
}

/**
 * The row shape of `dropCategory`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DropCategoryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `dropCategory`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DropCategoryParams = {
  /** `id` */
  readonly cat: Ids.CategoriesId;
};

/**
 * The refusals `dropCategory` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DropCategoryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `dropCategory` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function dropCategory(client: OrdoClient, params: DropCategoryParams): WriteCall<DropCategoryRow, DropCategoryRefusal> {
  return new WriteCall(client, spec_dropCategory, params);
}

/**
 * The row shape of `dropCategoryTree`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DropCategoryTreeRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `dropCategoryTree`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DropCategoryTreeParams = {
  /** `id` */
  readonly cat: Ids.CategoriesId;
};

/**
 * The refusals `dropCategoryTree` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DropCategoryTreeRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `dropCategoryTree` — delete.
 *
 * Mode: **deletes subtree**.
 * Terminates in `.commit()`.
 */
export function dropCategoryTree(client: OrdoClient, params: DropCategoryTreeParams): WriteCall<DropCategoryTreeRow, DropCategoryTreeRefusal> {
  return new WriteCall(client, spec_dropCategoryTree, params);
}

/**
 * The row shape of `dropFolder`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DropFolderRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `dropFolder`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DropFolderParams = {
  /** `id` */
  readonly f: Ids.FoldersId;
};

/**
 * The refusals `dropFolder` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DropFolderRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `dropFolder` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function dropFolder(client: OrdoClient, params: DropFolderParams): WriteCall<DropFolderRow, DropFolderRefusal> {
  return new WriteCall(client, spec_dropFolder, params);
}

/**
 * The row shape of `dropFolderTree`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type DropFolderTreeRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `dropFolderTree`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type DropFolderTreeParams = {
  /** `id` */
  readonly f: Ids.FoldersId;
};

/**
 * The refusals `dropFolderTree` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type DropFolderTreeRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `dropFolderTree` — delete.
 *
 * Mode: **deletes subtree**.
 * Terminates in `.commit()`.
 */
export function dropFolderTree(client: OrdoClient, params: DropFolderTreeParams): WriteCall<DropFolderTreeRow, DropFolderTreeRefusal> {
  return new WriteCall(client, spec_dropFolderTree, params);
}

/**
 * One row of `entryList`.
 *
 * Row identity: `ledgerEntries`. `synthetic_rows` is false.
 */
export interface EntryListRow {
  /** `text` */
  readonly externalId: string;
  /** `money` */
  readonly amount: string;
  /** `text` */
  readonly note: string;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `id` */
  readonly $id: Ids.LedgerEntriesId;
}

/** `entryList` takes no parameters. */
export type EntryListParams = Record<never, never>;

/**
 * The fields a `.where` on `entryList` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface EntryListFields {
  readonly externalId: OpenField<'externalId', string>;
  readonly amount: OpenField<'amount', string>;
  readonly note: OpenField<'note', string>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly $id: OpenField<'$id', Ids.LedgerEntriesId>;
}

/**
 * The refusals `entryList` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type EntryListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `entryList` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function entryList(client: OrdoClient): ReadCall<EntryListRow, EntryListRefusal, EntryListFields> {
  return new ReadCall(client, spec_entryList, {});
}

/**
 * The row shape of `finishCard`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type FinishCardRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `finishCard`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type FinishCardParams = {
  /** `id` */
  readonly card: Ids.CardsId;
};

/**
 * The refusals `finishCard` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type FinishCardRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `finishCard` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function finishCard(client: OrdoClient, params: FinishCardParams): WriteCall<FinishCardRow, FinishCardRefusal> {
  return new WriteCall(client, spec_finishCard, params);
}

/**
 * One row of `firstTask`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface FirstTaskRow {
  /** `text` */
  readonly title: string;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `firstTask` takes no parameters. */
export type FirstTaskParams = Record<never, never>;

/**
 * The fields a `.where` on `firstTask` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface FirstTaskFields {
  readonly title: OpenField<'title', string>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `firstTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type FirstTaskRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `firstTask` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function firstTask(client: OrdoClient): ReadCall<FirstTaskRow, FirstTaskRefusal, FirstTaskFields> {
  return new ReadCall(client, spec_firstTask, {});
}

/**
 * The row shape of `giftOrders`.
 *
 * The descriptor declares no output fields for this op.
 */
export type GiftOrdersRow = Readonly<Record<string, unknown>>;

/** `giftOrders` takes no parameters. */
export type GiftOrdersParams = Record<never, never>;

/**
 * The fields a `.where` on `giftOrders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type GiftOrdersFields = Record<never, never>;

/**
 * The refusals `giftOrders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type GiftOrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `giftOrders` — read.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * **Refused at registration**: `memo` is sealed: the database holds only its ciphertext, so it cannot filter, sort, aggregate or compute on it. Project `memo` as it is to read it opened on the server, `count` the rows that carry it, or declare it `sealed indexed` to look it up with `==` — calling it refuses before the wire.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function giftOrders(client: OrdoClient): ReadCall<GiftOrdersRow, GiftOrdersRefusal, GiftOrdersFields> {
  return new ReadCall(client, spec_giftOrders, {});
}

/**
 * The row shape of `grantBoardsAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type GrantBoardsAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `grantBoardsAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type GrantBoardsAccessParams = {
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `boardsAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `grantBoardsAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type GrantBoardsAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `grantBoardsAccess` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function grantBoardsAccess(client: OrdoClient, params: GrantBoardsAccessParams): WriteCall<GrantBoardsAccessRow, GrantBoardsAccessRefusal> {
  return new WriteCall(client, spec_grantBoardsAccess, params);
}

/**
 * One row of `memoDigest`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface MemoDigestRow {
  /** `date` */
  readonly placed: string;
  /** `text`, sealed — opened host-side per row. */
  readonly memo: string;
  /** `id` */
  readonly $id: Ids.OrdersId;
}

/** `memoDigest` takes no parameters. */
export type MemoDigestParams = Record<never, never>;

/**
 * The fields a `.where` on `memoDigest` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MemoDigestFields {
  readonly placed: OpenField<'placed', string>;
  readonly $id: OpenField<'$id', Ids.OrdersId>;
}

/**
 * The refusals `memoDigest` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MemoDigestRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'sealed_open_failed' | 'sealed_unreadable' | 'sealed_unsupported' | 'unknown_param' | 'version_skew'>;

/**
 * `memoDigest` — read.
 * Cost class `bounded-host`, rows: <= 3.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function memoDigest(client: OrdoClient): ReadCall<MemoDigestRow, MemoDigestRefusal, MemoDigestFields> {
  return new ReadCall(client, spec_memoDigest, {});
}

/**
 * One row of `monthlyRevenue`.
 */
export interface MonthlyRevenueRow {
  /** `text` */
  readonly month: string;
  /** `money` */
  readonly revenue: string;
}

/**
 * The parameters of `monthlyRevenue`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MonthlyRevenueParams = {
  /** `date` */
  readonly from: string;
};

/**
 * The fields a `.where` on `monthlyRevenue` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MonthlyRevenueFields {
  readonly month: OpenField<'month', string>;
  readonly revenue: OpenField<'revenue', string>;
}

/**
 * The refusals `monthlyRevenue` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MonthlyRevenueRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `monthlyRevenue` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function monthlyRevenue(client: OrdoClient, params: MonthlyRevenueParams): ReadCall<MonthlyRevenueRow, MonthlyRevenueRefusal, MonthlyRevenueFields> {
  return new ReadCall(client, spec_monthlyRevenue, params);
}

/**
 * The row shape of `moveBA`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type MoveBARow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `moveBA`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MoveBAParams = {
  /** `id` */
  readonly ba: Ids.BoardsAccessId;
  /** `id` · fills `boardsAccess.board` */
  readonly b: Ids.BoardsId;
};

/**
 * The refusals `moveBA` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MoveBARefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `moveBA` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function moveBA(client: OrdoClient, params: MoveBAParams): WriteCall<MoveBARow, MoveBARefusal> {
  return new WriteCall(client, spec_moveBA, params);
}

/**
 * The row shape of `moveCategory`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type MoveCategoryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `moveCategory`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MoveCategoryParams = {
  /** `id` */
  readonly cat: Ids.CategoriesId;
  /** `id` · fills `categories.parent` */
  readonly parent: Ids.CategoriesId;
};

/**
 * The refusals `moveCategory` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MoveCategoryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `moveCategory` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function moveCategory(client: OrdoClient, params: MoveCategoryParams): WriteCall<MoveCategoryRow, MoveCategoryRefusal> {
  return new WriteCall(client, spec_moveCategory, params);
}

/**
 * The row shape of `movePM`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type MovePMRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `movePM`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MovePMParams = {
  /** `id` */
  readonly pm: Ids.ProjectMembersId;
  /** `id` · fills `projectMembers.project` */
  readonly p: Ids.ProjectsId;
};

/**
 * The refusals `movePM` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MovePMRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `movePM` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function movePM(client: OrdoClient, params: MovePMParams): WriteCall<MovePMRow, MovePMRefusal> {
  return new WriteCall(client, spec_movePM, params);
}

/**
 * One row of `myAssignedTasks`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface MyAssignedTasksRow {
  /** `text` */
  readonly title: string;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `myAssignedTasks` takes no parameters. */
export type MyAssignedTasksParams = Record<never, never>;

/**
 * The fields a `.where` on `myAssignedTasks` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyAssignedTasksFields {
  readonly title: OpenField<'title', string>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `myAssignedTasks` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyAssignedTasksRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myAssignedTasks` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myAssignedTasks(client: OrdoClient): ReadCall<MyAssignedTasksRow, MyAssignedTasksRefusal, MyAssignedTasksFields> {
  return new ReadCall(client, spec_myAssignedTasks, {});
}

/**
 * One row of `myProjects`.
 *
 * Row identity: `projects`. `synthetic_rows` is false.
 */
export interface MyProjectsRow {
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `id` */
  readonly $id: Ids.ProjectsId;
}

/** `myProjects` takes no parameters. */
export type MyProjectsParams = Record<never, never>;

/**
 * The fields a `.where` on `myProjects` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyProjectsFields {
  readonly name: OpenField<'name', string>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly $id: OpenField<'$id', Ids.ProjectsId>;
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
 * One row of `noteFeed`.
 *
 * Row identity: `taskNotes`. `synthetic_rows` is false.
 */
export interface NoteFeedRow {
  /** `text` */
  readonly body: string;
  /** `id` */
  readonly task: Ids.TasksId;
  /** `id` */
  readonly $id: Ids.TaskNotesId;
}

/** `noteFeed` takes no parameters. */
export type NoteFeedParams = Record<never, never>;

/**
 * The fields a `.where` on `noteFeed` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface NoteFeedFields {
  readonly body: OpenField<'body', string>;
  readonly task: OpenField<'task', Ids.TasksId>;
  readonly $id: OpenField<'$id', Ids.TaskNotesId>;
}

/**
 * The refusals `noteFeed` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type NoteFeedRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `noteFeed` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function noteFeed(client: OrdoClient): ReadCall<NoteFeedRow, NoteFeedRefusal, NoteFeedFields> {
  return new ReadCall(client, spec_noteFeed, {});
}

/**
 * One row of `openTasks`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface OpenTasksRow {
  /** `text` */
  readonly title: string;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `openTasks` takes no parameters. */
export type OpenTasksParams = Record<never, never>;

/**
 * The fields a `.where` on `openTasks` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface OpenTasksFields {
  readonly title: OpenField<'title', string>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `openTasks` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type OpenTasksRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `openTasks` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function openTasks(client: OrdoClient): ReadCall<OpenTasksRow, OpenTasksRefusal, OpenTasksFields> {
  return new ReadCall(client, spec_openTasks, {});
}

/**
 * One row of `orderMemos`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface OrderMemosRow {
  /** `date` */
  readonly placed: string;
  /** `money` */
  readonly total: string;
  /** `text`, sealed — opened host-side per row. */
  readonly memo: string;
  /** `id` */
  readonly $id: Ids.OrdersId;
}

/** `orderMemos` takes no parameters. */
export type OrderMemosParams = Record<never, never>;

/**
 * The fields a `.where` on `orderMemos` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface OrderMemosFields {
  readonly placed: OpenField<'placed', string>;
  readonly total: OpenField<'total', string>;
  readonly $id: OpenField<'$id', Ids.OrdersId>;
}

/**
 * The refusals `orderMemos` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type OrderMemosRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'sealed_open_failed' | 'sealed_unreadable' | 'sealed_unsupported' | 'unknown_param' | 'version_skew'>;

/**
 * `orderMemos` — read.
 * Cost class `bounded-host`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function orderMemos(client: OrdoClient): ReadCall<OrderMemosRow, OrderMemosRefusal, OrderMemosFields> {
  return new ReadCall(client, spec_orderMemos, {});
}

/**
 * One row of `productRevenue`.
 */
export interface ProductRevenueRow {
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly sold: number;
  /** `money` */
  readonly revenue: string;
}

/** `productRevenue` takes no parameters. */
export type ProductRevenueParams = Record<never, never>;

/**
 * The fields a `.where` on `productRevenue` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ProductRevenueFields {
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly sold: OpenField<'sold', number>;
  readonly revenue: OpenField<'revenue', string>;
}

/**
 * The refusals `productRevenue` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ProductRevenueRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `productRevenue` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function productRevenue(client: OrdoClient): ReadCall<ProductRevenueRow, ProductRevenueRefusal, ProductRevenueFields> {
  return new ReadCall(client, spec_productRevenue, {});
}

/**
 * The row shape of `purgeDone`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type PurgeDoneRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `purgeDone`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type PurgeDoneParams = {
  /** `id` · fills `tasks.project` */
  readonly project: Ids.ProjectsId;
};

/**
 * The refusals `purgeDone` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PurgeDoneRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `purgeDone` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function purgeDone(client: OrdoClient, params: PurgeDoneParams): WriteCall<PurgeDoneRow, PurgeDoneRefusal> {
  return new WriteCall(client, spec_purgeDone, params);
}

/**
 * The row shape of `purgeNotes`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type PurgeNotesRow = Readonly<Record<string, unknown>>;

/** `purgeNotes` takes no parameters. */
export type PurgeNotesParams = Record<never, never>;

/**
 * The refusals `purgeNotes` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PurgeNotesRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'log_append_failed' | 'method_mismatch' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `purgeNotes` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function purgeNotes(client: OrdoClient): WriteCall<PurgeNotesRow, PurgeNotesRefusal> {
  return new WriteCall(client, spec_purgeNotes, {});
}

/**
 * The row shape of `reassign`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ReassignRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `reassign`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ReassignParams = {
  /** `id` */
  readonly task: Ids.TasksId;
  /** `id` · fills `tasks.assignee` */
  readonly who: Ids.MembersId;
};

/**
 * The refusals `reassign` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ReassignRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `reassign` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function reassign(client: OrdoClient, params: ReassignParams): WriteCall<ReassignRow, ReassignRefusal> {
  return new WriteCall(client, spec_reassign, params);
}

/**
 * One row of `recentOrders`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface RecentOrdersRow {
  /** `date` */
  readonly placed: string;
  /** `text` */
  readonly status: string;
  /** `money` */
  readonly total: string;
  /** `id` */
  readonly $id: Ids.OrdersId;
}

/**
 * The parameters of `recentOrders`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RecentOrdersParams = {
  /** `date` · fills `orders.placed` · has a default; omit to take it */
  readonly since?: string;
};

/**
 * The fields a `.where` on `recentOrders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface RecentOrdersFields {
  readonly placed: OpenField<'placed', string>;
  readonly status: OpenField<'status', string>;
  readonly total: OpenField<'total', string>;
  readonly $id: OpenField<'$id', Ids.OrdersId>;
}

/**
 * The refusals `recentOrders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RecentOrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `recentOrders` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function recentOrders(client: OrdoClient, params: RecentOrdersParams): ReadCall<RecentOrdersRow, RecentOrdersRefusal, RecentOrdersFields> {
  return new ReadCall(client, spec_recentOrders, params);
}

/**
 * The row shape of `recordEntry`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RecordEntryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `recordEntry`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RecordEntryParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
  /** `text` · fills `ledgerEntries.note` · has a default; omit to take it */
  readonly note?: string;
};

/**
 * The refusals `recordEntry` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RecordEntryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `recordEntry` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function recordEntry(client: OrdoClient, params: RecordEntryParams): WriteCall<RecordEntryRow, RecordEntryRefusal> {
  return new WriteCall(client, spec_recordEntry, params);
}

/**
 * The row shape of `recordEntryFor`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RecordEntryForRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `recordEntryFor`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RecordEntryForParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
  /** `id` · fills `ledgerEntries.owner` */
  readonly owner: Ids.MembersId;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
};

/**
 * The refusals `recordEntryFor` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RecordEntryForRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `recordEntryFor` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function recordEntryFor(client: OrdoClient, params: RecordEntryForParams): WriteCall<RecordEntryForRow, RecordEntryForRefusal> {
  return new WriteCall(client, spec_recordEntryFor, params);
}

/**
 * The row shape of `recordEntryOnce`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RecordEntryOnceRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `recordEntryOnce`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RecordEntryOnceParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
};

/**
 * The refusals `recordEntryOnce` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RecordEntryOnceRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `recordEntryOnce` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function recordEntryOnce(client: OrdoClient, params: RecordEntryOnceParams): WriteCall<RecordEntryOnceRow, RecordEntryOnceRefusal> {
  return new WriteCall(client, spec_recordEntryOnce, params);
}

/**
 * The row shape of `renameTask`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RenameTaskRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `renameTask`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RenameTaskParams = {
  /** `id` */
  readonly task: Ids.TasksId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
};

/**
 * The refusals `renameTask` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RenameTaskRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `renameTask` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function renameTask(client: OrdoClient, params: RenameTaskParams): WriteCall<RenameTaskRow, RenameTaskRefusal> {
  return new WriteCall(client, spec_renameTask, params);
}

/**
 * The row shape of `renameTaskIf`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RenameTaskIfRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `renameTaskIf`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RenameTaskIfParams = {
  /** `id` */
  readonly task: Ids.TasksId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `int` */
  readonly rev: number;
};

/**
 * The refusals `renameTaskIf` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RenameTaskIfRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `renameTaskIf` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function renameTaskIf(client: OrdoClient, params: RenameTaskIfParams): WriteCall<RenameTaskIfRow, RenameTaskIfRefusal> {
  return new WriteCall(client, spec_renameTaskIf, params);
}

/**
 * The row shape of `renumberEntry`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RenumberEntryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `renumberEntry`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RenumberEntryParams = {
  /** `id` */
  readonly entry: Ids.LedgerEntriesId;
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
};

/**
 * The refusals `renumberEntry` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RenumberEntryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `renumberEntry` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function renumberEntry(client: OrdoClient, params: RenumberEntryParams): WriteCall<RenumberEntryRow, RenumberEntryRefusal> {
  return new WriteCall(client, spec_renumberEntry, params);
}

/**
 * The row shape of `renumberEntryOnce`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RenumberEntryOnceRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `renumberEntryOnce`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RenumberEntryOnceParams = {
  /** `id` */
  readonly entry: Ids.LedgerEntriesId;
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
};

/**
 * The refusals `renumberEntryOnce` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RenumberEntryOnceRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `renumberEntryOnce` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function renumberEntryOnce(client: OrdoClient, params: RenumberEntryOnceParams): WriteCall<RenumberEntryOnceRow, RenumberEntryOnceRefusal> {
  return new WriteCall(client, spec_renumberEntryOnce, params);
}

/**
 * The row shape of `reviseEntry`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type ReviseEntryRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `reviseEntry`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ReviseEntryParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly ext: string;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
  /** `int` */
  readonly rev: number;
};

/**
 * The refusals `reviseEntry` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ReviseEntryRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `reviseEntry` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function reviseEntry(client: OrdoClient, params: ReviseEntryParams): WriteCall<ReviseEntryRow, ReviseEntryRefusal> {
  return new WriteCall(client, spec_reviseEntry, params);
}

/**
 * The row shape of `revokeBoardsAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RevokeBoardsAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `revokeBoardsAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RevokeBoardsAccessParams = {
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `revokeBoardsAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RevokeBoardsAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `revokeBoardsAccess` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function revokeBoardsAccess(client: OrdoClient, params: RevokeBoardsAccessParams): WriteCall<RevokeBoardsAccessRow, RevokeBoardsAccessRefusal> {
  return new WriteCall(client, spec_revokeBoardsAccess, params);
}

/**
 * The row shape of `setBoardsAccessLevel`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type SetBoardsAccessLevelRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `setBoardsAccessLevel`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SetBoardsAccessLevelParams = {
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `boardsAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `setBoardsAccessLevel` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SetBoardsAccessLevelRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `setBoardsAccessLevel` — write.
 *
 * Mode: **bulk**.
 * Terminates in `.commit()`.
 */
export function setBoardsAccessLevel(client: OrdoClient, params: SetBoardsAccessLevelParams): WriteCall<SetBoardsAccessLevelRow, SetBoardsAccessLevelRefusal> {
  return new WriteCall(client, spec_setBoardsAccessLevel, params);
}

/**
 * The row shape of `setMemo`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type SetMemoRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `setMemo`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SetMemoParams = {
  /** `id` */
  readonly order: Ids.OrdersId;
  /** `text` · fills `orders.memo` */
  readonly memo: string;
};

/**
 * The refusals `setMemo` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SetMemoRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `setMemo` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function setMemo(client: OrdoClient, params: SetMemoParams): WriteCall<SetMemoRow, SetMemoRefusal> {
  return new WriteCall(client, spec_setMemo, params);
}

/**
 * One row of `specCount`.
 */
export interface SpecCountRow {
  /** `int` */
  readonly n: number;
}

/** `specCount` takes no parameters. */
export type SpecCountParams = Record<never, never>;

/**
 * The fields a `.where` on `specCount` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface SpecCountFields {
  readonly n: OpenField<'n', number>;
}

/**
 * The refusals `specCount` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SpecCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `specCount` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function specCount(client: OrdoClient): SingleReadCall<SpecCountRow, SpecCountRefusal, SpecCountFields> {
  return new SingleReadCall(client, spec_specCount, {});
}

/**
 * One row of `specTasks`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface SpecTasksRow {
  /** `text` */
  readonly title: string;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `specTasks` takes no parameters. */
export type SpecTasksParams = Record<never, never>;

/**
 * The fields a `.where` on `specTasks` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface SpecTasksFields {
  readonly title: OpenField<'title', string>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `specTasks` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SpecTasksRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `specTasks` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function specTasks(client: OrdoClient): ReadCall<SpecTasksRow, SpecTasksRefusal, SpecTasksFields> {
  return new ReadCall(client, spec_specTasks, {});
}

/**
 * One row of `spendByCustomer`.
 */
export interface SpendByCustomerRow {
  /** `id` */
  readonly customer: Ids.CustomersId;
  /** `money` */
  readonly spent: string;
  /** `int` */
  readonly n: number;
}

/**
 * The parameters of `spendByCustomer`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SpendByCustomerParams = {
  /** `date` */
  readonly from: string;
};

/**
 * The fields a `.where` on `spendByCustomer` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface SpendByCustomerFields {
  readonly customer: OpenField<'customer', Ids.CustomersId>;
  readonly spent: OpenField<'spent', string>;
  readonly n: OpenField<'n', number>;
}

/**
 * The refusals `spendByCustomer` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SpendByCustomerRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `spendByCustomer` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function spendByCustomer(client: OrdoClient, params: SpendByCustomerParams): ReadCall<SpendByCustomerRow, SpendByCustomerRefusal, SpendByCustomerFields> {
  return new ReadCall(client, spec_spendByCustomer, params);
}

/**
 * One row of `std.boards.count`.
 */
export interface StdBoardsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.boards.count` takes no parameters. */
export type StdBoardsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.boards.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdBoardsCountFields = Record<never, never>;

/**
 * The refusals `std.boards.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boards$count(client: OrdoClient): SingleReadCall<StdBoardsCountRow, StdBoardsCountRefusal, StdBoardsCountFields> {
  return new SingleReadCall(client, spec_std$boards$count, {});
}

/**
 * The row shape of `std.boards.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boards.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsCreateParams = {
  /** `text` · fills `boards.name` */
  readonly name: string;
};

/**
 * The refusals `std.boards.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$boards$create(client: OrdoClient, params: StdBoardsCreateParams): WriteCall<StdBoardsCreateRow, StdBoardsCreateRefusal> {
  return new WriteCall(client, spec_std$boards$create, params);
}

/**
 * The row shape of `std.boards.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boards.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsDeleteParams = {
  /** `id` */
  readonly id: Ids.BoardsId;
};

/**
 * The refusals `std.boards.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$boards$delete(client: OrdoClient, params: StdBoardsDeleteParams): WriteCall<StdBoardsDeleteRow, StdBoardsDeleteRefusal> {
  return new WriteCall(client, spec_std$boards$delete, params);
}

/**
 * One row of `std.boards.get`.
 *
 * Row identity: `boards`. `synthetic_rows` is false.
 */
export interface StdBoardsGetRow {
  /** `id` */
  readonly id: Ids.BoardsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
}

/**
 * The parameters of `std.boards.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsGetParams = {
  /** `id` */
  readonly id: Ids.BoardsId;
};

/**
 * The fields a `.where` on `std.boards.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdBoardsGetFields {
  readonly id: OpenField<'id', Ids.BoardsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
}

/**
 * The refusals `std.boards.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boards$get(client: OrdoClient, params: StdBoardsGetParams): ReadCall<StdBoardsGetRow, StdBoardsGetRefusal, StdBoardsGetFields> {
  return new ReadCall(client, spec_std$boards$get, params);
}

/**
 * One row of `std.boards.list`.
 *
 * Row identity: `boards`. `synthetic_rows` is false.
 */
export interface StdBoardsListRow {
  /** `id` */
  readonly id: Ids.BoardsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
}

/** `std.boards.list` takes no parameters. */
export type StdBoardsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.boards.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdBoardsListFields {
  readonly id: OpenField<'id', Ids.BoardsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
}

/**
 * The refusals `std.boards.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boards$list(client: OrdoClient): ReadCall<StdBoardsListRow, StdBoardsListRefusal, StdBoardsListFields> {
  return new ReadCall(client, spec_std$boards$list, {});
}

/**
 * The row shape of `std.boards.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boards.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsUpdateParams = {
  /** `id` */
  readonly id: Ids.BoardsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `boards.name` */
  readonly name: string;
};

/**
 * The refusals `std.boards.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boards.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$boards$update(client: OrdoClient, params: StdBoardsUpdateParams): WriteCall<StdBoardsUpdateRow, StdBoardsUpdateRefusal> {
  return new WriteCall(client, spec_std$boards$update, params);
}

/**
 * One row of `std.boardsAccess.count`.
 */
export interface StdBoardsAccessCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.boardsAccess.count` takes no parameters. */
export type StdBoardsAccessCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.boardsAccess.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdBoardsAccessCountFields = Record<never, never>;

/**
 * The refusals `std.boardsAccess.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boardsAccess$count(client: OrdoClient): SingleReadCall<StdBoardsAccessCountRow, StdBoardsAccessCountRefusal, StdBoardsAccessCountFields> {
  return new SingleReadCall(client, spec_std$boardsAccess$count, {});
}

/**
 * The row shape of `std.boardsAccess.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsAccessCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boardsAccess.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsAccessCreateParams = {
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `boardsAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.boardsAccess.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$boardsAccess$create(client: OrdoClient, params: StdBoardsAccessCreateParams): WriteCall<StdBoardsAccessCreateRow, StdBoardsAccessCreateRefusal> {
  return new WriteCall(client, spec_std$boardsAccess$create, params);
}

/**
 * The row shape of `std.boardsAccess.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsAccessDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boardsAccess.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsAccessDeleteParams = {
  /** `id` */
  readonly id: Ids.BoardsAccessId;
};

/**
 * The refusals `std.boardsAccess.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$boardsAccess$delete(client: OrdoClient, params: StdBoardsAccessDeleteParams): WriteCall<StdBoardsAccessDeleteRow, StdBoardsAccessDeleteRefusal> {
  return new WriteCall(client, spec_std$boardsAccess$delete, params);
}

/**
 * One row of `std.boardsAccess.get`.
 *
 * Row identity: `boardsAccess`. `synthetic_rows` is false.
 */
export interface StdBoardsAccessGetRow {
  /** `id` */
  readonly id: Ids.BoardsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly board: Ids.BoardsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/**
 * The parameters of `std.boardsAccess.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsAccessGetParams = {
  /** `id` */
  readonly id: Ids.BoardsAccessId;
};

/**
 * The fields a `.where` on `std.boardsAccess.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdBoardsAccessGetFields {
  readonly id: OpenField<'id', Ids.BoardsAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly board: OpenField<'board', Ids.BoardsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.boardsAccess.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boardsAccess$get(client: OrdoClient, params: StdBoardsAccessGetParams): ReadCall<StdBoardsAccessGetRow, StdBoardsAccessGetRefusal, StdBoardsAccessGetFields> {
  return new ReadCall(client, spec_std$boardsAccess$get, params);
}

/**
 * One row of `std.boardsAccess.list`.
 *
 * Row identity: `boardsAccess`. `synthetic_rows` is false.
 */
export interface StdBoardsAccessListRow {
  /** `id` */
  readonly id: Ids.BoardsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly board: Ids.BoardsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/** `std.boardsAccess.list` takes no parameters. */
export type StdBoardsAccessListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.boardsAccess.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdBoardsAccessListFields {
  readonly id: OpenField<'id', Ids.BoardsAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly board: OpenField<'board', Ids.BoardsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.boardsAccess.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$boardsAccess$list(client: OrdoClient): ReadCall<StdBoardsAccessListRow, StdBoardsAccessListRefusal, StdBoardsAccessListFields> {
  return new ReadCall(client, spec_std$boardsAccess$list, {});
}

/**
 * The row shape of `std.boardsAccess.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsAccessPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boardsAccess.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsAccessPutParams = {
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `boardsAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.boardsAccess.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$boardsAccess$put(client: OrdoClient, params: StdBoardsAccessPutParams): WriteCall<StdBoardsAccessPutRow, StdBoardsAccessPutRefusal> {
  return new WriteCall(client, spec_std$boardsAccess$put, params);
}

/**
 * The row shape of `std.boardsAccess.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdBoardsAccessUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.boardsAccess.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdBoardsAccessUpdateParams = {
  /** `id` */
  readonly id: Ids.BoardsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `boardsAccess.board` */
  readonly board: Ids.BoardsId;
  /** `id` · fills `boardsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `boardsAccess.level` */
  readonly level: 'admin' | 'read' | 'write';
};

/**
 * The refusals `std.boardsAccess.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdBoardsAccessUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.boardsAccess.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$boardsAccess$update(client: OrdoClient, params: StdBoardsAccessUpdateParams): WriteCall<StdBoardsAccessUpdateRow, StdBoardsAccessUpdateRefusal> {
  return new WriteCall(client, spec_std$boardsAccess$update, params);
}

/**
 * One row of `std.cards.count`.
 */
export interface StdCardsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.cards.count` takes no parameters. */
export type StdCardsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.cards.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdCardsCountFields = Record<never, never>;

/**
 * The refusals `std.cards.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$cards$count(client: OrdoClient): SingleReadCall<StdCardsCountRow, StdCardsCountRefusal, StdCardsCountFields> {
  return new SingleReadCall(client, spec_std$cards$count, {});
}

/**
 * The row shape of `std.cards.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCardsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.cards.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCardsCreateParams = {
  /** `id` · fills `cards.board` */
  readonly board: Ids.BoardsId;
  /** `text` · fills `cards.title` */
  readonly title: string;
  /** `bool` · fills `cards.done` · has a default; omit to take it */
  readonly done?: boolean;
};

/**
 * The refusals `std.cards.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$cards$create(client: OrdoClient, params: StdCardsCreateParams): WriteCall<StdCardsCreateRow, StdCardsCreateRefusal> {
  return new WriteCall(client, spec_std$cards$create, params);
}

/**
 * The row shape of `std.cards.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCardsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.cards.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCardsDeleteParams = {
  /** `id` */
  readonly id: Ids.CardsId;
};

/**
 * The refusals `std.cards.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$cards$delete(client: OrdoClient, params: StdCardsDeleteParams): WriteCall<StdCardsDeleteRow, StdCardsDeleteRefusal> {
  return new WriteCall(client, spec_std$cards$delete, params);
}

/**
 * One row of `std.cards.get`.
 *
 * Row identity: `cards`. `synthetic_rows` is false.
 */
export interface StdCardsGetRow {
  /** `id` */
  readonly id: Ids.CardsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly board: Ids.BoardsId;
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
}

/**
 * The parameters of `std.cards.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCardsGetParams = {
  /** `id` */
  readonly id: Ids.CardsId;
};

/**
 * The fields a `.where` on `std.cards.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCardsGetFields {
  readonly id: OpenField<'id', Ids.CardsId>;
  readonly rev: OpenField<'rev', number>;
  readonly board: OpenField<'board', Ids.BoardsId>;
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
}

/**
 * The refusals `std.cards.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$cards$get(client: OrdoClient, params: StdCardsGetParams): ReadCall<StdCardsGetRow, StdCardsGetRefusal, StdCardsGetFields> {
  return new ReadCall(client, spec_std$cards$get, params);
}

/**
 * One row of `std.cards.list`.
 *
 * Row identity: `cards`. `synthetic_rows` is false.
 */
export interface StdCardsListRow {
  /** `id` */
  readonly id: Ids.CardsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly board: Ids.BoardsId;
  /** `text` */
  readonly title: string;
  /** `bool` */
  readonly done: boolean;
}

/** `std.cards.list` takes no parameters. */
export type StdCardsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.cards.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCardsListFields {
  readonly id: OpenField<'id', Ids.CardsId>;
  readonly rev: OpenField<'rev', number>;
  readonly board: OpenField<'board', Ids.BoardsId>;
  readonly title: OpenField<'title', string>;
  readonly done: OpenField<'done', boolean>;
}

/**
 * The refusals `std.cards.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$cards$list(client: OrdoClient): ReadCall<StdCardsListRow, StdCardsListRefusal, StdCardsListFields> {
  return new ReadCall(client, spec_std$cards$list, {});
}

/**
 * The row shape of `std.cards.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCardsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.cards.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCardsUpdateParams = {
  /** `id` */
  readonly id: Ids.CardsId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `cards.board` */
  readonly board: Ids.BoardsId;
  /** `text` · fills `cards.title` */
  readonly title: string;
  /** `bool` · fills `cards.done` */
  readonly done: boolean;
};

/**
 * The refusals `std.cards.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCardsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.cards.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$cards$update(client: OrdoClient, params: StdCardsUpdateParams): WriteCall<StdCardsUpdateRow, StdCardsUpdateRefusal> {
  return new WriteCall(client, spec_std$cards$update, params);
}

/**
 * One row of `std.categories.count`.
 */
export interface StdCategoriesCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.categories.count` takes no parameters. */
export type StdCategoriesCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.categories.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdCategoriesCountFields = Record<never, never>;

/**
 * The refusals `std.categories.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$categories$count(client: OrdoClient): SingleReadCall<StdCategoriesCountRow, StdCategoriesCountRefusal, StdCategoriesCountFields> {
  return new SingleReadCall(client, spec_std$categories$count, {});
}

/**
 * The row shape of `std.categories.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCategoriesCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.categories.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesCreateParams = {
  /** `text` · fills `categories.name` */
  readonly name: string;
  /** `id` · fills `categories.parent` · has a default; omit it or pass null to take it */
  readonly parent?: Ids.CategoriesId | null;
};

/**
 * The refusals `std.categories.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$categories$create(client: OrdoClient, params: StdCategoriesCreateParams): WriteCall<StdCategoriesCreateRow, StdCategoriesCreateRefusal> {
  return new WriteCall(client, spec_std$categories$create, params);
}

/**
 * The row shape of `std.categories.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCategoriesDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.categories.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesDeleteParams = {
  /** `id` */
  readonly id: Ids.CategoriesId;
};

/**
 * The refusals `std.categories.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$categories$delete(client: OrdoClient, params: StdCategoriesDeleteParams): WriteCall<StdCategoriesDeleteRow, StdCategoriesDeleteRefusal> {
  return new WriteCall(client, spec_std$categories$delete, params);
}

/**
 * The row shape of `std.categories.deleteSubtree`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCategoriesDeleteSubtreeRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.categories.deleteSubtree`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesDeleteSubtreeParams = {
  /** `id` */
  readonly id: Ids.CategoriesId;
};

/**
 * The refusals `std.categories.deleteSubtree` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesDeleteSubtreeRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.deleteSubtree` — delete.
 *
 * Mode: **deletes subtree**.
 * Terminates in `.commit()`.
 */
export function std$categories$deleteSubtree(client: OrdoClient, params: StdCategoriesDeleteSubtreeParams): WriteCall<StdCategoriesDeleteSubtreeRow, StdCategoriesDeleteSubtreeRefusal> {
  return new WriteCall(client, spec_std$categories$deleteSubtree, params);
}

/**
 * One row of `std.categories.get`.
 *
 * Row identity: `categories`. `synthetic_rows` is false.
 */
export interface StdCategoriesGetRow {
  /** `id` */
  readonly id: Ids.CategoriesId;
  /** `int` */
  readonly rev: number;
  /** `int` */
  readonly depth: number;
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly parent: Ids.CategoriesId;
}

/**
 * The parameters of `std.categories.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesGetParams = {
  /** `id` */
  readonly id: Ids.CategoriesId;
};

/**
 * The fields a `.where` on `std.categories.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCategoriesGetFields {
  readonly id: OpenField<'id', Ids.CategoriesId>;
  readonly rev: OpenField<'rev', number>;
  readonly depth: OpenField<'depth', number>;
  readonly name: OpenField<'name', string>;
  readonly parent: OpenField<'parent', Ids.CategoriesId>;
}

/**
 * The refusals `std.categories.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$categories$get(client: OrdoClient, params: StdCategoriesGetParams): ReadCall<StdCategoriesGetRow, StdCategoriesGetRefusal, StdCategoriesGetFields> {
  return new ReadCall(client, spec_std$categories$get, params);
}

/**
 * One row of `std.categories.list`.
 *
 * Row identity: `categories`. `synthetic_rows` is false.
 */
export interface StdCategoriesListRow {
  /** `id` */
  readonly id: Ids.CategoriesId;
  /** `int` */
  readonly rev: number;
  /** `int` */
  readonly depth: number;
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly parent: Ids.CategoriesId;
}

/** `std.categories.list` takes no parameters. */
export type StdCategoriesListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.categories.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCategoriesListFields {
  readonly id: OpenField<'id', Ids.CategoriesId>;
  readonly rev: OpenField<'rev', number>;
  readonly depth: OpenField<'depth', number>;
  readonly name: OpenField<'name', string>;
  readonly parent: OpenField<'parent', Ids.CategoriesId>;
}

/**
 * The refusals `std.categories.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$categories$list(client: OrdoClient): ReadCall<StdCategoriesListRow, StdCategoriesListRefusal, StdCategoriesListFields> {
  return new ReadCall(client, spec_std$categories$list, {});
}

/**
 * The row shape of `std.categories.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCategoriesPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.categories.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesPutParams = {
  /** `text` · fills `categories.name` */
  readonly name: string;
  /** `id` · fills `categories.parent` · always written: pass the current value to keep it, or null to write the default (null clears it when the default is null) */
  readonly parent: Ids.CategoriesId | null;
};

/**
 * The refusals `std.categories.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$categories$put(client: OrdoClient, params: StdCategoriesPutParams): WriteCall<StdCategoriesPutRow, StdCategoriesPutRefusal> {
  return new WriteCall(client, spec_std$categories$put, params);
}

/**
 * The row shape of `std.categories.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCategoriesUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.categories.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCategoriesUpdateParams = {
  /** `id` */
  readonly id: Ids.CategoriesId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `categories.name` */
  readonly name: string;
  /** `id` · fills `categories.parent` */
  readonly parent: Ids.CategoriesId;
};

/**
 * The refusals `std.categories.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCategoriesUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.categories.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$categories$update(client: OrdoClient, params: StdCategoriesUpdateParams): WriteCall<StdCategoriesUpdateRow, StdCategoriesUpdateRefusal> {
  return new WriteCall(client, spec_std$categories$update, params);
}

/**
 * One row of `std.coupons.count`.
 */
export interface StdCouponsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.coupons.count` takes no parameters. */
export type StdCouponsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.coupons.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdCouponsCountFields = Record<never, never>;

/**
 * The refusals `std.coupons.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCouponsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.coupons.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$coupons$count(client: OrdoClient): SingleReadCall<StdCouponsCountRow, StdCouponsCountRefusal, StdCouponsCountFields> {
  return new SingleReadCall(client, spec_std$coupons$count, {});
}

/**
 * The row shape of `std.coupons.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCouponsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.coupons.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCouponsDeleteParams = {
  /** `id` */
  readonly id: Ids.CouponsId;
};

/**
 * The refusals `std.coupons.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCouponsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.coupons.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$coupons$delete(client: OrdoClient, params: StdCouponsDeleteParams): WriteCall<StdCouponsDeleteRow, StdCouponsDeleteRefusal> {
  return new WriteCall(client, spec_std$coupons$delete, params);
}

/**
 * One row of `std.coupons.get`.
 *
 * Row identity: `coupons`. `synthetic_rows` is false.
 */
export interface StdCouponsGetRow {
  /** `id` */
  readonly id: Ids.CouponsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly code: string;
  /** `text` */
  readonly slug: string;
  /** `money` */
  readonly value: string;
}

/**
 * The parameters of `std.coupons.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCouponsGetParams = {
  /** `id` */
  readonly id: Ids.CouponsId;
};

/**
 * The fields a `.where` on `std.coupons.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCouponsGetFields {
  readonly id: OpenField<'id', Ids.CouponsId>;
  readonly rev: OpenField<'rev', number>;
  readonly code: OpenField<'code', string>;
  readonly slug: OpenField<'slug', string>;
  readonly value: OpenField<'value', string>;
}

/**
 * The refusals `std.coupons.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCouponsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.coupons.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$coupons$get(client: OrdoClient, params: StdCouponsGetParams): ReadCall<StdCouponsGetRow, StdCouponsGetRefusal, StdCouponsGetFields> {
  return new ReadCall(client, spec_std$coupons$get, params);
}

/**
 * One row of `std.coupons.list`.
 *
 * Row identity: `coupons`. `synthetic_rows` is false.
 */
export interface StdCouponsListRow {
  /** `id` */
  readonly id: Ids.CouponsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly code: string;
  /** `text` */
  readonly slug: string;
  /** `money` */
  readonly value: string;
}

/** `std.coupons.list` takes no parameters. */
export type StdCouponsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.coupons.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCouponsListFields {
  readonly id: OpenField<'id', Ids.CouponsId>;
  readonly rev: OpenField<'rev', number>;
  readonly code: OpenField<'code', string>;
  readonly slug: OpenField<'slug', string>;
  readonly value: OpenField<'value', string>;
}

/**
 * The refusals `std.coupons.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCouponsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.coupons.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$coupons$list(client: OrdoClient): ReadCall<StdCouponsListRow, StdCouponsListRefusal, StdCouponsListFields> {
  return new ReadCall(client, spec_std$coupons$list, {});
}

/**
 * The row shape of `std.coupons.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCouponsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.coupons.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCouponsUpdateParams = {
  /** `id` */
  readonly id: Ids.CouponsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `coupons.code` */
  readonly code: string;
  /** `text` · fills `coupons.slug` */
  readonly slug: string;
  /** `money` · fills `coupons.value` */
  readonly value: string;
};

/**
 * The refusals `std.coupons.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCouponsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.coupons.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$coupons$update(client: OrdoClient, params: StdCouponsUpdateParams): WriteCall<StdCouponsUpdateRow, StdCouponsUpdateRefusal> {
  return new WriteCall(client, spec_std$coupons$update, params);
}

/**
 * One row of `std.customers.count`.
 */
export interface StdCustomersCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.customers.count` takes no parameters. */
export type StdCustomersCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.customers.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdCustomersCountFields = Record<never, never>;

/**
 * The refusals `std.customers.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$customers$count(client: OrdoClient): SingleReadCall<StdCustomersCountRow, StdCustomersCountRefusal, StdCustomersCountFields> {
  return new SingleReadCall(client, spec_std$customers$count, {});
}

/**
 * The row shape of `std.customers.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCustomersCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.customers.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCustomersCreateParams = {
  /** `text` · fills `customers.name` */
  readonly name: string;
  /** `text` · fills `customers.email` · has a default; omit it or pass null to take it */
  readonly email?: string | null;
  /** `text` · fills `customers.tier` · has a default; omit to take it */
  readonly tier?: string;
};

/**
 * The refusals `std.customers.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$customers$create(client: OrdoClient, params: StdCustomersCreateParams): WriteCall<StdCustomersCreateRow, StdCustomersCreateRefusal> {
  return new WriteCall(client, spec_std$customers$create, params);
}

/**
 * The row shape of `std.customers.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCustomersDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.customers.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCustomersDeleteParams = {
  /** `id` */
  readonly id: Ids.CustomersId;
};

/**
 * The refusals `std.customers.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$customers$delete(client: OrdoClient, params: StdCustomersDeleteParams): WriteCall<StdCustomersDeleteRow, StdCustomersDeleteRefusal> {
  return new WriteCall(client, spec_std$customers$delete, params);
}

/**
 * One row of `std.customers.get`.
 *
 * Row identity: `customers`. `synthetic_rows` is false.
 */
export interface StdCustomersGetRow {
  /** `id` */
  readonly id: Ids.CustomersId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly tier: string;
}

/**
 * The parameters of `std.customers.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCustomersGetParams = {
  /** `id` */
  readonly id: Ids.CustomersId;
};

/**
 * The fields a `.where` on `std.customers.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCustomersGetFields {
  readonly id: OpenField<'id', Ids.CustomersId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly tier: OpenField<'tier', string>;
}

/**
 * The refusals `std.customers.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$customers$get(client: OrdoClient, params: StdCustomersGetParams): ReadCall<StdCustomersGetRow, StdCustomersGetRefusal, StdCustomersGetFields> {
  return new ReadCall(client, spec_std$customers$get, params);
}

/**
 * One row of `std.customers.list`.
 *
 * Row identity: `customers`. `synthetic_rows` is false.
 */
export interface StdCustomersListRow {
  /** `id` */
  readonly id: Ids.CustomersId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly tier: string;
}

/** `std.customers.list` takes no parameters. */
export type StdCustomersListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.customers.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdCustomersListFields {
  readonly id: OpenField<'id', Ids.CustomersId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly tier: OpenField<'tier', string>;
}

/**
 * The refusals `std.customers.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$customers$list(client: OrdoClient): ReadCall<StdCustomersListRow, StdCustomersListRefusal, StdCustomersListFields> {
  return new ReadCall(client, spec_std$customers$list, {});
}

/**
 * The row shape of `std.customers.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCustomersPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.customers.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCustomersPutParams = {
  /** `text` · fills `customers.name` */
  readonly name: string;
  /** `text` · fills `customers.email` · always written: pass the current value to keep it, or null to write the default (null clears it when the default is null) */
  readonly email: string | null;
  /** `text` · fills `customers.tier` · has a default; omit to take it */
  readonly tier?: string;
};

/**
 * The refusals `std.customers.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$customers$put(client: OrdoClient, params: StdCustomersPutParams): WriteCall<StdCustomersPutRow, StdCustomersPutRefusal> {
  return new WriteCall(client, spec_std$customers$put, params);
}

/**
 * The row shape of `std.customers.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdCustomersUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.customers.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdCustomersUpdateParams = {
  /** `id` */
  readonly id: Ids.CustomersId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `customers.name` */
  readonly name: string;
  /** `text` · fills `customers.tier` */
  readonly tier: string;
};

/**
 * The refusals `std.customers.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdCustomersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.customers.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$customers$update(client: OrdoClient, params: StdCustomersUpdateParams): WriteCall<StdCustomersUpdateRow, StdCustomersUpdateRefusal> {
  return new WriteCall(client, spec_std$customers$update, params);
}

/**
 * One row of `std.folders.count`.
 */
export interface StdFoldersCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.folders.count` takes no parameters. */
export type StdFoldersCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.folders.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdFoldersCountFields = Record<never, never>;

/**
 * The refusals `std.folders.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$folders$count(client: OrdoClient): SingleReadCall<StdFoldersCountRow, StdFoldersCountRefusal, StdFoldersCountFields> {
  return new SingleReadCall(client, spec_std$folders$count, {});
}

/**
 * The row shape of `std.folders.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdFoldersCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.folders.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdFoldersCreateParams = {
  /** `id` · fills `folders.owner` */
  readonly owner: Ids.MembersId;
  /** `text` · fills `folders.name` */
  readonly name: string;
  /** `id` · fills `folders.parent` · has a default; omit it or pass null to take it */
  readonly parent?: Ids.FoldersId | null;
};

/**
 * The refusals `std.folders.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$folders$create(client: OrdoClient, params: StdFoldersCreateParams): WriteCall<StdFoldersCreateRow, StdFoldersCreateRefusal> {
  return new WriteCall(client, spec_std$folders$create, params);
}

/**
 * The row shape of `std.folders.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdFoldersDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.folders.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdFoldersDeleteParams = {
  /** `id` */
  readonly id: Ids.FoldersId;
};

/**
 * The refusals `std.folders.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$folders$delete(client: OrdoClient, params: StdFoldersDeleteParams): WriteCall<StdFoldersDeleteRow, StdFoldersDeleteRefusal> {
  return new WriteCall(client, spec_std$folders$delete, params);
}

/**
 * The row shape of `std.folders.deleteSubtree`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdFoldersDeleteSubtreeRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.folders.deleteSubtree`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdFoldersDeleteSubtreeParams = {
  /** `id` */
  readonly id: Ids.FoldersId;
};

/**
 * The refusals `std.folders.deleteSubtree` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersDeleteSubtreeRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'tree_has_children' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.deleteSubtree` — delete.
 *
 * Mode: **deletes subtree**.
 * Terminates in `.commit()`.
 */
export function std$folders$deleteSubtree(client: OrdoClient, params: StdFoldersDeleteSubtreeParams): WriteCall<StdFoldersDeleteSubtreeRow, StdFoldersDeleteSubtreeRefusal> {
  return new WriteCall(client, spec_std$folders$deleteSubtree, params);
}

/**
 * One row of `std.folders.get`.
 *
 * Row identity: `folders`. `synthetic_rows` is false.
 */
export interface StdFoldersGetRow {
  /** `id` */
  readonly id: Ids.FoldersId;
  /** `int` */
  readonly rev: number;
  /** `int` */
  readonly depth: number;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly parent: Ids.FoldersId;
}

/**
 * The parameters of `std.folders.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdFoldersGetParams = {
  /** `id` */
  readonly id: Ids.FoldersId;
};

/**
 * The fields a `.where` on `std.folders.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdFoldersGetFields {
  readonly id: OpenField<'id', Ids.FoldersId>;
  readonly rev: OpenField<'rev', number>;
  readonly depth: OpenField<'depth', number>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly name: OpenField<'name', string>;
  readonly parent: OpenField<'parent', Ids.FoldersId>;
}

/**
 * The refusals `std.folders.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$folders$get(client: OrdoClient, params: StdFoldersGetParams): ReadCall<StdFoldersGetRow, StdFoldersGetRefusal, StdFoldersGetFields> {
  return new ReadCall(client, spec_std$folders$get, params);
}

/**
 * One row of `std.folders.list`.
 *
 * Row identity: `folders`. `synthetic_rows` is false.
 */
export interface StdFoldersListRow {
  /** `id` */
  readonly id: Ids.FoldersId;
  /** `int` */
  readonly rev: number;
  /** `int` */
  readonly depth: number;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `text` */
  readonly name: string;
  /** `id` */
  readonly parent: Ids.FoldersId;
}

/** `std.folders.list` takes no parameters. */
export type StdFoldersListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.folders.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdFoldersListFields {
  readonly id: OpenField<'id', Ids.FoldersId>;
  readonly rev: OpenField<'rev', number>;
  readonly depth: OpenField<'depth', number>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly name: OpenField<'name', string>;
  readonly parent: OpenField<'parent', Ids.FoldersId>;
}

/**
 * The refusals `std.folders.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$folders$list(client: OrdoClient): ReadCall<StdFoldersListRow, StdFoldersListRefusal, StdFoldersListFields> {
  return new ReadCall(client, spec_std$folders$list, {});
}

/**
 * The row shape of `std.folders.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdFoldersUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.folders.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdFoldersUpdateParams = {
  /** `id` */
  readonly id: Ids.FoldersId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `folders.owner` */
  readonly owner: Ids.MembersId;
  /** `text` · fills `folders.name` */
  readonly name: string;
  /** `id` · fills `folders.parent` */
  readonly parent: Ids.FoldersId;
};

/**
 * The refusals `std.folders.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdFoldersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'tree_cycle' | 'tree_depth_exceeded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.folders.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$folders$update(client: OrdoClient, params: StdFoldersUpdateParams): WriteCall<StdFoldersUpdateRow, StdFoldersUpdateRefusal> {
  return new WriteCall(client, spec_std$folders$update, params);
}

/**
 * One row of `std.ledgerEntries.count`.
 */
export interface StdLedgerEntriesCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.ledgerEntries.count` takes no parameters. */
export type StdLedgerEntriesCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.ledgerEntries.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdLedgerEntriesCountFields = Record<never, never>;

/**
 * The refusals `std.ledgerEntries.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$ledgerEntries$count(client: OrdoClient): SingleReadCall<StdLedgerEntriesCountRow, StdLedgerEntriesCountRefusal, StdLedgerEntriesCountFields> {
  return new SingleReadCall(client, spec_std$ledgerEntries$count, {});
}

/**
 * The row shape of `std.ledgerEntries.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdLedgerEntriesCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.ledgerEntries.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdLedgerEntriesCreateParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly externalId: string;
  /** `id` · fills `ledgerEntries.owner` */
  readonly owner: Ids.MembersId;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
  /** `text` · fills `ledgerEntries.note` · has a default; omit to take it */
  readonly note?: string;
};

/**
 * The refusals `std.ledgerEntries.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$ledgerEntries$create(client: OrdoClient, params: StdLedgerEntriesCreateParams): WriteCall<StdLedgerEntriesCreateRow, StdLedgerEntriesCreateRefusal> {
  return new WriteCall(client, spec_std$ledgerEntries$create, params);
}

/**
 * The row shape of `std.ledgerEntries.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdLedgerEntriesDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.ledgerEntries.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdLedgerEntriesDeleteParams = {
  /** `id` */
  readonly id: Ids.LedgerEntriesId;
};

/**
 * The refusals `std.ledgerEntries.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$ledgerEntries$delete(client: OrdoClient, params: StdLedgerEntriesDeleteParams): WriteCall<StdLedgerEntriesDeleteRow, StdLedgerEntriesDeleteRefusal> {
  return new WriteCall(client, spec_std$ledgerEntries$delete, params);
}

/**
 * One row of `std.ledgerEntries.get`.
 *
 * Row identity: `ledgerEntries`. `synthetic_rows` is false.
 */
export interface StdLedgerEntriesGetRow {
  /** `id` */
  readonly id: Ids.LedgerEntriesId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly externalId: string;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `money` */
  readonly amount: string;
  /** `text` */
  readonly note: string;
}

/**
 * The parameters of `std.ledgerEntries.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdLedgerEntriesGetParams = {
  /** `id` */
  readonly id: Ids.LedgerEntriesId;
};

/**
 * The fields a `.where` on `std.ledgerEntries.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdLedgerEntriesGetFields {
  readonly id: OpenField<'id', Ids.LedgerEntriesId>;
  readonly rev: OpenField<'rev', number>;
  readonly externalId: OpenField<'externalId', string>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly amount: OpenField<'amount', string>;
  readonly note: OpenField<'note', string>;
}

/**
 * The refusals `std.ledgerEntries.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$ledgerEntries$get(client: OrdoClient, params: StdLedgerEntriesGetParams): ReadCall<StdLedgerEntriesGetRow, StdLedgerEntriesGetRefusal, StdLedgerEntriesGetFields> {
  return new ReadCall(client, spec_std$ledgerEntries$get, params);
}

/**
 * One row of `std.ledgerEntries.list`.
 *
 * Row identity: `ledgerEntries`. `synthetic_rows` is false.
 */
export interface StdLedgerEntriesListRow {
  /** `id` */
  readonly id: Ids.LedgerEntriesId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly externalId: string;
  /** `id` */
  readonly owner: Ids.MembersId;
  /** `money` */
  readonly amount: string;
  /** `text` */
  readonly note: string;
}

/** `std.ledgerEntries.list` takes no parameters. */
export type StdLedgerEntriesListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.ledgerEntries.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdLedgerEntriesListFields {
  readonly id: OpenField<'id', Ids.LedgerEntriesId>;
  readonly rev: OpenField<'rev', number>;
  readonly externalId: OpenField<'externalId', string>;
  readonly owner: OpenField<'owner', Ids.MembersId>;
  readonly amount: OpenField<'amount', string>;
  readonly note: OpenField<'note', string>;
}

/**
 * The refusals `std.ledgerEntries.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$ledgerEntries$list(client: OrdoClient): ReadCall<StdLedgerEntriesListRow, StdLedgerEntriesListRefusal, StdLedgerEntriesListFields> {
  return new ReadCall(client, spec_std$ledgerEntries$list, {});
}

/**
 * The row shape of `std.ledgerEntries.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdLedgerEntriesPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.ledgerEntries.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdLedgerEntriesPutParams = {
  /** `text` · fills `ledgerEntries.externalId` */
  readonly externalId: string;
  /** `id` · fills `ledgerEntries.owner` */
  readonly owner: Ids.MembersId;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
  /** `text` · fills `ledgerEntries.note` · has a default; omit to take it */
  readonly note?: string;
};

/**
 * The refusals `std.ledgerEntries.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$ledgerEntries$put(client: OrdoClient, params: StdLedgerEntriesPutParams): WriteCall<StdLedgerEntriesPutRow, StdLedgerEntriesPutRefusal> {
  return new WriteCall(client, spec_std$ledgerEntries$put, params);
}

/**
 * The row shape of `std.ledgerEntries.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdLedgerEntriesUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.ledgerEntries.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdLedgerEntriesUpdateParams = {
  /** `id` */
  readonly id: Ids.LedgerEntriesId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `ledgerEntries.externalId` */
  readonly externalId: string;
  /** `id` · fills `ledgerEntries.owner` */
  readonly owner: Ids.MembersId;
  /** `money` · fills `ledgerEntries.amount` */
  readonly amount: string;
  /** `text` · fills `ledgerEntries.note` */
  readonly note: string;
};

/**
 * The refusals `std.ledgerEntries.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdLedgerEntriesUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.ledgerEntries.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$ledgerEntries$update(client: OrdoClient, params: StdLedgerEntriesUpdateParams): WriteCall<StdLedgerEntriesUpdateRow, StdLedgerEntriesUpdateRefusal> {
  return new WriteCall(client, spec_std$ledgerEntries$update, params);
}

/**
 * One row of `std.orderItems.count`.
 */
export interface StdOrderItemsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.orderItems.count` takes no parameters. */
export type StdOrderItemsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.orderItems.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdOrderItemsCountFields = Record<never, never>;

/**
 * The refusals `std.orderItems.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orderItems$count(client: OrdoClient): SingleReadCall<StdOrderItemsCountRow, StdOrderItemsCountRefusal, StdOrderItemsCountFields> {
  return new SingleReadCall(client, spec_std$orderItems$count, {});
}

/**
 * The row shape of `std.orderItems.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrderItemsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orderItems.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrderItemsCreateParams = {
  /** `id` · fills `orderItems.order` */
  readonly order: Ids.OrdersId;
  /** `id` · fills `orderItems.product` */
  readonly product: Ids.ProductsId;
  /** `int` · fills `orderItems.qty` */
  readonly qty: number;
  /** `money` · fills `orderItems.amount` */
  readonly amount: string;
};

/**
 * The refusals `std.orderItems.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$orderItems$create(client: OrdoClient, params: StdOrderItemsCreateParams): WriteCall<StdOrderItemsCreateRow, StdOrderItemsCreateRefusal> {
  return new WriteCall(client, spec_std$orderItems$create, params);
}

/**
 * The row shape of `std.orderItems.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrderItemsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orderItems.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrderItemsDeleteParams = {
  /** `id` */
  readonly id: Ids.OrderItemsId;
};

/**
 * The refusals `std.orderItems.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$orderItems$delete(client: OrdoClient, params: StdOrderItemsDeleteParams): WriteCall<StdOrderItemsDeleteRow, StdOrderItemsDeleteRefusal> {
  return new WriteCall(client, spec_std$orderItems$delete, params);
}

/**
 * One row of `std.orderItems.get`.
 *
 * Row identity: `orderItems`. `synthetic_rows` is false.
 */
export interface StdOrderItemsGetRow {
  /** `id` */
  readonly id: Ids.OrderItemsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly order: Ids.OrdersId;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly qty: number;
  /** `money` */
  readonly amount: string;
}

/**
 * The parameters of `std.orderItems.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrderItemsGetParams = {
  /** `id` */
  readonly id: Ids.OrderItemsId;
};

/**
 * The fields a `.where` on `std.orderItems.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdOrderItemsGetFields {
  readonly id: OpenField<'id', Ids.OrderItemsId>;
  readonly rev: OpenField<'rev', number>;
  readonly order: OpenField<'order', Ids.OrdersId>;
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly qty: OpenField<'qty', number>;
  readonly amount: OpenField<'amount', string>;
}

/**
 * The refusals `std.orderItems.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orderItems$get(client: OrdoClient, params: StdOrderItemsGetParams): ReadCall<StdOrderItemsGetRow, StdOrderItemsGetRefusal, StdOrderItemsGetFields> {
  return new ReadCall(client, spec_std$orderItems$get, params);
}

/**
 * One row of `std.orderItems.list`.
 *
 * Row identity: `orderItems`. `synthetic_rows` is false.
 */
export interface StdOrderItemsListRow {
  /** `id` */
  readonly id: Ids.OrderItemsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly order: Ids.OrdersId;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly qty: number;
  /** `money` */
  readonly amount: string;
}

/** `std.orderItems.list` takes no parameters. */
export type StdOrderItemsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.orderItems.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdOrderItemsListFields {
  readonly id: OpenField<'id', Ids.OrderItemsId>;
  readonly rev: OpenField<'rev', number>;
  readonly order: OpenField<'order', Ids.OrdersId>;
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly qty: OpenField<'qty', number>;
  readonly amount: OpenField<'amount', string>;
}

/**
 * The refusals `std.orderItems.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orderItems$list(client: OrdoClient): ReadCall<StdOrderItemsListRow, StdOrderItemsListRefusal, StdOrderItemsListFields> {
  return new ReadCall(client, spec_std$orderItems$list, {});
}

/**
 * The row shape of `std.orderItems.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrderItemsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orderItems.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrderItemsUpdateParams = {
  /** `id` */
  readonly id: Ids.OrderItemsId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `orderItems.order` */
  readonly order: Ids.OrdersId;
  /** `id` · fills `orderItems.product` */
  readonly product: Ids.ProductsId;
  /** `int` · fills `orderItems.qty` */
  readonly qty: number;
  /** `money` · fills `orderItems.amount` */
  readonly amount: string;
};

/**
 * The refusals `std.orderItems.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrderItemsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orderItems.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$orderItems$update(client: OrdoClient, params: StdOrderItemsUpdateParams): WriteCall<StdOrderItemsUpdateRow, StdOrderItemsUpdateRefusal> {
  return new WriteCall(client, spec_std$orderItems$update, params);
}

/**
 * One row of `std.orders.count`.
 */
export interface StdOrdersCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.orders.count` takes no parameters. */
export type StdOrdersCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.orders.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdOrdersCountFields = Record<never, never>;

/**
 * The refusals `std.orders.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orders$count(client: OrdoClient): SingleReadCall<StdOrdersCountRow, StdOrdersCountRefusal, StdOrdersCountFields> {
  return new SingleReadCall(client, spec_std$orders$count, {});
}

/**
 * The row shape of `std.orders.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrdersCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orders.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrdersCreateParams = {
  /** `id` · fills `orders.customer` */
  readonly customer: Ids.CustomersId;
  /** `date` · fills `orders.placed` */
  readonly placed: string;
  /** `text` · fills `orders.status` · has a default; omit to take it */
  readonly status?: string;
  /** `money` · fills `orders.total` */
  readonly total: string;
  /** `text` · fills `orders.memo` · has a default; omit to take it */
  readonly memo?: string;
};

/**
 * The refusals `std.orders.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$orders$create(client: OrdoClient, params: StdOrdersCreateParams): WriteCall<StdOrdersCreateRow, StdOrdersCreateRefusal> {
  return new WriteCall(client, spec_std$orders$create, params);
}

/**
 * The row shape of `std.orders.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrdersDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orders.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrdersDeleteParams = {
  /** `id` */
  readonly id: Ids.OrdersId;
};

/**
 * The refusals `std.orders.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$orders$delete(client: OrdoClient, params: StdOrdersDeleteParams): WriteCall<StdOrdersDeleteRow, StdOrdersDeleteRefusal> {
  return new WriteCall(client, spec_std$orders$delete, params);
}

/**
 * One row of `std.orders.get`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface StdOrdersGetRow {
  /** `id` */
  readonly id: Ids.OrdersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly customer: Ids.CustomersId;
  /** `date` */
  readonly placed: string;
  /** `text` */
  readonly status: string;
  /** `money` */
  readonly total: string;
}

/**
 * The parameters of `std.orders.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrdersGetParams = {
  /** `id` */
  readonly id: Ids.OrdersId;
};

/**
 * The fields a `.where` on `std.orders.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdOrdersGetFields {
  readonly id: OpenField<'id', Ids.OrdersId>;
  readonly rev: OpenField<'rev', number>;
  readonly customer: OpenField<'customer', Ids.CustomersId>;
  readonly placed: OpenField<'placed', string>;
  readonly status: OpenField<'status', string>;
  readonly total: OpenField<'total', string>;
}

/**
 * The refusals `std.orders.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orders$get(client: OrdoClient, params: StdOrdersGetParams): ReadCall<StdOrdersGetRow, StdOrdersGetRefusal, StdOrdersGetFields> {
  return new ReadCall(client, spec_std$orders$get, params);
}

/**
 * One row of `std.orders.list`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface StdOrdersListRow {
  /** `id` */
  readonly id: Ids.OrdersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly customer: Ids.CustomersId;
  /** `date` */
  readonly placed: string;
  /** `text` */
  readonly status: string;
  /** `money` */
  readonly total: string;
}

/** `std.orders.list` takes no parameters. */
export type StdOrdersListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.orders.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdOrdersListFields {
  readonly id: OpenField<'id', Ids.OrdersId>;
  readonly rev: OpenField<'rev', number>;
  readonly customer: OpenField<'customer', Ids.CustomersId>;
  readonly placed: OpenField<'placed', string>;
  readonly status: OpenField<'status', string>;
  readonly total: OpenField<'total', string>;
}

/**
 * The refusals `std.orders.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$orders$list(client: OrdoClient): ReadCall<StdOrdersListRow, StdOrdersListRefusal, StdOrdersListFields> {
  return new ReadCall(client, spec_std$orders$list, {});
}

/**
 * The row shape of `std.orders.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdOrdersUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.orders.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdOrdersUpdateParams = {
  /** `id` */
  readonly id: Ids.OrdersId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `orders.customer` */
  readonly customer: Ids.CustomersId;
  /** `date` · fills `orders.placed` */
  readonly placed: string;
  /** `text` · fills `orders.status` */
  readonly status: string;
  /** `money` · fills `orders.total` */
  readonly total: string;
};

/**
 * The refusals `std.orders.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.orders.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$orders$update(client: OrdoClient, params: StdOrdersUpdateParams): WriteCall<StdOrdersUpdateRow, StdOrdersUpdateRefusal> {
  return new WriteCall(client, spec_std$orders$update, params);
}

/**
 * One row of `std.products.count`.
 */
export interface StdProductsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.products.count` takes no parameters. */
export type StdProductsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.products.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdProductsCountFields = Record<never, never>;

/**
 * The refusals `std.products.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$products$count(client: OrdoClient): SingleReadCall<StdProductsCountRow, StdProductsCountRefusal, StdProductsCountFields> {
  return new SingleReadCall(client, spec_std$products$count, {});
}

/**
 * The row shape of `std.products.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProductsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.products.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProductsCreateParams = {
  /** `id` · fills `products.vendor` */
  readonly vendor: Ids.VendorsId;
  /** `id` · fills `products.category` */
  readonly category: Ids.CategoriesId;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `bool` · fills `products.active` · has a default; omit to take it */
  readonly active?: boolean;
};

/**
 * The refusals `std.products.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$products$create(client: OrdoClient, params: StdProductsCreateParams): WriteCall<StdProductsCreateRow, StdProductsCreateRefusal> {
  return new WriteCall(client, spec_std$products$create, params);
}

/**
 * The row shape of `std.products.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProductsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.products.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProductsDeleteParams = {
  /** `id` */
  readonly id: Ids.ProductsId;
};

/**
 * The refusals `std.products.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$products$delete(client: OrdoClient, params: StdProductsDeleteParams): WriteCall<StdProductsDeleteRow, StdProductsDeleteRefusal> {
  return new WriteCall(client, spec_std$products$delete, params);
}

/**
 * One row of `std.products.get`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface StdProductsGetRow {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly vendor: Ids.VendorsId;
  /** `id` */
  readonly category: Ids.CategoriesId;
  /** `text` */
  readonly name: string;
  /** `money` */
  readonly price: string;
  /** `bool` */
  readonly active: boolean;
}

/**
 * The parameters of `std.products.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProductsGetParams = {
  /** `id` */
  readonly id: Ids.ProductsId;
};

/**
 * The fields a `.where` on `std.products.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProductsGetFields {
  readonly id: OpenField<'id', Ids.ProductsId>;
  readonly rev: OpenField<'rev', number>;
  readonly vendor: OpenField<'vendor', Ids.VendorsId>;
  readonly category: OpenField<'category', Ids.CategoriesId>;
  readonly name: OpenField<'name', string>;
  readonly price: OpenField<'price', string>;
  readonly active: OpenField<'active', boolean>;
}

/**
 * The refusals `std.products.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$products$get(client: OrdoClient, params: StdProductsGetParams): ReadCall<StdProductsGetRow, StdProductsGetRefusal, StdProductsGetFields> {
  return new ReadCall(client, spec_std$products$get, params);
}

/**
 * One row of `std.products.list`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface StdProductsListRow {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly vendor: Ids.VendorsId;
  /** `id` */
  readonly category: Ids.CategoriesId;
  /** `text` */
  readonly name: string;
  /** `money` */
  readonly price: string;
  /** `bool` */
  readonly active: boolean;
}

/** `std.products.list` takes no parameters. */
export type StdProductsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.products.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProductsListFields {
  readonly id: OpenField<'id', Ids.ProductsId>;
  readonly rev: OpenField<'rev', number>;
  readonly vendor: OpenField<'vendor', Ids.VendorsId>;
  readonly category: OpenField<'category', Ids.CategoriesId>;
  readonly name: OpenField<'name', string>;
  readonly price: OpenField<'price', string>;
  readonly active: OpenField<'active', boolean>;
}

/**
 * The refusals `std.products.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$products$list(client: OrdoClient): ReadCall<StdProductsListRow, StdProductsListRefusal, StdProductsListFields> {
  return new ReadCall(client, spec_std$products$list, {});
}

/**
 * The row shape of `std.products.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProductsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.products.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProductsUpdateParams = {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `products.vendor` */
  readonly vendor: Ids.VendorsId;
  /** `id` · fills `products.category` */
  readonly category: Ids.CategoriesId;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `bool` · fills `products.active` */
  readonly active: boolean;
};

/**
 * The refusals `std.products.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$products$update(client: OrdoClient, params: StdProductsUpdateParams): WriteCall<StdProductsUpdateRow, StdProductsUpdateRefusal> {
  return new WriteCall(client, spec_std$products$update, params);
}

/**
 * One row of `std.projectMembers.count`.
 */
export interface StdProjectMembersCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.projectMembers.count` takes no parameters. */
export type StdProjectMembersCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.projectMembers.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdProjectMembersCountFields = Record<never, never>;

/**
 * The refusals `std.projectMembers.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projectMembers$count(client: OrdoClient): SingleReadCall<StdProjectMembersCountRow, StdProjectMembersCountRefusal, StdProjectMembersCountFields> {
  return new SingleReadCall(client, spec_std$projectMembers$count, {});
}

/**
 * The row shape of `std.projectMembers.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectMembersCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projectMembers.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectMembersCreateParams = {
  /** `id` · fills `projectMembers.project` */
  readonly project: Ids.ProjectsId;
  /** `id` · fills `projectMembers.member` */
  readonly member: Ids.MembersId;
  /** `bool` · fills `projectMembers.canWrite` · has a default; omit to take it */
  readonly canWrite?: boolean;
};

/**
 * The refusals `std.projectMembers.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$projectMembers$create(client: OrdoClient, params: StdProjectMembersCreateParams): WriteCall<StdProjectMembersCreateRow, StdProjectMembersCreateRefusal> {
  return new WriteCall(client, spec_std$projectMembers$create, params);
}

/**
 * The row shape of `std.projectMembers.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectMembersDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projectMembers.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectMembersDeleteParams = {
  /** `id` */
  readonly id: Ids.ProjectMembersId;
};

/**
 * The refusals `std.projectMembers.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$projectMembers$delete(client: OrdoClient, params: StdProjectMembersDeleteParams): WriteCall<StdProjectMembersDeleteRow, StdProjectMembersDeleteRefusal> {
  return new WriteCall(client, spec_std$projectMembers$delete, params);
}

/**
 * One row of `std.projectMembers.get`.
 *
 * Row identity: `projectMembers`. `synthetic_rows` is false.
 */
export interface StdProjectMembersGetRow {
  /** `id` */
  readonly id: Ids.ProjectMembersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `bool` */
  readonly canWrite: boolean;
}

/**
 * The parameters of `std.projectMembers.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectMembersGetParams = {
  /** `id` */
  readonly id: Ids.ProjectMembersId;
};

/**
 * The fields a `.where` on `std.projectMembers.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProjectMembersGetFields {
  readonly id: OpenField<'id', Ids.ProjectMembersId>;
  readonly rev: OpenField<'rev', number>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly canWrite: OpenField<'canWrite', boolean>;
}

/**
 * The refusals `std.projectMembers.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projectMembers$get(client: OrdoClient, params: StdProjectMembersGetParams): ReadCall<StdProjectMembersGetRow, StdProjectMembersGetRefusal, StdProjectMembersGetFields> {
  return new ReadCall(client, spec_std$projectMembers$get, params);
}

/**
 * One row of `std.projectMembers.list`.
 *
 * Row identity: `projectMembers`. `synthetic_rows` is false.
 */
export interface StdProjectMembersListRow {
  /** `id` */
  readonly id: Ids.ProjectMembersId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `bool` */
  readonly canWrite: boolean;
}

/** `std.projectMembers.list` takes no parameters. */
export type StdProjectMembersListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.projectMembers.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdProjectMembersListFields {
  readonly id: OpenField<'id', Ids.ProjectMembersId>;
  readonly rev: OpenField<'rev', number>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly canWrite: OpenField<'canWrite', boolean>;
}

/**
 * The refusals `std.projectMembers.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$projectMembers$list(client: OrdoClient): ReadCall<StdProjectMembersListRow, StdProjectMembersListRefusal, StdProjectMembersListFields> {
  return new ReadCall(client, spec_std$projectMembers$list, {});
}

/**
 * The row shape of `std.projectMembers.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProjectMembersUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.projectMembers.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProjectMembersUpdateParams = {
  /** `id` */
  readonly id: Ids.ProjectMembersId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `projectMembers.project` */
  readonly project: Ids.ProjectsId;
  /** `id` · fills `projectMembers.member` */
  readonly member: Ids.MembersId;
  /** `bool` · fills `projectMembers.canWrite` */
  readonly canWrite: boolean;
};

/**
 * The refusals `std.projectMembers.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProjectMembersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.projectMembers.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$projectMembers$update(client: OrdoClient, params: StdProjectMembersUpdateParams): WriteCall<StdProjectMembersUpdateRow, StdProjectMembersUpdateRefusal> {
  return new WriteCall(client, spec_std$projectMembers$update, params);
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
  /** `id` · fills `projects.owner` */
  readonly owner: Ids.MembersId;
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
  /** `id` */
  readonly owner: Ids.MembersId;
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
  readonly owner: OpenField<'owner', Ids.MembersId>;
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
  /** `id` */
  readonly owner: Ids.MembersId;
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
  readonly owner: OpenField<'owner', Ids.MembersId>;
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
  /** `id` · fills `projects.owner` */
  readonly owner: Ids.MembersId;
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
 * One row of `std.taskNotes.count`.
 */
export interface StdTaskNotesCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.taskNotes.count` takes no parameters. */
export type StdTaskNotesCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.taskNotes.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdTaskNotesCountFields = Record<never, never>;

/**
 * The refusals `std.taskNotes.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$taskNotes$count(client: OrdoClient): SingleReadCall<StdTaskNotesCountRow, StdTaskNotesCountRefusal, StdTaskNotesCountFields> {
  return new SingleReadCall(client, spec_std$taskNotes$count, {});
}

/**
 * The row shape of `std.taskNotes.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTaskNotesCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.taskNotes.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTaskNotesCreateParams = {
  /** `id` · fills `taskNotes.task` */
  readonly task: Ids.TasksId;
  /** `text` · fills `taskNotes.body` */
  readonly body: string;
};

/**
 * The refusals `std.taskNotes.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$taskNotes$create(client: OrdoClient, params: StdTaskNotesCreateParams): WriteCall<StdTaskNotesCreateRow, StdTaskNotesCreateRefusal> {
  return new WriteCall(client, spec_std$taskNotes$create, params);
}

/**
 * The row shape of `std.taskNotes.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTaskNotesDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.taskNotes.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTaskNotesDeleteParams = {
  /** `id` */
  readonly id: Ids.TaskNotesId;
};

/**
 * The refusals `std.taskNotes.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$taskNotes$delete(client: OrdoClient, params: StdTaskNotesDeleteParams): WriteCall<StdTaskNotesDeleteRow, StdTaskNotesDeleteRefusal> {
  return new WriteCall(client, spec_std$taskNotes$delete, params);
}

/**
 * One row of `std.taskNotes.get`.
 *
 * Row identity: `taskNotes`. `synthetic_rows` is false.
 */
export interface StdTaskNotesGetRow {
  /** `id` */
  readonly id: Ids.TaskNotesId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly task: Ids.TasksId;
  /** `text` */
  readonly body: string;
}

/**
 * The parameters of `std.taskNotes.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTaskNotesGetParams = {
  /** `id` */
  readonly id: Ids.TaskNotesId;
};

/**
 * The fields a `.where` on `std.taskNotes.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTaskNotesGetFields {
  readonly id: OpenField<'id', Ids.TaskNotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly task: OpenField<'task', Ids.TasksId>;
  readonly body: OpenField<'body', string>;
}

/**
 * The refusals `std.taskNotes.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$taskNotes$get(client: OrdoClient, params: StdTaskNotesGetParams): ReadCall<StdTaskNotesGetRow, StdTaskNotesGetRefusal, StdTaskNotesGetFields> {
  return new ReadCall(client, spec_std$taskNotes$get, params);
}

/**
 * One row of `std.taskNotes.list`.
 *
 * Row identity: `taskNotes`. `synthetic_rows` is false.
 */
export interface StdTaskNotesListRow {
  /** `id` */
  readonly id: Ids.TaskNotesId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly task: Ids.TasksId;
  /** `text` */
  readonly body: string;
}

/** `std.taskNotes.list` takes no parameters. */
export type StdTaskNotesListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.taskNotes.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdTaskNotesListFields {
  readonly id: OpenField<'id', Ids.TaskNotesId>;
  readonly rev: OpenField<'rev', number>;
  readonly task: OpenField<'task', Ids.TasksId>;
  readonly body: OpenField<'body', string>;
}

/**
 * The refusals `std.taskNotes.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$taskNotes$list(client: OrdoClient): ReadCall<StdTaskNotesListRow, StdTaskNotesListRefusal, StdTaskNotesListFields> {
  return new ReadCall(client, spec_std$taskNotes$list, {});
}

/**
 * The row shape of `std.taskNotes.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdTaskNotesUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.taskNotes.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdTaskNotesUpdateParams = {
  /** `id` */
  readonly id: Ids.TaskNotesId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `taskNotes.task` */
  readonly task: Ids.TasksId;
  /** `text` · fills `taskNotes.body` */
  readonly body: string;
};

/**
 * The refusals `std.taskNotes.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdTaskNotesUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.taskNotes.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$taskNotes$update(client: OrdoClient, params: StdTaskNotesUpdateParams): WriteCall<StdTaskNotesUpdateRow, StdTaskNotesUpdateRefusal> {
  return new WriteCall(client, spec_std$taskNotes$update, params);
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
  /** `id` · fills `tasks.assignee` · has a default; omit it or pass null to take it */
  readonly assignee?: Ids.MembersId | null;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `text` · fills `tasks.status` · has a default; omit to take it */
  readonly status?: string;
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
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly assignee: Ids.MembersId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
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
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
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
  /** `id` */
  readonly assignee: Ids.MembersId;
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
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
  readonly assignee: OpenField<'assignee', Ids.MembersId>;
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
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
  /** `id` · fills `tasks.assignee` */
  readonly assignee: Ids.MembersId;
  /** `text` · fills `tasks.title` */
  readonly title: string;
  /** `text` · fills `tasks.status` */
  readonly status: string;
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
 * One row of `std.vendors.count`.
 */
export interface StdVendorsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.vendors.count` takes no parameters. */
export type StdVendorsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.vendors.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdVendorsCountFields = Record<never, never>;

/**
 * The refusals `std.vendors.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$vendors$count(client: OrdoClient): SingleReadCall<StdVendorsCountRow, StdVendorsCountRefusal, StdVendorsCountFields> {
  return new SingleReadCall(client, spec_std$vendors$count, {});
}

/**
 * The row shape of `std.vendors.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdVendorsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.vendors.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdVendorsCreateParams = {
  /** `text` · fills `vendors.name` */
  readonly name: string;
  /** `text` · fills `vendors.region` */
  readonly region: string;
  /** `bool` · fills `vendors.active` · has a default; omit to take it */
  readonly active?: boolean;
};

/**
 * The refusals `std.vendors.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$vendors$create(client: OrdoClient, params: StdVendorsCreateParams): WriteCall<StdVendorsCreateRow, StdVendorsCreateRefusal> {
  return new WriteCall(client, spec_std$vendors$create, params);
}

/**
 * The row shape of `std.vendors.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdVendorsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.vendors.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdVendorsDeleteParams = {
  /** `id` */
  readonly id: Ids.VendorsId;
};

/**
 * The refusals `std.vendors.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$vendors$delete(client: OrdoClient, params: StdVendorsDeleteParams): WriteCall<StdVendorsDeleteRow, StdVendorsDeleteRefusal> {
  return new WriteCall(client, spec_std$vendors$delete, params);
}

/**
 * One row of `std.vendors.get`.
 *
 * Row identity: `vendors`. `synthetic_rows` is false.
 */
export interface StdVendorsGetRow {
  /** `id` */
  readonly id: Ids.VendorsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly region: string;
  /** `bool` */
  readonly active: boolean;
}

/**
 * The parameters of `std.vendors.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdVendorsGetParams = {
  /** `id` */
  readonly id: Ids.VendorsId;
};

/**
 * The fields a `.where` on `std.vendors.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdVendorsGetFields {
  readonly id: OpenField<'id', Ids.VendorsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly region: OpenField<'region', string>;
  readonly active: OpenField<'active', boolean>;
}

/**
 * The refusals `std.vendors.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$vendors$get(client: OrdoClient, params: StdVendorsGetParams): ReadCall<StdVendorsGetRow, StdVendorsGetRefusal, StdVendorsGetFields> {
  return new ReadCall(client, spec_std$vendors$get, params);
}

/**
 * One row of `std.vendors.list`.
 *
 * Row identity: `vendors`. `synthetic_rows` is false.
 */
export interface StdVendorsListRow {
  /** `id` */
  readonly id: Ids.VendorsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly region: string;
  /** `bool` */
  readonly active: boolean;
}

/** `std.vendors.list` takes no parameters. */
export type StdVendorsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.vendors.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdVendorsListFields {
  readonly id: OpenField<'id', Ids.VendorsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
  readonly region: OpenField<'region', string>;
  readonly active: OpenField<'active', boolean>;
}

/**
 * The refusals `std.vendors.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$vendors$list(client: OrdoClient): ReadCall<StdVendorsListRow, StdVendorsListRefusal, StdVendorsListFields> {
  return new ReadCall(client, spec_std$vendors$list, {});
}

/**
 * The row shape of `std.vendors.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdVendorsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.vendors.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdVendorsUpdateParams = {
  /** `id` */
  readonly id: Ids.VendorsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `vendors.name` */
  readonly name: string;
  /** `text` · fills `vendors.region` */
  readonly region: string;
  /** `bool` · fills `vendors.active` */
  readonly active: boolean;
};

/**
 * The refusals `std.vendors.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdVendorsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.vendors.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$vendors$update(client: OrdoClient, params: StdVendorsUpdateParams): WriteCall<StdVendorsUpdateRow, StdVendorsUpdateRefusal> {
  return new WriteCall(client, spec_std$vendors$update, params);
}

/**
 * One row of `taskBoard`.
 *
 * Row identity: `tasks`. `synthetic_rows` is false.
 */
export interface TaskBoardRow {
  /** `text` */
  readonly title: string;
  /** `text` */
  readonly status: string;
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly $id: Ids.TasksId;
}

/** `taskBoard` takes no parameters. */
export type TaskBoardParams = Record<never, never>;

/**
 * The fields a `.where` on `taskBoard` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface TaskBoardFields {
  readonly title: OpenField<'title', string>;
  readonly status: OpenField<'status', string>;
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly $id: OpenField<'$id', Ids.TasksId>;
}

/**
 * The refusals `taskBoard` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type TaskBoardRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `taskBoard` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function taskBoard(client: OrdoClient): ReadCall<TaskBoardRow, TaskBoardRefusal, TaskBoardFields> {
  return new ReadCall(client, spec_taskBoard, {});
}

/**
 * One row of `teamRoster`.
 *
 * Row identity: `projectMembers`. `synthetic_rows` is false.
 */
export interface TeamRosterRow {
  /** `id` */
  readonly project: Ids.ProjectsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `bool` */
  readonly canWrite: boolean;
  /** `id` */
  readonly $id: Ids.ProjectMembersId;
}

/** `teamRoster` takes no parameters. */
export type TeamRosterParams = Record<never, never>;

/**
 * The fields a `.where` on `teamRoster` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface TeamRosterFields {
  readonly project: OpenField<'project', Ids.ProjectsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly canWrite: OpenField<'canWrite', boolean>;
  readonly $id: OpenField<'$id', Ids.ProjectMembersId>;
}

/**
 * The refusals `teamRoster` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type TeamRosterRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `teamRoster` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function teamRoster(client: OrdoClient): ReadCall<TeamRosterRow, TeamRosterRefusal, TeamRosterFields> {
  return new ReadCall(client, spec_teamRoster, {});
}

/**
 * One row of `toolDeals`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface ToolDealsRow {
  /** `text` */
  readonly name: string;
  /** `money` */
  readonly price: string;
  /** `id` */
  readonly $id: Ids.ProductsId;
}

/**
 * The parameters of `toolDeals`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ToolDealsParams = {
  /** `money` */
  readonly cap: string;
  /** `id` */
  readonly root: string;
};

/**
 * The fields a `.where` on `toolDeals` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ToolDealsFields {
  readonly name: OpenField<'name', string>;
  readonly price: OpenField<'price', string>;
  readonly $id: OpenField<'$id', Ids.ProductsId>;
}

/**
 * The refusals `toolDeals` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ToolDealsRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `toolDeals` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function toolDeals(client: OrdoClient, params: ToolDealsParams): ReadCall<ToolDealsRow, ToolDealsRefusal, ToolDealsFields> {
  return new ReadCall(client, spec_toolDeals, params);
}

/**
 * One row of `top3Orders`.
 *
 * Row identity: `orders`. `synthetic_rows` is false.
 */
export interface Top3OrdersRow {
  /** `date` */
  readonly placed: string;
  /** `money` */
  readonly total: string;
  /** `id` */
  readonly customer: Ids.CustomersId;
  /** `id` */
  readonly $id: Ids.OrdersId;
}

/** `top3Orders` takes no parameters. */
export type Top3OrdersParams = Record<never, never>;

/**
 * The fields a `.where` on `top3Orders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface Top3OrdersFields {
  readonly placed: OpenField<'placed', string>;
  readonly total: OpenField<'total', string>;
  readonly customer: OpenField<'customer', Ids.CustomersId>;
  readonly $id: OpenField<'$id', Ids.OrdersId>;
}

/**
 * The refusals `top3Orders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type Top3OrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `top3Orders` — read.
 * Cost class `sql`, rows: <= 3.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function top3Orders(client: OrdoClient): ReadCall<Top3OrdersRow, Top3OrdersRefusal, Top3OrdersFields> {
  return new ReadCall(client, spec_top3Orders, {});
}

/**
 * The row shape of `upsertCustomer`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type UpsertCustomerRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `upsertCustomer`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type UpsertCustomerParams = {
  /** `text` · fills `customers.name` */
  readonly name: string;
  /** `text` · fills `customers.email` */
  readonly email: string;
  /** `text` · fills `customers.tier` · has a default; omit to take it */
  readonly tier?: string;
};

/**
 * The refusals `upsertCustomer` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type UpsertCustomerRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'sealed_open_failed' | 'sealed_unreadable' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `upsertCustomer` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function upsertCustomer(client: OrdoClient, params: UpsertCustomerParams): WriteCall<UpsertCustomerRow, UpsertCustomerRefusal> {
  return new WriteCall(client, spec_upsertCustomer, params);
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  activeProducts: activeProducts,
  addAnonCustomer: addAnonCustomer,
  addBoard: addBoard,
  addCard: addCard,
  addCategory: addCategory,
  addCustomer: addCustomer,
  addNote: addNote,
  addRootCategory: addRootCategory,
  addTask: addTask,
  allTasks: allTasks,
  anonCustomers: anonCustomers,
  boardList: boardList,
  cardList: cardList,
  catInfo: catInfo,
  cheapActive: cheapActive,
  claimCoupon: claimCoupon,
  claimTask: claimTask,
  clearCustomerEmail: clearCustomerEmail,
  closeTask: closeTask,
  customerByEmail: customerByEmail,
  doneOrders: doneOrders,
  dropCategory: dropCategory,
  dropCategoryTree: dropCategoryTree,
  dropFolder: dropFolder,
  dropFolderTree: dropFolderTree,
  entryList: entryList,
  finishCard: finishCard,
  firstTask: firstTask,
  giftOrders: giftOrders,
  grantBoardsAccess: grantBoardsAccess,
  memoDigest: memoDigest,
  monthlyRevenue: monthlyRevenue,
  moveBA: moveBA,
  moveCategory: moveCategory,
  movePM: movePM,
  myAssignedTasks: myAssignedTasks,
  myProjects: myProjects,
  noteFeed: noteFeed,
  openTasks: openTasks,
  orderMemos: orderMemos,
  productRevenue: productRevenue,
  purgeDone: purgeDone,
  purgeNotes: purgeNotes,
  reassign: reassign,
  recentOrders: recentOrders,
  recordEntry: recordEntry,
  recordEntryFor: recordEntryFor,
  recordEntryOnce: recordEntryOnce,
  renameTask: renameTask,
  renameTaskIf: renameTaskIf,
  renumberEntry: renumberEntry,
  renumberEntryOnce: renumberEntryOnce,
  reviseEntry: reviseEntry,
  revokeBoardsAccess: revokeBoardsAccess,
  setBoardsAccessLevel: setBoardsAccessLevel,
  setMemo: setMemo,
  specCount: specCount,
  specTasks: specTasks,
  spendByCustomer: spendByCustomer,
  'std.boards.count': std$boards$count,
  'std.boards.create': std$boards$create,
  'std.boards.delete': std$boards$delete,
  'std.boards.get': std$boards$get,
  'std.boards.list': std$boards$list,
  'std.boards.update': std$boards$update,
  'std.boardsAccess.count': std$boardsAccess$count,
  'std.boardsAccess.create': std$boardsAccess$create,
  'std.boardsAccess.delete': std$boardsAccess$delete,
  'std.boardsAccess.get': std$boardsAccess$get,
  'std.boardsAccess.list': std$boardsAccess$list,
  'std.boardsAccess.put': std$boardsAccess$put,
  'std.boardsAccess.update': std$boardsAccess$update,
  'std.cards.count': std$cards$count,
  'std.cards.create': std$cards$create,
  'std.cards.delete': std$cards$delete,
  'std.cards.get': std$cards$get,
  'std.cards.list': std$cards$list,
  'std.cards.update': std$cards$update,
  'std.categories.count': std$categories$count,
  'std.categories.create': std$categories$create,
  'std.categories.delete': std$categories$delete,
  'std.categories.deleteSubtree': std$categories$deleteSubtree,
  'std.categories.get': std$categories$get,
  'std.categories.list': std$categories$list,
  'std.categories.put': std$categories$put,
  'std.categories.update': std$categories$update,
  'std.coupons.count': std$coupons$count,
  'std.coupons.delete': std$coupons$delete,
  'std.coupons.get': std$coupons$get,
  'std.coupons.list': std$coupons$list,
  'std.coupons.update': std$coupons$update,
  'std.customers.count': std$customers$count,
  'std.customers.create': std$customers$create,
  'std.customers.delete': std$customers$delete,
  'std.customers.get': std$customers$get,
  'std.customers.list': std$customers$list,
  'std.customers.put': std$customers$put,
  'std.customers.update': std$customers$update,
  'std.folders.count': std$folders$count,
  'std.folders.create': std$folders$create,
  'std.folders.delete': std$folders$delete,
  'std.folders.deleteSubtree': std$folders$deleteSubtree,
  'std.folders.get': std$folders$get,
  'std.folders.list': std$folders$list,
  'std.folders.update': std$folders$update,
  'std.ledgerEntries.count': std$ledgerEntries$count,
  'std.ledgerEntries.create': std$ledgerEntries$create,
  'std.ledgerEntries.delete': std$ledgerEntries$delete,
  'std.ledgerEntries.get': std$ledgerEntries$get,
  'std.ledgerEntries.list': std$ledgerEntries$list,
  'std.ledgerEntries.put': std$ledgerEntries$put,
  'std.ledgerEntries.update': std$ledgerEntries$update,
  'std.orderItems.count': std$orderItems$count,
  'std.orderItems.create': std$orderItems$create,
  'std.orderItems.delete': std$orderItems$delete,
  'std.orderItems.get': std$orderItems$get,
  'std.orderItems.list': std$orderItems$list,
  'std.orderItems.update': std$orderItems$update,
  'std.orders.count': std$orders$count,
  'std.orders.create': std$orders$create,
  'std.orders.delete': std$orders$delete,
  'std.orders.get': std$orders$get,
  'std.orders.list': std$orders$list,
  'std.orders.update': std$orders$update,
  'std.products.count': std$products$count,
  'std.products.create': std$products$create,
  'std.products.delete': std$products$delete,
  'std.products.get': std$products$get,
  'std.products.list': std$products$list,
  'std.products.update': std$products$update,
  'std.projectMembers.count': std$projectMembers$count,
  'std.projectMembers.create': std$projectMembers$create,
  'std.projectMembers.delete': std$projectMembers$delete,
  'std.projectMembers.get': std$projectMembers$get,
  'std.projectMembers.list': std$projectMembers$list,
  'std.projectMembers.update': std$projectMembers$update,
  'std.projects.count': std$projects$count,
  'std.projects.create': std$projects$create,
  'std.projects.delete': std$projects$delete,
  'std.projects.get': std$projects$get,
  'std.projects.list': std$projects$list,
  'std.projects.update': std$projects$update,
  'std.taskNotes.count': std$taskNotes$count,
  'std.taskNotes.create': std$taskNotes$create,
  'std.taskNotes.delete': std$taskNotes$delete,
  'std.taskNotes.get': std$taskNotes$get,
  'std.taskNotes.list': std$taskNotes$list,
  'std.taskNotes.update': std$taskNotes$update,
  'std.tasks.count': std$tasks$count,
  'std.tasks.create': std$tasks$create,
  'std.tasks.delete': std$tasks$delete,
  'std.tasks.get': std$tasks$get,
  'std.tasks.list': std$tasks$list,
  'std.tasks.update': std$tasks$update,
  'std.vendors.count': std$vendors$count,
  'std.vendors.create': std$vendors$create,
  'std.vendors.delete': std$vendors$delete,
  'std.vendors.get': std$vendors$get,
  'std.vendors.list': std$vendors$list,
  'std.vendors.update': std$vendors$update,
  taskBoard: taskBoard,
  teamRoster: teamRoster,
  toolDeals: toolDeals,
  top3Orders: top3Orders,
  upsertCustomer: upsertCustomer,
} as const;
