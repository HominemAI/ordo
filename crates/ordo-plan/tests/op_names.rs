//! An op may not share its name with an entity (`op_named_like_entity`).
//!
//! `name(conds)[fields]` is the source form of both, so a name that is both
//! could mean either. Before this refusal the door's call of such an op
//! answered the entity's own read, unfiltered (reported fix,
//! `examples/note-viewer`, 2026-11-19). A regression looks like any of these
//! programs compiling.

use ordo_sema::Registry;

fn compile(src: &str) -> Result<(), String> {
    let program = ordo_syntax::parse_program(src).map_err(|e| format!("parse: {}", e.message))?;
    let schema = ordo_schema::build(&program).map_err(|e| format!("{}: {}", e.code, e.message))?;
    let mut registry = Registry::from_program(&program);
    registry.extend_from(&ordo_schema::acl_admin_ops(&schema));
    registry.extend_from(&ordo_schema::std_ops(&schema));
    ordo_plan::cost::check_registration(&schema, &registry)
        .map_err(|e| format!("{}: {}", e.code, e.message))
}

fn refused(src: &str, says: &[&str]) {
    match compile(src) {
        Ok(()) => panic!("expected `op_named_like_entity`, and it compiled:\n{src}"),
        Err(e) => {
            assert!(e.starts_with("op_named_like_entity"), "{e}");
            for s in says {
                assert!(e.contains(s), "the message should say {s:?}: {e}");
            }
        }
    }
}

const NOTICES: &str = "\
entity notices {
  public
  body:  text
  shown: bool
}
";

const POSTS: &str = "\
external posts (key: post) from crm.posts shared {
  public
  id <- \"post_id\"
  title: text <- \"title\"
  open: bool <- \"open\"
}
";

#[test]
fn a_public_op_named_like_a_native_entity_is_refused() {
    refused(
        &format!("{NOTICES}\nop notices public =\n  notices(shown == true)[body]\n  take 50\n"),
        &[
            "op `notices`",
            "the entity `notices`",
            "Rename the op or the entity",
        ],
    );
}

#[test]
fn a_public_op_named_like_an_external_is_refused() {
    refused(
        &format!("{POSTS}\nop posts public =\n  posts(open == true)[title]\n  take 50\n"),
        &[
            "op `posts`",
            "the external entity `posts`",
            "Rename the op or the external entity",
        ],
    );
}

#[test]
fn a_signed_in_op_named_like_an_entity_is_refused() {
    refused(
        "entity tasks {\n  title: text\n  done: bool\n}\n\nop tasks =\n  tasks(done == false)[title]\n  take 50\n",
        &["op `tasks`", "the entity `tasks`"],
    );
}

#[test]
fn an_op_named_like_the_members_entity_is_refused() {
    refused(
        "entity tasks {\n  title: text\n}\n\nop members =\n  tasks[title]\n  take 50\n",
        &["op `members`", "the entity `members`"],
    );
}

#[test]
fn an_op_with_its_own_name_compiles() {
    compile(&format!(
        "{NOTICES}\n{POSTS}\nop shownNotices public =\n  notices(shown == true)[body]\n  take 50\n\n\
         op openPosts public =\n  posts(open == true)[title]\n  take 50\n"
    ))
    .expect("compiles");
}
