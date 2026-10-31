/**
 * `useFunction`: a backend function, `POST /fn/<name>`, as a verb and its
 * state (`docs/howto-functions.md`, "Calling a function").
 *
 * `call(params)` answers the function's JSON, or `null` when it was refused
 * or failed; `error` then holds why, as the SDK's `OrdoRefusal`. This
 * package renders nothing: the page shows `error.message`.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { FunctionCallOptions } from '@hominemai/ordo-sdk/core/function.js';
import type { JsonValue } from '../json.js';
import { useOrdo } from './context.js';

export interface UseFunction<R = JsonValue> {
  readonly call: (params?: Readonly<Record<string, unknown>>, options?: FunctionCallOptions) => Promise<R | null>;
  /** A call is running. */
  readonly busy: boolean;
  /** Why the last call did not answer, or `null`. */
  readonly error: Error | null;
}

export function useFunction<R = JsonValue>(name: string): UseFunction<R> {
  const client = useOrdo();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const call = useCallback(
    async (params: Readonly<Record<string, unknown>> = {}, options: FunctionCallOptions = {}): Promise<R | null> => {
      setBusy(true);
      setError(null);
      try {
        return (await client.callFunction<R>(name, params, options)).result;
      } catch (e) {
        if (mounted.current) setError(e instanceof Error ? e : new Error(String(e)));
        return null;
      } finally {
        if (mounted.current) setBusy(false);
      }
    },
    [client, name],
  );

  return { call, busy, error };
}
