/**
 * Hooks for the platform ops (`std.space.share` and the rest), and a client
 * for a guest. Neither is in the program's `interface.json`, so both are
 * tested against the golden descriptor with the table added.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react-test-renderer';
import { guestClient } from '../src/guest.js';
import { useCommit } from '../src/hooks/useCommit.js';
import { useOpQuery } from '../src/hooks/useOpQuery.js';
import { PLATFORM_OPS, isPlatformOp, withPlatformOps } from '../src/platform.js';
import { DescriptorClient } from '../src/client.js';
import { OrdoInterface } from '../src/load.js';
import { MockDoor, client, iface, refusal } from './harness.js';
import { mount, until, type Mounted } from './live-door.js';

let mounted: Mounted<unknown> | null = null;
afterEach(() => {
  mounted?.unmount();
  mounted = null;
});

const withSharing = () =>
  withPlatformOps(iface(), 'std.space.share', 'std.space.shareLink', 'std.space.revokeLink', 'std.space.links');
const denied = { status: 403, body: refusal('access_denied', 'no') };
const SHARE_VALUES = {
  ops: 'myNotes',
  rows: JSON.stringify([{ entity: 'notes', id: 'note_1', level: 'read' }]),
  expires: '365d',
  label: 'A note',
};

describe('platform ops through the hooks', () => {
  it('shares with useCommit, as a POST that names the platform version and mints an idempotency key', async () => {
    const door = new MockDoor().program({ body: { rows: [{ ticket: 'tkt_1', expiresAt: 5 }] } });
    const m = await mount(() => useCommit('std.space.share'), { client: client(door, { iface: withSharing() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit(SHARE_VALUES);
    });
    const state = m.result.current.state;
    expect(state.status).toBe('committed');
    // The row is typed by the op's name: no cast to reach `ticket`.
    if (state.status === 'committed') expect(state.response.rows[0]?.ticket).toBe('tkt_1');
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.space.share');
    expect(door.last.headers['X-Ordo-Version']).toBe('platform');
    expect(door.last.headers['Idempotency-Key']).toBe('idem-0');
    expect(door.last.body).toEqual(SHARE_VALUES);
  });

  it('lists links with useOpQuery, as a GET', async () => {
    const link = { id: 'lnk_1', label: 'A note', opens: 2, expiresAt: 9, state: 'live', rows: [{ id: 'note_1' }] };
    const door = new MockDoor().always({ body: { rows: [link] } });
    const m = await mount(() => useOpQuery('std.space.links'), { client: client(door, { iface: withSharing() }) });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the links');
    expect(m.result.current.rows[0]?.state).toBe('live');
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.space.links');
    expect(m.result.current.source).toBe('network');
  });

  it('shows the door’s refusal as a typed refusal for a token that may not share', async () => {
    const door = new MockDoor().always(denied);
    const m = await mount(() => useCommit('std.space.shareLink'), { client: client(door, { iface: withSharing() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ ops: 'myNotes', rows: '[]' });
    });
    expect(m.result.current.refusal?.class).toBe('access_denied');
  });

  it('reaches only the platform ops the app named, so an unnamed one is absent like any op the token cannot call', () => {
    const i = withSharing();
    expect(i.op('std.space.share')).toBeDefined();
    expect(i.op('std.space.revokeShare')).toBeUndefined();
    expect(() => i.requireOp('std.space.revokeShare')).toThrow(/may not call it/);
    // And naming none leaves the interface itself.
    const base = iface();
    expect(withPlatformOps(base)).toBe(base);
  });

  it('leaves the program’s own descriptor alone when the door already carries a platform op', () => {
    const base = iface();
    const carried = { ...PLATFORM_OPS['std.space.links'], version: 'from-the-door' };
    const withCarried = new OrdoInterface({ ...base.doc, ops: [...base.doc.ops, carried] });
    expect(withPlatformOps(withCarried, 'std.space.links').requireOp('std.space.links').version).toBe('from-the-door');
  });

  it('describes each op as a platform op with the verbs the hooks need', () => {
    for (const [name, op] of Object.entries(PLATFORM_OPS)) {
      expect(isPlatformOp(name)).toBe(true);
      expect(op.platform).toBe(true);
      expect(op.refine.tier).toBe('none');
      expect(op.call.terminal).toEqual(op.kind === 'read' ? ['fetch'] : ['commit']);
    }
    expect(isPlatformOp('std.tasks.list')).toBe(false);
  });
});

describe('a platform write that wants a fresh touch', () => {
  it('asks for one on access_denied and tries once more, with a new idempotency key', async () => {
    const door = new MockDoor().program(denied, { body: { rows: [{ ticket: 'tkt_2' }] } });
    let touches = 0;
    const m = await mount(() => useCommit('std.space.share'), {
      client: client(door, { iface: withSharing() }),
      stepUp: async () => {
        touches += 1;
      },
    });
    mounted = m;
    await act(async () => {
      await m.result.current.commit(SHARE_VALUES);
    });
    expect(touches).toBe(1);
    expect(m.result.current.state.status).toBe('committed');
    const sent = door.requests.map((r) => r.headers['Idempotency-Key']);
    expect(sent).toEqual(['idem-0', 'idem-1']);
  });

  it('believes the second refusal, and asks only once', async () => {
    const door = new MockDoor().always(denied);
    let touches = 0;
    const m = await mount(() => useCommit('std.space.share'), {
      client: client(door, { iface: withSharing() }),
      stepUp: async () => {
        touches += 1;
      },
    });
    mounted = m;
    await act(async () => {
      await m.result.current.commit(SHARE_VALUES);
    });
    expect(touches).toBe(1);
    expect(door.requests).toHaveLength(2);
    expect(m.result.current.refusal?.class).toBe('access_denied');
  });

  it('never asks for a program op, which a touch cannot help', async () => {
    const door = new MockDoor().always(denied);
    let touches = 0;
    const m = await mount(() => useCommit('std.tasks.create'), {
      client: client(door),
      stepUp: async () => {
        touches += 1;
      },
    });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ project: 'proj_1', title: 'x' });
    });
    expect(touches).toBe(0);
    expect(door.requests).toHaveLength(1);
  });
});

describe('a guest’s client', () => {
  it('reads through the hooks with the feed’s tenant and token, renewed on every call', async () => {
    const door = new MockDoor().always({ body: { rows: [{ id: 'task_1', title: 'shared' }] } });
    let n = 0;
    const feed = { space: 'spc_sharer', token: () => `win-${++n}` };
    const guest = guestClient({ baseUrl: 'http://door.test', iface: iface(), feed, fetch: door.fetch });
    const m = await mount(() => useOpQuery('std.tasks.list'), { client: guest });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the shared rows');
    expect(guest).toBeInstanceOf(DescriptorClient);
    expect(guest.space).toBe('spc_sharer');
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_sharer');
    expect(door.last.headers['Authorization']).toBe('Bearer win-1');
    await act(async () => {
      m.result.current.refetch();
    });
    await until(() => door.requests.length === 2, 'a second read');
    expect(door.last.headers['Authorization']).toBe('Bearer win-2');
  });

  it('holds no token the page could read or log', () => {
    const feed = { space: 'spc_sharer', token: () => 'win-secret' };
    const guest = guestClient({ baseUrl: 'http://door.test', iface: iface(), feed });
    expect(JSON.stringify(guest)).not.toContain('win-secret');
    expect(Object.keys(guest).sort()).toEqual(['iface', 'space']);
  });
});

describe('the invitation and role ops', () => {
  const withMembers = () =>
    withPlatformOps(
      iface(),
      'std.space.invite',
      'std.space.invitations',
      'std.space.revokeInvitation',
      'std.space.setRole',
    );

  it('are in the table, and an app opts in by name', () => {
    for (const n of ['std.space.invite', 'std.space.invitations', 'std.space.revokeInvitation', 'std.space.setRole']) {
      expect(isPlatformOp(n)).toBe(true);
    }
    expect(iface().op('std.space.setRole')).toBeUndefined();
    expect(withMembers().op('std.space.setRole')?.platform).toBe(true);
    expect(PLATFORM_OPS['std.space.invite'].call.method).toBe('POST');
    expect(PLATFORM_OPS['std.space.invitations'].call.method).toBe('GET');
    expect(PLATFORM_OPS['std.space.setRole'].params.map((p) => [p.name, p.required])).toEqual([
      ['member', true],
      ['role', true],
    ]);
  });

  it('invites with useCommit and reads the ticket typed', async () => {
    const door = new MockDoor().program({ body: { rows: [{ ticket: 'tkt_9', expiresAt: 5, role: 'editor' }] } });
    const m = await mount(() => useCommit('std.space.invite'), { client: client(door, { iface: withMembers() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ role: 'editor', expires: '7d' });
    });
    const state = m.result.current.state;
    expect(state.status).toBe('committed');
    if (state.status === 'committed') expect(state.response.rows[0]?.ticket).toBe('tkt_9');
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.space.invite');
    expect(door.last.headers['X-Ordo-Version']).toBe('platform');
    expect(door.last.body).toEqual({ role: 'editor', expires: '7d' });
  });

  it('sets a role with useCommit', async () => {
    const door = new MockDoor().program({
      body: { rows: [{ member: 'mem_1', principal: 'usr_1', role: 'manager', status: 'active', outcome: 'changed' }] },
    });
    const m = await mount(() => useCommit('std.space.setRole'), { client: client(door, { iface: withMembers() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ member: 'mem_1', role: 'manager' });
    });
    expect(m.result.current.state.status).toBe('committed');
    expect(door.last.url).toContain('/op/std.space.setRole');
    expect(door.last.body).toEqual({ member: 'mem_1', role: 'manager' });
  });

  it('lists invitations with useOpQuery, as a GET', async () => {
    const row = { id: 'inv_1', role: 'editor', for: null, label: 'Ravi', status: 'open', expiresAt: 9 };
    const door = new MockDoor().always({ body: { rows: [row] } });
    const m = await mount(() => useOpQuery('std.space.invitations'), { client: client(door, { iface: withMembers() }) });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the invitations');
    expect(m.result.current.rows[0]?.status).toBe('open');
    expect(door.last.method).toBe('GET');
  });
});

describe('delegation ops through the hooks', () => {
  const withDelegation = () =>
    withPlatformOps(
      iface(),
      'std.delegation.review',
      'std.delegation.approve',
      'std.delegation.decline',
      'std.delegation.mine',
      'std.delegation.revoke',
    );

  it('reviews with useOpQuery, as a GET naming the request', async () => {
    const row = { request: 'dr_1', tenant: 'spc_test', ceiling: 'a ceiling', ops: [] };
    const door = new MockDoor().always({ body: { rows: [row] } });
    const m = await mount(() => useOpQuery('std.delegation.review', { params: { request: 'dr_1' } }), {
      client: client(door, { iface: withDelegation() }),
    });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the disclosure');
    expect(m.result.current.rows[0]?.ceiling).toBe('a ceiling');
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.delegation.review');
    expect(door.last.query.get('request')).toBe('dr_1');
  });

  it('approves with useCommit, as a POST, and shows a missing touch as a typed refusal', async () => {
    const door = new MockDoor().always({
      body: { rows: [{ request: 'dr_1', delegation: 'dlg_1', outcome: 'approved' }] },
    });
    const m = await mount(() => useCommit('std.delegation.approve'), {
      client: client(door, { iface: withDelegation() }),
    });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ request: 'dr_1' });
    });
    const state = m.result.current.state;
    expect(state.status).toBe('committed');
    if (state.status === 'committed') expect(state.response.rows[0]?.delegation).toBe('dlg_1');
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.delegation.approve');
    expect(door.last.headers['X-Ordo-Version']).toBe('platform');
    expect(door.last.body).toEqual({ request: 'dr_1' });

    const refused = new MockDoor().always(denied);
    const r = await mount(() => useCommit('std.delegation.approve'), {
      client: client(refused, { iface: withDelegation() }),
    });
    mounted = r;
    await act(async () => {
      await r.result.current.commit({ request: 'dr_1' });
    });
    expect(r.result.current.refusal?.class).toBe('access_denied');
  });

  it('declines and revokes as POSTs', async () => {
    const door = new MockDoor().always({ body: { rows: [] } });
    const c = client(door, { iface: withDelegation() });
    const d = await mount(() => useCommit('std.delegation.decline'), { client: c });
    mounted = d;
    await act(async () => {
      await d.result.current.commit({ request: 'dr_1' });
    });
    expect(door.last.url).toContain('/op/std.delegation.decline');
    expect(door.last.body).toEqual({ request: 'dr_1' });
    d.unmount();

    const r = await mount(() => useCommit('std.delegation.revoke'), { client: c });
    mounted = r;
    await act(async () => {
      await r.result.current.commit({ delegation: 'dlg_1' });
    });
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.delegation.revoke');
    expect(door.last.body).toEqual({ delegation: 'dlg_1' });
  });

  it('lists the person’s delegations with useOpQuery, as a GET', async () => {
    const row = { request: 'dr_1', delegation: null, agentName: 'Bot', state: 'waiting', ops: ['x'], expiresAt: 9 };
    const door = new MockDoor().always({ body: { rows: [row] } });
    const m = await mount(() => useOpQuery('std.delegation.mine'), { client: client(door, { iface: withDelegation() }) });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the delegations');
    expect(m.result.current.rows[0]?.state).toBe('waiting');
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.delegation.mine');
  });
});

describe('the access and group ops', () => {
  const NAMES = [
    'std.space.profile',
    'std.space.setOpAccess',
    'std.space.resetOpAccess',
    'std.space.setMemberAccess',
    'std.space.createGroup',
    'std.space.addToGroup',
    'std.space.removeFromGroup',
    'std.space.deleteGroup',
    'std.space.groups',
  ] as const;
  const withAccess = () => withPlatformOps(iface(), ...NAMES);

  it('are in the table with the kind, params and fields the door serves', () => {
    const shape = (n: (typeof NAMES)[number]) => ({
      method: PLATFORM_OPS[n].call.method,
      params: PLATFORM_OPS[n].params.map((p) => [p.name, p.required]),
      fields: PLATFORM_OPS[n].result.outputs.map((o) => o.name),
    });
    for (const n of NAMES) expect(isPlatformOp(n)).toBe(true);
    expect(shape('std.space.profile')).toEqual({
      method: 'GET',
      params: [['op', false]],
      fields: ['op', 'roles', 'groups', 'allow', 'deny', 'changed'],
    });
    expect(shape('std.space.setOpAccess')).toEqual({
      method: 'POST',
      params: [['op', true], ['roles', false], ['groups', false]],
      fields: ['op', 'roles', 'groups', 'outcome', 'door'],
    });
    expect(shape('std.space.resetOpAccess')).toEqual({
      method: 'POST',
      params: [['op', true]],
      fields: ['op', 'outcome', 'door'],
    });
    expect(shape('std.space.setMemberAccess')).toEqual({
      method: 'POST',
      params: [['member', true], ['op', true], ['effect', true]],
      fields: ['member', 'op', 'effect', 'door'],
    });
    expect(shape('std.space.createGroup')).toEqual({
      method: 'POST',
      params: [['name', true]],
      fields: ['group', 'name', 'outcome', 'door'],
    });
    for (const n of ['std.space.addToGroup', 'std.space.removeFromGroup'] as const) {
      expect(shape(n)).toEqual({
        method: 'POST',
        params: [['group', true], ['member', true]],
        fields: ['group', 'member', 'outcome', 'door'],
      });
    }
    expect(shape('std.space.deleteGroup')).toEqual({
      method: 'POST',
      params: [['group', true]],
      fields: ['group', 'outcome', 'links', 'door'],
    });
    expect(shape('std.space.groups')).toEqual({ method: 'GET', params: [], fields: ['group', 'name', 'members'] });
    expect(iface().op('std.space.profile')).toBeUndefined();
    expect(withAccess().op('std.space.groups')?.platform).toBe(true);
  });

  it('sets op access with useCommit', async () => {
    const door = new MockDoor().program({
      body: { rows: [{ op: 'myNotes', roles: ['editor'], groups: ['grp_1'], outcome: 'set' }] },
    });
    const m = await mount(() => useCommit('std.space.setOpAccess'), { client: client(door, { iface: withAccess() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ op: 'myNotes', roles: 'editor', groups: 'grp_1' });
    });
    const state = m.result.current.state;
    expect(state.status).toBe('committed');
    if (state.status === 'committed') expect(state.response.rows[0]?.roles).toEqual(['editor']);
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.space.setOpAccess');
    expect(door.last.body).toEqual({ op: 'myNotes', roles: 'editor', groups: 'grp_1' });
  });

  it('reads the profile as a GET', async () => {
    const row = { op: 'myNotes', roles: ['viewer'], groups: [], allow: [], deny: [], changed: true };
    const door = new MockDoor().always({ body: { rows: [row] } });
    const m = await mount(() => useOpQuery('std.space.profile'), { client: client(door, { iface: withAccess() }) });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the profile');
    expect(m.result.current.rows[0]?.changed).toBe(true);
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.space.profile');
  });
});

describe('the membership ops', () => {
  const withMembers = () =>
    withPlatformOps(
      iface(),
      'std.space.remove',
      'std.space.suspend',
      'std.space.reinstate',
      'std.space.leave',
      'std.space.members',
      'std.space.admit',
    );

  it('removes a member with useCommit, typed by name', async () => {
    const seat = { member: 'mem_2', principal: 'p_2', role: 'editor', status: 'removed', outcome: 'removed', door: {} };
    const door = new MockDoor().program({ body: { rows: [seat] } });
    const m = await mount(() => useCommit('std.space.remove'), { client: client(door, { iface: withMembers() }) });
    mounted = m;
    await act(async () => {
      await m.result.current.commit({ member: 'mem_2' });
    });
    const state = m.result.current.state;
    expect(state.status).toBe('committed');
    if (state.status === 'committed') expect(state.response.rows[0]?.status).toBe('removed');
    expect(door.last.method).toBe('POST');
    expect(door.last.url).toContain('/op/std.space.remove');
    expect(door.last.body).toEqual({ member: 'mem_2' });
  });

  it('lists members a page at a time; leave takes no params', async () => {
    const door = new MockDoor().always({ body: { rows: [{ member: 'mem_1', name: 'Ana', role: 'owner', groups: [] }] } });
    const m = await mount(() => useOpQuery('std.space.members', { params: { after: 'mem_0' } }), {
      client: client(door, { iface: withMembers() }),
    });
    mounted = m;
    await until(() => m.result.current.rows.length === 1, 'the members');
    expect(m.result.current.rows[0]?.name).toBe('Ana');
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.space.members?after=mem_0');
    const i = withMembers();
    expect(i.op('std.space.leave')?.params).toEqual([]);
    expect(i.op('std.space.admit')?.params.map((p) => p.name)).toEqual(['principal', 'role', 'display']);
  });
});
