//! `TextEncoder`, `TextDecoder` and `crypto` as a function sees them, on the
//! real engine. Digest and HMAC answers are checked against published
//! vectors (FIPS 180 examples, RFC 2202 and RFC 4231 test case 2).

use std::sync::Arc;

use ordo_fn::{
    FetchError, FnCall, FnFetch, FnFetchRequest, FnFetchResponse, FnHost, HostConfig, Limits,
    NoScrub,
};
use serde_json::{json, Value};

struct NoFetch;

#[async_trait::async_trait]
impl FnFetch for NoFetch {
    async fn fetch(&self, _: &str, _: FnFetchRequest) -> Result<FnFetchResponse, FetchError> {
        panic!("these functions make no outbound call")
    }

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, FetchError> {
        panic!("this double holds no secret")
    }
}

/// Runs `body` as the function's `call(input)` and returns its answer.
async fn run(body: &str) -> Value {
    let h = FnHost::new(HostConfig::default()).await.expect("engine");
    let code = format!(
        "const hex = (b) => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, '0')).join('');\n\
         const enc = (s) => new TextEncoder().encode(s);\n\
         export default {{ async call(input) {{ {body} }} }}"
    );
    let run = h
        .call(FnCall {
            space: "acme".into(),
            function: "f".into(),
            version: "v1".into(),
            run_id: "run-1".into(),
            code: Arc::from(code.as_bytes()),
            input: json!(null),
            egress: vec![],
            secrets: vec![],
            limits: Limits::default(),
            fetch: Arc::new(NoFetch),
            scrub: Arc::new(NoScrub),
            blobs: Arc::new(ordo_fn::NoBlobs),
            blob_args: Vec::new(),
        })
        .await;
    match run.outcome {
        Ok(v) => v,
        Err(e) => panic!("the function failed: {e:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn digests_match_the_published_vectors() {
    let out = run(r#"
        const d = async (alg) => hex(await crypto.subtle.digest(alg, enc("abc")));
        return {
          sha1: await d("SHA-1"),
          sha256: await d("SHA-256"),
          sha384: await d({ name: "sha-384" }),
          sha512: await d("SHA-512"),
          empty: hex(await crypto.subtle.digest("SHA-256", new ArrayBuffer(0))),
          view: hex(await crypto.subtle.digest("SHA-256", enc("xabcx").subarray(1, 4))),
        };
    "#)
    .await;
    assert_eq!(out["sha1"], "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        out["sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        out["sha384"],
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        out["sha512"],
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(
        out["empty"],
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    // A view over part of a buffer hashes only its own bytes.
    assert_eq!(out["view"], out["sha256"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn hmac_signs_and_verifies_like_the_rfc_vectors() {
    let out = run(r#"
        const key = (hash, usages) => crypto.subtle.importKey(
          "raw", enc("Jefe"), { name: "HMAC", hash }, false, usages || ["sign", "verify"]);
        const data = enc("what do ya want for nothing?");
        const sign = async (hash) => hex(await crypto.subtle.sign("HMAC", await key(hash), data));
        const k = await key("SHA-256");
        const sig = await crypto.subtle.sign({ name: "HMAC" }, k, data);
        const bad = new Uint8Array(sig).slice(); bad[0] ^= 1;
        let signWithVerifyKey = "";
        try { await crypto.subtle.sign("HMAC", await key("SHA-256", ["verify"]), data); }
        catch (e) { signWithVerifyKey = e.name; }
        let exported = "";
        try { await crypto.subtle.exportKey("raw", k); } catch (e) { exported = e.name; }
        let unsupported = "";
        try { await crypto.subtle.digest("MD5", data); } catch (e) { unsupported = e.name; }
        const gen = await crypto.subtle.generateKey({ name: "HMAC", hash: "SHA-256" }, true, ["sign"]);
        return {
          sha1: await sign("SHA-1"),
          sha256: await sign("SHA-256"),
          sha384: await sign({ name: "SHA-384" }),
          sha512: await sign("SHA-512"),
          ok: await crypto.subtle.verify("HMAC", k, sig, data),
          tampered: await crypto.subtle.verify("HMAC", k, bad, data),
          short: await crypto.subtle.verify("HMAC", k, new Uint8Array(sig).slice(0, 16), data),
          algorithm: k.algorithm,
          isKey: k instanceof CryptoKey,
          signWithVerifyKey,
          exported,
          unsupported,
          generated: (await crypto.subtle.exportKey("raw", gen)).byteLength,
        };
    "#)
    .await;
    assert_eq!(out["sha1"], "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
    assert_eq!(
        out["sha256"],
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        out["sha384"],
        "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649"
    );
    assert_eq!(
        out["sha512"],
        "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
    );
    assert_eq!(out["ok"], true);
    assert_eq!(out["tampered"], false);
    assert_eq!(out["short"], false);
    assert_eq!(
        out["algorithm"],
        json!({ "name": "HMAC", "hash": { "name": "SHA-256" }, "length": 32 })
    );
    assert_eq!(out["isKey"], true);
    assert_eq!(out["signWithVerifyKey"], "InvalidAccessError");
    assert_eq!(out["exported"], "InvalidAccessError");
    assert_eq!(out["unsupported"], "NotSupportedError");
    assert_eq!(out["generated"], 64);
}

#[tokio::test(flavor = "multi_thread")]
async fn text_encoder_and_decoder_round_trip_utf8() {
    let out = run(r#"
        const s = "héllo € \u{1F600}";
        const bytes = new TextEncoder().encode(s);
        const into = new Uint8Array(8);
        const r = new TextEncoder().encodeInto(s, into);
        let fatal = "";
        try { new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array([0xff])); }
        catch (e) { fatal = e.name; }
        let label = "";
        try { new TextDecoder("latin1"); } catch (e) { label = e.name; }
        const bom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61]);
        return {
          isU8: bytes instanceof Uint8Array,
          hex: hex(bytes),
          back: new TextDecoder().decode(bytes),
          // "héllo " is 7 bytes; the euro sign's 3 do not fit in 8.
          into: [r.read, r.written],
          lone: hex(new TextEncoder().encode("\ud800")),
          lossy: new TextDecoder().decode(new Uint8Array([0x61, 0xff])),
          fatal,
          label,
          bom: new TextDecoder().decode(bom),
          keepBom: new TextDecoder("utf8", { ignoreBOM: true }).decode(bom).length,
          encoding: new TextDecoder("UTF-8").encoding,
        };
    "#)
    .await;
    assert_eq!(out["isU8"], true);
    assert_eq!(out["hex"], "68c3a96c6c6f20e282ac20f09f9880");
    assert_eq!(out["back"], "h\u{e9}llo \u{20ac} \u{1F600}");
    assert_eq!(out["into"], json!([6, 7]));
    assert_eq!(out["lone"], "efbfbd");
    assert_eq!(out["lossy"], "a\u{fffd}");
    assert_eq!(out["fatal"], "TypeError");
    assert_eq!(out["label"], "RangeError");
    assert_eq!(out["bom"], "a");
    assert_eq!(out["keepBom"], 2);
    assert_eq!(out["encoding"], "utf-8");
}

#[tokio::test(flavor = "multi_thread")]
async fn randomness_is_fresh_per_call() {
    let body = r#"
        const a = crypto.getRandomValues(new Uint8Array(32));
        const w = crypto.getRandomValues(new Uint32Array(4));
        let quota = "";
        try { crypto.getRandomValues(new Uint8Array(65537)); } catch (e) { quota = e.name; }
        let float = "";
        try { crypto.getRandomValues(new Float64Array(2)); } catch (e) { float = e.name; }
        return { a: hex(a), w: w.length, uuid: crypto.randomUUID(), uuid2: crypto.randomUUID(), quota, float };
    "#;
    let one = run(body).await;
    let two = run(body).await;
    // Two calls start from the same snapshot; their bytes still differ.
    assert_ne!(one["a"], two["a"]);
    assert_ne!(one["a"], "0".repeat(64));
    assert_eq!(one["w"], 4);
    let uuid = one["uuid"].as_str().unwrap();
    assert_eq!(uuid.len(), 36);
    assert_eq!(&uuid[14..15], "4", "{uuid}");
    assert!("89ab".contains(&uuid[19..20]), "{uuid}");
    assert_ne!(one["uuid"], one["uuid2"]);
    assert_ne!(one["uuid"], two["uuid"]);
    assert_eq!(one["quota"], "QuotaExceededError");
    assert_eq!(one["float"], "TypeMismatchError");
}
