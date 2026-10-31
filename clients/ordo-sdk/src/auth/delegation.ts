/**
 * The delegation consent ceremony: an agent asks a person for a narrow,
 * short-lived token, and the person reads exactly what it would let the agent
 * do before saying yes.
 *
 * ## The agent's half (no token, `fetch` to the auth routes)
 *
 * | Function | Route | What it does |
 * |---|---|---|
 * | `requestDelegation` | `POST /auth/delegation/request` | asks a person for a set of ops; answers a request id and a binder |
 * | `redeemDelegation` | `POST /auth/delegation/redeem` | one try at collecting the token: granted, or still waiting |
 * | `waitForDelegation` | `POST /auth/delegation/redeem` | polls `redeemDelegation` until the person answers |
 *
 * The **binder** is a secret that only the asking agent holds. The request id
 * can be shown to the person; the token is only ever paid out to whoever
 * presents both. Keep the binder out of any URL the person sees.
 *
 * ## The person's half (platform ops through the op door)
 *
 * | Function | Op | What it does |
 * |---|---|---|
 * | `reviewDelegation` | `std.delegation.review` (GET) | the disclosure: what the agent could read, write and delete |
 * | `approveDelegation` | `std.delegation.approve` (POST) | grants it |
 * | `declineDelegation` | `std.delegation.decline` (POST) | refuses it |
 * | `myDelegations` | `std.delegation.mine` (GET) | the person's own list, waiting and live and ended |
 * | `revokeDelegation` | `std.delegation.revoke` (POST) | ends a live delegation now |
 *
 * These take an {@link OrdoClient} bound to the tenant, signed in with the
 * person's session token. Approving needs a fresh passkey touch unless the app
 * developer exempted it, so call `stepUp` (see `passkey.ts`) and build the
 * client from the re-minted session before `approveDelegation`. Without it the
 * door answers `access_denied`, which arrives as an `OrdoRefusal`. Nothing
 * here forces a passkey.
 */

import { OrdoClient, type Params } from '../core/client.js';
import type { BusyRetryOptions } from '../core/busy.js';
import type { NetworkOptions } from '../core/network.js';
import { OrdoRefusal, failureDetail } from '../core/refusal.js';
import { authRequest } from './request.js';
import type { OpSpec } from '../core/spec.js';

/** Why the door refused, or the request could not be made. */
export type DelegationFailure =
  /** The deployment's settings do not allow this request. `message` names the setting. */
  | 'refused'
  /** Too many requests are waiting for this agent or this person. `message` names the setting. */
  | 'limit'
  /** The deployment runs a human check and the request's `human` token was missing or refused. */
  | 'human_check'
  /** The person said no. */
  | 'declined'
  /** The request ran out of time. */
  | 'expired'
  /** The token was already collected. */
  | 'redeemed'
  /** Unknown request, or the wrong binder. The door gives one answer for both. */
  | 'not_found'
  /** The request was malformed. */
  | 'invalid_request'
  /** The deployment cannot run delegations right now. */
  | 'unavailable'
  /** The door answered something unreadable. A call that got no answer throws `OrdoNetworkError`. */
  | 'network'
  /** `waitForDelegation` gave up at its `timeoutMs`, or the request expired. */
  | 'timeout'
  /** `waitForDelegation` was aborted by the caller. */
  | 'aborted'
  | 'unknown';

/** A refusal in the delegation ceremony. */
/**
 * An {@link OrdoRefusal}: `code` is the door's own error code when it sent
 * one (`delegation_refused`, `delegation_declined`, ...), else the failure;
 * `status` is the HTTP status, 0 when the door did not answer.
 */
export class DelegationError extends OrdoRefusal<string> {
  readonly failure: DelegationFailure;
  constructor(failure: DelegationFailure, message: string, code: string | null = null, status: number | null = null) {
    super(failureDetail(failure, message, { status: status ?? 0, code }), null, { message });
    this.name = 'DelegationError';
    this.failure = failure;
  }
}

/** Where the door is. Same shape as the other auth endpoints. */
export interface DelegationEndpoint {
  /** Door base URL. `/auth/...` is appended. */
  readonly baseUrl: string;
  /** For tests and for runtimes without a global `fetch`. */
  readonly fetch?: typeof globalThis.fetch;
  /** How long one call waits for the door, in milliseconds. Default 15 000. */
  readonly callTimeoutMs?: number;
  /** How a call that got no answer is named (`OrdoNetworkError`). */
  readonly network?: NetworkOptions;
  /** How a call is sent again on `server_busy`. As `PasskeyEndpoint.busyRetry`. */
  readonly busyRetry?: BusyRetryOptions;
}

/** What an agent asks for. */
export interface DelegationRequest {
  /** The tenant the ops are in. */
  readonly space: string;
  /** The agent's id. */
  readonly agent: string;
  /** The person's member id. */
  readonly person: string;
  /** The ops the agent asks to call. */
  readonly ops: readonly string[];
  /** How long the delegation should last once approved. The deployment caps it. */
  readonly hours?: number;
  /**
   * The human check's token (a Turnstile token), when the deployment runs one
   * (`ORDO_AUTH_HUMAN_CHECK`). `GET /auth/methods` says whether it does. The
   * person's page solves the check and hands the token to the agent.
   */
  readonly human?: string;
}

/** A request the door took. */
export interface RequestedDelegation {
  /** The request id (`dr_...`). Safe to show the person. */
  readonly request: string;
  /** The secret that collects the token. Never leaves the agent. */
  readonly binder: string;
  /** When the request stops being answerable, seconds since the epoch. */
  readonly expiresAt: number;
  /** The op the person calls to review it. */
  readonly review: string;
}

/** The request id and binder that identify one request. */
export interface DelegationClaim {
  readonly request: string;
  readonly binder: string;
}

/** The token a person's approval paid out. */
export interface GrantedDelegation {
  readonly status: 'granted';
  readonly token: string;
  readonly expiresAt: number;
  readonly delegation: string;
  readonly space: string;
  readonly ops: readonly string[];
}

/** One try at collecting the token. */
export type DelegationRedemption =
  | GrantedDelegation
  | { readonly status: 'waiting'; readonly expiresAt: number };

/** Options for {@link waitForDelegation}. */
export interface WaitOptions {
  /** Milliseconds between tries. Default 2000. */
  readonly intervalMs?: number;
  /** Stop waiting. The wait rejects with a `DelegationError` of failure `aborted`. */
  readonly signal?: AbortSignal;
  /** Give up after this long. The wait rejects with failure `timeout`. */
  readonly timeoutMs?: number;
  /**
   * For tests: the clock, in milliseconds since the epoch. Default `Date.now`.
   * Without `timeoutMs` the wait ends at the delegation's `expiresAt`.
   */
  readonly now?: () => number;
  /** Called after each try that found the person had not answered. */
  readonly onWaiting?: (state: { readonly expiresAt: number }) => void;
  /** For tests: replaces the timer between tries. */
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
}

type Wire = Record<string, unknown>;

function errorOf(json: unknown): { code: string | null; message: string | null } {
  if (json === null || typeof json !== 'object') return { code: null, message: null };
  const e = (json as Wire)['error'];
  if (e === null || typeof e !== 'object') return { code: null, message: null };
  const c = (e as Wire)['code'];
  const m = (e as Wire)['message'];
  return { code: typeof c === 'string' ? c : null, message: typeof m === 'string' ? m : null };
}

function failureOf(status: number, code: string | null): DelegationFailure {
  switch (code) {
    case 'delegation_refused':
      return 'refused';
    case 'delegation_limit':
      return 'limit';
    case 'authentication_failed':
      return 'human_check';
    case 'delegation_declined':
      return 'declined';
    case 'delegation_expired':
      return 'expired';
    case 'delegation_redeemed':
      return 'redeemed';
    case 'not_found':
      return 'not_found';
    case 'invalid_request':
      return 'invalid_request';
    case 'auth_unconfigured':
    case 'delegation_store_unavailable':
      return 'unavailable';
  }
  if (status === 404) return 'not_found';
  if (status === 410) return 'expired';
  if (status === 503) return 'unavailable';
  return 'unknown';
}

async function post(
  endpoint: DelegationEndpoint,
  path: string,
  body: Wire,
  signal?: AbortSignal,
): Promise<{ status: number; json: Wire }> {
  const init: RequestInit = {
    method: 'POST',
    headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  };
  let response: Response;
  try {
    response = await authRequest(endpoint, path, init, signal ? { signal } : {});
  } catch (e) {
    if (signal?.aborted) throw new DelegationError('aborted', 'the wait was aborted');
    throw e;
  }
  let json: unknown = null;
  try {
    json = await response.json();
  } catch {
    // An empty body: nothing to read.
  }
  if (!response.ok) {
    const { code, message } = errorOf(json);
    throw new DelegationError(
      failureOf(response.status, code),
      message ?? 'the door refused that delegation call',
      code,
      response.status,
    );
  }
  if (json === null || typeof json !== 'object') {
    throw new DelegationError('network', 'the door answered with something that is not JSON', null, response.status);
  }
  return { status: response.status, json: json as Wire };
}

function str(o: Wire, key: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new DelegationError('network', `the door's answer has no \`${key}\``);
  return v;
}

function numOf(o: Wire, key: string): number {
  const v = o[key];
  if (typeof v !== 'number') throw new DelegationError('network', `the door's answer has no \`${key}\``);
  return v;
}

function strings(o: Wire, key: string): string[] {
  const v = o[key];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

/**
 * Ask a person for a delegation. Needs no token. The deployment's settings
 * decide whether the agent may ask, for how long and for how many ops; a
 * refusal is a {@link DelegationError} of failure `refused` whose `message`
 * names the setting.
 */
export async function requestDelegation(
  endpoint: DelegationEndpoint,
  request: DelegationRequest,
  options: { readonly signal?: AbortSignal } = {},
): Promise<RequestedDelegation> {
  const body: Wire = {
    space: request.space,
    agent: request.agent,
    person: request.person,
    ops: [...request.ops],
  };
  if (request.hours !== undefined) body['hours'] = request.hours;
  if (request.human !== undefined) body['human'] = request.human;
  const { json } = await post(endpoint, '/auth/delegation/request', body, options.signal);
  return {
    request: str(json, 'request'),
    binder: str(json, 'binder'),
    expiresAt: numOf(json, 'expiresAt'),
    review: str(json, 'review'),
  };
}

/**
 * One try at collecting the token. Answers `granted` with the token, or
 * `waiting` while the person has not answered. A final refusal throws: failure
 * `declined`, `expired`, `redeemed` or `not_found`. An unknown request and a
 * wrong binder are one answer, `not_found`.
 */
export async function redeemDelegation(
  endpoint: DelegationEndpoint,
  claim: DelegationClaim,
  options: { readonly signal?: AbortSignal } = {},
): Promise<DelegationRedemption> {
  const { status, json } = await post(
    endpoint,
    '/auth/delegation/redeem',
    { request: claim.request, binder: claim.binder },
    options.signal,
  );
  if (status === 202 || json['status'] === 'waiting') {
    return { status: 'waiting', expiresAt: numOf(json, 'expiresAt') };
  }
  return {
    status: 'granted',
    token: str(json, 'token'),
    expiresAt: numOf(json, 'expiresAt'),
    delegation: str(json, 'delegation'),
    space: str(json, 'space'),
    ops: strings(json, 'ops'),
  };
}

function defaultSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new DelegationError('aborted', 'the wait was aborted'));
      return;
    }
    const onAbort = () => {
      clearTimeout(t);
      reject(new DelegationError('aborted', 'the wait was aborted'));
    };
    const t = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * Poll {@link redeemDelegation} until the person answers. Resolves with the
 * granted token. Rejects with a {@link DelegationError} on a final refusal
 * (`declined`, `expired`, `redeemed`, `not_found`), on `timeout` and on
 * `aborted`. A call that gets no answer rejects at once with an
 * `OrdoNetworkError`, so the caller decides whether
 * to start again.
 */
export async function waitForDelegation(
  endpoint: DelegationEndpoint,
  claim: DelegationClaim,
  options: WaitOptions = {},
): Promise<GrantedDelegation> {
  const interval = options.intervalMs ?? 2000;
  const sleep = options.sleep ?? defaultSleep;
  const now = options.now ?? Date.now;
  const started = now();
  for (;;) {
    if (options.signal?.aborted) throw new DelegationError('aborted', 'the wait was aborted');
    const r = await redeemDelegation(endpoint, claim, options.signal ? { signal: options.signal } : {});
    if (r.status === 'granted') return r;
    options.onWaiting?.({ expiresAt: r.expiresAt });
    if (options.timeoutMs !== undefined) {
      if (now() - started + interval > options.timeoutMs) {
        throw new DelegationError('timeout', 'the person has not answered yet');
      }
    } else if (now() + interval > r.expiresAt * 1000) {
      // No deadline given: the request's own expiry (seconds) is the deadline.
      throw new DelegationError('timeout', 'the request expired before the person answered');
    }
    await sleep(interval, options.signal);
  }
}

// ---- the person's half ----------------------------------------------------

/** One entity a delegated op reads, writes or examines, and the fields it touches. */
export interface DisclosedAccess {
  readonly entity: string;
  readonly fields: readonly string[];
}

/** One op the agent asks for, as the person is shown it. */
export interface DisclosedOp {
  readonly op: string;
  readonly kind: string;
  readonly reads: readonly DisclosedAccess[];
  readonly writes: readonly DisclosedAccess[];
  readonly examines: readonly DisclosedAccess[];
  readonly deletes: readonly string[];
  /** The op in plain words. Show these. */
  readonly words: readonly string[];
  /** Whether the op acts only on the person's own rows. */
  readonly yours: boolean;
}

/** The disclosure the person reads before approving. Show every part of it. */
export interface DelegationDisclosure {
  readonly request: string;
  readonly tenant: string;
  readonly agent: { readonly id: string; readonly name: string; readonly namedBy: string };
  readonly person: string;
  readonly term: {
    readonly hours: number;
    readonly expiresIfApprovedNow: number;
    readonly expiresText: string;
  };
  readonly requestExpiresAt: number;
  readonly ops: readonly DisclosedOp[];
  /** Ops the agent asked for that the delegation will not grant, and why. */
  readonly notGranted: readonly { readonly op: string; readonly reason: string }[];
  /** How many asked-for names the door did not know. */
  readonly unknownNames: number;
  /** What the agent can never do, whatever it is granted. */
  readonly cannot: readonly string[];
  /** The ceiling sentence. Show it. */
  readonly ceiling: string;
  /** How to take it back. Show it. */
  readonly revoke: string;
  /** What the tenant owner can see of this delegation, or `null`. */
  readonly ownerSees: string | null;
}

/** What approving made. */
export interface ApprovedDelegation {
  readonly request: string;
  readonly delegation: string;
  readonly agent: string;
  readonly ops: readonly string[];
  readonly expiresAt: number;
  /** Earlier delegations this one replaced. */
  readonly replaced: readonly string[];
  readonly outcome: 'approved';
}

/** What declining made. */
export interface DeclinedDelegation {
  readonly request: string;
  readonly outcome: 'declined';
}

/** The state of one entry in the person's list. */
export type DelegationState = 'waiting' | 'live' | 'revoked' | 'expired' | 'declined' | 'failed';

/** One entry in the person's own list of delegations. */
export interface MyDelegation {
  /** `null` once the request is gone. */
  readonly request: string | null;
  /** `null` while waiting. */
  readonly delegation: string | null;
  readonly agent: string;
  readonly agentName: string;
  readonly state: DelegationState;
  readonly ops: readonly string[];
  readonly grantedAt: number | null;
  readonly expiresAt: number;
  readonly revokedAt: number | null;
}

/** What revoking made. */
export interface RevokedDelegation {
  readonly delegation: string;
  readonly outcome: 'revoked' | 'already ended';
}

const REFUSALS = ['access_denied', 'invalid_param', 'method_mismatch', 'missing_param', 'unknown_param'] as const;

function platformSpec(op: string, method: 'GET' | 'POST', params: readonly string[]): OpSpec {
  return {
    op,
    version: 'platform',
    method,
    kind: method === 'GET' ? 'read' : 'write',
    terminal: method === 'GET' ? ['fetch'] : ['commit'],
    paged: false,
    singleRow: false,
    casParam: null,
    params: params.map((name) => ({
      name,
      type: 'text',
      required: true,
      idEntity: null,
      enumValues: null,
      min: null,
      max: null,
    })),
    refusals: REFUSALS,
    standingError: null,
    sync: null,
    refine: { tier: 'none', fields: [] },
    order: null,
  };
}

const REVIEW = platformSpec('std.delegation.review', 'GET', ['request']);
const APPROVE = platformSpec('std.delegation.approve', 'POST', ['request']);
const DECLINE = platformSpec('std.delegation.decline', 'POST', ['request']);
const MINE = platformSpec('std.delegation.mine', 'GET', []);
const REVOKE = platformSpec('std.delegation.revoke', 'POST', ['delegation']);

async function firstRow<R>(client: OrdoClient, spec: OpSpec, params: Params): Promise<R> {
  const r = await client.request<R>(spec, params);
  const row = r.rows[0];
  if (row === undefined) {
    throw new DelegationError('unknown', `${spec.op} answered no row`);
  }
  return row;
}

/** The disclosure for a request: what the agent could do, in the person's terms. */
export function reviewDelegation(client: OrdoClient, request: string): Promise<DelegationDisclosure> {
  return firstRow<DelegationDisclosure>(client, REVIEW, { request });
}

/**
 * Grant a request. Needs a fresh passkey touch unless the app developer
 * exempted it: run `stepUp` first and build the client from the session it
 * answers. Otherwise the door refuses `access_denied` (an `OrdoRefusal`).
 */
export function approveDelegation(client: OrdoClient, request: string): Promise<ApprovedDelegation> {
  return firstRow<ApprovedDelegation>(client, APPROVE, { request });
}

/** Refuse a request. The agent's next redeem is `declined`. */
export function declineDelegation(client: OrdoClient, request: string): Promise<DeclinedDelegation> {
  return firstRow<DeclinedDelegation>(client, DECLINE, { request });
}

/** The person's own delegations: waiting, live and ended. */
export async function myDelegations(client: OrdoClient): Promise<readonly MyDelegation[]> {
  const r = await client.request<MyDelegation>(MINE, {});
  return r.rows;
}

/** End a live delegation now. Revoking one that already ended answers `already ended`. */
export function revokeDelegation(client: OrdoClient, delegation: string): Promise<RevokedDelegation> {
  return firstRow<RevokedDelegation>(client, REVOKE, { delegation });
}
