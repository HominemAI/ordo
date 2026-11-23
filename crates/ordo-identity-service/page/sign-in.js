// Sign in (plan 5.4): a passkey, or a recovery code. A recovery code opens
// a session that may only make a new passkey (the server holds to this);
// making one retires the old passkeys and issues new codes, shown once.
//
// `?client=console&return=<url>&deployment=<handle>` sends the browser
// back once signed in. The service checks the address against that
// deployment's return URLs (POST /sign-in/return): its own clients
// (ORDO_IDENTITY_CLIENT_ORIGINS), or a registered deployment's list.
// Anything else goes to the account page.

import { $, BASE, busy, explain, leave, me, needPasskeys, post, problem, showCodes, step } from './common.js';
import { assert, passkeysAvailable, register, touch } from './webauthn.js';

const params = new URLSearchParams(location.search);
const client = params.get('client') === 'console' ? 'console' : undefined;
const wantsTouch = params.get('touch') === '1';

async function confirm() {
  step('touch');
  needPasskeys(passkeysAvailable());
  const area = $('touch-area');
  // A click starts the ceremony: Safari needs a user gesture for it.
  $('touch').addEventListener('click', async () => {
    let gone = false;
    const touched = await busy(
      area,
      'Follow the passkey prompt from your browser.',
      () =>
        touch().catch((e) => {
          // The session ended since the page loaded: sign in instead.
          if (e?.code === 'not_signed_in' || e?.code === 'session_ended') gone = true;
          throw e;
        }),
      'touch',
    );
    if (touched) done();
    else if (gone) await signIn();
  });
}

const deployment = params.get('deployment') || undefined;

function done() {
  return leave(deployment);
}

async function enrol() {
  step('enrol');
  needPasskeys(passkeysAvailable());
  const area = $('enrol-area');
  $('enrol').addEventListener('click', async () => {
    const made = await busy(
      area,
      'Follow the passkey prompt from your browser.',
      () => register('/passkeys/begin', '/passkeys/finish'),
    );
    if (!made) return;
    if (made.recoveryCodes) {
      step('codes');
      await showCodes(document.querySelector('section[data-step="codes"]'), made.recoveryCodes);
    }
    done();
  });
}

/**
 * Signing in to an app through the service (its OIDC sign-in sends
 * `deployment`, and `signUp=open` when the app takes anyone): name the app,
 * and link to making an account. The service still decides who may join.
 */
function forApp() {
  if (!deployment) return;
  $('sign-in-intro').textContent = `Sign in to ${deployment}. Use the passkey you made when you joined.`;
  if (params.get('signUp') !== 'open') return;
  const q = new URLSearchParams({ deployment });
  const back = params.get('return');
  if (back) q.set('return', back);
  $('sign-up-link').href = `${BASE}/sign-up?${q}`;
  $('sign-up-line').hidden = false;
}

/** The sign-in step: a passkey, or a recovery code. */
async function signIn() {
  step('sign-in');
  needPasskeys(passkeysAvailable());
  forApp();

  const area = $('passkey-area');
  $('passkey').addEventListener('click', async () => {
    const signed = await busy(
      area,
      'Follow the passkey prompt from your browser.',
      () => assert('/sign-in/begin', '/sign-in/finish', client ? { client } : {}),
      'sign-in',
    );
    if (signed) done();
  });

  const form = $('recover-form');
  const code = $('code');
  code.addEventListener('input', () => {
    if (code.value.trim()) code.removeAttribute('aria-invalid');
  });
  form.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const typed = code.value.trim().toUpperCase();
    if (!typed) {
      code.setAttribute('aria-invalid', 'true');
      code.focus();
      return;
    }
    const opened = await busy(
      form,
      'Checking the code.',
      () => post('/recover', { code: typed }),
      'recover',
    );
    if (!opened) return;
    code.value = '';
    await enrol();
  });
}

async function start() {
  let who = null;
  try {
    who = await me();
  } catch (e) {
    problem(explain(e));
  }
  if (who && who.mustEnrol) {
    await enrol();
    return;
  }
  if (who && wantsTouch) {
    await confirm();
    return;
  }
  if (who) {
    done();
    return;
  }
  await signIn();
}

start();
