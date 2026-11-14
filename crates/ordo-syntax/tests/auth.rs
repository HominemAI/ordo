//! The `auth` block parses (`plan-configuration-in-the-file.md` §2.2), and
//! each clause it cannot read is refused at that clause.

use ordo_syntax::token::DurationUnit;
use ordo_syntax::{
    parse_program, AuthDecl, ParseError, ProviderKind, ProviderSource, Provisioning, RememberMax,
    SecondFactor, UserVerification, DUPLICATE_AUTH,
};

/// The plan's full example, every clause written.
pub const FULL: &str = r#"auth {
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

fn auth(src: &str) -> AuthDecl {
    parse_program(src)
        .unwrap_or_else(|e| panic!("{src}\n{e}"))
        .auth
        .expect("an auth block")
}

fn refused(src: &str) -> ParseError {
    match parse_program(src) {
        Ok(p) => panic!("{src} parsed: {:?}", p.auth),
        Err(e) => e,
    }
}

/// The text the span of `e` covers.
fn at<'a>(src: &'a str, e: &ParseError) -> &'a str {
    &src[e.span.start..e.span.end]
}

#[test]
fn the_full_example_parses_to_every_value_it_states() {
    let a = auth(FULL);
    let names: Vec<&str> = a
        .methods
        .as_ref()
        .unwrap()
        .names
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["passkey", "email", "google", "work"]);

    assert_eq!(a.providers.len(), 2);
    assert_eq!(a.providers[0].name.name, "google");
    assert!(matches!(&a.providers[0].source, ProviderSource::Preset(p) if p.name == "google"));
    assert_eq!(a.providers[0].kind(), Some(ProviderKind::Oidc));
    assert_eq!(a.providers[0].label, None);
    assert_eq!(a.providers[1].name.name, "work");
    assert_eq!(
        a.providers[1].source,
        ProviderSource::Kind(ProviderKind::Oidc)
    );
    assert_eq!(a.providers[1].label.as_deref(), Some("Work account"));

    assert_eq!(a.passkey_name.as_ref().unwrap().text, "Human Agent Link");
    let Provisioning::Domains(ds) = &a.provisioning.as_ref().unwrap().mode else {
        panic!("domains")
    };
    let ds: Vec<&str> = ds.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(ds, ["example.com", "example.org"]);
    assert!(!a.self_serve.as_ref().unwrap().on);

    let g = a.guest.as_ref().unwrap();
    assert!(g.on);
    let keep = g.keep.as_ref().unwrap();
    assert_eq!((keep.n, keep.unit), (7, DurationUnit::D));
    assert_eq!(keep.secs(), Some(7 * 86_400));

    let l = a.linking.as_ref().unwrap();
    assert!(l.on);
    assert_eq!(l.window.as_ref().unwrap().secs(), Some(300));

    assert_eq!(
        a.second_factor.as_ref().unwrap().level,
        SecondFactor::Optional
    );
    assert_eq!(
        a.user_verification.as_ref().unwrap().level,
        UserVerification::Required
    );

    let s = a.session.as_ref().unwrap();
    assert_eq!(s.token.secs(), Some(3_600));
    assert_eq!(s.max.as_ref().unwrap().secs(), Some(12 * 3_600));
    let t = s.tenants.as_ref().unwrap();
    assert_eq!((t.min.secs(), t.max.secs()), (Some(900), Some(4 * 3_600)));

    let r = a.remember.as_ref().unwrap();
    assert!(r.on);
    assert_eq!(r.idle.as_ref().unwrap().secs(), Some(30 * 86_400));
    assert!(matches!(r.max, Some(RememberMax::Unbounded { .. })));
}

/// The block sits beside the other declarations, in any place.
#[test]
fn the_block_sits_among_other_declarations() {
    let src = format!("domain hal @ 1.0.0\n\n{FULL}\nentity notes {{\n  body: text\n}}\n");
    let p = parse_program(&src).expect("parses");
    assert!(p.auth.is_some());
    assert_eq!(p.entities.len(), 1);
}

#[test]
fn an_empty_block_parses_and_every_clause_is_absent() {
    let a = auth("auth {\n}\n");
    assert!(a.methods.is_none() && a.providers.is_empty() && a.session.is_none());
}

#[test]
fn clauses_go_in_any_order_and_options_too() {
    let a = auth(
        "auth {\n  remember on, max 90d, idle 7d\n  methods passkey\n  \
         session token 30m, tenants 5m to 1h, max 8h\n}\n",
    );
    let r = a.remember.unwrap();
    assert_eq!(r.idle.unwrap().secs(), Some(7 * 86_400));
    assert!(matches!(r.max, Some(RememberMax::After(d)) if d.secs() == Some(90 * 86_400)));
    let s = a.session.unwrap();
    assert_eq!(s.max.unwrap().secs(), Some(8 * 3_600));
}

#[test]
fn every_spelling_of_the_closed_words_parses() {
    for (w, level) in [
        ("off", SecondFactor::Off),
        ("optional", SecondFactor::Optional),
        (
            "required-without-passkey",
            SecondFactor::RequiredWithoutPasskey,
        ),
        ("required", SecondFactor::Required),
    ] {
        let a = auth(&format!("auth {{\n  secondFactor {w}\n}}\n"));
        assert_eq!(a.second_factor.unwrap().level, level, "{w}");
    }
    for (w, mode) in [("invite-only", "invite-only"), ("open", "open")] {
        let a = auth(&format!("auth {{\n  provisioning {w}\n}}\n"));
        assert_eq!(a.provisioning.unwrap().mode.mode(), mode);
    }
    let a = auth("auth {\n  userVerification preferred\n  guest off\n  linking off\n  remember off\n  selfServe on\n}\n");
    assert_eq!(
        a.user_verification.unwrap().level,
        UserVerification::Preferred
    );
    assert!(!a.guest.unwrap().on && !a.linking.unwrap().on && !a.remember.unwrap().on);
    assert!(a.self_serve.unwrap().on);
    let a = auth("auth {\n  provider gh kind oauth2, label \"GitHub\"\n}\n");
    assert_eq!(
        a.providers[0].source,
        ProviderSource::Kind(ProviderKind::Oauth2)
    );
}

#[test]
fn a_domain_with_a_hyphen_is_written_in_quotes() {
    let a = auth("auth {\n  provisioning domains \"my-co.example\", example.com\n}\n");
    let Provisioning::Domains(ds) = a.provisioning.unwrap().mode else {
        panic!()
    };
    assert_eq!(ds[0].name, "my-co.example");
}

// ── refusals, one per bad clause ─────────────────────────────────────────

#[test]
fn a_second_block_is_refused() {
    let src = "auth {\n  methods passkey\n}\nauth {\n  methods email\n}\n";
    let e = refused(src);
    assert_eq!(e.code, DUPLICATE_AUTH);
    assert!(at(src, &e).starts_with("auth {\n  methods email"), "{e}");
}

#[test]
fn a_clause_written_twice_is_refused_at_the_second() {
    for clause in [
        "methods passkey",
        "passkeyName \"A\"",
        "provisioning open",
        "selfServe off",
        "guest off",
        "linking off",
        "secondFactor off",
        "userVerification required",
        "session token 1h",
        "remember off",
    ] {
        let src = format!("auth {{\n  {clause}\n  {clause}\n}}\n");
        let e = refused(&src);
        let word = clause.split(' ').next().unwrap();
        assert!(e.message.contains("this is a second"), "{clause}: {e}");
        assert_eq!(at(&src, &e), word, "{clause}");
        assert_eq!(e.span.line, 3, "{clause}");
    }
}

#[test]
fn each_bad_clause_is_refused_where_it_goes_wrong() {
    // (source line, the text the error points at, a phrase of the message)
    let cases: &[(&str, &str, &str)] = &[
        ("colour blue", "colour", "unknown auth clause `colour`"),
        ("methods", "}", "a sign-in method"),
        ("methods passkey,", "}", "a sign-in method"),
        (
            "provider google",
            "}",
            "`preset <name>` or `kind oauth2|oidc`",
        ),
        (
            "provider work kind saml",
            "saml",
            "unknown provider kind `saml`",
        ),
        (
            "provider work kind oidc, colour \"x\"",
            "colour",
            "a provider takes `label",
        ),
        (
            "provider work kind oidc, label work",
            "work",
            "button text in quotes",
        ),
        ("passkeyName hal", "hal", "in quotes"),
        (
            "provisioning everyone",
            "everyone",
            "unknown provisioning `everyone`",
        ),
        (
            "provisioning invite-everyone",
            "invite-everyone",
            "unknown provisioning",
        ),
        ("provisioning domains", "}", "a host"),
        ("selfServe yes", "yes", "`selfServe` is `on` or `off`"),
        ("guest on, keep forever", "forever", "`keep` is a duration"),
        ("guest on, keep 1mo", "1mo", "in s, m, h, d or w"),
        ("guest on, window 5m", "window", "`guest` takes `keep`"),
        ("guest off, keep 7d", "keep", "`guest off` takes no options"),
        (
            "linking off, window 5m",
            "window",
            "`linking off` takes no options",
        ),
        ("linking on, window 500ms", "500ms", "in s, m, h, d or w"),
        ("linking on, window 5m, window 6m", "window", "written once"),
        (
            "secondFactor sometimes",
            "sometimes",
            "unknown secondFactor",
        ),
        (
            "secondFactor required-with-passkey",
            "required-with-passkey",
            "unknown secondFactor",
        ),
        (
            "userVerification discouraged",
            "discouraged",
            "unknown userVerification",
        ),
        ("session max 12h", "max", "begins with the token life"),
        (
            "session token 1h, tenants 15m 4h",
            "4h",
            "`tenants 15m to 4h`",
        ),
        (
            "session token 1h, idle 5m",
            "idle",
            "`session` takes `max` and `tenants`",
        ),
        ("remember on, max forever", "forever", "`max` is a duration"),
        (
            "remember on, keep 7d",
            "keep",
            "`remember` takes `idle` and `max`",
        ),
        (
            "requires mail",
            "requires",
            "not part of the auth block yet",
        ),
    ];
    for (clause, points_at, says) in cases {
        let src = format!("auth {{\n  {clause}\n}}\n");
        let e = refused(&src);
        assert!(
            e.message.contains(says),
            "`{clause}` answered: {}",
            e.message
        );
        assert_eq!(at(&src, &e), *points_at, "`{clause}`: {}", e.message);
    }
}

#[test]
fn the_block_needs_its_braces() {
    let e = refused("auth methods passkey\n");
    assert!(e.message.contains("`{` opening the auth block"), "{e}");
}

/// `auth` is a reserved word, so it is not a field name today; that is
/// unchanged by the block.
#[test]
fn auth_stays_reserved_as_a_field_name() {
    assert!(parse_program("entity users {\n  auth: text\n}\n").is_err());
}
