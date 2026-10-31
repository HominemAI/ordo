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
  digest: 'e065c609db8302ea',
} as const;

export const spec_addAttachment = {
    op: 'addAttachment',
    version: '0e504b7ab63c277f',
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
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'kind', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'size', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_allNotes = {
    op: 'allNotes',
    version: 'd247a6bd361eb06a',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created', 'pinned', 'archived'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime', pinned: 'bool', archived: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_archiveNote = {
    op: 'archiveNote',
    version: '21b4746ee5d145f4',
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
      { name: 'at', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_clearReminder = {
    op: 'clearReminder',
    version: '0285fedb9f9b40d8',
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
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

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
    version: '20d8b4d363fb8b0d',
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

export const spec_myFiles = {
    op: 'myFiles',
    version: 'af7fe95d0dd29539',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'attachments', key: 'id' },
    order: { keys: [{ path: 'created', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'name', 'kind', 'size', 'created', 'file'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', name: 'text', kind: 'text', size: 'int', created: 'datetime', file: 'blob' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
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

export const spec_myReminders = {
    op: 'myReminders',
    version: '78d6a28fdf74258a',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'reminders', key: 'id' },
    order: { keys: [{ path: 'due', desc: false }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'due', 'fired'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', due: 'datetime', fired: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_notesTagged = {
    op: 'notesTagged',
    version: 'b15dc86a74f4e1da',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created', 'pinned', 'archived'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime', pinned: 'bool', archived: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'tag', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_pinNote = {
    op: 'pinNote',
    version: '7bdd95510fe07739',
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
      { name: 'pinned', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_removeAttachment = {
    op: 'removeAttachment',
    version: '33ee4371b262e493',
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
      { name: 'id', type: 'id', required: true, idEntity: 'attachments', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
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

export const spec_setReminder = {
    op: 'setReminder',
    version: '2d0a521a7e007153',
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
      { name: 'due', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$count = {
    op: 'std.attachments.count',
    version: 'd4b600e918b7f4a9',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'name', 'kind', 'size', 'created', 'file'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$create = {
    op: 'std.attachments.create',
    version: '33fa86b571b83dc4',
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
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'kind', type: 'text', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'size', type: 'int', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$delete = {
    op: 'std.attachments.delete',
    version: '59810ac2d454b56e',
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
      { name: 'id', type: 'id', required: true, idEntity: 'attachments', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$get = {
    op: 'std.attachments.get',
    version: '815c8c93e93055da',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'attachments', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'name', 'kind', 'size', 'created', 'file'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', name: 'text', kind: 'text', size: 'int', created: 'datetime', file: 'blob' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'attachments', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$list = {
    op: 'std.attachments.list',
    version: 'd0d6d00d23eb8228',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'attachments', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'name', 'kind', 'size', 'created', 'file'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', name: 'text', kind: 'text', size: 'int', created: 'datetime', file: 'blob' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$attachments$update = {
    op: 'std.attachments.update',
    version: '5c9c49869552d03d',
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
      { name: 'id', type: 'id', required: true, idEntity: 'attachments', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'name', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'kind', type: 'text', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'size', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'created', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
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
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created', 'pinned', 'archived'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$create = {
    op: 'std.notes.create',
    version: '930c79b16660533a',
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
      { name: 'pinned', type: 'bool', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'archived', type: 'datetime', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$delete = {
    op: 'std.notes.delete',
    version: '8d4368b7bbb9d033',
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
    version: '90234d403cc158d9',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created', 'pinned', 'archived'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime', pinned: 'bool', archived: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$list = {
    op: 'std.notes.list',
    version: '2c2e85f8c59342b9',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'notes', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'title', 'body', 'created', 'pinned', 'archived'], decimals: [], types: { id: 'id', rev: 'int', title: 'text', body: 'text', tags: 'json', created: 'datetime', pinned: 'bool', archived: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$notes$update = {
    op: 'std.notes.update',
    version: '71e55fab7a5bd7e6',
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
      { name: 'pinned', type: 'bool', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'archived', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
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

export const spec_std$reminders$count = {
    op: 'std.reminders.count',
    version: '4bc7aa3397b3b0b8',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'due', 'fired'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$create = {
    op: 'std.reminders.create',
    version: '083a70c8dddbc027',
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
      { name: 'due', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'fired', type: 'datetime', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$delete = {
    op: 'std.reminders.delete',
    version: '18932815aa023189',
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
      { name: 'id', type: 'id', required: true, idEntity: 'reminders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$get = {
    op: 'std.reminders.get',
    version: '1c7f85c7c4b6fc29',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'reminders', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'due', 'fired'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', due: 'datetime', fired: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'reminders', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$list = {
    op: 'std.reminders.list',
    version: '3136d71225e48beb',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'reminders', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'rev', 'note', 'due', 'fired'], decimals: [], types: { id: 'id', rev: 'int', note: 'id', due: 'datetime', fired: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$put = {
    op: 'std.reminders.put',
    version: '0c5de4040e1670fd',
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
      { name: 'due', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'fired', type: 'datetime', required: false, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'conflicting_identity', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$reminders$update = {
    op: 'std.reminders.update',
    version: '681ea1eea24a1303',
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
      { name: 'id', type: 'id', required: true, idEntity: 'reminders', enumValues: null, min: null, max: null, places: null },
      { name: 'rev', type: 'int', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'note', type: 'id', required: true, idEntity: 'notes', enumValues: null, min: null, max: null, places: null },
      { name: 'due', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
      { name: 'fired', type: 'datetime', required: true, idEntity: null, enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'duplicate', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_unarchiveNote = {
    op: 'unarchiveNote',
    version: '3b9cb87bf314bbb5',
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
    ],
    refusals: ['access_denied', 'access_guard', 'cas_miss', 'idempotency_in_flight', 'idempotency_mismatch', 'invalid_param', 'log_append_failed', 'method_mismatch', 'missing_param', 'not_found', 'serialization_conflict', 'unknown_param', 'version_skew'],
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
  addAttachment: spec_addAttachment,
  allNotes: spec_allNotes,
  archiveNote: spec_archiveNote,
  clearReminder: spec_clearReminder,
  createNote: spec_createNote,
  deleteNote: spec_deleteNote,
  editNote: spec_editNote,
  grantNotesAccess: spec_grantNotesAccess,
  myFiles: spec_myFiles,
  myNotes: spec_myNotes,
  myReminders: spec_myReminders,
  notesTagged: spec_notesTagged,
  pinNote: spec_pinNote,
  removeAttachment: spec_removeAttachment,
  revokeNotesAccess: spec_revokeNotesAccess,
  setNotesAccessLevel: spec_setNotesAccessLevel,
  setReminder: spec_setReminder,
  'std.attachments.count': spec_std$attachments$count,
  'std.attachments.create': spec_std$attachments$create,
  'std.attachments.delete': spec_std$attachments$delete,
  'std.attachments.get': spec_std$attachments$get,
  'std.attachments.list': spec_std$attachments$list,
  'std.attachments.update': spec_std$attachments$update,
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
  'std.reminders.count': spec_std$reminders$count,
  'std.reminders.create': spec_std$reminders$create,
  'std.reminders.delete': spec_std$reminders$delete,
  'std.reminders.get': spec_std$reminders$get,
  'std.reminders.list': spec_std$reminders$list,
  'std.reminders.put': spec_std$reminders$put,
  'std.reminders.update': spec_std$reminders$update,
  unarchiveNote: spec_unarchiveNote,
  unshareNote: spec_unshareNote,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
