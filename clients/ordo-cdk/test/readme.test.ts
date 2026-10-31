/**
 * The README's example compiles and produces the `.ordo` it prints.
 *
 * `docs_parse.rs` pushes every fenced block in the Ordo docs through the real
 * parser, on the argument that "I copied it from a real file" is a claim a
 * test should check rather than a promise a reader has to take. A package
 * README's opening example is the highest-traffic code in the repository and
 * had no such check; this is it.
 *
 * Kept as source rather than extracted from the Markdown: the point is that
 * it *typechecks*, and a string pulled out of a fence at run time does not.
 * The pairing is checked by eye once and by `synth` for ever after — if the
 * builder changes shape, this file stops compiling, which is exactly when the
 * README is wrong.
 */

import { describe, expect, it } from 'vitest';

import { domain, entity, op, src, synth } from '../src/core/build.js';
import { agg, f, lit, t } from '../src/generated/lang.js';

describe('the README opening example', () => {
  const vendors = entity('vendors', {
    name: t.text(),
    region: t.text(),
    active: t.bool().default(lit(true)),
  });

  const activeProducts = op('activeProducts', {}, (b) =>
    b.source(src('products').args(f.active.eq(lit(true))).pick('name', 'price')),
  ).unbounded();

  it('synthesizes exactly what the README prints', () => {
    expect(synth(domain('market', '1.1.0'), vendors, activeProducts)).toBe(
      [
        'domain market @ 1.1.0',
        '',
        'entity vendors {',
        '  name: text',
        '  region: text',
        '  active: bool = true',
        '}',
        '',
        'op activeProducts unbounded =',
        '  products(active == true)[name, price]',
        '',
      ].join('\n'),
    );
  });

  it('the aggregate idiom the sketch pane shows also works', () => {
    // `group` keys the aggregates by their OUTPUT name, which is the shape
    // `group customer { spent: sum total }` has — one output name in one
    // place, rather than one in the object key and another in an argument.
    const vendorSpend = op('vendorSpend', { from: t.date() }, (b, p) =>
      b
        .source(src('invoices').args(f.issued.ge(p.from)).pick('vendor', 'total'))
        .group([f.vendor], { n: agg.count(), billed: agg.sum(f.total) })
        .where(f.billed.gt(lit(0))),
    );
    expect(synth(vendorSpend)).toBe(
      [
        'op vendorSpend($from: date) =',
        '  invoices(issued >= $from)[vendor, total]',
        '    group vendor { n: count, billed: sum total }',
        '    | billed > 0',
        '',
      ].join('\n'),
    );
  });
});
