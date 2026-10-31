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
  domain: 'teamspace',
  digest: '519caef8ef5d030f',
} as const;

export const spec_board = {
    op: 'board',
    version: 'ddb09fd5932d42b8',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: 'id' },
    order: { keys: [{ path: 'added', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'status', 'assignee', 'assigneeName', 'authorName', 'due', 'added'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', status: 'text', assignee: 'id', assigneeName: 'text', authorName: 'text', due: 'date', added: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_createTask = {
    op: 'createTask',
    version: '1d8d2e5df98bbb35',
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
      { name: 'assignee', type: 'id', required: false, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'due', type: 'date', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'added', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_editTask = {
    op: 'editTask',
    version: '11931e1efaa2604e',
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
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'assignee', type: 'id', required: false, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'due', type: 'date', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
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

export const spec_moveTask = {
    op: 'moveTask',
    version: '500c70f442ef0792',
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
      { name: 'status', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myProjects = {
    op: 'myProjects',
    version: 'd965f17cb2c2f939',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'team', key: '$id' },
    order: { keys: [{ path: 'name', desc: false }, { path: '$id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'name', 'summary', '$id'], decimals: [], types: { id: 'id', name: 'text', summary: 'text', $id: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_people = {
    op: 'people',
    version: '2bc537b1591e91da',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'members', key: 'id' },
    order: { keys: [{ path: 'displayName', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'displayName', 'role', 'status', 'joined'], decimals: [], types: { id: 'id', displayName: 'text', role: 'text', status: 'text', joined: 'date' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_projectTeam = {
    op: 'projectTeam',
    version: '6a214c6577fb7e79',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'team', key: 'id' },
    order: { keys: [{ path: 'memberName', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'member', 'memberName', 'memberRole'], decimals: [], types: { id: 'id', member: 'id', memberName: 'text', memberRole: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_reportByPerson = {
    op: 'reportByPerson',
    version: '5f92754927573c6f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'members', key: 'id' },
    order: { keys: [{ path: 'displayName', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'displayName', 'role', 'total', 'open', 'done', 'overdue'], decimals: [], types: { id: 'id', displayName: 'text', role: 'text', total: 'int', open: 'int', done: 'int', overdue: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'today', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_reportByProject = {
    op: 'reportByProject',
    version: '86d13d6d3d1a6960',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: 'id' },
    order: { keys: [{ path: 'name', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'name', 'total', 'todo', 'doing', 'review', 'done', 'overdue'], decimals: [], types: { id: 'id', name: 'text', total: 'int', todo: 'int', doing: 'int', review: 'int', done: 'int', overdue: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'today', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$count = {
    op: 'std.projects.count',
    version: '0f0b7f907e0fd630',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'summary', 'added'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$create = {
    op: 'std.projects.create',
    version: '9dbc741c7463daca',
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
      { name: 'summary', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'added', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$delete = {
    op: 'std.projects.delete',
    version: '14d38079b7262543',
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
    version: 'e76b977a24880175',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'summary', 'added'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', summary: 'text', added: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$list = {
    op: 'std.projects.list',
    version: 'bdab11580d71019d',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'projects', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'name', 'summary', 'added'], decimals: [], types: { id: 'id', rev: 'int', name: 'text', summary: 'text', added: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$projects$update = {
    op: 'std.projects.update',
    version: 'f1adb366adb3d32f',
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
      { name: 'summary', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'added', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$count = {
    op: 'std.tasks.count',
    version: 'f3c5dc5cea6c35b6',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'title', 'status', 'assignee', 'author', 'due', 'added'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$create = {
    op: 'std.tasks.create',
    version: '268d432017aa56c1',
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
      { name: 'status', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'assignee', type: 'id', required: false, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'author', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'due', type: 'date', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'added', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$delete = {
    op: 'std.tasks.delete',
    version: '94117294716b789c',
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
    refusals: ['access_denied', 'access_guard', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$get = {
    op: 'std.tasks.get',
    version: '1332427c9f67b869',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'title', 'status', 'assignee', 'author', 'due', 'added'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', title: 'text', status: 'text', assignee: 'id', author: 'id', due: 'date', added: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'tasks', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$list = {
    op: 'std.tasks.list',
    version: '2e1e14642349e1e6',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'tasks', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'title', 'status', 'assignee', 'author', 'due', 'added'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', title: 'text', status: 'text', assignee: 'id', author: 'id', due: 'date', added: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$tasks$update = {
    op: 'std.tasks.update',
    version: '04d116a43793f5ce',
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
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'status', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'assignee', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'author', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'due', type: 'date', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'added', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'check_violation', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$count = {
    op: 'std.team.count',
    version: 'a9e97e7f445fcd2d',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$create = {
    op: 'std.team.create',
    version: 'f09c36bd816bc285',
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
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$delete = {
    op: 'std.team.delete',
    version: '159c892a4b379cf6',
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
      { name: 'id', type: 'id', required: true, idEntity: 'team', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$get = {
    op: 'std.team.get',
    version: 'd27f9ae5a501d15f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'team', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', member: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'team', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$list = {
    op: 'std.team.list',
    version: 'b847915092e6e6ac',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'team', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'project', 'member'], decimals: [], types: { id: 'id', rev: 'int', project: 'id', member: 'id' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$put = {
    op: 'std.team.put',
    version: 'ca2f115fd11f5447',
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
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$team$update = {
    op: 'std.team.update',
    version: '7eb5db44405d5fda',
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
      { name: 'id', type: 'id', required: true, idEntity: 'team', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'project', type: 'id', required: true, idEntity: 'projects', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const SPECS = {
  board: spec_board,
  createTask: spec_createTask,
  editTask: spec_editTask,
  me: spec_me,
  moveTask: spec_moveTask,
  myProjects: spec_myProjects,
  people: spec_people,
  projectTeam: spec_projectTeam,
  reportByPerson: spec_reportByPerson,
  reportByProject: spec_reportByProject,
  'std.projects.count': spec_std$projects$count,
  'std.projects.create': spec_std$projects$create,
  'std.projects.delete': spec_std$projects$delete,
  'std.projects.get': spec_std$projects$get,
  'std.projects.list': spec_std$projects$list,
  'std.projects.update': spec_std$projects$update,
  'std.tasks.count': spec_std$tasks$count,
  'std.tasks.create': spec_std$tasks$create,
  'std.tasks.delete': spec_std$tasks$delete,
  'std.tasks.get': spec_std$tasks$get,
  'std.tasks.list': spec_std$tasks$list,
  'std.tasks.update': spec_std$tasks$update,
  'std.team.count': spec_std$team$count,
  'std.team.create': spec_std$team$create,
  'std.team.delete': spec_std$team$delete,
  'std.team.get': spec_std$team$get,
  'std.team.list': spec_std$team$list,
  'std.team.put': spec_std$team$put,
  'std.team.update': spec_std$team$update,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
