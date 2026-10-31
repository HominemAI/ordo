/**
 * A door that answers what a test tells it to, in order, and records what it
 * was sent, and a stand-in for `navigator.credentials`. No server, no browser.
 */

import { toBase64Url } from '@hominemai/ordo-sdk/auth/passkey.js';
import { DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { INTERFACE } from '../src/interface.js';

export interface Sent {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: Record<string, unknown> | null;
}

export interface Reply {
  readonly status?: number;
  readonly json?: unknown;
}

export interface Door {
  readonly sent: Sent[];
  readonly fetch: typeof globalThis.fetch;
}

/** Replies are used in order; the last one repeats. A function chooses by request. */
export function door(...replies: Array<Reply | ((s: Sent) => Reply)>): Door {
  const sent: Sent[] = [];
  let i = 0;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const raw = init?.body;
    const request: Sent = {
      url: String(input),
      method: init?.method ?? 'GET',
      headers: Object.fromEntries(Object.entries((init?.headers ?? {}) as Record<string, string>)),
      body: typeof raw === 'string' ? (JSON.parse(raw) as Record<string, unknown>) : null,
    };
    sent.push(request);
    const pick = replies[Math.min(i++, replies.length - 1)] ?? { status: 500 };
    const r = typeof pick === 'function' ? pick(request) : pick;
    const status = r.status ?? 200;
    return new Response(status === 204 || r.json === undefined ? null : JSON.stringify(r.json), { status });
  };
  return { sent, fetch };
}

export function refusal(status: number, code: string): Reply {
  return { status, json: { error: { code, message: `${code} at the door` } } };
}

/** A token as the door writes them: `<base64url v4|principal|expiry|...>.<signature>`. */
export function tokenFor(principal: string, n = 1): string {
  const payload = new TextEncoder().encode(`v4|${principal}|9999999999||1||${n}||`);
  return `${toBase64Url(payload)}.signature${n}`;
}

const bytes = (...n: number[]) => Uint8Array.from(n);
const buf = (u: Uint8Array): ArrayBuffer =>
  u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength) as ArrayBuffer;

export const CHALLENGE = toBase64Url(Uint8Array.from({ length: 32 }, (_, i) => i + 1));

export const CREATION_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'localhost',
  rp_name: 'localhost',
  user: { id: toBase64Url(bytes(9, 9, 9)), name: 'Ada', display_name: 'Ada' },
  algorithms: [-7, -8],
  user_verification: 'required',
  resident_key: 'required',
  attestation: 'none',
  exclude_credentials: [] as string[],
  timeout_ms: 300000,
};

export const REQUEST_OPTIONS = {
  challenge: CHALLENGE,
  method: 'passkey',
  rp_id: 'localhost',
  user_verification: 'required',
  allow_credentials: [] as string[],
  timeout_ms: 300000,
};

/** What a browser hands back for a create and for a get. */
export const credentials = {
  create: async () =>
    ({
      rawId: buf(bytes(1)),
      response: { clientDataJSON: buf(bytes(2)), attestationObject: buf(bytes(3)) },
    }) as unknown as Credential,
  get: async () =>
    ({
      rawId: buf(bytes(1)),
      response: {
        clientDataJSON: buf(bytes(2)),
        authenticatorData: buf(bytes(3)),
        signature: buf(bytes(4)),
      },
    }) as unknown as Credential,
};

/** A client over a door, bound to a space. */
export function clientOver(d: Door, space = 'spc_ada', token = 'tok'): OrdoClient {
  return new OrdoClient({
    baseUrl: 'https://door.example',
    space,
    token,
    fetch: d.fetch,
    newIdempotencyKey: () => 'key',
  });
}

/** The hooks' client over a door, bound to a space: what `<OrdoProvider>` is given. */
export function hooksOver(d: Door, space = 'spc_ada', token = 'tok'): HooksClient {
  return new HooksClient({
    baseUrl: 'https://door.example',
    iface: INTERFACE,
    space,
    token,
    fetch: d.fetch,
    newIdempotencyKey: () => 'key',
  });
}

/** What `/auth/complete` answers when the deployment gave the person a tenant. */
export function signedIn(principal = 'usr_ada', space: string | null = 'spc_ada', extra = {}) {
  return {
    token: tokenFor(principal),
    expires_at: 4000,
    session_expires_at: 90000,
    space,
    provisioning: null,
    ...extra,
  };
}
