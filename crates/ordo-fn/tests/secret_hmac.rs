//! HMAC with a secret as the key (owner ruling 2026-10-31), on the real
//! engine and the real egress kernel over a secret source double. The
//! function imports the key with `importKey("secret", name, ...)`; the host
//! opens the value and computes. Answers are checked against RFC 4231 test
//! cases 1 and 2, and RFC 2202 test case 2 for SHA-1.

use std::sync::Arc;

use ordo_egress::{DenyList, FnEgress, SecretBytes, SecretSource, TlsRoots};
use ordo_fn::{FetchError, FnCall, FnError, FnHost, FnRun, HostConfig, Limits, Scrub};
use serde_json::{json, Value};

/// `jefe` and `tc1` are declared and set, `unset` is declared with no value.
/// Each is a signing key, bound to no host.
struct Store;

#[async_trait::async_trait]
impl SecretSource for Store {
    fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
        matches!(name, "jefe" | "tc1" | "unset").then(Vec::new)
    }
    async fn open(&self, _: &str, name: &str) -> Result<Option<SecretBytes>, FetchError> {
        Ok(match name {
            "jefe" => Some(SecretBytes::new(b"Jefe".to_vec())),
            "tc1" => Some(SecretBytes::new(vec![0x0b; 20])),
            _ => None,
        })
    }
}

/// Knows the value of `jefe`.
struct JefeScrub;

impl Scrub for JefeScrub {
    fn scrub(&self, _space: &str, text: &str) -> String {
        text.replace("Jefe", "[secret]")
    }
}

/// Runs `body` as `call(input, ctx)` with `secrets` listed.
async fn run(body: &str, secrets: &[&str], input: Value) -> FnRun {
    let h = FnHost::new(HostConfig::default()).await.expect("engine");
    let code = format!(
        "const hex = (b) => Array.from(new Uint8Array(b), (x) => x.toString(16).padStart(2, '0')).join('');\n\
         const enc = (s) => new TextEncoder().encode(s);\n\
         const key = (name, hash, usages) => crypto.subtle.importKey(\n\
           'secret', name, {{ name: 'HMAC', hash }}, false, usages || ['sign', 'verify']);\n\
         const caught = async (f) => {{ try {{ await f(); return 'none'; }} catch (e) {{ return e.code || e.name; }} }};\n\
         export default {{ async call(input, ctx) {{ {body} }} }}"
    );
    let egress = FnEgress::new(
        Arc::new(Store),
        TlsRoots::webpki(),
        DenyList::categories_only(),
    );
    h.call(FnCall {
        space: "acme".into(),
        function: "f".into(),
        version: "v1".into(),
        run_id: "run-1".into(),
        code: Arc::from(code.as_bytes()),
        input,
        egress: vec![],
        secrets: secrets.iter().map(|s| s.to_string()).collect(),
        limits: Limits::default(),
        fetch: Arc::new(egress),
        scrub: Arc::new(JefeScrub),
        blobs: Arc::new(ordo_fn::NoBlobs),
        blob_args: Vec::new(),
    })
    .await
}

async fn answer(body: &str, secrets: &[&str]) -> Value {
    match run(body, secrets, json!(null)).await.outcome {
        Ok(v) => v,
        Err(e) => panic!("the function failed: {e:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_key_signs_and_verifies_like_the_rfc_vectors() {
    let out = answer(
        r#"
        const data = enc("what do ya want for nothing?");
        const sign = async (name, hash, d) => hex(await crypto.subtle.sign("HMAC", await key(name, hash), d));
        const k = await key("jefe", "SHA-256");
        const sig = await crypto.subtle.sign("HMAC", k, data);
        const bad = new Uint8Array(sig).slice(); bad[31] ^= 1;
        const handle = await crypto.subtle.importKey(
          "secret", ctx.secret("jefe"), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
        return {
          sha1: await sign("jefe", "SHA-1", data),
          sha256: await sign("jefe", "SHA-256", data),
          sha384: await sign("jefe", { name: "SHA-384" }, data),
          sha512: await sign("jefe", "SHA-512", data),
          tc1: await sign("tc1", "SHA-256", enc("Hi There")),
          tc1_512: await sign("tc1", "SHA-512", enc("Hi There")),
          ok: await crypto.subtle.verify("HMAC", k, sig, data),
          tampered: await crypto.subtle.verify("HMAC", k, bad, data),
          short: await crypto.subtle.verify("HMAC", k, new Uint8Array(sig).slice(0, 16), data),
          view: hex(await crypto.subtle.sign("HMAC", k, enc("xwhat do ya want for nothing?x").subarray(1, 29))),
          byHandle: await crypto.subtle.verify("HMAC", handle, sig, data),
          algorithm: k.algorithm,
          extractable: k.extractable,
          isKey: k instanceof CryptoKey,
        };
    "#,
        &["jefe", "tc1"],
    )
    .await;
    assert_eq!(out["sha1"], "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
    let tc2 = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    assert_eq!(out["sha256"], tc2);
    assert_eq!(
        out["sha384"],
        "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649"
    );
    assert_eq!(
        out["sha512"],
        "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737"
    );
    assert_eq!(
        out["tc1"],
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        out["tc1_512"],
        "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
    );
    assert_eq!(out["ok"], true);
    assert_eq!(out["tampered"], false);
    assert_eq!(out["short"], false);
    // A view over part of a buffer signs only its own bytes.
    assert_eq!(out["view"], tc2);
    assert_eq!(out["byHandle"], true);
    assert_eq!(
        out["algorithm"],
        json!({ "name": "HMAC", "hash": { "name": "SHA-256" } })
    );
    assert_eq!(out["extractable"], false);
    assert_eq!(out["isKey"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_names_its_reason_and_the_key_never_leaves() {
    let out = answer(
        r#"
        const data = enc("x");
        const k = await key("jefe", "SHA-256");
        let exportMessage = "";
        try { await crypto.subtle.exportKey("raw", k); } catch (e) { exportMessage = e.message; }
        let unsetMessage = "";
        try { await crypto.subtle.sign("HMAC", await key("unset", "SHA-256"), data); }
        catch (e) { unsetMessage = e.message; }
        return {
          exported: await caught(() => crypto.subtle.exportKey("raw", k)),
          exportMessage,
          extractable: await caught(() => crypto.subtle.importKey(
            "secret", "jefe", { name: "HMAC", hash: "SHA-256" }, true, ["sign"])),
          notListed: await caught(async () => crypto.subtle.sign("HMAC", await key("tc1", "SHA-256"), data)),
          unknown: await caught(async () => crypto.subtle.sign("HMAC", await key("nosuch", "SHA-256"), data)),
          unset: await caught(async () => crypto.subtle.verify("HMAC", await key("unset", "SHA-256"), data, data)),
          unsetMessage,
          noName: await caught(() => key("", "SHA-256")),
          signOnly: await caught(async () => crypto.subtle.verify("HMAC", await key("jefe", "SHA-256", ["sign"]), data, data)),
          refusedName: await (async () => { try { await crypto.subtle.sign("HMAC", await key("unset", "SHA-256"), data); } catch (e) { return e.name; } })(),
        };
    "#,
        &["jefe", "unset", "nosuch"],
    )
    .await;
    assert_eq!(out["exported"], "InvalidAccessError");
    assert!(
        out["exportMessage"]
            .as_str()
            .unwrap()
            .contains("never enters the function"),
        "{out}"
    );
    assert_eq!(out["extractable"], "InvalidAccessError");
    assert_eq!(out["notListed"], "secret_not_listed");
    assert_eq!(out["unknown"], "secret_unknown");
    assert_eq!(out["unset"], "secret_not_set");
    assert!(
        out["unsetMessage"]
            .as_str()
            .unwrap()
            .contains("ordo secret set unset"),
        "{out}"
    );
    assert_eq!(out["noName"], "TypeError");
    assert_eq!(out["signOnly"], "InvalidAccessError");
    assert_eq!(out["refusedName"], "SecretRefused");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_uncaught_refusal_ends_the_call_with_its_reason() {
    let run = run(
        r#"return hex(await crypto.subtle.sign("HMAC", await key("unset", "SHA-256"), enc("x")));"#,
        &["unset"],
        json!(null),
    )
    .await;
    match &run.outcome {
        Err(FnError::Fetch { code, message }) => {
            assert_eq!(code, "secret_not_set");
            assert!(message.contains("ordo secret set unset"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(run.class(), "secret_not_set");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_function_echoes_is_scrubbed_from_its_log_and_answer() {
    // The function cannot read the value; here the caller handed it in.
    let run = run(
        r#"console.log("got", input.text); return { echo: input.text };"#,
        &["jefe"],
        json!({ "text": "the key is Jefe" }),
    )
    .await;
    let out = run.outcome.expect("ok");
    assert_eq!(out["echo"], "the key is [secret]");
    assert_eq!(run.report.logs[0].text, "got the key is [secret]");
}
