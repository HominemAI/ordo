import { useState } from 'react';
import { explain } from '../api';

/** One action at a time, with its error in plain words. */
export function useBusy() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, setError, run };
}
