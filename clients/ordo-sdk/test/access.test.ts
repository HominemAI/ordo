/**
 * The access and group ops, against a mock door.
 *
 * What is proved is what this package owns: the method, the path, how the
 * parameters are encoded (arrays joined with commas) and the typed results.
 */

import { describe, expect, it } from 'vitest';
import { OrdoNetworkError } from '../src/core/network.js';
import {
  AccessError,
  addToGroup,
  createGroup,
  deleteGroup,
  groups,
  profile,
  removeFromGroup,
  resetOpAccess,
  setMemberAccess,
  setOpAccess,
  type AccessEndpoint,
} from '../src/auth/access.js';

interface Sent {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: Record<string, unknown> | null;
}

function door(...replies: Array<{ status?: number; json?: unknown }>) {
  const sent: Sent[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const raw = init?.body;
    sent.push({
      url: String(input),
      method: String(init?.method ?? 'GET'),
      headers: { ...(init?.headers as Record<string, string>) },
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    });
    const r = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    return new Response(r.json === undefined ? null : JSON.stringify(r.json), {
      status: r.status ?? 200,
    });
  };
  return { sent, fetch };
}

const endpoint = (fetch: typeof globalThis.fetch): AccessEndpoint => ({
  baseUrl: 'https://door.example/',
  fetch,
  space: 'sp1',
  token: 'tok',
});

describe('access ops', () => {
  it('reads the profile with the op in the query', async () => {
    const row = { op: 'a.b', roles: ['editor'], groups: ['g1'], allow: [], deny: ['m1'], changed: true };
    const d = door({ json: { rows: [row] } });
    const rows = await profile(endpoint(d.fetch), 'a.b');
    expect(rows).toEqual([row]);
    expect(rows[0]?.roles).toEqual(['editor']);
    const s = d.sent[0]!;
    expect(s.method).toBe('GET');
    expect(s.url).toBe('https://door.example/op/std.space.profile?op=a.b');
    expect(s.headers['Authorization']).toBe('Bearer tok');
    expect(s.headers['X-Ordo-Space']).toBe('sp1');
    expect(s.body).toBeNull();
  });

  it('reads the whole profile with no query', async () => {
    const d = door({ json: { rows: [] } });
    expect(await profile(endpoint(d.fetch))).toEqual([]);
    expect(d.sent[0]!.url).toBe('https://door.example/op/std.space.profile');
  });

  it('joins roles and groups with commas', async () => {
    const d = door({
      json: { rows: [{ op: 'a.b', roles: ['owner', 'manager'], groups: ['g1', 'g2'], outcome: 'set', door: null }] },
    });
    const row = await setOpAccess(endpoint(d.fetch), { op: 'a.b', roles: ['owner', 'manager'], groups: ['g1', 'g2'] });
    expect(row.roles).toEqual(['owner', 'manager']);
    const s = d.sent[0]!;
    expect(s.method).toBe('POST');
    expect(s.url).toBe('https://door.example/op/std.space.setOpAccess');
    expect(s.body).toEqual({ op: 'a.b', roles: 'owner,manager', groups: 'g1,g2' });
    expect(s.headers['Idempotency-Key']).toBeTruthy();
  });

  it('leaves out roles and groups that are not given', async () => {
    const d = door({ json: { rows: [{ op: 'a.b', roles: [], groups: [], outcome: 'set', door: null }] } });
    await setOpAccess(endpoint(d.fetch), { op: 'a.b' });
    expect(d.sent[0]!.body).toEqual({ op: 'a.b' });
  });

  it('resets an op', async () => {
    const d = door({ json: { rows: [{ op: 'a.b', outcome: 'reset', door: null }] } });
    const row = await resetOpAccess(endpoint(d.fetch), 'a.b');
    expect(row.outcome).toBe('reset');
    expect(d.sent[0]!.url).toBe('https://door.example/op/std.space.resetOpAccess');
    expect(d.sent[0]!.body).toEqual({ op: 'a.b' });
  });

  it('sets a member effect', async () => {
    const d = door({ json: { rows: [{ member: 'm1', op: 'a.b', effect: 'deny', door: null }] } });
    const row = await setMemberAccess(endpoint(d.fetch), { member: 'm1', op: 'a.b', effect: 'deny' });
    expect(row.effect).toBe('deny');
    expect(d.sent[0]!.url).toBe('https://door.example/op/std.space.setMemberAccess');
    expect(d.sent[0]!.body).toEqual({ member: 'm1', op: 'a.b', effect: 'deny' });
  });

  it('creates a group and changes its members', async () => {
    const d = door(
      { json: { rows: [{ group: 'g1', name: 'Staff', outcome: 'created', door: null }] } },
      { json: { rows: [{ group: 'g1', member: 'm1', outcome: 'added', door: null }] } },
      { json: { rows: [{ group: 'g1', member: 'm1', outcome: 'removed', door: null }] } },
    );
    const e = endpoint(d.fetch);
    expect((await createGroup(e, 'Staff')).group).toBe('g1');
    expect((await addToGroup(e, 'g1', 'm1')).outcome).toBe('added');
    expect((await removeFromGroup(e, 'g1', 'm1')).outcome).toBe('removed');
    expect(d.sent.map((s) => [s.method, s.url, s.body])).toEqual([
      ['POST', 'https://door.example/op/std.space.createGroup', { name: 'Staff' }],
      ['POST', 'https://door.example/op/std.space.addToGroup', { group: 'g1', member: 'm1' }],
      ['POST', 'https://door.example/op/std.space.removeFromGroup', { group: 'g1', member: 'm1' }],
    ]);
  });

  it('deletes a group', async () => {
    const d = door({
      json: { rows: [{ group: 'g1', outcome: 'deleted', links: 2, door: null }] },
    });
    const row = await deleteGroup(endpoint(d.fetch), 'g1');
    expect(row).toMatchObject({ outcome: 'deleted', links: 2 });
    expect(d.sent.map((s) => [s.method, s.url, s.body])).toEqual([
      ['POST', 'https://door.example/op/std.space.deleteGroup', { group: 'g1' }],
    ]);
  });

  it('lists groups', async () => {
    const d = door({ json: { rows: [{ group: 'g1', name: 'Staff', members: ['m1', 'm2'] }] } });
    const rows = await groups(endpoint(d.fetch));
    expect(rows[0]?.members).toEqual(['m1', 'm2']);
    expect(d.sent[0]!.method).toBe('GET');
    expect(d.sent[0]!.url).toBe('https://door.example/op/std.space.groups');
  });

  it('turns a refusal into an AccessError', async () => {
    const d = door({ status: 403, json: { error: { code: 'access_denied', message: 'no' } } });
    const err = await groups(endpoint(d.fetch)).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AccessError);
    expect((err as AccessError).code).toBe('access_denied');
    expect((err as AccessError).status).toBe(403);
  });

  it('reports an unreachable door', async () => {
    const e = endpoint(async () => {
      throw new Error('down');
    });
    const err = await groups(e).catch((x: unknown) => x);
    expect(err).toBeInstanceOf(OrdoNetworkError);
    expect((err as OrdoNetworkError).kind).toBe('unreachable');
  });
});
