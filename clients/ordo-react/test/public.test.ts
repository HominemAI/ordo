import { describe, expect, it } from 'vitest';
import { DescriptorClient } from '../src/client.js';
import { parseInterface } from '../src/load.js';
import { OrdoLocalRefusal } from '../src/refusal.js';
import { GOLDEN, MockDoor, client } from './harness.js';

// The golden, with `std.tasks.list` marked public as the emitter would.
const doc = GOLDEN as unknown as { ops: Record<string, unknown>[] };
const publicIface = parseInterface({
  ...GOLDEN,
  ops: doc.ops.map((o) => (o['op'] === 'std.tasks.list' ? { ...o, public: true } : o)),
});

const anon = (door: MockDoor) =>
  new DescriptorClient({ baseUrl: 'http://door.test', iface: publicIface, space: 'system', fetch: door.fetch });

describe('a public op', () => {
  it('with no token sends no Authorization and no X-Ordo-Space', async () => {
    const door = new MockDoor();
    await anon(door).fetchPage('std.tasks.list');
    expect(door.last.headers['Authorization']).toBeUndefined();
    expect(door.last.headers['X-Ordo-Space']).toBeUndefined();
    expect(door.last.headers['X-Ordo-Version']).toBeDefined();
  });

  it('with a token still sends the token and the space', async () => {
    const door = new MockDoor();
    await client(door, { iface: publicIface }).fetchPage('std.tasks.list');
    expect(door.last.headers['Authorization']).toBe('Bearer tok-abc');
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_test');
  });

  it('a non-public op with no token still sends X-Ordo-Space', async () => {
    const door = new MockDoor();
    await anon(door).fetchOp('std.tasks.get', { params: { id: 'task_1' } });
    expect(door.last.headers['X-Ordo-Space']).toBe('system');
    expect(door.last.headers['Authorization']).toBeUndefined();
  });

  it('refuses a refinement before the wire as invalid_refinement', async () => {
    const door = new MockDoor();
    const err = await anon(door)
      .fetchPage('std.tasks.list', { refine: { where: { title: 'x' } } as never })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OrdoLocalRefusal);
    expect((err as OrdoLocalRefusal).class).toBe('invalid_refinement');
    expect(door.requests).toHaveLength(0);
  });
});
