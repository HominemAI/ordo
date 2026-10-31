import { describe, expect, it } from 'vitest';
import { actionMenu, discover } from '../src/actions.js';
import { iface } from './harness.js';

const i = iface();
const sample = (key: string) => `${key}_01J8ZKQ4X7GHT2`;

describe('ui-gen §2 action discovery, for every id type in the catalog', () => {
  it('resolves each id_types entry from an id alone', () => {
    expect(i.doc.id_types.length).toBeGreaterThan(10);
    for (const t of i.doc.id_types) {
      const row = discover(i, sample(t.key));
      expect(row.known, t.key).toBe(true);
      if (!row.known) continue;

      expect(row.entity).toBe(t.entity);
      expect(row.key).toBe(t.key);
      // The lookup neither adds nor drops: it is the descriptor's index,
      // resolved to op descriptors.
      expect(row.actions.map((a) => a.op)).toEqual(t.actions.map((a) => a.op));
      expect(row.detail.map((a) => a.op)).toEqual(t.detail.map((a) => a.op));

      for (const a of [...row.actions, ...row.detail]) {
        // Every discovered op is one this catalog actually describes — the
        // index is derived after the catalog filter, so a button can never
        // point at an op the token may not call.
        expect(i.op(a.op), a.op).toBeDefined();
        // And the param it binds into is a real id param addressing this entity.
        const p = a.descriptor.params.find((x) => x.name === a.param);
        expect(p, `${a.op}.${a.param}`).toBeDefined();
        expect(p?.type).toBe('id');
        expect(p?.id_entity).toBe(t.entity);
      }
    }
  });

  it('every discovered action is a write or a delete, every detail op is a read', () => {
    for (const t of i.doc.id_types) {
      const row = discover(i, sample(t.key));
      if (!row.known) continue;
      for (const a of row.actions) expect(a.descriptor.kind, a.op).not.toBe('read');
      for (const d of row.detail) expect(d.descriptor.kind, d.op).toBe('read');
    }
  });

  it('binds the row id into the op’s own param name, whatever it is called', () => {
    // The rule is about the *entity* an id param addresses, never about the
    // param being called `id`: `renameTaskIf` takes `task`, `moveBA` takes `ba`.
    const task = discover(i, sample('task'));
    expect(task.known).toBe(true);
    if (!task.known) return;
    const rename = task.actions.find((a) => a.op === 'renameTaskIf');
    expect(rename?.bind(task.id)).toEqual({ task: task.id });
    const update = task.actions.find((a) => a.op === 'std.tasks.update');
    expect(update?.bind(task.id)).toEqual({ id: task.id });

    const ba = discover(i, sample('boar-acce'));
    if (!ba.known) throw new Error('expected boardsAccess');
    expect(ba.actions.find((a) => a.op === 'moveBA')?.bind(ba.id)).toEqual({ ba: ba.id });
  });

  it('a multi-word prefix resolves to its own entity, not a neighbour', () => {
    // `boar-acce` vs `board`: an implementation splitting on `-`, or matching
    // by prefix rather than by whole key, hands boardsAccess rows the boards
    // action menu.
    const board = discover(i, sample('board'));
    const access = discover(i, sample('boar-acce'));
    expect(board.known && board.entity).toBe('boards');
    expect(access.known && access.entity).toBe('boardsAccess');
    if (board.known && access.known) {
      expect(board.actions.map((a) => a.op)).not.toEqual(access.actions.map((a) => a.op));
    }
  });
});

describe('the three outcomes are three, deliberately', () => {
  it('unparseable', () => {
    expect(discover(i, 'not-an-id')).toMatchObject({ known: false, reason: 'unparseable', key: null });
    expect(discover(i, '')).toMatchObject({ known: false, reason: 'unparseable' });
  });

  it('unknown prefix — which under catalog scoping may mean “not yours”', () => {
    expect(discover(i, 'zzz_01J8ZK')).toMatchObject({ known: false, reason: 'unknown-prefix', key: 'zzz' });
  });

  it('known but inert: a described entity no op accepts the id of', () => {
    // `groupMembers` (key `gm`) is in `entities[]` and absent from `id_types`,
    // because no visible op takes a `gm` id. That is a real row with no
    // affordances, and a UI must not render it as an unknown id.
    expect(i.entityForKey('gm')?.entity).toBe('groupMembers');
    expect(i.idTypeForKey('gm')).toBeUndefined();
    const row = discover(i, sample('gm'));
    expect(row).toMatchObject({ known: true, entity: 'groupMembers', inert: true });
    if (row.known) {
      expect(row.actions).toEqual([]);
      expect(row.detail).toEqual([]);
    }
  });
});

describe('the action menu separates destructive actions by derivation', () => {
  it('deletes are destructive; nothing else is claimed to be', () => {
    const board = discover(i, sample('board'));
    const menu = actionMenu(board);
    expect(menu.destructive.length).toBeGreaterThan(0);
    for (const a of menu.destructive) expect(a.kind).toBe('delete');
    for (const a of menu.safe) expect(a.kind).not.toBe('delete');
    expect(menu.safe.length + menu.destructive.length).toBe(board.known ? board.actions.length : 0);
  });

  it('an unknown id gets an empty menu, not a throw', () => {
    expect(actionMenu(discover(i, 'zzz_1'))).toEqual({ safe: [], destructive: [] });
  });
});
