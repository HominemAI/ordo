/**
 * Resuming the sign-in on load, and what to do when the door cannot be asked.
 *
 * The door can answer a resume three ways. It can continue the sign-in. It can
 * refuse (no cookie, or one it no longer honours), and then the person signs
 * in again. Or it can fail to answer at all because the network is down
 * (`OrdoNetworkError`, or a `PasskeyError` of `unavailable`). That last one
 * says nothing about the sign-in, so the page must not act as if it had been
 * refused: it opens the notes held on this device, says it is offline, and
 * asks again when the browser reports the network back.
 *
 * To open the device copy with no session the page needs the tenant and the
 * person. Both are ids, not secrets, and the page keeps them in `localStorage`
 * after a sign-in and erases them when the sign-in ends. The token is never
 * kept. The rows themselves stay sealed in IndexedDB as `device.ts` says.
 */

import { PasskeyError } from '@hominemai/ordo-sdk/auth/passkey.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { OrdoNetworkError } from '@hominemai/ordo-sdk/core/network.js';
import type { Backend } from '@hominemai/ordo-sdk/replica/backend.js';
import type { OrdoDevice } from '@hominemai/ordo-sdk/replica/device.js';
import { DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import { openNotesDevice } from './device.js';
import { INTERFACE } from './interface.js';
import { Session, type SessionOptions } from './session.js';

/** Who the device copy on this browser belongs to. */
export interface Remembered {
  readonly space: string;
  readonly principal: string;
}

const KEY = 'notes-v2.device-owner';

/** Keep whose notes this device holds. Storage that is blocked is no loss: the page then shows sign-in. */
export function remember(who: Remembered, storage: Pick<Storage, 'setItem'> | undefined = safeStorage()): void {
  try {
    storage?.setItem(KEY, JSON.stringify(who));
  } catch {
    // Private window or blocked storage.
  }
}

export function recall(storage: Pick<Storage, 'getItem'> | undefined = safeStorage()): Remembered | null {
  try {
    const raw = storage?.getItem(KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as Partial<Remembered>;
    return typeof v.space === 'string' && v.space !== '' && typeof v.principal === 'string' && v.principal !== ''
      ? { space: v.space, principal: v.principal }
      : null;
  } catch {
    return null;
  }
}

export function forget(storage: Pick<Storage, 'removeItem'> | undefined = safeStorage()): void {
  try {
    storage?.removeItem(KEY);
  } catch {
    // Nothing to do.
  }
}

function safeStorage(): Storage | undefined {
  try {
    return typeof localStorage === 'undefined' ? undefined : localStorage;
  } catch {
    return undefined;
  }
}

/** True when the door could not be asked, which is not the door saying no. */
export function isNetworkFailure(e: unknown): boolean {
  return e instanceof OrdoNetworkError || (e instanceof PasskeyError && e.failure === 'unavailable');
}

export type ResumeOutcome =
  | { readonly kind: 'session'; readonly session: Session }
  | { readonly kind: 'none' }
  | { readonly kind: 'offline'; readonly error: unknown };

/**
 * Ask the door to continue the sign-in. A refusal is `none`; a network
 * failure is `offline`. Anything else (a door answer this page cannot read)
 * is thrown for the caller to show.
 */
export async function resumeOutcome(options: SessionOptions): Promise<ResumeOutcome> {
  try {
    const session = await Session.resume(options);
    return session === null ? { kind: 'none' } : { kind: 'session', session };
  } catch (e) {
    if (isNetworkFailure(e)) return { kind: 'offline', error: e };
    throw e;
  }
}

/** The notes on this device, opened with no session, and the client the hooks need beside them. */
export interface OfflineNotes {
  readonly device: OrdoDevice;
  readonly hooks: HooksClient;
}

/**
 * Open the device copy for `who`. The clients hold no token: any call that
 * reaches for the door fails at once as a network failure, and the device
 * serves its rows without one.
 */
export async function openOfflineNotes(
  options: SessionOptions,
  who: Remembered,
  backend?: Backend,
): Promise<OfflineNotes> {
  const noToken = (): Promise<string> => Promise.reject(new Error('offline: no sign-in is held on this page yet'));
  const fetch = options.fetch ? { fetch: options.fetch } : {};
  const client = new OrdoClient({ baseUrl: options.baseUrl, space: who.space, token: noToken, ...fetch });
  const device = await openNotesDevice(client, who.space, who.principal, backend);
  const hooks = new HooksClient({
    baseUrl: options.baseUrl,
    iface: INTERFACE,
    space: who.space,
    token: noToken,
    ...(options.fetch ? { fetch: options.fetch } : {}),
  });
  return { device, hooks };
}

/**
 * Call `again` each time the browser says the network is back. Returns the
 * function that stops listening.
 */
export function whenOnline(target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'>, again: () => void): () => void {
  target.addEventListener('online', again);
  return () => target.removeEventListener('online', again);
}
