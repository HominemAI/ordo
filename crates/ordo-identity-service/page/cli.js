// Approve `ordo login` (plan 5.5). The CLI opened this page with
// `?port=<loopback port>&state=<its state>&code_challenge=<PKCE S256>`.
//
// Approving needs a passkey at that moment: a touch inside the session,
// or a sign-in when there is none. Then the service mints a 60 second code
// bound to the PKCE challenge, and the page sends the browser to
// http://127.0.0.1:<port>/callback?code=…&state=…, on this computer only:
// the host is fixed here, never read from the link. Deny stays on this
// page and sends nothing anywhere: when the person did not start the
// request there is nothing to tell, and a waiting CLI gives up on its own.
//
// This page is a phishing target: a stranger can send anyone a link to it.
// It says plainly what approving does and to approve only a request the
// person just started.

import { $, busy, explain, me, needPasskeys, post, problem, step } from './common.js';
import { assert, passkeysAvailable, touch } from './webauthn.js';

const params = new URLSearchParams(location.search);
const port = Number(params.get('port'));
const state = params.get('state') || '';
const challenge = params.get('code_challenge') || '';

function wellFormed() {
  return (
    Number.isInteger(port) &&
    port >= 1024 &&
    port <= 65535 &&
    /^[A-Za-z0-9._~-]{8,128}$/.test(state) &&
    /^[A-Za-z0-9_-]{43}$/.test(challenge)
  );
}

function callback(code) {
  const q = new URLSearchParams({ code, state });
  return `http://127.0.0.1:${port}/callback?${q}`;
}

function hours(h) {
  return h % 24 === 0 ? `${h / 24} days` : `${h} hours`;
}

async function start() {
  if (!wellFormed()) {
    step('bad');
    return;
  }
  let who = null;
  try {
    who = await me();
  } catch (e) {
    problem(explain(e));
  }
  if (who && who.mustEnrol) {
    step('must-enrol');
    return;
  }
  $('port').textContent = String(port);
  if (who) {
    $('who').textContent = `${who.displayName} (${who.person}), after you confirm with your passkey`;
    const grant = $('grant');
    grant.append(
      ` Unused for ${hours(who.sessionIdleHours)}, it ends; it ends after ${hours(who.sessionMaxHours)} at most.`,
    );
  }
  step('ask');
  needPasskeys(passkeysAvailable());

  const answer = $('answer');
  $('approve').addEventListener('click', async () => {
    const code = await busy(
      answer,
      'Confirm with your passkey.',
      async () => {
        if (who) {
          await touch();
        } else {
          await assert('/sign-in/begin', '/sign-in/finish');
        }
        const granted = await post('/cli/approve', { codeChallenge: challenge });
        return granted.code;
      },
      who ? '' : 'sign-in',
    );
    if (!code) return;
    step('approved');
    location.assign(callback(code));
  });
  $('deny').addEventListener('click', () => {
    step('denied');
  });
}

start();
