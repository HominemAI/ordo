import { describe, expect, it } from 'vitest';
import { commitAddress, commitOnce, retryAfterConflict } from '../src/commit.js';
import { MockDoor, client, iface, refusal } from './harness.js';

const casMiss = { status: 409, body: refusal('cas_miss', 'rev mismatch', { param: 'rev' }) };

const values = {
  id: 'task_01J8ZK',
  project: 'proj_01J8ZK',
  assignee: 'mem_01J8ZK',
  title: 'Ship it',
  status: 'open',
};

describe('CAS is the default, not an option', () => {
  it('a CAS op committed without a rev is refused before the wire', async () => {
    const door = new MockDoor();
    const out = await commitOnce(client(door), 'std.tasks.update', { values });

    expect(out.status).toBe('refused');
    if (out.status !== 'refused') return;
    expect(out.refusal.class).toBe('missing_param');
    expect(out.refusal.param).toBe('rev');
    expect(out.refusal.message).toContain('no last-write-wins path');
    // The point: there is no request the runtime would have sent.
    expect(door.requests).toHaveLength(0);
  });

  it('a supplied rev rides the write', async () => {
    const door = new MockDoor().program({ body: { rows: [{ id: values.id, rev: 8 }] } });
    const out = await commitOnce(client(door), 'std.tasks.update', { values, rev: 7 });
    expect(out.status).toBe('committed');
    expect(door.last.body).toMatchObject({ ...values, rev: 7 });
  });

  it('a rev on an op that declares none is refused as unknown_param', async () => {
    const door = new MockDoor();
    const out = await commitOnce(client(door), 'std.tasks.create', {
      values: { project: 'proj_1', assignee: 'mem_1', title: 't', status: 'open' },
      rev: 1,
    });
    expect(out.status).toBe('refused');
    if (out.status !== 'refused') return;
    expect(out.refusal.class).toBe('unknown_param');
    expect(door.requests).toHaveLength(0);
  });

  it('a non-CAS write goes through untouched', async () => {
    const door = new MockDoor().program({ body: { rows: [{ id: 'task_new' }] } });
    const out = await commitOnce(client(door), 'std.tasks.create', {
      values: { project: 'proj_1', assignee: 'mem_1', title: 't', status: 'open' },
    });
    expect(out.status).toBe('committed');
    expect(door.last.body).not.toHaveProperty('rev');
  });
});

describe('cas_miss is a conflict state the consumer must handle', () => {
  it('never retries, and hands back everything a merge UI needs', async () => {
    const door = new MockDoor().program(casMiss);
    const out = await commitOnce(client(door), 'std.tasks.update', { values, rev: 7 });

    expect(door.requests).toHaveLength(1); // exactly one attempt. No retry.
    expect(out.status).toBe('conflict');
    if (out.status !== 'conflict') return;
    expect(out.conflict.kind).toBe('cas_miss');
    expect(out.conflict.sentRev).toBe(7);
    expect(out.conflict.attempted).toEqual(values);
    expect(out.conflict.address).toEqual({
      param: 'id',
      entity: 'tasks',
      id: values.id,
      detailOp: 'std.tasks.get',
    });
  });

  it('the refetch affordance reads the row back through its detail op', async () => {
    const door = new MockDoor().program(casMiss, { body: { rows: [{ id: values.id, rev: 9, title: 'Renamed' }] } });
    const out = await commitOnce(client(door), 'std.tasks.update', { values, rev: 7 });
    if (out.status !== 'conflict') throw new Error(out.status);

    const current = await out.conflict.refetch();
    expect(current).toEqual({ id: values.id, rev: 9, title: 'Renamed' });
    // A GET through the catalog's own detail op, addressed by the id param.
    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.tasks.get');
    expect(door.last.query.get('id')).toBe(values.id);
  });

  it('finds the address through statements ∩ id params, whatever the param is called', () => {
    const i = iface();
    // `renameTaskIf` addresses its row through `task`, not `id`.
    expect(commitAddress(i.requireOp('renameTaskIf'), { task: 'task_1', title: 'x' }, 'std.tasks.get')).toEqual({
      param: 'task',
      entity: 'tasks',
      id: 'task_1',
      detailOp: 'std.tasks.get',
    });
    // `moveBA` takes two ids; only one addresses the entity it writes.
    expect(commitAddress(i.requireOp('moveBA'), { ba: 'boar-acce_1', b: 'board_1' }, null)?.param).toBe('ba');
  });

  it('returns no address rather than guessing at the wrong row', () => {
    const i = iface();
    // `std.boardsAccess.create` writes boardsAccess and takes ids of boards
    // and members — neither addresses the written row, because it does not
    // exist yet.
    expect(commitAddress(i.requireOp('std.boardsAccess.create'), { board: 'board_1', member: 'mem_1' }, null)).toBeNull();
  });

  it('a refetch with no detail op available answers null instead of inventing one', async () => {
    const door = new MockDoor().program(casMiss);
    const out = await commitOnce(client(door), 'reviseEntry', { values: { ext: 'x', amount: '1.00' }, rev: 1 });
    if (out.status !== 'conflict') throw new Error(out.status);
    // `reviseEntry` addresses its row by an external key, not a typed id, so
    // the runtime has nothing to refetch with and says so.
    expect(out.conflict.address).toBeNull();
    expect(await out.conflict.refetch()).toBeNull();
  });
});

describe('the retry path cannot quietly re-send a losing write', () => {
  it('refuses a retry that reuses the conflicted rev', async () => {
    const door = new MockDoor().program(casMiss);
    const c = client(door);
    const out = await commitOnce(c, 'std.tasks.update', { values, rev: 7 });
    if (out.status !== 'conflict') throw new Error(out.status);

    const again = await retryAfterConflict(c, 'std.tasks.update', out.conflict, { values, rev: 7 });
    expect(again.status).toBe('refused');
    if (again.status !== 'refused') return;
    expect(again.refusal.class).toBe('cas_miss');
    expect(again.refusal.message).toContain('conflict.refetch()');
    expect(door.requests).toHaveLength(1); // still one. The retry never left.
  });

  it('accepts a retry against the refetched revision', async () => {
    const door = new MockDoor().program(
      casMiss,
      { body: { rows: [{ id: values.id, rev: 9, title: 'Renamed' }] } },
      { body: { rows: [{ id: values.id, rev: 10 }] } },
    );
    const c = client(door);
    const out = await commitOnce(c, 'std.tasks.update', { values, rev: 7 });
    if (out.status !== 'conflict') throw new Error(out.status);

    const current = await out.conflict.refetch();
    const merged = { ...values, title: `${String(current?.['title'])} (mine)` };
    const again = await retryAfterConflict(c, 'std.tasks.update', out.conflict, { values: merged, rev: 9 });

    expect(again.status).toBe('committed');
    expect(door.last.body).toMatchObject({ rev: 9, title: 'Renamed (mine)' });
    // A new attempt is a new payload, so it needs a new idempotency key: the
    // same key with a changed payload is idempotency_mismatch, and for a
    // creating op that binding is permanent (§9).
    const keys = door.requests.filter((r) => r.method === 'POST').map((r) => r.headers['Idempotency-Key']);
    expect(new Set(keys).size).toBe(keys.length);
  });
});

describe('every other refusal stays a refusal, not a conflict', () => {
  it('access_guard does not become a merge dialog', async () => {
    const door = new MockDoor().program({ status: 403, body: refusal('access_guard', 'guard refused') });
    const out = await commitOnce(client(door), 'std.tasks.update', { values, rev: 7 });
    expect(out.status).toBe('refused');
    if (out.status !== 'refused') return;
    expect(out.refusal.class).toBe('access_guard');
  });

  it('duplicate is a conflict class but not a CAS conflict — no rev to remerge', async () => {
    const door = new MockDoor().program({ status: 409, body: refusal('duplicate', 'unique violation') });
    const out = await commitOnce(client(door), 'std.tasks.update', { values, rev: 7 });
    expect(out.status).toBe('refused');
  });
});
