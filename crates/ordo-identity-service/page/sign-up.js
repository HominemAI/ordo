// Open sign-up into one app's deployment (owner, 2026-11-17):
// `?deployment=<handle>&return=<url>`. A person with no account makes one
// with a passkey and gets ten recovery codes, shown once. A person already
// signed in joins with the account they hold. Either way the browser then
// goes back to `return` when the service says that deployment lists it
// (POST /sign-in/return), else to the account page.
//
// The service refuses a deployment that takes people by invite; the page
// says so.

import { $, BASE, busy, explain, leave, me, needPasskeys, post, problem, showCodes, step } from './common.js';
import { passkeysAvailable, register } from './webauthn.js';

const params = new URLSearchParams(location.search);
const deployment = params.get('deployment') || '';

function nameApp() {
  for (const el of document.querySelectorAll('[data-app]')) el.textContent = deployment;
}

/** The sign-in page, coming back here signed in to join. */
function linkSignIn() {
  const back = location.pathname.slice(BASE.length) + location.search;
  const q = new URLSearchParams({ deployment, return: back });
  $('sign-in-link').href = `${BASE}/sign-in?${q}`;
}

function create() {
  step('name');
  needPasskeys(passkeysAvailable());
  linkSignIn();
  const form = $('sign-up-form');
  const name = $('display-name');
  name.focus();
  name.addEventListener('input', () => {
    if (name.value.trim()) name.removeAttribute('aria-invalid');
  });
  form.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const displayName = name.value.trim();
    if (!displayName) {
      name.setAttribute('aria-invalid', 'true');
      name.focus();
      return;
    }
    name.removeAttribute('aria-invalid');
    const made = await busy(
      form,
      'Follow the passkey prompt from your browser.',
      () => register('/sign-up/begin', '/sign-up/finish', { deployment, displayName }),
      'sign-up',
    );
    if (!made) return;
    step('codes');
    await showCodes(document.querySelector('section[data-step="codes"]'), made.recoveryCodes);
    await leave(deployment);
  });
}

function join(who) {
  step('join');
  for (const el of document.querySelectorAll('[data-who]')) el.textContent = who.displayName;
  const area = $('join-area');
  $('join').addEventListener('click', async () => {
    const joined = await busy(area, 'Joining.', () => post('/sign-up/join', { deployment }), 'sign-up');
    if (joined) await leave(deployment);
  });
}

async function start() {
  if (!deployment) {
    step('no-app');
    return;
  }
  nameApp();
  let who = null;
  try {
    who = await me();
  } catch (e) {
    problem(explain(e));
  }
  if (who && !who.mustEnrol) join(who);
  else create();
}

start();
