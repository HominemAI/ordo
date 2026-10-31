//! `secret <name> for <host>[, <host>]` (`plan-backend-functions.md` §5).

use ordo_syntax::{parse_program, INVALID_SECRET_HOST};

#[test]
fn a_secret_is_bound_to_its_hosts() {
    let p = parse_program(
        "secret openaiKey for api.openai.com\n\
         secret mistralKey for api.mistral.ai, eu.mistral.ai, api.mistral.ai\n\
         secret odd for api-v2.example.com, \"1password.example\"\n",
    )
    .expect("parses");
    assert_eq!(p.secrets.len(), 3);
    assert_eq!(p.secrets[0].name.name, "openaiKey");
    assert_eq!(p.secrets[0].hosts, ["api.openai.com"]);
    // Deduplicated, in the order written.
    assert_eq!(p.secrets[1].hosts, ["api.mistral.ai", "eu.mistral.ai"]);
    assert_eq!(
        p.secrets[2].hosts,
        ["api-v2.example.com", "1password.example"]
    );
}

#[test]
fn a_secret_sits_beside_the_other_declarations() {
    let p = parse_program(
        "domain notes\n\
         secret k for api.example.com\n\
         entity notes { title: text }\n",
    )
    .expect("parses");
    assert_eq!(p.secrets.len(), 1);
    assert_eq!(p.entities.len(), 1);
}

#[test]
fn only_exact_host_names_are_accepted() {
    for (src, says) in [
        ("secret k for https://api.example.com", "is a URL"),
        ("secret k for api.example.com/v1", "is a URL"),
        ("secret k for *.example.com", "wildcard"),
        ("secret k for api.example.com:8443", "port"),
        ("secret k for localhost", "not a host name"),
        ("secret k for -bad.example.com", "not a host name"),
    ] {
        let e = parse_program(src).expect_err(src);
        assert_eq!(e.code, INVALID_SECRET_HOST, "{src}: {e}");
        assert!(e.message.contains(says), "{src}: {}", e.message);
    }
}

#[test]
fn a_secret_needs_a_name_for_and_a_host() {
    let e = parse_program("secret k api.example.com").expect_err("no for");
    assert!(e.message.contains("expected `for`"), "{}", e.message);
    let e = parse_program("secret k for\n").expect_err("no host");
    assert!(e.message.contains("expected a host name"), "{}", e.message);
    let e = parse_program("secret k for a.example.com,\n").expect_err("trailing comma");
    assert!(e.message.contains("expected a host name"), "{}", e.message);
}
