//! The `function` declaration (`plan-backend-functions.md` §4.1).

use crate::decl::FunctionDecl;
use crate::declparse::parse_program;

fn one(src: &str) -> FunctionDecl {
    let p = parse_program(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
    assert_eq!(p.functions.len(), 1, "{src}");
    p.functions[0].clone()
}

fn refused(src: &str) -> String {
    match parse_program(src) {
        Ok(_) => panic!("parsed: {src}"),
        Err(e) => e.message,
    }
}

#[test]
fn a_page_called_function_parses_with_every_clause() {
    let f = one("function suggestTitle in notes {\n\
           call($text: text) -> json by member\n\
           code \"fn/suggest-title.js\"\n\
           egress api.anthropic.com, \"my-model.example.com\"\n\
           secrets llmKey, otherKey\n\
           limit wall 30s, cpu 2s, memory 64 mb\n\
           nopause\n\
         }\n");
    assert_eq!(f.name.name, "suggestTitle");
    assert_eq!(f.domain.as_ref().unwrap().name, "notes");
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name.name, "text");
    assert_eq!(f.returns.name, "json");
    assert_eq!(f.by.name, "member");
    assert_eq!(f.code, "fn/suggest-title.js");
    let hosts: Vec<&str> = f.egress.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(hosts, ["api.anthropic.com", "my-model.example.com"]);
    let secrets: Vec<&str> = f.secrets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(secrets, ["llmKey", "otherKey"]);
    assert_eq!(f.limits.wall_ms, Some(30_000));
    assert_eq!(f.limits.cpu_ms, Some(2_000));
    assert_eq!(f.limits.memory_bytes, Some(64 * 1024 * 1024));
    assert!(f.nopause);
}

#[test]
fn a_minimal_function_defaults_to_member_and_no_limits() {
    let f = one("function hello {\n  call() -> json\n  code \"fn/hello.js\"\n}\n");
    assert_eq!(f.by.name, "member");
    assert!(f.params.is_empty());
    assert!(f.egress.is_empty());
    assert_eq!(f.limits.wall_ms, None);
    assert!(!f.nopause);
}

#[test]
fn a_function_sits_beside_ops_and_entities() {
    let p = parse_program(
        "domain notes\nentity notes { title: text }\n\
         function hello {\n  call() -> json\n  code \"fn/hello.js\"\n}\n\
         op allNotes = notes[title]\n",
    )
    .unwrap();
    assert_eq!(p.functions.len(), 1);
    assert_eq!(p.ops.len(), 1);
    assert_eq!(p.entities.len(), 1);
}

#[test]
fn a_function_without_call_or_code_is_refused_by_name() {
    assert!(refused("function f {\n  code \"fn/f.js\"\n}\n").contains("no `call(...) -> json`"));
    assert!(refused("function f {\n  call() -> json\n}\n").contains("no `code"));
}

#[test]
fn unbuilt_wake_ups_and_clauses_say_so() {
    assert!(refused("function f {\n  trigger every 5m\n}\n").contains("not built yet"));
    assert!(refused("function f {\n  call() -> json\n  uses x\n}\n").contains("not built yet"));
}

#[test]
fn a_glued_size_names_the_spelling_that_works() {
    let toks = crate::lexer::lex("limit memory 64mb");
    let msg = format!("{:?}", toks.unwrap_err());
    assert!(msg.contains("`64 mb`"), "{msg}");
}

#[test]
fn an_unknown_limit_is_refused() {
    assert!(
        refused("function f {\n  call() -> json\n  code \"f.js\"\n  limit disk 3 mb\n}\n")
            .contains("unknown limit `disk`")
    );
}
