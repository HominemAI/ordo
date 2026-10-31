import { useCommit, useOpQuery, useRow } from '@hominemai/ordo-react';
import { useMemo, useState } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import {
  linkOfShare,
  madeLinkOf,
  readLinksOf,
  readersIn,
  shareLinkParams,
  shareParams,
  type MadeLink,
  type Note,
} from '../notes.js';

interface Props {
  readonly note: Note;
  readonly online: boolean;
}

const when = (secs: number) => new Date(secs * 1000).toLocaleString();

/**
 * Share one note, read-only, in either of two ways, and take it back.
 *
 * Every call is a hook. The sharing ops are the platform's, so their hooks
 * come from the platform table (`withPlatformOps` in `interface.ts`), and the
 * provider's `stepUp` asks for the fresh passkey touch they want.
 */
export function SharePanel({ note, online }: Props) {
  // Who holds `read` on this note: the access rows other than the owner's own.
  // The list op is syncable, but a share list is the permission map and a
  // device never holds it, so the hook goes to the door with no option set.
  const access = useOpQuery('std.notesAccess.list', { live: true, enabled: online });
  const links = useOpQuery('std.space.links', { enabled: online });
  const readers = useMemo(() => readersIn(access.rows, note), [access.rows, note]);
  const readLinks = useMemo(() => readLinksOf(links.rows, note), [links.rows, note]);
  const unshare = useCommit('unshareNote');
  const invite = useCommit('std.space.share');
  const mint = useCommit('std.space.shareLink');
  const revokeLink = useCommit('std.space.revokeLink');
  const revokeShare = useCommit('std.space.revokeShare');
  const { bind } = useRow(note.id);
  const [sent, setSent] = useState<string | null>(null);
  const [made, setMade] = useState<MadeLink | null>(null);
  const [thrown, setThrown] = useState<string | null>(null);

  const refusal =
    mint.refusal ?? invite.refusal ?? revokeLink.refusal ?? revokeShare.refusal ?? unshare.refusal ?? links.refusal ?? access.refusal;
  const shownError = thrown ?? (refusal ? refusalText(refusal) : null);
  const busy =
    mint.committing || invite.committing || revokeLink.committing || revokeShare.committing || unshare.committing;

  const run = async (act: () => Promise<void>) => {
    // A new action starts clean: the last one's refusal is not this one's.
    for (const hook of [mint, invite, revokeLink, revokeShare, unshare]) hook.reset();
    setThrown(null);
    try {
      await act();
    } catch (e) {
      setThrown(message(e));
    }
  };

  return (
    <div className="share">
      <h3>Share this note</h3>

      <section aria-label="Anyone with the link can read">
        <h4>Anyone with the link can read</h4>
        <p className="hint">
          Whoever holds the link reads this note and only this note, with no account and no sign-in. They cannot change
          it. The link stops working when it expires or when you revoke it.
        </p>
        <button
          disabled={busy || !online}
          onClick={() =>
            void run(async () => {
              const out = await mint.commit(shareLinkParams(note));
              if (out.status !== 'committed') return;
              setMade(madeLinkOf(out.response.rows[0], window.location.origin));
              links.refetch();
            })
          }
        >
          Make a link anyone can read
        </button>
        {made !== null && (
          <div className="link">
            <input
              readOnly
              value={made.url}
              aria-label="Link anyone can read"
              onFocus={(e) => e.currentTarget.select()}
            />
            <button onClick={() => void navigator.clipboard?.writeText(made.url)}>Copy</button>
            <p className="hint">
              This is the only time the link is shown. It works until {when(made.expiresAt)}. Send it the way you would
              send a password.
            </p>
          </div>
        )}
        <h5>Links to this note</h5>
        {readLinks.length === 0 && <p className="hint">No links yet.</p>}
        <ul className="readers" aria-label="Links to this note">
          {readLinks.map((l) => (
            <li key={l.id}>
              <span>
                {l.state === 'live' && `Live until ${when(l.expiresAt)}`}
                {l.state === 'expired' && `Expired ${when(l.expiresAt)}`}
                {l.state === 'revoked' && 'Revoked'}
                {`, opened ${l.opens} ${l.opens === 1 ? 'time' : 'times'}`}
              </span>
              {l.state === 'live' && (
                <button
                  disabled={busy || !online}
                  onClick={() =>
                    void run(async () => {
                      const out = await revokeLink.commit({ link: l.id });
                      if (out.status === 'committed') links.refetch();
                    })
                  }
                >
                  Revoke link
                </button>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section aria-label="Share with a person">
        <h4>Share with a person</h4>
        <p className="hint">
          The person signs in with a passkey and agrees to show you their name. You then see who reads this note, and
          you can stop at any time.
        </p>
        <button
          disabled={busy || !online}
          onClick={() =>
            void run(async () => {
              const out = await invite.commit(shareParams(note));
              if (out.status !== 'committed') return;
              setSent(linkOfShare(out.response.rows[0], window.location.origin).url);
            })
          }
        >
          Make an invitation for a person
        </button>
        {sent !== null && (
          <div className="link">
            <input
              readOnly
              value={sent}
              aria-label="Invitation to send"
              onFocus={(e) => e.currentTarget.select()}
            />
            <button onClick={() => void navigator.clipboard?.writeText(sent)}>Copy</button>
            <p className="hint">
              Send it the way you would send a password: it is a secret, and whoever opens it first with their own
              passkey gets to read the note.
            </p>
          </div>
        )}
        <h5>Who can read it</h5>
        {readers.length === 0 && <p className="hint">Nobody yet.</p>}
        <ul className="readers">
          {readers.map((r, i) => (
            <li key={r.member}>
              <span>Reader {i + 1}</span>
              <button
                disabled={busy || !online}
                onClick={() =>
                  void run(async () => {
                    const out = await unshare.commit({ ...bind('unshareNote'), member: r.member });
                    if (out.status === 'committed') access.refetch();
                  })
                }
              >
                Stop sharing this note
              </button>
              <button
                disabled={busy || !online}
                onClick={() =>
                  void run(async () => {
                    const out = await revokeShare.commit({ seat: r.member });
                    if (out.status === 'committed') access.refetch();
                  })
                }
              >
                Remove from all my notes
              </button>
            </li>
          ))}
        </ul>
      </section>

      {!online && <p className="hint">Sharing needs the network.</p>}
      {shownError !== null && (
        <p role="alert" className="error">
          {shownError}
        </p>
      )}
    </div>
  );
}
