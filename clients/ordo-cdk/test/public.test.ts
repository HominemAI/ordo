import { describe, expect, it } from 'vitest';

import { entity, f, lit, op, src, synth, t } from '../src/index.js';

describe('public', () => {
  it('prints a field mark after sealed indexed', () => {
    const e = entity('plans', {
      name: t.text().unique().sealed().indexed().public(),
      price: t.money().public(),
      margin: t.money(),
    });
    const out = synth(e);
    expect(out).toContain('  name: text unique sealed indexed public');
    expect(out).toContain('  price: money public');
    expect(out).toContain('  margin: money\n');
  });

  it('prints the entity line public and public*', () => {
    expect(synth(entity('a', { x: t.text() }).public())).toContain('\n  public\n}');
    expect(synth(entity('b', { x: t.text() }).publicAll())).toContain('\n  public*\n}');
  });

  it('puts the entity line after acl', () => {
    const out = synth(entity('a', { x: t.text() }).acl('read', 'write').public());
    expect(out.indexOf('acl (read, write)')).toBeLessThan(out.indexOf('\n  public\n'));
  });

  it('prints the op flag after nopause', () => {
    const plans = entity('plans', { name: t.text().public(), price: t.money().public() });
    const o = op('pricing', {}, (b) =>
      b.source(src(plans).where(f.price.gt(lit(0))).pick('name', 'price')),
    )
      .nopause()
      .public();
    expect(synth(o)).toMatch(/^op pricing nopause public =/);
  });

  it('refuses public on a parameter', () => {
    expect(() => op('x', { a: t.text().public() }, (b) => b.source(src('plans')))).toThrow(
      /public/,
    );
  });
});
