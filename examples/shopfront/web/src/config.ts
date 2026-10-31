import type { Origins } from './pages.js';

/**
 * Where the door is, where the three pages are, and which ways in it serves.
 *
 * Every value here is read when the page is built or served (`VITE_*`). The
 * same build is served at all three origins. In a real deployment they are
 * `https://login.example.com`, `https://shop.example.com` and
 * `https://admin.example.com`.
 *
 * - `VITE_ORDO_URL` is the door. Default `http://127.0.0.1:4010`, the
 *   `ordo-host` that `../dev/serve.sh` starts. A built page has no default:
 *   one that guessed would talk to the wrong door.
 * - `VITE_ORDO_SPACE` is the deployment's tenant, which every sign-in asks to
 *   be admitted to. Default `shopfront`.
 * - `VITE_LOGIN_URL`, `VITE_SHOP_URL`, `VITE_ADMIN_URL` are the three page
 *   origins. Defaults `http://localhost:5180`, `:5181`, `:5182`. They must
 *   match the deployment's `ORDO_AUTH_ORIGINS`, `ORDO_CORS_ORIGINS` and (shop
 *   and admin) `ORDO_AUTH_RETURN_URLS`.
 * - The ways in come from the door: the login page asks `GET /auth/methods` at
 *   start (`methods.ts`). `VITE_ORDO_SIGN_IN` is a fallback only, used when the
 *   door cannot answer that route. It lists the ways in, in the words of
 *   `ORDO_AUTH_METHODS`. This page offers `passkey` and `email`. Unset, it is
 *   `passkey`.
 */

function setting(value: string | undefined, dev: string, name: string): string {
  if (value) return value;
  if (import.meta.env.DEV) return dev;
  throw new Error(`This page was built without ${name}, so it does not know where to go.`);
}

export const DOOR: string = setting(import.meta.env.VITE_ORDO_URL, 'http://127.0.0.1:4010', 'VITE_ORDO_URL');

/** The tenant everyone is admitted to. */
export const SPACE: string = import.meta.env.VITE_ORDO_SPACE || 'shopfront';

export const ORIGINS: Origins = {
  login: setting(import.meta.env.VITE_LOGIN_URL, 'http://localhost:5180', 'VITE_LOGIN_URL'),
  shop: setting(import.meta.env.VITE_SHOP_URL, 'http://localhost:5181', 'VITE_SHOP_URL'),
  admin: setting(import.meta.env.VITE_ADMIN_URL, 'http://localhost:5182', 'VITE_ADMIN_URL'),
};

/** The ways in this page offers. */
export interface SignInMethods {
  readonly passkey: boolean;
  readonly email: boolean;
}

/**
 * Read a list such as `passkey,email`. Words are trimmed and lowercased.
 * An empty list is `passkey`, the default.
 */
export function signInMethods(raw: string | undefined): SignInMethods {
  const words = (raw ?? '')
    .split(',')
    .map((w) => w.trim().toLowerCase())
    .filter((w) => w !== '');
  const seen = new Set(words.length === 0 ? ['passkey'] : words);
  return { passkey: seen.has('passkey'), email: seen.has('email') };
}

/** The build variable's reading: the fallback when the door cannot be asked. */
export const SIGN_IN: SignInMethods = signInMethods(import.meta.env.VITE_ORDO_SIGN_IN);

/** The variables the deployment owner sets so this page's origin is accepted. */
export function originHint(origin: string): string {
  return (
    `ORDO_CORS_ORIGINS=${origin} (to call the door) and ` +
    `ORDO_AUTH_ORIGINS=${origin} (to run a passkey ceremony)`
  );
}
