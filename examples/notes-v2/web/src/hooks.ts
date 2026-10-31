import { useEffect, useState } from 'react';

/** Whether the browser thinks it has a network. */
export function useOnline(): boolean {
  const [online, setOnline] = useState(() => (typeof navigator === 'undefined' ? true : navigator.onLine));
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener('online', on);
    window.addEventListener('offline', off);
    return () => {
      window.removeEventListener('online', on);
      window.removeEventListener('offline', off);
    };
  }, []);
  return online;
}

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

/** The ticket in `#/accept/<ticket>`, or null. */
export function acceptTicket(hash: string): string | null {
  const m = /^#\/accept\/([A-Za-z0-9._~-]+)$/.exec(hash);
  return m?.[1] ?? null;
}

/** The secret in `#/link/<secret>`, or null. */
export function linkSecret(hash: string): string | null {
  const m = /^#\/link\/([A-Za-z0-9._~-]+)$/.exec(hash);
  return m?.[1] ?? null;
}

/** What to show a person for an error, without a stack. */
export function message(e: unknown): string {
  if (e instanceof Error) return e.message;
  return 'Something went wrong.';
}
