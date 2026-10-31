/**
 * The test harness: the real golden descriptor, and a mock door.
 *
 * There is no fixture of our own. `crates/ordo-cli/tests/golden/market.interface.json`
 * is the emitter's own golden — the file `ordo-cli/tests/interface.rs` walks
 * back to the manifest in both directions — so a test here that passes
 * against it is a test against the shape the build actually emits. A
 * hand-written fixture would be a second, drifting statement of the contract.
 */

import golden from '../../../crates/ordo-cli/tests/golden/market.interface.json' with { type: 'json' };
import { DescriptorClient, type DescriptorClientOptions } from '../src/client.js';
import { parseInterface, type OrdoInterface } from '../src/load.js';

export const GOLDEN = golden as unknown as Record<string, unknown>;

export function iface(): OrdoInterface {
  return parseInterface(GOLDEN);
}

export interface RecordedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
  readonly query: URLSearchParams;
}

export interface Programmed {
  readonly status?: number;
  readonly body?: unknown;
  readonly headers?: Record<string, string>;
}

/**
 * A door that answers from a script. Every call is recorded, so a test can
 * assert on the *request* — which is where the header discipline, the cursor
 * threading and the method mapping actually live.
 */
export class MockDoor {
  readonly requests: RecordedRequest[] = [];
  #script: Programmed[] = [];
  #default: Programmed = { status: 200, body: { rows: [] } };

  /** Queue responses, consumed in order. */
  program(...responses: Programmed[]): this {
    this.#script.push(...responses);
    return this;
  }

  always(response: Programmed): this {
    this.#default = response;
    return this;
  }

  get last(): RecordedRequest {
    const r = this.requests.at(-1);
    if (!r) throw new Error('no request recorded');
    return r;
  }

  readonly fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : String(input);
    const parsed = new URL(url, 'http://door.test');
    const headers: Record<string, string> = {};
    for (const [k, v] of Object.entries((init?.headers ?? {}) as Record<string, string>)) {
      headers[k] = v;
    }
    this.requests.push({
      url,
      method: init?.method ?? 'GET',
      headers,
      body: typeof init?.body === 'string' ? (JSON.parse(init.body) as unknown) : undefined,
      query: parsed.searchParams,
    });

    const next = this.#script.shift() ?? this.#default;
    const status = next.status ?? 200;
    return new Response(next.body === undefined ? '' : JSON.stringify(next.body), {
      status,
      headers: { 'Content-Type': 'application/json', ...(next.headers ?? {}) },
    });
  };
}

export function client(door: MockDoor, overrides: Partial<DescriptorClientOptions> = {}): DescriptorClient {
  // A counter rather than a constant: two attempts must not share a key, and
  // a fixed stub would hide it if they did.
  let n = 0;
  return new DescriptorClient({
    baseUrl: 'http://door.test',
    iface: iface(),
    space: 'spc_test',
    token: 'tok-abc',
    fetch: door.fetch,
    newIdempotencyKey: () => `idem-${n++}`,
    ...overrides,
  });
}

/** The wire's error envelope, for programming a refusal. */
export function refusal(code: string, message = 'refused', extra: Record<string, string> = {}): Programmed['body'] {
  return { error: { code, message, ...extra } };
}
