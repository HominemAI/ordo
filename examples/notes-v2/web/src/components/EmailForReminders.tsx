import { useAccountEmail, type AccountEmailError } from '@hominemai/ordo-react';
import { useState } from 'react';

interface Props {
  /** The door's base URL. */
  readonly baseUrl: string;
  /** The person's current token. */
  readonly token: () => Promise<string>;
  /** For tests. */
  readonly fetch?: typeof globalThis.fetch;
}

/** What to tell a person when a step failed. */
function problemText(p: AccountEmailError, waitingForCode: boolean): string {
  switch (p.problem) {
    case 'refused':
      return waitingForCode
        ? 'That code was not accepted. It may be wrong, used or too old. Send a new code.'
        : 'That did not work. Sign in again and try once more.';
    case 'email_unavailable':
      return `This deployment sends no mail, so no address can be added. The deployment owner turns mail on with ${p.setting ?? 'ORDO_MAIL_KIND'}.`;
    case 'unavailable':
      return 'The deployment cannot answer right now. Try again in a moment.';
    case 'malformed':
      return 'The deployment answered something this page could not read.';
  }
}

/**
 * Email for reminders (owner ruling 2026-10-22): a person who signs in by
 * passkey or an outside connection adds an address, so a reminder reaches
 * them by email too. It counts once they type the code mailed to it.
 * Changing it tells the old address. When the app requires an address, the
 * section says so.
 */
export function EmailForReminders({ baseUrl, token, fetch }: Props) {
  const mail = useAccountEmail({ baseUrl, token, ...(fetch ? { fetch } : {}) });
  const [address, setAddress] = useState('');
  const [code, setCode] = useState('');
  const [editing, setEditing] = useState(false);
  const current = mail.email?.address ?? null;
  const fromSignIn = mail.email?.source === 'sign-in';

  if (mail.email === null && mail.problem === null) return null;
  if (mail.email !== null && !mail.email.mail) {
    return (
      <section className="card" aria-labelledby="email-title">
        <h2 id="email-title">Email for reminders</h2>
        <p className="hint">This deployment sends no mail. Reminders show here, in the app.</p>
      </section>
    );
  }

  return (
    <section className="card" aria-labelledby="email-title">
      <h2 id="email-title">Email for reminders</h2>
      {mail.needed && (
        <p className="banner" role="status">
          This app needs an email address on your account before some actions work. Add one below.
        </p>
      )}
      {current !== null && !editing && mail.pending === null ? (
        <>
          <p className="hint" data-testid="email-current">
            Reminders are emailed to <strong>{current}</strong>
            {fromSignIn ? ', the address you sign in with.' : '.'}
          </p>
          <div className="row">
            <button type="button" disabled={mail.busy} onClick={() => setEditing(true)}>
              Change
            </button>
            {!fromSignIn && (
              <button type="button" disabled={mail.busy} onClick={() => void mail.remove()}>
                Remove
              </button>
            )}
          </div>
        </>
      ) : mail.pending === null ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void mail.begin(address);
          }}
        >
          <p className="hint">
            {current === null
              ? 'Add an address and reminders reach you by email as well. We email you a code to check it is yours.'
              : 'We email a code to the new address. Your current one is told about the change.'}
          </p>
          <label htmlFor="reminder-email">Email address</label>
          <input
            id="reminder-email"
            name="email"
            type="email"
            autoComplete="email"
            spellCheck={false}
            required
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            disabled={mail.busy}
          />
          <div className="row">
            <button type="submit" className="primary" disabled={mail.busy || address.trim() === ''}>
              Email me a code
            </button>
            {editing && (
              <button type="button" disabled={mail.busy} onClick={() => setEditing(false)}>
                Keep the current one
              </button>
            )}
          </div>
        </form>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void mail.confirm(code).then(() => {
              setCode('');
              setEditing(false);
            });
          }}
        >
          <p className="hint" role="status">
            A code is on its way to <strong>{mail.pending}</strong>. Type it here.
          </p>
          <label htmlFor="reminder-email-code">Code</label>
          <input
            id="reminder-email-code"
            name="code"
            inputMode="numeric"
            autoComplete="one-time-code"
            required
            value={code}
            onChange={(e) => setCode(e.target.value)}
            disabled={mail.busy}
          />
          <div className="row">
            <button type="submit" className="primary" disabled={mail.busy || code.trim() === ''}>
              Use this address
            </button>
            <button type="button" disabled={mail.busy} onClick={() => mail.cancel()}>
              Start again
            </button>
          </div>
        </form>
      )}
      {mail.problem && (
        <p className="error" role="alert">
          {problemText(mail.problem, mail.pending !== null)}
        </p>
      )}
    </section>
  );
}
