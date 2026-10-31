/**
 * The builder's types: what compiles and what does not.
 *
 * `tsc --noEmit` covers this file, so each `@ts-expect-error` is an
 * assertion. If the line it marks ever compiles, the directive itself is the
 * error and the typecheck fails. The run-time half only builds calls; nothing
 * reaches a door.
 */

import { describe, expect, expectTypeOf, it } from 'vitest';

import { MockDoor } from './harness.js';
import { activeProducts, addAnonCustomer, memoDigest, specCount, std$categories$create, std$categories$put } from '../src/generated/index.js';
import type { Page, SealedField } from '../src/index.js';
import type { ActiveProductsRow, MemoDigestRow } from '../src/generated/index.js';
import type * as Ids from '../src/generated/ids.js';

const client = new MockDoor().client();

describe('the field proxy is typed from the output manifest', () => {
  it('each field takes values of its own row type', () => {
    activeProducts(client).where((v) => {
      expectTypeOf(v.price.gt).parameter(0).toEqualTypeOf<string>();
      expectTypeOf(v.vendor.eq).parameter(0).toEqualTypeOf<Ids.VendorsId>();
      return v.name.eq('saw');
    });
    specCount(client).where((v) => {
      expectTypeOf(v.n.lt).parameter(0).toEqualTypeOf<number>();
      return v.n.ge(1);
    });
  });

  it('a field that is not an output does not compile', () => {
    expectTypeOf(() =>
      activeProducts(client).where((v) =>
        // @ts-expect-error `margin` is not an output of activeProducts.
        v.margin.eq('1'),
      ),
    ).toBeFunction();
  });

  it('a sealed field that did not opt in is not offered', () => {
    expectTypeOf(() =>
      memoDigest(client).where((v) =>
        // @ts-expect-error `memo` is sealed and not refinable.
        v.memo.eq('x'),
      ),
    ).toBeFunction();
  });

  it('a sealed refinable field offers eq and in, and no ordering', () => {
    type Memo = SealedField<'memo', string>;
    expectTypeOf<Memo>().toHaveProperty('eq');
    expectTypeOf<Memo>().toHaveProperty('in');
    expectTypeOf<Memo>().not.toHaveProperty('gt');
    expectTypeOf<Memo>().not.toHaveProperty('isNull');
  });

  it('a value of the wrong type does not compile', () => {
    expectTypeOf(() => {
      // @ts-expect-error money is an exact decimal string, never a number.
      activeProducts(client).where((v) => v.price.gt(4000));
      // @ts-expect-error `n` is an int.
      specCount(client).where((v) => v.n.eq('3'));
      // @ts-expect-error `is_null` takes no value.
      activeProducts(client).where((v) => v.vendor.isNull('x'));
    }).toBeFunction();
  });

  it('an operator outside the closed set does not exist', () => {
    expectTypeOf(() =>
      activeProducts(client).where((v) =>
        // @ts-expect-error `like` is not a Tier-0 operator.
        v.name.like('s%'),
      ),
    ).toBeFunction();
  });
});

describe('.pick narrows the row type', () => {
  it('to Pick<Row, K>', () => {
    const call = activeProducts(client).pick('name', 'price');
    expectTypeOf(call.fetch).returns.resolves.toEqualTypeOf<Page<Pick<ActiveProductsRow, 'name' | 'price'>>>();
    expectTypeOf(call.rows).returns.resolves.toEqualTypeOf<Pick<ActiveProductsRow, 'name' | 'price'>[]>();
  });

  it('a later .pick replaces the earlier one and chooses from the whole row', () => {
    const call = activeProducts(client).pick('name').pick('vendor');
    expectTypeOf(call.fetch).returns.resolves.toEqualTypeOf<Page<Pick<ActiveProductsRow, 'vendor'>>>();
  });

  it('.where after .pick keeps the narrowed row', () => {
    const call = activeProducts(client)
      .pick('name')
      .where((v) => v.price.gt('1'));
    expectTypeOf(call.fetch).returns.resolves.toEqualTypeOf<Page<Pick<ActiveProductsRow, 'name'>>>();
  });

  it('a sealed output can be picked, though it cannot be tested', () => {
    const call = memoDigest(client).pick('memo');
    expectTypeOf(call.fetch).returns.resolves.toEqualTypeOf<Page<Pick<MemoDigestRow, 'memo'>>>();
  });

  it('an empty pick, or a field that is not an output, does not compile', () => {
    expectTypeOf(() => {
      // @ts-expect-error a pick names at least one field.
      activeProducts(client).pick();
      // @ts-expect-error `margin` is not an output.
      activeProducts(client).pick('margin');
    }).toBeFunction();
  });

  it('a single-row read narrows too', () => {
    const call = specCount(client).pick('n');
    expectTypeOf(call.row).returns.resolves.toEqualTypeOf<{ readonly n: number } | null>();
  });
});

describe('a write has no builder', () => {
  it('neither .where nor .pick exists', () => {
    const call = addAnonCustomer(client, { name: 'Ada' });
    expectTypeOf(call).not.toHaveProperty('where');
    expectTypeOf(call).not.toHaveProperty('pick');
  });
});

describe('clearing an optional field (2026-10-30)', () => {
  const parent = 'cat_1' as Ids.CategoriesId;

  it('a converge must say keep or clear: the current value, or null', () => {
    std$categories$put(client, { name: 'Saws', parent });
    std$categories$put(client, { name: 'Saws', parent: null });
    // @ts-expect-error leaving `parent` out would write its default and clear it.
    std$categories$put(client, { name: 'Saws' });
  });

  it('a create takes null or nothing for the same field', () => {
    std$categories$create(client, { name: 'Saws' });
    std$categories$create(client, { name: 'Saws', parent: null });
  });

  it('null travels as null, which the door reads as the default', async () => {
    const door = new MockDoor();
    await std$categories$put(door.client(), { name: 'Saws', parent: null }).commit();
    expect(door.calls[0]?.body).toEqual({ name: 'Saws', parent: null });
  });
});
