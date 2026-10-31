// The passkey ceremonies, with the browser's own WebAuthn API.
//
// Vendored from the console page's ceremony code
// (clients/ordo-console-web/src/webauthn.ts), written out as plain
// JavaScript because this page has no build step. The identity service
// answers with the same option shapes as the console
// (ordo_identity::passkey::creation_json and request_json), so the same
// code reads them. The SDK's ceremony (clients/ordo-sdk/src/auth/passkey.ts)
// is bound to the door's /auth routes and its client classes, so it is not
// the one used here.
//
// Two rules make a ceremony verify:
// - clientDataJSON goes back as the exact bytes the browser made, in
//   base64url, never parsed and written again;
// - the challenge string the service sent is posted back as it came (it
//   names the ceremony), and its decoded bytes are what the authenticator
//   signs.

import { post } from './common.js';

function fromB64url(s) {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

export function toB64url(buf) {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function descriptors(list) {
  return (list ?? []).map((d) => ({ type: 'public-key', id: fromB64url(d.id) }));
}

// The browser's own JSON readers where they exist; a few lines in their
// place for older browsers.
function creationOptions(o) {
  if (typeof PublicKeyCredential.parseCreationOptionsFromJSON === 'function') {
    return PublicKeyCredential.parseCreationOptionsFromJSON(o);
  }
  return {
    ...o,
    challenge: fromB64url(o.challenge),
    user: { ...o.user, id: fromB64url(o.user.id) },
    excludeCredentials: descriptors(o.excludeCredentials),
  };
}

function requestOptions(o) {
  if (typeof PublicKeyCredential.parseRequestOptionsFromJSON === 'function') {
    return PublicKeyCredential.parseRequestOptionsFromJSON(o);
  }
  return {
    ...o,
    challenge: fromB64url(o.challenge),
    allowCredentials: descriptors(o.allowCredentials),
  };
}

/** Whether this browser can make and use passkeys at all. */
export function passkeysAvailable() {
  return typeof window.PublicKeyCredential === 'function' && !!navigator.credentials;
}

/**
 * Run a registration: `beginPath` answers the options, then `finishPath`
 * gets the new credential with `extra` fields beside it.
 */
export async function register(beginPath, finishPath, extra = {}) {
  const begun = await post(beginPath, extra);
  const publicKey = creationOptions(begun.publicKey);
  const cred = await navigator.credentials.create({ publicKey });
  if (!cred) throw new DOMException('No passkey was made.', 'NotAllowedError');
  const r = cred.response;
  return post(finishPath, {
    ...extra,
    challenge: begun.publicKey.challenge,
    clientDataJSON: toB64url(r.clientDataJSON),
    attestationObject: toB64url(r.attestationObject),
  });
}

/** Run an assertion: a sign-in or a fresh touch. */
export async function assert(beginPath, finishPath, extra = {}) {
  const begun = await post(beginPath, extra);
  const publicKey = requestOptions(begun.publicKey);
  const cred = await navigator.credentials.get({ publicKey });
  if (!cred) throw new DOMException('No passkey was used.', 'NotAllowedError');
  const r = cred.response;
  return post(finishPath, {
    ...extra,
    challenge: begun.publicKey.challenge,
    credentialId: toB64url(cred.rawId),
    clientDataJSON: toB64url(r.clientDataJSON),
    authenticatorData: toB64url(r.authenticatorData),
    signature: toB64url(r.signature),
  });
}

/** A fresh touch inside the signed-in session. */
export function touch() {
  return assert('/touch/begin', '/touch/finish');
}
