/**
 * A person's own tenant while it is being set up (owner, 2026-10-21).
 *
 * On a deployment that gives each person a tenant of their own
 * (`ORDO_PROVISION_SELF_SERVE=on-signup`), sign-up answers at once and the
 * tenant is made after it. Nothing can be written to it until it is ready:
 * the door answers `tenant_not_ready` (503, `Retry-After`). This module reads
 * `GET /auth/provisioning` and starts a setup again with
 * `POST /auth/provisioning`. `useTenantStatus` and `TenantGate` are built on
 * it.
 *
 * Its calls go through the SDK's sign-in request helper (`authRequest`), so
 * a busy door is asked again and a call that got no answer is named. It
 * needs no `DescriptorClient`, because the client names a space and the
 * space is what this is waiting for.
 */

import type { TokenSource } from './client.js';
import { authRequest } from '@hominemai/ordo-sdk/auth/request.js';
import { reconnectDelayMs, type BusyRetryOptions } from '@hominemai/ordo-sdk/core/busy.js';
import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import {
  OrdoRefusal,
  failureDetail,
  isErrorEnvelope,
  type WireAnswer,
} from '@hominemai/ordo-sdk/core/refusal.js';

/** Where a person's own tenant stands. */
// The status shape and its pause are the SDK's (owner ruling, 2026-10-23:
// ordo-react imports ordo-sdk's code and carries no copy).
import { DEFAULT_STUCK_PAUSE_MS } from '@hominemai/ordo-sdk/auth/provisioning.js';
import type { TenantState, TenantStatus } from '@hominemai/ordo-sdk/auth/provisioning.js';
export { DEFAULT_STUCK_PAUSE_MS };
export type { TenantState, TenantStatus };

/** Why the status could not be read, or why waiting stopped. */
export type TenantProblem =
  /** The session was refused: sign in again. */
  | 'refused'
  /** Too many tenants were made this hour. */
  | 'limit_reached'
  /** The door answered something this package cannot read. */
  | 'malformed'
  /** Busy, slow or unreachable for a moment. Reading goes on. */
  | 'unavailable';

/**
 * Why the status could not be read. An `OrdoRefusal` (the SDK's one error
 * shape): `code` and `status` are the door's when it answered, `cause` is
 * what failed, `problem` says what to do.
 */
export class TenantStatusError extends OrdoRefusal<string> {
  readonly problem: TenantProblem;
  constructor(problem: TenantProblem, message: string, wire: WireAnswer = {}, cause?: unknown) {
    super(failureDetail(problem, message, wire), cause instanceof OrdoRefusal ? cause.retryAfterMs : null, {
      message,
      cause,
    });
    this.name = 'TenantStatusError';
    this.problem = problem;
  }
}

/** Where the door is, and who is asking. */
export interface TenantEndpoint {
  /** The door's base URL. `/auth/provisioning` is appended. */
  readonly baseUrl: string;
  /** The person's token, or a function that gives the current one. */
  readonly token: TokenSource;
  /** Injectable for tests; defaults to the global. */
  readonly fetch?: typeof globalThis.fetch;
  /** How long each read waits for the door. As the SDK's sign-in calls. */
  readonly callTimeoutMs?: number;
  /** How a read is sent again on `server_busy`. As the SDK's sign-in calls. */
  readonly busyRetry?: BusyRetryOptions;
}

const STATES: readonly string[] = ['off', 'none', 'provisioning', 'ready', 'failed'];

function statusOf(w: Record<string, unknown>): TenantStatus {
  const state = w['state'];
  if (typeof state !== 'string' || !STATES.includes(state)) {
    throw new TenantStatusError('malformed', 'the door answered a provisioning status this client cannot read');
  }
  const out: { -readonly [K in keyof TenantStatus]: TenantStatus[K] } = { state: state as TenantState };
  for (const k of ['space', 'step', 'reason', 'setting'] as const) {
    const v = w[k];
    if (typeof v === 'string') out[k] = v;
  }
  if (typeof w['stuck'] === 'boolean') out.stuck = w['stuck'];
  if (typeof w['attempts'] === 'number' && Number.isFinite(w['attempts'])) out.attempts = w['attempts'];
  return out;
}

async function call(endpoint: TenantEndpoint, method: 'GET' | 'POST'): Promise<TenantStatus> {
  const token = typeof endpoint.token === 'string' ? endpoint.token : await endpoint.token();
  let response: Response;
  try {
    // The SDK's one request helper for sign-in calls: a deadline, busy sent
    // again under `busyRetry`, a call that got no answer named.
    response = await authRequest(endpoint, '/auth/provisioning', {
      method,
      headers: { Accept: 'application/json', Authorization: `Bearer ${token}` },
    });
  } catch (e) {
    // Busy past its retries, or no answer: reading goes on.
    if (e instanceof OrdoRefusal || e instanceof OrdoNetworkError) {
      throw new TenantStatusError('unavailable', 'the door could not be reached', {}, e);
    }
    throw e;
  }
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // Read below as malformed.
  }
  const wire = { status: response.status, code: codeOf(json) };
  if (response.status === 401 || response.status === 403) {
    throw new TenantStatusError('refused', 'the session was refused; sign in again', wire);
  }
  if (response.status === 429) {
    throw new TenantStatusError('limit_reached', 'too many accounts were set up this hour; try again later', wire);
  }
  if (response.status >= 500) {
    throw new TenantStatusError('unavailable', 'the door cannot read the tenant registry right now', wire);
  }
  if (!response.ok || json === null || typeof json !== 'object') {
    throw new TenantStatusError('malformed', 'the door answered a provisioning status this client cannot read', wire);
  }
  return statusOf(json as Record<string, unknown>);
}

/** The envelope's `code`, when the body is an Ordo error envelope. */
function codeOf(json: unknown): string | null {
  return isErrorEnvelope(json) ? json.error.code : null;
}

/** Where the signed-in person's own tenant stands. */
export function readTenantStatus(endpoint: TenantEndpoint): Promise<TenantStatus> {
  return call(endpoint, 'GET');
}

/** Start the person's own tenant again after it failed, or for the first time. */
export function retryTenantSetup(endpoint: TenantEndpoint): Promise<TenantStatus> {
  return call(endpoint, 'POST');
}

/** The pauses between reads while a setup is under way. */
export const TENANT_PAUSES_MS: readonly number[] = [250, 250, 500, 500, 1_000, 1_000, 2_000];
/** The pause between reads once a setup is reported stuck. */
/** The longest pause after reads that failed in a row. */
const MAX_ERROR_PAUSE_MS = 30_000;

/** The pause before read number `i` (from 0). */
export function tenantPause(i: number, stuck: boolean, failedReads: number, stuckPauseMs: number): number {
  let pause = TENANT_PAUSES_MS[Math.min(i, TENANT_PAUSES_MS.length - 1)] as number;
  if (stuck) pause = Math.max(pause, stuckPauseMs);
  // Reads that failed in a row back off by the SDK's one reconnect backoff.
  if (failedReads > 0) {
    pause = Math.max(pause, reconnectDelayMs(failedReads, null, { firstDelayMs: 1_000, maxDelayMs: MAX_ERROR_PAUSE_MS }));
  }
  return pause;
}
