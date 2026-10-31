/**
 * A public op can be called with no token: the call names no space and
 * carries no Authorization. A public read takes no `$refine`.
 */

import { describe, expect, it } from 'vitest';

import { OrdoClient, OrdoLocalRefusal } from '../src/index.js';
import type { OpSpec } from '../src/index.js';
import { MockDoor } from './harness.js';

const base = {
  op: 'pricing',
  version: '0123456789abcdef',
  method: 'GET',
  kind: 'read',
  terminal: ['fetch'],
  paged: false,
  singleRow: false,
  casParam: null,
  params: [],
  refusals: [],
  standingError: null,
  sync: null,
  refine: { tier: 'none', fields: [] },
  order: { keys: [], stated: false },
} as unknown as OpSpec;

const pub: OpSpec = { ...base, public: true };

const anon = (door: MockDoor) =>
  new OrdoClient({ baseUrl: 'https://door.example/api', space: 'system', fetch: door.fetch });

describe('a public op', () => {
  it('with no token sends no Authorization and no X-Ordo-Space', async () => {
    const door = new MockDoor();
    await anon(door).request(pub, {});
    expect(door.calls[0]!.headers['Authorization']).toBeUndefined();
    expect(door.calls[0]!.headers['X-Ordo-Space']).toBeUndefined();
    expect(door.calls[0]!.headers['X-Ordo-Version']).toBe(pub.version);
  });

  it('with a token still sends the token and the space', async () => {
    const door = new MockDoor();
    await door.client().request(pub, {});
    expect(door.calls[0]!.headers['Authorization']).toBe('Bearer tok');
    expect(door.calls[0]!.headers['X-Ordo-Space']).toBe('spc_test');
  });

  it('a non-public op still sends X-Ordo-Space', async () => {
    const door = new MockDoor();
    await anon(door).request(base, {});
    expect(door.calls[0]!.headers['X-Ordo-Space']).toBe('system');
    expect(door.calls[0]!.headers['Authorization']).toBeUndefined();
  });

  it('refuses a refinement locally as invalid_refinement, sending nothing', async () => {
    const door = new MockDoor();
    const err = await anon(door)
      .request(pub, {}, { refine: { where: { all: [] } } as never })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoLocalRefusal);
    expect((err as OrdoLocalRefusal).class).toBe('invalid_refinement');
    expect(door.calls).toHaveLength(0);
  });
});
