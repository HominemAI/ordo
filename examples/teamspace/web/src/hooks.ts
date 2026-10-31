import { useEffect, useState } from 'react';

/** The part of the address after `#`, kept current. */
export function useHash(): string {
  const [hash, setHash] = useState(() => window.location.hash);
  useEffect(() => {
    const change = () => setHash(window.location.hash);
    window.addEventListener('hashchange', change);
    return () => window.removeEventListener('hashchange', change);
  }, []);
  return hash;
}

/** What to show a person for an error, without a stack. */
export function message(e: unknown): string {
  if (e instanceof Error) return e.message;
  return 'Something went wrong.';
}
