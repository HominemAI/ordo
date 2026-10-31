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
  domain: 'notes',
  digest: '8185d475d4e2920c',
} as const;

export const spec_createNote = {
    op: 'createNote',
    version: '3c03cea194c34f42',
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
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tags', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_deleteNote = {
    op: 'deleteNote',
    version: '829a90000278661a',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_editNote = {
    op: 'editNote',
    version: '07d0d1a5327179ad',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tags', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_grantNotesAccess = {
    op: 'grantNotesAccess',
    version: 'df1bdbff5a7ce2a2',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_myNotes = {
    op: 'myNotes',
    version: '30b561d59dff8885',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_notesTagged = {
    op: 'notesTagged',
    version: '4093b8e3358e308b',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'tag', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_revokeNotesAccess = {
    op: 'revokeNotesAccess',
    version: '2383acbd95c9fe69',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_setNotesAccessLevel = {
    op: 'setNotesAccessLevel',
    version: '0f10b066f32a10f0',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$count = {
    op: 'std.notes.count',
    version: 'c0ea62aac4b06597',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$create = {
    op: 'std.notes.create',
    version: 'fdf03a08214fef32',
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
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tags', type: 'json', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$delete = {
    op: 'std.notes.delete',
    version: '758e3854fd9b93db',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$get = {
    op: 'std.notes.get',
    version: '24fae415ff237bce',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$list = {
    op: 'std.notes.list',
    version: '44b70b6f464e35be',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$update = {
    op: 'std.notes.update',
    version: 'fd53072351d46798',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'title', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'body', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'tags', type: 'json', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$count = {
    op: 'std.notesAccess.count',
    version: '5fdb6a0784a64891',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'member', 'level'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$create = {
    op: 'std.notesAccess.create',
    version: 'aecbb927e07585f1',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$delete = {
    op: 'std.notesAccess.delete',
    version: '4b244b0a2dc032d9',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notesAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$get = {
    op: 'std.notesAccess.get',
    version: '693492bf144626cc',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notesAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'notesAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$list = {
    op: 'std.notesAccess.list',
    version: '5d072ec550c3c535',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notesAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'member', 'level'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', member: 'id', level: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$put = {
    op: 'std.notesAccess.put',
    version: '580bcb0c8410d22b',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notesAccess$update = {
    op: 'std.notesAccess.update',
    version: 'afccde8fd1ba0854',
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
      { name: 'id', type: 'id', required: true, idEntity: 'notesAccess', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
      { name: 'level', type: 'text', required: true, idEntity: null, enumValues: ['read', 'write', 'admin'], min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_unshareNote = {
    op: 'unshareNote',
    version: '70059304cfcbd488',
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
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'member', type: 'id', required: true, idEntity: 'members', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const SPECS = {
  createNote: spec_createNote,
  deleteNote: spec_deleteNote,
  editNote: spec_editNote,
  grantNotesAccess: spec_grantNotesAccess,
  myNotes: spec_myNotes,
  notesTagged: spec_notesTagged,
  revokeNotesAccess: spec_revokeNotesAccess,
  setNotesAccessLevel: spec_setNotesAccessLevel,
  'std.notes.count': spec_std$notes$count,
  'std.notes.create': spec_std$notes$create,
  'std.notes.delete': spec_std$notes$delete,
  'std.notes.get': spec_std$notes$get,
  'std.notes.list': spec_std$notes$list,
  'std.notes.update': spec_std$notes$update,
  'std.notesAccess.count': spec_std$notesAccess$count,
  'std.notesAccess.create': spec_std$notesAccess$create,
  'std.notesAccess.delete': spec_std$notesAccess$delete,
  'std.notesAccess.get': spec_std$notesAccess$get,
  'std.notesAccess.list': spec_std$notesAccess$list,
  'std.notesAccess.put': spec_std$notesAccess$put,
  'std.notesAccess.update': spec_std$notesAccess$update,
  unshareNote: spec_unshareNote,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
