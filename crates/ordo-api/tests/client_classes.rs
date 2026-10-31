//! The TypeScript client keeps the lists of codes the door can send, and a
//! code missing from them decodes as `internal`. That happened to twelve
//! classes, `space_paused` and `server_busy` among them, before this test:
//! a paused tenant read as a server fault. Each list must name every code
//! its server inventory has, and nothing else.
//!
//! Two inventories, two lists (`docs/audit-service-interfaces.md`, fix
//! order step 7): the op door's [`ErrorCode`] is the SDK's
//! `REFUSAL_CLASSES`, and the other surfaces' [`EdgeCode`] is its
//! `EDGE_CODES`. ordo-react re-exports both, so the SDK's are the only ones.

use ordo_api::{EdgeCode, ErrorCode};
use std::collections::BTreeSet;
use std::path::Path;

const SDK: &str = "clients/ordo-sdk/src/core/refusal.ts";

/// The quoted names in a TypeScript `export const <name> = [ ... ] as const`.
fn ts_list(rel: &str, name: &str) -> BTreeSet<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let decl = format!("export const {name} = [");
    let start = src
        .find(&decl)
        .unwrap_or_else(|| panic!("{rel} has no `{decl}`"));
    let body = &src[start + decl.len()..];
    let end = body
        .find("] as const")
        .expect("the list ends with `] as const`");
    body[..end]
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            l.strip_prefix('\'')
                .and_then(|r| r.strip_suffix("',"))
                .map(str::to_string)
        })
        .collect()
}

fn assert_same(list: &str, door: BTreeSet<String>, client: BTreeSet<String>) {
    let missing: Vec<_> = door.difference(&client).collect();
    let unknown: Vec<_> = client.difference(&door).collect();
    assert!(
        missing.is_empty() && unknown.is_empty(),
        "{SDK} {list} is out of step with the door: missing {missing:?}, unknown {unknown:?}. \
         Add or remove the names in it."
    );
}

#[test]
fn the_sdk_names_every_refusal_class_the_op_door_sends() {
    let door = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    assert_same("REFUSAL_CLASSES", door, ts_list(SDK, "REFUSAL_CLASSES"));
}

#[test]
fn the_sdk_names_every_edge_code_the_other_surfaces_send() {
    let door = EdgeCode::ALL
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    assert_same("EDGE_CODES", door, ts_list(SDK, "EDGE_CODES"));
}

#[test]
fn no_code_is_in_both_inventories() {
    // A word in both would decode to one class from two meanings. The one
    // exception has one meaning on both surfaces, by design: a passkey
    // touch newer than the one held (`ErrorCode::PresenceRequired`).
    const SAME_MEANING: [&str; 1] = ["presence_required"];
    let ops: BTreeSet<_> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
    let both: Vec<_> = EdgeCode::ALL
        .iter()
        .map(|c| c.as_str())
        .filter(|w| ops.contains(w) && !SAME_MEANING.contains(w))
        .collect();
    assert!(
        both.is_empty(),
        "codes in both ErrorCode and EdgeCode: {both:?}"
    );
}
