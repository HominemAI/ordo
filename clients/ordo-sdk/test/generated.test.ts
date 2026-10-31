/**
 * The generated surface, against a mock door.
 *
 * Every assertion here is about a fact the generator read off the descriptor
 * and turned into either a type or a row of the op table — never about a
 * name. There is no op-name heuristic in this package, which is the property
 * `ordo.interface/2`'s `call` stamp exists to buy, and the way to check it is
 * to assert the routing decisions came from the spec.
 */

import { describe, expect, it } from 'vitest';

import { MockDoor, q } from './harness.js';
import {
  DESCRIPTOR,
  SPECS,
  specList,
  activeProducts,
  addAnonCustomer,
  cheapActive,
  giftOrders,
  grantBoardsAccess,
  memoDigest,
  ops,
  renameTaskIf,
  specCount,
} from '../src/generated/index.js';
import { OrdoLocalRefusal, OrdoRefusal, REFUSAL_CLASSES } from '../src/core/refusal.js';
import { ReadCall, SingleReadCall, WriteCall } from '../src/core/call.js';
import { checkParams } from '../src/core/client.js';

describe('the generated table is the descriptor', () => {
  it('names the descriptor it came from', () => {
    expect(DESCRIPTOR.schemaVersion).toBe('ordo.interface/2');
    expect(DESCRIPTOR.domain).toBe('market');
    expect(DESCRIPTOR.digest).toMatch(/^[0-9a-f]{16}$/);
  });

  it('covers every op in the catalog', () => {
    // Non-vacuity, and the count is the golden's. A generator that emitted
    // half the catalog would pass every other test in this file.
    expect(specList().length).toBe(165);
    expect(Object.keys(ops).length).toBe(specList().length);
  });

  it('every spec carries a per-op version pin', () => {
    for (const spec of specList()) {
      expect(spec.version, spec.op).toMatch(/^[0-9a-f]{16}$/);
    }
  });

  it('every declared refusal is a member of the closed inventory', () => {
    // The weaker check the package can actually make. The inventory is closed
    // by contract (`api-engine-design.md` §4) — the door cannot construct a
    // class outside it — so this catches the copy in `core/refusal.ts` going
    // stale against a deployment, which is the way it would really break.
    const known = new Set<string>(REFUSAL_CLASSES);
    for (const spec of specList()) {
      for (const cls of spec.refusals) {
        expect(known.has(cls), `${spec.op} declares \`${cls}\``).toBe(true);
      }
    }
  });

  it('a dotted op name survives both spellings', () => {
    // `std.tasks.update` is a legal op name and an illegal binding, so the
    // generator mangles one and keeps the other. The wire only ever sees the
    // real name.
    expect(SPECS['std.tasks.update'].op).toBe('std.tasks.update');
    expect(ops['std.tasks.update']).toBeTypeOf('function');
  });
});

describe('the terminal-verb split', () => {
  it('a paged read is a ReadCall', () => {
    expect(activeProducts(new MockDoor().client())).toBeInstanceOf(ReadCall);
  });

  it('a single-row read has no second page', () => {
    // `ordo.interface/2` narrowed `call.terminal` and `call.paged` on exactly
    // these; this is what that narrowing buys. `.pages` is not a method that
    // throws — it does not exist.
    const call = specCount(new MockDoor().client());
    expect(call).toBeInstanceOf(SingleReadCall);
    expect('pages' in call).toBe(false);
    expect(SPECS.specCount.singleRow).toBe(true);
  });

  it('a write terminates in commit and has no read verbs', () => {
    const call = addAnonCustomer(new MockDoor().client(), { name: 'Ada' });
    expect(call).toBeInstanceOf(WriteCall);
    expect('fetch' in call).toBe(false);
    expect('pages' in call).toBe(false);
    expect(typeof call.commit).toBe('function');
  });
});

describe('routing comes off the spec, never the name', () => {
  it('a read is a GET with params in the query string', async () => {
    const door = new MockDoor().reply({ json: { rows: [{ name: 'saw', price: '10.00' }] } });
    const res = await cheapActive(door.client(), { cap: '4000.00' }).fetch();
    expect(res.rows).toHaveLength(1);

    const call = door.calls[0]!;
    expect(call.method).toBe('GET');
    expect(call.url).toContain('/op/cheapActive');
    expect(q(call.url).get('cap')).toBe('4000.00');
    expect(call.headers['X-Ordo-Space']).toBe('spc_test');
    expect(call.headers['X-Ordo-Version']).toBe(SPECS.cheapActive.version);
    expect(call.headers['Authorization']).toBe('Bearer tok');
    // A read carries no idempotency key. The door ignores one, but sending
    // it would still be a lie about what the call is.
    expect(call.headers['Idempotency-Key']).toBeUndefined();
  });

  it('a write is a POST with a JSON body and an idempotency key', async () => {
    const door = new MockDoor().reply({ json: { rows: [{ created: 'cust_1' }] } });
    await addAnonCustomer(door.client(), { name: 'Ada' }).commit();

    const call = door.calls[0]!;
    expect(call.method).toBe('POST');
    expect(call.body).toEqual({ name: 'Ada' });
    expect(call.headers['Idempotency-Key']).toBe('idem-fixed');
  });

  it('a supplied idempotency key makes a retry a replay', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    await addAnonCustomer(door.client(), { name: 'Ada' }).commit({ idempotencyKey: 'k-1' });
    expect(door.calls[0]!.headers['Idempotency-Key']).toBe('k-1');
  });

  it('a cursor rides as a header, never the query string', async () => {
    // SH-6: a cursor in a query string ends up in proxy logs, browser
    // history and `Referer`.
    const door = new MockDoor().reply({ json: { rows: [], next: undefined } });
    await activeProducts(door.client()).fetch({ page: 'cur_abc' });
    const call = door.calls[0]!;
    expect(call.headers['X-Ordo-Page']).toBe('cur_abc');
    expect(call.url).not.toContain('cur_abc');
  });
});

describe('paging terminates on a missing next and on nothing else', () => {
  it('an empty page in the middle does not stop the loop', async () => {
    // §7's consequence: a page window applies before any narrowing, so short
    // and empty pages are ordinary and a loop that stopped on one would
    // silently lose the rows after it.
    const door = new MockDoor().reply(
      { json: { rows: [{ name: 'a' }], next: 'c1' } },
      { json: { rows: [], next: 'c2' } },
      { json: { rows: [{ name: 'b' }] } },
    );
    const rows = await activeProducts(door.client()).rows();
    expect(rows.map((r) => r.name)).toEqual(['a', 'b']);
    expect(door.calls).toHaveLength(3);
    expect(door.calls[1]!.headers['X-Ordo-Page']).toBe('c1');
    expect(door.calls[2]!.headers['X-Ordo-Page']).toBe('c2');
  });

  it('collects a page of 200,000 rows without overflowing the argument limit', async () => {
    const big = Array.from({ length: 200_000 }, (_, i) => ({ name: String(i) }));
    const door = new MockDoor().reply({ json: { rows: big } });
    const rows = await activeProducts(door.client()).rows();
    expect(rows).toHaveLength(200_000);
    expect(rows[199_999]!.name).toBe('199999');
  });

  it('hasMore is `next !== undefined`, never `rows.length > 0`', async () => {
    const door = new MockDoor().reply({ json: { rows: [], next: 'c1' } });
    const page = await activeProducts(door.client()).fetch();
    expect(page.rows).toHaveLength(0);
    expect(page.hasMore).toBe(true);
  });
});

describe('parameters are bound before the wire, in the door’s own order', () => {
  it('an unknown key is refused first', () => {
    // The door's order (§5): typos surface before "missing" noise.
    expect(() =>
      // @ts-expect-error — the generated params type has no `nope`, which is
      // the compile-time half of the same refusal.
      cheapActive(new MockDoor().client(), { cap: '1.00', nope: 1 }),
    ).toThrowError(/unknown_param/);
  });

  it('a missing required parameter is refused locally', () => {
    expect(() =>
      // @ts-expect-error — `cap` is required.
      cheapActive(new MockDoor().client(), {}),
    ).toThrowError(/missing_param/);
  });

  it('money must be an exact decimal string, never a number', () => {
    expect(() =>
      // @ts-expect-error — a number here is the bug that rounds money.
      cheapActive(new MockDoor().client(), { cap: 4000 }),
    ).toThrowError(/never a float/);
  });

  it('money finer than its places is refused before the wire', () => {
    // The fixture's `money` is `money(0)`: whole units (27ak). "40.50"
    // would have to round to be stored, so the client refuses it as the
    // door does; a trailing zero names no value and is accepted.
    expect(SPECS.cheapActive.params[0]?.places).toBe(0);
    expect(() => cheapActive(new MockDoor().client(), { cap: '40.50' })).toThrowError(
      /money\(0\)/,
    );
    expect(() => cheapActive(new MockDoor().client(), { cap: '40.00' })).not.toThrow();
  });

  it('a text(n) bound counts characters, as the door does, not UTF-16 units', () => {
    // No op in the golden carries a text bound yet, so one is written here
    // over a real spec: `text(3)`.
    const base = SPECS.cheapActive;
    const spec = {
      ...base,
      params: [{ ...base.params[0]!, name: 'note', type: 'text', min: null, max: '3', places: null }],
    };
    // Three emoji are six UTF-16 units and three characters: accepted,
    // as the door's `chars().count()` accepts them.
    expect(() => checkParams(spec, { note: '😀😀😀' })).not.toThrow();
    expect(() => checkParams(spec, { note: 'abcd' })).toThrowError(/bound_violation|at most 3 characters/);
  });

  it('a json parameter takes any JSON value and refuses what the wire would change', () => {
    // No op in the golden takes a `json` parameter, so one is written here
    // over a real spec (28b).
    const base = SPECS.cheapActive;
    const spec = {
      ...base,
      params: [{ ...base.params[0]!, name: 'doc', type: 'json', min: null, max: null, places: null }],
    };
    for (const doc of [{ a: [1, 'two', null] }, [1, 2], 'text', 3.5, true]) {
      expect(() => checkParams(spec, { doc })).not.toThrow();
    }
    expect(() => checkParams(spec, { doc: { at: new Date() } as never })).toThrowError(/invalid_param|`json`/);
    expect(() => checkParams(spec, { doc: [Number.NaN] })).toThrowError(/invalid_param|`json`/);
  });

  it('an enumerated parameter refuses a value outside its set', () => {
    // `enum_values` is a `/2` fact: the descriptor resolves the param's
    // `field_ref` against the entity half of the same document. Before it,
    // this was a `text` param with no link back and no way to know.
    expect(() =>
      grantBoardsAccess(new MockDoor().client(), {
        board: 'board_1' as never,
        member: 'memb_1' as never,
        // @ts-expect-error — the generated type is the closed union.
        level: 'owner',
      }),
    ).toThrowError(/read, write, admin/);
  });
});

describe('CAS is unavoidable, not merely available', () => {
  it('a revision-asserting write refuses a commit with no rev', () => {
    // `ui-gen.md` §1: the fetched row's `rev` rides the write. There is no
    // last-write-wins path through this package — the refusal is local and
    // wears the door's own class.
    expect(SPECS.renameTaskIf.casParam).toBe('rev');
    expect(() =>
      // @ts-expect-error — `rev` is a required parameter of the op.
      renameTaskIf(new MockDoor().client(), { task: 'task_1', title: 'x' }),
    ).toThrowError(/asserts a revision/);
  });

  it('`call.cas` is a revision assertion, not a mutation flag', () => {
    // The `/1` → `/2` change, checked from the consumer side: an ordinary
    // create asserts nothing.
    expect(SPECS.addAnonCustomer.casParam).toBeNull();
  });
});

describe('refusals are typed, never stringly', () => {
  it('the door’s envelope becomes a typed refusal', async () => {
    const door = new MockDoor().reply({
      status: 409,
      json: { error: { code: 'cas_miss', message: 'the row moved', op: 'renameTaskIf' } },
    });
    const call = renameTaskIf(door.client(), { task: 'task_1' as never, title: 'x', rev: 3 });
    await expect(call.commit()).rejects.toThrowError(OrdoRefusal);
    await call.commit().catch((e: unknown) => {
      const r = e as OrdoRefusal;
      expect(r.class).toBe('cas_miss');
      expect(r.detail.status).toBe(409);
      expect(r.detail.declared).toBe(true);
    });
  });

  it('a class this client does not know becomes `internal`, and says so', async () => {
    // The inventory is closed by contract, so an unknown class means the
    // deployment is newer than this generated client. Guessing which of the
    // 48 it resembles would be worse than saying so.
    const door = new MockDoor().reply({
      status: 400,
      json: { error: { code: 'brand_new_class' } },
    });
    await activeProducts(door.client())
      .fetch()
      .catch((e: unknown) => {
        const r = e as OrdoRefusal;
        expect(r.class).toBe('internal');
        expect(r.detail.message).toContain('regenerate');
      });
  });

  it('a standing registration failure refuses before the wire', async () => {
    // `giftOrders` projects a sealed field the executor cannot open; the
    // descriptor already carries the refusal, so the round trip is pointless
    // and the local error wears the descriptor's own class.
    expect(SPECS.giftOrders.standingError?.code).toBe('sealed_unsupported');
    const door = new MockDoor();
    await expect(giftOrders(door.client()).fetch()).rejects.toThrowError(OrdoLocalRefusal);
    expect(door.calls).toHaveLength(0);
  });
});

describe('the envelope', () => {
  it('reads the drift and replay headers', async () => {
    const door = new MockDoor().reply({
      json: { rows: [] },
      headers: {
        'X-Ordo-Op-Version': 'deadbeefdeadbeef',
        'X-Ordo-Version-Skew': 'stale',
        'X-Ordo-Idempotent-Replay': 'true',
      },
    });
    const res = await activeProducts(door.client()).fetch();
    expect(res.servedVersion).toBe('deadbeefdeadbeef');
    expect(res.stale).toBe(true);
    expect(res.replayed).toBe(true);
  });

  it('reads the chain position and the signed head', async () => {
    const door = new MockDoor().reply({
      json: { rows: [] },
      headers: {
        'X-Ordo-Position': '42',
        'X-Ordo-Head': 'ab'.repeat(32),
        'X-Ordo-Head-Key': 'chain-key-2026',
        'X-Ordo-Head-Sig': 'cd'.repeat(64),
        'X-Ordo-Verifiable-From': '7',
      },
    });
    const res = await activeProducts(door.client()).fetch();
    expect(res.pin).toEqual({
      position: 42,
      head: 'ab'.repeat(32),
      keyId: 'chain-key-2026',
      signature: 'cd'.repeat(64),
      verifiableFrom: 7,
    });
  });

  it('a response with no position has no pin', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    const res = await activeProducts(door.client()).fetch();
    expect(res.pin).toBeNull();
  });

  it('a position it cannot hold exactly is no pin, never a rounded one', async () => {
    // A replica orders rows by position, so a number rounded to the nearest
    // representable value would order them wrongly without saying so.
    for (const position of ['-1', '4.2', '0x10', '9007199254740993', '']) {
      const door = new MockDoor().reply({
        json: { rows: [] },
        headers: { 'X-Ordo-Position': position, 'X-Ordo-Head': 'ab'.repeat(32) },
      });
      const res = await activeProducts(door.client()).fetch();
      expect(res.pin, position).toBeNull();
    }
  });

  it('a malformed 200 is empty rows, not a crash', async () => {
    // Defending against `rows`' absence rather than trusting it keeps a bad
    // response from becoming a crash three call sites away.
    const door = new MockDoor().reply({ json: { op: 'activeProducts' } });
    const res = await activeProducts(door.client()).fetch();
    expect(res.rows).toEqual([]);
  });
});

describe('refinement: the builder reaches the wire as $refine', () => {
  const sent = (url: string): unknown => JSON.parse(q(url).get('$refine') ?? 'null') as unknown;

  it('a refined GET carries the envelope as URL-encoded JSON, in the exact wire form', async () => {
    const door = new MockDoor().reply({ json: { rows: [{ name: 'saw', price: '4100' }] } });
    const res = await cheapActive(door.client(), { cap: '9000' })
      .where((v) => [v.price.gt('4000'), v.name.in(['saw', 'vise'])])
      .pick('name', 'price')
      .fetch();
    expect(res.rows[0]?.price).toBe('4100');

    const url = door.calls[0]!.url;
    // One query key, `$refine`, whose value is the JSON the router parses.
    expect(url).toContain('%24refine=');
    expect(q(url).get('$refine')).toBe(
      '{"where":[{"field":"price","op":"gt","value":"4000"},{"field":"name","op":"in","value":["saw","vise"]}],' +
        '"pick":["name","price"]}',
    );
    // Parameters ride beside it, unchanged.
    expect(q(url).get('cap')).toBe('9000');
  });

  it('`is_null` carries no value key, and an unrefined call sends no $refine', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    await activeProducts(door.client()).where((v) => v.vendor.isNull()).fetch();
    await activeProducts(door.client()).fetch();
    expect(sent(door.calls[0]!.url)).toEqual({ where: [{ field: 'vendor', op: 'is_null' }] });
    expect(q(door.calls[1]!.url).has('$refine')).toBe(false);
  });

  it('text is NFC on the wire, as the door compares it', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    const decomposed = 'Café';
    await activeProducts(door.client()).where((v) => v.name.eq(decomposed)).fetch();
    expect(sent(door.calls[0]!.url)).toEqual({ where: [{ field: 'name', op: 'eq', value: 'Café' }] });
  });

  it('a second .where appends (AND); a second .pick replaces; the builder is immutable', async () => {
    const door = new MockDoor().reply({ json: { rows: [] } });
    const base = activeProducts(door.client()).where((v) => v.price.ge('10'));
    const narrower = base.where((v) => v.price.lt('20')).pick('name').pick('price');
    await base.fetch();
    await narrower.fetch();
    expect(sent(door.calls[0]!.url)).toEqual({ where: [{ field: 'price', op: 'ge', value: '10' }] });
    expect(sent(door.calls[1]!.url)).toEqual({
      where: [
        { field: 'price', op: 'ge', value: '10' },
        { field: 'price', op: 'lt', value: '20' },
      ],
      pick: ['price'],
    });
  });

  it('.pages() sends the same refinement on every page and stops only on an absent next', async () => {
    const door = new MockDoor().reply(
      { json: { rows: [{ name: 'a' }], next: 'c1' } },
      { json: { rows: [], next: 'c2' } },
      { json: { rows: [{ name: 'b' }] } },
    );
    const rows = await activeProducts(door.client())
      .where((v) => v.price.gt('100'))
      .pick('name')
      .rows();
    expect(rows.map((r) => r.name)).toEqual(['a', 'b']);
    expect(door.calls).toHaveLength(3);
    for (const call of door.calls) {
      expect(sent(call.url)).toEqual({ where: [{ field: 'price', op: 'gt', value: '100' }], pick: ['name'] });
    }
    expect(door.calls[1]!.headers['X-Ordo-Page']).toBe('c1');
    expect(door.calls[2]!.headers['X-Ordo-Page']).toBe('c2');
  });

  it('a single-row read takes .where and .pick too', async () => {
    const door = new MockDoor().reply({ json: { rows: [{ n: 3 }] } });
    const row = await specCount(door.client())
      .where((v) => v.n.gt(0))
      .pick('n')
      .row();
    expect(row).toEqual({ n: 3 });
    expect(sent(door.calls[0]!.url)).toEqual({ where: [{ field: 'n', op: 'gt', value: 0 }], pick: ['n'] });
  });
});

describe('refinement: refused at the call site, as the door would', () => {
  type Loose = Record<string, { eq(x: unknown): never; gt(x: unknown): never }>;
  const refusalOf = (f: () => unknown): OrdoLocalRefusal => {
    try {
      f();
    } catch (e) {
      if (e instanceof OrdoLocalRefusal) return e;
      throw e;
    }
    throw new Error('expected a local refusal');
  };

  it('a field the op does not return is invalid_refinement, naming the field', () => {
    const door = new MockDoor();
    // Plain JavaScript, or a cast, gets past the types. The proxy hands back
    // a field for any name, so the refusal is typed and never a TypeError.
    const r = refusalOf(() => activeProducts(door.client()).where((v) => (v as unknown as Loose).margin!.eq('1')));
    expect(r.class).toBe('invalid_refinement');
    expect(r.detail.param).toBe('margin');
    expect(r.message).toMatch(/offers \[name, price, vendor, category, \$id\]/);
    expect(door.calls).toHaveLength(0);
  });

  it('a sealed field opted out of refinement is refused by name, and can still be picked', () => {
    const r = refusalOf(() => memoDigest(new MockDoor().client()).where((v) => (v as unknown as Loose).memo!.eq('x')));
    expect(r.class).toBe('invalid_refinement');
    expect(r.message).toMatch(/sealed field and is not refinable/);
    expect(() => memoDigest(new MockDoor().client()).pick('memo')).not.toThrow();
  });

  it('a sealed refinable field takes eq and in only', () => {
    // No op in the golden has a sealed refinable field, so one is made here
    // over a real spec: `memo` opted in (`sealed indexed refinable`).
    const base = SPECS.memoDigest;
    const spec = { ...base, refine: { ...base.refine, fields: ['placed', 'memo', '$id'] } };
    const call = new ReadCall(new MockDoor().client(), spec, {});
    expect(() => call.where((v) => v.memo!.eq('x'))).not.toThrow();
    expect(() => call.where((v) => v.memo!.in(['x', 'y']))).not.toThrow();
    const r = refusalOf(() => call.where((v) => v.memo!.gt('x')));
    expect(r.message).toMatch(/accepts equality only/);
  });

  it('a value of the wrong type is refused, and money takes a decimal string, never a float', () => {
    const door = new MockDoor();
    expect(refusalOf(() => specCount(door.client()).where((v) => v.n.eq('3' as never))).message).toMatch(/`int`/);
    expect(refusalOf(() => activeProducts(door.client()).where((v) => v.price.gt(40.5 as never))).message).toMatch(
      /never a float/,
    );
    // An integer is an exact decimal too, as the door accepts it.
    expect(() => activeProducts(door.client()).where((v) => v.price.gt(40 as never))).not.toThrow();
  });

  it('the shapes the door refuses: an empty pick and a null where a scalar goes', () => {
    const door = new MockDoor();
    expect(() => activeProducts(door.client()).pick(...([] as unknown as ['name']))).toThrowError(
      /at least one field/,
    );
    expect(() => activeProducts(door.client()).where((v) => v.name.eq(null as never))).toThrowError(
      /non-null scalar/,
    );
  });

  it('every basic read states the size limits, and the client checks them before the wire', () => {
    // The descriptor carries the deployment's limits on every basic read
    // (ruled 2026-10-05, refinement-default ask 6: 16 conditions, 200 values).
    const base = SPECS.activeProducts;
    expect(base.refine.limits).toEqual({ conditions: 16, inValues: 200 });
    for (const spec of specList()) {
      if (spec.refine.tier === 'basic') expect(spec.refine.limits, spec.op).toEqual({ conditions: 16, inValues: 200 });
    }
    const call = activeProducts(new MockDoor().client());
    expect(() => call.where((v) => Array.from({ length: 17 }, () => v.name.ne('x')))).toThrowError(
      /at most 16 conditions/,
    );
    expect(() => call.where((v) => Array.from({ length: 16 }, () => v.name.ne('x')))).not.toThrow();
    const values = (n: number) => Array.from({ length: n }, (_, i) => `v${i}`);
    expect(() => call.where((v) => v.name.in(values(201)))).toThrowError(/at most 200 values/);
    expect(() => call.where((v) => v.name.in(values(200)))).not.toThrow();

    // A deployment that set its own limits states them, and they are the ones checked.
    const limited = { ...base, refine: { ...base.refine, limits: { conditions: 2, inValues: 3 } } };
    const small = new ReadCall(new MockDoor().client(), limited, {});
    expect(() => small.where((v) => [v.name!.eq('a'), v.name!.ne('b'), v.name!.ne('c')])).toThrowError(
      /at most 2 conditions/,
    );
    expect(() => small.where((v) => v.name!.in(['a', 'b', 'c', 'd']))).toThrowError(/at most 3 values/);

    // A descriptor with no limits (one written before they were stated)
    // leaves them to the door.
    const open = { ...base, refine: { ...base.refine, limits: null } };
    const unlimited = new ReadCall(new MockDoor().client(), open, {});
    expect(() => unlimited.where((v) => Array.from({ length: 17 }, () => v.name!.ne('x')))).not.toThrow();
  });
});

describe('refinement: the refusals join each read’s typed union', () => {
  it('every read declares invalid_refinement and refinement_unbounded', () => {
    for (const spec of specList()) {
      if (spec.kind !== 'read') continue;
      expect(spec.refusals, spec.op).toContain('invalid_refinement');
      expect(spec.refusals, spec.op).toContain('refinement_unbounded');
    }
  });

  it('a read with a sealed output declares sealed_unsupported', () => {
    expect(SPECS.memoDigest.refusals).toContain('sealed_unsupported');
    expect(SPECS.activeProducts.refusals).not.toContain('sealed_unsupported');
  });

  it('a refusal from the door is typed and declared', async () => {
    const door = new MockDoor().reply({
      status: 400,
      json: { error: { code: 'refinement_unbounded', message: 'raises the class', param: 'name', op: 'activeProducts' } },
    });
    const err = (await activeProducts(door.client())
      .where((v) => v.name.eq('x'))
      .fetch()
      .then(
        () => null,
        (e: unknown) => e,
      )) as OrdoRefusal;
    expect(err.class).toBe('refinement_unbounded');
    expect(err.detail.param).toBe('name');
    expect(err.detail.declared).toBe(true);
  });

  it('a write has no .where', () => {
    const call = addAnonCustomer(new MockDoor().client(), { name: 'Ada' });
    expect('where' in call).toBe(false);
  });
});
