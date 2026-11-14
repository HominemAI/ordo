//! The `auth` block travels in the compiled program and in the interface
//! descriptor (`plan-configuration-in-the-file.md` §2.4, §5.2).

use ordo_plan::artifact::{auth_artifact, catalog_view, entity_artifacts, program_artifact};
use ordo_plan::interface::{digest, interface_document, with_auth, SCHEMA_VERSION};
use ordo_schema::build;
use ordo_sema::Registry;
use ordo_syntax::parse_program;
use serde_json::{json, Value};

const PROGRAM: &str = r#"domain hal @ 1.0.0

auth {
  methods           passkey, email, google, work
  provider google   preset google
  provider work     kind oidc, label "Work account"
  passkeyName       "Human Agent Link"
  provisioning      domains example.com, example.org
  selfServe         off
  guest             on, keep 7d
  linking           on, window 5m
  secondFactor      optional
  userVerification  required
  session           token 1h, max 12h, tenants 15m to 4h
  remember          on, idle 30d, max none
}

entity notes {
  body: text
}

op allNotes =
  notes[body]
"#;

fn compiled(src: &str) -> (ordo_schema::Schema, Value) {
    let program = parse_program(src).expect("parses");
    let schema = build(&program).expect("builds");
    let registry = Registry::from_program(&program);
    let art = program_artifact(&schema, &registry).expect("artifact");
    (schema, art)
}

fn descriptor(art: &Value, schema: &ordo_schema::Schema) -> Value {
    let ops: Vec<Value> = art["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(catalog_view)
        .collect();
    let doc = interface_document(&art["domain"], &entity_artifacts(schema), &ops);
    with_auth(doc, art.get("auth"))
}

#[test]
fn the_compiled_program_carries_every_value_with_defaults_filled() {
    let (_, art) = compiled(PROGRAM);
    assert_eq!(
        art["auth"],
        json!({
            "methods": ["passkey", "email", "google", "work"],
            "providers": [
                { "name": "google", "preset": "google", "kind": "oidc", "label": null },
                { "name": "work", "preset": null, "kind": "oidc", "label": "Work account" },
            ],
            "passkey_name": "Human Agent Link",
            "provisioning": { "mode": "domains", "domains": ["example.com", "example.org"] },
            "self_serve": false,
            "guest": { "on": true, "keep_secs": 604800 },
            "linking": { "on": true, "window_secs": 300 },
            "second_factor": "optional",
            "user_verification": "required",
            "session": {
                "token_secs": 3600,
                "max_secs": 43200,
                "tenants": { "min_secs": 900, "max_secs": 14400 },
            },
            "remember": { "on": true, "idle_secs": 2592000, "max_secs": null },
        })
    );
}

#[test]
fn an_empty_block_carries_the_secure_defaults() {
    let (_, art) = compiled("auth {\n}\n\nentity notes {\n  body: text\n}\n");
    assert_eq!(
        art["auth"],
        json!({
            "methods": [],
            "providers": [],
            "passkey_name": null,
            "provisioning": { "mode": "invite-only", "domains": [] },
            "self_serve": false,
            "guest": { "on": false, "keep_secs": null },
            "linking": { "on": true, "window_secs": 300 },
            "second_factor": "optional",
            "user_verification": "required",
            "session": { "token_secs": 3600, "max_secs": 43200, "tenants": null },
            "remember": { "on": true, "idle_secs": 2592000, "max_secs": null },
        })
    );
}

/// A program with no block keeps the exact document, and so the version,
/// it had before the block existed.
#[test]
fn a_program_without_a_block_keeps_its_document() {
    let (schema, art) = compiled("entity notes {\n  body: text\n}\n");
    assert!(art.get("auth").is_none());
    assert!(auth_artifact(&schema).is_none());
    let d = descriptor(&art, &schema);
    assert!(d.get("auth").is_none());
}

#[test]
fn the_descriptor_carries_the_mode_and_never_the_domains() {
    let (schema, art) = compiled(PROGRAM);
    let d = descriptor(&art, &schema);
    assert_eq!(d["schema_version"], SCHEMA_VERSION, "an addition keeps /2");
    assert_eq!(
        d["auth"],
        json!({
            "methods": ["passkey", "email", "google", "work"],
            "providers": [
                { "name": "google", "preset": "google", "kind": "oidc", "label": null },
                { "name": "work", "preset": null, "kind": "oidc", "label": "Work account" },
            ],
            "provisioning": "domains",
            "self_serve": false,
            "guest": true,
            "linking": true,
            "second_factor": "optional",
            "user_verification": "required",
        })
    );
    let text = d.to_string();
    assert!(
        !text.contains("example.com") && !text.contains("example.org"),
        "the domain list stays on the server (owner, 2026-11-14): {text}"
    );
    assert_eq!(
        d["digest"],
        json!(digest(&d)),
        "the digest covers the auth key"
    );
}
