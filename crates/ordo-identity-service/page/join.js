// Sign up by invite link (plan 5.3): a name, a passkey, ten recovery codes
// shown once. The invite secret rides in the link's fragment, so no server
// log sees it; it leaves the address bar once sign-up is done.

import { $, BASE, busy, needPasskeys, showCodes, step } from './common.js';
import { passkeysAvailable, register } from './webauthn.js';

const secret = location.hash.slice(1);

function start() {
  if (!secret) {
    step('no-link');
    return;
  }
  step('name');
  needPasskeys(passkeysAvailable());
  const form = $('join-form');
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
    const joined = await busy(
      form,
      'Follow the passkey prompt from your browser.',
      () => register('/join/begin', '/join/finish', { secret, displayName }),
      'join',
    );
    if (!joined) return;
    history.replaceState(null, '', location.pathname);
    step('codes');
    await showCodes(document.querySelector('section[data-step="codes"]'), joined.recoveryCodes);
    location.assign(BASE + '/account');
  });
}

start();
