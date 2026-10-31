import { describe, expect, it } from 'vitest';
import { deriveForm, enumOptionsFor, toParams, validateForm } from '../src/form.js';
import { GOLDEN, iface } from './harness.js';
import { parseInterface } from '../src/load.js';

const i = iface();
const form = (op: string) => deriveForm(i, op);
const field = (op: string, name: string) => {
  const f = form(op).fields.find((x) => x.name === name);
  if (!f) throw new Error(`no field ${name} on ${op}`);
  return f;
};

describe('field derivation, per param type', () => {
  it('required and optional come straight off the param', () => {
    const f = form('std.customers.create');
    expect(f.fields.map((x) => [x.name, x.required])).toEqual([
      ['name', true],
      ['email', false],
      ['tier', false],
    ]);
  });

  it('scalars keep their declared type — money is money, not a number', () => {
    expect(field('std.products.create', 'price').type).toBe('money');
    expect(field('std.products.create', 'price').kind).toBe('scalar');
    expect(field('std.products.create', 'active').type).toBe('bool');
    expect(field('recentOrders', 'since').type).toBe('date');
    expect(field('renameTaskIf', 'title').type).toBe('text');
  });

  it('an id param becomes a typed reference carrying its entity and prefix', () => {
    const vendor = field('std.products.create', 'vendor');
    expect(vendor.kind).toBe('reference');
    expect(vendor.candidates?.entity).toBe('vendors');
    expect(vendor.candidates?.key).toBe('vend');
    expect(vendor.candidates?.listOps).toContain('std.vendors.list');
    expect(vendor.candidates?.detailOps).toContain('std.vendors.get');
    expect(vendor.candidates?.blind).toBe(false);
  });

  it('reads the listing index rather than ranking op names', () => {
    // Under `/1` this asserted that `std.E.list` sorted first, because the
    // package ranked candidates by NAME. `/2` states the index, so the order
    // is the descriptor's and the assertion is membership plus the property
    // that made the name rank unnecessary: every candidate really returns
    // rows of the entity.
    const cat = field('std.products.create', 'category').candidates!;
    expect(cat.listOps).toContain('std.categories.list');
    for (const name of cat.listOps) {
      expect(i.requireOp(name).result.row_identity?.entity).toBe('categories');
    }
    // An authored listing qualifies by exactly the same rule as a derived
    // one — which is the whole point of keying on row identity.
    expect(field('std.tasks.update', 'project').candidates?.listOps).toContain('myProjects');
  });

  it('separates "can stock a cold picker" from "lists the entity"', () => {
    // `cheapActive($cap)` lists products and needs an argument. It belongs in
    // `listOps` — a caller holding a cap can use it — and not in the subset a
    // picker with nothing in hand may call. `/1` conflated the two, because
    // its reconstruction had to filter on required params to find the index
    // at all.
    const vendor = field('std.products.create', 'vendor').candidates!;
    expect(vendor.blind).toBe(false);
    const cat = field('std.products.create', 'category').candidates!;
    expect(cat.listOps).toContain('std.categories.list');
    const products = i.idTypeForEntity('products')!;
    expect(products.list.map((l) => l.op)).toContain('cheapActive');
    expect(i.listOpsForBlindPicker('products').map((o) => o.op)).not.toContain('cheapActive');
  });

  it('resolves a picker label through `get`, never through `detail`', () => {
    // `detail` holds every read that ACCEPTS the id; `get` holds the ones
    // that return that row. Only the second can label a chosen value.
    for (const e of i.doc.id_types) {
      for (const g of e.get) {
        expect(i.requireOp(g.op).result.row_identity?.entity, g.op).toBe(e.entity);
        expect(typeof g.param).toBe('string');
      }
    }
    expect(field('std.products.create', 'vendor').candidates?.detailOps).toEqual(['std.vendors.get']);
  });

  it('announces a blind reference rather than pretending a picker exists', () => {
    // `members` is a platform entity and this catalog grants no unparameterised
    // read anchored on it, so nothing can stock the picker. The consumer is
    // told up front instead of getting an empty dropdown.
    const assignee = field('std.tasks.update', 'assignee');
    expect(assignee.candidates?.entity).toBe('members');
    expect(assignee.candidates?.blind).toBe(true);
    expect(assignee.candidates?.listOps).toEqual([]);
  });

  it('an enum-valued param becomes an option list', () => {
    const level = field('std.boardsAccess.create', 'level');
    expect(level.kind).toBe('enum');
    expect(level.options).toEqual(['read', 'write', 'admin']);
  });

  it('leaves an ordinary text param alone', () => {
    expect(field('std.customers.create', 'tier').kind).toBe('scalar');
    expect(field('std.customers.create', 'tier').options).toBeNull();
  });

  it('reads enum options off the param, linked to the field it fills', () => {
    // Under `/1` this joined `statements[].entity` to a same-named field and
    // refused on ambiguity. `/2` carries `field_ref`, so the link is stated
    // and the option list rides with it.
    const level = i.requireOp('std.boardsAccess.update').params.find((p) => p.name === 'level')!;
    expect(level.field_ref).toEqual({ entity: 'boardsAccess', field: 'level' });
    expect(enumOptionsFor(level)).toEqual(['read', 'write', 'admin']);

    // The link exists on ordinary params too — it is a field reference, not
    // an enum channel — and carries no options where the field declares none.
    const name = i.requireOp('std.vendors.create').params.find((p) => p.name === 'name')!;
    expect(name.field_ref).toEqual({ entity: 'vendors', field: 'name' });
    expect(enumOptionsFor(name)).toBeNull();

    // And an authored op reaches the same ladder, through the same fact — a
    // name join would have had to guess which of two written entities.
    const granted = i.requireOp('grantBoardsAccess').params.find((p) => p.name === 'level')!;
    expect(enumOptionsFor(granted)).toEqual(['read', 'write', 'admin']);
  });

  it('links a read parameter to the field its source condition compares it to', () => {
    // `statements[]` is empty on a read, so `/1`'s join could never have
    // produced this: `recentOrders($since)` filters `orders(placed >= $since)`
    // and the parameter is an `orders.placed` value.
    const since = i.requireOp('recentOrders').params.find((p) => p.name === 'since')!;
    expect(since.field_ref).toEqual({ entity: 'orders', field: 'placed' });
  });

  it('never carries an empty option list, which would read as a dead dropdown', () => {
    for (const o of i.ops) {
      for (const p of o.params) {
        if (p.enum_values !== null) expect(p.enum_values.length).toBeGreaterThan(0);
        // A `field_ref` is whole or absent; a half-populated one would send a
        // consumer looking up a field that is not there.
        if (p.field_ref !== null) {
          const e = i.entity(p.field_ref.entity);
          expect(e, `${o.op} $${p.name}`).toBeDefined();
          expect(e!.fields.map((f) => f.name)).toContain(p.field_ref.field);
        }
      }
    }
  });
});

describe('the rev field is the runtime’s, not the form’s', () => {
  it('is derived, hidden, and excluded from what a consumer draws', () => {
    const f = form('std.tasks.update');
    expect(f.revField?.name).toBe('rev');
    expect(f.revField?.kind).toBe('rev');
    expect(f.revField?.hidden).toBe(true);
    expect(f.visible.map((x) => x.name)).not.toContain('rev');
    expect(f.fields.map((x) => x.name)).toContain('rev');
  });

  it('every CAS op in the golden gets exactly one rev field', () => {
    const withRev = i.ops.filter((o) => o.params.some((p) => p.name === 'rev'));
    expect(withRev.length).toBeGreaterThan(10);
    for (const o of withRev) {
      const m = deriveForm(i, o.op);
      expect(m.revField, o.op).not.toBeNull();
      expect(m.fields.filter((x) => x.kind === 'rev').length, o.op).toBe(1);
    }
  });

  it('an op with no rev param reports no CAS field', () => {
    expect(form('std.tasks.create').revField).toBeNull();
    expect(form('std.tasks.list').revField).toBeNull();
  });
});

describe('every op in the golden derives a form without throwing', () => {
  it('all 165', () => {
    for (const o of i.ops) {
      const m = deriveForm(i, o.op);
      expect(m.fields.length, o.op).toBe(o.params.length);
      // Nothing but rev is hidden — a form that quietly drops a required
      // param would produce a write the door refuses with missing_param.
      for (const f of m.fields) {
        if (f.hidden) expect(f.kind, `${o.op}.${f.name}`).toBe('rev');
      }
    }
  });

  it('marks deletes destructive, and says the derivation out loud', () => {
    expect(form('std.tasks.delete').danger).toBe(true);
    expect(form('std.tasks.update').danger).toBe(false);
  });
});

describe('validation mirrors the door’s own classes', () => {
  const model = form('std.products.create');

  it('missing required -> missing_param, and explicit null counts as absent', () => {
    const issues = validateForm(model, { vendor: null }, i);
    expect(issues.map((x) => [x.param, x.class])).toEqual(
      expect.arrayContaining([
        ['vendor', 'missing_param'],
        ['name', 'missing_param'],
        ['price', 'missing_param'],
      ]),
    );
    // `active` is optional: absence is fine.
    expect(issues.find((x) => x.param === 'active')).toBeUndefined();
  });

  it('money must be an exact decimal string, never a float', () => {
    const asFloat = validateForm(model, { price: 12.5 } as never, i).find((x) => x.param === 'price');
    expect(asFloat?.class).toBe('invalid_param');
    expect(asFloat?.message).toContain('never a float');
    expect(validateForm(model, { price: '-12.50' }, i).find((x) => x.param === 'price')).toBeUndefined();
  });

  it('bool, date and int are checked in their wire shapes', () => {
    expect(validateForm(model, { active: 'true' }, i).find((x) => x.param === 'active')?.class).toBe('invalid_param');
    expect(validateForm(model, { active: true }, i).find((x) => x.param === 'active')).toBeUndefined();

    const dates = form('recentOrders');
    expect(validateForm(dates, { since: '01/02/2026' }, i)[0]?.class).toBe('invalid_param');
    expect(validateForm(dates, { since: '2026-02-30' }, i)[0]?.class).toBe('invalid_param');
    expect(validateForm(dates, { since: '2026-02-28' }, i)).toEqual([]);

    const ints = form('std.orderItems.create');
    expect(validateForm(ints, { qty: 1.5 }, i).find((x) => x.param === 'qty')?.class).toBe('invalid_param');
  });

  it('a text(n) bound counts characters, as the door does, not UTF-16 units', () => {
    // No op in the golden carries a text bound yet, so one is put on `name`.
    const bounded = (f: (typeof model.fields)[number]) => (f.name === 'name' ? { ...f, max: '3' } : f);
    const m = { ...model, fields: model.fields.map(bounded), visible: model.visible.map(bounded) };
    const name = (v: string) => validateForm(m, { name: v }, i).find((x) => x.param === 'name');
    // Three emoji are six UTF-16 units and three characters: accepted.
    expect(name('😀😀😀')).toBeUndefined();
    const over = name('abcd');
    expect(over?.class).toBe('bound_violation');
    expect(over?.message).toContain('got 4 characters');
  });

  it('an id carrying another entity’s prefix is refused before the wire', () => {
    const issue = validateForm(model, { vendor: 'prod_01J8ZKQ4X7' }, i).find((x) => x.param === 'vendor');
    expect(issue?.class).toBe('invalid_param');
    expect(issue?.message).toContain('prefix "vend_"');
    // The message names what the caller actually passed, because "invalid id"
    // sends a developer looking in the wrong place.
    expect(issue?.message).toContain('that is a products id');
    expect(validateForm(model, { vendor: 'vend_01J8ZKQ4X7' }, i).find((x) => x.param === 'vendor')).toBeUndefined();
  });

  it('a non-id string is refused as a shape, not as a prefix mismatch', () => {
    const issue = validateForm(model, { vendor: 'just-a-string' }, i).find((x) => x.param === 'vendor');
    expect(issue?.message).toContain('typed id');
  });

  it('an enum value outside the option list is refused', () => {
    const acl = form('std.boardsAccess.create');
    expect(validateForm(acl, { level: 'owner' }, i).find((x) => x.param === 'level')?.class).toBe('invalid_param');
    expect(validateForm(acl, { level: 'admin' }, i).find((x) => x.param === 'level')).toBeUndefined();
  });

  it('a fully bound form validates clean and serialises to params', () => {
    const values = {
      vendor: 'vend_01J8ZKQ4X7',
      category: 'cat_01J8ZKQ4X7',
      name: 'Anvil',
      price: '19.99',
      active: true,
    };
    expect(validateForm(model, values, i)).toEqual([]);
    expect(toParams(model, values)).toEqual(values);
  });

  it('drops absent values from the wire payload rather than sending nulls', () => {
    expect(toParams(model, { name: 'Anvil', active: undefined })).toEqual({ name: 'Anvil' });
  });
});

describe('a json parameter (28b)', () => {
  // No op in the golden takes a `json` parameter, so one is made here: the
  // golden with `std.products.create`'s `name` retyped to `json`.
  const doc = JSON.parse(JSON.stringify(GOLDEN)) as {
    ops: { op: string; params: { name: string; type: string }[] }[];
  };
  const create = doc.ops.find((o) => o.op === 'std.products.create')!;
  create.params.find((p) => p.name === 'name')!.type = 'json';
  const j = parseInterface(doc as unknown as Record<string, unknown>);
  const model = deriveForm(j, 'std.products.create');

  it('derives the json kind, so a renderer offers a document editor', () => {
    const f = model.fields.find((x) => x.name === 'name')!;
    expect(f.kind).toBe('json');
    expect(f.type).toBe('json');
    expect(f.hidden).toBe(false);
  });

  it('takes any JSON value and refuses what the wire would change', () => {
    const issue = (v: unknown) => validateForm(model, { name: v as never }, j).find((x) => x.param === 'name');
    for (const v of [{ a: [1, 'two', null] }, [1, 2], 'text', 3.5, false]) {
      expect(issue(v)).toBeUndefined();
    }
    expect(issue({ at: new Date() })?.class).toBe('invalid_param');
    expect(issue([Number.POSITIVE_INFINITY])?.class).toBe('invalid_param');
  });
});
