/**
 * Sealing for the device replica.
 *
 * Every record is encrypted at rest, and every key it is stored under is a
 * keyed hash, so a copied disk or browser profile shows neither rows nor the
 * ids and parameters that would identify them.
 *
 * v1's keys are device-local and non-extractable (`plan-client-cache.md` Q-3,
 * ruled 2026-09-14). They protect a copied disk; they do not protect against a
 * script running in the app's own origin while the replica is open, because
 * the app itself has to use them. The passkey-PRF wrap and the
 * server-released key of 5g-rev change how the keys are obtained, not the
 * format of what they seal.
 */

/** The two keys one replica is sealed and named under. */
export interface DeviceKeys {
  /** AES-GCM, 256-bit, non-extractable: seals every record. */
  readonly seal: CryptoKey;
  /** HMAC-SHA-256, non-extractable: names every record. */
  readonly name: CryptoKey;
}

const NONCE = 12;

function subtle(): SubtleCrypto {
  const s = globalThis.crypto?.subtle;
  if (!s) {
    throw new Error('the local replica needs WebCrypto (crypto.subtle), which this runtime lacks');
  }
  return s;
}

/** A copy whose buffer is a plain `ArrayBuffer`, as WebCrypto requires. */
function bytes(b: Uint8Array): Uint8Array<ArrayBuffer> {
  return new Uint8Array(b);
}

function hex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

/**
 * The parts of a name, encoded so that two different lists never encode the
 * same: a JSON array quotes and escapes every part, so no id can smuggle in a
 * boundary.
 */
function encodeParts(parts: readonly string[]): Uint8Array<ArrayBuffer> {
  return bytes(new TextEncoder().encode(JSON.stringify(parts)));
}

/** Fresh keys for a new replica. Neither can be read back out of the runtime. */
export async function createDeviceKeys(): Promise<DeviceKeys> {
  const seal = await subtle().generateKey({ name: 'AES-GCM', length: 256 }, false, [
    'encrypt',
    'decrypt',
  ]);
  const name = await subtle().generateKey({ name: 'HMAC', hash: 'SHA-256', length: 256 }, false, [
    'sign',
  ]);
  return { seal, name };
}

/** `nonce || ciphertext`, under a fresh random nonce. */
export async function sealBytes(keys: DeviceKeys, plain: Uint8Array): Promise<Uint8Array> {
  const nonce = globalThis.crypto.getRandomValues(new Uint8Array(NONCE));
  const ct = new Uint8Array(
    await subtle().encrypt({ name: 'AES-GCM', iv: nonce }, keys.seal, bytes(plain)),
  );
  const out = new Uint8Array(NONCE + ct.length);
  out.set(nonce, 0);
  out.set(ct, NONCE);
  return out;
}

/** Open what {@link sealBytes} produced. Throws if it was altered. */
export async function openBytes(keys: DeviceKeys, sealed: Uint8Array): Promise<Uint8Array> {
  if (sealed.length <= NONCE) throw new Error('a sealed record is too short to hold its nonce');
  const nonce = bytes(sealed.subarray(0, NONCE));
  const ct = bytes(sealed.subarray(NONCE));
  return new Uint8Array(await subtle().decrypt({ name: 'AES-GCM', iv: nonce }, keys.seal, ct));
}

/** The name a record is stored under: a keyed hash of its parts. */
export async function nameOf(keys: DeviceKeys, parts: readonly string[]): Promise<string> {
  return hex(new Uint8Array(await subtle().sign('HMAC', keys.name, encodeParts(parts))));
}

/** An unkeyed name, for what has to be found before any key is loaded. */
export async function digestOf(parts: readonly string[]): Promise<string> {
  return hex(new Uint8Array(await subtle().digest('SHA-256', encodeParts(parts))));
}
