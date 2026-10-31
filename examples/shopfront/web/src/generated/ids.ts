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
 * An id addressing `groupMembers`.
 */
export type GroupMembersId = Branded<string, 'groupMembers'>;

/**
 * An id addressing `shops`.
 *
 * Typed-id prefix `shop`, so a value looks like `shop_…`. The brand is compile-time only; the wire sees the string.
 */
export type ShopsId = Branded<string, 'shops'>;

/**
 * An id addressing `products`.
 *
 * Typed-id prefix `prod`, so a value looks like `prod_…`. The brand is compile-time only; the wire sees the string.
 */
export type ProductsId = Branded<string, 'products'>;

/**
 * An id addressing `orders`.
 *
 * Typed-id prefix `orde`, so a value looks like `orde_…`. The brand is compile-time only; the wire sees the string.
 */
export type OrdersId = Branded<string, 'orders'>;

/**
 * An id addressing `shipments`.
 *
 * Typed-id prefix `ship`, so a value looks like `ship_…`. The brand is compile-time only; the wire sees the string.
 */
export type ShipmentsId = Branded<string, 'shipments'>;

/**
 * An id addressing `shopsAccess`.
 *
 * Typed-id prefix `shop-acce`, so a value looks like `shop-acce_…`. The brand is compile-time only; the wire sees the string.
 */
export type ShopsAccessId = Branded<string, 'shopsAccess'>;

