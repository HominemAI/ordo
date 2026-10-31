/**
 * Where the door is, and which ways in it serves.
 *
 * Every value here is read when the page is built or served (`VITE_*`).
 *
 * - `VITE_ORDO_URL` is the door. Locally it is the `ordo-host` that
 *   `../dev/serve.sh` starts, and that is the default under `npm run dev`. A
 *   built page has no default: one that guessed would talk to the wrong door,
 *   so a build made without the variable refuses to start and says why.
 * - The ways in come from the door: the page asks `GET /auth/methods` at start
 *   (`methods.ts`). `VITE_ORDO_SIGN_IN` is a fallback only, used when the door
 *   cannot answer that route. It lists the ways in, in the words of
 *   `ORDO_AUTH_METHODS`: `passkey`, `email`, `guest`, and the name of each
 *   outside connection (`discord`, `github`, `google`, ...). Unset, it is
 *   `passkey`.
 * - `VITE_ORDO_RETURN_URL` is where an outside provider sends the person back.
 *   Its origin must be in the deployment's `ORDO_AUTH_RETURN_URLS`. Unset, it
 *   is this page's own address.
 */
const url: string | undefined = import.meta.env.VITE_ORDO_URL;

function door(): string {
  if (url) return url;
  if (import.meta.env.DEV) return 'http://127.0.0.1:4001';
  throw new Error('This page was built without VITE_ORDO_URL, so it does not know where the deployment is.');
}

export const DOOR: string = door();

/** One outside connection: the name the deployment knows it by, and the button's words. */
export interface Connection {
  readonly name: string;
  readonly label: string;
}

/** The ways in this page offers. */
export interface SignInMethods {
  readonly passkey: boolean;
  readonly email: boolean;
  readonly guest: boolean;
  readonly connections: readonly Connection[];
}

/** How the well-known providers are named on a button. Any other name is shown as written. */
const KNOWN: Readonly<Record<string, string>> = {
  discord: 'Discord',
  github: 'GitHub',
  google: 'Google',
  microsoft: 'Microsoft',
};

/** A connection name is 1 to 32 of `a-z`, `0-9` and `-`, as the deployment's are. */
const CONNECTION_NAME = /^[a-z0-9-]{1,32}$/;

/**
 * Read a list such as `passkey,email,guest,discord`. Words are trimmed and
 * lowercased, and repeats count once. A word that cannot be a connection name
 * is dropped. An empty list is `passkey`, the default.
 */
export function signInMethods(raw: string | undefined): SignInMethods {
  const words = (raw ?? '')
    .split(',')
    .map((w) => w.trim().toLowerCase())
    .filter((w) => w !== '');
  const seen = new Set(words.length === 0 ? ['passkey'] : words);
  const connections: Connection[] = [];
  for (const w of seen) {
    if (w === 'passkey' || w === 'email' || w === 'guest' || w === 'password') continue;
    if (!CONNECTION_NAME.test(w)) continue;
    connections.push({ name: w, label: KNOWN[w] ?? w });
  }
  return { passkey: seen.has('passkey'), email: seen.has('email'), guest: seen.has('guest'), connections };
}

/** The build variable's reading: the fallback when the door cannot be asked. */
export const SIGN_IN: SignInMethods = signInMethods(import.meta.env.VITE_ORDO_SIGN_IN);

/** Where an outside provider sends the person back: this page, with no query or fragment. */
export function returnUrl(location: Pick<Location, 'origin' | 'pathname'>): string {
  const set: string | undefined = import.meta.env.VITE_ORDO_RETURN_URL;
  return set || `${location.origin}${location.pathname}`;
}

/** The variables the deployment owner sets so this page's origin is accepted. */
export function originHint(origin: string): string {
  return (
    `ORDO_CORS_ORIGINS=${origin} (to call the door) and ` +
    `ORDO_AUTH_ORIGINS=${origin} (to run a passkey ceremony)`
  );
}
