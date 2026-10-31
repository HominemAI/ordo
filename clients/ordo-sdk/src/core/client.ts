/**
 * The door client: every routing decision read off the generated spec
 * rather than guessed from an op's name.
 *
 * **Static half.** There is one `request` here and it knows about no op: the
 * method, the version pin, the CAS parameter and the bounds all arrive as
 * data. So this file is reviewed once and a deploy regenerates the data.
 *
 * Every call goes through the one request helper (`core/door.ts`): the
 * deadline, a failure to get an answer named, the one retry policy. A
 * refusal is the one `OrdoRefusal`. `@hominemai/ordo-react`'s
 * `DescriptorClient` sends the same headers to the same routes through the
 * same helper, and builds its answers with {@link opResponseOf}.
 */

import { checkRefine, encodeRefine, type Refine } from './refine.js';
import { OrdoLocalRefusal, type RefusalClass } from './refusal.js';
import type { OpSpec, ParamSpec } from './spec.js';
import { busyRetryOf, conflictRetryOf } from './busy.js';
import { NO_RETRY, busyOnly, doorRequest, fetchOf, readBody, refusalOf } from './door.js';
import type {
  BusyRetryOptions,
  ConflictRetryOptions,
  OnRetry,
  ResolvedBusyRetry,
  ResolvedConflictRetry,
} from './busy.js';
import { isJsonValue, type JsonValue } from './json.js';
import { callFunction, type FunctionCallOptions, type FunctionResponse } from './function.js';
import type { FileDoor } from './files.js';
import {
  DEFAULT_CLIENT_CALL_TIMEOUT_MS,
  networkError,
  streamError,
  type NetworkOptions,
  type NetworkSite,
  type OrdoNetworkError,
} from './network.js';

export type Row = Record<string, unknown>;
/**
 * A parameter's value. Every scalar the door binds is a JSON string, number,
 * boolean or null; a `json` parameter takes any JSON value (28b).
 */
export type ParamValue = JsonValue;
export type Params = Readonly<Record<string, ParamValue>>;

export interface OpResponse<R = Row> {
  readonly op: string;
  readonly version: string;
  readonly mode?: string;
  readonly rows: readonly R[];
  /**
   * Present iff another page exists. **Its absence is the only terminator**
   * (§7): a narrow page can be short or empty while `next` is still set, and
   * a loop that stops on an empty page silently loses rows.
   */
  readonly next?: string;
  readonly servedVersion: string | null;
  readonly stale: boolean;
  readonly replayed: boolean;
  /**
   * The space's command-log position this answer was read at, and the signed
   * chain head there (layer-1 pinning). `null` when the door sent no position,
   * or one this client cannot represent exactly.
   *
   * A local replica orders what it holds by `pin.position`: an answer is only
   * newer than another if its position is higher. The door takes the position
   * in the same snapshot as the rows, so it is the rows' version rather than a
   * later one (`plan-client-cache.md` C1).
   */
  readonly pin: Pin | null;
}

/** The signed chain head a response was read at. */
export interface Pin {
  /** Groups the space's chain held when the rows were read. */
  readonly position: number;
  /** The chain head at that position, hex. */
  readonly head: string;
  /** Which key signed the head, so its public half can be found. */
  readonly keyId: string | null;
  /** Detached signature over the head, hex. */
  readonly signature: string | null;
  /**
   * The earliest position the chain can still be verified from. A client
   * holding an older pin can no longer recompute the span between the two.
   */
  readonly verifiableFrom: number | null;
}

export type TokenSource = string | (() => string | Promise<string>);

/** The name refusals of `GET /download` carry as their op. */
export const DOWNLOAD_NAME = 'download';

/** A person's own download: the zip, and the name the door gave it. */
export interface OwnDownload {
  readonly blob: Blob;
  /** `download-2026-10-22.zip`, from `Content-Disposition`. */
  readonly name: string;
}

/** The file name in a `Content-Disposition` header, or `download.zip`. */
export function downloadName(header: string | null): string {
  const m = /filename="([^"\\/]+)"/.exec(header ?? '');
  return m?.[1] ?? 'download.zip';
}

export interface OrdoClientOptions {
  /** Door base URL. `/op/<name>` is appended. */
  readonly baseUrl: string;
  /**
   * The space every call binds to. There is no default space and none is
   * inferred from the token (§2) — a human belongs to many — so it is
   * required, and it rides as `X-Ordo-Space` on every method.
   */
  readonly space: string;
  /**
   * The caller's token. Optional: a public op (`spec.public`) can be called
   * with none, and then the call sends no `Authorization` and no
   * `X-Ordo-Space` (the server runs it as the anonymous principal in the
   * deployment's own space). Any other op needs a token.
   */
  readonly token?: TokenSource | undefined;
  /**
   * Mint the token again now, after the door refused the one held. Answers
   * `true` when a new one was minted and `false` when the door will not give
   * one (the window was closed). A guest's feed has one
   * ({@link import('../auth/share.js').GuestFeed.renew}): accepting a second
   * share replaces the grant the held token names, so that token is refused
   * while the share itself is still open.
   */
  readonly renewToken?: (() => Promise<boolean>) | undefined;
  readonly fetch?: typeof globalThis.fetch;
  /**
   * How long a call waits for the door's answer, in milliseconds. Default
   * {@link DEFAULT_CLIENT_CALL_TIMEOUT_MS}. A call that gets none fails with
   * an {@link OrdoNetworkError} of kind `timeout` that names this option. For
   * a feed it is the wait for the door to accept the connection; an open feed
   * has no deadline.
   */
  readonly callTimeoutMs?: number;
  /**
   * How a call that got no answer is named: which path tells a blocked
   * cross-origin call from a door that is down, and how long that answer is
   * kept. See {@link OrdoNetworkError}.
   */
  readonly network?: NetworkOptions;
  readonly newIdempotencyKey?: () => string;
  /**
   * How a call is sent again when the door answers `server_busy` (a `503`
   * with `Retry-After`: every database connection was in use and nothing
   * ran). Defaults: `{ retries: 2, firstDelayMs: 500, factor: 2,
   * maxDelayMs: 10000, jitter: true }`. Retry `n` waits
   * `min(maxDelayMs, firstDelayMs * factor^n)`, never less than the
   * `Retry-After` (held to `maxDelayMs`), plus up to 20% jitter. `retries: 0`
   * turns retrying off. A bad value throws `TypeError`. No other refusal is
   * retried.
   */
  readonly busyRetry?: BusyRetryOptions;
  /** Shorthand for `busyRetry.retries`. */
  readonly busyRetries?: number;
  /**
   * How a call is sent again when the door answers `serialization_conflict`
   * (a `409`: the write lost a race with other writes to the same rows, the
   * server's own retries, `ORDO_WRITE_RETRIES`, ran out, and nothing was
   * applied). Defaults: `{ retries: 3, firstDelayMs: 50, factor: 2,
   * maxDelayMs: 1000, jitter: true }`. Retry `n` waits
   * `min(maxDelayMs, firstDelayMs * factor^n)`, with jitter a random time
   * from half that to all of it. `retries: 0` turns retrying off. A bad
   * value throws `TypeError`. A write keeps its `Idempotency-Key` across
   * retries.
   */
  readonly conflictRetry?: ConflictRetryOptions;
  /** Shorthand for `conflictRetry.retries`. */
  readonly conflictRetries?: number;
}

export interface RequestOptions {
  readonly page?: string | undefined;
  readonly idempotencyKey?: string | undefined;
  readonly signal?: AbortSignal | undefined;
  /**
   * Called before each wait on a `server_busy` or `serialization_conflict`
   * answer, with the retry's number for that reason (from 1), the wait in
   * milliseconds and the reason, so a screen can say it is retrying (L7).
   */
  readonly onRetry?: OnRetry | undefined;
  /**
   * A refinement for a read: a filter and a pick over the op's own outputs.
   * Checked before the wire by the shared validator (`core/refine.ts`), then
   * sent as `$refine`: URL-encoded JSON in the query string of a GET, or a
   * body key of a POST. The generated builders (`.where`, `.pick`) fill it.
   */
  readonly refine?: Refine | undefined;
}

/**
 * Ask `renew` for a new token, or answer `false` when there is none. The one
 * body of {@link OrdoClient.renewToken}, shared with ordo-react's client.
 */
export function renewedBy(renew: (() => Promise<boolean>) | undefined): Promise<boolean> {
  return renew ? renew() : Promise.resolve(false);
}

export class OrdoClient {
  readonly space: string;

  readonly #baseUrl: string;
  readonly #token: TokenSource | undefined;
  readonly #renew: (() => Promise<boolean>) | undefined;
  readonly #fetch: typeof globalThis.fetch;
  readonly #site: NetworkSite;
  readonly #callTimeoutMs: number | undefined;
  readonly #newKey: () => string;
  readonly #busyRetry: ResolvedBusyRetry;
  readonly #conflictRetry: ResolvedConflictRetry;

  constructor(options: OrdoClientOptions) {
    this.space = options.space;
    this.#baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.#token = options.token;
    this.#renew = options.renewToken;
    this.#fetch = fetchOf(options.fetch);
    this.#callTimeoutMs = options.callTimeoutMs;
    this.#site = {
      baseUrl: this.#baseUrl,
      fetch: this.#fetch,
      network: options.network,
      timeoutOption: 'callTimeoutMs',
      timeoutMs: options.callTimeoutMs ?? DEFAULT_CLIENT_CALL_TIMEOUT_MS,
    };
    this.#newKey = options.newIdempotencyKey ?? (() => crypto.randomUUID());
    this.#busyRetry = busyRetryOf(options.busyRetry, options.busyRetries);
    this.#conflictRetry = conflictRetryOf(options.conflictRetry, options.conflictRetries);
  }

  /**
   * What the file helpers (`uploadFile`, `fileUrl`, `downloadFile`) need: the
   * door, the space, the token and the `fetch` this client uses. A failure to
   * get an answer is named as an {@link OrdoNetworkError}.
   */
  fileDoor(): FileDoor {
    return {
      baseUrl: this.#baseUrl,
      space: this.space,
      token: async () => (this.#token === undefined ? undefined : typeof this.#token === 'function' ? await this.#token() : this.#token),
      fetch: this.#fetch,
      wrap: async (e) => networkError(e, this.#site),
      busyRetry: this.#busyRetry,
    };
  }

  /**
   * Call a backend function, `POST /fn/<name>`, as the signed-in person
   * (`docs/howto-functions.md`). The answer is the function's JSON under
   * `result`. A refusal is thrown as the ordinary {@link OrdoRefusal}.
   */
  callFunction<R = JsonValue>(
    name: string,
    params: Readonly<Record<string, unknown>> = {},
    options: FunctionCallOptions = {},
  ): Promise<FunctionResponse<R>> {
    return callFunction<R>(
      {
        baseUrl: this.#baseUrl,
        space: this.space,
        token: async () =>
          this.#token === undefined ? undefined : typeof this.#token === 'function' ? await this.#token() : this.#token,
        transport: (url, init, onRetry) => this.#send(url, init, 'busy', onRetry),
      },
      name,
      params,
      options,
    );
  }

  /** A client bound to a different space; everything else is shared. */
  withSpace(space: string): OrdoClient {
    return new OrdoClient({
      baseUrl: this.#baseUrl,
      space,
      token: this.#token,
      renewToken: this.#renew,
      fetch: this.#fetch,
      ...(this.#callTimeoutMs === undefined ? {} : { callTimeoutMs: this.#callTimeoutMs }),
      ...(this.#site.network ? { network: this.#site.network } : {}),
      newIdempotencyKey: this.#newKey,
      busyRetry: this.#busyRetry,
      conflictRetry: this.#conflictRetry,
    });
  }

  /**
   * Mint the token again after the door refused it. `true` when a new one
   * was minted, so the refused read is worth one more try; `false` when this
   * client has no way to renew or the door will not renew it, so the refusal
   * stands. A failure to reach the door is thrown, as by any call.
   */
  renewToken(): Promise<boolean> {
    return renewedBy(this.#renew);
  }

  /**
   * The {@link OrdoNetworkError} for a feed's body that broke after the door
   * had answered, or `null` for an abort the caller asked for.
   */
  feedBroken(e: unknown): OrdoNetworkError | null {
    return streamError(e, this.#site);
  }

  /**
   * One call through the request helper: the deadline, a failure to get an
   * answer named, and the client's retry policy:
   *
   * - `op`: busy and conflict, for an op call (a write's `Idempotency-Key`
   *   makes a resend safe).
   * - `busy`: busy only, for the download.
   * - `feed`: none. A feed's opener (the sync manager, the live hub)
   *   reconnects on the same classes with the door's `Retry-After`, so a
   *   retry here would be a second loop over the first.
   */
  #send(url: string, init: RequestInit, retry: 'op' | 'busy' | 'feed', onRetry?: OnRetry): Promise<Response> {
    return doorRequest(this.#site, url, init, {
      retry:
        retry === 'op'
          ? { busy: this.#busyRetry, conflict: this.#conflictRetry }
          : retry === 'busy'
            ? busyOnly(this.#busyRetry)
            : NO_RETRY,
      onRetry,
    });
  }

  /**
   * A person's own download, `GET /download`: everything the app developer
   * marked `download` in the program that this person may read, as one zip
   * file (`docs/howto-leave.md`, "A person's own download"). Needs a token.
   *
   * A refusal is thrown as an {@link OrdoRefusal}: `access_denied` when the
   * app developer has not turned the download on (the message says how),
   * `quota_exceeded` past `ORDO_OWN_DOWNLOAD_PER_HOUR`.
   */
  async downloadOwn(options: { readonly signal?: AbortSignal } = {}): Promise<OwnDownload> {
    const token = this.#token === undefined
      ? undefined
      : typeof this.#token === 'function' ? await this.#token() : this.#token;
    if (!token) {
      throw new OrdoLocalRefusal({
        class: 'access_denied',
        message: "a download is a signed-in person's own: this client has no token",
        op: DOWNLOAD_NAME,
        param: null,
      });
    }
    const headers: Record<string, string> = {
      Authorization: `Bearer ${token}`,
      'X-Ordo-Space': this.space,
      Accept: 'application/zip',
    };
    // Never through the browser's HTTP cache: the file is the person's own.
    const init: RequestInit = { method: 'GET', headers, cache: 'no-store' };
    if (options.signal) init.signal = options.signal;
    const response = await this.#send(`${this.#baseUrl}/download`, init, 'busy');
    if (!response.ok) throw await refusalOf(response, DOWNLOAD_NAME);
    return { blob: await response.blob(), name: downloadName(response.headers.get('Content-Disposition')) };
  }

  /** The headers every call to `spec` carries. */
  async #headers(spec: OpSpec, options: RequestOptions): Promise<Record<string, string>> {
    const write = spec.kind !== 'read';
    const token = this.#token === undefined
      ? undefined
      : typeof this.#token === 'function' ? await this.#token() : this.#token;
    // A tokenless public call names no space: the server uses the
    // deployment's own space, and naming a tenant's space is refused.
    const anonymousPublic = spec.public === true && !token;
    const headers: Record<string, string> = {};
    // Preferred over `$space` on every method: a space id in a query string
    // ends up in proxy logs and `Referer` headers (§2).
    if (!anonymousPublic) headers['X-Ordo-Space'] = this.space;
    // The pin is per-op (§10). A registry-wide pin marks every call stale
    // after any unrelated deploy, and a signal that fires constantly is one
    // clients learn to ignore.
    headers['X-Ordo-Version'] = spec.version;
    headers['Accept'] = 'application/json';
    if (token) headers['Authorization'] = `Bearer ${token}`;
    // Cursors are data, not addressing: the header form keeps them out of
    // proxy logs, browser history and `Referer`.
    if (options.page) headers['X-Ordo-Page'] = options.page;
    if (write) headers['Idempotency-Key'] = options.idempotencyKey ?? this.#newKey();
    return headers;
  }

  /**
   * Open the op's sync feed, `?live=1&sync=1` (`plan-replica-feed.md` C4b).
   * The response body is the event stream; a refusal at the door is thrown as
   * the ordinary {@link OrdoRefusal}. Most callers want `syncOp`, which
   * applies the frames to the device replica.
   *
   * `refine` narrows the sync to the rows its `where` matches (R6, ruled
   * 2026-10-05, ask 2). A `pick` is refused here as the door would refuse
   * it: the device stores whole rows keyed by id, so pick on the device.
   */
  async openFeed(
    spec: OpSpec,
    params: Params,
    signal?: AbortSignal,
    resume?: string,
    refine?: Refine,
  ): Promise<Response> {
    if (spec.standingError) {
      throw new OrdoLocalRefusal({
        class: spec.standingError.code as RefusalClass,
        message: spec.standingError.message,
        op: spec.op,
        param: null,
      });
    }
    checkParams(spec, params);
    const where = syncRefine(spec, refine);
    const headers = await this.#headers(spec, {});
    headers['Accept'] = 'text/event-stream';
    // What this device already holds, as the server's own hash of the result
    // it last sent (`plan-replica-feed.md` C4d). The server either resumes on
    // it or ignores it and reloads; it is never trusted as row content.
    if (resume) headers['X-Ordo-Resume'] = resume;
    const q = new URLSearchParams({ live: '1', sync: '1' });
    for (const [k, v] of Object.entries(params)) {
      if (v === null || v === undefined) continue;
      q.set(k, String(v));
    }
    if (where !== null) q.set('$refine', where);
    // Never through the browser's HTTP cache. A stream it caches holds the
    // cache entry for its URL, and Firefox makes the next request for that
    // URL wait until the stream ends, which a feed never does. Another tab,
    // or this op in another space, is the same URL: the space rides in a
    // header.
    const init: RequestInit = { method: 'GET', headers, cache: 'no-store' };
    if (signal) init.signal = signal;
    const response = await this.#send(`${this.#baseUrl}/op/${encodeURIComponent(spec.op)}?${q}`, init, 'feed');
    // A busy door's wait rides on the refusal, so the sync manager can back
    // off for as long as the door asked before it reconnects.
    if (!response.ok) throw await refusalOf(response, spec.op, spec.refusals);
    return response;
  }

  async request<R>(
    spec: OpSpec,
    params: Params,
    options: RequestOptions = {},
  ): Promise<OpResponse<R>> {
    // A standing registration failure is knowable before the round trip.
    if (spec.standingError) {
      throw new OrdoLocalRefusal({
        class: spec.standingError.code as RefusalClass,
        message: spec.standingError.message,
        op: spec.op,
        param: null,
      });
    }
    // A call built by the generated functions checked these at its call
    // site; the same frozen object is not walked again on each page (L6).
    if (CHECKED.get(params) !== spec) checkParams(spec, params);
    // A public read is cached and shared, so it takes no refinement.
    if (spec.public === true && options.refine) {
      throw new OrdoLocalRefusal({
        class: 'invalid_refinement',
        message: `${spec.op} is a public op: a public read takes no $refine`,
        op: spec.op,
        param: null,
      });
    }
    // A refinement narrows a read. It is checked here as well as in the
    // builder, so a caller who hands `request` an envelope directly meets
    // the same refusal before the wire.
    const refine = options.refine ? encodeRefine(checkRefine(spec, options.refine, 'door')) : null;

    const headers = await this.#headers(spec, options);

    let url = `${this.#baseUrl}/op/${encodeURIComponent(spec.op)}`;
    let body: string | undefined;
    if (spec.method === 'POST') {
      headers['Content-Type'] = 'application/json';
      body = JSON.stringify(refine === null ? params : { ...params, $refine: JSON.parse(refine) as unknown });
    } else {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(params)) {
        if (v === null || v === undefined) continue;
        q.set(k, String(v));
      }
      // The router's GET form (`ordo-api/src/router.rs`): `$refine` is one
      // query key whose value is the envelope as JSON, URL-encoded.
      if (refine !== null) q.set('$refine', refine);
      const qs = q.toString();
      if (qs) url += `?${qs}`;
    }

    const init: RequestInit = { method: spec.method, headers };
    if (body !== undefined) init.body = body;
    if (options.signal) init.signal = options.signal;

    const response = await this.#send(url, init, 'op', options.onRetry);
    if (!response.ok) throw await refusalOf(response, spec.op, spec.refusals);
    return opResponseOf<R>(response, await readBody(response), spec.op, spec.version);
  }
}

/**
 * An op's answer, from the door's 2xx response and its body. `op` and
 * `version` stand in when the envelope leaves them out. Shared with
 * ordo-react's `DescriptorClient`.
 */
export function opResponseOf<R = Row>(response: Response, payload: unknown, op: string, version: string): OpResponse<R> {
  const envelope = (payload ?? {}) as {
    op?: string;
    version?: string;
    mode?: string;
    rows?: readonly R[];
    next?: string;
  };
  const out: {
    op: string;
    version: string;
    mode?: string;
    rows: readonly R[];
    next?: string;
    servedVersion: string | null;
    stale: boolean;
    replayed: boolean;
    pin: Pin | null;
  } = {
    op: envelope.op ?? op,
    version: envelope.version ?? version,
    // `rows` is always an array, even for one row (§3). Defending against
    // its absence keeps a malformed 200 from becoming a crash three
    // components away.
    rows: Array.isArray(envelope.rows) ? envelope.rows : [],
    servedVersion: response.headers.get('X-Ordo-Op-Version'),
    stale: response.headers.get('X-Ordo-Version-Skew') === 'stale',
    replayed: response.headers.get('X-Ordo-Idempotent-Replay') === 'true',
    pin: readPin(response.headers),
  };
  if (envelope.mode !== undefined) out.mode = envelope.mode;
  if (envelope.next !== undefined && envelope.next !== null) out.next = envelope.next;
  return out as OpResponse<R>;
}

/**
 * The pin a response carries, or `null`. A position is required and must be a
 * plain count this client can hold exactly: a replica orders rows by it, so a
 * position rounded to the nearest representable number would be worse than
 * none.
 */
function readPin(headers: Headers): Pin | null {
  const position = count(headers.get('X-Ordo-Position'));
  const head = headers.get('X-Ordo-Head');
  if (position === null || head === null) return null;
  return {
    position,
    head,
    keyId: headers.get('X-Ordo-Head-Key'),
    signature: headers.get('X-Ordo-Head-Sig'),
    verifiableFrom: count(headers.get('X-Ordo-Verifiable-From')),
  };
}

/** A non-negative integer the platform sent as text, if it is exactly one. */
function count(value: string | null): number | null {
  if (value === null || !/^\d+$/.test(value)) return null;
  const n = Number(value);
  return Number.isSafeInteger(n) ? n : null;
}

const MONEY = /^-?\d+(\.\d+)?$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

/**
 * Bind order, locally: unknown keys first, then each declared param.
 *
 * The same order the door uses (§5), and for the same reason — a typo
 * surfaces before "missing" noise. The refusals wear the door's own classes:
 * this is the door's check, run early, not a second contract.
 *
 * Called from two places — here, before a request, and from the `Call`
 * constructor, so a bad argument is refused **at the call site** rather than
 * at the terminal verb. One function, two call sites: a second
 * implementation would be the drift this repository spends its life
 * refusing, but a projection reached twice is only ever one answer.
 */
/** Parameter objects already checked against a spec, by {@link checkParamsOnce}. */
const CHECKED = new WeakMap<object, OpSpec>();

/**
 * Check `params` against `spec` and return a frozen copy that
 * {@link OrdoClient.request} knows is checked, so a call's pages do not
 * check it again (L6).
 */
export function checkParamsOnce(spec: OpSpec, params: Params): Params {
  checkParams(spec, params);
  const frozen = Object.freeze({ ...params });
  CHECKED.set(frozen, spec);
  return frozen;
}

export function checkParams(spec: OpSpec, params: Params): void {
  const declared = new Map(spec.params.map((p) => [p.name, p]));
  for (const key of Object.keys(params)) {
    if (!declared.has(key)) {
      throw new OrdoLocalRefusal({
        class: 'unknown_param',
        message: `${spec.op} has no parameter \`${key}\` (it declares ${
          spec.params.map((p) => p.name).join(', ') || 'none'
        })`,
        op: spec.op,
        param: key,
      });
    }
  }
  // CAS is unavoidable, not merely available (`ui-gen.md` §1). An op that
  // asserts a revision and is committed without one is refused HERE, so
  // there is no last-write-wins path through this package.
  //
  // Checked before the ordinary per-parameter loop, and the CLASS is
  // identical either way (`missing_param` — the door's own, since the rev is
  // a declared parameter like any other). Only the message differs, and it
  // has to: "renameTaskIf requires `rev` (int)" is true and tells a caller
  // nothing about why a stale rev is not an option.
  if (spec.casParam && (params[spec.casParam] === undefined || params[spec.casParam] === null)) {
    throw new OrdoLocalRefusal({
      class: 'missing_param',
      message:
        `${spec.op} asserts a revision: pass the \`${spec.casParam}\` you read with the row. ` +
        'A write without it would be last-write-wins, which this op is not.',
      op: spec.op,
      param: spec.casParam,
    });
  }
  for (const p of spec.params) {
    const v = params[p.name];
    if (v === undefined || v === null) {
      if (p.required) {
        throw new OrdoLocalRefusal({
          class: 'missing_param',
          message: `${spec.op} requires \`${p.name}\` (${p.type})`,
          op: spec.op,
          param: p.name,
        });
      }
      continue;
    }
    checkValue(spec, p, v);
  }
}

function checkValue(spec: OpSpec, p: ParamSpec, v: ParamValue): void {
  const bad = (cls: RefusalClass, message: string): never => {
    throw new OrdoLocalRefusal({ class: cls, message, op: spec.op, param: p.name });
  };
  if (p.enumValues && typeof v === 'string' && !p.enumValues.includes(v)) {
    bad('invalid_param', `${p.name} is one of ${p.enumValues.join(', ')}, not ${JSON.stringify(v)}`);
  }
  switch (p.type) {
    case 'int':
      if (typeof v !== 'number' || !Number.isInteger(v)) {
        bad('invalid_param', `${p.name} is \`int\`: a JSON integer`);
      }
      if (p.max !== null && (v as number) > Number(p.max)) bad('bound_violation', `${p.name} <= ${p.max}`);
      if (p.min !== null && (v as number) < Number(p.min)) bad('bound_violation', `${p.name} >= ${p.min}`);
      return;
    case 'money':
    case 'decimal':
      // Exact decimal string, never a float (§5). A number here is the bug
      // that silently rounds money, so it is refused rather than coerced.
      if (typeof v !== 'string' || !MONEY.test(v)) {
        bad('invalid_param', `${p.name} is \`${p.type}\`: an exact decimal string like "-12.50", never a float`);
      }
      // `money(2)` holds cents and `decimal(4)` four places: a finer value
      // would have to round to be stored, and the door refuses it rather
      // than rounding (27ak). Trailing zeros name no value, so "29.90" fits
      // `money(1)`.
      if (p.places !== undefined && p.places !== null) {
        const frac = (v as string).split('.')[1] ?? '';
        if (frac.replace(/0+$/, '').length > p.places) {
          bad('invalid_param', `${p.name} is \`${p.type}(${p.places})\`: at most ${p.places} places`);
        }
      }
      if (p.max !== null && compareDecimal(v as string, p.max) > 0) {
        bad('bound_violation', `${p.name} <= ${p.max}`);
      }
      if (p.min !== null && compareDecimal(v as string, p.min) < 0) {
        bad('bound_violation', `${p.name} >= ${p.min}`);
      }
      return;
    case 'date':
      if (typeof v !== 'string' || !DATE.test(v)) {
        bad('invalid_param', `${p.name} is \`date\`: "YYYY-MM-DD"`);
      }
      // Lexicographic IS chronological for this format, which is why §5 says
      // to compare it that way rather than parsing a Date.
      if (p.max !== null && (v as string) > p.max) bad('bound_violation', `${p.name} <= ${p.max}`);
      if (p.min !== null && (v as string) < p.min) bad('bound_violation', `${p.name} >= ${p.min}`);
      return;
    case 'bool':
      if (typeof v !== 'boolean') bad('invalid_param', `${p.name} is \`bool\``);
      return;
    case 'text':
      if (typeof v !== 'string') bad('invalid_param', `${p.name} is \`text\``);
      // A `text` bound can only ever be a character count: `<=` on `text` is
      // refused by the language, so `max` here is the `text(n)` length. It
      // counts characters (code points), as the door's `chars().count()` and
      // Postgres's `char_length` do; `.length` would count UTF-16 units and
      // refuse eight emoji against `text(8)` that the door accepts.
      if (p.max !== null && Array.from(v as string).length > Number(p.max)) {
        bad('bound_violation', `${p.name} is at most ${p.max} characters`);
      }
      return;
    case 'id':
      if (typeof v !== 'string') bad('invalid_param', `${p.name} is an \`id\` string`);
      return;
    case 'json':
      // Any JSON value, sent as JSON (28b). What is refused is a value the
      // wire would change: a non-finite number, a class instance, or
      // `undefined` inside. The size and depth limits are the deployment's,
      // and the door checks them.
      if (!isJsonValue(v)) {
        bad('invalid_param', `${p.name} is \`json\`: a JSON value (plain objects, arrays, finite numbers)`);
      }
      return;
    default:
      // `datetime` and `duration` reach the door as validated strings and
      // take no bounds in v1; anything else is a type this client was
      // generated before. Neither is a reason to refuse locally — the door
      // is the authority, and a client that refused what the door accepts
      // would be the drift the version pin exists to catch.
      return;
  }
}

/** Exact-decimal comparison, the way the door does it (§5). No floats. */
export function compareDecimal(a: string, b: string): number {
  const parse = (s: string): [bigint, number] => {
    const [int, frac = ''] = s.split('.');
    return [BigInt((int ?? '0') + frac), frac.length];
  };
  const [av, ap] = parse(a);
  const [bv, bp] = parse(b);
  const scale = Math.max(ap, bp);
  const lift = (v: bigint, p: number): bigint => v * 10n ** BigInt(scale - p);
  const x = lift(av, ap);
  const y = lift(bv, bp);
  return x < y ? -1 : x > y ? 1 : 0;
}

/**
 * A sync's refinement as its `$refine` value, or `null` when it narrows
 * nothing. A sync takes `where` and refuses `pick` (ruled 2026-10-05,
 * `refinement-default` ask 2), with the door's class and words.
 */
export function syncRefine(spec: OpSpec, refine: Refine | undefined): string | null {
  if (!refine) return null;
  if (refine.pick !== undefined) {
    throw new OrdoLocalRefusal({
      class: 'invalid_refinement',
      message:
        'a sync takes a refinement\'s `where` and refuses its `pick`: the device stores whole rows keyed ' +
        'by id, so a picked row would overwrite a whole one. Sync with `where` alone and pick on the device',
      op: spec.op,
      param: 'pick',
    });
  }
  return encodeRefine(checkRefine(spec, refine, 'door'));
}
