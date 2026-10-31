/**
 * A person's own tenant, made after they sign up.
 *
 * On a deployment that gives each person a tenant of their own
 * (`ORDO_PROVISION_SELF_SERVE=on-signup`), the sign-in call answers at once
 * and the tenant is made after it (owner, 2026-10-21: "The process of
 * provisioning should not be a waiting call."). The sign-in's answer then
 * carries `provisioning: "in progress"` and no `space`. These functions read
 * where it stands, wait for it, and start it again after a failure.
 *
 * | Function | Route | Yields |
 * |---|---|---|
 * | `tenantStatus` | `GET /auth/provisioning` | a `TenantStatus` |
 * | `retryTenant` | `POST /auth/provisioning` | a `TenantStatus` |
 * | `waitForTenant` | `GET /auth/provisioning`, until ready or a time limit | the tenant's space id |
 * | `watchTenant` | `GET /auth/provisioning`, until ready, with no time limit | a `TenantWatch` |
 *
 * `waitForTenant` is the one a sign-up screen needs. It reads the status
 * until the tenant is ready, tells `onProgress` each step, and rejects with a
 * `TenantError` that says why when it cannot finish.
 *
 * `watchTenant` is the tenant status hook (owner, 2026-10-21). It calls the
 * app developer's `onStatus` each time the status changes and never gives up
 * while the tenant is still being set up, because the server never does. A
 * setup that takes longer than the deployment's `ORDO_PROVISION_STUCK_SECS`
 * comes through as `stuck: true` with a `reason` for the person. What to show
 * is the app developer's choice.
 */

import { reconnectDelayMs } from '../core/busy.js';
import type { TokenSource } from '../core/client.js';
import { OrdoRefusal, failureDetail } from '../core/refusal.js';
import { PasskeyError, type PasskeyEndpoint, type Wire } from './passkey.js';
import { authRequest } from './request.js';

/** Where a person's own tenant stands. */
export type TenantState =
  /** The deployment gives nobody a tenant of their own. `setting` names the switch. */
  | 'off'
  /** None was started. `retryTenant` starts one. */
  | 'none'
  /** Being made. `step` says how far it got. */
  | 'provisioning'
  /** Made, and the door serves the person in it. `space` is its id. */
  | 'ready'
  /** It stopped. `reason` is a sentence for the person. `retryTenant` starts again. */
  | 'failed';

export interface TenantStatus {
  readonly state: TenantState;
  /** The tenant's id, once a tenant exists. */
  readonly space?: string;
  /**
   * While `provisioning`: the last step done (`reserved`, `targeted`,
   * `custody`, `database`, `attached`, `founded`), or `serving` while the door
   * takes it in.
   */
  readonly step?: string;
  /** When `failed`: why, in words for the person. */
  readonly reason?: string;
  /**
   * When `off`: the deployment setting that turns it on. When `stuck`:
   * `ORDO_PROVISION_STUCK_SECS`, the setting that decides when.
   */
  readonly setting?: string;
  /**
   * While `provisioning`: true once setting up has taken longer than the
   * deployment's `ORDO_PROVISION_STUCK_SECS`. The server keeps trying, and
   * `reason` says so in words for the person.
   */
  readonly stuck?: boolean;
  /** While `provisioning`: how many tries at setting it up failed so far. */
  readonly attempts?: number;
}

export type TenantFailure =
  /** The tenant could not be made. `reason` says why; `retryTenant` starts again. */
  | 'failed'
  /** The deployment gives nobody a tenant of their own. */
  | 'off'
  /** No tenant was started for this person. `retryTenant` starts one. */
  | 'none'
  /** `waitForTenant` waited `timeoutMs` and the tenant was not ready. */
  | 'timeout'
  /** The token was refused: the person is signed out. */
  | 'refused'
  /** Too many tenants were made this hour. Try again later. */
  | 'limit_reached'
  /** The caller's signal stopped the wait. */
  | 'aborted';

/** Why the person's own tenant is not there. An {@link OrdoRefusal}, with `failure` to switch on. */
export class TenantError extends OrdoRefusal<string> {
  readonly failure: TenantFailure;
  /** When `failed`: the reason the door gave, for the person. */
  readonly reason?: string;
  /** The tenant's id, when one exists. */
  readonly space?: string;
  constructor(
    failure: TenantFailure,
    message: string,
    detail: { reason?: string; space?: string; status?: number; code?: string } = {},
  ) {
    super(failureDetail(failure, message, { status: detail.status, code: detail.code }), null, { message });
    this.name = 'TenantError';
    this.failure = failure;
    if (detail.reason !== undefined) this.reason = detail.reason;
    if (detail.space !== undefined) this.space = detail.space;
  }
}

/** A token, or a function that gives the current one (a session renews it). */
async function tokenOf(token: TokenSource): Promise<string> {
  return typeof token === 'string' ? token : token();
}

function statusOf(w: Wire): TenantStatus {
  const text = (k: string) => (typeof w[k] === 'string' ? (w[k] as string) : undefined);
  const state = text('state');
  const known: readonly string[] = ['off', 'none', 'provisioning', 'ready', 'failed'];
  if (state === undefined || !known.includes(state)) {
    throw new PasskeyError('malformed', 'the door answered a provisioning status this client cannot read');
  }
  const out: { -readonly [K in keyof TenantStatus]: TenantStatus[K] } = { state: state as TenantState };
  for (const k of ['space', 'step', 'reason', 'setting'] as const) {
    const v = text(k);
    if (v !== undefined) out[k] = v;
  }
  if (typeof w['stuck'] === 'boolean') out.stuck = w['stuck'];
  if (typeof w['attempts'] === 'number' && Number.isFinite(w['attempts'])) out.attempts = w['attempts'];
  return out;
}

/** One call to `/auth/provisioning`. */
async function door(endpoint: PasskeyEndpoint, token: TokenSource, method: 'GET' | 'POST'): Promise<TenantStatus> {
  const init: RequestInit = {
    method,
    headers: { Accept: 'application/json', Authorization: `Bearer ${await tokenOf(token)}` },
  };
  const response = await authRequest(endpoint, '/auth/provisioning', init);
  if (response.status === 429) {
    throw new TenantError('limit_reached', 'too many accounts were set up this hour; try again later');
  }
  if (response.status === 401 || response.status === 403) {
    throw new TenantError('refused', 'the session was refused; sign in again');
  }
  if (response.status === 503) {
    throw new PasskeyError('unavailable', 'the door cannot read the tenant registry right now');
  }
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // Read below as malformed.
  }
  if (!response.ok || json === null || typeof json !== 'object') {
    throw new PasskeyError('malformed', 'the door answered a provisioning status this client cannot read');
  }
  return statusOf(json as Wire);
}

/** Where the signed-in person's own tenant stands. */
export async function tenantStatus(endpoint: PasskeyEndpoint, token: TokenSource): Promise<TenantStatus> {
  return door(endpoint, token, 'GET');
}

/**
 * Start the person's own tenant again after it failed, or for the first time.
 * A tenant already made or being made is left alone. Answers at once.
 */
export async function retryTenant(endpoint: PasskeyEndpoint, token: TokenSource): Promise<TenantStatus> {
  return door(endpoint, token, 'POST');
}

export interface WaitForTenantOptions {
  /** Called with each status read while waiting, for a progress display. */
  readonly onProgress?: (status: TenantStatus) => void;
  /** Stops the wait. It rejects with `aborted`. */
  readonly signal?: AbortSignal;
  /** How long to wait in all, in milliseconds. Default {@link DEFAULT_TENANT_WAIT_MS}. */
  readonly timeoutMs?: number;
  /** For tests: how to pause between reads. */
  readonly wait?: (ms: number) => Promise<void>;
}

/** How long `waitForTenant` waits in all: two minutes. */
export const DEFAULT_TENANT_WAIT_MS = 120_000;

/** The pauses between reads: quick at first, then every two seconds. */
const PAUSES_MS = [250, 250, 500, 500, 1_000, 1_000, 2_000];

/**
 * Wait until the person's own tenant is ready, and give its space id.
 *
 * Reads `GET /auth/provisioning` with short pauses that grow to two seconds.
 * A read that fails because the door is busy or slow is tried again until
 * `timeoutMs`. Rejects with a {@link TenantError}: `failed` (with the
 * reason), `off`, `none`, `refused`, `timeout` or `aborted`.
 */
export async function waitForTenant(
  endpoint: PasskeyEndpoint,
  token: TokenSource,
  options: WaitForTenantOptions = {},
): Promise<string> {
  const wait = options.wait ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const deadline = Date.now() + (options.timeoutMs ?? DEFAULT_TENANT_WAIT_MS);
  let last: TenantStatus | null = null;
  for (let i = 0; ; i++) {
    if (options.signal?.aborted) throw new TenantError('aborted', 'the wait for the tenant was stopped');
    let status: TenantStatus | null = null;
    try {
      status = await tenantStatus(endpoint, token);
    } catch (e) {
      // Busy, slow or unreachable for a moment: read again. A refused session
      // or an answer this client cannot read will not change by waiting.
      if (e instanceof TenantError) throw e;
      if (e instanceof PasskeyError && e.failure === 'malformed') throw e;
    }
    if (status !== null) {
      last = status;
      options.onProgress?.(status);
      switch (status.state) {
        case 'ready':
          if (status.space) return status.space;
          break;
        case 'failed': {
          const detail: { reason?: string; space?: string } = {};
          if (status.reason !== undefined) detail.reason = status.reason;
          if (status.space !== undefined) detail.space = status.space;
          throw new TenantError('failed', status.reason ?? 'the tenant could not be made', detail);
        }
        case 'off':
          throw new TenantError(
            'off',
            `this deployment gives nobody a tenant of their own (${status.setting ?? 'ORDO_PROVISION_SELF_SERVE'})`,
          );
        case 'none':
          throw new TenantError('none', 'no tenant was started for this person; retryTenant starts one');
        case 'provisioning':
          break;
      }
    }
    const pause = PAUSES_MS[Math.min(i, PAUSES_MS.length - 1)] as number;
    if (Date.now() + pause > deadline) {
      const step = last?.step ? ` (last step: ${last.step})` : '';
      throw new TenantError('timeout', `the tenant was not ready in time${step}`, last?.space ? { space: last.space } : {});
    }
    await wait(pause);
  }
}

export interface WatchTenantOptions {
  /** Told why a read failed (busy, slow, unreachable). The watch keeps reading. */
  readonly onError?: (error: unknown) => void;
  /** Stops the watch, like `stop()`. `ready` rejects with `aborted`. */
  readonly signal?: AbortSignal;
  /**
   * The pause between reads while the setup is stuck, in milliseconds.
   * Default {@link DEFAULT_STUCK_PAUSE_MS}. Reads before that use the same
   * quick pauses as `waitForTenant`.
   */
  readonly stuckPauseMs?: number;
  /** For tests: how to pause between reads. */
  readonly wait?: (ms: number) => Promise<void>;
}

/** The pause between reads once a setup is reported stuck: ten seconds. */
export const DEFAULT_STUCK_PAUSE_MS = 10_000;

/** The longest pause after reads that failed in a row: thirty seconds. */
const MAX_ERROR_PAUSE_MS = 30_000;

/** A running watch on the person's own tenant. */
export interface TenantWatch {
  /**
   * The tenant's space id once it is ready. Rejects with a `TenantError`
   * when it cannot be: `failed`, `off`, `none`, `refused`, `limit_reached`,
   * or `aborted` after `stop()`. Never `timeout`.
   */
  readonly ready: Promise<string>;
  /** Stop reading. */
  stop(): void;
}

function sameStatus(a: TenantStatus | null, b: TenantStatus): boolean {
  return (
    a !== null &&
    a.state === b.state &&
    a.space === b.space &&
    a.step === b.step &&
    a.stuck === b.stuck &&
    a.attempts === b.attempts &&
    a.reason === b.reason
  );
}

/**
 * **The tenant status hook.** Read `GET /auth/provisioning` until the
 * person's own tenant is ready, and call `onStatus` each time the status
 * changes. There is no time limit: the server keeps trying a setup until it
 * is made (owner, 2026-10-21), so a watch waits as long. A read that fails
 * for a moment is told to `onError` and read again after a growing pause.
 *
 * ```ts
 * const watch = watchTenant(endpoint, session.token, (s) => {
 *   if (s.stuck) show(s.reason);
 *   else if (s.state === 'provisioning') show('Setting up your account');
 * });
 * const space = await watch.ready;
 * ```
 */
export function watchTenant(
  endpoint: PasskeyEndpoint,
  token: TokenSource,
  onStatus: (status: TenantStatus) => void,
  options: WatchTenantOptions = {},
): TenantWatch {
  const wait = options.wait ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const stuckPause = options.stuckPauseMs ?? DEFAULT_STUCK_PAUSE_MS;
  let stopped = false;
  const stopping = (): boolean => stopped || options.signal?.aborted === true;
  const aborted = () => new TenantError('aborted', 'the watch on the tenant was stopped');
  const ready = (async (): Promise<string> => {
    let last: TenantStatus | null = null;
    let errors = 0;
    for (let i = 0; ; i++) {
      if (stopping()) throw aborted();
      let status: TenantStatus | null = null;
      try {
        status = await tenantStatus(endpoint, token);
        errors = 0;
      } catch (e) {
        if (e instanceof TenantError) throw e;
        if (e instanceof PasskeyError && e.failure === 'malformed') throw e;
        errors++;
        options.onError?.(e);
      }
      if (stopping()) throw aborted();
      if (status !== null) {
        if (!sameStatus(last, status)) onStatus(status);
        last = status;
        switch (status.state) {
          case 'ready':
            if (status.space) return status.space;
            break;
          case 'failed': {
            const detail: { reason?: string; space?: string } = {};
            if (status.reason !== undefined) detail.reason = status.reason;
            if (status.space !== undefined) detail.space = status.space;
            throw new TenantError('failed', status.reason ?? 'the tenant could not be made', detail);
          }
          case 'off':
            throw new TenantError(
              'off',
              `this deployment gives nobody a tenant of their own (${status.setting ?? 'ORDO_PROVISION_SELF_SERVE'})`,
            );
          case 'none':
            throw new TenantError('none', 'no tenant was started for this person; retryTenant starts one');
          case 'provisioning':
            break;
        }
      }
      let pause = PAUSES_MS[Math.min(i, PAUSES_MS.length - 1)] as number;
      if (last?.stuck) pause = Math.max(pause, stuckPause);
      // Reads that failed in a row back off by the one reconnect backoff.
      if (errors > 0) pause = Math.max(pause, reconnectDelayMs(errors, null, { firstDelayMs: 1_000, maxDelayMs: MAX_ERROR_PAUSE_MS }));
      await wait(pause);
    }
  })();
  // A watch nobody awaits must not report an unhandled rejection when it is
  // stopped; whoever awaits `ready` still sees it.
  ready.catch(() => {});
  return {
    ready,
    stop() {
      stopped = true;
    },
  };
}
