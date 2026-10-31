/**
 * Resuming on load: a network failure is not a refusal. A refusal goes to
 * sign-in; a network failure opens the device copy and asks again when the
 * network returns.
 */

import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import { MemoryBackend } from '@hominemai/ordo-sdk/replica/backend.js';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  forget,
  isNetworkFailure,
  openOfflineNotes,
  recall,
  remember,
  resumeOutcome,
  whenOnline,
} from '../src/resume.js';
import { door, refusal, signedIn } from './support.js';

const BASE = 'https://door.example/';
const down: typeof globalThis.fetch = async () => {
  throw new TypeError('Failed to fetch');
};
const network = { pageOrigin: 'http://localhost:5173', probe: false } as never;

class Store {
  readonly m = new Map<string, string>();
  getItem(k: string) {
    return this.m.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.m.set(k, v);
  }
  removeItem(k: string) {
    this.m.delete(k);
  }
}

describe('telling a network failure from a refusal', () => {
  it('reads a resume that cannot reach the door as offline', async () => {
    const out = await resumeOutcome({ baseUrl: BASE, fetch: down, network });
    expect(out.kind).toBe('offline');
    expect(out.kind === 'offline' && isNetworkFailure(out.error)).toBe(true);
  });

  it('reads a door that cannot check right now (503) as offline too', async () => {
    const d = door({ status: 503, json: { error: { code: 'unavailable', message: 'busy' } } });
    expect((await resumeOutcome({ baseUrl: BASE, fetch: d.fetch })).kind).toBe('offline');
  });

  it('reads a refusal as nothing to resume, so the page shows sign-in', async () => {
    const d = door(refusal(401, 'unauthenticated'));
    expect((await resumeOutcome({ baseUrl: BASE, fetch: d.fetch })).kind).toBe('none');
  });

  it('does not take an unreadable answer for a network failure', async () => {
    const d = door({ status: 200, json: 'nonsense' });
    await expect(resumeOutcome({ baseUrl: BASE, fetch: d.fetch })).rejects.toBeDefined();
    expect(isNetworkFailure(new Error('x'))).toBe(false);
    expect(isNetworkFailure(new OrdoNetworkError('unreachable', { baseUrl: BASE, pageOrigin: null }))).toBe(true);
  });
});

describe('then, when the network comes back', () => {
  it('opens the device copy offline, and the resume signs in on the next try', async () => {
    const options = { baseUrl: BASE, fetch: down, network };
    const first = await resumeOutcome(options);
    expect(first.kind).toBe('offline');

    // The device copy opens for the person this browser remembers, with no token.
    const backend = new MemoryBackend();
    const opened = await openOfflineNotes(options, { space: 'spc_ada', principal: 'usr_ada' }, backend);
    expect(opened.device.space).toBe('spc_ada');
    expect(opened.device.principal).toBe('usr_ada');

    // The browser reports the network: the page asks again, and the door answers.
    const target = new EventTarget();
    let again: ReturnType<typeof resumeOutcome> | undefined;
    const back = door({ json: signedIn('usr_ada', 'spc_ada', { resumed: true, via: 'passkey' }) });
    const stop = whenOnline(target, () => {
      again = resumeOutcome({ baseUrl: BASE, fetch: back.fetch });
    });
    target.dispatchEvent(new Event('online'));
    if (!again) throw new Error('the page did not ask again');
    const out = await again;
    expect(out.kind).toBe('session');
    expect(out.kind === 'session' && out.session.space).toBe('spc_ada');
    stop();
    target.dispatchEvent(new Event('online'));
    expect(back.sent.length).toBe(1);
    await opened.device.close();
  });
});

describe('whose device copy this is', () => {
  let store: Store;
  beforeEach(() => {
    store = new Store();
  });
  afterEach(() => undefined);

  it('is kept after sign-in and erased when the sign-in ends', () => {
    expect(recall(store)).toBeNull();
    remember({ space: 'spc_ada', principal: 'usr_ada' }, store);
    expect(recall(store)).toEqual({ space: 'spc_ada', principal: 'usr_ada' });
    forget(store);
    expect(recall(store)).toBeNull();
  });

  it('is read as nobody when the stored text is damaged', () => {
    store.setItem('notes-v2.device-owner', '{"space":1}');
    expect(recall(store)).toBeNull();
  });
});
