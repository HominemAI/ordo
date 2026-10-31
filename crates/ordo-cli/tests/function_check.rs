//! `ordo check` on backend functions: the notes v2 example passes, and a
//! function's secrets must be declared and reachable.

const V2: &str = include_str!("../../../examples/notes-v2/notes.ordo");

#[test]
fn notes_v2_checks_and_lists_its_function() {
    let out = ordo_cli::check(V2).expect("notes v2 checks");
    assert!(out.contains("FUNCTIONS"), "{out}");
    assert!(
        out.contains("suggestTitle($text: text) -> json by member"),
        "{out}"
    );
}

#[test]
fn a_function_using_an_undeclared_secret_is_refused() {
    let src = V2.replace("secret llmKey for api.anthropic.com\n", "");
    let e = ordo_cli::check(&src).expect_err("an undeclared secret");
    assert!(e.contains("declares no such secret"), "{e}");
    assert!(e.contains("llmKey"), "{e}");
}

#[test]
fn a_secret_bound_to_no_egress_host_is_refused_naming_both() {
    let src = V2.replace(
        "secret llmKey for api.anthropic.com",
        "secret llmKey for api.openai.com",
    );
    let e = ordo_cli::check(&src).expect_err("an unreachable secret");
    assert!(
        e.contains("api.openai.com") && e.contains("api.anthropic.com"),
        "{e}"
    );
}
