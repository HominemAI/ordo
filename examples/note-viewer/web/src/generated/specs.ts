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
  domain: 'noteviewer',
  digest: '893323dbcf89c4e1',
} as const;

export const spec_myNotes = {
    op: 'myNotes',
    version: 'e378212ae36588dd',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'entries', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_publicNotes = {
    op: 'publicNotes',
    version: '256df79f92babaee',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'published', key: 'id' },
    order: { keys: [{ path: 'created', desc: true }, { path: 'id', desc: false }], stated: true },
    refine: { tier: 'none', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', created: 'datetime' }, sealed: [], limits: null },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'method_mismatch', 'unknown_param', 'version_skew', 'invalid_refinement', 'refinement_unbounded'],
    standingError: null,
    public: true,
} as const satisfies OpSpec;

export const spec_std$entries$count = {
    op: 'std.entries.count',
    version: '60b61aef3a569c45',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$entries$get = {
    op: 'std.entries.get',
    version: '714f491b9f4b9489',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'entries', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'entries', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$entries$list = {
    op: 'std.entries.list',
    version: '97a25094f8eceb99',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'entries', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$entryAccess$count = {
    op: 'std.entryAccess.count',
    version: 'a985027120d1e5d1',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'entry', 'member'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$entryAccess$get = {
    op: 'std.entryAccess.get',
    version: '42c7aea4752c5cda',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'entryAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'entry', 'member'], decimals: [], types: { id: 'id', entry: 'id', member: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'entryAccess', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$entryAccess$list = {
    op: 'std.entryAccess.list',
    version: '596685d72c11472f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'entryAccess', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'entry', 'member'], decimals: [], types: { id: 'id', entry: 'id', member: 'text' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$published$count = {
    op: 'std.published.count',
    version: 'a93d543d26868491',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live'],
    paged: false,
    singleRow: true,
    sync: null,
    order: { keys: [{ path: 'id', desc: false }], stated: false },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { n: 'int' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$published$get = {
    op: 'std.published.get',
    version: '66d5c19055269e1f',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'published', key: 'id' },
    order: { keys: [{ path: 'id', desc: false }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
      { name: 'id', type: 'id', required: true, idEntity: 'published', enumValues: null, min: null, max: null, places: null },
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_param', 'invalid_refinement', 'method_mismatch', 'missing_param', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const spec_std$published$list = {
    op: 'std.published.list',
    version: '67d17e55f822abea',
    method: 'GET',
    kind: 'read',
    terminal: ['fetch', 'live', 'pages'],
    paged: true,
    singleRow: false,
    sync: { entity: 'published', key: 'id' },
    order: { keys: [{ path: 'id', desc: true }], stated: true },
    refine: { tier: 'basic', fields: ['id', 'title', 'body', 'created'], decimals: [], types: { id: 'id', title: 'text', body: 'text', tags: 'json', created: 'datetime' }, sealed: [], limits: { conditions: 16, inValues: 200 } },
    casParam: null,
    params: [
    ],
    refusals: ['access_denied', 'invalid_cursor', 'invalid_refinement', 'method_mismatch', 'refinement_unbounded', 'unknown_param', 'version_skew'],
    standingError: null,
} as const satisfies OpSpec;

export const SPECS = {
  myNotes: spec_myNotes,
  publicNotes: spec_publicNotes,
  'std.entries.count': spec_std$entries$count,
  'std.entries.get': spec_std$entries$get,
  'std.entries.list': spec_std$entries$list,
  'std.entryAccess.count': spec_std$entryAccess$count,
  'std.entryAccess.get': spec_std$entryAccess$get,
  'std.entryAccess.list': spec_std$entryAccess$list,
  'std.published.count': spec_std$published$count,
  'std.published.get': spec_std$published$get,
  'std.published.list': spec_std$published$list,
} as const satisfies Record<string, OpSpec>;

export type OpName = keyof typeof SPECS;

let list: readonly OpSpec[] | undefined;

/** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */
export function specList(): readonly OpSpec[] {
  return (list ??= Object.values(SPECS));
}
