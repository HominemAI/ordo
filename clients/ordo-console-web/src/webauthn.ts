/**
 * The passkey ceremonies, with the browser's own WebAuthn API.
 *
 * The console sends options as WebAuthn JSON (base64url fields). The
 * browser's `parseCreationOptionsFromJSON` / `parseRequestOptionsFromJSON`
 * read them where they exist; a few lines below do the same for older
 * browsers, in place of a polyfill package. Answers go back as base64url
 * fields, with the challenge the console handed out, which is how it finds
 * the ceremony again.
 */

import { post } from './api';

type Json = Record<string, unknown>;

function fromB64url(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

export function toB64url(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

interface Descriptor {
  type: 'public-key';
  id: string;
}

function descriptors(list: unknown): PublicKeyCredentialDescriptor[] {
  return ((list as Descriptor[] | undefined) ?? []).map((d) => ({ type: 'public-key', id: fromB64url(d.id) }));
}

interface ParseJson {
  parseCreationOptionsFromJSON?: (o: Json) => PublicKeyCredentialCreationOptions;
  parseRequestOptionsFromJSON?: (o: Json) => PublicKeyCredentialRequestOptions;
}

function creationOptions(o: Json): PublicKeyCredentialCreationOptions {
  const native = (PublicKeyCredential as unknown as ParseJson).parseCreationOptionsFromJSON;
  if (native) return native(o);
  const user = o['user'] as { id: string; name: string; displayName: string };
  return {
    ...(o as unknown as PublicKeyCredentialCreationOptions),
    challenge: fromB64url(o['challenge'] as string),
    user: { ...user, id: fromB64url(user.id) },
    excludeCredentials: descriptors(o['excludeCredentials']),
  };
}

function requestOptions(o: Json): PublicKeyCredentialRequestOptions {
  const native = (PublicKeyCredential as unknown as ParseJson).parseRequestOptionsFromJSON;
  if (native) return native(o);
  return {
    ...(o as unknown as PublicKeyCredentialRequestOptions),
    challenge: fromB64url(o['challenge'] as string),
    allowCredentials: descriptors(o['allowCredentials']),
  };
}

/** Whether this browser can make and use passkeys at all. */
export function passkeysAvailable(): boolean {
  return typeof window.PublicKeyCredential === 'function' && !!navigator.credentials;
}

/**
 * Run a registration: `begin` answers the options, then `finish` gets the
 * new credential with `extra` fields (a setup code, a label) beside it.
 */
export async function register<T>(beginPath: string, finishPath: string, extra: Json = {}): Promise<T> {
  const begun = await post<{ publicKey: Json }>(beginPath, extra);
  const publicKey = creationOptions(begun.publicKey);
  const cred = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential | null;
  if (!cred) throw new DOMException('No passkey was made.', 'NotAllowedError');
  const r = cred.response as AuthenticatorAttestationResponse;
  return post<T>(finishPath, {
    ...extra,
    challenge: begun.publicKey['challenge'],
    clientDataJSON: toB64url(r.clientDataJSON),
    attestationObject: toB64url(r.attestationObject),
  });
}

/** Run an assertion against the console: sign-in or a fresh touch. */
export function assert<T>(beginPath: string, finishPath: string): Promise<T> {
  return assertWith<T>(post, beginPath, finishPath);
}

/**
 * Run an assertion through `send`: the console's own API, or the identity
 * service's touch routes (`identity.ts`).
 */
export async function assertWith<T>(
  send: <R>(path: string, body?: unknown) => Promise<R>,
  beginPath: string,
  finishPath: string,
): Promise<T> {
  const begun = await send<{ publicKey: Json }>(beginPath);
  const publicKey = requestOptions(begun.publicKey);
  const cred = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential | null;
  if (!cred) throw new DOMException('No passkey was used.', 'NotAllowedError');
  const r = cred.response as AuthenticatorAssertionResponse;
  return send<T>(finishPath, {
    challenge: begun.publicKey['challenge'],
    credentialId: toB64url(cred.rawId),
    clientDataJSON: toB64url(r.clientDataJSON),
    authenticatorData: toB64url(r.authenticatorData),
    signature: toB64url(r.signature),
  });
}
