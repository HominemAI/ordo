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
  spec_addProduct,
  spec_allOrders,
  spec_catalogue,
  spec_editProduct,
  spec_grantShopsAccess,
  spec_markShipped,
  spec_me,
  spec_myOrders,
  spec_myShops,
  spec_openShop,
  spec_placeOrder,
  spec_productsWhere,
  spec_restock,
  spec_revokeShopsAccess,
  spec_setShopsAccessLevel,
  spec_staffOf,
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
  spec_std$products$put,
  spec_std$products$update,
  spec_std$shipments$count,
  spec_std$shipments$create,
  spec_std$shipments$delete,
  spec_std$shipments$get,
  spec_std$shipments$list,
  spec_std$shipments$put,
  spec_std$shipments$update,
  spec_std$shops$count,
  spec_std$shops$create,
  spec_std$shops$delete,
  spec_std$shops$get,
  spec_std$shops$list,
  spec_std$shops$update,
  spec_std$shopsAccess$count,
  spec_std$shopsAccess$create,
  spec_std$shopsAccess$delete,
  spec_std$shopsAccess$get,
  spec_std$shopsAccess$list,
  spec_std$shopsAccess$put,
  spec_std$shopsAccess$update,
} from './specs.js';
import { ReadCall, SingleReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
import type { OpenField } from '@hominemai/ordo-sdk/core/refine.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';
import type * as Ids from './ids.js';

/**
 * The row shape of `addProduct`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type AddProductRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `addProduct`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type AddProductParams = {
  /** `id` · fills `products.shop` */
  readonly shop: Ids.ShopsId;
  /** `text` · fills `products.sku` */
  readonly sku: string;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `text` · fills `products.category` */
  readonly category: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `int` · fills `products.stock` */
  readonly stock: number;
  /** `json` · fills `products.attributes` */
  readonly attributes: JsonValue;
};

/**
 * The refusals `addProduct` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AddProductRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `addProduct` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function addProduct(client: OrdoClient, params: AddProductParams): WriteCall<AddProductRow, AddProductRefusal> {
  return new WriteCall(client, spec_addProduct, params);
}

/**
 * One row of `allOrders`.
 */
export interface AllOrdersRow {
  /** `id` */
  readonly id: Ids.OrdersId;
  /** `int` */
  readonly quantity: number;
  /** `datetime` */
  readonly placed: string;
  /** `text` */
  readonly note: string;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `text` */
  readonly customer: string;
  /** `text` */
  readonly productName: string;
  /** `money` */
  readonly price: string;
  /** `money` */
  readonly total: string;
  /** `datetime` */
  readonly shipped: string;
  /** `text` */
  readonly carrier: string;
}

/** `allOrders` takes no parameters. */
export type AllOrdersParams = Record<never, never>;

/**
 * The fields a `.where` on `allOrders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface AllOrdersFields {
  readonly id: OpenField<'id', Ids.OrdersId>;
  readonly quantity: OpenField<'quantity', number>;
  readonly placed: OpenField<'placed', string>;
  readonly note: OpenField<'note', string>;
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly customer: OpenField<'customer', string>;
  readonly productName: OpenField<'productName', string>;
  readonly price: OpenField<'price', string>;
  readonly total: OpenField<'total', string>;
  readonly shipped: OpenField<'shipped', string>;
  readonly carrier: OpenField<'carrier', string>;
}

/**
 * The refusals `allOrders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type AllOrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `allOrders` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function allOrders(client: OrdoClient): ReadCall<AllOrdersRow, AllOrdersRefusal, AllOrdersFields> {
  return new ReadCall(client, spec_allOrders, {});
}

/**
 * One row of `catalogue`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface CatalogueRow {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `text` */
  readonly sku: string;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly category: string;
  /** `money` */
  readonly price: string;
  /** `int` */
  readonly stock: number;
  /** `json` */
  readonly attributes: JsonValue;
}

/** `catalogue` takes no parameters. */
export type CatalogueParams = Record<never, never>;

/**
 * The fields a `.where` on `catalogue` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface CatalogueFields {
  readonly id: OpenField<'id', Ids.ProductsId>;
  readonly sku: OpenField<'sku', string>;
  readonly name: OpenField<'name', string>;
  readonly category: OpenField<'category', string>;
  readonly price: OpenField<'price', string>;
  readonly stock: OpenField<'stock', number>;
}

/**
 * The refusals `catalogue` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type CatalogueRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `catalogue` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function catalogue(client: OrdoClient): ReadCall<CatalogueRow, CatalogueRefusal, CatalogueFields> {
  return new ReadCall(client, spec_catalogue, {});
}

/**
 * The row shape of `editProduct`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type EditProductRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `editProduct`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type EditProductParams = {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `text` · fills `products.category` */
  readonly category: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `bool` · fills `products.active` */
  readonly active: boolean;
  /** `json` · fills `products.attributes` */
  readonly attributes: JsonValue;
};

/**
 * The refusals `editProduct` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type EditProductRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `editProduct` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function editProduct(client: OrdoClient, params: EditProductParams): WriteCall<EditProductRow, EditProductRefusal> {
  return new WriteCall(client, spec_editProduct, params);
}

/**
 * The row shape of `grantShopsAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type GrantShopsAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `grantShopsAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type GrantShopsAccessParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `shopsAccess.level` */
  readonly level: 'admin' | 'staff';
};

/**
 * The refusals `grantShopsAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type GrantShopsAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `grantShopsAccess` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function grantShopsAccess(client: OrdoClient, params: GrantShopsAccessParams): WriteCall<GrantShopsAccessRow, GrantShopsAccessRefusal> {
  return new WriteCall(client, spec_grantShopsAccess, params);
}

/**
 * The row shape of `markShipped`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type MarkShippedRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `markShipped`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type MarkShippedParams = {
  /** `id` · fills `shipments.order` */
  readonly order: Ids.OrdersId;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly quantity: number;
  /** `datetime` · fills `shipments.shipped` */
  readonly shipped: string;
  /** `text` · fills `shipments.carrier` */
  readonly carrier: string;
};

/**
 * The refusals `markShipped` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MarkShippedRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `markShipped` — write.
 *
 * Mode: **creates + converges**.
 * Terminates in `.commit()`.
 */
export function markShipped(client: OrdoClient, params: MarkShippedParams): WriteCall<MarkShippedRow, MarkShippedRefusal> {
  return new WriteCall(client, spec_markShipped, params);
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
 * One row of `myOrders`.
 */
export interface MyOrdersRow {
  /** `id` */
  readonly id: Ids.OrdersId;
  /** `int` */
  readonly quantity: number;
  /** `datetime` */
  readonly placed: string;
  /** `text` */
  readonly note: string;
  /** `text` */
  readonly productName: string;
  /** `money` */
  readonly price: string;
  /** `money` */
  readonly total: string;
  /** `datetime` */
  readonly shipped: string;
  /** `text` */
  readonly carrier: string;
}

/** `myOrders` takes no parameters. */
export type MyOrdersParams = Record<never, never>;

/**
 * The fields a `.where` on `myOrders` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyOrdersFields {
  readonly id: OpenField<'id', Ids.OrdersId>;
  readonly quantity: OpenField<'quantity', number>;
  readonly placed: OpenField<'placed', string>;
  readonly note: OpenField<'note', string>;
  readonly productName: OpenField<'productName', string>;
  readonly price: OpenField<'price', string>;
  readonly total: OpenField<'total', string>;
  readonly shipped: OpenField<'shipped', string>;
  readonly carrier: OpenField<'carrier', string>;
}

/**
 * The refusals `myOrders` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyOrdersRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myOrders` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myOrders(client: OrdoClient): ReadCall<MyOrdersRow, MyOrdersRefusal, MyOrdersFields> {
  return new ReadCall(client, spec_myOrders, {});
}

/**
 * One row of `myShops`.
 *
 * Row identity: `shops`. `synthetic_rows` is false.
 */
export interface MyShopsRow {
  /** `id` */
  readonly id: Ids.ShopsId;
  /** `text` */
  readonly name: string;
}

/** `myShops` takes no parameters. */
export type MyShopsParams = Record<never, never>;

/**
 * The fields a `.where` on `myShops` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface MyShopsFields {
  readonly id: OpenField<'id', Ids.ShopsId>;
  readonly name: OpenField<'name', string>;
}

/**
 * The refusals `myShops` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type MyShopsRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `myShops` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function myShops(client: OrdoClient): ReadCall<MyShopsRow, MyShopsRefusal, MyShopsFields> {
  return new ReadCall(client, spec_myShops, {});
}

/**
 * The row shape of `openShop`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type OpenShopRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `openShop`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type OpenShopParams = {
  /** `text` · fills `shops.name` */
  readonly name: string;
};

/**
 * The refusals `openShop` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type OpenShopRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `openShop` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function openShop(client: OrdoClient, params: OpenShopParams): WriteCall<OpenShopRow, OpenShopRefusal> {
  return new WriteCall(client, spec_openShop, params);
}

/**
 * The row shape of `placeOrder`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type PlaceOrderRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `placeOrder`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type PlaceOrderParams = {
  /** `id` · fills `orders.product` */
  readonly product: Ids.ProductsId;
  /** `int` · fills `orders.quantity` */
  readonly quantity: number;
  /** `datetime` · fills `orders.placed` */
  readonly placed: string;
  /** `text` · fills `orders.note` */
  readonly note: string;
};

/**
 * The refusals `placeOrder` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type PlaceOrderRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `placeOrder` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function placeOrder(client: OrdoClient, params: PlaceOrderParams): WriteCall<PlaceOrderRow, PlaceOrderRefusal> {
  return new WriteCall(client, spec_placeOrder, params);
}

/**
 * One row of `productsWhere`.
 *
 * Row identity: `products`. `synthetic_rows` is false.
 */
export interface ProductsWhereRow {
  /** `id` */
  readonly id: Ids.ProductsId;
  /** `text` */
  readonly sku: string;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly category: string;
  /** `money` */
  readonly price: string;
  /** `int` */
  readonly stock: number;
  /** `json` */
  readonly attributes: JsonValue;
}

/**
 * The parameters of `productsWhere`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type ProductsWhereParams = {
  /** `text` */
  readonly color: string;
};

/**
 * The fields a `.where` on `productsWhere` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface ProductsWhereFields {
  readonly id: OpenField<'id', Ids.ProductsId>;
  readonly sku: OpenField<'sku', string>;
  readonly name: OpenField<'name', string>;
  readonly category: OpenField<'category', string>;
  readonly price: OpenField<'price', string>;
  readonly stock: OpenField<'stock', number>;
}

/**
 * The refusals `productsWhere` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type ProductsWhereRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `productsWhere` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function productsWhere(client: OrdoClient, params: ProductsWhereParams): ReadCall<ProductsWhereRow, ProductsWhereRefusal, ProductsWhereFields> {
  return new ReadCall(client, spec_productsWhere, params);
}

/**
 * The row shape of `restock`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RestockRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `restock`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RestockParams = {
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly by: number;
};

/**
 * The refusals `restock` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RestockRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `restock` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function restock(client: OrdoClient, params: RestockParams): WriteCall<RestockRow, RestockRefusal> {
  return new WriteCall(client, spec_restock, params);
}

/**
 * The row shape of `revokeShopsAccess`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type RevokeShopsAccessRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `revokeShopsAccess`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type RevokeShopsAccessParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
};

/**
 * The refusals `revokeShopsAccess` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type RevokeShopsAccessRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `revokeShopsAccess` — delete.
 *
 * Mode: **bulk deletes**.
 * Terminates in `.commit()`.
 */
export function revokeShopsAccess(client: OrdoClient, params: RevokeShopsAccessParams): WriteCall<RevokeShopsAccessRow, RevokeShopsAccessRefusal> {
  return new WriteCall(client, spec_revokeShopsAccess, params);
}

/**
 * The row shape of `setShopsAccessLevel`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type SetShopsAccessLevelRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `setShopsAccessLevel`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type SetShopsAccessLevelParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `shopsAccess.level` */
  readonly level: 'admin' | 'staff';
};

/**
 * The refusals `setShopsAccessLevel` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type SetShopsAccessLevelRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `setShopsAccessLevel` — write.
 *
 * Mode: **bulk**.
 * Terminates in `.commit()`.
 */
export function setShopsAccessLevel(client: OrdoClient, params: SetShopsAccessLevelParams): WriteCall<SetShopsAccessLevelRow, SetShopsAccessLevelRefusal> {
  return new WriteCall(client, spec_setShopsAccessLevel, params);
}

/**
 * One row of `staffOf`.
 *
 * Row identity: `shopsAccess`. `synthetic_rows` is false.
 */
export interface StaffOfRow {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly level: string;
}

/**
 * The parameters of `staffOf`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StaffOfParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
};

/**
 * The fields a `.where` on `staffOf` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StaffOfFields {
  readonly id: OpenField<'id', Ids.ShopsAccessId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly name: OpenField<'name', string>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `staffOf` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StaffOfRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `staffOf` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function staffOf(client: OrdoClient, params: StaffOfParams): ReadCall<StaffOfRow, StaffOfRefusal, StaffOfFields> {
  return new ReadCall(client, spec_staffOf, params);
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
  readonly customer: Ids.MembersId;
  /** `id` · fills `orders.product` */
  readonly product: Ids.ProductsId;
  /** `int` · fills `orders.quantity` */
  readonly quantity: number;
  /** `datetime` · fills `orders.placed` */
  readonly placed: string;
  /** `text` · fills `orders.note` · has a default; omit to take it */
  readonly note?: string;
};

/**
 * The refusals `std.orders.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
export type StdOrdersDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
  readonly customer: Ids.MembersId;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly quantity: number;
  /** `datetime` */
  readonly placed: string;
  /** `text` */
  readonly note: string;
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
  readonly customer: OpenField<'customer', Ids.MembersId>;
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly quantity: OpenField<'quantity', number>;
  readonly placed: OpenField<'placed', string>;
  readonly note: OpenField<'note', string>;
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
  readonly customer: Ids.MembersId;
  /** `id` */
  readonly product: Ids.ProductsId;
  /** `int` */
  readonly quantity: number;
  /** `datetime` */
  readonly placed: string;
  /** `text` */
  readonly note: string;
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
  readonly customer: OpenField<'customer', Ids.MembersId>;
  readonly product: OpenField<'product', Ids.ProductsId>;
  readonly quantity: OpenField<'quantity', number>;
  readonly placed: OpenField<'placed', string>;
  readonly note: OpenField<'note', string>;
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
  readonly customer: Ids.MembersId;
  /** `id` · fills `orders.product` */
  readonly product: Ids.ProductsId;
  /** `int` · fills `orders.quantity` */
  readonly quantity: number;
  /** `datetime` · fills `orders.placed` */
  readonly placed: string;
  /** `text` · fills `orders.note` */
  readonly note: string;
};

/**
 * The refusals `std.orders.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdOrdersUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
  /** `id` · fills `products.shop` */
  readonly shop: Ids.ShopsId;
  /** `text` · fills `products.sku` */
  readonly sku: string;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `text` · fills `products.category` */
  readonly category: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `int` · fills `products.stock` · has a default; omit to take it */
  readonly stock?: number;
  /** `bool` · fills `products.active` · has a default; omit to take it */
  readonly active?: boolean;
  /** `json` · fills `products.attributes` · has a default; omit to take it */
  readonly attributes?: JsonValue;
};

/**
 * The refusals `std.products.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
export type StdProductsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'duplicate' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
  readonly shop: Ids.ShopsId;
  /** `text` */
  readonly sku: string;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly category: string;
  /** `money` */
  readonly price: string;
  /** `int` */
  readonly stock: number;
  /** `bool` */
  readonly active: boolean;
  /** `json` */
  readonly attributes: JsonValue;
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
  readonly shop: OpenField<'shop', Ids.ShopsId>;
  readonly sku: OpenField<'sku', string>;
  readonly name: OpenField<'name', string>;
  readonly category: OpenField<'category', string>;
  readonly price: OpenField<'price', string>;
  readonly stock: OpenField<'stock', number>;
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
  readonly shop: Ids.ShopsId;
  /** `text` */
  readonly sku: string;
  /** `text` */
  readonly name: string;
  /** `text` */
  readonly category: string;
  /** `money` */
  readonly price: string;
  /** `int` */
  readonly stock: number;
  /** `bool` */
  readonly active: boolean;
  /** `json` */
  readonly attributes: JsonValue;
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
  readonly shop: OpenField<'shop', Ids.ShopsId>;
  readonly sku: OpenField<'sku', string>;
  readonly name: OpenField<'name', string>;
  readonly category: OpenField<'category', string>;
  readonly price: OpenField<'price', string>;
  readonly stock: OpenField<'stock', number>;
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
 * The row shape of `std.products.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdProductsPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.products.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdProductsPutParams = {
  /** `id` · fills `products.shop` */
  readonly shop: Ids.ShopsId;
  /** `text` · fills `products.sku` */
  readonly sku: string;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `text` · fills `products.category` */
  readonly category: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `int` · fills `products.stock` · has a default; omit to take it */
  readonly stock?: number;
  /** `bool` · fills `products.active` · has a default; omit to take it */
  readonly active?: boolean;
  /** `json` · fills `products.attributes` · has a default; omit to take it */
  readonly attributes?: JsonValue;
};

/**
 * The refusals `std.products.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'check_violation' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.products.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$products$put(client: OrdoClient, params: StdProductsPutParams): WriteCall<StdProductsPutRow, StdProductsPutRefusal> {
  return new WriteCall(client, spec_std$products$put, params);
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
  /** `id` · fills `products.shop` */
  readonly shop: Ids.ShopsId;
  /** `text` · fills `products.sku` */
  readonly sku: string;
  /** `text` · fills `products.name` */
  readonly name: string;
  /** `text` · fills `products.category` */
  readonly category: string;
  /** `money` · fills `products.price` */
  readonly price: string;
  /** `int` · fills `products.stock` */
  readonly stock: number;
  /** `bool` · fills `products.active` */
  readonly active: boolean;
  /** `json` · fills `products.attributes` */
  readonly attributes: JsonValue;
};

/**
 * The refusals `std.products.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdProductsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'check_violation' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

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
 * One row of `std.shipments.count`.
 */
export interface StdShipmentsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.shipments.count` takes no parameters. */
export type StdShipmentsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shipments.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdShipmentsCountFields = Record<never, never>;

/**
 * The refusals `std.shipments.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shipments$count(client: OrdoClient): SingleReadCall<StdShipmentsCountRow, StdShipmentsCountRefusal, StdShipmentsCountFields> {
  return new SingleReadCall(client, spec_std$shipments$count, {});
}

/**
 * The row shape of `std.shipments.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShipmentsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shipments.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShipmentsCreateParams = {
  /** `id` · fills `shipments.order` */
  readonly order: Ids.OrdersId;
  /** `datetime` · fills `shipments.shipped` */
  readonly shipped: string;
  /** `text` · fills `shipments.carrier` · has a default; omit to take it */
  readonly carrier?: string;
};

/**
 * The refusals `std.shipments.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$shipments$create(client: OrdoClient, params: StdShipmentsCreateParams): WriteCall<StdShipmentsCreateRow, StdShipmentsCreateRefusal> {
  return new WriteCall(client, spec_std$shipments$create, params);
}

/**
 * The row shape of `std.shipments.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShipmentsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shipments.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShipmentsDeleteParams = {
  /** `id` */
  readonly id: Ids.ShipmentsId;
};

/**
 * The refusals `std.shipments.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$shipments$delete(client: OrdoClient, params: StdShipmentsDeleteParams): WriteCall<StdShipmentsDeleteRow, StdShipmentsDeleteRefusal> {
  return new WriteCall(client, spec_std$shipments$delete, params);
}

/**
 * One row of `std.shipments.get`.
 *
 * Row identity: `shipments`. `synthetic_rows` is false.
 */
export interface StdShipmentsGetRow {
  /** `id` */
  readonly id: Ids.ShipmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly order: Ids.OrdersId;
  /** `datetime` */
  readonly shipped: string;
  /** `text` */
  readonly carrier: string;
}

/**
 * The parameters of `std.shipments.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShipmentsGetParams = {
  /** `id` */
  readonly id: Ids.ShipmentsId;
};

/**
 * The fields a `.where` on `std.shipments.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShipmentsGetFields {
  readonly id: OpenField<'id', Ids.ShipmentsId>;
  readonly rev: OpenField<'rev', number>;
  readonly order: OpenField<'order', Ids.OrdersId>;
  readonly shipped: OpenField<'shipped', string>;
  readonly carrier: OpenField<'carrier', string>;
}

/**
 * The refusals `std.shipments.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shipments$get(client: OrdoClient, params: StdShipmentsGetParams): ReadCall<StdShipmentsGetRow, StdShipmentsGetRefusal, StdShipmentsGetFields> {
  return new ReadCall(client, spec_std$shipments$get, params);
}

/**
 * One row of `std.shipments.list`.
 *
 * Row identity: `shipments`. `synthetic_rows` is false.
 */
export interface StdShipmentsListRow {
  /** `id` */
  readonly id: Ids.ShipmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly order: Ids.OrdersId;
  /** `datetime` */
  readonly shipped: string;
  /** `text` */
  readonly carrier: string;
}

/** `std.shipments.list` takes no parameters. */
export type StdShipmentsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shipments.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShipmentsListFields {
  readonly id: OpenField<'id', Ids.ShipmentsId>;
  readonly rev: OpenField<'rev', number>;
  readonly order: OpenField<'order', Ids.OrdersId>;
  readonly shipped: OpenField<'shipped', string>;
  readonly carrier: OpenField<'carrier', string>;
}

/**
 * The refusals `std.shipments.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shipments$list(client: OrdoClient): ReadCall<StdShipmentsListRow, StdShipmentsListRefusal, StdShipmentsListFields> {
  return new ReadCall(client, spec_std$shipments$list, {});
}

/**
 * The row shape of `std.shipments.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShipmentsPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shipments.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShipmentsPutParams = {
  /** `id` · fills `shipments.order` */
  readonly order: Ids.OrdersId;
  /** `datetime` · fills `shipments.shipped` */
  readonly shipped: string;
  /** `text` · fills `shipments.carrier` · has a default; omit to take it */
  readonly carrier?: string;
};

/**
 * The refusals `std.shipments.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$shipments$put(client: OrdoClient, params: StdShipmentsPutParams): WriteCall<StdShipmentsPutRow, StdShipmentsPutRefusal> {
  return new WriteCall(client, spec_std$shipments$put, params);
}

/**
 * The row shape of `std.shipments.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShipmentsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shipments.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShipmentsUpdateParams = {
  /** `id` */
  readonly id: Ids.ShipmentsId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `shipments.order` */
  readonly order: Ids.OrdersId;
  /** `datetime` · fills `shipments.shipped` */
  readonly shipped: string;
  /** `text` · fills `shipments.carrier` */
  readonly carrier: string;
};

/**
 * The refusals `std.shipments.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShipmentsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shipments.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$shipments$update(client: OrdoClient, params: StdShipmentsUpdateParams): WriteCall<StdShipmentsUpdateRow, StdShipmentsUpdateRefusal> {
  return new WriteCall(client, spec_std$shipments$update, params);
}

/**
 * One row of `std.shops.count`.
 */
export interface StdShopsCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.shops.count` takes no parameters. */
export type StdShopsCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shops.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdShopsCountFields = Record<never, never>;

/**
 * The refusals `std.shops.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shops$count(client: OrdoClient): SingleReadCall<StdShopsCountRow, StdShopsCountRefusal, StdShopsCountFields> {
  return new SingleReadCall(client, spec_std$shops$count, {});
}

/**
 * The row shape of `std.shops.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shops.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsCreateParams = {
  /** `text` · fills `shops.name` */
  readonly name: string;
};

/**
 * The refusals `std.shops.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$shops$create(client: OrdoClient, params: StdShopsCreateParams): WriteCall<StdShopsCreateRow, StdShopsCreateRefusal> {
  return new WriteCall(client, spec_std$shops$create, params);
}

/**
 * The row shape of `std.shops.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shops.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsDeleteParams = {
  /** `id` */
  readonly id: Ids.ShopsId;
};

/**
 * The refusals `std.shops.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'has_referrers' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$shops$delete(client: OrdoClient, params: StdShopsDeleteParams): WriteCall<StdShopsDeleteRow, StdShopsDeleteRefusal> {
  return new WriteCall(client, spec_std$shops$delete, params);
}

/**
 * One row of `std.shops.get`.
 *
 * Row identity: `shops`. `synthetic_rows` is false.
 */
export interface StdShopsGetRow {
  /** `id` */
  readonly id: Ids.ShopsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
}

/**
 * The parameters of `std.shops.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsGetParams = {
  /** `id` */
  readonly id: Ids.ShopsId;
};

/**
 * The fields a `.where` on `std.shops.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShopsGetFields {
  readonly id: OpenField<'id', Ids.ShopsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
}

/**
 * The refusals `std.shops.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shops$get(client: OrdoClient, params: StdShopsGetParams): ReadCall<StdShopsGetRow, StdShopsGetRefusal, StdShopsGetFields> {
  return new ReadCall(client, spec_std$shops$get, params);
}

/**
 * One row of `std.shops.list`.
 *
 * Row identity: `shops`. `synthetic_rows` is false.
 */
export interface StdShopsListRow {
  /** `id` */
  readonly id: Ids.ShopsId;
  /** `int` */
  readonly rev: number;
  /** `text` */
  readonly name: string;
}

/** `std.shops.list` takes no parameters. */
export type StdShopsListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shops.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShopsListFields {
  readonly id: OpenField<'id', Ids.ShopsId>;
  readonly rev: OpenField<'rev', number>;
  readonly name: OpenField<'name', string>;
}

/**
 * The refusals `std.shops.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shops$list(client: OrdoClient): ReadCall<StdShopsListRow, StdShopsListRefusal, StdShopsListFields> {
  return new ReadCall(client, spec_std$shops$list, {});
}

/**
 * The row shape of `std.shops.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shops.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsUpdateParams = {
  /** `id` */
  readonly id: Ids.ShopsId;
  /** `int` */
  readonly rev: number;
  /** `text` · fills `shops.name` */
  readonly name: string;
};

/**
 * The refusals `std.shops.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shops.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$shops$update(client: OrdoClient, params: StdShopsUpdateParams): WriteCall<StdShopsUpdateRow, StdShopsUpdateRefusal> {
  return new WriteCall(client, spec_std$shops$update, params);
}

/**
 * One row of `std.shopsAccess.count`.
 */
export interface StdShopsAccessCountRow {
  /** `int` */
  readonly n: number;
}

/** `std.shopsAccess.count` takes no parameters. */
export type StdShopsAccessCountParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shopsAccess.count` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export type StdShopsAccessCountFields = Record<never, never>;

/**
 * The refusals `std.shopsAccess.count` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessCountRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.count` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shopsAccess$count(client: OrdoClient): SingleReadCall<StdShopsAccessCountRow, StdShopsAccessCountRefusal, StdShopsAccessCountFields> {
  return new SingleReadCall(client, spec_std$shopsAccess$count, {});
}

/**
 * The row shape of `std.shopsAccess.create`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsAccessCreateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shopsAccess.create`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsAccessCreateParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `shopsAccess.level` */
  readonly level: 'admin' | 'staff';
};

/**
 * The refusals `std.shopsAccess.create` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessCreateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.create` — write.
 *
 * Mode: **creates**.
 * Terminates in `.commit()`.
 */
export function std$shopsAccess$create(client: OrdoClient, params: StdShopsAccessCreateParams): WriteCall<StdShopsAccessCreateRow, StdShopsAccessCreateRefusal> {
  return new WriteCall(client, spec_std$shopsAccess$create, params);
}

/**
 * The row shape of `std.shopsAccess.delete`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsAccessDeleteRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shopsAccess.delete`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsAccessDeleteParams = {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
};

/**
 * The refusals `std.shopsAccess.delete` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessDeleteRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.delete` — delete.
 *
 * Mode: **deletes**.
 * Terminates in `.commit()`.
 */
export function std$shopsAccess$delete(client: OrdoClient, params: StdShopsAccessDeleteParams): WriteCall<StdShopsAccessDeleteRow, StdShopsAccessDeleteRefusal> {
  return new WriteCall(client, spec_std$shopsAccess$delete, params);
}

/**
 * One row of `std.shopsAccess.get`.
 *
 * Row identity: `shopsAccess`. `synthetic_rows` is false.
 */
export interface StdShopsAccessGetRow {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly shop: Ids.ShopsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/**
 * The parameters of `std.shopsAccess.get`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsAccessGetParams = {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
};

/**
 * The fields a `.where` on `std.shopsAccess.get` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShopsAccessGetFields {
  readonly id: OpenField<'id', Ids.ShopsAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly shop: OpenField<'shop', Ids.ShopsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.shopsAccess.get` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessGetRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_param' | 'invalid_refinement' | 'method_mismatch' | 'missing_param' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.get` — read.
 * Cost class `sql`, rows: <= 1.
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shopsAccess$get(client: OrdoClient, params: StdShopsAccessGetParams): ReadCall<StdShopsAccessGetRow, StdShopsAccessGetRefusal, StdShopsAccessGetFields> {
  return new ReadCall(client, spec_std$shopsAccess$get, params);
}

/**
 * One row of `std.shopsAccess.list`.
 *
 * Row identity: `shopsAccess`. `synthetic_rows` is false.
 */
export interface StdShopsAccessListRow {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` */
  readonly shop: Ids.ShopsId;
  /** `id` */
  readonly member: Ids.MembersId;
  /** `text` */
  readonly level: string;
}

/** `std.shopsAccess.list` takes no parameters. */
export type StdShopsAccessListParams = Record<never, never>;

/**
 * The fields a `.where` on `std.shopsAccess.list` may test.
 *
 * One per output in `refine.fields`, typed as the row types it. An open field takes all 8 operators; a sealed one takes `eq` and `in`, because a sealed condition rides the blind index. An output that is not here is not refinable, and naming it does not compile.
 */
export interface StdShopsAccessListFields {
  readonly id: OpenField<'id', Ids.ShopsAccessId>;
  readonly rev: OpenField<'rev', number>;
  readonly shop: OpenField<'shop', Ids.ShopsId>;
  readonly member: OpenField<'member', Ids.MembersId>;
  readonly level: OpenField<'level', string>;
}

/**
 * The refusals `std.shopsAccess.list` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessListRefusal = Extract<RefusalClass, 'access_denied' | 'invalid_cursor' | 'invalid_refinement' | 'method_mismatch' | 'refinement_unbounded' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.list` — read.
 * Cost class `sql`, rows: unbounded (pageable).
 * Terminates in `.fetch()` / `.live()` / `.pages()`.
 * `.live()` is **not generated**: the descriptor carries no log position on the read path, so a subscription cannot be ordered against its own first frame. See the package README.
 * Narrow it with `.where(v => …)` and `.pick(…)` before the verb.
 */
export function std$shopsAccess$list(client: OrdoClient): ReadCall<StdShopsAccessListRow, StdShopsAccessListRefusal, StdShopsAccessListFields> {
  return new ReadCall(client, spec_std$shopsAccess$list, {});
}

/**
 * The row shape of `std.shopsAccess.put`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsAccessPutRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shopsAccess.put`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsAccessPutParams = {
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `shopsAccess.level` */
  readonly level: 'admin' | 'staff';
};

/**
 * The refusals `std.shopsAccess.put` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessPutRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'conflicting_identity' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.put` — write.
 *
 * Mode: **creates-or-converges**.
 * Terminates in `.commit()`.
 */
export function std$shopsAccess$put(client: OrdoClient, params: StdShopsAccessPutParams): WriteCall<StdShopsAccessPutRow, StdShopsAccessPutRefusal> {
  return new WriteCall(client, spec_std$shopsAccess$put, params);
}

/**
 * The row shape of `std.shopsAccess.update`.
 *
 * A write's rows are the summary `api-engine-design.md` §3 describes; the descriptor declares no output shape for it (`outputs: []`), so this is the honest type rather than an invented one.
 */
export type StdShopsAccessUpdateRow = Readonly<Record<string, unknown>>;

/**
 * The parameters of `std.shopsAccess.update`.
 *
 * A type alias rather than an `interface`, deliberately: only an object type alias carries an implicit index signature, and the client's `request` takes the parameters as a record. An `interface` here would need a cast at every generated call site — a hole in exactly the code nobody reviews.
 */
export type StdShopsAccessUpdateParams = {
  /** `id` */
  readonly id: Ids.ShopsAccessId;
  /** `int` */
  readonly rev: number;
  /** `id` · fills `shopsAccess.shop` */
  readonly shop: Ids.ShopsId;
  /** `id` · fills `shopsAccess.member` */
  readonly member: Ids.MembersId;
  /** `text` · fills `shopsAccess.level` */
  readonly level: 'admin' | 'staff';
};

/**
 * The refusals `std.shopsAccess.update` declares.
 *
 * The op's own set, not the whole table. The door has refusals that precede any op (`missing_space`, `caller_supplied`, `delegation_required`), so a caught refusal outside this union is not a bug — it is the door speaking before the op did, and `Refusal.declared` says which happened.
 */
export type StdShopsAccessUpdateRefusal = Extract<RefusalClass, 'access_denied' | 'access_guard' | 'cas_miss' | 'duplicate' | 'idempotency_in_flight' | 'idempotency_mismatch' | 'invalid_param' | 'log_append_failed' | 'method_mismatch' | 'missing_param' | 'not_found' | 'serialization_conflict' | 'unknown_param' | 'version_skew'>;

/**
 * `std.shopsAccess.update` — write.
 *
 * Mode: **converges**.
 * Terminates in `.commit()`.
 */
export function std$shopsAccess$update(client: OrdoClient, params: StdShopsAccessUpdateParams): WriteCall<StdShopsAccessUpdateRow, StdShopsAccessUpdateRefusal> {
  return new WriteCall(client, spec_std$shopsAccess$update, params);
}

/**
 * Every op, keyed by its real (possibly dotted) name.
 *
 * For dispatch that is dynamic on the caller's side. Prefer the named exports where the op is known at author time — they tree-shake and this object does not.
 */
export const ops = {
  addProduct: addProduct,
  allOrders: allOrders,
  catalogue: catalogue,
  editProduct: editProduct,
  grantShopsAccess: grantShopsAccess,
  markShipped: markShipped,
  me: me,
  myOrders: myOrders,
  myShops: myShops,
  openShop: openShop,
  placeOrder: placeOrder,
  productsWhere: productsWhere,
  restock: restock,
  revokeShopsAccess: revokeShopsAccess,
  setShopsAccessLevel: setShopsAccessLevel,
  staffOf: staffOf,
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
  'std.products.put': std$products$put,
  'std.products.update': std$products$update,
  'std.shipments.count': std$shipments$count,
  'std.shipments.create': std$shipments$create,
  'std.shipments.delete': std$shipments$delete,
  'std.shipments.get': std$shipments$get,
  'std.shipments.list': std$shipments$list,
  'std.shipments.put': std$shipments$put,
  'std.shipments.update': std$shipments$update,
  'std.shops.count': std$shops$count,
  'std.shops.create': std$shops$create,
  'std.shops.delete': std$shops$delete,
  'std.shops.get': std$shops$get,
  'std.shops.list': std$shops$list,
  'std.shops.update': std$shops$update,
  'std.shopsAccess.count': std$shopsAccess$count,
  'std.shopsAccess.create': std$shopsAccess$create,
  'std.shopsAccess.delete': std$shopsAccess$delete,
  'std.shopsAccess.get': std$shopsAccess$get,
  'std.shopsAccess.list': std$shopsAccess$list,
  'std.shopsAccess.put': std$shopsAccess$put,
  'std.shopsAccess.update': std$shopsAccess$update,
} as const;
