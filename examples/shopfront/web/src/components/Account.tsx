import type { Subdomain } from '../subdomain.js';

/** The page's message while it looks for a session. */
export function Starting() {
  return (
    <main>
      <p className="hint">Loading.</p>
    </main>
  );
}

/** What a page shows with no session: one button that goes to the login page. */
export function SignedOut(props: { readonly title: string; readonly text: string; readonly sub: Subdomain }) {
  const { sub } = props;
  return (
    <main>
      <h1>{props.title}</h1>
      <section className="card">
        <p>{props.text}</p>
        <div className="row">
          <button type="button" className="primary" onClick={sub.signIn} disabled={sub.busy}>
            Sign in
          </button>
        </div>
        {sub.error !== null && (
          <p role="alert" className="error">
            {sub.error}
          </p>
        )}
      </section>
    </main>
  );
}

/** Who is signed in, and the way out. Signing out forgets this tab's session only. */
export function AccountStrip(props: {
  readonly name: string | null;
  readonly memberId: string | null;
  readonly onSignOut: () => void;
}) {
  return (
    <div className="strip">
      <span>
        {props.name ?? 'Signed in'}
        {props.memberId !== null && <span className="hint"> member {props.memberId}</span>}
      </span>
      <button type="button" onClick={props.onSignOut}>
        Sign out
      </button>
    </div>
  );
}

/** An error line, or nothing. */
export function Problem(props: { readonly text: string | null }) {
  return props.text === null ? null : (
    <p role="alert" className="error">
      {props.text}
    </p>
  );
}
