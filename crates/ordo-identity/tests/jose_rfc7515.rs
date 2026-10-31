//! The JWS verifier against the published test vectors.
//!
//! The vectors are RFC 7515 Appendix A and RFC 8037 Appendix A.4, copied
//! from the RFC text. Each good token verifies and yields its payload.
//! HS256, `none` and ES512 are refused by name. A changed bit fails.
//! None of these tokens has a `kid`, so the JWKS path refuses them.

use ordo_identity::jose::{verify_with_jwks, verify_with_key, JoseError, Jwk, Jwks, ALL_ALGS};
use serde_json::{json, Value};

const A1_HS256: &str = "eyJ0eXAiOiJKV1QiLA0KICJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

const A2_N: &str = "ofgWCuLjybRlzo0tZWJjNiuSfb4p4fAkd_wWJcyQoTbji9k0l8W26mPddxHmfHQp-Vaw-4qPCJrcS2mJPMEzP1Pt0Bm4d4QlL-yRT-SFd2lZS-pCgNMsD1W_YpRPEwOWvG6b32690r2jZ47soMZo9wGzjb_7OMg0LOL-bSf63kpaSHSXndS5z5rexMdbBYUsLA9e-KXBdQOS-UTo7WTBEMa2R2CapHg665xsmtdVMTBQY4uDZlxvb3qCo5ZwKh9kG4LT6_I5IhlJH7aGhyxXFvUK-DWNmoudF8NAco9_h9iaGNj8q2ethFkMLs91kzk2PAcDTW9gb54h4FRWyuXpoQ";
const A2_RS256: &str = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ.cC4hiUPoj9Eetdgtv3hF80EGrhuB__dzERat0XF9g2VtQgr9PJbu3XOiZj5RZmh7AAuHIm4Bh-0Qc_lF5YKt_O8W2Fp5jujGbds9uJdbF9CUAr7t1dnZcAcQjbKBYNX4BAynRFdiuB--f_nZLgrnbyTyWzO75vRK5h6xBArLIARNPvkSjtQBMHlb1L07Qe7K0GarZRmB_eSN9383LcOLn6_dO--xi12jzDwusC-eOkHWEsqtFZESc6BfI7noOPqvhJ1phCnvWh6IeYI2w9QOYEUipUTI8np6LbgGY9Fs98rqVt5AXLIhWkWywlVmtVrBp0igcN_IoypGlUPQGe77Rw";

const A3_X: &str = "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU";
const A3_Y: &str = "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0";
const A3_ES256: &str = "eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ.DtEhU3ljbEg8L38VWAfUAqOyKAM6-Xx-F4GawxaepmXFCgfTjDxw5djxLa8ISlSApmWQxfKTUJqPP3-Kg6NU1Q";

const A4_ES512: &str = "eyJhbGciOiJFUzUxMiJ9.UGF5bG9hZA.AdwMgeerwtHoh-l192l60hp9wAHZFVJbLfD_UxMi70cwnZOYaRI1bKPWROc-mZZqwqT2SI-KGDKB34XO0aw_7XdtAG8GaSwFKdCAPZgoXD2YBJZCPEX3xKpRwcdOO8KpEHwJjyqOgzDO7iKvU8vcnwNrmxYbSW9ERBXukOXolLzeO_Jn";

const A5_NONE: &str = "eyJhbGciOiJub25lIn0.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ.";

const ED_X: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";
const ED_A4: &str = "eyJhbGciOiJFZERTQSJ9.RXhhbXBsZSBvZiBFZDI1NTE5IHNpZ25pbmc.hgyY0il_MGCjP0JzlnLWG1PPOt7-09PGcvMg3AIbQR6dWbhijcNR4ki4iylGjg5BhVsPt9g7sVvpAr_MuM0KAg";

const JOE_PAYLOAD: &[u8] =
    b"{\"iss\":\"joe\",\r\n \"exp\":1300819380,\r\n \"http://example.com/is_root\":true}";

fn rsa_json() -> Value {
    json!({"kty": "RSA", "n": A2_N, "e": "AQAB"})
}

fn rsa_jwk() -> Jwk {
    Jwk::parse(&rsa_json()).expect("the A.2 key")
}

fn ec_jwk() -> Jwk {
    Jwk::parse(&json!({"kty": "EC", "crv": "P-256", "x": A3_X, "y": A3_Y})).expect("the A.3 key")
}

fn ed_jwk() -> Jwk {
    Jwk::parse(&json!({"kty": "OKP", "crv": "Ed25519", "x": ED_X})).expect("the 8037 key")
}

/// Change one character of segment `seg` (0 header, 1 payload, 2 signature)
/// at `at`, to another base64url character.
fn flip(token: &str, seg: usize, at: usize) -> String {
    let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
    let mut chars: Vec<char> = parts[seg].chars().collect();
    chars[at] = if chars[at] == 'A' { 'B' } else { 'A' };
    parts[seg] = chars.into_iter().collect();
    parts.join(".")
}

#[test]
fn rfc7515_a2_rs256_verifies_and_yields_the_payload() {
    let v = verify_with_key(A2_RS256, &rsa_jwk(), &ALL_ALGS).expect("A.2 verifies");
    assert_eq!(v.payload(), JOE_PAYLOAD);
}

#[test]
fn rfc7515_a3_es256_verifies() {
    let v = verify_with_key(A3_ES256, &ec_jwk(), &ALL_ALGS).expect("A.3 verifies");
    assert_eq!(v.payload(), JOE_PAYLOAD);
}

#[test]
fn rfc8037_a4_eddsa_verifies() {
    let v = verify_with_key(ED_A4, &ed_jwk(), &ALL_ALGS).expect("8037 A.4 verifies");
    assert_eq!(v.payload(), b"Example of Ed25519 signing");
}

#[test]
fn rfc7515_a1_hs256_is_refused_by_name() {
    assert_eq!(
        verify_with_key(A1_HS256, &rsa_jwk(), &ALL_ALGS).err(),
        Some(JoseError("alg-hmac"))
    );
}

#[test]
fn rfc7515_a5_unsecured_is_refused_by_name() {
    assert_eq!(
        verify_with_key(A5_NONE, &rsa_jwk(), &ALL_ALGS).err(),
        Some(JoseError("alg-none"))
    );
}

#[test]
fn rfc7515_a4_es512_is_not_on_the_list() {
    assert_eq!(
        verify_with_key(A4_ES512, &ec_jwk(), &ALL_ALGS).err(),
        Some(JoseError("alg-unknown"))
    );
}

#[test]
fn a_flipped_bit_in_any_vector_fails_its_signature() {
    let vectors = [
        (A2_RS256, rsa_jwk()),
        (A3_ES256, ec_jwk()),
        (ED_A4, ed_jwk()),
    ];
    for (token, key) in &vectors {
        assert!(verify_with_key(token, key, &ALL_ALGS).is_ok());
        let sig_len = token.split('.').nth(2).unwrap().len();
        let payload_len = token.split('.').nth(1).unwrap().len();
        // Mid-segment, so the change is never in a non-canonical tail.
        let in_signature = flip(token, 2, sig_len / 2);
        let in_payload = flip(token, 1, payload_len / 2);
        assert_ne!(&in_signature, token);
        assert_ne!(&in_payload, token);
        assert!(verify_with_key(&in_signature, key, &ALL_ALGS).is_err());
        assert!(verify_with_key(&in_payload, key, &ALL_ALGS).is_err());
    }
}

#[test]
fn the_rfc_vectors_carry_no_kid_so_the_jwks_path_needs_one_usable_key() {
    let mut key = rsa_json();
    key["kid"] = json!("x");
    let one = json!({"keys": [key.clone()]}).to_string();
    let jwks = Jwks::parse(one.as_bytes()).expect("a JWKS");
    assert!(verify_with_jwks(A2_RS256, &jwks, &ALL_ALGS).is_ok());
    let mut other = key.clone();
    other["kid"] = json!("y");
    let two = json!({"keys": [key, other]}).to_string();
    let jwks = Jwks::parse(two.as_bytes()).expect("a JWKS");
    assert_eq!(
        verify_with_jwks(A2_RS256, &jwks, &ALL_ALGS).err(),
        Some(JoseError("kid-missing"))
    );
}
