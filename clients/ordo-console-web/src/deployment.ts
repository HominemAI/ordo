/**
 * The deployment's passkey, used from the console's page
 * (docs/plan-console.md 8.3).
 *
 * The deployment is its own relying party. The browser runs its ceremonies
 * here because the deployment lists this page's origin: in
 * ORDO_AUTH_ORIGINS on a shared domain, or ORDO_AUTH_RELATED_ORIGINS
 * (Related Origin Requests) on another. The console server only relays: the
 * deployment's options come back as the deployment wrote them, and the
 * answer goes back in the deployment's own body shape.
 */

import { ConsoleCallError, get, post } from './api';
import { toB64url } from './webauthn';

function fromB64url(s: string): ArrayBuffer {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out.buffer;
}

/** The deployment's request options (`/auth/begin`, `/auth/step-up/begin`). */
export interface DoorRequest {
  challenge: string;
  rp_id: string;
  user_verification: UserVerificationRequirement;
  allow_credentials?: string[];
  timeout_ms?: number;
  /** The console's name for the session a touch is for. */
  session?: string;
}

const ids = (list: string[] | undefined): PublicKeyCredentialDescriptor[] =>
  (list ?? []).map((id) => ({ type: 'public-key', id: fromB64url(id) }));

/** An assertion with the owner's passkey at the deployment. */
export async function doorGet(o: DoorRequest): Promise<Record<string, string>> {
  const cred = (await navigator.credentials.get({
    publicKey: {
      challenge: fromB64url(o.challenge),
      rpId: o.rp_id,
      userVerification: o.user_verification,
      allowCredentials: ids(o.allow_credentials),
      ...(o.timeout_ms ? { timeout: o.timeout_ms } : {}),
    },
  })) as PublicKeyCredential | null;
  if (!cred) throw new DOMException('No passkey was used.', 'NotAllowedError');
  const r = cred.response as AuthenticatorAssertionResponse;
  return {
    kind: 'passkey_assertion',
    challenge: o.challenge,
    credential_id: toB64url(cred.rawId),
    client_data_json: toB64url(r.clientDataJSON),
    authenticator_data: toB64url(r.authenticatorData),
    signature: toB64url(r.signature),
  };
}

/** The owner's touch at the deployment, relayed for the console's session. */
export async function touchDeployment(): Promise<void> {
  const begun = await post<DoorRequest>('door/stepup/begin');
  const credential = await doorGet(begun);
  await post('door/stepup/finish', { credential, session: begun.session });
}

/** Whether a refusal asks for the owner's touch at the deployment. */
export function needsDeploymentTouch(e: unknown): boolean {
  return e instanceof ConsoleCallError && e.code === 'deployment_touch_required';
}

/** Where the console stands with the deployment (`GET door`). */
export interface Link {
  linked: boolean;
  door: string;
  space: string;
  /** When not linked: how an owner is named, and the identity settings. */
  howToLink?: string;
  key?: string;
  name?: string;
  linkedAt?: number;
  sessionUntil?: number | null;
  presentUntil?: number | null;
  rotateDays?: number;
  rotateDueAt?: number;
  rotateDue?: boolean;
  /** Set when people sign in at the identity service: no link, no key. */
  via?: 'identity-service';
  /** Whether the door answered `GET /health/live`. */
  reachable?: boolean;
  reachError?: string | null;
}

export interface ConsoleKey {
  id: string;
  name: string;
  createdAt: number;
  lastUsedAt: number | null;
  revokedAt: number | null;
  thisConsole: boolean;
}

export const link = (): Promise<Link> => get<Link>('door');
