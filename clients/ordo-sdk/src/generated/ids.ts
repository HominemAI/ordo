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

import type { Branded } from '../core/brand.js';

/** Every entity id type in this deployment, keyed by entity name. */
/**
 * An id addressing `members`.
 *
 * Typed-id prefix `mem`, so a value looks like `mem_…`. The brand is compile-time only; the wire sees the string.
 */
export type MembersId = Branded<string, 'members'>;

/**
 * An id addressing `groupMembers`.
 */
export type GroupMembersId = Branded<string, 'groupMembers'>;

/**
 * An id addressing `vendors`.
 *
 * Typed-id prefix `vend`, so a value looks like `vend_…`. The brand is compile-time only; the wire sees the string.
 */
export type VendorsId = Branded<string, 'vendors'>;

/**
 * An id addressing `categories`.
 *
 * Typed-id prefix `cat`, so a value looks like `cat_…`. The brand is compile-time only; the wire sees the string.
 */
export type CategoriesId = Branded<string, 'categories'>;

/**
 * An id addressing `products`.
 *
 * Typed-id prefix `prod`, so a value looks like `prod_…`. The brand is compile-time only; the wire sees the string.
 */
export type ProductsId = Branded<string, 'products'>;

/**
 * An id addressing `customers`.
 *
 * Typed-id prefix `cust`, so a value looks like `cust_…`. The brand is compile-time only; the wire sees the string.
 */
export type CustomersId = Branded<string, 'customers'>;

/**
 * An id addressing `orders`.
 *
 * Typed-id prefix `ord`, so a value looks like `ord_…`. The brand is compile-time only; the wire sees the string.
 */
export type OrdersId = Branded<string, 'orders'>;

/**
 * An id addressing `orderItems`.
 *
 * Typed-id prefix `item`, so a value looks like `item_…`. The brand is compile-time only; the wire sees the string.
 */
export type OrderItemsId = Branded<string, 'orderItems'>;

/**
 * An id addressing `projects`.
 *
 * Typed-id prefix `proj`, so a value looks like `proj_…`. The brand is compile-time only; the wire sees the string.
 */
export type ProjectsId = Branded<string, 'projects'>;

/**
 * An id addressing `projectMembers`.
 *
 * Typed-id prefix `proj-memb`, so a value looks like `proj-memb_…`. The brand is compile-time only; the wire sees the string.
 */
export type ProjectMembersId = Branded<string, 'projectMembers'>;

/**
 * An id addressing `tasks`.
 *
 * Typed-id prefix `task`, so a value looks like `task_…`. The brand is compile-time only; the wire sees the string.
 */
export type TasksId = Branded<string, 'tasks'>;

/**
 * An id addressing `taskNotes`.
 *
 * Typed-id prefix `note`, so a value looks like `note_…`. The brand is compile-time only; the wire sees the string.
 */
export type TaskNotesId = Branded<string, 'taskNotes'>;

/**
 * An id addressing `boards`.
 *
 * Typed-id prefix `board`, so a value looks like `board_…`. The brand is compile-time only; the wire sees the string.
 */
export type BoardsId = Branded<string, 'boards'>;

/**
 * An id addressing `cards`.
 *
 * Typed-id prefix `card`, so a value looks like `card_…`. The brand is compile-time only; the wire sees the string.
 */
export type CardsId = Branded<string, 'cards'>;

/**
 * An id addressing `folders`.
 *
 * Typed-id prefix `fold`, so a value looks like `fold_…`. The brand is compile-time only; the wire sees the string.
 */
export type FoldersId = Branded<string, 'folders'>;

/**
 * An id addressing `ledgerEntries`.
 *
 * Typed-id prefix `led`, so a value looks like `led_…`. The brand is compile-time only; the wire sees the string.
 */
export type LedgerEntriesId = Branded<string, 'ledgerEntries'>;

/**
 * An id addressing `coupons`.
 *
 * Typed-id prefix `coup`, so a value looks like `coup_…`. The brand is compile-time only; the wire sees the string.
 */
export type CouponsId = Branded<string, 'coupons'>;

/**
 * An id addressing `boardsAccess`.
 *
 * Typed-id prefix `boar-acce`, so a value looks like `boar-acce_…`. The brand is compile-time only; the wire sees the string.
 */
export type BoardsAccessId = Branded<string, 'boardsAccess'>;

