import { useEffect, useRef, useState, type FormEvent } from 'react';
import { DOOR, TURNSTILE_SITE_KEY } from './config.js';

type Row = Record<string, unknown>;

const str = (v: unknown): string => (typeof v === 'string' ? v : v == null ? '' : String(v));
const list = (v: unknown): string[] => (Array.isArray(v) ? v.map(str) : []);

/** A money amount as it comes off the wire, shown with two places. */
function money(v: unknown): string {
  const n = Number(str(v));
  return Number.isFinite(n) ? `$${n.toFixed(2)}` : str(v);
}

type Read = { readonly rows: Row[]; readonly loading: boolean; readonly failed: boolean };

/**
 * A public read: `GET <door>/op/<name>` with no token and no space header.
 * The door answers `{ "op": ..., "rows": [...] }`.
 */
function usePublicRead(op: string): Read {
  const [state, setState] = useState<Read>({ rows: [], loading: true, failed: false });
  useEffect(() => {
    let alive = true;
    fetch(`${DOOR}/op/${op}`, { headers: { Accept: 'application/json' } })
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then(
        (body: { rows?: Row[] }) => alive && setState({ rows: body.rows ?? [], loading: false, failed: false }),
        () => alive && setState({ rows: [], loading: false, failed: true }),
      );
    return () => {
      alive = false;
    };
  }, [op]);
  return state;
}

/**
 * The landing page. It reads four public ops with no token and sends the
 * contact form to one public write. There is no sign-in anywhere in this app.
 */
export function App() {
  return (
    <>
      <header className="hero">
        <h1>Lumen Studio</h1>
        <p>Calm tools for small teams. Everything on this page is read live from an Ordo database.</p>
        <a className="button" href="#pricing">
          See pricing
        </a>
      </header>
      <main>
        <Products />
        <Pricing />
        <Testimonials />
        <Contact />
      </main>
      <footer>Synthesized demo content. No account needed to read it.</footer>
    </>
  );
}

function Status({ q, what }: { q: Read; what: string }) {
  if (q.failed) {
    return (
      <p role="alert" className="error">
        {what} could not be read.
      </p>
    );
  }
  return q.loading ? <p className="hint">Loading.</p> : null;
}

function Products() {
  const q = usePublicRead('catalogue');
  return (
    <section id="products">
      <h2>Products</h2>
      <Status q={q} what="The catalogue" />
      <div className="grid">
        {q.rows.map((r) => (
          <article key={str(r['id'])} className="card">
            <h3>{str(r['name'])}</h3>
            <p className="tag">{str(r['tagline'])}</p>
            <p>{str(r['description'])}</p>
            <ul>
              {list(r['features']).map((f) => (
                <li key={f}>{f}</li>
              ))}
            </ul>
            <p className="price">{money(r['price'])}</p>
          </article>
        ))}
      </div>
    </section>
  );
}

function Pricing() {
  const q = usePublicRead('pricing');
  return (
    <section id="pricing">
      <h2>Pricing</h2>
      <Status q={q} what="Pricing" />
      <div className="grid">
        {q.rows.map((r) => (
          <article key={str(r['id'])} className={r['highlighted'] === true ? 'card plan best' : 'card plan'}>
            <h3>{str(r['name'])}</h3>
            <p className="tag">{str(r['tagline'])}</p>
            <p className="price">
              {money(r['price'])} <span>/ {str(r['period'])}</span>
            </p>
            <ul>
              {list(r['features']).map((f) => (
                <li key={f}>{f}</li>
              ))}
            </ul>
          </article>
        ))}
      </div>
    </section>
  );
}

function Testimonials() {
  const q = usePublicRead('reviews');
  return (
    <section id="testimonials">
      <h2>What people say</h2>
      <Status q={q} what="Testimonials" />
      <div className="grid">
        {q.rows.map((r) => (
          <blockquote key={str(r['id'])} className="card">
            <p>{str(r['quote'])}</p>
            <footer>
              {'★'.repeat(Number(r['stars']) || 0)} {str(r['author'])}, {str(r['role'])}
              {r['company'] ? `, ${str(r['company'])}` : ''}
            </footer>
          </blockquote>
        ))}
      </div>
    </section>
  );
}

/** The bit of the Turnstile widget script this page uses. */
type Turnstile = {
  render: (el: HTMLElement, options: { sitekey: string; callback: (token: string) => void }) => string;
  reset: (id?: string) => void;
};
declare global {
  interface Window {
    turnstile?: Turnstile;
  }
}

const TURNSTILE_SCRIPT = 'https://challenges.cloudflare.com/turnstile/v0/api.js';

/** What to tell the visitor for a refusal the door named. */
function refusalText(status: number, code: string): string {
  if (code === 'human_check_failed') return 'The human check did not pass. Please try again.';
  if (code === 'quota_exceeded') return 'Too many messages have been sent just now. Please try again later.';
  return `The message was not sent (${status}).`;
}

type Sent = { readonly kind: 'idle' } | { readonly kind: 'sending' } | { readonly kind: 'sent'; readonly id: string } | { readonly kind: 'error'; readonly text: string };

/**
 * The contact form: one public write, `contact`. The visitor gets back the id
 * of the new row and can read nothing. With a Turnstile site key the widget
 * runs and its token goes in `X-Ordo-Human`; without one no header is sent.
 */
function Contact() {
  const [sent, setSent] = useState<Sent>({ kind: 'idle' });
  const box = useRef<HTMLDivElement>(null);
  const human = useRef('');

  useEffect(() => {
    if (!TURNSTILE_SITE_KEY || !box.current) return;
    const el = box.current;
    const show = () => window.turnstile?.render(el, { sitekey: TURNSTILE_SITE_KEY, callback: (t) => (human.current = t) });
    if (window.turnstile) {
      show();
      return;
    }
    const script = document.createElement('script');
    script.src = TURNSTILE_SCRIPT;
    script.async = true;
    script.onload = show;
    document.head.appendChild(script);
  }, []);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const data = new FormData(form);
    const headers: Record<string, string> = { 'Content-Type': 'application/json', Accept: 'application/json' };
    if (TURNSTILE_SITE_KEY) headers['X-Ordo-Human'] = human.current;
    setSent({ kind: 'sending' });
    try {
      const r = await fetch(`${DOOR}/op/contact`, {
        method: 'POST',
        headers,
        body: JSON.stringify({
          name: str(data.get('name')),
          email: str(data.get('email')),
          company: str(data.get('company')),
          message: str(data.get('message')),
        }),
      });
      const body = await r.json().catch(() => null);
      if (!r.ok) {
        setSent({ kind: 'error', text: refusalText(r.status, str(body?.error?.code)) });
      } else {
        setSent({ kind: 'sent', id: str(body?.rows?.[0]?.created) });
        form.reset();
      }
    } catch {
      setSent({ kind: 'error', text: 'The message was not sent. The server could not be reached.' });
    }
    // A Turnstile token works once.
    human.current = '';
    window.turnstile?.reset();
  }

  return (
    <section id="contact">
      <h2>Contact us</h2>
      <form className="card form" onSubmit={submit}>
        <label>
          Name
          <input name="name" required maxLength={120} />
        </label>
        <label>
          Email
          <input name="email" type="email" required maxLength={200} />
        </label>
        <label>
          Company (optional)
          <input name="company" maxLength={120} />
        </label>
        <label>
          Message
          <textarea name="message" required maxLength={2000} rows={5} />
        </label>
        {TURNSTILE_SITE_KEY && <div ref={box} />}
        <button className="button" type="submit" disabled={sent.kind === 'sending'}>
          {sent.kind === 'sending' ? 'Sending.' : 'Send'}
        </button>
        {sent.kind === 'sent' && (
          <p role="status" className="hint">
            Thank you. Your message is received (reference {sent.id}).
          </p>
        )}
        {sent.kind === 'error' && (
          <p role="alert" className="error">
            {sent.text}
          </p>
        )}
      </form>
    </section>
  );
}
