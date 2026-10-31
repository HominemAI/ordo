/**
 * A mock door. No server, no network, no DOM.
 *
 * It records what the client sent and answers what the test tells it to, so
 * every assertion below is about *this package's* behaviour rather than about
 * a deployment that happened to be running.
 */

import { OrdoClient } from '../src/core/client.js';

export interface Recorded {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

export interface Reply {
  readonly status?: number;
  readonly json?: unknown;
  readonly headers?: Record<string, string>;
}

export class MockDoor {
  readonly calls: Recorded[] = [];
  #replies: Reply[] = [];

  /** Queue one reply per expected call; the last one repeats. */
  reply(...replies: Reply[]): this {
    this.#replies.push(...replies);
    return this;
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : String(input);
    const headers = Object.fromEntries(
      Object.entries((init?.headers ?? {}) as Record<string, string>),
    );
    const raw = init?.body;
    this.calls.push({
      url,
      method: init?.method ?? 'GET',
      headers,
      body: typeof raw === 'string' ? (JSON.parse(raw) as unknown) : null,
    });
    const reply = this.#replies.length > 1 ? this.#replies.shift()! : (this.#replies[0] ?? {});
    const status = reply.status ?? 200;
    return new Response(JSON.stringify(reply.json ?? { rows: [] }), {
      status,
      headers: { 'Content-Type': 'application/json', ...(reply.headers ?? {}) },
    });
  };

  client(space = 'spc_test'): OrdoClient {
    return new OrdoClient({
      baseUrl: 'https://door.example/api',
      space,
      token: 'tok',
      fetch: this.fetch,
      // Deterministic, so an assertion about the header is an assertion.
      newIdempotencyKey: () => 'idem-fixed',
    });
  }
}

export const q = (url: string): URLSearchParams => new URL(url).searchParams;
