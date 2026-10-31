// GENERATED — do not edit; regenerate with `ordo gen ts`.
//
// The op table: every fact a call needs, as data for the static core.
//
// Source of truth: the deployment's `interface.json` (`ordo.interface/2`), itself a projection of `op_artifact`
//
// This file is the GENERATED half of this package. The hand-written
// half is under `src/core/` and is versioned by hand; the boundary
// between them is documented in the package README. An edit here is
// lost the next time the generator runs.

import type { OpSpec } from '@hominemai/ordo-sdk/core/spec.js';

/**
 * The descriptor this file was generated from.
 *
 * `digest` identifies the whole document and `domain` names the program. A client sends neither: the version pin is per-op (`api-engine-design.md` §10), because a registry-wide pin marks every call stale after any unrelated deploy and a signal that fires constantly is one clients learn to ignore. These are here so a bug report can say which descriptor.
 */
export const DESCRIPTOR = {
  schemaVersion: 'ordo.interface/2',
  domain: 'shopfront',
  digest: '786729e18a4cf43c',
} as const;

export const spec_addProduct = {
    op: 'addProduct',
    version: '29cfb92463d3a56c',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'sku', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 2 },
      { name: 'stock', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'attributes', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_allOrders = {
    op: 'allOrders',
    version: '52a35d946c92d9e9',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: { keys: [{ path: 'placed', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'quantity', 'placed', 'note', 'product', 'customer', 'productName', 'price', 'total', 'shipped', 'carrier'], decimals: ['price', 'total'], types: { id: 'id', quantity: 'int', placed: 'datetime', note: 'text', product: 'id', customer: 'text', productName: 'text', price: 'money', total: 'money', shipped: 'datetime', carrier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_catalogue = {
    op: 'catalogue',
    version: 'a9e2fb56e8fb1d81',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'category', desc: false }, { path: 'name', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'sku', 'name', 'category', 'price', 'stock'], decimals: ['price'], types: { id: 'id', sku: 'text', name: 'text', category: 'text', price: 'money', stock: 'int', attributes: 'json' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_editProduct = {
    op: 'editProduct',
    version: 'a719081047a53e6e',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 2 },
      { name: 'active', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'attributes', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_grantShopsAccess = {
    op: 'grantShopsAccess',
    version: '394eed88320d0037',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['staff', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_markShipped = {
    op: 'markShipped',
    version: 'd43bce720e67717a',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'order', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'quantity', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'shipped', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'carrier', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_me = {
    op: 'me',
    version: '0f835e6886837cd5',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'members', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'displayName', 'role'], decimals: [], types: { id: 'id', displayName: 'text', role: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myOrders = {
    op: 'myOrders',
    version: '50e9b9cb78bb0287',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: { keys: [{ path: 'placed', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'quantity', 'placed', 'note', 'productName', 'price', 'total', 'shipped', 'carrier'], decimals: ['price', 'total'], types: { id: 'id', quantity: 'int', placed: 'datetime', note: 'text', productName: 'text', price: 'money', total: 'money', shipped: 'datetime', carrier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myShops = {
    op: 'myShops',
    version: 'aafe9c86d97ec63f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shops', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'name'], decimals: [], types: { id: 'id', name: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_openShop = {
    op: 'openShop',
    version: 'e1735770f65e0494',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_placeOrder = {
    op: 'placeOrder',
    version: '8adc9ddea6fd42ac',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'quantity', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'placed', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_productsWhere = {
    op: 'productsWhere',
    version: '6ae3f549dda1cbf7',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'name', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'sku', 'name', 'category', 'price', 'stock'], decimals: ['price'], types: { id: 'id', sku: 'text', name: 'text', category: 'text', price: 'money', stock: 'int', attributes: 'json' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'color', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_restock = {
    op: 'restock',
    version: 'b2e1940d0d65aa18',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'by', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_revokeShopsAccess = {
    op: 'revokeShopsAccess',
    version: '2d87e3a21b9d957e',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_setShopsAccessLevel = {
    op: 'setShopsAccessLevel',
    version: '6b2766e1a2ca8a45',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['staff', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_staffOf = {
    op: 'staffOf',
    version: '663283275945c84e',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shopsAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'member', 'name', 'level'], decimals: [], types: { id: 'id', member: 'id', name: 'text', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$count = {
    op: 'std.orders.count',
    version: 'c39a260fb4559feb',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'product', 'quantity', 'placed', 'note'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$create = {
    op: 'std.orders.create',
    version: '8f1826c79a0b3979',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'customer', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'quantity', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'placed', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$delete = {
    op: 'std.orders.delete',
    version: 'c6f71483f4a53b44',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$get = {
    op: 'std.orders.get',
    version: '2d73cc9f59e60c3d',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'product', 'quantity', 'placed', 'note'], decimals: [], types: { id: 'id', rev: 'int', customer: 'id', product: 'id', quantity: 'int', placed: 'datetime', note: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$list = {
    op: 'std.orders.list',
    version: 'd06e852801c41c35',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'product', 'quantity', 'placed', 'note'], decimals: [], types: { id: 'id', rev: 'int', customer: 'id', product: 'id', quantity: 'int', placed: 'datetime', note: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$update = {
    op: 'std.orders.update',
    version: '8cdcbee690079f37',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'customer', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'quantity', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'placed', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$count = {
    op: 'std.products.count',
    version: '0a084cc9f08d1378',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'sku', 'name', 'category', 'price', 'stock', 'active'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$create = {
    op: 'std.products.create',
    version: '22f0c7cde0d8bddf',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'sku', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 2 },
      { name: 'stock', type: 'int', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'active', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'attributes', type: 'json', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$delete = {
    op: 'std.products.delete',
    version: '848c83dfe07ca49a',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$get = {
    op: 'std.products.get',
    version: '3981e6d198a42980',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'sku', 'name', 'category', 'price', 'stock', 'active'], decimals: ['price'], types: { id: 'id', rev: 'int', shop: 'id', sku: 'text', name: 'text', category: 'text', price: 'money', stock: 'int', active: 'bool', attributes: 'json' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$list = {
    op: 'std.products.list',
    version: '1fdf43849b3e80fc',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'sku', 'name', 'category', 'price', 'stock', 'active'], decimals: ['price'], types: { id: 'id', rev: 'int', shop: 'id', sku: 'text', name: 'text', category: 'text', price: 'money', stock: 'int', active: 'bool', attributes: 'json' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$put = {
    op: 'std.products.put',
    version: '954cd59143f1f8c4',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'sku', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 2 },
      { name: 'stock', type: 'int', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'active', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'attributes', type: 'json', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$update = {
    op: 'std.products.update',
    version: '6f3101bda4f4f478',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'sku', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 2 },
      { name: 'stock', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'active', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'attributes', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$count = {
    op: 'std.shipments.count',
    version: '86762375198c8dab',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'shipped', 'carrier'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$create = {
    op: 'std.shipments.create',
    version: '906181f8c5914c64',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'order', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'shipped', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'carrier', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$delete = {
    op: 'std.shipments.delete',
    version: '20f06f35a4204bdd',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shipments', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$get = {
    op: 'std.shipments.get',
    version: '43c9574e6ec6cd5f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shipments', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'shipped', 'carrier'], decimals: [], types: { id: 'id', rev: 'int', order: 'id', shipped: 'datetime', carrier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shipments', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$list = {
    op: 'std.shipments.list',
    version: '06c85bca184c6589',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shipments', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'shipped', 'carrier'], decimals: [], types: { id: 'id', rev: 'int', order: 'id', shipped: 'datetime', carrier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$put = {
    op: 'std.shipments.put',
    version: 'f064202683d22cc4',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'order', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'shipped', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'carrier', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shipments$update = {
    op: 'std.shipments.update',
    version: '4de77c3e3e697a6f',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shipments', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'order', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'shipped', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'carrier', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$count = {
    op: 'std.shops.count',
    version: '555c7d2e458819af',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$create = {
    op: 'std.shops.create',
    version: '1aa4440381b9ed13',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$delete = {
    op: 'std.shops.delete',
    version: '4ae106badba6e8a0',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$get = {
    op: 'std.shops.get',
    version: '8fe81bf2081da321',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shops', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name'], decimals: [], types: { id: 'id', rev: 'int', name: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$list = {
    op: 'std.shops.list',
    version: '3bd8673f0a81b83e',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shops', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name'], decimals: [], types: { id: 'id', rev: 'int', name: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shops$update = {
    op: 'std.shops.update',
    version: 'dc176d5cf0fc9441',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$count = {
    op: 'std.shopsAccess.count',
    version: '0e3b3b89af296b79',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'member', 'level'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$create = {
    op: 'std.shopsAccess.create',
    version: '0e65b2371e31cb87',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['staff', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$delete = {
    op: 'std.shopsAccess.delete',
    version: '9c813d9add4811e5',
    method: 'DELETE',
    kind: 'delete',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shopsAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$get = {
    op: 'std.shopsAccess.get',
    version: '18fa7deb8a3b06e0',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shopsAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', shop: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shopsAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$list = {
    op: 'std.shopsAccess.list',
    version: 'a4c33cf0ac65d771',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'shopsAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'shop', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', shop: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$put = {
    op: 'std.shopsAccess.put',
    version: '827e5a8901bf5829',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['staff', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$shopsAccess$update = {
    op: 'std.shopsAccess.update',
    version: 'dfb1507e0cdd0cc6',
    method: 'POST',
    kind: 'write',
    terminal: ['commit'],
    paged: false,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: 'rev',
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'shopsAccess', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'shop', type: 'id', required: true, idEntity: 'shops', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['staff', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const SPECS = {
  addProduct: spec_addProduct,
  allOrders: spec_allOrders,
  catalogue: spec_catalogue,
  editProduct: spec_editProduct,
  grantShopsAccess: spec_grantShopsAccess,
  markShipped: spec_markShipped,
  me: spec_me,
  myOrders: spec_myOrders,
  myShops: spec_myShops,
  openShop: spec_openShop,
  placeOrder: spec_placeOrder,
  productsWhere: spec_productsWhere,
  restock: spec_restock,
  revokeShopsAccess: spec_revokeShopsAccess,
  setShopsAccessLevel: spec_setShopsAccessLevel,
  staffOf: spec_staffOf,
  'std.orders.count': spec_std$orders$count,
  'std.orders.create': spec_std$orders$create,
  'std.orders.delete': spec_std$orders$delete,
  'std.orders.get': spec_std$orders$get,
  'std.orders.list': spec_std$orders$list,
  'std.orders.update': spec_std$orders$update,
  'std.products.count': spec_std$products$count,
  'std.products.create': spec_std$products$create,
  'std.products.delete': spec_std$products$delete,
  'std.products.get': spec_std$products$get,
  'std.products.list': spec_std$products$list,
  'std.products.put': spec_std$products$put,
  'std.products.update': spec_std$products$update,
  'std.shipments.count': spec_std$shipments$count,
  'std.shipments.create': spec_std$shipments$create,
  'std.shipments.delete': spec_std$shipments$delete,
  'std.shipments.get': spec_std$shipments$get,
  'std.shipments.list': spec_std$shipments$list,
  'std.shipments.put': spec_std$shipments$put,
  'std.shipments.update': spec_std$shipments$update,
  'std.shops.count': spec_std$shops$count,
  'std.shops.create': spec_std$shops$create,
  'std.shops.delete': spec_std$shops$delete,
  'std.shops.get': spec_std$shops$get,
  'std.shops.list': spec_std$shops$list,
  'std.shops.update': spec_std$shops$update,
  'std.shopsAccess.count': spec_std$shopsAccess$count,
  'std.shopsAccess.create': spec_std$shopsAccess$create,
  'std.shopsAccess.delete': spec_std$shopsAccess$delete,
  'std.shopsAccess.get': spec_std$shopsAccess$get,
  'std.shopsAccess.list': spec_std$shopsAccess$list,
  'std.shopsAccess.put': spec_std$shopsAccess$put,
  'std.shopsAccess.update': spec_std$shopsAccess$update,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
