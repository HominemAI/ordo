//! Pin 5, as an assertion over this crate's own manifest.
//!
//! §7b: *"Egress runs under an identity that holds nothing — a dedicated
//! egress worker or outbound proxy with no instance-credential access and no
//! `KeyProvider` reachability, so a bypass of everything above still lands
//! somewhere with nothing to steal. This is the control that survives a bug
//! in the other four."*
//!
//! A single-process platform cannot give that sentence its full meaning; a
//! separate pod with a credential-less service account is a **deployment**
//! obligation, and `deploying.md` carries it. What *can* be enforced here is
//! the static half, and it is not nothing: the code that opens a socket to a
//! customer-named host cannot *name* `KeyProvider`, cannot name `Pool`, and
//! therefore cannot reach key material or database credentials, because it
//! does not depend on the crates that define them.
//!
//! This test reads `Cargo.toml`. That is deliberate — the failure mode it
//! guards is somebody adding `ordo-crypto = { path = "../ordo-crypto" }` to
//! make one thing convenient, and a test that reads the manifest fails on
//! exactly that line. A test that instead checked for a `use` would pass
//! until the second commit.

use std::path::Path;

fn manifest() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(p).expect("this crate's own manifest")
}

/// Dependency lines only — comments and the `[features]`/`[lints]` sections
/// are not dependencies, and the manifest's comments legitimately *name* the
/// crates this test forbids, in the course of explaining why.
fn dependency_names(kind: &str) -> Vec<String> {
    let src = manifest();
    let mut out = Vec::new();
    let mut inside = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == kind;
            continue;
        }
        if !inside || t.starts_with('#') || t.is_empty() {
            continue;
        }
        if let Some((name, _)) = t.split_once('=') {
            out.push(name.trim().trim_matches('"').to_string());
        }
    }
    out
}

#[test]
fn the_egress_crate_depends_on_no_ordo_crate() {
    let deps = dependency_names("[dependencies]");
    assert!(!deps.is_empty(), "the parser found no dependencies at all");
    let ordo: Vec<&String> = deps.iter().filter(|d| d.starts_with("ordo-")).collect();
    assert!(
        ordo.is_empty(),
        "pin 5 says egress holds nothing. These dependencies give it \
         something to reach: {ordo:?}. `ordo-crypto` defines `KeyProvider`; \
         `ordo-exec` owns the pool and therefore the database credentials. \
         If a delivery genuinely needs a value from one of them, pass the \
         value in — do not give the socket-opening crate the whole crate."
    );
}

#[test]
fn the_egress_crate_cannot_speak_to_a_database() {
    let deps = dependency_names("[dependencies]");
    for forbidden in [
        "tokio-postgres",
        "postgres",
        "sqlx",
        "diesel",
        "deadpool-postgres",
    ] {
        assert!(
            !deps.iter().any(|d| d == forbidden),
            "`{forbidden}` would give the egress path a way to reach the \
             deployment's own data"
        );
    }
}

#[test]
fn the_dependency_set_is_the_one_that_was_argued_for() {
    // A ratchet, not a lock: adding a dependency here is fine, and updating
    // this list is how you say you meant to. What it stops is a dependency
    // arriving as a side effect of something else — which is how a crate that
    // exists to hold nothing ends up holding a config loader that reads the
    // environment.
    let mut deps = dependency_names("[dependencies]");
    deps.sort();
    assert_eq!(
        deps,
        vec![
            "hmac",
            "rustls",
            "rustls-pki-types",
            "sha2",
            "tokio",
            "tokio-rustls",
            "webpki-roots",
        ],
        "the egress crate's dependency set changed; if that was deliberate, \
         say so here and in the manifest's comment"
    );
}

#[test]
fn no_crate_ships_the_test_escapes() {
    // `test-pin` gates two constructors that skip a pin: `pin_for_test`,
    // which mints a `PinnedDestination` without resolving, and
    // `permitting_loopback_for_tests`. Both are correct for an integration
    // suite and catastrophic in a deployment, so the guard is that **no
    // crate in this workspace enables the feature as a non-dev dependency**.
    // A build that could reach either is then a diff a reviewer sees on the
    // line that causes it.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut checked = 0;
    for entry in std::fs::read_dir(crates).expect("crates/").flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(src) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        checked += 1;
        let mut section = "";
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                section = if t.contains("dev-dependencies") {
                    "dev"
                } else if t.contains("dependencies") {
                    "deps"
                } else {
                    ""
                };
                continue;
            }
            if section == "deps" && t.starts_with("ordo-egress") {
                assert!(
                    !t.contains("test-pin"),
                    "{} enables `test-pin` as a real dependency; that ships a constructor \
                     which skips resolve-then-pin and one which permits loopback",
                    manifest.display()
                );
            }
        }
    }
    assert!(checked > 5, "the scan found only {checked} manifests");
}

#[test]
fn the_loopback_escape_never_opens_link_local() {
    // 169.254.169.254 is the address §7b was written about. A test escape
    // that reached it would be a test escape that could hide the bug.
    #[cfg(feature = "test-pin")]
    {
        let deny = ordo_egress::DenyList::categories_only().permitting_loopback_for_tests();
        assert_eq!(deny.check("127.0.0.1".parse().unwrap()), Ok(()));
        assert_eq!(
            deny.check("169.254.169.254".parse().unwrap()),
            Err(ordo_egress::DeniedCategory::LinkLocal)
        );
        assert_eq!(
            deny.check("10.0.0.1".parse().unwrap()),
            Err(ordo_egress::DeniedCategory::Private)
        );
    }
}

#[test]
fn there_is_no_way_to_turn_certificate_validation_off() {
    // rustls' escape hatch is `dangerous_configuration`, and it is a feature
    // flag. Not enabling it means `ClientConfig::dangerous()` does not exist
    // in this build, so pin 1 is not a discipline anybody has to keep.
    let src = manifest();
    assert!(
        !src.contains("dangerous"),
        "a `dangerous` rustls feature would make certificate validation \
         optional, and §7b says there is no opt-out at any deployment size"
    );
    for file in ["src/client.rs", "src/lib.rs", "src/dest.rs"] {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            !body.contains("ServerCertVerifier") && !body.contains("dangerous()"),
            "{file} reaches for rustls' verification escape hatch"
        );
    }
}

#[test]
fn tls_below_one_two_is_not_expressible() {
    // The floor is a property of the library, not a setting: rustls has no
    // code for SSLv3, TLS 1.0 or TLS 1.1. What this asserts is the narrower
    // thing a manifest can say — that nothing here reaches for a version knob
    // — plus that `tls12` is on, since without it the floor would silently be
    // 1.3 and a legitimate receiver would fail for a reason nobody predicted.
    let src = manifest();
    assert!(src.contains("\"tls12\""), "the TLS 1.2 floor is deliberate");
    let client =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/client.rs"))
            .unwrap();
    assert!(!client.contains("with_protocol_versions"));
}
