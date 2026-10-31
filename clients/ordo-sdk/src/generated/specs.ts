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

import type { OpSpec } from '../core/spec.js';

/**
 * The descriptor this file was generated from.
 *
 * `digest` identifies the whole document and `domain` names the program. A client sends neither: the version pin is per-op (`api-engine-design.md` §10), because a registry-wide pin marks every call stale after any unrelated deploy and a signal that fires constantly is one clients learn to ignore. These are here so a bug report can say which descriptor.
 */
export const DESCRIPTOR = {
  schemaVersion: 'ordo.interface/2',
  domain: 'market',
  digest: 'f5cc0225707e1c8c',
} as const;

export const spec_activeProducts = {
    op: 'activeProducts',
    version: 'b70a43acd0632679',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'price', 'vendor', 'category', '$id'], decimals: ['price'], types: { name: 'text', price: 'money', vendor: 'id', category: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addAnonCustomer = {
    op: 'addAnonCustomer',
    version: '4a30cc53665f910a',
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
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addBoard = {
    op: 'addBoard',
    version: 'ef45b9838ca26726',
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

export const spec_addCard = {
    op: 'addCard',
    version: '43f820647a8a4a90',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addCategory = {
    op: 'addCategory',
    version: 'e0421edfdf1ba3f2',
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
      { name: 'parent', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addCustomer = {
    op: 'addCustomer',
    version: '09d90239286e9f1a',
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
      { name: 'email', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addNote = {
    op: 'addNote',
    version: '6def52ed4d4fc052',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addRootCategory = {
    op: 'addRootCategory',
    version: 'fc9adaaf92190144',
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
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_addTask = {
    op: 'addTask',
    version: 'aeaa93e65392c81b',
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
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_allTasks = {
    op: 'allTasks',
    version: '0cbc66f1165bc975',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', 'status', '$id'], decimals: [], types: { title: 'text', status: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_anonCustomers = {
    op: 'anonCustomers',
    version: 'c980f82bdf67d3db',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'customers', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', '$id'], decimals: [], types: { name: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'sealed_open_failed', 'sealed_unreadable', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_boardList = {
    op: 'boardList',
    version: '5e8f1db3d3c326a7',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'boards', key: '$id' },
    order: { keys: [{ path: 'name', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', '$id'], decimals: [], types: { name: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_cardList = {
    op: 'cardList',
    version: '170eda4fe37c4bd7',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'cards', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', 'done', 'board', '$id'], decimals: [], types: { title: 'text', done: 'bool', board: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_catInfo = {
    op: 'catInfo',
    version: '3a6a1502ceb59625',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'categories', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'parent', 'depth', '$id'], decimals: [], types: { name: 'text', parent: 'id', depth: 'int', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_cheapActive = {
    op: 'cheapActive',
    version: 'fb7bfb85c34b45c1',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'price', 'category', '$id'], decimals: ['price'], types: { name: 'text', price: 'money', category: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'cap', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_claimCoupon = {
    op: 'claimCoupon',
    version: '04e87c797d6af3e4',
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
      { name: 'code', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'slug', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'value', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_claimTask = {
    op: 'claimTask',
    version: '4bf996cb8418dcb1',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_clearCustomerEmail = {
    op: 'clearCustomerEmail',
    version: '50c435b065073eac',
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
      { name: 'c', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_closeTask = {
    op: 'closeTask',
    version: 'a3a30e422d39e4e5',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_customerByEmail = {
    op: 'customerByEmail',
    version: '7991e27931292be4',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'customers', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'tier', '$id'], decimals: [], types: { name: 'text', tier: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'e', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'sealed_open_failed', 'sealed_unreadable', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_doneOrders = {
    op: 'doneOrders',
    version: 'c06a3a53f1e8ce47',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['placed', 'total', 'customer', '$id'], decimals: ['total'], types: { placed: 'date', total: 'money', customer: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'from', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_dropCategory = {
    op: 'dropCategory',
    version: '3a4faca6200527c7',
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
      { name: 'cat', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_dropCategoryTree = {
    op: 'dropCategoryTree',
    version: 'b8c76a1205e8f13b',
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
      { name: 'cat', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_dropFolder = {
    op: 'dropFolder',
    version: '035205f835239e82',
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
      { name: 'f', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_dropFolderTree = {
    op: 'dropFolderTree',
    version: '37e5cf048a78c6bc',
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
      { name: 'f', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_entryList = {
    op: 'entryList',
    version: '2aaec543d540d239',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'ledgerEntries', key: '$id' },
    order: { keys: [{ path: 'externalId', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['externalId', 'amount', 'note', 'owner', '$id'], decimals: ['amount'], types: { externalId: 'text', amount: 'money', note: 'text', owner: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_finishCard = {
    op: 'finishCard',
    version: 'fda366e7c6318852',
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
      { name: 'card', type: 'id', required: true, idEntity: 'cards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_firstTask = {
    op: 'firstTask',
    version: 'dfcb594ef17a6591',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', '$id'], decimals: [], types: { title: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_giftOrders = {
    op: 'giftOrders',
    version: 'd9501b07ed80cf65',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: null,
    refine: { tier: 'none', fields: [], decimals: [], types: {}, sealed: [], limits: null },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'method_mismatch', 'unknown_param', 'version_skew', 'invalid_refinement', 'refinement_unbounded'],
    standingError: { code: 'sealed_unsupported', message: '`memo` is sealed: the database holds only its ciphertext, so it cannot filter, sort, aggregate or compute on it. Project `memo` as it is to read it opened on the server, `count` the rows that carry it, or declare it `sealed indexed` to look it up with `==`' },
} as const satisfies OpSpec;

export const spec_grantBoardsAccess = {
    op: 'grantBoardsAccess',
    version: '66b421d7ad5c70ae',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_memoDigest = {
    op: 'memoDigest',
    version: 'ade62aad78bfeae6',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: '$id' },
    order: { keys: [{ path: 'placed', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['placed', '$id'], decimals: [], types: { placed: 'date', memo: 'text', $id: 'id' }, sealed: ['memo'], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'sealed_open_failed', 'sealed_unreadable', 'unknown_param', 'version_skew', 'sealed_unsupported'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_monthlyRevenue = {
    op: 'monthlyRevenue',
    version: 'ac72c6b06fdf4f2d',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['month', 'revenue'], decimals: ['revenue'], types: { month: 'text', revenue: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'from', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_moveBA = {
    op: 'moveBA',
    version: '680bf9aa320ecfb7',
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
      { name: 'ba', type: 'id', required: true, idEntity: 'boardsAccess', enumValues: null, min: null, max: null, places: null },
      { name: 'b', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_moveCategory = {
    op: 'moveCategory',
    version: '8b62be10f33691a2',
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
      { name: 'cat', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
      { name: 'parent', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_movePM = {
    op: 'movePM',
    version: '886de50cc7e70199',
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
      { name: 'pm', type: 'id', required: true, idEntity: 'projectMembers', enumValues: null, min: null, max: null, places: null },
      { name: 'p', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myAssignedTasks = {
    op: 'myAssignedTasks',
    version: '138ba43829f830aa',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', '$id'], decimals: [], types: { title: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myProjects = {
    op: 'myProjects',
    version: '1346c15600c140e6',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: '$id' },
    order: { keys: [{ path: 'name', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'owner', '$id'], decimals: [], types: { name: 'text', owner: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_noteFeed = {
    op: 'noteFeed',
    version: '42dfd1f0761e25a7',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'taskNotes', key: '$id' },
    order: { keys: [{ path: 'body', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['body', 'task', '$id'], decimals: [], types: { body: 'text', task: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_openTasks = {
    op: 'openTasks',
    version: 'b0a30de6789bb9ab',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', 'project', '$id'], decimals: [], types: { title: 'text', project: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_orderMemos = {
    op: 'orderMemos',
    version: 'f1bcff40c5bd5f07',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['placed', 'total', '$id'], decimals: ['total'], types: { placed: 'date', total: 'money', memo: 'text', $id: 'id' }, sealed: ['memo'], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'sealed_open_failed', 'sealed_unreadable', 'unknown_param', 'version_skew', 'sealed_unsupported'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_productRevenue = {
    op: 'productRevenue',
    version: '83d75323a103aadc',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['product', 'sold', 'revenue'], decimals: ['revenue'], types: { product: 'id', sold: 'int', revenue: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_purgeDone = {
    op: 'purgeDone',
    version: 'f5a17a4912eabd66',
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
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_purgeNotes = {
    op: 'purgeNotes',
    version: '10d81c3a7fa1b626',
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
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'log_append_failed', 'method_mismatch', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_reassign = {
    op: 'reassign',
    version: '7d81316e7448ec72',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'who', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_recentOrders = {
    op: 'recentOrders',
    version: '88705c5aeeeb20d3',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: '$id' },
    order: { keys: [{ path: 'placed', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['placed', 'status', 'total', '$id'], decimals: ['total'], types: { placed: 'date', status: 'text', total: 'money', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'since', type: 'date', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_recordEntry = {
    op: 'recordEntry',
    version: '8a4c76fe09f3524b',
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
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'note', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_recordEntryFor = {
    op: 'recordEntryFor',
    version: 'bda107f6ef0d5efd',
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
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_recordEntryOnce = {
    op: 'recordEntryOnce',
    version: 'ab491d81e4174a3a',
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
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_renameTask = {
    op: 'renameTask',
    version: '842879343326cef3',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_renameTaskIf = {
    op: 'renameTaskIf',
    version: '02246eed47b4f93f',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_renumberEntry = {
    op: 'renumberEntry',
    version: '478c41b2f5facfa1',
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
      { name: 'entry', type: 'id', required: true, idEntity: 'ledgerEntries', enumValues: null, min: null, max: null, places: null },
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_renumberEntryOnce = {
    op: 'renumberEntryOnce',
    version: '7cf8d5896a409835',
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
      { name: 'entry', type: 'id', required: true, idEntity: 'ledgerEntries', enumValues: null, min: null, max: null, places: null },
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_reviseEntry = {
    op: 'reviseEntry',
    version: '2170f8ea82a162e2',
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
      { name: 'ext', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_revokeBoardsAccess = {
    op: 'revokeBoardsAccess',
    version: '1b21863d41309acc',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_setBoardsAccessLevel = {
    op: 'setBoardsAccessLevel',
    version: 'b36cabf5217c4114',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_setMemo = {
    op: 'setMemo',
    version: '13b6184dd8b159f6',
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
      { name: 'memo', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_specCount = {
    op: 'specCount',
    version: '3be68cd42472b7ca',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['n'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_specTasks = {
    op: 'specTasks',
    version: '1e66157b46e71034',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', '$id'], decimals: [], types: { title: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_spendByCustomer = {
    op: 'spendByCustomer',
    version: '484a10f90751b021',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['customer', 'spent', 'n'], decimals: ['spent'], types: { customer: 'id', spent: 'money', n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'from', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boards$count = {
    op: 'std.boards.count',
    version: '5facf5c698468c16',
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

export const spec_std$boards$create = {
    op: 'std.boards.create',
    version: '8b1851130ff73a22',
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

export const spec_std$boards$delete = {
    op: 'std.boards.delete',
    version: '2f67cac10369c835',
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
      { name: 'id', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boards$get = {
    op: 'std.boards.get',
    version: '5b017f604514b583',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'boards', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name'], decimals: [], types: { id: 'id', rev: 'int', name: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boards$list = {
    op: 'std.boards.list',
    version: 'dca30876b273f480',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'boards', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name'], decimals: [], types: { id: 'id', rev: 'int', name: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boards$update = {
    op: 'std.boards.update',
    version: '26cf817cd870ed89',
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
      { name: 'id', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$count = {
    op: 'std.boardsAccess.count',
    version: '0bbe19118a891a29',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'member', 'level'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$create = {
    op: 'std.boardsAccess.create',
    version: '7e5a895eb5a3a2f4',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$delete = {
    op: 'std.boardsAccess.delete',
    version: '8cfa8c1e12d13f59',
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
      { name: 'id', type: 'id', required: true, idEntity: 'boardsAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$get = {
    op: 'std.boardsAccess.get',
    version: '9e96da251683e0d4',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'boardsAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', board: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'boardsAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$list = {
    op: 'std.boardsAccess.list',
    version: '261a77ef6b811391',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'boardsAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', board: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$put = {
    op: 'std.boardsAccess.put',
    version: '7847f0ef0f4204f3',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$boardsAccess$update = {
    op: 'std.boardsAccess.update',
    version: '869ce749d43a5efe',
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
      { name: 'id', type: 'id', required: true, idEntity: 'boardsAccess', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$count = {
    op: 'std.cards.count',
    version: 'bf39b852661778fb',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'title', 'done'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$create = {
    op: 'std.cards.create',
    version: '1c870f686f80f2a4',
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
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'done', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$delete = {
    op: 'std.cards.delete',
    version: 'bbfc03794db3d30e',
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
      { name: 'id', type: 'id', required: true, idEntity: 'cards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$get = {
    op: 'std.cards.get',
    version: '538233d52a2c2662',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'cards', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'title', 'done'], decimals: [], types: { id: 'id', rev: 'int', board: 'id', title: 'text', done: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'cards', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$list = {
    op: 'std.cards.list',
    version: '33a74ee7d18e6689',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'cards', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'board', 'title', 'done'], decimals: [], types: { id: 'id', rev: 'int', board: 'id', title: 'text', done: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$cards$update = {
    op: 'std.cards.update',
    version: '19e121d6646a6f3c',
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
      { name: 'id', type: 'id', required: true, idEntity: 'cards', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'board', type: 'id', required: true, idEntity: 'boards', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'done', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$count = {
    op: 'std.categories.count',
    version: 'c365f1ce212e5ee7',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'name', 'parent'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$create = {
    op: 'std.categories.create',
    version: 'fcd544f6306f449f',
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
      { name: 'parent', type: 'id', required: false, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$delete = {
    op: 'std.categories.delete',
    version: '0cd810e490bf614f',
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
      { name: 'id', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$deleteSubtree = {
    op: 'std.categories.deleteSubtree',
    version: '15d5f40fc857414f',
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
      { name: 'id', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$get = {
    op: 'std.categories.get',
    version: 'b3c7e311a53760c6',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'categories', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'name', 'parent'], decimals: [], types: { id: 'id', rev: 'int', depth: 'int', name: 'text', parent: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$list = {
    op: 'std.categories.list',
    version: '40cca70c58d1f459',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'categories', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'name', 'parent'], decimals: [], types: { id: 'id', rev: 'int', depth: 'int', name: 'text', parent: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$put = {
    op: 'std.categories.put',
    version: '719b4d6dca2214f1',
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
      { name: 'parent', type: 'id', required: false, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$categories$update = {
    op: 'std.categories.update',
    version: '95218807e8cd4265',
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
      { name: 'id', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'parent', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$coupons$count = {
    op: 'std.coupons.count',
    version: 'af4244a7fab224db',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'code', 'slug', 'value'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$coupons$delete = {
    op: 'std.coupons.delete',
    version: '17e1735bb121b767',
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
      { name: 'id', type: 'id', required: true, idEntity: 'coupons', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$coupons$get = {
    op: 'std.coupons.get',
    version: 'e63a6077fd5c19a5',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'coupons', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'code', 'slug', 'value'], decimals: ['value'], types: { id: 'id', rev: 'int', code: 'text', slug: 'text', value: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'coupons', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$coupons$list = {
    op: 'std.coupons.list',
    version: 'ee98fd110b43eeb0',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'coupons', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'code', 'slug', 'value'], decimals: ['value'], types: { id: 'id', rev: 'int', code: 'text', slug: 'text', value: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$coupons$update = {
    op: 'std.coupons.update',
    version: '8ba6451a7eff720c',
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
      { name: 'id', type: 'id', required: true, idEntity: 'coupons', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'code', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'slug', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'value', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$count = {
    op: 'std.customers.count',
    version: 'c948385c7388afab',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'tier'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$create = {
    op: 'std.customers.create',
    version: 'eb57da6bffad7cd3',
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
      { name: 'email', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tier', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$delete = {
    op: 'std.customers.delete',
    version: '3466b4bca025122e',
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
      { name: 'id', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$get = {
    op: 'std.customers.get',
    version: '5a0a6ff61e5f0387',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'customers', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'tier'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', tier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$list = {
    op: 'std.customers.list',
    version: 'e25f7e2f655680bc',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'customers', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'tier'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', tier: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$put = {
    op: 'std.customers.put',
    version: 'eb57c9834ad73987',
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
      { name: 'email', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tier', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$customers$update = {
    op: 'std.customers.update',
    version: 'a2702f0e1c9e0057',
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
      { name: 'id', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tier', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$count = {
    op: 'std.folders.count',
    version: '214a81897faaf590',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'owner', 'name', 'parent'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$create = {
    op: 'std.folders.create',
    version: 'b6ef5d47c3c5c190',
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
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'parent', type: 'id', required: false, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$delete = {
    op: 'std.folders.delete',
    version: '9f56b2236098f699',
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
      { name: 'id', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$deleteSubtree = {
    op: 'std.folders.deleteSubtree',
    version: '6d4c4d5b5366095b',
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
      { name: 'id', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'tree_has_children', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$get = {
    op: 'std.folders.get',
    version: 'f77afb7985ce0d41',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'folders', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'owner', 'name', 'parent'], decimals: [], types: { id: 'id', rev: 'int', depth: 'int', owner: 'id', name: 'text', parent: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$list = {
    op: 'std.folders.list',
    version: '024c3f07bb30ff43',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'folders', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'depth', 'owner', 'name', 'parent'], decimals: [], types: { id: 'id', rev: 'int', depth: 'int', owner: 'id', name: 'text', parent: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$folders$update = {
    op: 'std.folders.update',
    version: '7ee750a6741bfdfa',
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
      { name: 'id', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'parent', type: 'id', required: true, idEntity: 'folders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'tree_cycle', 'tree_depth_exceeded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$count = {
    op: 'std.ledgerEntries.count',
    version: '29831de03fd88f67',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'externalId', 'owner', 'amount', 'note'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$create = {
    op: 'std.ledgerEntries.create',
    version: '639f186c133b5092',
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
      { name: 'externalId', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'note', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$delete = {
    op: 'std.ledgerEntries.delete',
    version: 'ea6df0cebb3788db',
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
      { name: 'id', type: 'id', required: true, idEntity: 'ledgerEntries', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$get = {
    op: 'std.ledgerEntries.get',
    version: '2408c9c31600b33f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'ledgerEntries', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'externalId', 'owner', 'amount', 'note'], decimals: ['amount'], types: { id: 'id', rev: 'int', externalId: 'text', owner: 'id', amount: 'money', note: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'ledgerEntries', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$list = {
    op: 'std.ledgerEntries.list',
    version: 'a1d724434a5c9b64',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'ledgerEntries', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'externalId', 'owner', 'amount', 'note'], decimals: ['amount'], types: { id: 'id', rev: 'int', externalId: 'text', owner: 'id', amount: 'money', note: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$put = {
    op: 'std.ledgerEntries.put',
    version: 'fee9762061670d36',
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
      { name: 'externalId', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'note', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$ledgerEntries$update = {
    op: 'std.ledgerEntries.update',
    version: 'e88c2270d04d5a71',
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
      { name: 'id', type: 'id', required: true, idEntity: 'ledgerEntries', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'externalId', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'note', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$count = {
    op: 'std.orderItems.count',
    version: 'fc4503f0a46cfe34',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'product', 'qty', 'amount'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$create = {
    op: 'std.orderItems.create',
    version: '2709e867008136e6',
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
      { name: 'qty', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$delete = {
    op: 'std.orderItems.delete',
    version: '03f1dd866144f9aa',
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
      { name: 'id', type: 'id', required: true, idEntity: 'orderItems', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$get = {
    op: 'std.orderItems.get',
    version: '81ea1a9ed59a6973',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orderItems', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'product', 'qty', 'amount'], decimals: ['amount'], types: { id: 'id', rev: 'int', order: 'id', product: 'id', qty: 'int', amount: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'orderItems', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$list = {
    op: 'std.orderItems.list',
    version: 'a702dae543600ec2',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orderItems', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'order', 'product', 'qty', 'amount'], decimals: ['amount'], types: { id: 'id', rev: 'int', order: 'id', product: 'id', qty: 'int', amount: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orderItems$update = {
    op: 'std.orderItems.update',
    version: '417127e000903a3a',
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
      { name: 'id', type: 'id', required: true, idEntity: 'orderItems', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'order', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
      { name: 'product', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
      { name: 'qty', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'amount', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$count = {
    op: 'std.orders.count',
    version: '98cb85089f907ea1',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'placed', 'status', 'total'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$create = {
    op: 'std.orders.create',
    version: '6e05e121725f66a1',
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
      { name: 'customer', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
      { name: 'placed', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'status', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'total', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'memo', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$delete = {
    op: 'std.orders.delete',
    version: 'f6437c0898a49cad',
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
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$get = {
    op: 'std.orders.get',
    version: 'a90de2f42eca443d',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'placed', 'status', 'total'], decimals: ['total'], types: { id: 'id', rev: 'int', customer: 'id', placed: 'date', status: 'text', total: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'orders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$list = {
    op: 'std.orders.list',
    version: '41bad8f584279e7f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'customer', 'placed', 'status', 'total'], decimals: ['total'], types: { id: 'id', rev: 'int', customer: 'id', placed: 'date', status: 'text', total: 'money' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$orders$update = {
    op: 'std.orders.update',
    version: '43c76ca73e1d4cc1',
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
      { name: 'customer', type: 'id', required: true, idEntity: 'customers', enumValues: null, min: null, max: null, places: null },
      { name: 'placed', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'status', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'total', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
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
    refine: { tier: 'basic', fields: ['id', 'rev', 'vendor', 'category', 'name', 'price', 'active'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$create = {
    op: 'std.products.create',
    version: '433d3ec81f489597',
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
      { name: 'vendor', type: 'id', required: true, idEntity: 'vendors', enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'active', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$delete = {
    op: 'std.products.delete',
    version: 'e2238f81e12c68b1',
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
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$get = {
    op: 'std.products.get',
    version: '4be08c88be38181a',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'vendor', 'category', 'name', 'price', 'active'], decimals: ['price'], types: { id: 'id', rev: 'int', vendor: 'id', category: 'id', name: 'text', price: 'money', active: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'products', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$list = {
    op: 'std.products.list',
    version: 'f553dcfb80962894',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'vendor', 'category', 'name', 'price', 'active'], decimals: ['price'], types: { id: 'id', rev: 'int', vendor: 'id', category: 'id', name: 'text', price: 'money', active: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$products$update = {
    op: 'std.products.update',
    version: '588d2cb858b68284',
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
      { name: 'vendor', type: 'id', required: true, idEntity: 'vendors', enumValues: null, min: null, max: null, places: null },
      { name: 'category', type: 'id', required: true, idEntity: 'categories', enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'price', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'active', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$count = {
    op: 'std.projectMembers.count',
    version: '90545e517c5f1ec5',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member', 'canWrite'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$create = {
    op: 'std.projectMembers.create',
    version: 'feb8a5c074f88814',
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
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'canWrite', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$delete = {
    op: 'std.projectMembers.delete',
    version: 'b23f34c62ad9b53c',
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
      { name: 'id', type: 'id', required: true, idEntity: 'projectMembers', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$get = {
    op: 'std.projectMembers.get',
    version: '168befbb6940d6f5',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projectMembers', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member', 'canWrite'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', member: 'id', canWrite: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'projectMembers', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$list = {
    op: 'std.projectMembers.list',
    version: 'a6477d88a7096419',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projectMembers', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member', 'canWrite'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', member: 'id', canWrite: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projectMembers$update = {
    op: 'std.projectMembers.update',
    version: '2149533a5f04e8eb',
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
      { name: 'id', type: 'id', required: true, idEntity: 'projectMembers', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'canWrite', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$count = {
    op: 'std.projects.count',
    version: '9efbb976e56e4ca2',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'owner'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$create = {
    op: 'std.projects.create',
    version: '48ba1ceefff2e87a',
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
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$delete = {
    op: 'std.projects.delete',
    version: 'a9c4bfa9d32c0ede',
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
      { name: 'id', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$get = {
    op: 'std.projects.get',
    version: '072c1c6688c925ce',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'owner'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', owner: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$list = {
    op: 'std.projects.list',
    version: '616aecc67043c8b9',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'owner'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', owner: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$update = {
    op: 'std.projects.update',
    version: 'c35a0ad2bb4a2e91',
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
      { name: 'id', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'owner', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$count = {
    op: 'std.taskNotes.count',
    version: 'f96eef8ba1d6a8d3',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'task', 'body'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$create = {
    op: 'std.taskNotes.create',
    version: 'b15a2ef9a60649e0',
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
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$delete = {
    op: 'std.taskNotes.delete',
    version: '5a504896c7ce10b4',
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
      { name: 'id', type: 'id', required: true, idEntity: 'taskNotes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$get = {
    op: 'std.taskNotes.get',
    version: 'b50b5ff04b0884ab',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'taskNotes', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'task', 'body'], decimals: [], types: { id: 'id', rev: 'int', task: 'id', body: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'taskNotes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$list = {
    op: 'std.taskNotes.list',
    version: 'f0f8e1c915d14e05',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'taskNotes', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'task', 'body'], decimals: [], types: { id: 'id', rev: 'int', task: 'id', body: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$taskNotes$update = {
    op: 'std.taskNotes.update',
    version: '79517f5630a6f611',
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
      { name: 'id', type: 'id', required: true, idEntity: 'taskNotes', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'task', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$count = {
    op: 'std.tasks.count',
    version: 'f00ac7a647884b52',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'assignee', 'title', 'status'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$create = {
    op: 'std.tasks.create',
    version: '258a3764fd3ec31d',
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
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'assignee', type: 'id', required: false, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'status', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$delete = {
    op: 'std.tasks.delete',
    version: 'e9c4b3f0dbe6b16d',
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
      { name: 'id', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$get = {
    op: 'std.tasks.get',
    version: 'ed44271fa54f6179',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'assignee', 'title', 'status'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', assignee: 'id', title: 'text', status: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$list = {
    op: 'std.tasks.list',
    version: 'fdfe15fbfdcd00ce',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'assignee', 'title', 'status'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', assignee: 'id', title: 'text', status: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$update = {
    op: 'std.tasks.update',
    version: 'fc2328099ee311f5',
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
      { name: 'id', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'assignee', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'status', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$count = {
    op: 'std.vendors.count',
    version: 'cb64ecd8694c007e',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'region', 'active'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$create = {
    op: 'std.vendors.create',
    version: '4a083c61440006a7',
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
      { name: 'region', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'active', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$delete = {
    op: 'std.vendors.delete',
    version: '405dfa65a5cb9d4c',
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
      { name: 'id', type: 'id', required: true, idEntity: 'vendors', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'has_referrers', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$get = {
    op: 'std.vendors.get',
    version: 'f996f6fd4d56c402',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'vendors', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'region', 'active'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', region: 'text', active: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'vendors', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$list = {
    op: 'std.vendors.list',
    version: '13b90a64ff2360fa',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'vendors', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'region', 'active'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', region: 'text', active: 'bool' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$vendors$update = {
    op: 'std.vendors.update',
    version: 'f41b628418d74d3f',
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
      { name: 'id', type: 'id', required: true, idEntity: 'vendors', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'region', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'active', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_taskBoard = {
    op: 'taskBoard',
    version: '5aa4c85c34b76b73',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: '$id' },
    order: { keys: [{ path: 'title', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['title', 'status', 'project', '$id'], decimals: [], types: { title: 'text', status: 'text', project: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_teamRoster = {
    op: 'teamRoster',
    version: 'e89e2687d1f83b49',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projectMembers', key: '$id' },
    order: { keys: [{ path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['project', 'member', 'canWrite', '$id'], decimals: [], types: { project: 'id', member: 'id', canWrite: 'bool', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_toolDeals = {
    op: 'toolDeals',
    version: 'ad4c909a11b28d14',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'products', key: '$id' },
    order: { keys: [{ path: 'price', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['name', 'price', '$id'], decimals: ['price'], types: { name: 'text', price: 'money', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'cap', type: 'money', required: true, idEntity: null, enumValues: null, min: null, max: null, places: 0 },
      { name: 'root', type: 'id', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_top3Orders = {
    op: 'top3Orders',
    version: '477f9faf06ddcbf8',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'orders', key: '$id' },
    order: { keys: [{ path: 'total', desc: true }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['placed', 'total', 'customer', '$id'], decimals: ['total'], types: { placed: 'date', total: 'money', customer: 'id', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_upsertCustomer = {
    op: 'upsertCustomer',
    version: '24a2a9c925f7ff44',
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
      { name: 'email', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tier', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'sealed_open_failed', 'sealed_unreadable', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const SPECS = {
  activeProducts: spec_activeProducts,
  addAnonCustomer: spec_addAnonCustomer,
  addBoard: spec_addBoard,
  addCard: spec_addCard,
  addCategory: spec_addCategory,
  addCustomer: spec_addCustomer,
  addNote: spec_addNote,
  addRootCategory: spec_addRootCategory,
  addTask: spec_addTask,
  allTasks: spec_allTasks,
  anonCustomers: spec_anonCustomers,
  boardList: spec_boardList,
  cardList: spec_cardList,
  catInfo: spec_catInfo,
  cheapActive: spec_cheapActive,
  claimCoupon: spec_claimCoupon,
  claimTask: spec_claimTask,
  clearCustomerEmail: spec_clearCustomerEmail,
  closeTask: spec_closeTask,
  customerByEmail: spec_customerByEmail,
  doneOrders: spec_doneOrders,
  dropCategory: spec_dropCategory,
  dropCategoryTree: spec_dropCategoryTree,
  dropFolder: spec_dropFolder,
  dropFolderTree: spec_dropFolderTree,
  entryList: spec_entryList,
  finishCard: spec_finishCard,
  firstTask: spec_firstTask,
  giftOrders: spec_giftOrders,
  grantBoardsAccess: spec_grantBoardsAccess,
  memoDigest: spec_memoDigest,
  monthlyRevenue: spec_monthlyRevenue,
  moveBA: spec_moveBA,
  moveCategory: spec_moveCategory,
  movePM: spec_movePM,
  myAssignedTasks: spec_myAssignedTasks,
  myProjects: spec_myProjects,
  noteFeed: spec_noteFeed,
  openTasks: spec_openTasks,
  orderMemos: spec_orderMemos,
  productRevenue: spec_productRevenue,
  purgeDone: spec_purgeDone,
  purgeNotes: spec_purgeNotes,
  reassign: spec_reassign,
  recentOrders: spec_recentOrders,
  recordEntry: spec_recordEntry,
  recordEntryFor: spec_recordEntryFor,
  recordEntryOnce: spec_recordEntryOnce,
  renameTask: spec_renameTask,
  renameTaskIf: spec_renameTaskIf,
  renumberEntry: spec_renumberEntry,
  renumberEntryOnce: spec_renumberEntryOnce,
  reviseEntry: spec_reviseEntry,
  revokeBoardsAccess: spec_revokeBoardsAccess,
  setBoardsAccessLevel: spec_setBoardsAccessLevel,
  setMemo: spec_setMemo,
  specCount: spec_specCount,
  specTasks: spec_specTasks,
  spendByCustomer: spec_spendByCustomer,
  'std.boards.count': spec_std$boards$count,
  'std.boards.create': spec_std$boards$create,
  'std.boards.delete': spec_std$boards$delete,
  'std.boards.get': spec_std$boards$get,
  'std.boards.list': spec_std$boards$list,
  'std.boards.update': spec_std$boards$update,
  'std.boardsAccess.count': spec_std$boardsAccess$count,
  'std.boardsAccess.create': spec_std$boardsAccess$create,
  'std.boardsAccess.delete': spec_std$boardsAccess$delete,
  'std.boardsAccess.get': spec_std$boardsAccess$get,
  'std.boardsAccess.list': spec_std$boardsAccess$list,
  'std.boardsAccess.put': spec_std$boardsAccess$put,
  'std.boardsAccess.update': spec_std$boardsAccess$update,
  'std.cards.count': spec_std$cards$count,
  'std.cards.create': spec_std$cards$create,
  'std.cards.delete': spec_std$cards$delete,
  'std.cards.get': spec_std$cards$get,
  'std.cards.list': spec_std$cards$list,
  'std.cards.update': spec_std$cards$update,
  'std.categories.count': spec_std$categories$count,
  'std.categories.create': spec_std$categories$create,
  'std.categories.delete': spec_std$categories$delete,
  'std.categories.deleteSubtree': spec_std$categories$deleteSubtree,
  'std.categories.get': spec_std$categories$get,
  'std.categories.list': spec_std$categories$list,
  'std.categories.put': spec_std$categories$put,
  'std.categories.update': spec_std$categories$update,
  'std.coupons.count': spec_std$coupons$count,
  'std.coupons.delete': spec_std$coupons$delete,
  'std.coupons.get': spec_std$coupons$get,
  'std.coupons.list': spec_std$coupons$list,
  'std.coupons.update': spec_std$coupons$update,
  'std.customers.count': spec_std$customers$count,
  'std.customers.create': spec_std$customers$create,
  'std.customers.delete': spec_std$customers$delete,
  'std.customers.get': spec_std$customers$get,
  'std.customers.list': spec_std$customers$list,
  'std.customers.put': spec_std$customers$put,
  'std.customers.update': spec_std$customers$update,
  'std.folders.count': spec_std$folders$count,
  'std.folders.create': spec_std$folders$create,
  'std.folders.delete': spec_std$folders$delete,
  'std.folders.deleteSubtree': spec_std$folders$deleteSubtree,
  'std.folders.get': spec_std$folders$get,
  'std.folders.list': spec_std$folders$list,
  'std.folders.update': spec_std$folders$update,
  'std.ledgerEntries.count': spec_std$ledgerEntries$count,
  'std.ledgerEntries.create': spec_std$ledgerEntries$create,
  'std.ledgerEntries.delete': spec_std$ledgerEntries$delete,
  'std.ledgerEntries.get': spec_std$ledgerEntries$get,
  'std.ledgerEntries.list': spec_std$ledgerEntries$list,
  'std.ledgerEntries.put': spec_std$ledgerEntries$put,
  'std.ledgerEntries.update': spec_std$ledgerEntries$update,
  'std.orderItems.count': spec_std$orderItems$count,
  'std.orderItems.create': spec_std$orderItems$create,
  'std.orderItems.delete': spec_std$orderItems$delete,
  'std.orderItems.get': spec_std$orderItems$get,
  'std.orderItems.list': spec_std$orderItems$list,
  'std.orderItems.update': spec_std$orderItems$update,
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
  'std.products.update': spec_std$products$update,
  'std.projectMembers.count': spec_std$projectMembers$count,
  'std.projectMembers.create': spec_std$projectMembers$create,
  'std.projectMembers.delete': spec_std$projectMembers$delete,
  'std.projectMembers.get': spec_std$projectMembers$get,
  'std.projectMembers.list': spec_std$projectMembers$list,
  'std.projectMembers.update': spec_std$projectMembers$update,
  'std.projects.count': spec_std$projects$count,
  'std.projects.create': spec_std$projects$create,
  'std.projects.delete': spec_std$projects$delete,
  'std.projects.get': spec_std$projects$get,
  'std.projects.list': spec_std$projects$list,
  'std.projects.update': spec_std$projects$update,
  'std.taskNotes.count': spec_std$taskNotes$count,
  'std.taskNotes.create': spec_std$taskNotes$create,
  'std.taskNotes.delete': spec_std$taskNotes$delete,
  'std.taskNotes.get': spec_std$taskNotes$get,
  'std.taskNotes.list': spec_std$taskNotes$list,
  'std.taskNotes.update': spec_std$taskNotes$update,
  'std.tasks.count': spec_std$tasks$count,
  'std.tasks.create': spec_std$tasks$create,
  'std.tasks.delete': spec_std$tasks$delete,
  'std.tasks.get': spec_std$tasks$get,
  'std.tasks.list': spec_std$tasks$list,
  'std.tasks.update': spec_std$tasks$update,
  'std.vendors.count': spec_std$vendors$count,
  'std.vendors.create': spec_std$vendors$create,
  'std.vendors.delete': spec_std$vendors$delete,
  'std.vendors.get': spec_std$vendors$get,
  'std.vendors.list': spec_std$vendors$list,
  'std.vendors.update': spec_std$vendors$update,
  taskBoard: spec_taskBoard,
  teamRoster: spec_teamRoster,
  toolDeals: spec_toolDeals,
  top3Orders: spec_top3Orders,
  upsertCustomer: spec_upsertCustomer,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
