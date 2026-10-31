import { binderHash, HANDOFF_BINDER_KEY } from '@hominemai/ordo-sdk/auth/handoff.js';
import { describe, expect, it } from 'vitest';
import { signInMethods } from '../src/config.js';
import { loginAddress } from '../src/subdomain.js';

function fakeStorage() {
  const kept = new Map<string, string>();
  return {
    kept,
    getItem: (k: string) => kept.get(k) ?? null,
    setItem: (k: string, v: string) => void kept.set(k, v),
    removeItem: (k: string) => void kept.delete(k),
  };
}

describe('the shop starting a sign-in', () => {
  it('builds a login address with return_to and the hash of the binder it kept', async () => {
    const storage = fakeStorage();
    const address = await loginAddress('http://localhost:5180', 'http://localhost:5181/?handoff_code=old', storage);

    const url = new URL(address);
    expect(url.origin).toBe('http://localhost:5180');
    // The one-time code of an earlier try is not carried along.
    expect(url.searchParams.get('return_to')).toBe('http://localhost:5181/');

    const binder = storage.kept.get(HANDOFF_BINDER_KEY);
    expect(binder).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(url.searchParams.get('binder_hash')).toBe(await binderHash(binder ?? ''));
    // The binder itself never travels in the address.
    expect(address).not.toContain(binder ?? 'x');
  });
});

describe('the sign-in methods setting', () => {
  it('is passkey when unset', () => {
    expect(signInMethods(undefined)).toEqual({ passkey: true, email: false });
  });

  it('reads a list', () => {
    expect(signInMethods(' Passkey , email ')).toEqual({ passkey: true, email: true });
    expect(signInMethods('email')).toEqual({ passkey: false, email: true });
  });
});
