/**
 * Which page an address is, and where a hand-off may go. Pure functions of
 * the three origins, so they are tested with no browser.
 */

export type PageName = 'login' | 'shop' | 'admin';

export interface Origins {
  readonly login: string;
  readonly shop: string;
  readonly admin: string;
}

/** The origin of an address, or null when it is not an address. */
export function originOf(url: string): string | null {
  try {
    const u = new URL(url);
    return u.origin === 'null' ? null : u.origin;
  } catch {
    return null;
  }
}

/** The page served at `origin`, or null when it is none of the three. */
export function pageFor(origin: string, origins: Origins): PageName | null {
  const here = originOf(origin);
  if (here === null) return null;
  for (const name of ['login', 'shop', 'admin'] as const) {
    if (originOf(origins[name]) === here) return name;
  }
  return null;
}

/**
 * Whether the login page may send a person to `returnTo`: its origin is the
 * shop's or the staff console's, with any path. The door checks again against
 * `ORDO_AUTH_RETURN_URLS`.
 */
export function allowedReturn(returnTo: string, origins: Origins): boolean {
  const target = originOf(returnTo);
  if (target === null) return false;
  return target === originOf(origins.shop) || target === originOf(origins.admin);
}
