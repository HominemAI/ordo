//! Passkeys, argon2id, and the end-to-end proof that step-up is reachable.
//!
//! # What a test can and cannot prove here
//!
//! There is no browser and no authenticator in a `cargo test`, so nothing
//! below pretends to run the *ceremony*: no user gesture, no secure enclave,
//! no platform prompt. What it does instead is stand a real authenticator's
//! **output** in front of the verifier — a genuine P-256 or Ed25519 key
//! signing a genuine `authData ‖ SHA-256(clientDataJSON)` inside genuine
//! CTAP2 CBOR — and check that Ordo accepts exactly what it should and
//! refuses everything else. [`TestAuthenticator`] holds a private key and
//! nothing else; every byte it emits is one a real authenticator emits, and
//! every negative case is produced by making it *misbehave* rather than by
//! reaching into the provider.
//!
//! That is the honest line. Ordo's half of WebAuthn is verification, and
//! verification is fully covered. The half a real device owns is stated as
//! left out rather than faked.

use crate::access::{AccessEngine, Decision, DenyReason, TokenClaims};
use crate::directory::{Directory, DirectoryError, LinkCeremony, Provisioning};
use crate::passkey::{
    PasskeyProvider, RelyingParty, RelyingPartyError, UserVerification, CHALLENGE_TTL_SECS,
};
use crate::principal::{
    AccessProfile, AuthMethodKind, CapabilityGrant, GrantId, ManifestHash, PrincipalId,
    PrincipalKind, Role, SpaceId, Timestamp,
};
use crate::provider::{
    Argon2idHasher, AuthError, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeId, ChallengeOrRedirect, CompleteRequest, LocalPasswordProvider, PasswordHasher,
    PlainTestHasher, StubOidcProvider, CEREMONY_TTL_SECS,
};
use crate::registry::ProviderRegistry;
use crate::session::{refresh_presence, sign_in, SessionClaim, SessionError};
use crate::token::{mint, verify, TokenKey};
use crate::webauthn::{b64url_encode, rp_id_hash, signed_message, WebAuthnError};

use crate::software_authenticator::*;

const RP_ID: &str = DEFAULT_RP_ID;
const ORIGIN: &str = DEFAULT_ORIGIN;
const NOW: Timestamp = Timestamp(1_000_000);

// ══════════════════════════════ harness ══════════════════════════════════

fn rp() -> RelyingParty {
    RelyingParty::new(RP_ID, &[ORIGIN]).expect("a well-formed relying party")
}

fn provider() -> PasskeyProvider {
    PasskeyProvider::new(rp())
}

fn registration_challenge(p: &PasskeyProvider, now: Timestamp) -> String {
    p.begin_registration(now, &[])
        .expect("the memory store answers")
        .challenge
}

fn assertion_challenge(p: &PasskeyProvider, now: Timestamp) -> String {
    p.begin_assertion(now, &[])
        .expect("the memory store answers")
        .challenge
}

/// Enroll `device` and return the subject the enrollment produced.
fn enroll(p: &PasskeyProvider, device: &TestAuthenticator, now: Timestamp) -> AuthenticatedSubject {
    let ch = registration_challenge(p, now);
    p.complete(&device.register(&ch), now)
        .expect("a well-formed registration enrolls")
}

// ═══════════════════════ the relying party is validated ══════════════════

#[test]
fn a_relying_party_refuses_the_configurations_that_would_not_work() {
    // Uppercase never matches: browsers lowercase the RP id before hashing.
    assert_eq!(
        RelyingParty::new("Example.com", &[ORIGIN]),
        Err(RelyingPartyError::InvalidRpId {
            rp_id: "Example.com".into()
        })
    );
    // A scheme or a port in the RP id is the commonest mistake.
    assert!(matches!(
        RelyingParty::new("https://example.com", &[ORIGIN]),
        Err(RelyingPartyError::InvalidRpId { .. })
    ));
    assert!(matches!(
        RelyingParty::new("example.com:443", &[ORIGIN]),
        Err(RelyingPartyError::InvalidRpId { .. })
    ));
    assert_eq!(
        RelyingParty::new(RP_ID, &[]),
        Err(RelyingPartyError::NoOrigins)
    );
    assert_eq!(
        RelyingParty::new(RP_ID, &["http://app.example.com"]),
        Err(RelyingPartyError::InsecureOrigin {
            origin: "http://app.example.com".into()
        })
    );
    assert!(matches!(
        RelyingParty::new(RP_ID, &["app.example.com"]),
        Err(RelyingPartyError::InvalidOrigin { .. })
    ));
}

#[test]
fn an_origin_outside_the_rp_id_is_refused_at_configuration_time() {
    // This is the misconfiguration that would make a phishing host
    // authoritative. The browser refuses it anyway; refusing it here turns a
    // ceremony that silently never works into a deploy-time error.
    assert_eq!(
        RelyingParty::new(RP_ID, &["https://example.com.evil.test"]),
        Err(RelyingPartyError::OriginNotUnderRpId {
            origin: "https://example.com.evil.test".into(),
            rp_id: RP_ID.into(),
        })
    );
    // Subdomains of the apex are the point of an apex RP id.
    assert!(RelyingParty::new(RP_ID, &["https://app.example.com", "https://example.com"]).is_ok());
    // Loopback development hosts are the one plaintext exception.
    assert!(RelyingParty::new(RP_ID, &["http://localhost:5173"]).is_ok());
    // 25bc: one label under localhost is a secure context, and the RP id
    // `localhost` covers it. Nothing broader, and not under another RP id.
    assert!(RelyingParty::new(
        "localhost",
        &[
            "http://login.localhost:5180",
            "http://shop.localhost:5181",
            "http://localhost:5182"
        ]
    )
    .is_ok());
    assert!(RelyingParty::new("localhost", &["http://a.b.localhost:5180"]).is_err());
    assert!(RelyingParty::new("localhost", &["http://x.localhost.evil.com"]).is_err());
    assert!(RelyingParty::new(RP_ID, &["http://shop.localhost:5181"]).is_err());
}

// ═════════════════════ registration and assertion work ═══════════════════

#[test]
fn a_registration_enrolls_a_credential_and_proves_presence() {
    let p = provider();
    let device = TestAuthenticator::es256();
    let subject = enroll(&p, &device, NOW);

    assert_eq!(subject.method, AuthMethodKind::Passkey);
    assert_eq!(subject.authenticator, device.authenticator_id());
    let presence = subject
        .presence
        .expect("a passkey ceremony proves presence");
    assert_eq!(presence.at(), NOW, "the stamp is the server's clock");
    assert!(presence.user_verified());
    assert_eq!(
        p.credential(&device.credential_id)
            .expect("enrolled")
            .algorithm(),
        -7
    );
}

#[test]
fn an_assertion_verifies_and_mints_presence() {
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);

    let later = Timestamp(NOW.0 + 60);
    let ch = assertion_challenge(&p, later);
    let subject = p
        .complete(&device.assert(&ch), later)
        .expect("a well-formed assertion verifies");
    assert_eq!(subject.authenticator, device.authenticator_id());
    assert_eq!(subject.presence.expect("presence").at(), later);
}

#[test]
fn ed25519_credentials_work_too() {
    let p = provider();
    let device = TestAuthenticator::ed25519();
    let subject = enroll(&p, &device, NOW);
    assert_eq!(
        p.credential(&device.credential_id)
            .expect("enrolled")
            .algorithm(),
        -8
    );
    assert!(subject.presence.is_some());

    let ch = assertion_challenge(&p, NOW);
    assert!(p.complete(&device.assert(&ch), NOW).is_ok());
}

// ═══════════════════════════ the refusals ════════════════════════════════

#[test]
fn a_tampered_signature_and_an_unknown_credential_are_the_same_refusal() {
    // Anti-enumeration (`security-logging-design.md` §2): probing credential
    // ids must not be a way to learn which ones exist.
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);

    let ch = assertion_challenge(&p, NOW);
    let mut req = device.assert(&ch);
    if let CompleteRequest::PasskeyAssertion { signature, .. } = &mut req {
        let last = signature.len() - 1;
        signature[last] ^= 0x01;
    }
    assert_eq!(p.complete(&req, NOW), Err(AuthError::BadCredentials));

    let ch = assertion_challenge(&p, NOW);
    let stranger = TestAuthenticator {
        credential_id: b"never-enrolled".to_vec(),
        ..TestAuthenticator::es256()
    };
    assert_eq!(
        p.complete(&stranger.assert(&ch), NOW),
        Err(AuthError::BadCredentials)
    );
}

#[test]
fn an_assertion_made_on_another_site_is_refused() {
    // The phishing case. The signature is genuine; it covers a clientData
    // whose origin is the attacker's page, and the origin check is what makes
    // it worthless here.
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&device.assert_at(&ch, "https://app.evil.test"), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::OriginNotAllowed))
    );
}

#[test]
fn an_assertion_for_another_relying_party_is_refused() {
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let foreign = TestAuthenticator {
        rp_id: "other.test".into(),
        ..TestAuthenticator::es256()
    };
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&foreign.assert(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::RpIdMismatch))
    );
}

#[test]
fn a_cross_origin_ceremony_is_refused() {
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let ch = assertion_challenge(&p, NOW);
    let client_data_json = device.client_data("webauthn.get", &ch, ORIGIN, true);
    let authenticator_data = device.auth_data(false);
    let signature = device
        .key
        .sign(&signed_message(&authenticator_data, &client_data_json));
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyAssertion {
                challenge: ChallengeId(ch),
                credential_id: device.credential_id.clone(),
                client_data_json,
                authenticator_data,
                signature,
            },
            NOW
        ),
        Err(AuthError::WebAuthn(WebAuthnError::CrossOrigin))
    );
}

#[test]
fn no_user_presence_means_no_presence_assertion() {
    // The load-bearing one. If this check went missing, a silent authenticator
    // — one that signed without anybody touching it — would mint a step-up
    // proof, and every step-up op in the platform would be satisfiable by a
    // stolen device sitting in a drawer.
    let p = provider();
    let absent = TestAuthenticator {
        user_present: false,
        user_verified: false,
        ..TestAuthenticator::es256()
    };
    let ch = registration_challenge(&p, NOW);
    assert_eq!(
        p.complete(&absent.register(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::UserPresenceMissing))
    );
    // …and the credential was not enrolled on the way past.
    assert!(p.credential(&absent.credential_id).is_none());
}

#[test]
fn user_verification_is_required_by_default_and_visible_when_it_is_not() {
    let p = provider();
    let touch_only = TestAuthenticator {
        user_verified: false,
        ..TestAuthenticator::es256()
    };
    let ch = registration_challenge(&p, NOW);
    assert_eq!(
        p.complete(&touch_only.register(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::UserVerificationMissing))
    );

    // A deployment that carries UV-incapable keys relaxes it — and the weaker
    // proof shows up in the presence rather than disappearing.
    let lenient = PasskeyProvider::new(rp().with_user_verification(UserVerification::Preferred));
    let ch = registration_challenge(&lenient, NOW);
    let subject = lenient
        .complete(&touch_only.register(&ch), NOW)
        .expect("presence without verification is still presence");
    assert!(!subject.presence.expect("presence").user_verified());
}

#[test]
fn a_challenge_is_one_shot_and_expires() {
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);

    let ch = assertion_challenge(&p, NOW);
    let req = device.assert(&ch);
    assert!(p.complete(&req, NOW).is_ok());
    // Replaying the whole successful response finds no challenge to match.
    assert_eq!(
        p.complete(&req, NOW),
        Err(AuthError::UnknownChallenge(ChallengeId(ch)))
    );

    // And an unanswered challenge dies with its window.
    let ch = assertion_challenge(&p, NOW);
    let stale = Timestamp(NOW.0 + CHALLENGE_TTL_SECS + 1);
    assert_eq!(
        p.complete(&device.assert(&ch), stale),
        Err(AuthError::UnknownChallenge(ChallengeId(ch)))
    );
}

#[test]
fn a_registration_challenge_cannot_answer_a_sign_in() {
    // Two independent checks catch this crossing: the challenge's recorded
    // purpose, and `clientData.type`. Both are asserted, because either one
    // alone would leave the other untested.
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);

    let reg_ch = registration_challenge(&p, NOW);
    assert_eq!(
        p.complete(&device.assert(&reg_ch), NOW),
        Err(AuthError::UnknownChallenge(ChallengeId(reg_ch)))
    );

    // Now with a legitimate assertion challenge but a `webauthn.create`
    // clientData: the purpose check passes and the type check catches it.
    let ch = assertion_challenge(&p, NOW);
    let client_data_json = device.client_data("webauthn.create", &ch, ORIGIN, false);
    let authenticator_data = device.auth_data(false);
    let signature = device
        .key
        .sign(&signed_message(&authenticator_data, &client_data_json));
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyAssertion {
                challenge: ChallengeId(ch),
                credential_id: device.credential_id.clone(),
                client_data_json,
                authenticator_data,
                signature,
            },
            NOW
        ),
        Err(AuthError::WebAuthn(WebAuthnError::ClientDataTypeMismatch {
            expected: "webauthn.get"
        }))
    );
}

#[test]
fn a_signature_counter_that_goes_backwards_is_named_as_a_clone() {
    let p = provider();
    let mut device = TestAuthenticator {
        sign_count: 5,
        ..TestAuthenticator::es256()
    };
    enroll(&p, &device, NOW);

    device.sign_count = 6;
    let ch = assertion_challenge(&p, NOW);
    assert!(p.complete(&device.assert(&ch), NOW).is_ok());
    assert_eq!(p.credential(&device.credential_id).unwrap().sign_count(), 6);

    // A second device with the same credential, replaying an older count.
    device.sign_count = 6;
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&device.assert(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::SignCountRegression {
            stored: 6,
            presented: 6,
        }))
    );
}

#[test]
fn a_counter_that_stays_at_zero_is_not_a_clone() {
    // Platform passkeys (Apple, Google) do not keep a counter and report 0
    // forever. Treating that as a regression would refuse every phone.
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    for _ in 0..3 {
        let ch = assertion_challenge(&p, NOW);
        assert!(p.complete(&device.assert(&ch), NOW).is_ok());
    }
}

#[test]
fn a_counter_alert_cannot_be_provoked_without_a_signature() {
    // The counter is compared after the signature verifies, so a caller who
    // cannot sign cannot raise a clone alarm on somebody else's credential —
    // which would be a denial of service dressed as a security signal.
    let p = provider();
    let device = TestAuthenticator {
        sign_count: 9,
        ..TestAuthenticator::es256()
    };
    enroll(&p, &device, NOW);
    let regressed = TestAuthenticator {
        key: TestKey::es256(11),
        sign_count: 1,
        ..TestAuthenticator::es256()
    };
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&regressed.assert(&ch), NOW),
        Err(AuthError::BadCredentials)
    );
    assert_eq!(p.credential(&device.credential_id).unwrap().sign_count(), 9);
}

#[test]
fn a_credential_cannot_be_re_registered_over_itself() {
    // Overwriting a live public key under an unchanged authenticator id would
    // hand the account over with nothing in the directory to show for it.
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let impostor = TestAuthenticator {
        key: TestKey::es256(23),
        ..TestAuthenticator::es256()
    };
    let ch = registration_challenge(&p, NOW);
    assert_eq!(
        p.complete(&impostor.register(&ch), NOW),
        Err(AuthError::CredentialAlreadyEnrolled)
    );
}

#[test]
fn attestation_formats_other_than_none_are_refused() {
    let p = provider();
    let device = TestAuthenticator::es256();
    let ch = registration_challenge(&p, NOW);
    let req = CompleteRequest::PasskeyRegistration {
        challenge: ChallengeId(ch.clone()),
        client_data_json: device.client_data("webauthn.create", &ch, ORIGIN, false),
        attestation_object: device.attestation_object("packed"),
    };
    assert_eq!(
        p.complete(&req, NOW),
        Err(AuthError::WebAuthn(WebAuthnError::UnsupportedAttestation {
            fmt: "packed".into()
        }))
    );
}

#[test]
fn malformed_bytes_are_refused_rather_than_interpreted() {
    let p = provider();
    let device = TestAuthenticator::es256();

    // Truncated attestation object.
    let ch = registration_challenge(&p, NOW);
    let mut att = device.attestation_object("none");
    att.truncate(att.len() / 2);
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyRegistration {
                challenge: ChallengeId(ch.clone()),
                client_data_json: device.client_data("webauthn.create", &ch, ORIGIN, false),
                attestation_object: att,
            },
            NOW
        ),
        Err(AuthError::WebAuthn(WebAuthnError::MalformedCbor))
    );

    // Trailing bytes after authenticatorData. `authData` is signed as a whole;
    // bytes the structure did not account for mean we split it wrongly.
    enroll(&p, &device, NOW);
    let ch = assertion_challenge(&p, NOW);
    let client_data_json = device.client_data("webauthn.get", &ch, ORIGIN, false);
    let mut authenticator_data = device.auth_data(false);
    authenticator_data.push(0xff);
    let signature = device
        .key
        .sign(&signed_message(&authenticator_data, &client_data_json));
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyAssertion {
                challenge: ChallengeId(ch),
                credential_id: device.credential_id.clone(),
                client_data_json,
                authenticator_data,
                signature,
            },
            NOW
        ),
        Err(AuthError::WebAuthn(
            WebAuthnError::MalformedAuthenticatorData
        ))
    );

    // clientDataJSON that is not JSON at all.
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyAssertion {
                challenge: ChallengeId(ch),
                credential_id: device.credential_id.clone(),
                client_data_json: b"not json".to_vec(),
                authenticator_data: device.auth_data(false),
                signature: vec![0u8; 70],
            },
            NOW
        ),
        Err(AuthError::WebAuthn(WebAuthnError::MalformedClientData))
    );
}

#[test]
fn a_cose_algorithm_we_do_not_advertise_is_refused() {
    // RS256. An authenticator only ever picks from `pubKeyCredParams`, so this
    // arriving at all means a client ignored the options it was handed.
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut cose = cbor_map(3);
    cose.extend(cbor_int(1));
    cose.extend(cbor_int(3)); // kty: RSA
    cose.extend(cbor_int(3));
    cose.extend(cbor_int(-257)); // alg: RS256
    cose.extend(cbor_int(-1));
    cose.extend(cbor_bstr(&[0u8; 256]));

    let mut auth_data = Vec::new();
    auth_data.extend_from_slice(&rp_id_hash(RP_ID));
    auth_data.push(FLAG_UP | FLAG_UV | FLAG_AT);
    auth_data.extend_from_slice(&0u32.to_be_bytes());
    auth_data.extend_from_slice(&[0u8; 16]);
    auth_data.extend_from_slice(&(device.credential_id.len() as u16).to_be_bytes());
    auth_data.extend_from_slice(&device.credential_id);
    auth_data.extend(cose);

    let mut att = cbor_map(3);
    att.extend(cbor_tstr("fmt"));
    att.extend(cbor_tstr("none"));
    att.extend(cbor_tstr("attStmt"));
    att.extend(cbor_map(0));
    att.extend(cbor_tstr("authData"));
    att.extend(cbor_bstr(&auth_data));

    let ch = registration_challenge(&p, NOW);
    assert_eq!(
        p.complete(
            &CompleteRequest::PasskeyRegistration {
                challenge: ChallengeId(ch.clone()),
                client_data_json: device.client_data("webauthn.create", &ch, ORIGIN, false),
                attestation_object: att,
            },
            NOW
        ),
        Err(AuthError::WebAuthn(WebAuthnError::UnsupportedAlgorithm {
            alg: -257
        }))
    );
}

#[test]
fn the_provider_refuses_another_methods_payload() {
    let p = provider();
    assert_eq!(
        p.complete(
            &CompleteRequest::Password {
                challenge: ChallengeId("x".into()),
                identifier: "alice".into(),
                password: "p".into(),
            },
            NOW
        ),
        Err(AuthError::MethodMismatch {
            provider: AuthMethodKind::Passkey
        })
    );
}

#[test]
fn the_trait_begin_issues_an_assertion_challenge() {
    let p = provider();
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let ChallengeOrRedirect::Challenge { id, method } =
        p.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("a passkey ceremony is a challenge, never a redirect");
    };
    assert_eq!(method, AuthMethodKind::Passkey);
    assert!(p.complete(&device.assert(&id.0), NOW).is_ok());
}

#[test]
fn passkey_is_a_row_in_the_auth_block_like_any_other_method() {
    // `auth { methods passkey, password, google }` maps 1:1 onto the registry
    // (`security-logging-design.md` §2). The step-up source is not a special
    // case in the routing; it is a method name like the rest, and the login
    // endpoint that routes by name never learns which family answered.
    let mut r = ProviderRegistry::new();
    r.register("passkey", Box::new(provider())).unwrap();
    r.register(
        "password",
        Box::new(LocalPasswordProvider::new(Argon2idHasher::weak_for_tests())),
    )
    .unwrap();
    r.register(
        "google",
        Box::new(StubOidcProvider::new("google", "https://idp/a", "ordo")),
    )
    .unwrap();

    assert_eq!(r.get("passkey").unwrap().kind(), AuthMethodKind::Passkey);
    let methods: Vec<_> = r.methods().collect();
    assert_eq!(
        methods,
        vec![
            ("google", AuthMethodKind::Oidc),
            ("passkey", AuthMethodKind::Passkey),
            ("password", AuthMethodKind::Password),
        ]
    );
}

// ═════════════════════════════ argon2id ══════════════════════════════════

#[test]
fn argon2id_pins_the_parameters_in_the_stored_hash() {
    // The pin, asserted rather than merely commented. If somebody turns the
    // cost down, this fails and names the new value.
    let h = Argon2idHasher::new().hash("correct horse battery staple");
    assert!(
        h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
        "argon2id parameters are not the pinned OWASP profile: {h}"
    );
}

#[test]
fn argon2id_salts_every_hash_and_verifies_only_the_right_password() {
    let h = Argon2idHasher::weak_for_tests();
    let a = h.hash("hunter2");
    let b = h.hash("hunter2");
    assert_ne!(a, b, "the same password must not hash to the same string");
    assert!(h.verify("hunter2", &a));
    assert!(h.verify("hunter2", &b));
    assert!(!h.verify("hunter3", &a));
    // A corrupt stored hash refuses a login; it does not take the process down.
    assert!(!h.verify("hunter2", "not-a-phc-string"));
    assert!(!h.verify("hunter2", ""));
    // And the old plaintext-shaped "hash" is not a hash any more.
    assert!(!h.verify("hunter2", "plain-test$hunter2"));
}

#[test]
fn a_password_provider_backed_by_argon2id_round_trips() {
    let p = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    p.enroll("alice", "correct horse").expect("enrolled");
    let ChallengeOrRedirect::Challenge { id, .. } =
        p.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("password is a challenge");
    };
    let subject = p
        .complete(
            &CompleteRequest::Password {
                challenge: id,
                identifier: "alice".into(),
                password: "correct horse".into(),
            },
            NOW,
        )
        .expect("good credentials verify");
    assert_eq!(
        subject.authenticator,
        AuthenticatorId("password:alice".into())
    );
    assert!(
        subject.presence.is_none(),
        "a password is a secret, not a human in the room"
    );
}

// ══════════════════ ceremony handles are time-boxed ══════════════════════

#[test]
fn a_password_challenge_expires_and_reads_as_unknown() {
    // Found while adding the clock: the pending maps had no expiry at all, so
    // a challenge or a `state` observed once stayed redeemable for the life of
    // the process. One-shot was enforced; one-*time* was not.
    let p = LocalPasswordProvider::new(PlainTestHasher);
    p.enroll("alice", "pw").expect("enrolled");
    let ChallengeOrRedirect::Challenge { id, .. } =
        p.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("password is a challenge");
    };
    let req = CompleteRequest::Password {
        challenge: id.clone(),
        identifier: "alice".into(),
        password: "pw".into(),
    };
    let stale = Timestamp(NOW.0 + CEREMONY_TTL_SECS + 1);
    assert_eq!(
        p.complete(&req, stale),
        Err(AuthError::UnknownChallenge(id)),
        "an expired challenge is indistinguishable from one that never existed"
    );
}

#[test]
fn an_oidc_state_expires() {
    let p = StubOidcProvider::new("google", "https://idp.example/authorize", "ordo");
    let ChallengeOrRedirect::Redirect { url, .. } =
        p.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("oidc is a redirect");
    };
    let state = url
        .split("state=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .expect("state")
        .to_string();
    let nonce = url.split("nonce=").nth(1).expect("nonce").to_string();
    p.issue("c", "s", &nonce);
    let stale = Timestamp(NOW.0 + CEREMONY_TTL_SECS + 1);
    assert_eq!(
        p.complete(
            &CompleteRequest::OidcCallback {
                state: state.clone(),
                code: "c".into(),
                iss: None,
            },
            stale
        ),
        Err(AuthError::StateMismatch(state))
    );
}

// ══════════════ the gate: step-up is now actually satisfiable ════════════

fn space() -> SpaceId {
    SpaceId("spc_clinic".into())
}

/// An engine where `transfer.approve` is step-up gated and `txns.list` is not.
fn gated_engine(owner: &PrincipalId) -> AccessEngine {
    let mut e = AccessEngine::new();
    e.set_profile(
        &space(),
        AccessProfile::new()
            .allow(Role::Owner, "txns.list")
            .allow(Role::Owner, "transfer.approve"),
    );
    e.set_membership(&space(), owner, Role::Owner);
    e.require_step_up("transfer.approve");
    e
}

/// The whole loop, with nothing stubbed between the signature and the door.
fn door_answer(
    engine: &AccessEngine,
    key: &TokenKey,
    token: &str,
    op: &str,
    at: Timestamp,
) -> Decision {
    let claims = verify(key, token, at).expect("a live token verifies");
    engine.check(&claims, &space(), op, at)
}

#[test]
fn a_passkey_sign_in_satisfies_step_up_end_to_end() {
    // THE test this work exists for. Before it, `StepUpRequirement` was a gate
    // with no key: nothing in the crate could produce a `user_presence_at`, so
    // every step-up op denied for every caller in every deployment.
    //
    // Ceremony → subject → directory → token → verify → door. No step is
    // simulated and no claim is hand-built.
    let p = provider();
    let device = TestAuthenticator::es256();

    let mut directory = Directory::new(Provisioning::InviteOnly);
    let principal = directory
        .create_principal(PrincipalKind::Human, "Dr. Reyes", NOW)
        .unwrap();

    // The invitation being accepted: enroll, then bootstrap-link.
    let enrolled = enroll(&p, &device, NOW);
    directory
        .link_first(&principal, &enrolled.authenticator, NOW)
        .expect("the account holds no factor yet");

    // Sign in with an assertion.
    let ch = assertion_challenge(&p, NOW);
    let subject = p
        .complete(&device.assert(&ch), NOW)
        .expect("the assertion verifies");
    let session =
        sign_in(&mut directory, &subject, "Dr. Reyes", NOW).expect("linked authenticator");
    assert_eq!(session.principal, principal);

    let key = TokenKey::from_secret(b"deployment-secret");
    let token = mint(
        &key,
        &session.token_spec(SessionClaim::starting_at(NOW), Timestamp(NOW.0 + 900), None),
    );

    let engine = gated_engine(&principal);
    assert!(
        door_answer(&engine, &key, &token, "transfer.approve", NOW).is_allow(),
        "a fresh passkey presence must open a step-up gated op"
    );
    assert!(door_answer(&engine, &key, &token, "txns.list", NOW).is_allow());
}

#[test]
fn the_same_loop_with_a_password_is_denied_at_the_step_up_gate() {
    // The control. Identical plumbing; only the ceremony differs, and the
    // difference is decided by the type system rather than by a check.
    let pw = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    pw.enroll("reyes", "correct horse").expect("enrolled");
    let ChallengeOrRedirect::Challenge { id, .. } =
        pw.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("password is a challenge");
    };
    let subject = pw
        .complete(
            &CompleteRequest::Password {
                challenge: id,
                identifier: "reyes".into(),
                password: "correct horse".into(),
            },
            NOW,
        )
        .expect("good credentials");

    let mut directory = Directory::new(Provisioning::InviteOnly);
    let principal = directory
        .create_principal(PrincipalKind::Human, "Dr. Reyes", NOW)
        .unwrap();
    directory
        .link_first(&principal, &subject.authenticator, NOW)
        .unwrap();
    let session = sign_in(&mut directory, &subject, "Dr. Reyes", NOW).unwrap();

    let key = TokenKey::from_secret(b"deployment-secret");
    let token = mint(
        &key,
        &session.token_spec(SessionClaim::starting_at(NOW), Timestamp(NOW.0 + 900), None),
    );

    let engine = gated_engine(&principal);
    assert!(
        door_answer(&engine, &key, &token, "txns.list", NOW).is_allow(),
        "an ungated op is unaffected — this is not a broken session"
    );
    assert_eq!(
        door_answer(&engine, &key, &token, "transfer.approve", NOW),
        Decision::Deny {
            op: "transfer.approve".into(),
            reason: DenyReason::StepUpRequired,
        }
    );
}

#[test]
fn presence_goes_stale_while_the_token_stays_valid() {
    // Presence is a moment, not a bit set at login. The token is still live
    // and the ungated op still works; only the gated one closes.
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let enrolled = enroll(&p, &device, NOW);
    let session = sign_in(&mut directory, &enrolled, "Dr. Reyes", NOW).unwrap();

    let key = TokenKey::from_secret(b"deployment-secret");
    let token = mint(
        &key,
        &session.token_spec(
            SessionClaim::starting_at(NOW),
            Timestamp(NOW.0 + 86_400),
            None,
        ),
    );
    let engine = gated_engine(&session.principal);

    let window = engine.step_up_max_age_secs();
    let just_inside = Timestamp(NOW.0 + window);
    let just_outside = Timestamp(NOW.0 + window + 1);
    assert!(door_answer(&engine, &key, &token, "transfer.approve", just_inside).is_allow());
    assert_eq!(
        door_answer(&engine, &key, &token, "transfer.approve", just_outside),
        Decision::Deny {
            op: "transfer.approve".into(),
            reason: DenyReason::StepUpStale {
                age_secs: window + 1,
                max_age_secs: window,
            },
        }
    );
    assert!(
        door_answer(&engine, &key, &token, "txns.list", just_outside).is_allow(),
        "a stale presence closes the gated op and nothing else"
    );
}

#[test]
fn a_step_up_prompt_re_opens_the_gate_on_a_stale_session() {
    // The other half of the loop: not a fresh sign-in but a re-assertion
    // inside a session that already exists — which is what a step-up prompt
    // is, and what a deployment will call most often.
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let enrolled = enroll(&p, &device, NOW);
    let session = sign_in(&mut directory, &enrolled, "Dr. Reyes", NOW).unwrap();

    let key = TokenKey::from_secret(b"deployment-secret");
    let engine = gated_engine(&session.principal);
    let expires = Timestamp(NOW.0 + 86_400);
    let token = mint(
        &key,
        &session.token_spec(SessionClaim::starting_at(NOW), expires, None),
    );

    let much_later = Timestamp(NOW.0 + 40_000);
    assert!(!door_answer(&engine, &key, &token, "transfer.approve", much_later).is_allow());

    let claims = verify(&key, &token, much_later).expect("the token is still live");
    let ch = assertion_challenge(&p, much_later);
    let fresh = p
        .complete(&device.assert(&ch), much_later)
        .expect("the assertion verifies");
    let stepped = refresh_presence(&directory, &claims, &fresh, expires).expect("same principal");
    let token = mint(&key, &stepped);

    assert!(door_answer(&engine, &key, &token, "transfer.approve", much_later).is_allow());
}

#[test]
fn a_step_up_must_be_asserted_by_the_sessions_own_authenticator() {
    // The attack a step-up prompt exists to stop: a stolen bearer token, and
    // an attacker who owns a perfectly good passkey of their own. The
    // assertion is genuine, the presence is genuine, and it is not this
    // session's. Nothing upstream can catch this — the provider knows
    // credentials and not principals, and the token was minted long ago.
    let p = provider();
    let victim_device = TestAuthenticator::es256();
    let attacker_device = TestAuthenticator {
        key: TestKey::es256(31),
        credential_id: b"attacker-key".to_vec(),
        ..TestAuthenticator::es256()
    };

    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let victim = sign_in(
        &mut directory,
        &enroll(&p, &victim_device, NOW),
        "Victim",
        NOW,
    )
    .unwrap();
    // The attacker has an account here too — enrolled, linked, ordinary.
    sign_in(
        &mut directory,
        &enroll(&p, &attacker_device, NOW),
        "Attacker",
        NOW,
    )
    .unwrap();

    let key = TokenKey::from_secret(b"deployment-secret");
    let expires = Timestamp(NOW.0 + 86_400);
    let stolen = mint(
        &key,
        &victim.token_spec(SessionClaim::starting_at(NOW), expires, None),
    );
    let later = Timestamp(NOW.0 + 40_000);
    let claims = verify(&key, &stolen, later).unwrap();

    let ch = assertion_challenge(&p, later);
    let attackers_presence = p
        .complete(&attacker_device.assert(&ch), later)
        .expect("the attacker's own assertion is perfectly valid");

    assert_eq!(
        refresh_presence(&directory, &claims, &attackers_presence, expires),
        Err(SessionError::PresenceForAnotherPrincipal {
            session: victim.principal.clone(),
            authenticator: attacker_device.authenticator_id(),
        })
    );

    // And the victim's own authenticator does open it, so the refusal above
    // is a binding check and not a blanket one.
    let ch = assertion_challenge(&p, later);
    let ours = p.complete(&victim_device.assert(&ch), later).unwrap();
    assert!(refresh_presence(&directory, &claims, &ours, expires).is_ok());
}

#[test]
fn a_capability_token_can_never_step_up() {
    // Presence is not delegable (`security-logging-design.md` §5). The engine
    // already refused this; the refresh path has to refuse it too, or an agent
    // could mint itself a presence-bearing token and walk in through the front.
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let session = sign_in(
        &mut directory,
        &enroll(&p, &device, NOW),
        "Agent Runner",
        NOW,
    )
    .unwrap();

    let pins = std::collections::BTreeMap::from([(
        "transfer.approve".to_string(),
        crate::pins::OpPin {
            manifest_hash: ManifestHash([0u8; 32]),
            unguarded: false,
            entities: Vec::new(),
        },
    )]);
    let claims = TokenClaims {
        principal: session.principal.clone(),
        capability: Some(CapabilityGrant::new(
            GrantId("gr_1".into()),
            space(),
            pins,
            Timestamp(NOW.0 + 86_400),
        )),
        client: None,
        user_presence_at: None,
        delegation: None,
        session: None,
    };

    let ch = assertion_challenge(&p, NOW);
    let fresh = p.complete(&device.assert(&ch), NOW).unwrap();
    assert_eq!(
        refresh_presence(&directory, &claims, &fresh, Timestamp(NOW.0 + 900)),
        Err(SessionError::PresenceNotDelegable {
            principal: session.principal
        })
    );
}

#[test]
fn a_password_re_prompt_is_not_a_step_up() {
    // Re-authenticating and being present are different claims. A deployment
    // that wired a password re-prompt into the step-up path would be gating on
    // a secret an attacker already has.
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let session = sign_in(&mut directory, &enroll(&p, &device, NOW), "Dr. Reyes", NOW).unwrap();

    let key = TokenKey::from_secret(b"deployment-secret");
    let expires = Timestamp(NOW.0 + 86_400);
    let claims = verify(
        &key,
        &mint(
            &key,
            &session.token_spec(SessionClaim::starting_at(NOW), expires, None),
        ),
        NOW,
    )
    .unwrap();

    let pw = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests());
    pw.enroll("reyes", "correct horse").expect("enrolled");
    let ChallengeOrRedirect::Challenge { id, .. } =
        pw.begin(&BeginRequest::default(), NOW).expect("begins")
    else {
        panic!("password is a challenge");
    };
    let subject = pw
        .complete(
            &CompleteRequest::Password {
                challenge: id,
                identifier: "reyes".into(),
                password: "correct horse".into(),
            },
            NOW,
        )
        .unwrap();

    assert_eq!(
        refresh_presence(&directory, &claims, &subject, expires),
        Err(SessionError::MethodCannotProvePresence {
            method: AuthMethodKind::Password
        })
    );
}

/// A step-up re-mints the token and must **not** re-mint the session.
///
/// If it started a new one, stepping up would be a way out of a logout: the
/// holder of a revoked token runs a passkey ceremony, gets a token naming a
/// session the deny list has never heard of, and is back in.
#[test]
fn a_step_up_keeps_the_session_it_stepped_up_in() {
    let p = provider();
    let device = TestAuthenticator::es256();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let session = sign_in(&mut directory, &enroll(&p, &device, NOW), "Dr. Reyes", NOW).unwrap();

    let started = SessionClaim::starting_at(NOW);
    let key = TokenKey::from_secret(b"deployment-secret");
    let expires = Timestamp(NOW.0 + 900);
    let token = mint(&key, &session.token_spec(started.clone(), expires, None));
    let claims = verify(&key, &token, NOW).unwrap();

    let later = Timestamp(NOW.0 + 300);
    let ch = assertion_challenge(&p, later);
    let fresh = p
        .complete(&device.assert(&ch), later)
        .expect("the assertion verifies");

    let spec = refresh_presence(&directory, &claims, &fresh, Timestamp(later.0 + 900))
        .expect("the authenticator is this session's");
    assert_eq!(
        spec.session,
        Some(started.with_via("passkey".into())),
        "the same session, with fresh presence"
    );
    assert_eq!(spec.user_presence_at, Some(later));
}

// ═══════════ the linking ceremony now has a key too (Z-006) ══════════════

#[test]
fn a_second_authenticator_can_now_actually_be_linked() {
    // `LinkCeremony::from_claims` demands a presence stamp, so before this
    // work the dual-proof link was as unreachable as step-up: a deployment
    // could enroll a first factor and never a second. The whole point of
    // "one human, many linked authenticators" was inert.
    let p = provider();
    let first = TestAuthenticator::es256();
    let second = TestAuthenticator {
        key: TestKey::es256(41),
        credential_id: b"second-key".to_vec(),
        ..TestAuthenticator::es256()
    };

    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let session = sign_in(&mut directory, &enroll(&p, &first, NOW), "Dr. Reyes", NOW).unwrap();
    let key = TokenKey::from_secret(b"deployment-secret");
    let token = mint(
        &key,
        &session.token_spec(SessionClaim::starting_at(NOW), Timestamp(NOW.0 + 900), None),
    );
    let claims = verify(&key, &token, NOW).unwrap();

    // Enrolling the second credential is its own ceremony; linking it is the
    // audited command, vouched for by the first.
    let enrolled = enroll(&p, &second, NOW);
    let ceremony = LinkCeremony::from_claims(&claims, &first.authenticator_id())
        .expect("the session carries a fresh presence");
    let event = directory
        .link(&session.principal, &enrolled.authenticator, &ceremony, NOW)
        .expect("a fresh presence from an already-linked factor authorizes the link");
    assert_eq!(
        event.existing_authenticator,
        Some(first.authenticator_id()),
        "the audit event names what authorized the new factor"
    );
    assert_eq!(
        directory.principal_for(&second.authenticator_id()).unwrap(),
        Some(session.principal.clone())
    );

    // And the same link from a session whose presence has gone stale is not.
    let stale_claims = TokenClaims {
        user_presence_at: Some(Timestamp(NOW.0 - 100_000)),
        ..claims
    };
    let third = TestAuthenticator {
        key: TestKey::es256(43),
        credential_id: b"third-key".to_vec(),
        ..TestAuthenticator::es256()
    };
    let enrolled3 = enroll(&p, &third, NOW);
    let stale_ceremony =
        LinkCeremony::from_claims(&stale_claims, &first.authenticator_id()).unwrap();
    assert!(matches!(
        directory.link(
            &session.principal,
            &enrolled3.authenticator,
            &stale_ceremony,
            NOW
        ),
        Err(DirectoryError::StepUpStale { .. })
    ));
}

// ═══════════════════ two providers over one pair of stores ═══════════════

fn shared_stores() -> (
    std::sync::Arc<crate::passkey::MemoryPasskeyStore>,
    std::sync::Arc<crate::ceremony::MemoryCeremonyStore>,
) {
    (
        std::sync::Arc::new(crate::passkey::MemoryPasskeyStore::new()),
        std::sync::Arc::new(crate::ceremony::MemoryCeremonyStore::new()),
    )
}

/// A second provider over the stores of a first: another replica, or the
/// same process after a restart. It has none of the first one's memory.
fn provider_over(
    keys: &std::sync::Arc<crate::passkey::MemoryPasskeyStore>,
    ceremonies: &std::sync::Arc<crate::ceremony::MemoryCeremonyStore>,
) -> PasskeyProvider {
    PasskeyProvider::with_stores(rp(), keys.clone(), ceremonies.clone())
}

#[test]
fn a_credential_registered_through_one_provider_asserts_through_another_over_the_same_store() {
    let (keys, ceremonies) = shared_stores();
    let one = provider_over(&keys, &ceremonies);
    let two = provider_over(&keys, &ceremonies);
    let device = TestAuthenticator::es256();
    enroll(&one, &device, NOW);

    // The challenge is minted by the first and answered by the second.
    let ch = assertion_challenge(&one, NOW);
    let subject = two
        .complete(&device.assert(&ch), NOW)
        .expect("the second provider knows the first one's credential");
    assert_eq!(subject.authenticator, device.authenticator_id());
    assert!(subject.presence.is_some());
}

#[test]
fn a_challenge_taken_once_is_gone_for_every_provider_over_the_store() {
    let (keys, ceremonies) = shared_stores();
    let one = provider_over(&keys, &ceremonies);
    let two = provider_over(&keys, &ceremonies);
    let device = TestAuthenticator::es256();
    enroll(&one, &device, NOW);

    let ch = assertion_challenge(&one, NOW);
    let req = device.assert(&ch);
    assert!(one.complete(&req, NOW).is_ok());
    // The replay reaches the other replica, which never saw the challenge
    // and finds none: the row was deleted by the first.
    assert_eq!(
        two.complete(&req, NOW),
        Err(AuthError::UnknownChallenge(ChallengeId(ch)))
    );
    assert!(ceremonies.is_empty(), "nothing is left open");
}

#[test]
fn a_challenge_is_stored_by_its_hash_and_never_by_itself() {
    let (keys, ceremonies) = shared_stores();
    let p = provider_over(&keys, &ceremonies);
    let ch = assertion_challenge(&p, NOW);
    // The row is filed under a handle that is not the challenge, so the
    // challenge presented is what finds it and a reader of the table cannot
    // present one.
    use crate::ceremony::CeremonyStore as _;
    assert_eq!(
        ceremonies.take(&ch, "passkey", "assert", NOW),
        Ok(None),
        "the challenge string is not a handle"
    );
    assert_eq!(ceremonies.len(), 1);
}

#[test]
fn a_ceremony_of_another_phase_is_not_taken_and_not_burned() {
    let (keys, ceremonies) = shared_stores();
    let p = provider_over(&keys, &ceremonies);
    let device = TestAuthenticator::es256();
    let reg = registration_challenge(&p, NOW);
    // Presented as a sign-in: refused, and the registration is still open.
    assert_eq!(
        p.complete(&device.assert(&reg), NOW),
        Err(AuthError::UnknownChallenge(ChallengeId(reg.clone())))
    );
    assert_eq!(ceremonies.len(), 1);
    assert!(p.complete(&device.register(&reg), NOW).is_ok());
}

#[test]
fn an_expired_ceremony_is_swept_and_answers_nothing() {
    use crate::ceremony::CeremonyStore as _;
    let (keys, ceremonies) = shared_stores();
    let p = provider_over(&keys, &ceremonies);
    let _ = assertion_challenge(&p, NOW);
    let later = Timestamp(NOW.0 + CHALLENGE_TTL_SECS);
    assert_eq!(ceremonies.sweep(Timestamp(NOW.0)), Ok(0));
    assert_eq!(
        ceremonies.sweep(later),
        Ok(1),
        "expired at exactly its instant"
    );
    assert!(ceremonies.is_empty());
}

#[test]
fn a_cose_key_round_trips_and_a_bad_point_does_not_load() {
    use crate::webauthn::{CoseKey, ALG_EDDSA, ALG_ES256};
    let (keys, ceremonies) = shared_stores();
    let p = provider_over(&keys, &ceremonies);
    for device in [TestAuthenticator::es256(), TestAuthenticator::ed25519()] {
        enroll(&p, &device, NOW);
        let stored = p
            .credential(&device.credential_id)
            .expect("the credential is stored");
        let key = stored.key().clone();
        let back = CoseKey::from_bytes(stored.algorithm(), &key.to_bytes())
            .expect("a stored key loads again");
        assert_eq!(back, key);
    }
    // A point that is not on the curve.
    let mut off_curve = vec![0x04];
    off_curve.extend_from_slice(&[1u8; 64]);
    assert_eq!(
        CoseKey::from_bytes(ALG_ES256, &off_curve),
        Err(WebAuthnError::MalformedCoseKey)
    );
    // Wrong lengths, a compressed prefix, and an algorithm nobody offers.
    assert!(CoseKey::from_bytes(ALG_ES256, &off_curve[..64]).is_err());
    off_curve[0] = 0x02;
    assert!(CoseKey::from_bytes(ALG_ES256, &off_curve).is_err());
    assert!(CoseKey::from_bytes(ALG_EDDSA, &[0u8; 31]).is_err());
    assert_eq!(
        CoseKey::from_bytes(-257, &[0u8; 32]),
        Err(WebAuthnError::UnsupportedAlgorithm { alg: -257 })
    );
}

#[test]
fn the_backup_flags_and_the_user_handle_are_recorded_at_registration() {
    let (keys, ceremonies) = shared_stores();
    let p = provider_over(&keys, &ceremonies);
    let synced = TestAuthenticator {
        backup_eligible: true,
        backed_up: true,
        ..TestAuthenticator::es256()
    };
    let bound = TestAuthenticator {
        credential_id: b"credential-two".to_vec(),
        ..TestAuthenticator::es256()
    };

    let options = p.begin_registration(NOW, &[]).expect("options");
    assert_eq!(options.user_handle.len(), 43, "32 bytes, base64url");
    p.complete(&synced.register(&options.challenge), NOW)
        .expect("registers");
    let one = p.stored(&synced.credential_id).unwrap().expect("stored");
    assert!(one.backup_eligible && one.backed_up);
    assert_eq!(b64url_encode(&one.user_handle), options.user_handle);
    assert_eq!(one.registered_at, NOW);
    assert_eq!(one.last_used_at, Timestamp(0));

    enroll(&p, &bound, NOW);
    let two = p.stored(&bound.credential_id).unwrap().expect("stored");
    assert!(!two.backup_eligible && !two.backed_up);
    assert_ne!(one.user_handle, two.user_handle, "a handle per ceremony");
}

#[test]
fn the_sign_count_is_written_through_to_the_store() {
    let (keys, ceremonies) = shared_stores();
    let one = provider_over(&keys, &ceremonies);
    let mut device = TestAuthenticator::es256();
    enroll(&one, &device, NOW);

    device.sign_count = 5;
    let ch = assertion_challenge(&one, NOW);
    one.complete(&device.assert(&ch), Timestamp(NOW.0 + 1))
        .expect("count 5 is fresh");
    // A different provider, over the same store, sees the counter and the
    // use: it refuses the same count and every lower one.
    let two = provider_over(&keys, &ceremonies);
    let stored = two.stored(&device.credential_id).unwrap().expect("stored");
    assert_eq!(stored.credential.sign_count(), 5);
    assert_eq!(stored.last_used_at, Timestamp(NOW.0 + 1));
    let ch = assertion_challenge(&two, NOW);
    assert_eq!(
        two.complete(&device.assert(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::SignCountRegression {
            stored: 5,
            presented: 5
        }))
    );
}

/// A store that cannot be written must not let an assertion through: a
/// replayed one would otherwise succeed for as long as the outage lasted.
#[test]
fn an_assertion_is_refused_when_the_counter_cannot_be_written() {
    use crate::passkey::{PasskeyStore, PasskeyStoreError, StoredPasskey};
    struct ReadOnly(crate::passkey::MemoryPasskeyStore);
    impl PasskeyStore for ReadOnly {
        fn get(&self, id: &[u8]) -> Result<Option<StoredPasskey>, PasskeyStoreError> {
            self.0.get(id)
        }
        fn insert(&self, cred: &StoredPasskey) -> Result<(), PasskeyStoreError> {
            self.0.insert(cred)
        }
        fn set_sign_count(
            &self,
            _: &[u8],
            _: u32,
            _: Timestamp,
        ) -> Result<bool, PasskeyStoreError> {
            Err(PasskeyStoreError::Unavailable("read only".into()))
        }
        fn remove(&self, id: &[u8]) -> Result<(), PasskeyStoreError> {
            self.0.remove(id)
        }
    }
    let p = PasskeyProvider::with_stores(
        rp(),
        std::sync::Arc::new(ReadOnly(crate::passkey::MemoryPasskeyStore::new())),
        std::sync::Arc::new(crate::ceremony::MemoryCeremonyStore::new()),
    );
    let device = TestAuthenticator::es256();
    enroll(&p, &device, NOW);
    let ch = assertion_challenge(&p, NOW);
    assert!(matches!(
        p.complete(&device.assert(&ch), NOW),
        Err(AuthError::StoreUnavailable(_))
    ));
}

// ═══════════════════════════ related origins ═════════════════════════════

const OTHER_ORIGIN: &str = "https://app.other-brand.test";

#[test]
fn an_origin_on_another_domain_is_refused_unless_it_is_named_as_related() {
    assert_eq!(
        RelyingParty::new(RP_ID, &[OTHER_ORIGIN]),
        Err(RelyingPartyError::OriginNotUnderRpId {
            origin: OTHER_ORIGIN.into(),
            rp_id: RP_ID.into()
        }),
        "the ordinary list still refuses it"
    );
    let rp = rp().with_related_origins(&[OTHER_ORIGIN]).expect("related");
    assert_eq!(rp.related_origins(), [OTHER_ORIGIN]);
    assert_eq!(rp.allowed_origins(), [ORIGIN, OTHER_ORIGIN]);
    assert_eq!(
        rp.well_known_document().as_deref(),
        Some(r#"{"origins":["https://app.other-brand.test"]}"#)
    );
    assert_eq!(rp.origins(), [ORIGIN], "the own-domain list is unchanged");
}

#[test]
fn a_related_origin_must_be_plain_https() {
    for bad in [
        "http://app.other-brand.test",
        "http://localhost:3000",
        "https://app.other-brand.test/path",
        "https://",
        "https://Upper.test",
        "https://host:port",
        "app.other-brand.test",
    ] {
        assert!(
            matches!(
                rp().with_related_origins(&[bad]),
                Err(RelyingPartyError::InvalidRelatedOrigin { .. })
            ),
            "{bad}"
        );
    }
    assert_eq!(rp().well_known_document(), None, "nothing to publish");
}

#[test]
fn a_ceremony_from_a_related_origin_completes_and_an_unlisted_one_does_not() {
    let p = PasskeyProvider::new(rp().with_related_origins(&[OTHER_ORIGIN]).unwrap());
    let there = TestAuthenticator {
        origin: OTHER_ORIGIN.into(),
        ..TestAuthenticator::es256()
    };
    enroll(&p, &there, NOW);
    let ch = assertion_challenge(&p, NOW);
    assert!(p.complete(&there.assert(&ch), NOW).is_ok());

    let stranger = TestAuthenticator {
        origin: "https://evil.test".into(),
        ..TestAuthenticator::es256()
    };
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&stranger.assert(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::OriginNotAllowed))
    );
    // Without the option the same related page is refused.
    let plain = provider();
    let ch = registration_challenge(&plain, NOW);
    assert_eq!(
        plain.complete(&there.register(&ch), NOW),
        Err(AuthError::WebAuthn(WebAuthnError::OriginNotAllowed))
    );
}

// ═══════════════ recovery R3: retire, quarantine, contest ════════════════

use crate::directory::RecoveryTicket;

fn second_device() -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(41),
        credential_id: b"recovered-key".to_vec(),
        ..TestAuthenticator::es256()
    }
}

/// A person signed up with `lost`, and a recovery that added `new`.
struct Recovered {
    directory: Directory,
    principal: PrincipalId,
    lost: TestAuthenticator,
    new: TestAuthenticator,
    provider: PasskeyProvider,
    outcome: crate::directory::Recovered,
}

const QUARANTINE: u64 = 86_400;
const CONTEST: u64 = 3 * 86_400;

fn recovered() -> Recovered {
    let provider = provider();
    let lost = TestAuthenticator::es256();
    let new = second_device();
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let session = sign_in(&mut directory, &enroll(&provider, &lost, NOW), "Ana", NOW).unwrap();
    // The new passkey is registered through its own ceremony, as the door
    // does, and only then does the directory recover onto it.
    let registered = enroll(&provider, &new, NOW);
    let ticket = RecoveryTicket::new(
        session.principal.clone(),
        "rec_1".into(),
        QUARANTINE,
        CONTEST,
    );
    let outcome = directory
        .recover(&ticket, &registered.authenticator, &[], NOW)
        .expect("a matured ticket recovers");
    Recovered {
        directory,
        principal: session.principal,
        lost,
        new,
        provider,
        outcome,
    }
}

/// A recovery asks for no presence and no vouching factor, and a link asks
/// for both. What each writes differs, and neither accepts the other's
/// argument: a `LinkCeremony` cannot recover and a `RecoveryTicket` cannot
/// link, by type. This pins the behaviour that follows from that.
#[test]
fn recover_shares_no_state_with_link() {
    let mut r = recovered();
    // `recover` needed no presence and no vouching factor: the person's only
    // factor was retired by it, and the new link is marked and quarantined.
    let meta = r
        .directory
        .link_meta(&r.new.authenticator_id())
        .unwrap()
        .expect("linked");
    assert_eq!(meta.recovered_case.as_deref(), Some("rec_1"));
    assert_eq!(meta.presence_not_before, NOW.0 + QUARANTINE);
    assert!(meta.retired.is_none());
    assert_eq!(r.outcome.retired, vec![r.lost.authenticator_id()]);

    // `link` writes an ordinary link: no case, no quarantine.
    let key = TokenKey::from_secret(b"deployment-secret");
    let claims = TokenClaims {
        principal: r.principal.clone(),
        capability: None,
        client: None,
        user_presence_at: Some(NOW),
        delegation: None,
        session: None,
    };
    let third = TestAuthenticator {
        key: TestKey::es256(43),
        credential_id: b"third-key".to_vec(),
        ..TestAuthenticator::es256()
    };
    let enrolled3 = enroll(&r.provider, &third, NOW);
    let ceremony = LinkCeremony::from_claims(&claims, &r.new.authenticator_id()).unwrap();
    r.directory
        .link(&r.principal, &enrolled3.authenticator, &ceremony, NOW)
        .expect("an active factor vouches");
    let plain = r
        .directory
        .link_meta(&third.authenticator_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        plain,
        crate::directory::LinkMeta {
            linked_at: NOW.0,
            ..Default::default()
        }
    );
    let _ = key;

    // And `link_first`, the bootstrap, still refuses a person who holds a
    // link: recovery is not a second door into it.
    let fourth = AuthenticatorId("passkey:fourth".into());
    assert!(matches!(
        r.directory.link_first(&r.principal, &fourth, NOW),
        Err(DirectoryError::StepUpRequired { .. })
    ));
}

/// **A retired authenticator can contest and do nothing else.** It cannot
/// sign in (and is not mistaken for a stranger and onboarded), cannot step
/// up, cannot vouch a link; it can contest, which removes what the recovery
/// added and restores it.
#[test]
fn a_retired_authenticator_can_contest_and_do_nothing_else() {
    let mut r = recovered();
    let lost_id = r.lost.authenticator_id();
    let later = Timestamp(NOW.0 + 60);

    // It does not sign in, under a policy that would onboard a stranger.
    let again = {
        let ch = assertion_challenge(&r.provider, later);
        r.provider.complete(&r.lost.assert(&ch), later).unwrap()
    };
    match sign_in(&mut r.directory, &again, "Ana", later) {
        Err(DirectoryError::AuthenticatorRetired {
            authenticator,
            case,
            contest_until,
        }) => {
            assert_eq!(authenticator, lost_id);
            assert_eq!(case, "rec_1");
            assert_eq!(contest_until, NOW.0 + CONTEST);
        }
        other => panic!("a retired authenticator signed in: {other:?}"),
    }
    assert_eq!(
        r.directory.principal_for(&lost_id).unwrap(),
        Some(r.principal.clone()),
        "and it was not onboarded as anybody else"
    );

    // It does not step up a session of its own principal.
    let key = TokenKey::from_secret(b"deployment-secret");
    let claims = TokenClaims {
        principal: r.principal.clone(),
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: Some(SessionClaim::starting_at(later)),
    };
    assert!(matches!(
        refresh_presence(&r.directory, &claims, &again, Timestamp(later.0 + 900)),
        Err(SessionError::PresenceForAnotherPrincipal { .. })
    ));
    let _ = key;

    // It does not vouch a link.
    let with_presence = TokenClaims {
        user_presence_at: Some(later),
        ..claims
    };
    let ceremony = LinkCeremony::from_claims(&with_presence, &lost_id).unwrap();
    let stranger = AuthenticatorId("passkey:stranger".into());
    assert!(matches!(
        r.directory.link(&r.principal, &stranger, &ceremony, later),
        Err(DirectoryError::AuthenticatorRetired { .. })
    ));

    // The active passkey is not a contest: it is not retired.
    assert!(matches!(
        r.directory
            .contest(&r.new.authenticator_id(), later, OPENED),
        Err(DirectoryError::NotRetired(_))
    ));

    // It contests: the recovery's link goes and its own comes back.
    let contested = r
        .directory
        .contest(&lost_id, later, OPENED)
        .expect("contests");
    assert_eq!(contested.removed, vec![r.new.authenticator_id()]);
    assert_eq!(contested.restored, vec![lost_id.clone()]);
    assert_eq!(
        r.directory
            .principal_for(&r.new.authenticator_id())
            .unwrap(),
        None
    );
    let signed = sign_in(&mut r.directory, &again, "Ana", later).expect("restored in full");
    assert_eq!(signed.principal, r.principal);
    assert!(signed.presence.is_some());
}

/// A contest after the window is not one, and the window's end removes the
/// retired links for good.
#[test]
fn the_contest_window_closes_and_the_retired_are_removed() {
    let mut r = recovered();
    let lost_id = r.lost.authenticator_id();
    let after = Timestamp(NOW.0 + CONTEST);
    assert!(matches!(
        r.directory.contest(&lost_id, after, OPENED),
        Err(DirectoryError::NotRetired(_))
    ));
    assert!(r
        .directory
        .purge_retired(Timestamp(NOW.0 + CONTEST - 1))
        .unwrap()
        .is_empty());
    let gone = r.directory.purge_retired(after).unwrap();
    assert_eq!(
        gone,
        vec![(lost_id.clone(), r.principal.clone(), "rec_1".into())]
    );
    assert_eq!(r.directory.principal_for(&lost_id).unwrap(), None);
    assert_eq!(
        r.directory.authenticators_of(&r.principal).unwrap(),
        vec![r.new.authenticator_id()]
    );
}

/// **The recovered passkey signs in and carries no presence until its
/// quarantine ends**, and a step-up with it gains none.
#[test]
fn a_recovered_passkey_signs_in_without_presence_until_the_quarantine_ends() {
    let mut r = recovered();
    let during = Timestamp(NOW.0 + 3_600);
    let ch = assertion_challenge(&r.provider, during);
    let subject = r.provider.complete(&r.new.assert(&ch), during).unwrap();
    assert!(subject.presence.is_some(), "the ceremony proved presence");
    let signed = sign_in(&mut r.directory, &subject, "Ana", during).unwrap();
    assert!(signed.presence.is_none(), "and the quarantine withholds it");
    assert_eq!(
        signed.presence_available_at,
        Some(Timestamp(NOW.0 + QUARANTINE))
    );
    let spec = signed.token_spec(
        SessionClaim::starting_at(during),
        Timestamp(during.0 + 900),
        None,
    );
    assert_eq!(spec.user_presence_at, None);

    // A step-up on that session with the same passkey gains none.
    let claims = TokenClaims {
        principal: r.principal.clone(),
        capability: None,
        client: None,
        user_presence_at: None,
        delegation: None,
        session: spec.session.clone(),
    };
    let ch = assertion_challenge(&r.provider, during);
    let fresh = r.provider.complete(&r.new.assert(&ch), during).unwrap();
    let stepped =
        refresh_presence(&r.directory, &claims, &fresh, Timestamp(during.0 + 900)).unwrap();
    assert_eq!(stepped.user_presence_at, None);

    // After the quarantine it counts.
    let after = Timestamp(NOW.0 + QUARANTINE);
    let ch = assertion_challenge(&r.provider, after);
    let subject = r.provider.complete(&r.new.assert(&ch), after).unwrap();
    let signed = sign_in(&mut r.directory, &subject, "Ana", after).unwrap();
    assert!(signed.presence.is_some());
    assert_eq!(signed.presence_available_at, None);
}

/// A recovery cannot name a lost authenticator that is not the person's, and
/// an authenticator already linked cannot be the new one.
#[test]
fn recover_refuses_what_is_not_the_persons() {
    let provider = provider();
    let (a, b) = (TestAuthenticator::es256(), second_device());
    let mut directory = Directory::new(Provisioning::AutoOnboard);
    let ana = sign_in(&mut directory, &enroll(&provider, &a, NOW), "Ana", NOW).unwrap();
    let ticket = RecoveryTicket::new(ana.principal.clone(), "rec_x".into(), 60, 600);
    let nobody = AuthenticatorId("passkey:nobody".into());
    assert!(matches!(
        directory.recover(&ticket, &b.authenticator_id(), &[nobody], NOW),
        Err(DirectoryError::RecoveryRefused(_))
    ));
    assert_eq!(
        directory.principal_for(&b.authenticator_id()).unwrap(),
        None,
        "a refused recovery wrote nothing"
    );
    assert!(matches!(
        directory.recover(&ticket, &a.authenticator_id(), &[], NOW),
        Err(DirectoryError::AlreadyLinked { .. })
    ));
}

// ═══════════════════════ a key as a second factor ════════════════════════

/// Register `device` as a second factor for `who`, storing it.
fn enroll_second_factor(
    p: &PasskeyProvider,
    device: &TestAuthenticator,
    who: &PrincipalId,
) -> crate::passkey::PendingRegistration {
    let options = p
        .begin_second_factor_registration(NOW, who, &[])
        .expect("the memory store answers");
    assert_eq!(options.user_verification, "preferred");
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = device.register(&options.challenge)
    else {
        panic!("a registration");
    };
    let (pending, begun_for) = p
        .verify_second_factor_registration(&challenge, &client_data_json, &attestation_object, NOW)
        .expect("a second-factor registration verifies");
    assert_eq!(&begun_for, who);
    p.store_registration(&pending).expect("stored");
    pending
}

/// A second factor's registration proves no presence, its assertion is refused
/// wherever a first factor is asked for, a first-factor passkey is refused
/// where a second factor is asked for, and no assertion of a second-factor key
/// carries presence.
#[test]
fn a_second_factor_key_holds_a_role_and_proves_no_presence() {
    let p = provider();
    let ana = PrincipalId("usr_ana".into());
    // A key with no PIN is accepted, and the record says so.
    let no_pin = TestAuthenticator {
        user_verified: false,
        credential_id: b"credential-no-pin".to_vec(),
        ..TestAuthenticator::es256()
    };
    let pending = enroll_second_factor(&p, &no_pin, &ana);
    assert!(
        !pending.user_verified(),
        "a key with no PIN is accepted and recorded"
    );
    let key = TestAuthenticator::es256();
    let pending = enroll_second_factor(&p, &key, &ana);
    assert_eq!(pending.stored().role, crate::passkey::KeyRole::SecondFactor);
    assert!(
        pending.subject().presence.is_none(),
        "no presence at enrolment"
    );

    // A sign-in assertion and a step-up assertion by it are the one refusal.
    let ch = assertion_challenge(&p, NOW);
    assert_eq!(
        p.complete(&key.assert(&ch), NOW),
        Err(AuthError::BadCredentials)
    );
    let up = p.begin_step_up(NOW, &ana, &[]).expect("options").challenge;
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = key.assert(&up)
    else {
        panic!("an assertion");
    };
    assert_eq!(
        p.complete_step_up(
            &challenge,
            &credential_id,
            &client_data_json,
            &authenticator_data,
            &signature,
            NOW
        )
        .map(|_| ()),
        Err(AuthError::BadCredentials)
    );

    // Its own ceremony accepts it, and what comes back has no presence in it.
    let options = p
        .begin_second_step(NOW, &ana, std::slice::from_ref(&key.credential_id))
        .expect("options");
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = key.assert(&options.challenge)
    else {
        panic!("an assertion");
    };
    let proof = p
        .complete_second_step(
            &challenge,
            &credential_id,
            &client_data_json,
            &authenticator_data,
            &signature,
            NOW,
        )
        .expect("the second step accepts a second-factor key");
    assert_eq!(proof.principal, ana);
    assert_eq!(proof.authenticator, key.authenticator_id());
    assert!(proof.user_verified, "this key verified its user");

    // A first-factor passkey cannot answer it.
    let passkey = TestAuthenticator::ed25519();
    enroll(&p, &passkey, NOW);
    let options = p.begin_second_step(NOW, &ana, &[]).expect("options");
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = passkey.assert(&options.challenge)
    else {
        panic!("an assertion");
    };
    assert_eq!(
        p.complete_second_step(
            &challenge,
            &credential_id,
            &client_data_json,
            &authenticator_data,
            &signature,
            NOW
        )
        .map(|_| ()),
        Err(AuthError::BadCredentials)
    );

    // A first-factor registration challenge cannot finish a second-factor one.
    let first = p.begin_registration(NOW, &[]).expect("options").challenge;
    let other = TestAuthenticator {
        credential_id: b"credential-three".to_vec(),
        ..TestAuthenticator::es256()
    };
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = other.register(&first)
    else {
        panic!("a registration");
    };
    assert!(p
        .verify_second_factor_registration(&challenge, &client_data_json, &attestation_object, NOW)
        .is_err());
}

// ═════════════════ an outside provider's sign-in is recovered elsewhere ═════════════════

/// A directory over a store the test keeps, with one person who holds the
/// given authenticators (the first through `link_first`, the rest through the
/// store, since `link` needs a presence proof an outside provider cannot give).
fn person_with(
    links: &[&str],
) -> (
    Directory,
    std::sync::Arc<crate::MemoryDirectoryStore>,
    PrincipalId,
) {
    use crate::directory::DirectoryStore;
    let store = std::sync::Arc::new(crate::MemoryDirectoryStore::new());
    let mut directory = Directory::with_store(Provisioning::InviteOnly, store.clone());
    let who = directory
        .create_principal(crate::principal::PrincipalKind::Human, "Ana", NOW)
        .unwrap();
    directory
        .link_first(&who, &AuthenticatorId(links[0].into()), NOW)
        .unwrap();
    let principal = store.principal(&who).unwrap().unwrap();
    for l in &links[1..] {
        store
            .insert_link(&AuthenticatorId((*l).into()), &principal, NOW)
            .unwrap();
    }
    (directory, store, who)
}

/// The spelling is the marker: the three outside families, and nothing Ordo
/// mints itself, however a name resembles them.
#[test]
fn an_outside_providers_link_is_recognised_by_its_spelling() {
    for outside in ["oidc:google:sub-1", "saml:acme:alice", "oauth2:gh:42"] {
        assert!(
            AuthenticatorId(outside.into()).is_outside_provider(),
            "{outside}"
        );
    }
    for own in [
        "passkey:oidc:x",
        "password:saml:alice",
        "passkey:abc",
        "oidcx:google:1",
        "oidc",
        "email-otp:a@b",
    ] {
        assert!(!AuthenticatorId(own.into()).is_outside_provider(), "{own}");
    }
}

/// When the case behind [`recovered`] counts as opened: after the person's
/// own links were made at `NOW`.
const OPENED: Timestamp = Timestamp(NOW.0 + 1);

/// **A contest removes every link made since the case opened** (owner
/// ruling 2026-10-25), and leaves one made before it.
#[test]
fn a_contest_removes_links_made_since_the_case_opened() {
    use crate::directory::DirectoryStore;
    let (mut directory, store, who) = person_with(&["passkey:lost", "passkey:kept"]);
    let new = AuthenticatorId("passkey:new".into());
    let ticket = RecoveryTicket::new(who.clone(), "rec_s".into(), 60, 600);
    directory
        .recover(
            &ticket,
            &new,
            &[AuthenticatorId("passkey:lost".into())],
            Timestamp(NOW.0 + 10),
        )
        .expect("recovers");
    let principal = store.principal(&who).unwrap().unwrap();
    let later = AuthenticatorId("passkey:later".into());
    store
        .insert_link(&later, &principal, Timestamp(NOW.0 + 100))
        .unwrap();
    let contested = directory
        .contest(
            &AuthenticatorId("passkey:lost".into()),
            Timestamp(NOW.0 + 200),
            Timestamp(NOW.0 + 5),
        )
        .expect("contests");
    assert_eq!(contested.enrolled_since, vec![later.clone()]);
    assert!(contested.removed.contains(&later));
    assert!(contested.removed.contains(&new));
    assert_eq!(directory.principal_for(&later).unwrap(), None);
    assert!(directory
        .principal_for(&AuthenticatorId("passkey:kept".into()))
        .unwrap()
        .is_some());
    assert!(directory
        .principal_for(&AuthenticatorId("passkey:lost".into()))
        .unwrap()
        .is_some());
}

/// **A recovery retires the person's own passkeys and leaves an outside
/// provider's link exactly as it was.**
#[test]
fn recovery_leaves_an_outside_providers_link_alone() {
    let (mut directory, _store, who) = person_with(&["passkey:lost", "oidc:google:sub-1"]);
    let new = AuthenticatorId("passkey:new".into());
    let ticket = RecoveryTicket::new(who.clone(), "rec_o".into(), 60, 600);
    let done = directory
        .recover(&ticket, &new, &[], NOW)
        .expect("recovers");
    assert_eq!(done.retired, vec![AuthenticatorId("passkey:lost".into())]);
    let links = directory.links_of(&who).unwrap();
    let google = links
        .iter()
        .find(|(a, _)| a.0 == "oidc:google:sub-1")
        .expect("still linked");
    assert!(google.1.retired.is_none(), "not retired");
    assert!(google.1.recovered_case.is_none());
    // Contesting puts back only what was retired, and still leaves it alone.
    let contested = directory
        .contest(
            &AuthenticatorId("passkey:lost".into()),
            Timestamp(NOW.0 + 1),
            OPENED,
        )
        .expect("contests");
    assert_eq!(
        contested.restored,
        vec![AuthenticatorId("passkey:lost".into())]
    );
    assert!(directory
        .principal_for(&AuthenticatorId("oidc:google:sub-1".into()))
        .unwrap()
        .is_some());
}

/// **A person whose only sign-in is an outside provider's is not recovered
/// here**, and naming the provider's link as the lost one is refused too.
#[test]
fn recovery_refuses_a_person_whose_only_sign_in_is_an_outside_providers() {
    let (mut directory, _store, who) = person_with(&["oidc:google:sub-1"]);
    let new = AuthenticatorId("passkey:new".into());
    let ticket = RecoveryTicket::new(who.clone(), "rec_o".into(), 60, 600);
    assert!(directory.only_outside_provider(&who).unwrap());
    assert_eq!(
        directory.recover(&ticket, &new, &[], NOW),
        Err(DirectoryError::RecoveryRefused(
            crate::directory::OUTSIDE_PROVIDER
        ))
    );
    assert_eq!(
        directory.principal_for(&new).unwrap(),
        None,
        "nothing written"
    );
    assert!(directory
        .link_meta(&AuthenticatorId("oidc:google:sub-1".into()))
        .unwrap()
        .unwrap()
        .retired
        .is_none());

    // A person with a passkey as well may not name the provider's link.
    let (mut mixed, _s, who) = person_with(&["passkey:mine", "oidc:google:sub-2"]);
    assert!(!mixed.only_outside_provider(&who).unwrap());
    let ticket = RecoveryTicket::new(who, "rec_p".into(), 60, 600);
    assert_eq!(
        mixed.recover(
            &ticket,
            &new,
            &[AuthenticatorId("oidc:google:sub-2".into())],
            NOW
        ),
        Err(DirectoryError::RecoveryRefused(
            crate::directory::OUTSIDE_PROVIDER
        ))
    );
}

/// `ORDO_AUTH_ORIGINS` may hold `https://*.<domain>` under the RP id, only
/// with `ORDO_WILDCARD_ORIGINS=on`, matching one label (owner, 2026-10-09).
#[test]
fn a_wildcard_origin_needs_the_setting_and_sits_under_the_rp_id() {
    let off = RelyingParty::with_wildcards("example.com", &["https://*.example.com"], false)
        .expect_err("off by default");
    assert!(
        off.to_string().contains("ORDO_WILDCARD_ORIGINS=on"),
        "{off}"
    );
    assert!(matches!(
        RelyingParty::with_wildcards("example.com", &["https://*.other.com"], true),
        Err(RelyingPartyError::OriginNotUnderRpId { .. })
    ));
    let rp =
        RelyingParty::with_wildcards("example.com", &["https://*.example.com"], true).expect("on");
    assert!(rp.allows_origin("https://app.example.com"));
    assert!(!rp.allows_origin("https://example.com"));
    assert!(!rp.allows_origin("https://a.b.example.com"));
    assert!(!rp.allows_origin("https://x.example.com.evil.com"));
    assert!(!rp.allows_origin("http://app.example.com"));
}
