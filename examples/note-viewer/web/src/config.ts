/**
 * Where note_viewer's door is, read when the page is built (`VITE_*`).
 *
 * - `VITE_ORDO_URL` is note_viewer's door. Under `npm run dev` it defaults
 *   to `http://localhost:4100`. A built page has no default: one that
 *   guessed would talk to the wrong door.
 * - `VITE_ORDO_SPACE` is the space the page works in and a sign-in arrives
 *   at: the deployment's own space, `system` unless the deployment sets
 *   another `ORDO_SPACE`.
 * - `VITE_ORDO_CONNECTION` is the door's OIDC connection to the identity
 *   service, as `ORDO_AUTH_METHODS` names it. `accounts` by default.
 */
const url: string | undefined = import.meta.env.VITE_ORDO_URL;

function door(): string {
  if (url) return url;
  if (import.meta.env.DEV) return 'http://localhost:4100';
  throw new Error('This page was built without VITE_ORDO_URL, so it does not know where the deployment is.');
}

export const DOOR: string = door();
export const SPACE: string = import.meta.env.VITE_ORDO_SPACE || 'system';
export const CONNECTION: string = import.meta.env.VITE_ORDO_CONNECTION || 'accounts';
