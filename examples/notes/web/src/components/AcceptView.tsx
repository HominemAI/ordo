import type { AcceptedShare, PeekedShare } from '@hominemai/ordo-sdk/auth/share.js';
import { useEffect, useState, type ReactNode } from 'react';
import { message } from '../hooks.js';
import type { Session } from '../session.js';
import { accept, peek } from '../shared.js';

interface Props {
  readonly ticket: string;
  readonly session: Session | null;
  /** The endpoint to peek through, which needs no session. */
  readonly baseUrl: string;
  /** The sign-in screen, shown while nobody is signed in. */
  readonly signedOut: ReactNode;
  readonly onAccepted: (accepted: AcceptedShare) => void;
  readonly onCancel: () => void;
}

/**
 * A link somebody sent. It says what is offered and what agreeing tells the
 * sharer, and nothing is granted until the person agrees while signed in.
 */
export function AcceptView(props: Props) {
  const { ticket, session, baseUrl, onAccepted } = props;
  const [offer, setOffer] = useState<PeekedShare | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    peek({ endpoint: { baseUrl } }, ticket).then(
      (o) => alive && setOffer(o),
      () => alive && setProblem('This link cannot be used. It may be used up, expired, or taken back.'),
    );
    return () => {
      alive = false;
    };
  }, [ticket, baseUrl]);

  const agree = async () => {
    if (session === null) return;
    setBusy(true);
    try {
      onAccepted(await accept(session, ticket, name.trim()));
    } catch (e) {
      setProblem(message(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card" aria-label="A shared note">
      <h2>Someone shared a note with you</h2>
      {problem !== null && (
        <p role="alert" className="error">
          {problem}
        </p>
      )}
      {offer !== null && (
        <>
          <p>
            <strong>{offer.name ?? 'Someone'}</strong> shared {offer.label === '' ? 'a note' : `"${offer.label}"`}. You
            will be able to read it and nothing else of theirs, and you cannot change it.
          </p>
          <p className="hint">{offer.discloses}</p>
          <p className="hint">{offer.notice}</p>
          <p className="hint">Open until {new Date(offer.windowEndsAt * 1000).toLocaleDateString()}.</p>
        </>
      )}
      {session === null ? (
        props.signedOut
      ) : (
        <>
          <label htmlFor="guest-name">The name to show them</label>
          <input id="guest-name" value={name} onChange={(e) => setName(e.target.value)} disabled={busy} />
          <div className="row">
            <button
              className="primary"
              disabled={busy || offer === null || name.trim() === ''}
              onClick={() => void agree()}
            >
              Agree and read it
            </button>
            <button onClick={props.onCancel}>Not now</button>
          </div>
        </>
      )}
    </section>
  );
}
