#!/usr/bin/env python3
"""A software passkey for the step 7 rehearsals (scripts/rehearse/).

It answers WebAuthn ceremonies as a browser's authenticator would, with an
ES256 key kept in a JSON file, the same shape as Ordo's own test
authenticator (crates/ordo-identity/src/software_authenticator.rs): fmt
`none`, user present and verified, a sign count that grows.

    passkey.py register KEYFILE ORIGIN < begin.json   # prints the finish body
    passkey.py assert KEYFILE ORIGIN < begin.json     # prints the finish body

`begin.json` is the begin route's answer; its `publicKey.challenge` and the
relying party id are read from it. Needs the `cryptography` package.
"""
import base64
import hashlib
import json
import os
import sys

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec


def b64url(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def cbor_head(major, n):
    if n < 24:
        return bytes([major << 5 | n])
    if n < 256:
        return bytes([major << 5 | 24, n])
    return bytes([major << 5 | 25]) + n.to_bytes(2, "big")


def cbor(v):
    if isinstance(v, bool):
        raise ValueError("no bools here")
    if isinstance(v, int):
        return cbor_head(0, v) if v >= 0 else cbor_head(1, -1 - v)
    if isinstance(v, bytes):
        return cbor_head(2, len(v)) + v
    if isinstance(v, str):
        b = v.encode()
        return cbor_head(3, len(b)) + b
    if isinstance(v, list):
        out = cbor_head(4, len(v))
        for x in v:
            out += cbor(x)
        return out
    if isinstance(v, dict):
        out = cbor_head(5, len(v))
        for k, x in v.items():
            out += cbor(k) + cbor(x)
        return out
    raise ValueError(type(v))


UP, UV, AT = 0x01, 0x04, 0x40


def rp_of(begin):
    pk = begin.get("publicKey", begin)
    if "rp" in pk:
        return pk["rp"]["id"]
    return pk["rpId"]


def main():
    cmd, keyfile, origin = sys.argv[1], sys.argv[2], sys.argv[3]
    begin = json.load(sys.stdin)
    challenge = begin.get("publicKey", begin)["challenge"]
    if cmd == "register":
        rp = rp_of(begin)
        key = ec.generate_private_key(ec.SECP256R1())
        cred = os.urandom(16)
        nums = key.public_key().public_numbers()
        cose = cbor({1: 2, 3: -7, -1: 1, -2: nums.x.to_bytes(32, "big"), -3: nums.y.to_bytes(32, "big")})
        auth = hashlib.sha256(rp.encode()).digest() + bytes([UP | UV | AT]) + (0).to_bytes(4, "big")
        auth += bytes(16) + len(cred).to_bytes(2, "big") + cred + cose
        client = json.dumps({"type": "webauthn.create", "challenge": challenge,
                             "origin": origin, "crossOrigin": False}, separators=(",", ":")).encode()
        att = cbor({"fmt": "none", "attStmt": {}, "authData": auth})
        pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                serialization.NoEncryption()).decode()
        with open(keyfile, "w") as f:
            json.dump({"rp": rp, "cred": b64url(cred), "pem": pem, "count": 0}, f)
        print(json.dumps({"challenge": challenge, "clientDataJSON": b64url(client),
                          "attestationObject": b64url(att)}))
    elif cmd == "assert":
        with open(keyfile) as f:
            k = json.load(f)
        k["count"] += 1
        key = serialization.load_pem_private_key(k["pem"].encode(), password=None)
        auth = hashlib.sha256(k["rp"].encode()).digest() + bytes([UP | UV]) + k["count"].to_bytes(4, "big")
        client = json.dumps({"type": "webauthn.get", "challenge": challenge,
                             "origin": origin, "crossOrigin": False}, separators=(",", ":")).encode()
        sig = key.sign(auth + hashlib.sha256(client).digest(), ec.ECDSA(hashes.SHA256()))
        with open(keyfile, "w") as f:
            json.dump(k, f)
        print(json.dumps({"challenge": challenge, "credentialId": k["cred"],
                          "clientDataJSON": b64url(client), "authenticatorData": b64url(auth),
                          "signature": b64url(sig)}))
    else:
        sys.exit("passkey.py register|assert KEYFILE ORIGIN < begin.json")


if __name__ == "__main__":
    main()
