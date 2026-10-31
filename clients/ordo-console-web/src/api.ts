/**
 * The console's JSON API, the one way this page talks to anything.
 *
 * Same origin. With the console's own sign-in the session rides in its
 * HttpOnly cookie. With the identity service (`identity.ts`) every call
 * carries the service's token as a bearer, held in memory only; a token the
 * console refuses is dropped and the call runs once more with a new one.
 * Every failure is one `ConsoleCallError` carrying the server's code, so
 * screens decide on the code and show plain words.
 */

import { forgetToken, identityMode, token } from './identity';

/** The deployment handle from the address: `/d/<handle>/...`. */
export function handleFromPath(path: string): string {
  const m = /^\/d\/([a-z0-9-]+)(?:\/|$)/.exec(path);
  return m?.[1] ?? 'main';
}

export const handle = handleFromPath(window.location.pathname);
export const base = `/d/${handle}/`;

/** What the server said when it refused. */
export class ConsoleCallError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status: number,
    readonly retry: boolean,
    /** The rest of the refusal: `setting`, `revision`, `rolledBackTo`. */
    readonly extra: { setting?: string; revision?: number; rolledBackTo?: number; doorError?: string } = {},
  ) {
    super(message);
    this.name = 'ConsoleCallError';
  }
}

/** Codes that mean the session is gone and the person must sign in again. */
export const SIGNED_OUT = new Set(['not_signed_in', 'session_ended', 'token_refused']);

async function call<T>(method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
  if (!identityMode()) return once<T>(method, path, body, null);
  try {
    return await once<T>(method, path, body, await token());
  } catch (e) {
    if (!(e instanceof ConsoleCallError && (e.code === 'token_refused' || e.code === 'not_signed_in'))) throw e;
  }
  forgetToken();
  return once<T>(method, path, body, await token());
}

async function once<T>(method: 'GET' | 'POST', path: string, body: unknown, bearer: string | null): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (bearer) headers['authorization'] = `Bearer ${bearer}`;
  let res: Response;
  try {
    res = await fetch(`${base}api/${path}`, {
      method,
      credentials: 'same-origin',
      cache: 'no-store',
      headers,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  } catch {
    throw new ConsoleCallError('offline', 'The console did not answer. Check the connection and try again.', 0, true);
  }
  const text = await res.text();
  let json: unknown = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = null;
  }
  if (!res.ok) {
    const e = (json ?? {}) as {
      error?: string;
      message?: string;
      retry?: boolean;
      setting?: string;
      revision?: number;
      rolledBackTo?: number;
      doorError?: string;
    };
    throw new ConsoleCallError(
      e.error ?? 'unknown',
      e.message ?? `The console answered ${res.status}.`,
      res.status,
      e.retry ?? false,
      {
        ...(e.setting === undefined ? {} : { setting: e.setting }),
        ...(e.revision === undefined ? {} : { revision: e.revision }),
        ...(e.rolledBackTo === undefined ? {} : { rolledBackTo: e.rolledBackTo }),
        ...(e.doorError === undefined ? {} : { doorError: e.doorError }),
      },
    );
  }
  return json as T;
}

export const get = <T>(path: string): Promise<T> => call<T>('GET', path);
export const post = <T>(path: string, body: unknown = {}): Promise<T> => call<T>('POST', path, body);

export interface Status {
  deployment: string;
  hasOwner: boolean;
  /** Set when people sign in at the identity service. */
  identity?: { origin: string; audience: string } | null;
}

export interface Session {
  owner: string;
  /** The person's `sub` at the identity service, in that mode. */
  person?: string;
  mustEnrol: boolean;
  createdAt: number;
  idleEndsAt: number;
  endsAt: number;
  touchFreshUntil: number | null;
}

export interface Owner {
  id: string;
  label: string;
  createdAt: number;
}

/** What an enrolment with a code answers. */
export interface Enrolled {
  owner: string;
  kind: 'first-run' | 'invite' | 'recover';
  recoveryCodes: string[];
  sessionsEnded: number;
}

export interface Invite {
  code: string;
  expiresAt: number;
}

/** Where the supervised deployment is (docs/plan-console.md 6.2). */
export type ChildState = 'starting' | 'ready' | 'restarting' | 'crash-looping' | 'held' | 'stopped';

export interface LastExit {
  code: number | null;
  signal: number | null;
  /** Set when the deployment could not be started at all. */
  error: string | null;
  at: number;
}

export interface RuntimeStatus {
  state: ChildState;
  phase: string;
  pid: number | null;
  /** The revision the running process was started with. */
  revision: number | null;
  /** The last good revision, or the one being tried. */
  liveRevision: number;
  restarts: number;
  lastExit: LastExit | null;
  readySince: number | null;
  autoChangesHeld: boolean;
  /** The revision an apply or rollback is bringing up, if one is. */
  applying: number | null;
}

export interface SecretRef {
  name: string;
  kind: 'managed' | 'referenced' | 'held';
  set: true;
  version?: number;
  path?: string;
  changedAt: number;
}

export interface Runtime {
  mode: 'supervise';
  status: RuntimeStatus;
  privilege: { separateUser: boolean; note: string | null };
  containerVariablesIgnored: string[];
  /** Names the console treats as secrets. */
  secretSettings?: string[];
  current: { revision: number; settings: Record<string, string>; secrets: SecretRef[] };
  logs: string[];
}

/** A part of what the door reports, or why it could not be read. */
export type Reported = Record<string, unknown> & { error?: string; message?: string };

/** The Runtime screen in door-only mode: read-only, what the door reports. */
export interface DoorOnly {
  mode: 'door-only';
  message: string;
  /** False when the door did not answer at all. Absent from older consoles. */
  answering?: boolean;
  reported?: { ready: Reported; deep: Reported; boot: Record<string, unknown> | null } | null;
  /** Set when no door address is known. */
  doorError?: string | null;
}

export interface Revision {
  number: number;
  settings: Record<string, string>;
  secrets: unknown;
  actor: string;
  reason: string;
  outcome: 'good' | 'refused' | 'failed' | 'crashed' | null;
  detail: string | null;
  createdAt: number;
  decidedAt: number | null;
}

export interface Revisions {
  live: number | null;
  revisions: Revision[];
}

export interface Diff {
  added: Record<string, string>;
  changed: Record<string, [string, string]>;
  removed: string[];
  secretsChanged: string[];
}

/** What apply and rollback answer once the new revision is ready. */
export interface Applied {
  revision: number;
  diff: Diff;
  status: RuntimeStatus;
}

/** A secret in a change: a value the console seals, a mounted file, or removal. */
export type SecretInput = { value: string } | { file: string } | null;

export interface Patch {
  set: Record<string, string>;
  unset: string[];
  secrets: Record<string, SecretInput>;
  reason: string;
}

/** `GET /health/ready`: no sign-in, outside the API prefix. */
export interface Ready {
  status: string;
  consoleDatabase: 'ok' | 'unreachable';
  deployment: ChildState | 'door-only';
}

export async function ready(): Promise<Ready> {
  const res = await fetch('/health/ready', { cache: 'no-store', credentials: 'same-origin' });
  return (await res.json()) as Ready;
}

/** Plain words for the codes a person can meet. */
export function explain(e: unknown): string {
  if (e instanceof ConsoleCallError) {
    switch (e.code) {
      case 'code_refused':
        return 'That code did not work. Check it and try again. Each code works once.';
      case 'passkey_refused':
        return 'That passkey was not accepted here. Try again, or use a recovery code.';
      case 'not_signed_in':
      case 'session_ended':
      case 'token_refused':
        return 'You are signed out. Sign in again.';
      case 'identity_not_answering':
        return 'The identity service did not answer. It may be starting; try again shortly.';
      case 'moved_to_identity_service':
        return 'You sign in at the identity service now. Reload the page.';
      case 'owner_limit':
        return 'The console already has two owners. Remove one first.';
      case 'last_owner':
        return 'You cannot remove the last owner.';
      case 'console_key_refused':
        return "The deployment refused this console's key: it was revoked there. Forget the link on the Deployment screen and link again.";
      case 'deployment_not_answering':
        return 'The deployment did not answer. It may be starting or down; try again shortly.';
      case 'origin_refused':
        return 'The console refused this page. Open it at the address the console was set up with.';
      default:
        return e.message;
    }
  }
  if (e instanceof DOMException) {
    if (e.name === 'NotAllowedError' || e.name === 'AbortError') {
      return 'The passkey prompt was closed or timed out. Try again.';
    }
    if (e.name === 'InvalidStateError') {
      return 'This device already holds a passkey for this console.';
    }
    if (e.name === 'SecurityError') {
      return 'The browser refused the passkey for this address. Open the console at its own address.';
    }
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}
