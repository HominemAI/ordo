/**
 * The conflict state's gap the notes page found: `current: null` meant both
 * "not fetched yet" and "deleted elsewhere". `state.currentIs` now says which.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react-test-renderer';
import { useCommit } from '../src/hooks/useCommit.js';
import { MockDoor, client, refusal } from './harness.js';
import { mount, until, type Mounted } from './live-door.js';

const casMiss = { status: 409, body: refusal('cas_miss', 'rev mismatch', { param: 'rev' }) };
const values = { id: 'task_01J8ZK', project: 'proj_01J8ZK', assignee: 'mem_01J8ZK', title: 'Ship it', status: 'open' };

let mounted: Mounted<unknown> | null = null;
afterEach(() => {
  mounted?.unmount();
  mounted = null;
});

async function conflicted(door: MockDoor, op = 'std.tasks.update', input = values, options = {}) {
  const m = await mount(() => useCommit(op, options), { client: client(door) });
  mounted = m;
  await act(async () => {
    await m.result.current.commit(input, 7);
  });
  return m;
}

const stateOf = (m: Mounted<ReturnType<typeof useCommit>>) => {
  const s = m.result.current.state;
  if (s.status !== 'conflict') throw new Error(`expected a conflict, got ${s.status}`);
  return s;
};

describe('a conflict says what it knows of the current row', () => {
  it('is not_fetched until the row is read back, with current null', async () => {
    const m = await conflicted(new MockDoor().program(casMiss));
    expect(stateOf(m)).toMatchObject({ status: 'conflict', current: null, currentIs: 'not_fetched' });
  });

  it('is found with the row once resolve reads it back', async () => {
    const door = new MockDoor().program(casMiss, { body: { rows: [{ id: values.id, rev: 9, title: 'Renamed' }] } });
    const m = await conflicted(door);
    await act(async () => {
      await m.result.current.resolve();
    });
    expect(stateOf(m)).toMatchObject({
      current: { id: values.id, rev: 9, title: 'Renamed' },
      currentIs: 'found',
    });
  });

  it('is deleted when the read finds no row, which is not the same as not fetched', async () => {
    const door = new MockDoor().program(casMiss, { body: { rows: [] } });
    const m = await conflicted(door);
    await act(async () => {
      await m.result.current.resolve();
    });
    expect(stateOf(m)).toMatchObject({ current: null, currentIs: 'deleted' });
  });

  it('is deleted when the door answers not_found for the row', async () => {
    const door = new MockDoor().program(casMiss, { status: 404, body: refusal('not_found', 'gone') });
    const m = await conflicted(door);
    await act(async () => {
      await m.result.current.resolve();
    });
    expect(stateOf(m)).toMatchObject({ current: null, currentIs: 'deleted' });
  });

  it('is unavailable when the catalog grants no read to fetch the row through', async () => {
    const door = new MockDoor().program(casMiss);
    const m = await conflicted(door, 'reviseEntry', { ext: 'x', amount: '1.00' } as never);
    await act(async () => {
      await m.result.current.resolve();
    });
    expect(stateOf(m)).toMatchObject({ current: null, currentIs: 'unavailable' });
  });

  it('is unavailable, and the error is thrown, when the read itself fails', async () => {
    const door = new MockDoor().program(casMiss, { status: 500, body: refusal('internal', 'boom') });
    const m = await conflicted(door);
    await act(async () => {
      await expect(m.result.current.resolve()).rejects.toMatchObject({ refusal: { class: 'internal' } });
    });
    expect(stateOf(m)).toMatchObject({ current: null, currentIs: 'unavailable' });
  });

  it('reads the row back by itself when autoResolve is on, and only once', async () => {
    const door = new MockDoor().program(casMiss, { body: { rows: [{ id: values.id, rev: 9, title: 'Renamed' }] } });
    const m = await conflicted(door, 'std.tasks.update', values, { autoResolve: true });
    await until(() => stateOf(m).currentIs === 'found', 'the row to be read back');
    expect(door.requests.filter((r) => r.url.includes('/op/std.tasks.get'))).toHaveLength(1);
  });
});
