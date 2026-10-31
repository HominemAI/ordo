import { describe, expect, it } from 'vitest';
import { FALLBACK, loadOffer, type Offer } from '../src/methods.js';

function door(reply: { status?: number; json?: unknown } | 'down') {
  const urls: string[] = [];
  const fetch: typeof globalThis.fetch = async (input) => {
    urls.push(String(input));
    if (reply === 'down') throw new TypeError('offline');
    return new Response(reply.json === undefined ? null : JSON.stringify(reply.json), {
      status: reply.status ?? 200,
    });
  };
  return { urls, endpoint: { baseUrl: 'https://door.example', fetch } };
}

const BASE = { passkey: true, password: false, email: false, guest: false, humanCheck: null, connections: [] };
const BUILD: Offer = {
  methods: { passkey: true, email: false, guest: false, connections: [] },
  siteKey: 'build-key',
  source: 'build',
};

describe('the ways in come from the door', () => {
  it('builds the offer from the door and ignores the build variables', async () => {
    const d = door({
      json: {
        ...BASE,
        email: true,
        guest: true,
        humanCheck: { kind: 'turnstile', siteKey: 'door-key' },
        connections: [{ name: 'discord', label: 'Discord', kind: 'oauth2' }],
      },
    });
    const got = await loadOffer(d.endpoint, BUILD);
    expect(d.urls).toEqual(['https://door.example/auth/methods']);
    expect(got).toEqual({
      methods: {
        passkey: true,
        email: true,
        guest: true,
        connections: [{ name: 'discord', label: 'Discord' }],
      },
      siteKey: 'door-key',
      source: 'door',
    });
  });

  it('has no widget when the door has the human check off, whatever the build says', async () => {
    const got = await loadOffer(door({ json: BASE }).endpoint, BUILD);
    expect(got.siteKey).toBeNull();
    expect(got.source).toBe('door');
  });

  it('uses the build key when the check is on and the door has no public key', async () => {
    const got = await loadOffer(door({ json: { ...BASE, humanCheck: { kind: 'turnstile' } } }).endpoint, BUILD);
    expect(got.siteKey).toBe('build-key');
  });

  it('asks for a tenant when told one', async () => {
    const d = door({ json: BASE });
    await loadOffer(d.endpoint, BUILD, 'spc_acme');
    expect(d.urls).toEqual(['https://door.example/auth/methods?tenant=spc_acme']);
  });

  it('falls back to the build variables when the door is down or has no such route', async () => {
    expect(await loadOffer(door('down').endpoint, BUILD)).toBe(BUILD);
    expect(await loadOffer(door({ status: 404 }).endpoint, BUILD)).toBe(BUILD);
    expect(await loadOffer(door({ json: { nonsense: true } }).endpoint, BUILD)).toBe(BUILD);
  });

  it('defaults the fallback to the passkey alone when no variable is set', () => {
    expect(FALLBACK.methods).toEqual({ passkey: true, email: false, guest: false, connections: [] });
    expect(FALLBACK.siteKey).toBeNull();
    expect(FALLBACK.source).toBe('build');
  });
});
