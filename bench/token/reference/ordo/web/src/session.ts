/**
 * The signed-in person, on Ordo.
 *
 * Every person signs in with an emailed code. The token lives in memory.
 * A reload resumes the sign-in through the door's renewal cookie
 * (`resumeSession`), so the person stays signed in.
 *
 * A team is a tenant. `listSpaces` lists the person's teams, each with the
 * role they hold there. A tenant's `name` is its owner's display name, and
 * every person's display name is their email (sent as `display` when the
 * emailed-code sign-in completes).
 */

import { beginEmailSignIn, completeEmailSignIn, type EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import type { PasskeyEndpoint, PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import { waitForTenant } from '@hominemai/ordo-sdk/auth/provisioning.js';
import { resumeSession } from '@hominemai/ordo-sdk/auth/resume.js';
import { listSpaces } from '@hominemai/ordo-sdk/auth/share.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { OrdoRefusal } from '@hominemai/ordo-sdk/core/refusal.js';

export const DOOR: string = import.meta.env.VITE_BACKEND_URL ?? 'http://localhost:4710';

/** The team and ticket of an invitation link, `#/join/<space>/<ticket>`. */
export interface Join {
  readonly space: string;
  readonly invitation: string;
}

export interface Team {
  readonly id: string;
  /** The owner's email. */
  readonly owner: string;
  readonly role: 'owner' | 'member';
}

export function joinOf(hash: string): Join | null {
  const m = /^#\/join\/([A-Za-z0-9_-]+)\/([A-Za-z0-9_-]+)$/.exec(hash);
  return m && m[1] && m[2] ? { space: m[1], invitation: m[2] } : null;
}

export function inviteLink(space: string, ticket: string): string {
  return `${window.location.origin}/#/join/${space}/${ticket}`;
}

/**
 * The SDK's emailed-code sign-in sends no `display`. The door takes one on
 * `/auth/complete` and makes it the person's display name, so this adds it.
 */
function endpointWithDisplay(display: string): PasskeyEndpoint {
  const send: typeof fetch = (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (url.endsWith('/auth/complete') && init && typeof init.body === 'string') {
      const body = JSON.parse(init.body) as Record<string, unknown>;
      body['display'] = display;
      return fetch(input, { ...init, body: JSON.stringify(body) });
    }
    return fetch(input, init);
  };
  return { baseUrl: DOOR, fetch: send };
}

const plain: PasskeyEndpoint = { baseUrl: DOOR };

export class Session {
  #token: string;
  #expiresAt: number;
  /** The team a sign-in from an invitation link joined, if any. */
  readonly joined: string | null;

  private constructor(done: PasskeySession) {
    this.#token = done.token;
    this.#expiresAt = done.expiresAt;
    const outcome = done.admission?.outcome;
    this.joined =
      done.admission && (outcome === 'admitted' || outcome === 'already-member') ? done.admission.space : null;
  }

  static sendCode(address: string, join: Join | null): Promise<EmailCodeSent> {
    return beginEmailSignIn(plain, { address, ...(join ? { space: join.space, invitation: join.invitation } : {}) });
  }

  static async signIn(address: string, sent: EmailCodeSent, code: string, join: Join | null): Promise<Session> {
    const endpoint = endpointWithDisplay(address);
    const result = await completeEmailSignIn(endpoint, sent, code, join ? { space: join.space, invitation: join.invitation } : {});
    if (result.kind !== 'session') throw new Error('This deployment asks for a second step, which this page does not offer.');
    const done = result.session;
    // A new person's team is made just after the sign-in answers.
    if (done.raw['provisioning'] === 'in progress') await waitForTenant(plain, done.token);
    return new Session(done);
  }

  /** The sign-in this browser already holds, or null. */
  static async resume(): Promise<Session | null> {
    const done = await resumeSession(plain);
    return done ? new Session(done) : null;
  }

  async token(): Promise<string> {
    if (this.#expiresAt - Date.now() / 1000 > 60) return this.#token;
    const r = await fetch(`${DOOR}/auth/refresh`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${this.#token}`, Accept: 'application/json' },
    });
    if (!r.ok) throw new Error('Your sign-in has ended. Sign in again.');
    const body = (await r.json()) as { token: string; expires_at: number };
    this.#token = body.token;
    this.#expiresAt = body.expires_at;
    return this.#token;
  }

  async teams(): Promise<Team[]> {
    const listed = await listSpaces(plain, await this.token());
    return listed
      .filter((s) => s.via === 'member' && s.role !== null)
      .map((s) => ({ id: s.space, owner: s.name ?? s.space, role: s.role === 'owner' ? 'owner' : 'member' }));
  }

  client(space: string): OrdoClient {
    return new OrdoClient({ baseUrl: DOOR, space, token: () => this.token() });
  }

  /** A platform op (`std.space.*`) in one team. */
  async platform(space: string, op: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    const r = await fetch(`${DOOR}/op/${op}`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${await this.token()}`,
        'X-Ordo-Space': space,
        'Content-Type': 'application/json',
        Accept: 'application/json',
      },
      body: JSON.stringify(params),
    });
    const body = (await r.json().catch(() => ({}))) as { rows?: Record<string, unknown>[]; error?: { message?: string } };
    if (!r.ok) throw new Error(body.error?.message ?? `The deployment refused ${op}.`);
    return body.rows?.[0] ?? {};
  }

  async signOut(): Promise<void> {
    await fetch(`${DOOR}/auth/logout`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${this.#token}`, Accept: 'application/json' },
      credentials: 'include',
    });
  }
}

/**
 * Run `read` until the door serves a team made or joined a moment ago. A
 * fresh team can be refused `access_denied` for a few seconds while the
 * door rebuilds.
 */
export async function untilServed<T>(read: () => Promise<T>): Promise<T> {
  for (let i = 1; ; i++) {
    try {
      return await read();
    } catch (e) {
      if (!(e instanceof OrdoRefusal && e.class === 'access_denied') || i >= 40) throw e;
      await new Promise((r) => setTimeout(r, 150));
    }
  }
}

export function describe(e: unknown): string {
  if (e instanceof OrdoRefusal) {
    if (e.class === 'cas_miss') return 'Someone changed this task meanwhile. It has been reloaded.';
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}
