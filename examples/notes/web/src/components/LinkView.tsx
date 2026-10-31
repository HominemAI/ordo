import { OrdoProvider, useOpQuery, type DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import type { PasskeyEndpoint } from '@hominemai/ordo-sdk/auth/passkey.js';
import { useEffect, useMemo, useState } from 'react';
import { friendly, refusalText } from '../errors.js';
import { noteOf } from '../notes.js';
import { PasskeyError } from '../session.js';
import { linkInto } from '../shared.js';

interface Props {
  readonly secret: string;
  readonly endpoint: PasskeyEndpoint;
}

type State =
  | { readonly kind: 'loading' }
  | { readonly kind: 'open'; readonly client: HooksClient }
  /** Unknown, expired or revoked. The door gives one answer, so the page says all three. */
  | { readonly kind: 'closed' }
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

  useEffect(() => {
    let alive = true;
    setState({ kind: 'loading' });
    linkInto(endpoint, secret).then(
      (c) => alive && setState({ kind: 'open', client: c.hooks }),
      (e: unknown) => {
        if (!alive) return;
        // A refusal at the open is a closed link.
        if (e instanceof PasskeyError && e.failure === 'refused') setState({ kind: 'closed' });
        else setState({ kind: 'error', text: friendly(e, endpoint.baseUrl) });
      },
    );
    return () => {
      alive = false;
    };
  }, [secret, endpoint]);

  return (
    <section className="card" aria-label="A shared note">
      <h2>A note shared with you</h2>
      {state.kind === 'loading' && <p className="hint">Opening the link.</p>}
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
        <OrdoProvider client={state.client}>
          <LinkNotes />
        </OrdoProvider>
      )}
    </section>
  );
}

/** What the link shows. A read the door refuses as a revocation means the link was taken back. */
function LinkNotes() {
  const query = useOpQuery('myNotes');
  const notes = useMemo(() => query.rows.map((r) => noteOf({ ...r })), [query.rows]);

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
      <ul className="notes">
        {notes.map((n) => (
          <li key={n.id} className="readonly">
            <strong>{n.title || 'Untitled'}</strong>
            <p>{n.body}</p>
          </li>
        ))}
      </ul>
    </>
  );
}
