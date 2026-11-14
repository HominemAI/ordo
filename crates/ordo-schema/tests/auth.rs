//! The `auth` block checked and resolved
//! (`plan-configuration-in-the-file.md` §2.2, §2.3, §6).

use ordo_schema::auth::AuthValues;
use ordo_schema::{build, SchemaError};
use ordo_syntax::parse_program;

const FULL: &str = r#"auth {
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
"#;

fn refused(block: &str) -> (SchemaError, String) {
    let src = format!("auth {{\n{block}\n}}\n");
    let program = parse_program(&src).unwrap_or_else(|e| panic!("{src}\n{e}"));
    let e = build(&program).expect_err("refused");
    let at = src[e.span.start..e.span.end].to_string();
    (e, at)
}

fn values(src: &str) -> AuthValues {
    let program = parse_program(src).expect("parses");
    let schema = build(&program).unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
    AuthValues::resolve(schema.auth.as_ref().expect("the schema keeps the block"))
}

#[test]
fn the_full_example_is_accepted_and_resolves_to_what_it_says() {
    let v = values(FULL);
    assert_eq!(v.methods, ["passkey", "email", "google", "work"]);
    assert_eq!(v.providers[0].preset.as_deref(), Some("google"));
    assert_eq!(v.providers[0].kind, "oidc");
    assert_eq!(v.providers[1].preset, None);
    assert_eq!(v.providers[1].label.as_deref(), Some("Work account"));
    assert_eq!(v.passkey_name.as_deref(), Some("Human Agent Link"));
    assert_eq!(v.provisioning, "domains");
    assert_eq!(v.domains, ["example.com", "example.org"]);
    assert!(!v.self_serve);
    assert!(v.guest);
    assert_eq!(v.guest_keep_secs, Some(7 * 86_400));
    assert!(v.linking);
    assert_eq!(v.linking_window_secs, 300);
    assert_eq!(v.second_factor, "optional");
    assert_eq!(v.user_verification, "required");
    assert_eq!(v.session_token_secs, 3_600);
    assert_eq!(v.session_max_secs, 43_200);
    assert_eq!(v.tenant_token_secs, Some((900, 14_400)));
    assert!(v.remember);
    assert_eq!(v.remember_idle_secs, 30 * 86_400);
    assert_eq!(v.remember_max_secs, None);
}

/// Every absent clause takes the secure default of §2.3.
#[test]
fn an_empty_block_resolves_to_the_secure_defaults() {
    let v = values("auth {\n}\n");
    assert!(v.methods.is_empty(), "no method: nobody signs in");
    assert!(v.providers.is_empty());
    assert_eq!(v.passkey_name, None, "the relying party id");
    assert_eq!(v.provisioning, "invite-only");
    assert!(v.domains.is_empty());
    assert!(!v.self_serve);
    assert!(!v.guest);
    assert_eq!(v.guest_keep_secs, None);
    assert!(v.linking);
    assert_eq!(v.linking_window_secs, 300);
    assert_eq!(v.second_factor, "optional");
    assert_eq!(v.user_verification, "required");
    assert_eq!(v.session_token_secs, 3_600);
    assert_eq!(v.session_max_secs, 12 * 3_600);
    assert_eq!(v.tenant_token_secs, None);
    assert!(v.remember);
    assert_eq!(v.remember_idle_secs, 30 * 86_400);
    assert_eq!(v.remember_max_secs, None);
}

#[test]
fn a_program_without_a_block_has_none() {
    let program = parse_program("entity notes {\n  body: text\n}\n").unwrap();
    assert!(build(&program).unwrap().auth.is_none());
}

#[test]
fn a_method_that_names_no_provider_is_refused() {
    let (e, at) = refused("  methods passkey, discord");
    assert_eq!(e.code, "auth_unknown_method");
    assert_eq!(at, "discord");
    assert!(
        e.message.contains("provider discord preset discord"),
        "{}",
        e.message
    );
}

#[test]
fn an_unused_provider_is_refused() {
    let (e, at) = refused("  methods passkey\n  provider gh preset github");
    assert_eq!(e.code, "auth_unused_provider");
    assert_eq!(at, "provider gh preset github");
}

#[test]
fn a_provider_without_any_methods_line_is_unused() {
    let (e, _) = refused("  provider gh preset github");
    assert_eq!(e.code, "auth_unused_provider");
}

#[test]
fn an_unknown_preset_is_refused() {
    let (e, at) = refused("  methods okta\n  provider okta preset okta");
    assert_eq!(e.code, "auth_unknown_preset");
    assert_eq!(at, "okta");
    assert!(
        e.message
            .contains("`discord`, `github`, `google`, `microsoft`"),
        "{}",
        e.message
    );
}

/// Each duration outside its platform range is refused at the duration,
/// and the message names the range.
#[test]
fn a_duration_outside_the_platform_range_is_refused_naming_the_range() {
    let cases: &[(&str, &str, &str)] = &[
        ("  linking on, window 30s", "30s", "1m to 15m"),
        ("  linking on, window 16m", "16m", "1m to 15m"),
        ("  guest on, keep 30s", "30s", "1m to 400d"),
        ("  guest on, keep 401d", "401d", "1m to 400d"),
        ("  session token 30s", "30s", "1m to 1d"),
        ("  session token 2d", "2d", "1m to 1d"),
        ("  session token 1h, max 31d", "31d", "1m to 30d"),
        ("  session token 1h, tenants 10s to 1h", "10s", "1m to 1d"),
        ("  session token 1h, tenants 1m to 2d", "2d", "1m to 1d"),
        ("  remember on, idle 500d", "500d", "1m to 400d"),
        ("  remember on, max 59s", "59s", "1m to 400d"),
    ];
    for (block, points_at, range) in cases {
        let (e, at) = refused(block);
        assert_eq!(e.code, "auth_duration_range", "{block}: {}", e.message);
        assert_eq!(at, *points_at, "{block}");
        assert!(e.message.contains(range), "{block}: {}", e.message);
    }
}

#[test]
fn each_other_bad_value_is_refused_as_invalid_auth() {
    let cases: &[(&str, &str, &str)] = &[
        ("  methods passkey, passkey", "passkey", "listed twice"),
        (
            "  methods gh\n  provider gh preset github\n  provider gh kind oauth2",
            "provider gh kind oauth2",
            "declared twice",
        ),
        (
            "  methods email\n  provider email kind oidc",
            "email",
            "built-in method",
        ),
        (
            "  provisioning domains localhost",
            "localhost",
            "email domain",
        ),
        (
            "  provisioning domains example.com, example.com",
            "example.com",
            "listed twice",
        ),
        (
            "  passkeyName \"  \"",
            "passkeyName \"  \"",
            "cannot be empty",
        ),
        ("  session token 2h, max 1h", "1h", "below its `token`"),
        (
            "  session token 1h, tenants 4h to 15m",
            "tenants 4h to 15m",
            "runs backwards",
        ),
    ];
    for (block, points_at, says) in cases {
        let (e, at) = refused(block);
        assert_eq!(e.code, "invalid_auth", "{block}: {}", e.message);
        assert!(e.message.contains(says), "{block}: {}", e.message);
        assert!(at.ends_with(points_at), "{block}: points at `{at}`");
    }
}

#[test]
fn a_second_block_is_refused() {
    let e = parse_program("auth {\n}\nauth {\n}\n").expect_err("refused");
    assert_eq!(e.code, "duplicate_auth");
}
