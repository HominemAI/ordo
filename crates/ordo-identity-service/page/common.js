// What every page of the identity service shares: the one way to call the
// service, what each refusal means to a person, steps, the problem and
// status lines, and the recovery codes panel.
//
// Everything on screen is built from elements and text. The page policy
// (require-trusted-types-for 'script') refuses HTML written from a string.

/** A call the service refused, or one that got no answer. */
export class Refusal extends Error {
  constructor(status, body) {
    super(body?.message || `the identity service answered ${status}`);
    this.name = 'Refusal';
    this.status = status;
    this.code = body?.error || (status === 0 ? 'network' : 'unknown');
    this.body = body || {};
  }
}

/**
 * Where the service is under this origin: empty at the root, or a path
 * such as `/identity` when the console passes it through on its own
 * origin. Every call and link the pages make starts with it.
 */
export const BASE = document.querySelector('meta[name="ordo-base"]')?.getAttribute('content') || '';

/** POST a JSON body to the service, on this origin, with the session cookie. */
export async function post(path, body = {}) {
  let res;
  try {
    res = await fetch(BASE + path, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Refusal(0, null);
  }
  let data = null;
  try {
    data = await res.json();
  } catch {
    data = null;
  }
  if (!res.ok) throw new Refusal(res.status, data);
  return data ?? {};
}

/** Who is signed in, or null when nobody is. */
export async function me() {
  try {
    return await post('/me');
  } catch (e) {
    if (e instanceof Refusal && (e.code === 'not_signed_in' || e.code === 'session_ended')) {
      return null;
    }
    throw e;
  }
}

/**
 * What went wrong, in words a person can act on. `when` names what was
 * being tried, for the few refusals whose meaning depends on it.
 */
export function explain(e, when = '') {
  if (e instanceof DOMException || e?.name === 'NotAllowedError') {
    switch (e.name) {
      case 'NotAllowedError':
        return 'The passkey prompt was closed or timed out. Try again when you are ready.';
      case 'InvalidStateError':
        return 'This device already holds a passkey for your account. Use another device or password manager, or close this and carry on.';
      case 'SecurityError':
        return 'This page address does not match the passkey settings. Tell the deployment owner to check ORDO_IDENTITY_ORIGIN and ORDO_IDENTITY_RP_ID.';
      case 'AbortError':
        return 'The passkey prompt was stopped. Try again.';
      case 'NotSupportedError':
        return 'This device cannot make this kind of passkey. Try another device or a security key.';
      default:
        return `The passkey did not work (${e.name}). Try again.`;
    }
  }
  if (!(e instanceof Refusal)) {
    return 'Something went wrong on this page. Reload it and try again.';
  }
  switch (e.code) {
    case 'network':
      return 'Could not reach the identity service. Check your connection and try again.';
    case 'code_refused':
      if (when === 'join') {
        return 'This link does not work. It may be used, expired or withdrawn. Ask the person who sent it for a new one.';
      }
      if (when === 'recover') {
        return 'That recovery code does not work. Check each letter, or try another of your codes.';
      }
      return 'That code does not work. Start again.';
    case 'passkey_refused':
      return when === 'sign-in'
        ? 'That passkey was not accepted here. Try another passkey, or use a recovery code.'
        : 'That passkey was not accepted. Try again with a passkey for this account.';
    case 'not_signed_in':
    case 'session_ended':
      return 'You are signed out. Sign in again to carry on.';
    case 'too_many_attempts': {
      const secs = e.body.retryAfterSecs;
      return secs
        ? `Too many tries. Wait ${secs} seconds, then try again.`
        : 'Too many tries. Wait a minute, then try again.';
    }
    case 'enrol_a_new_passkey':
      return 'You signed in with a recovery code. Make a new passkey first.';
    case 'presence_required':
      return 'This needs a fresh touch of your passkey. Try again and confirm with your passkey.';
    case 'last_passkey':
      return 'That is your only passkey. Add another passkey first, then remove this one.';
    case 'unknown_passkey':
      return 'That passkey is already gone. The list has been refreshed.';
    case 'server_busy':
    case 'try_again':
      return 'The identity service is busy. Try again in a moment.';
    case 'origin_refused':
      return `This page may not call the identity service. Tell the deployment owner: ${e.body.message || 'check ORDO_IDENTITY_CLIENT_ORIGINS'}.`;
    case 'bad_request':
      return `The service could not read that: ${e.body.message || 'bad request'}. Check what you typed and try again.`;
    default:
      return e.body.message
        ? `${capital(e.body.message)}.`
        : 'The identity service failed. Try again; if it keeps failing, tell the deployment owner.';
  }
}

function capital(s) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** The element with this id. */
export function $(id) {
  const el = document.getElementById(id);
  if (!el) throw new Error(`no #${id} on this page`);
  return el;
}

/** Show a problem in the page's alert line, which screen readers announce. */
export function problem(text) {
  // The line stays in the page, empty, so a screen reader is already
  // watching it when the text arrives.
  $('problem').textContent = text;
}

/** Say what is happening, politely, in the status line. */
export function status(text) {
  $('status').textContent = text;
}

/** Clear both lines. */
export function clear() {
  problem('');
  status('');
}

/**
 * Show one step (a `section[data-step]`) and hide the others. Focus moves
 * to its heading, so a screen reader and a keyboard start there.
 */
export function step(name) {
  for (const s of document.querySelectorAll('section[data-step]')) {
    s.hidden = s.dataset.step !== name;
  }
  const heading = document.querySelector(`section[data-step="${name}"] h1, section[data-step="${name}"] h2`);
  if (heading) {
    heading.setAttribute('tabindex', '-1');
    heading.focus();
  }
}

/**
 * Run `act` with every button in `scope` turned off, and show what went
 * wrong if it fails. Answers what `act` answers, or undefined on failure.
 */
export async function busy(scope, doing, act, when = '') {
  const buttons = [...scope.querySelectorAll('button')];
  for (const b of buttons) b.disabled = true;
  scope.setAttribute('aria-busy', 'true');
  problem('');
  status(doing);
  try {
    return await act();
  } catch (e) {
    status('');
    problem(explain(e, when));
    return undefined;
  } finally {
    for (const b of buttons) b.disabled = false;
    scope.removeAttribute('aria-busy');
  }
}

/** A date, the reader's way. */
export function day(secs) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(new Date(secs * 1000));
}

/** An element with text and attributes. */
export function el(tag, text = '', attrs = {}) {
  const e = document.createElement(tag);
  if (text) e.textContent = text;
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

/**
 * Where to go after signing in: `?return=` when it is this origin's path
 * or one of the origins the service lists, else `fallback`.
 */
export function returnTo(fallback = BASE + '/account') {
  const asked = new URLSearchParams(location.search).get('return');
  if (!asked) return fallback;
  if (asked.startsWith('/') && !asked.startsWith('//') && !asked.startsWith('/\\')) {
    return BASE && !asked.startsWith(BASE + '/') ? BASE + asked : asked;
  }
  const meta = document.querySelector('meta[name="ordo-return-origins"]');
  const allowed = (meta?.getAttribute('content') || '').split(' ').filter(Boolean);
  try {
    const url = new URL(asked);
    if (allowed.includes(url.origin)) return url.href;
  } catch {
    // not a URL: fall back
  }
  return fallback;
}

/**
 * Fill the recovery codes panel inside `root` and wait for "I saved
 * these". Resolves when the person continues. The codes leave the page
 * when it does.
 */
export function showCodes(root, codes) {
  const list = root.querySelector('[data-codes]');
  const saved = root.querySelector('[data-saved]');
  const go = root.querySelector('[data-continue]');
  const copy = root.querySelector('[data-copy]');
  const download = root.querySelector('[data-download]');
  list.replaceChildren(...codes.map((c) => el('li', c)));
  saved.checked = false;
  go.disabled = true;
  const text = `Ordo deployment recovery codes for ${location.host}\nEach code works once.\n\n${codes.join('\n')}\n`;
  copy.onclick = async () => {
    try {
      await navigator.clipboard.writeText(text);
      status('Copied the codes.');
    } catch {
      problem('Could not copy. Select the codes and copy them, or download them.');
    }
  };
  download.onclick = () => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    const a = el('a', '', { href: url, download: 'ordo-recovery-codes.txt' });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    status('Downloaded the codes. Keep the file somewhere safe.');
  };
  saved.onchange = () => {
    go.disabled = !saved.checked;
  };
  return new Promise((resolve) => {
    go.onclick = () => {
      if (!saved.checked) return;
      list.replaceChildren();
      resolve();
    };
  });
}

/** Turn the passkey button off, with a reason, on a browser without passkeys. */
export function needPasskeys(available) {
  if (available) return true;
  problem('This browser cannot use passkeys. Open this page in a current Chrome, Edge, Firefox or Safari.');
  for (const b of document.querySelectorAll('button[data-passkey]')) b.disabled = true;
  return false;
}
