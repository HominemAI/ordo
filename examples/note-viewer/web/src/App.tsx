/**
 * note_viewer's page: the notes notes v2 holds, read through a peer.
 *
 * - Everyone sees the public list: `publicNotes`, a public op, called with
 *   no token.
 * - "Sign in" goes through the identity service, the OIDC connection
 *   `VITE_ORDO_CONNECTION` (`accounts` by default). The service sends the
 *   person back here with `?handoff=`, and `/auth/complete` swaps it for a
 *   session. The sign-in arrives at the deployment's own space, where the
 *   deployment admits people (`VITE_ORDO_SPACE`, `system` by default).
 * - Signed in, `myNotes` lists every note the person may read in notes v2.
 *   The viewer's guard follows notes v2's acl through `$caller`.
 * - A reload resumes the sign-in through the door's renewal cookie.
 *
 * The token stays in memory. Only the binder waits in `sessionStorage` while
 * the browser is at the service.
 */

import { useEffect, useRef, useState } from 'react';
import { call, type PasskeyEndpoint, type PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import { result } from '@hominemai/ordo-sdk/auth/second-step.js';
import { resumeSession } from '@hominemai/ordo-sdk/auth/resume.js';
import { signOut } from '@hominemai/ordo-sdk/auth/session.js';
import { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { myNotes, publicNotes, type MyNotesRow, type PublicNotesRow } from './generated/ops.js';
import { CONNECTION, DOOR, SPACE } from './config.js';

const BINDER_KEY = 'ordo-note-viewer.binder';
const endpoint: PasskeyEndpoint = { baseUrl: DOOR };

/** A token held in memory, renewed through the cookie when it is old. */
interface Held {
  token: string;
  expiresAt: number;
}

function keep(done: PasskeySession): Held {
  return { token: done.token, expiresAt: done.expiresAt };
}

function stored(): string | null {
  try {
    return sessionStorage.getItem(BINDER_KEY);
  } catch {
    return null;
  }
}

function store(binder: string | null): void {
  try {
    if (binder === null) sessionStorage.removeItem(BINDER_KEY);
    else sessionStorage.setItem(BINDER_KEY, binder);
  } catch {
    // Nothing kept: the person signs in again.
  }
}

/** This page's address with no query, where the service sends people back. */
function here(): string {
  return `${window.location.origin}${window.location.pathname}`;
}

/** Back from the identity service: the session, or null when this is not a return. */
async function returned(): Promise<PasskeySession | null> {
  const q = new URLSearchParams(window.location.search);
  const handoff = q.get('handoff');
  const error = q.get('error');
  if (handoff === null && error === null) return null;
  const binder = stored();
  store(null);
  window.history.replaceState(null, '', here());
  if (error !== null) throw new Error(`Sign-in did not finish: ${error}.`);
  if (binder === null) throw new Error('Sign-in did not start in this tab. Press Sign in again.');
  const r = result(
    await call(endpoint, '/auth/complete', { method: CONNECTION, kind: 'handoff', handoff, binder, space: SPACE }),
  );
  if (r.kind !== 'session') throw new Error('This deployment asks for a second step, which this page does not offer.');
  const admitted = r.session.admission?.outcome;
  if (admitted !== 'admitted' && admitted !== 'already-member') {
    throw new Error('You are signed in, but this deployment did not let you in.');
  }
  return r.session;
}

export function App() {
  const [held, setHeld] = useState<Held | null>(null);
  const [checked, setChecked] = useState(false);
  const [open, setOpen] = useState<readonly PublicNotesRow[] | null>(null);
  const [mine, setMine] = useState<readonly MyNotesRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The public list, with no token.
  useEffect(() => {
    publicNotes(new OrdoClient({ baseUrl: DOOR, space: SPACE }))
      .rows()
      .then(setOpen, (e: unknown) => setError(String(e)));
  }, []);

  // A return from the service, or a sign-in this browser still holds.
  useEffect(() => {
    let alive = true;
    (async () => {
      const done = (await returned()) ?? (await resumeSession(endpoint));
      if (alive && done) setHeld(keep(done));
    })()
      .catch((e: unknown) => alive && setError(e instanceof Error ? e.message : String(e)))
      .finally(() => alive && setChecked(true));
    return () => {
      alive = false;
    };
  }, []);

  // The token as last renewed, for calls made after this render.
  const latest = useRef<Held | null>(null);
  latest.current = held;
  async function token(): Promise<string> {
    const now = latest.current;
    if (!now) throw new Error('Not signed in.');
    if (now.expiresAt - Date.now() / 1000 > 30) return now.token;
    const done = await resumeSession(endpoint);
    if (!done) throw new Error('Your sign-in ended. Sign in again.');
    latest.current = keep(done);
    return done.token;
  }

  // Signed in: every note this person may read in notes v2. Read once per
  // sign-in.
  const signedIn = held !== null;
  useEffect(() => {
    if (!signedIn) {
      setMine(null);
      return;
    }
    myNotes(new OrdoClient({ baseUrl: DOOR, space: SPACE, token }))
      .rows()
      .then(setMine, (e: unknown) => setError(String(e)));
  }, [signedIn]);

  async function signIn() {
    setError(null);
    try {
      const begun = await call(endpoint, '/auth/begin', { method: CONNECTION, return_to: here() });
      const redirect = begun['redirect'];
      const binder = begun['binder'];
      if (typeof redirect !== 'string' || typeof binder !== 'string') throw new Error('The door gave no address to go to.');
      store(binder);
      window.location.assign(redirect);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function leave() {
    if (held) await signOut(endpoint, held.token).catch(() => undefined);
    setHeld(null);
  }

  return (
    <main>
      <header>
        <h1>Note viewer</h1>
        {checked &&
          (held ? (
            <button type="button" onClick={() => void leave()}>
              Sign out
            </button>
          ) : (
            <button type="button" onClick={() => void signIn()}>
              Sign in
            </button>
          ))}
      </header>
      {error && <p role="alert">{error}</p>}

      <section aria-labelledby="public-heading">
        <h2 id="public-heading">Public notes</h2>
        <p>Notes tagged “public” in notes v2. Anyone can read them here.</p>
        <Notes rows={open} label="Public notes" empty="No public notes yet." />
      </section>

      {held && (
        <section aria-labelledby="mine-heading">
          <h2 id="mine-heading">Your notes</h2>
          <p>Every note you can read in notes v2: your own and the ones shared with you.</p>
          <Notes rows={mine} label="Your notes" empty="You have no notes in notes v2 yet." />
        </section>
      )}
    </main>
  );
}

function Notes(props: {
  rows: readonly { id: string; title: string; body: string }[] | null;
  label: string;
  empty: string;
}) {
  if (props.rows === null) return <progress aria-label={`Loading ${props.label.toLowerCase()}`} />;
  if (props.rows.length === 0) return <p>{props.empty}</p>;
  return (
    <ul aria-label={props.label}>
      {props.rows.map((n) => (
        <li key={n.id}>
          <h3>{n.title}</h3>
          {n.body && <p>{n.body}</p>}
        </li>
      ))}
    </ul>
  );
}
