// The account page: who is signed in, their passkeys (list, add, remove,
// never the last), and new recovery codes. Every change needs a passkey
// touch within the last 5 minutes; when the service answers
// `presence_required`, the page asks for one and tries once more.

import { $, BASE, Refusal, busy, day, el, me, needPasskeys, post, problem, explain, showCodes, status, step } from './common.js';
import { passkeysAvailable, register, touch } from './webauthn.js';

let who = null;

/** Run `act`; if it needs a fresh touch, get one and run it again. */
async function withPresence(act) {
  try {
    return await act();
  } catch (e) {
    if (!(e instanceof Refusal) || e.code !== 'presence_required') throw e;
    status('Confirm with your passkey.');
    await touch();
    return act();
  }
}

/** Tell the passkey provider which passkeys still work (where supported). */
async function signalAccepted(keys) {
  const signal = window.PublicKeyCredential?.signalAllAcceptedCredentials;
  if (typeof signal !== 'function') return;
  const byHandle = new Map();
  for (const k of keys) {
    if (!byHandle.has(k.userHandle)) byHandle.set(k.userHandle, []);
    byHandle.get(k.userHandle).push(k.id);
  }
  for (const [userId, allAcceptedCredentialIds] of byHandle) {
    try {
      await signal.call(PublicKeyCredential, { rpId: who.rpId, userId, allAcceptedCredentialIds });
    } catch {
      // A hint to the provider only; the service has already removed it.
    }
  }
}

function describe(k) {
  const parts = [`Added ${day(k.registeredAt)}`];
  parts.push(k.lastUsedAt ? `last used ${day(k.lastUsedAt)}` : 'not used to sign in yet');
  parts.push(k.backedUp ? 'synced across your devices' : 'on one device only');
  return parts.join(', ');
}

function render() {
  $('name').textContent = who.displayName;
  $('person').textContent = `(${who.person})`;
  const list = $('passkeys');
  const keys = who.passkeys;
  const only = keys.length < 2;
  list.replaceChildren(
    ...keys.map((k, i) => {
      const label = `Passkey ${i + 1}`;
      const item = el('li');
      const text = el('div');
      text.append(el('strong', label), el('p', describe(k), { class: 'quiet' }));
      item.append(text);
      if (!only) {
        const remove = el('button', 'Remove', {
          type: 'button',
          class: 'secondary',
          'aria-label': `Remove ${label}, ${describe(k)}`,
        });
        remove.addEventListener('click', () => confirmRemove(k, label, remove));
        item.append(remove);
      }
      return item;
    }),
  );
  $('only-one').hidden = !only;
  const left = who.recoveryCodesLeft;
  $('codes-left').textContent =
    left === 0
      ? 'You have no recovery codes left. Make new ones now.'
      : `You have ${left} unused ${left === 1 ? 'code' : 'codes'} left.`;
}

async function load() {
  try {
    who = await me();
  } catch (e) {
    problem(explain(e));
    return false;
  }
  if (!who) {
    location.replace(BASE + '/sign-in?return=%2Faccount');
    return false;
  }
  if (who.mustEnrol) {
    location.replace(BASE + '/sign-in');
    return false;
  }
  render();
  return true;
}

function confirmRemove(k, label, opener) {
  const dialog = $('confirm-remove');
  $('confirm-which').textContent = `${label}: ${describe(k)}.`;
  dialog.returnValue = '';
  dialog.addEventListener(
    'close',
    async () => {
      opener.focus();
      if (dialog.returnValue !== 'remove') return;
      const removed = await busy(
        $('passkeys'),
        'Removing the passkey.',
        () => withPresence(() => post(`/passkeys/${encodeURIComponent(k.id)}/remove`)),
      );
      if (!removed) {
        await load();
        return;
      }
      if (await load()) {
        await signalAccepted(who.passkeys);
        status(`Removed ${label}.`);
        $('add').focus();
      }
    },
    { once: true },
  );
  dialog.showModal();
}

async function start() {
  if (!(await load())) return;
  step('account');
  needPasskeys(passkeysAvailable());

  $('sign-out').addEventListener('click', async () => {
    const out = await busy($('signed-in'), 'Signing out.', () => post('/sessions/end'));
    if (out) location.replace(BASE + '/sign-in');
  });

  $('add').addEventListener('click', async () => {
    const added = await busy($('add-area'), 'Follow the passkey prompt from your browser.', () =>
      withPresence(() => register('/passkeys/begin', '/passkeys/finish')),
    );
    if (added && (await load())) status('Added a passkey.');
  });

  $('renew').addEventListener('click', async () => {
    const made = await busy($('renew-area'), 'Confirm with your passkey.', () =>
      withPresence(() => post('/recovery-codes')),
    );
    if (!made) return;
    step('codes');
    await showCodes(document.querySelector('section[data-step="codes"]'), made.recoveryCodes);
    step('account');
    if (await load()) status('Your new recovery codes are saved. The old ones no longer work.');
  });
}

start();
