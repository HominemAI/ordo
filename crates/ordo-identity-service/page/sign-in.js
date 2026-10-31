// Sign in (plan 5.4): a passkey, or a recovery code. A recovery code opens
// a session that may only make a new passkey (the server holds to this);
// making one retires the old passkeys and issues new codes, shown once.
//
// `?client=console&return=<url>` sends the browser back once signed in.
// Only this origin and the origins the service lists
// (ORDO_IDENTITY_CLIENT_ORIGINS) are honoured; anything else goes to the
// account page.

import { $, BASE, busy, explain, me, needPasskeys, post, problem, returnTo, showCodes, step } from './common.js';
import { assert, passkeysAvailable, register } from './webauthn.js';

const params = new URLSearchParams(location.search);
const client = params.get('client') === 'console' ? 'console' : undefined;

function done() {
  location.replace(returnTo(BASE + '/account'));
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
  if (who) {
    done();
    return;
  }
  step('sign-in');
  needPasskeys(passkeysAvailable());

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

start();
