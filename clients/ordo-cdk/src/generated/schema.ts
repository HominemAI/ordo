// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// This deployment's sources and field accessors, typed.
//
// Source of truth: the deployment's `interface.json` (`ordo.interface/2`)
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import { src, type Src } from '../core/build.js';
import type { AnyFields, Value } from '../core/value.js';
import { f as anyFields } from './lang.js';

/** The descriptor this file was generated from. */
export const SCHEMA = { domain: 'market', digest: 'f5cc0225707e1c8c' } as const;

// ── sources ──

/** The `members` entity, as a source. */
export const members: Src = src('members');
/** The `groupMembers` entity, as a source. */
export const groupMembers: Src = src('groupMembers');
/** The `vendors` entity, as a source. */
export const vendors: Src = src('vendors');
/** The `categories` entity, as a source. */
export const categories: Src = src('categories');
/** The `products` entity, as a source. */
export const products: Src = src('products');
/** The `customers` entity, as a source. */
export const customers: Src = src('customers');
/** The `orders` entity, as a source. */
export const orders: Src = src('orders');
/** The `orderItems` entity, as a source. */
export const orderItems: Src = src('orderItems');
/** The `projects` entity, as a source. */
export const projects: Src = src('projects');
/** The `projectMembers` entity, as a source. */
export const projectMembers: Src = src('projectMembers');
/** The `tasks` entity, as a source. */
export const tasks: Src = src('tasks');
/** The `taskNotes` entity, as a source. */
export const taskNotes: Src = src('taskNotes');
/** The `boards` entity, as a source. */
export const boards: Src = src('boards');
/** The `cards` entity, as a source. */
export const cards: Src = src('cards');
/** The `folders` entity, as a source. */
export const folders: Src = src('folders');
/** The `ledgerEntries` entity, as a source. */
export const ledgerEntries: Src = src('ledgerEntries');
/** The `coupons` entity, as a source. */
export const coupons: Src = src('coupons');
/** The `boardsAccess` entity, as a source. */
export const boardsAccess: Src = src('boardsAccess');

/** The `activeProducts` read op, as a composition source. */
export const activeProducts: Src = src('activeProducts');
/** The `allTasks` read op, as a composition source. */
export const allTasks: Src = src('allTasks');
/** The `anonCustomers` read op, as a composition source. */
export const anonCustomers: Src = src('anonCustomers');
/** The `boardList` read op, as a composition source. */
export const boardList: Src = src('boardList');
/** The `cardList` read op, as a composition source. */
export const cardList: Src = src('cardList');
/** The `catInfo` read op, as a composition source. */
export const catInfo: Src = src('catInfo');
/** The `cheapActive` read op, as a composition source. */
export const cheapActive: Src = src('cheapActive');
/** The `customerByEmail` read op, as a composition source. */
export const customerByEmail: Src = src('customerByEmail');
/** The `doneOrders` read op, as a composition source. */
export const doneOrders: Src = src('doneOrders');
/** The `entryList` read op, as a composition source. */
export const entryList: Src = src('entryList');
/** The `firstTask` read op, as a composition source. */
export const firstTask: Src = src('firstTask');
/** The `giftOrders` read op, as a composition source. */
export const giftOrders: Src = src('giftOrders');
/** The `memoDigest` read op, as a composition source. */
export const memoDigest: Src = src('memoDigest');
/** The `monthlyRevenue` read op, as a composition source. */
export const monthlyRevenue: Src = src('monthlyRevenue');
/** The `myAssignedTasks` read op, as a composition source. */
export const myAssignedTasks: Src = src('myAssignedTasks');
/** The `myProjects` read op, as a composition source. */
export const myProjects: Src = src('myProjects');
/** The `noteFeed` read op, as a composition source. */
export const noteFeed: Src = src('noteFeed');
/** The `openTasks` read op, as a composition source. */
export const openTasks: Src = src('openTasks');
/** The `orderMemos` read op, as a composition source. */
export const orderMemos: Src = src('orderMemos');
/** The `productRevenue` read op, as a composition source. */
export const productRevenue: Src = src('productRevenue');
/** The `recentOrders` read op, as a composition source. */
export const recentOrders: Src = src('recentOrders');
/** The `specCount` read op, as a composition source. */
export const specCount: Src = src('specCount');
/** The `specTasks` read op, as a composition source. */
export const specTasks: Src = src('specTasks');
/** The `spendByCustomer` read op, as a composition source. */
export const spendByCustomer: Src = src('spendByCustomer');
/** The `std.boards.count` read op, as a composition source. */
export const std$boards$count: Src = src('std.boards.count');
/** The `std.boards.get` read op, as a composition source. */
export const std$boards$get: Src = src('std.boards.get');
/** The `std.boards.list` read op, as a composition source. */
export const std$boards$list: Src = src('std.boards.list');
/** The `std.boardsAccess.count` read op, as a composition source. */
export const std$boardsAccess$count: Src = src('std.boardsAccess.count');
/** The `std.boardsAccess.get` read op, as a composition source. */
export const std$boardsAccess$get: Src = src('std.boardsAccess.get');
/** The `std.boardsAccess.list` read op, as a composition source. */
export const std$boardsAccess$list: Src = src('std.boardsAccess.list');
/** The `std.cards.count` read op, as a composition source. */
export const std$cards$count: Src = src('std.cards.count');
/** The `std.cards.get` read op, as a composition source. */
export const std$cards$get: Src = src('std.cards.get');
/** The `std.cards.list` read op, as a composition source. */
export const std$cards$list: Src = src('std.cards.list');
/** The `std.categories.count` read op, as a composition source. */
export const std$categories$count: Src = src('std.categories.count');
/** The `std.categories.get` read op, as a composition source. */
export const std$categories$get: Src = src('std.categories.get');
/** The `std.categories.list` read op, as a composition source. */
export const std$categories$list: Src = src('std.categories.list');
/** The `std.coupons.count` read op, as a composition source. */
export const std$coupons$count: Src = src('std.coupons.count');
/** The `std.coupons.get` read op, as a composition source. */
export const std$coupons$get: Src = src('std.coupons.get');
/** The `std.coupons.list` read op, as a composition source. */
export const std$coupons$list: Src = src('std.coupons.list');
/** The `std.customers.count` read op, as a composition source. */
export const std$customers$count: Src = src('std.customers.count');
/** The `std.customers.get` read op, as a composition source. */
export const std$customers$get: Src = src('std.customers.get');
/** The `std.customers.list` read op, as a composition source. */
export const std$customers$list: Src = src('std.customers.list');
/** The `std.folders.count` read op, as a composition source. */
export const std$folders$count: Src = src('std.folders.count');
/** The `std.folders.get` read op, as a composition source. */
export const std$folders$get: Src = src('std.folders.get');
/** The `std.folders.list` read op, as a composition source. */
export const std$folders$list: Src = src('std.folders.list');
/** The `std.ledgerEntries.count` read op, as a composition source. */
export const std$ledgerEntries$count: Src = src('std.ledgerEntries.count');
/** The `std.ledgerEntries.get` read op, as a composition source. */
export const std$ledgerEntries$get: Src = src('std.ledgerEntries.get');
/** The `std.ledgerEntries.list` read op, as a composition source. */
export const std$ledgerEntries$list: Src = src('std.ledgerEntries.list');
/** The `std.orderItems.count` read op, as a composition source. */
export const std$orderItems$count: Src = src('std.orderItems.count');
/** The `std.orderItems.get` read op, as a composition source. */
export const std$orderItems$get: Src = src('std.orderItems.get');
/** The `std.orderItems.list` read op, as a composition source. */
export const std$orderItems$list: Src = src('std.orderItems.list');
/** The `std.orders.count` read op, as a composition source. */
export const std$orders$count: Src = src('std.orders.count');
/** The `std.orders.get` read op, as a composition source. */
export const std$orders$get: Src = src('std.orders.get');
/** The `std.orders.list` read op, as a composition source. */
export const std$orders$list: Src = src('std.orders.list');
/** The `std.products.count` read op, as a composition source. */
export const std$products$count: Src = src('std.products.count');
/** The `std.products.get` read op, as a composition source. */
export const std$products$get: Src = src('std.products.get');
/** The `std.products.list` read op, as a composition source. */
export const std$products$list: Src = src('std.products.list');
/** The `std.projectMembers.count` read op, as a composition source. */
export const std$projectMembers$count: Src = src('std.projectMembers.count');
/** The `std.projectMembers.get` read op, as a composition source. */
export const std$projectMembers$get: Src = src('std.projectMembers.get');
/** The `std.projectMembers.list` read op, as a composition source. */
export const std$projectMembers$list: Src = src('std.projectMembers.list');
/** The `std.projects.count` read op, as a composition source. */
export const std$projects$count: Src = src('std.projects.count');
/** The `std.projects.get` read op, as a composition source. */
export const std$projects$get: Src = src('std.projects.get');
/** The `std.projects.list` read op, as a composition source. */
export const std$projects$list: Src = src('std.projects.list');
/** The `std.taskNotes.count` read op, as a composition source. */
export const std$taskNotes$count: Src = src('std.taskNotes.count');
/** The `std.taskNotes.get` read op, as a composition source. */
export const std$taskNotes$get: Src = src('std.taskNotes.get');
/** The `std.taskNotes.list` read op, as a composition source. */
export const std$taskNotes$list: Src = src('std.taskNotes.list');
/** The `std.tasks.count` read op, as a composition source. */
export const std$tasks$count: Src = src('std.tasks.count');
/** The `std.tasks.get` read op, as a composition source. */
export const std$tasks$get: Src = src('std.tasks.get');
/** The `std.tasks.list` read op, as a composition source. */
export const std$tasks$list: Src = src('std.tasks.list');
/** The `std.vendors.count` read op, as a composition source. */
export const std$vendors$count: Src = src('std.vendors.count');
/** The `std.vendors.get` read op, as a composition source. */
export const std$vendors$get: Src = src('std.vendors.get');
/** The `std.vendors.list` read op, as a composition source. */
export const std$vendors$list: Src = src('std.vendors.list');
/** The `taskBoard` read op, as a composition source. */
export const taskBoard: Src = src('taskBoard');
/** The `teamRoster` read op, as a composition source. */
export const teamRoster: Src = src('teamRoster');
/** The `toolDeals` read op, as a composition source. */
export const toolDeals: Src = src('toolDeals');
/** The `top3Orders` read op, as a composition source. */
export const top3Orders: Src = src('top3Orders');

// ── field accessors ──

/** The fields of `members`. */
export interface MembersFields {
  readonly principal: Value;
  readonly displayName: Value;
  readonly kind: Value;
  readonly memberCount: Value;
  readonly role: Value;
  readonly status: Value;
  readonly joined: Value;
}

/** The fields of `groupMembers`. */
export interface GroupMembersFields {
  readonly group: Value & MembersFields;
  readonly member: Value & MembersFields;
}

/** The fields of `vendors`. */
export interface VendorsFields {
  readonly name: Value;
  readonly region: Value;
  readonly active: Value;
}

/** The fields of `categories`. */
export interface CategoriesFields {
  readonly name: Value;
  readonly parent: Value & CategoriesFields;
}

/** The fields of `products`. */
export interface ProductsFields {
  readonly vendor: Value & VendorsFields;
  readonly category: Value & CategoriesFields;
  readonly name: Value;
  readonly price: Value;
  readonly active: Value;
}

/** The fields of `customers`. */
export interface CustomersFields {
  readonly name: Value;
  readonly email: Value;
  readonly tier: Value;
}

/** The fields of `orders`. */
export interface OrdersFields {
  readonly customer: Value & CustomersFields;
  readonly placed: Value;
  readonly status: Value;
  readonly total: Value;
  readonly memo: Value;
}

/** The fields of `orderItems`. */
export interface OrderItemsFields {
  readonly order: Value & OrdersFields;
  readonly product: Value & ProductsFields;
  readonly qty: Value;
  readonly amount: Value;
}

/** The fields of `projects`. */
export interface ProjectsFields {
  readonly name: Value;
  readonly owner: Value & MembersFields;
}

/** The fields of `projectMembers`. */
export interface ProjectMembersFields {
  readonly project: Value & ProjectsFields;
  readonly member: Value & MembersFields;
  readonly canWrite: Value;
}

/** The fields of `tasks`. */
export interface TasksFields {
  readonly project: Value & ProjectsFields;
  readonly assignee: Value & MembersFields;
  readonly title: Value;
  readonly status: Value;
}

/** The fields of `taskNotes`. */
export interface TaskNotesFields {
  readonly task: Value & TasksFields;
  readonly body: Value;
}

/** The fields of `boards`. */
export interface BoardsFields {
  readonly name: Value;
}

/** The fields of `cards`. */
export interface CardsFields {
  readonly board: Value & BoardsFields;
  readonly title: Value;
  readonly done: Value;
}

/** The fields of `folders`. */
export interface FoldersFields {
  readonly owner: Value & MembersFields;
  readonly name: Value;
  readonly parent: Value & FoldersFields;
}

/** The fields of `ledgerEntries`. */
export interface LedgerEntriesFields {
  readonly externalId: Value;
  readonly owner: Value & MembersFields;
  readonly amount: Value;
  readonly note: Value;
}

/** The fields of `coupons`. */
export interface CouponsFields {
  readonly code: Value;
  readonly slug: Value;
  readonly value: Value;
}

/** The fields of `boardsAccess`. */
export interface BoardsAccessFields {
  readonly board: Value & BoardsFields;
  readonly member: Value & MembersFields;
  readonly level: Value;
}

/**
 * Every field name in this deployment, with refs followed one hop.
 *
 * Field names are not unique across entities — the language scopes them to a source, and a schema-wide accessor cannot. Where two entities disagree about a name, the REF shape wins, because a value used as a scalar still type-checks and a scalar used as a ref does not. The compiler is the authority either way; this only decides what completes.
 */
export interface SchemaFields {
  readonly active: Value;
  readonly amount: Value;
  readonly assignee: Value & MembersFields;
  readonly board: Value & BoardsFields;
  readonly body: Value;
  readonly canWrite: Value;
  readonly category: Value & CategoriesFields;
  readonly code: Value;
  readonly customer: Value & CustomersFields;
  readonly displayName: Value;
  readonly done: Value;
  readonly email: Value;
  readonly externalId: Value;
  readonly group: Value & MembersFields;
  readonly joined: Value;
  readonly kind: Value;
  readonly level: Value;
  readonly member: Value & MembersFields;
  readonly memberCount: Value;
  readonly memo: Value;
  readonly name: Value;
  readonly note: Value;
  readonly order: Value & OrdersFields;
  readonly owner: Value & MembersFields;
  readonly parent: Value & CategoriesFields;
  readonly placed: Value;
  readonly price: Value;
  readonly principal: Value;
  readonly product: Value & ProductsFields;
  readonly project: Value & ProjectsFields;
  readonly qty: Value;
  readonly region: Value;
  readonly role: Value;
  readonly slug: Value;
  readonly status: Value;
  readonly task: Value & TasksFields;
  readonly tier: Value;
  readonly title: Value;
  readonly total: Value;
  readonly value: Value;
  readonly vendor: Value & VendorsFields;
}

/**
 * `f` — the same proxy the core exports, typed against this schema.
 *
 * The index signature stays, so a field the descriptor does not 
 * describe (an op's computed output, a name behind a scope the caller 
 * cannot see) is still reachable and still checked by the compiler 
 * rather than by this file.
 */
export const f = anyFields as unknown as SchemaFields & AnyFields;
