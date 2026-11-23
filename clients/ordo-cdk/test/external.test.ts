import { describe, expect, it } from 'vitest';

import { OPEN, entity, external, g, key, ref, synth, t } from '../src/index.js';

const accounts = () =>
  external(
    'accounts',
    {
      id: key.uuid('ref'),
      label: t.text().column('label'),
    },
    { schema: 'crm', table: 'accounts', key: 'acct', shared: true },
  );

const customers = (target = accounts()) =>
  external(
    'customers',
    {
      id: key.int('customer_id'),
      name: t.text().public().column('full_name'),
      email: t.text().optional().column('email'),
      account: ref(target).optional().column('account_id'),
    },
    { schema: 'crm', table: 'customers', shared: true },
  )
    .public()
    .accessRead(g.OPEN);

describe('external', () => {
  it('prints the accounts example byte for byte', () => {
    expect(synth(accounts())).toBe(
      [
        'external accounts (key: acct) from crm.accounts shared {',
        '  id: uuid <- "ref"',
        '  label: text <- "label"',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('prints the customers example byte for byte', () => {
    expect(synth(accounts(), customers())).toBe(
      [
        'external accounts (key: acct) from crm.accounts shared {',
        '  id: uuid <- "ref"',
        '  label: text <- "label"',
        '}',
        '',
        'external customers from crm.customers shared {',
        '  id <- "customer_id"',
        '  name: text public <- "full_name"',
        '  email: text? <- "email"',
        '  account -> accounts? <- "account_id"',
        '  public',
        '  access read open',
        '}',
        '',
      ].join('\n'),
    );
  });

  it('puts the key line first whatever the record order', () => {
    const e = external(
      'tags',
      { label: t.text().column('l'), id: key.text('code') },
      { schema: 'crm', table: 'tags', shared: true },
    );
    expect(synth(e)).toBe(
      'external tags from crm.tags shared {\n  id: text <- "code"\n  label: text <- "l"\n}\n',
    );
  });

  it('keeps column case and never splits schema from table on a dot', () => {
    const e = external(
      'rows',
      { id: key.int('ID'), v: t.int().column('Mixed_Case') },
      { schema: 'my schema', table: 'a.b', shared: true },
    );
    expect(synth(e)).toBe(
      'external rows from "my schema"."a.b" shared {\n  id <- "ID"\n  v: int <- "Mixed_Case"\n}\n',
    );
  });

  it('uses OPEN from the barrel', () => {
    expect(OPEN).toBeDefined();
  });
});

describe('external, refused at the type level', () => {
  it('a forgotten column', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      // @ts-expect-error a field with no `.column()` is not a mapped field
      external('a', { id: key.int('i'), x: t.text() }, { schema: 's', table: 't', shared: true });
      };
    void check;
  });

  it('shared: false, and a missing shared', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      // @ts-expect-error `shared` is the literal `true`
      external('a', { id: key.int('i') }, { schema: 's', table: 't', shared: false });
      // @ts-expect-error `shared` is required
      external('a', { id: key.int('i') }, { schema: 's', table: 't' });
      };
    void check;
  });

  it('public on the key, and optional on the key', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      // @ts-expect-error the key is never public
      key.int('i').public();
      // @ts-expect-error the key takes no optional
      key.int('i').optional();
      };
    void check;
  });

  it('a missing id', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      // @ts-expect-error an external names its key
      external('a', { x: t.text().column('x') }, { schema: 's', table: 't', shared: true });
      };
    void check;
  });

  it('entity() takes no mapped field', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      // @ts-expect-error a mapped spec is for `external()` only
      entity('a', { x: t.text().column('x') });
      };
    void check;
  });

  it('an external has no publicAll, acl, unique, index or check', () => {
  // Never called: the refusal is the compiler's, checked by `tsc`.
    const check = (): void => {
      const e = accounts();
      // @ts-expect-error an external owns nothing for the star to reach
      e.publicAll();
      // @ts-expect-error an external takes no acl
      e.acl('read');
      // @ts-expect-error an external takes no unique
      e.unique('a', 'b');
      // @ts-expect-error an external takes no index
      e.index('a');
      // @ts-expect-error an external is read-only
      e.accessWrite(g.OPEN);
      };
    void check;
  });
});

describe('external, refused at run time', () => {
  it.each([
    ['sealed', () => t.text().sealed().column('c'), /external_sealed/],
    ['indexed', () => t.text().indexed().column('c'), /external_sealed/],
    ['unique', () => t.text().unique().column('c'), /external_unsupported/],
    ['owned', () => ref('x').owned().column('c'), /external_owned/],
    ['tree', () => ref('x').tree().column('c'), /external_unsupported/],
    ['normalize', () => t.text().normalize('lower').column('c'), /external_unsupported/],
    ['blob', () => t.blob().column('c'), /external_unsupported/],
  ])('.column() refuses %s', (_w, build, cls) => {
    expect(build).toThrow(cls);
  });

  it('refuses a field without a column', () => {
    const bad = { id: key.int('i'), x: t.text() } as never;
    expect(() => external('a', bad, { schema: 's', table: 't', shared: true })).toThrow(/column/);
  });

  it('refuses shared: false', () => {
    expect(() =>
      external('a', { id: key.int('i') }, { schema: 's', table: 't', shared: false } as never),
    ).toThrow(/external_not_shared/);
  });

  it('synth refuses a ref to a native entity', () => {
    const native = entity('people', { name: t.text() });
    const e = external(
      'a',
      { id: key.int('i'), p: ref(native).column('p') },
      { schema: 's', table: 't', shared: true },
    );
    expect(() => synth(native, e)).toThrow(/external_ref_to_native/);
  });

  it('synth refuses a ref to an unknown name', () => {
    const e = external(
      'a',
      { id: key.int('i'), p: ref('nowhere').column('p') },
      { schema: 's', table: 't', shared: true },
    );
    expect(() => synth(e)).toThrow(/external_ref_to_native/);
  });
});
