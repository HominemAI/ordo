import { describe, expect, it } from 'vitest';
import { OrdoLocalRefusal, OrdoRefusal, dispositionOf } from '../src/refusal.js';
import { MockDoor, client, refusal } from './harness.js';

describe('a round trip per terminal verb', () => {
  it('.fetch() — GET, params in the query string', async () => {
    const door = new MockDoor().program({
      body: { op: 'std.tasks.get', version: 'd168d57a68e79536', rows: [{ id: 'task_1', rev: 3, title: 'Ship' }] },
      headers: { 'X-Ordo-Op-Version': 'd168d57a68e79536' },
    });
    const res = await client(door).fetchOp('std.tasks.get', { params: { id: 'task_1' } });

    expect(door.last.method).toBe('GET');
    expect(door.last.url).toContain('/op/std.tasks.get');
    expect(door.last.query.get('id')).toBe('task_1');
    expect(door.last.body).toBeUndefined();
    expect(res.rows).toHaveLength(1);
    expect(res.servedVersion).toBe('d168d57a68e79536');
    expect(res.stale).toBe(false);
  });

  it('.pages() — GET with the cursor in a header, never the query string', async () => {
    const door = new MockDoor().program({ body: { rows: [], next: 'tok2' } });
    await client(door).fetchPage('std.tasks.list', { page: 'tok1' });

    // SH-6: a cursor in a query string lands in proxy logs, browser history
    // and `Referer`. The header form is the SDK's.
    expect(door.last.headers['X-Ordo-Page']).toBe('tok1');
    expect(door.last.query.get('$page')).toBeNull();
    expect(door.last.url).not.toContain('tok1');
  });

  it('.commit() on a write — POST, params in the body, idempotency key attached', async () => {
    const door = new MockDoor().program({
      body: { op: 'std.tasks.update', mode: 'converges', rows: [{ id: 'task_1', rev: 4 }] },
    });
    const res = await client(door).commit('std.tasks.update', {
      params: { id: 'task_1', rev: 3, project: 'proj_1', assignee: 'mem_1', title: 'Ship', status: 'open' },
    });

    expect(door.last.method).toBe('POST');
    expect(door.last.headers['Content-Type']).toBe('application/json');
    expect(door.last.headers['Idempotency-Key']).toBe('idem-0');
    expect(door.last.body).toMatchObject({ id: 'task_1', rev: 3, title: 'Ship' });
    expect(res.mode).toBe('converges');
  });

  it('.commit() on a delete — DELETE, params in the query string', async () => {
    const door = new MockDoor().program({ body: { op: 'std.tasks.delete', mode: 'deletes', rows: [] } });
    await client(door).commit('std.tasks.delete', { params: { id: 'task_1' } });

    expect(door.last.method).toBe('DELETE');
    expect(door.last.query.get('id')).toBe('task_1');
    expect(door.last.body).toBeUndefined();
    // §1: `Idempotency-Key` is honored on POST/DELETE.
    expect(door.last.headers['Idempotency-Key']).toBe('idem-0');
  });
});

describe('the terminal-verb contract is enforced, not merely documented', () => {
  it('a read cannot be committed', async () => {
    const door = new MockDoor();
    await expect(client(door).commit('std.tasks.list')).rejects.toThrow(/terminates in .fetch\(\)/);
    expect(door.requests).toHaveLength(0);
  });

  it('a write cannot be fetched or paged', async () => {
    const door = new MockDoor();
    await expect(client(door).fetchOp('std.tasks.update')).rejects.toThrow(OrdoLocalRefusal);
    await expect(client(door).fetchPage('std.tasks.delete')).rejects.toThrow(/terminates in .commit\(\)/);
    // Nothing reached the wire: the descriptor already knew.
    expect(door.requests).toHaveLength(0);
  });

  it('wears the door’s own class, so a renderer needs one code path', async () => {
    try {
      await client(new MockDoor()).commit('std.tasks.list');
    } catch (e) {
      expect((e as OrdoRefusal).refusal.class).toBe('method_mismatch');
      expect(e).toBeInstanceOf(OrdoRefusal);
    }
  });
});

describe('the transport envelope', () => {
  it('threads the space as a header on every method, and never in the query', async () => {
    const door = new MockDoor().program({ body: { rows: [] } }, { body: { rows: [] } });
    const c = client(door);
    await c.fetchOp('std.tasks.list');
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_test');
    expect(door.last.query.get('$space')).toBeNull();

    await c.commit('std.tasks.delete', { params: { id: 'task_1' } });
    expect(door.last.headers['X-Ordo-Space']).toBe('spc_test');
    // §2: both present and disagreeing is an error, not a precedence rule.
    // Sending only one carrier means there is nothing to disagree with.
    expect(door.last.body).toBeUndefined();
  });

  it('threads the bearer token, resolving a lazy source', async () => {
    const door = new MockDoor().program({ body: { rows: [] } });
    await client(door, { token: async () => 'fresh-token' }).fetchOp('std.tasks.list');
    expect(door.last.headers['Authorization']).toBe('Bearer fresh-token');
  });

  it('pins the version per op, not registry-wide', async () => {
    const door = new MockDoor().program({ body: { rows: [] } }, { body: { rows: [] } });
    const c = client(door);
    await c.fetchOp('std.tasks.list');
    const listPin = door.last.headers['X-Ordo-Version'];
    await c.fetchOp('std.vendors.list');
    const vendorPin = door.last.headers['X-Ordo-Version'];

    expect(listPin).toBe(c.iface.requireOp('std.tasks.list').version);
    expect(vendorPin).toBe(c.iface.requireOp('std.vendors.list').version);
    // §10: a registry-wide pin marks every call stale after any unrelated
    // deploy, and a signal that fires constantly is one clients ignore.
    expect(listPin).not.toBe(vendorPin);
  });

  it('surfaces skew as a soft signal — the call still executed', async () => {
    const door = new MockDoor().program({
      body: { rows: [{ id: 'task_1' }] },
      headers: { 'X-Ordo-Version-Skew': 'stale', 'X-Ordo-Op-Version': 'newhash' },
    });
    const res = await client(door).fetchOp('std.tasks.list');
    expect(res.stale).toBe(true);
    expect(res.rows).toHaveLength(1);
    expect(res.servedVersion).toBe('newhash');
  });

  it('reports an idempotent replay', async () => {
    const door = new MockDoor().program({
      body: { rows: [{ id: 'task_1' }] },
      headers: { 'X-Ordo-Idempotent-Replay': 'true' },
    });
    const res = await client(door).commit('std.tasks.update', { params: { id: 'task_1', rev: 1 } });
    expect(res.replayed).toBe(true);
  });

  it('reuses a supplied idempotency key and mints one otherwise', async () => {
    const door = new MockDoor().program({ body: { rows: [] } }, { body: { rows: [] } });
    const c = client(door, { newIdempotencyKey: () => `k${door.requests.length}` });
    await c.commit('std.tasks.update', { params: { id: 't_1' }, idempotencyKey: 'mine' });
    expect(door.last.headers['Idempotency-Key']).toBe('mine');
    await c.commit('std.tasks.update', { params: { id: 't_1' } });
    expect(door.last.headers['Idempotency-Key']).toBe('k1');
  });
});

describe('the error envelope decodes into a typed refusal', () => {
  it('carries class, message, op, param and status', async () => {
    const door = new MockDoor().program({
      status: 409,
      body: refusal('cas_miss', 'rev mismatch', { op: 'std.tasks.update', param: 'rev' }),
    });
    try {
      await client(door).commit('std.tasks.update', { params: { id: 'task_1', rev: 1 } });
      throw new Error('expected a refusal');
    } catch (e) {
      const r = (e as OrdoRefusal).refusal;
      expect(r.class).toBe('cas_miss');
      expect(r.message).toBe('rev mismatch');
      expect(r.op).toBe('std.tasks.update');
      expect(r.param).toBe('rev');
      expect(r.status).toBe(409);
      // The op declared this class, which is what tells a renderer it has a
      // hand-written affordance for it.
      expect(r.declared).toBe(true);
      expect(dispositionOf('cas_miss')).toBe('conflict');
    }
  });

  it('marks an undeclared class undeclared rather than flattening it', async () => {
    const door = new MockDoor().program({ status: 403, body: refusal('delegation_required', 'not live') });
    try {
      await client(door).fetchOp('std.tasks.list');
    } catch (e) {
      const r = (e as OrdoRefusal).refusal;
      // The door has refusals that precede any op, so `declared: false` is
      // information, not an error.
      expect(r.class).toBe('delegation_required');
      expect(r.declared).toBe(false);
      expect(dispositionOf('delegation_required')).toBe('denied');
    }
  });

  it('reads sealed_unsupported as a 422 the app developer fixes, never a fault', async () => {
    const door = new MockDoor().program({
      status: 422,
      body: refusal('sealed_unsupported', '`memo` is sealed and the read has no take'),
    });
    try {
      await client(door).fetchOp('std.tasks.list');
      throw new Error('expected a refusal');
    } catch (e) {
      const r = (e as OrdoRefusal).refusal;
      expect(r.class).toBe('sealed_unsupported');
      expect(r.status).toBe(422);
      expect((e as OrdoRefusal).disposition).toBe('request');
      expect(dispositionOf('sealed_unsupported')).toBe('request');
    }
  });

  it('keeps a code this package has never heard of verbatim, as an internal class', async () => {
    const door = new MockDoor().program({ status: 409, body: refusal('some_future_class', 'newer server') });
    const e = await client(door)
      .fetchOp('std.tasks.list')
      .catch((x: unknown) => x);
    // One rule with the SDK: the class is `internal`, and `code` keeps the
    // evidence that the server is newer than this package.
    expect(e).toBeInstanceOf(OrdoRefusal);
    expect((e as OrdoRefusal).refusal.class).toBe('internal');
    expect((e as OrdoRefusal).code).toBe('some_future_class');
    expect((e as OrdoRefusal).refusal.code).toBe('some_future_class');
    expect((e as OrdoRefusal).status).toBe(409);
  });

  it('says so honestly when a non-2xx arrives without an envelope', async () => {
    const door = new MockDoor().program({ status: 502, body: '<html>gateway</html>' });
    try {
      await client(door).fetchOp('std.tasks.list');
    } catch (e) {
      const r = (e as OrdoRefusal).refusal;
      expect(r.class).toBe('internal');
      expect(r.message).toContain('without an Ordo error envelope');
      expect(r.status).toBe(502);
    }
  });
});

describe('a standing error is refused before the round trip', () => {
  it('uses the descriptor’s own code and message', async () => {
    // `giftOrders` carries `error: { code: "sealed_unsupported", … }` because
    // its plan needs a venue this executor does not serve. Clicking a button
    // that can only fail is a UI bug the descriptor already prevents.
    const door = new MockDoor();
    const c = client(door);
    expect(c.iface.requireOp('giftOrders').error?.code).toBe('sealed_unsupported');
    try {
      await c.fetchOp('giftOrders');
      throw new Error('expected a refusal');
    } catch (e) {
      expect(e).toBeInstanceOf(OrdoLocalRefusal);
      expect((e as OrdoRefusal).refusal.class).toBe('sealed_unsupported');
      expect((e as OrdoRefusal).refusal.message).toContain('is sealed');
    }
    expect(door.requests).toHaveLength(0);
  });
});

describe('a refinement reaches the wire; the door decides on the tier', () => {
  it('a refined query is sent as $refine, URL-encoded JSON, with text made NFC', async () => {
    // No local refusal on `refine.tier`: every program read takes a
    // refinement (ruled 2026-10-05), and the door's refusal is typed.
    const door = new MockDoor();
    await client(door).fetchOp('std.tasks.list', {
      refine: {
        where: [
          { field: 'status', op: 'eq', value: 'open' },
          { field: 'title', op: 'eq', value: 'Café' },
          { field: 'assignee', op: 'is_null' },
        ],
        pick: ['id', 'title'],
      },
    });
    expect(door.requests).toHaveLength(1);
    expect(door.last.query.get('$refine')).toBe(
      '{"where":[{"field":"status","op":"eq","value":"open"},{"field":"title","op":"eq","value":"Café"},' +
        '{"field":"assignee","op":"is_null"}],"pick":["id","title"]}',
    );
  });

  it('an envelope that narrows nothing is not sent', async () => {
    const door = new MockDoor();
    await client(door).fetchOp('std.tasks.list', { refine: {} });
    expect(door.last.query.has('$refine')).toBe(false);
  });

  it('refuses before the wire what the door would refuse, naming the field', async () => {
    const door = new MockDoor();
    const c = client(door);
    await expect(
      c.fetchOp('std.tasks.list', { refine: { where: [{ field: 'margin', op: 'eq', value: 1 }] } }),
    ).rejects.toThrow(/unknown refinement field "margin"/);
    await expect(
      c.fetchOp('std.tasks.list', { refine: { where: [{ field: 'rev', op: 'gt', value: '3' }] } }),
    ).rejects.toThrow(/`rev` is `int`/);
    await expect(
      c.fetchOp('orderMemos', { refine: { where: [{ field: 'total', op: 'gt', value: 2.5 }] } }),
    ).rejects.toThrow(/never a float/);
    await expect(
      c.fetchOp('memoDigest', { refine: { where: [{ field: 'memo', op: 'eq', value: 'x' }] } }),
    ).rejects.toThrow(/sealed field and is not refinable/);
    expect(door.requests).toHaveLength(0);
  });

  it('a write is never refinable', async () => {
    const door = new MockDoor();
    await expect(
      client(door).fetchOp('addAnonCustomer', {
        params: { name: 'Ada' },
        refine: { where: [{ field: 'name', op: 'eq', value: 'x' }] },
      }),
    ).rejects.toThrow(/writes are never refinable|method_mismatch/);
  });
});
