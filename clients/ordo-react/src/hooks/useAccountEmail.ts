/**
 * `useAccountEmail`: the signed-in person's own email address, and the three
 * steps that change it (owner ruling 2026-10-22).
 *
 * It reads `GET /auth/email` once, and again after each change. `begin`
 * mails a code to an address and holds the ceremony in memory; `confirm`
 * sends the code; `remove` takes the address off. `needed` is true when the
 * app developer requires an address and the person has none, so a page can
 * ask before an op is refused `email_required`.
 *
 * What the page shows is the app developer's choice.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { TokenSource } from '../client.js';
import {
  AccountEmailError,
  beginAccountEmail,
  confirmAccountEmail,
  needsAccountEmail,
  readAccountEmail,
  removeAccountEmail,
  type AccountEmail,
  type EmailCodeSent,
} from '../email.js';

export interface UseAccountEmailOptions {
  /** The door's base URL. */
  readonly baseUrl: string;
  /** The person's token, or a function that gives the current one. */
  readonly token: TokenSource;
  /** Injectable for tests; defaults to the global. */
  readonly fetch?: typeof globalThis.fetch;
  /** `false` holds the hook still (no reads). Default `true`. */
  readonly enabled?: boolean;
}

export interface AccountEmailResult {
  /** The last read, or `null` before the first. */
  readonly email: AccountEmail | null;
  /** The address a code was mailed to and not yet typed, else `null`. */
  readonly pending: string | null;
  /** True while a call is in flight. */
  readonly busy: boolean;
  /** The last call that failed, or `null`. A wrong code is `refused`. */
  readonly problem: AccountEmailError | null;
  /** The app requires an address of this person, and they have none. */
  readonly needed: boolean;
  /** Mail a code to `address`. */
  begin(address: string): Promise<void>;
  /** Send the code the person typed. The address is the account's once it lands. */
  confirm(code: string): Promise<void>;
  /** Forget the code that was mailed, without changing anything. */
  cancel(): void;
  /** Take the added address off. */
  remove(): Promise<void>;
  /** Read again. */
  refresh(): Promise<void>;
}

const asError = (e: unknown) =>
  e instanceof AccountEmailError ? e : new AccountEmailError('unavailable', String(e));

export function useAccountEmail(options: UseAccountEmailOptions): AccountEmailResult {
  const [email, setEmail] = useState<AccountEmail | null>(null);
  const [pending, setPending] = useState<{ address: string; sent: EmailCodeSent } | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<AccountEmailError | null>(null);
  // The token and fetch are read through a ref, so a new function each
  // render does not read again.
  const latest = useRef(options);
  latest.current = options;
  const live = useRef(true);
  const enabled = options.enabled ?? true;

  const endpoint = () => {
    const o = latest.current;
    return { baseUrl: o.baseUrl, token: o.token, ...(o.fetch ? { fetch: o.fetch } : {}) };
  };

  /** Run one call with `busy` set, keeping its failure as `problem`. */
  const step = useCallback(async (work: () => Promise<void>) => {
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (e) {
      if (live.current) setProblem(asError(e));
    } finally {
      if (live.current) setBusy(false);
    }
  }, []);

  const refresh = useCallback(
    () =>
      step(async () => {
        const e = await readAccountEmail(endpoint());
        if (live.current) setEmail(e);
      }),
    [step],
  );

  useEffect(() => {
    live.current = true;
    if (enabled) void refresh();
    return () => {
      live.current = false;
    };
  }, [enabled, options.baseUrl, refresh]);

  const begin = useCallback(
    (address: string) =>
      step(async () => {
        const sent = await beginAccountEmail(endpoint(), address);
        if (live.current) setPending({ address: address.trim(), sent });
      }),
    [step],
  );

  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const confirm = useCallback(
    (code: string) =>
      step(async () => {
        const p = pendingRef.current;
        if (!p) throw new AccountEmailError('refused', 'no code was mailed: begin with an address first');
        await confirmAccountEmail(endpoint(), p.sent, code);
        if (!live.current) return;
        setPending(null);
        setEmail(await readAccountEmail(endpoint()));
      }),
    [step],
  );

  const remove = useCallback(
    () =>
      step(async () => {
        await removeAccountEmail(endpoint());
        if (live.current) setEmail(await readAccountEmail(endpoint()));
      }),
    [step],
  );

  const cancel = useCallback(() => {
    setPending(null);
    setProblem(null);
  }, []);

  return {
    email,
    pending: pending?.address ?? null,
    busy,
    problem,
    needed: needsAccountEmail(email),
    begin,
    confirm,
    cancel,
    remove,
    refresh,
  };
}
