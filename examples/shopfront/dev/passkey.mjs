// A software passkey for the seed script. A port of the Rust software
// authenticator (crates/ordo-identity/src/software_authenticator.rs), with no
// dependencies. It makes a real P-256 key and signs real WebAuthn output, so
// the door accepts it as it would a browser's.
//
// Never use this for a real person. The key sits in a JSON file.
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

const FLAG_UP = 0x01;
const FLAG_UV = 0x04;
const FLAG_AT = 0x40;

export const b64url = (buf) => Buffer.from(buf).toString('base64url');

// ── Minimal CBOR ───────────────────────────────────────────────────────────
function head(major, arg) {
  if (arg < 24) return Buffer.from([(major << 5) | arg]);
  if (arg < 256) return Buffer.from([(major << 5) | 24, arg]);
  const b = Buffer.alloc(3);
  b[0] = (major << 5) | 25;
  b.writeUInt16BE(arg, 1);
  return b;
}
const cInt = (n) => (n < 0 ? head(1, -1 - n) : head(0, n));
const cBytes = (b) => Buffer.concat([head(2, b.length), b]);
const cText = (s) => Buffer.concat([head(3, Buffer.byteLength(s)), Buffer.from(s)]);
const cMap = (n) => head(5, n);

export class Passkey {
  constructor({ privateKey, credentialId, rpId = 'localhost', origin }) {
    this.privateKey = privateKey;
    this.credentialId = credentialId;
    this.rpId = rpId;
    this.origin = origin;
    this.signCount = 0;
  }

  static create({ origin, rpId = 'localhost' }) {
    const { privateKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
    return new Passkey({ privateKey, credentialId: randomBytes(16), rpId, origin });
  }

  // What to keep in a file so a later run can sign in again.
  serialise() {
    return {
      credentialId: b64url(this.credentialId),
      jwk: this.privateKey.export({ format: 'jwk' }),
      signCount: this.signCount,
    };
  }

  static restore(saved, { origin, rpId = 'localhost' }) {
    const p = new Passkey({
      privateKey: createPrivateKey({ key: saved.jwk, format: 'jwk' }),
      credentialId: Buffer.from(saved.credentialId, 'base64url'),
      rpId,
      origin,
    });
    p.signCount = saved.signCount ?? 0;
    return p;
  }

  #cose() {
    const jwk = createPublicKey(this.privateKey).export({ format: 'jwk' });
    return Buffer.concat([
      cMap(5),
      cInt(1), cInt(2), // kty EC2
      cInt(3), cInt(-7), // alg ES256
      cInt(-1), cInt(1), // crv P-256
      cInt(-2), cBytes(Buffer.from(jwk.x, 'base64url')),
      cInt(-3), cBytes(Buffer.from(jwk.y, 'base64url')),
    ]);
  }

  #authData(attested) {
    const count = Buffer.alloc(4);
    count.writeUInt32BE(this.signCount);
    const parts = [
      createHash('sha256').update(this.rpId).digest(),
      Buffer.from([FLAG_UP | FLAG_UV | (attested ? FLAG_AT : 0)]),
      count,
    ];
    if (attested) {
      const len = Buffer.alloc(2);
      len.writeUInt16BE(this.credentialId.length);
      parts.push(Buffer.alloc(16), len, this.credentialId, this.#cose());
    }
    return Buffer.concat(parts);
  }

  #clientData(type, challenge) {
    return Buffer.from(
      JSON.stringify({ type, challenge, origin: this.origin, crossOrigin: false }),
    );
  }

  // The fields of the `passkey_registration` body for /auth/complete.
  register(challenge) {
    const attestation = Buffer.concat([
      cMap(3),
      cText('fmt'), cText('none'),
      cText('attStmt'), cMap(0),
      cText('authData'), cBytes(this.#authData(true)),
    ]);
    return {
      method: 'passkey',
      kind: 'passkey_registration',
      challenge,
      client_data_json: b64url(this.#clientData('webauthn.create', challenge)),
      attestation_object: b64url(attestation),
    };
  }

  // The fields of the `passkey_assertion` body for /auth/complete.
  assert(challenge) {
    this.signCount += 1;
    const clientData = this.#clientData('webauthn.get', challenge);
    const authData = this.#authData(false);
    const message = Buffer.concat([authData, createHash('sha256').update(clientData).digest()]);
    // WebAuthn wants the signature in DER, which is node's default for ECDSA.
    const signature = sign('sha256', message, this.privateKey);
    return {
      method: 'passkey',
      kind: 'passkey_assertion',
      challenge,
      credential_id: b64url(this.credentialId),
      client_data_json: b64url(clientData),
      authenticator_data: b64url(authData),
      signature: b64url(signature),
    };
  }
}
