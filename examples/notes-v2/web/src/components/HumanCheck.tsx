import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * Cloudflare Turnstile, for a deployment that set
 * `ORDO_AUTH_HUMAN_CHECK=turnstile`.
 *
 * The widget gives a token, and the page sends it as `human` on the first leg
 * of a sign-in. The door checks each token once, so the widget is reset after
 * every try and gives a fresh one.
 */

interface Turnstile {
  render(el: HTMLElement, options: Record<string, unknown>): string;
  reset(id?: string): void;
  remove(id: string): void;
}

declare global {
  interface Window {
    turnstile?: Turnstile;
  }
}

const SCRIPT = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
let loading: Promise<Turnstile> | null = null;

function load(): Promise<Turnstile> {
  if (window.turnstile) return Promise.resolve(window.turnstile);
  loading ??= new Promise<Turnstile>((resolve, reject) => {
    const s = document.createElement('script');
    s.src = SCRIPT;
    s.async = true;
    s.onload = () => (window.turnstile ? resolve(window.turnstile) : reject(new Error('Turnstile did not load.')));
    s.onerror = () => {
      loading = null;
      reject(new Error('The human check could not be loaded.'));
    };
    document.head.appendChild(s);
  });
  return loading;
}

export interface HumanCheck {
  /** The current token, or undefined while there is none (or the check is off). */
  readonly token: string | undefined;
  /** False only while the check is on and has no token yet. */
  readonly ready: boolean;
  /** Ask the widget for a fresh token. Call it after each try. */
  readonly reset: () => void;
  /** The widget, or null when the check is off. */
  readonly widget: JSX.Element | null;
}

/** The widget and its token. With no site key the check is off and `ready` is true. */
export function useHumanCheck(siteKey: string | null): HumanCheck {
  const box = useRef<HTMLDivElement | null>(null);
  const id = useRef<string | null>(null);
  const [token, setToken] = useState<string | undefined>(undefined);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    if (siteKey === null) return;
    let gone = false;
    load()
      .then((t) => {
        if (gone || box.current === null) return;
        id.current = t.render(box.current, {
          sitekey: siteKey,
          callback: (v: string) => setToken(v),
          'expired-callback': () => setToken(undefined),
          'error-callback': () => setToken(undefined),
        });
      })
      .catch((e: unknown) => setFailed(e instanceof Error ? e.message : 'The human check could not be loaded.'));
    return () => {
      gone = true;
      if (id.current !== null) window.turnstile?.remove(id.current);
      id.current = null;
    };
  }, [siteKey]);

  const reset = useCallback(() => {
    if (siteKey === null) return;
    setToken(undefined);
    if (id.current !== null) window.turnstile?.reset(id.current);
  }, [siteKey]);

  const widget =
    siteKey === null ? null : (
      <div className="human-check">
        <div ref={box} />
        {failed !== null && <p className="error">{failed}</p>}
      </div>
    );
  return { token, ready: siteKey === null || token !== undefined, reset, widget };
}
