import { describe, expect, it } from 'vitest';

import { entity, external, key, op, synth, t } from '../src/index.js';

describe('prev', () => {
  it('prints a field rename after normalize', () => {
    const e = entity('customers', {
      fullName: t.text().prev('name'),
      email: t.text().normalize('lower').prev('mail'),
    });
    const out = synth(e);
    expect(out).toContain('  fullName: text prev(name)\n');
    expect(out).toContain('  email: text normalize(lower) prev(mail)\n');
  });

  it('prints a merge and a move', () => {
    const e = entity('customers', {
      fullName: t.text().prev('first', 'last'),
      note: t.text().optional().prev('orders.shippingNote'),
    });
    const out = synth(e);
    expect(out).toContain('  fullName: text prev(first, last)\n');
    expect(out).toContain('  note: text? prev(orders.shippingNote)\n');
  });

  it('prints the entity prev with and without key', () => {
    expect(synth(entity('customers', { a: t.text() }, { key: 'cust', prev: 'clients' }))).toContain(
      'entity customers (key: cust, prev: clients) {',
    );
    expect(synth(entity('customers', { a: t.text() }, { prev: 'clients' }))).toContain(
      'entity customers (prev: clients) {',
    );
    expect(synth(entity('customers', { a: t.text() }))).toContain('entity customers {');
  });

  it('refuses empty and malformed names', () => {
    expect(() => t.text().prev()).toThrow(/at least one/);
    expect(() => t.text().prev('a.b.c')).toThrow(/field.*entity\.field/);
    expect(() => t.text().prev('')).toThrow();
  });

  it('refuses prev on a parameter', () => {
    expect(() => op('find', { n: t.text().prev('x') }, () => [])).toThrow(/prev.*field modifier/);
  });

  it('refuses prev on an external field and an external entity', () => {
    expect(() => t.text().prev('x').column('c')).toThrow(/external_unsupported.*prev/);
    expect(() =>
      external(
        'c',
        { id: key.int('id') },
        { schema: 'crm', table: 'c', shared: true, prev: 'd' } as never,
      ),
    ).toThrow(/external_unsupported/);
  });
});
