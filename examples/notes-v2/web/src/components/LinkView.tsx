import { OrdoProvider } from '@hominemai/ordo-react';
import type { PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { useEffect, useState, type FormEvent } from 'react';
import { friendly, refusalText } from '../errors.js';
import { PasskeyError } from '../session.js';
import { linkInto, type GuestClients } from '../shared.js';
import { FilesProvider } from './Files.js';
import { PasswordInput } from './PasswordInput.js';
import { readableOf, SharedList, useSharedNotes, type Readable } from './ReadNotes.js';

interface Props {
  readonly secret: string;
  readonly endpoint: PasskeyEndpoint;
}

type State =
  | { readonly kind: 'loading' }
  | { readonly kind: 'open'; readonly clients: GuestClients }
  /** Unknown, expired or revoked. The door gives one answer, so the page says all three. */
  | { readonly kind: 'closed' }
  /**
   * The link wants a password. `problem` says why it is asked again: the last
   * one was wrong, or too many were and the link is locked until `until`
   * (milliseconds since the epoch).
   */
  | { readonly kind: 'password'; readonly problem: null | 'wrong' | 'locked'; readonly until?: number; readonly n: number }
  | { readonly kind: 'error'; readonly text: string };

const CLOSED = 'This link has expired or was revoked. Ask the person who sent it for a new one.';

/**
 * A note somebody shared with a link. It needs no account and no sign-in: the
 * secret in the address opens the link at the door, and what comes back is
 * read-only. A link that has expired or was taken back says so.
 *
 * The link gives the page a hooks client (`linkInto`), so the note is read
 * with `useOpQuery` like any other. The token behind it is never in the page.
 */
export function LinkView({ secret, endpoint }: Props) {
  const [state, setState] = useState<State>({ kind: 'loading' });
  // The password tried last, held in memory only. Each try opens the link again.
  const [attempt, setAttempt] = useState<{ readonly password?: string; readonly n: number }>({ n: 0 });

  useEffect(() => {
    let alive = true;
    setState((s) => (s.kind === 'password' ? s : { kind: 'loading' }));
    linkInto(endpoint, secret, attempt.password).then(
      (c) => alive && setState({ kind: 'open', clients: c }),
      (e: unknown) => {
        if (!alive) return;
        if (e instanceof PasskeyError) {
          // A refusal at the open is a closed link.
          if (e.failure === 'refused') return setState({ kind: 'closed' });
          if (e.failure === 'link_password_required') return setState({ kind: 'password', problem: null, n: attempt.n });
          if (e.failure === 'link_password_wrong') return setState({ kind: 'password', problem: 'wrong', n: attempt.n });
          if (e.failure === 'link_locked') {
            const until = Date.now() + Math.max(1, e.retryAfter ?? 60) * 1000;
            return setState({ kind: 'password', problem: 'locked', until, n: attempt.n });
          }
        }
        setState({ kind: 'error', text: friendly(e, endpoint.baseUrl) });
      },
    );
    return () => {
      alive = false;
    };
  }, [secret, endpoint, attempt]);

  return (
    <section className="card" aria-label="A shared note">
      <h2>A note shared with you</h2>
      {state.kind === 'loading' && <p className="hint">Opening the link.</p>}
      {state.kind === 'password' && (
        <PasswordForm
          problem={state.problem}
          until={state.until ?? null}
          answered={state.n}
          onTry={(password) => setAttempt((a) => ({ password, n: a.n + 1 }))}
        />
      )}
      {state.kind === 'closed' && (
        <p role="alert" className="error">
          {CLOSED}
        </p>
      )}
      {state.kind === 'error' && (
        <p role="alert" className="error">
          {state.text}
        </p>
      )}
      {state.kind === 'open' && (
        <OrdoProvider client={state.clients.hooks}>
          <FilesProvider>
            <LinkNotes readable={readableOf(state.clients.ops())} />
          </FilesProvider>
        </OrdoProvider>
      )}
    </section>
  );
}

/** What the link shows. A read the door refuses as a revocation means the link was taken back. */
function LinkNotes({ readable }: { readonly readable: Readable }) {
  const { query, notes } = useSharedNotes(readable);

  if (query.revoked) {
    return (
      <p role="alert" className="error">
        {CLOSED}
      </p>
    );
  }
  if (query.refusal) {
    return (
      <p role="alert" className="error">
        {refusalText(query.refusal)}
      </p>
    );
  }
  if (query.loading) return <p className="hint">Opening the link.</p>;
  return (
    <>
      <p className="hint">You can read this note. You cannot change it, and you did not need an account.</p>
      <SharedList notes={notes} readable={readable} />
    </>
  );
}

/** Minutes and seconds, as `4 minutes 5 seconds`. */
export function waitText(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  const part = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`;
  if (m === 0) return part(s, 'second');
  if (s === 0) return part(m, 'minute');
  return `${part(m, 'minute')} ${part(s, 'second')}`;
}

/** Seconds left until `until`, kept current once a second. */
function useSecondsLeft(until: number | null): number {
  const left = () => (until === null ? 0 : Math.max(0, Math.ceil((until - Date.now()) / 1000)));
  const [secs, setSecs] = useState(left);
  useEffect(() => {
    setSecs(left());
    if (until === null) return undefined;
    const timer = setInterval(() => {
      const now = left();
      setSecs(now);
      if (now === 0) clearInterval(timer);
    }, 1000);
    return () => clearInterval(timer);
    // `left` reads only `until`.
  }, [until]);
  return secs;
}

/**
 * The password a link asks for. A wrong one says so and asks again. Too many
 * wrong ones lock the link: the form says how long, counts down, and opens
 * again when the time is up.
 */
function PasswordForm({
  problem,
  until,
  answered,
  onTry,
}: {
  readonly problem: null | 'wrong' | 'locked';
  readonly until: number | null;
  /** Which try the door last answered. A new answer ends the try in flight. */
  readonly answered: number;
  readonly onTry: (password: string) => void;
}) {
  const [password, setPassword] = useState('');
  const [sent, setSent] = useState(false);
  const secs = useSecondsLeft(problem === 'locked' ? until : null);
  const locked = problem === 'locked' && secs > 0;

  // Each answer from the door ends the try.
  useEffect(() => setSent(false), [answered]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (password === '' || locked) return;
    setSent(true);
    onTry(password);
  };

  return (
    <form onSubmit={submit} aria-label="Password for this link">
      <p>This link needs a password. The person who sent it can tell you what it is.</p>
      <PasswordInput
        id="current-password"
        label="Password"
        value={password}
        onChange={setPassword}
        autoComplete="current-password"
        required
        disabled={locked}
        invalid={problem === 'wrong'}
        describedBy="link-password-problem"
      />
      <p id="link-password-problem" role="alert" className={problem === null ? 'hint' : 'error'}>
        {problem === 'wrong' && !sent && 'That password is not right. Try again.'}
        {locked &&
          `Too many wrong passwords. This link is locked for ${waitText(secs)}, until ${new Date(until ?? 0).toLocaleTimeString()}.`}
        {problem === 'locked' && !locked && 'The link is open to tries again.'}
      </p>
      <div className="row">
        <button type="submit" className="primary" disabled={locked || sent || password === ''}>
          Open the note
        </button>
      </div>
    </form>
  );
}
