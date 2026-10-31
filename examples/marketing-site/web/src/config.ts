/**
 * Where the door is, and the Turnstile site key for the contact form.
 *
 * Both are read when the page is built or served. The page holds no secret and
 * no token: its reads are public. The site key is public by design. With no
 * key the page sends no human-check header, which works while the door's check
 * is off (`ORDO_PUBLIC_HUMAN_CHECK=off`, as `dev/serve.sh` sets it).
 */
export const DOOR: string = import.meta.env.VITE_ORDO_URL || 'http://localhost:4010';
export const TURNSTILE_SITE_KEY: string = import.meta.env.VITE_TURNSTILE_SITE_KEY || '';
