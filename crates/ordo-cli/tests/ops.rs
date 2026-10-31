//! `ordo ops` and `ordo gen ts --app`: the derived lookup of what an app may
//! call, and the app-mode generation that writes it beside the client.
//!
//! Run through the built binary against `examples/teamspace`, so what is
//! tested is what an agent types.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const TEAMSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/teamspace");

fn ordo(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ordo"))
        .args(args)
        .output()
        .expect("the ordo binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).expect("utf-8"),
        String::from_utf8(out.stderr).expect("utf-8"),
    )
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ordo-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp directory");
    dir
}

#[test]
fn the_index_is_one_line_per_op_grouped_by_entity() {
    let (code, out, err) = ordo(&["ops", TEAMSPACE]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\n## tasks\n"), "{out}");
    assert!(
        out.contains(
            "createTask  write  (project: id(projects), title: text, assignee?: id(members) = null, \
             due?: date = null, added: datetime filled) -> tasks created id  \
             [editor,manager,owner; rows guarded: tasks]\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "me  read  () -> rows {id, displayName, role}  [viewer,editor,manager,owner]\n"
        ),
        "{out}"
    );
    // The platform ops an app calls, last, with the door's own roles.
    let platform = out
        .split("## platform\n")
        .nth(1)
        .expect("a platform section");
    assert!(platform.contains("std.space.setRole  write  (member: text(256), role: text(32))"));
    assert!(platform.contains("[owner]"));
    assert!(platform.contains("std.space.invite  write  (role?: text(32), expires?: duration"));
    // Byte-stable.
    assert_eq!(out, ordo(&["ops", TEAMSPACE]).1);
}

#[test]
fn an_op_name_prints_its_card_with_calls_and_how_to_clear() {
    let (code, out, err) = ordo(&["ops", TEAMSPACE, "editTask"]);
    assert_eq!(code, 0, "{err}");
    for want in [
        "editTask  write, converges  (group tasks)\n",
        "  assignee  id(members)  optional, default null; fills tasks.assignee\n",
        "to clear tasks.assignee: send null or leave it out",
        "Leaving it out clears it too, so send the current value to keep it",
        "rev       int  required; the row's rev as read; a stale one answers cas_miss\n",
        "  { entity: \"tasks\", updated: count }\n",
        "cas_miss",
        "roles editor, manager, owner",
        "  const { rows } = await editTask(client, { id, rev, title, assignee, due }).commit();\n",
        "  const { commit, refusal } = useCommit('editTask');\n  await commit({ id, title, assignee, due }, rev);\n",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    // A required parameter filling an optional field: the op cannot clear it.
    let (_, out, _) = ordo(&["ops", TEAMSPACE, "std.tasks.update"]);
    assert!(out.contains("this op cannot clear tasks.assignee"), "{out}");
    // A read: the generated client's fetch and the React hook.
    let (_, out, _) = ordo(&["ops", TEAMSPACE, "board"]);
    assert!(out.contains("const page = await board(client, { project }).fetch();"));
    assert!(out.contains("useOpQuery('board', { params: { project } });"));
}

#[test]
fn a_platform_op_card_names_the_wire_and_the_react_table() {
    let (_, out, _) = ordo(&["ops", TEAMSPACE, "std.space.invite"]);
    assert!(out.contains("POST /op/std.space.invite"), "{out}");
    assert!(
        out.contains("withPlatformOps(iface, 'std.space.invite')"),
        "{out}"
    );
    // The membership ops are in the table too (2026-10-30).
    let (_, out, _) = ordo(&["ops", TEAMSPACE, "std.space.remove"]);
    assert!(
        out.contains("withPlatformOps(iface, 'std.space.remove')"),
        "{out}"
    );
}

#[test]
fn any_other_word_searches_and_no_match_says_what_to_run() {
    let (code, out, _) = ordo(&["ops", TEAMSPACE, "ASSIGNEE"]);
    assert_eq!(code, 0);
    assert!(out.contains("createTask  write"), "{out}");
    assert!(out.contains("std.tasks.get  read"), "{out}");
    assert!(!out.contains("me  read"), "{out}");

    let (code, out, err) = ordo(&["ops", TEAMSPACE, "zzzz"]);
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert!(err.contains("contains `zzzz`"), "{err}");
    assert!(err.contains(&format!("ordo ops {TEAMSPACE}")), "{err}");
}

#[test]
fn a_program_that_does_not_compile_prints_the_refusal() {
    let dir = tempdir("ops-broken");
    std::fs::write(dir.join("a.ordo"), "entity things { name: text\n").unwrap();
    let (code, out, err) = ordo(&["ops", dir.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert!(err.contains("a.ordo"), "{err}");
}

/// Two files: teamspace, and one op in a second file.
fn two_files() -> PathBuf {
    let dir = tempdir("ops-files");
    std::fs::copy(
        Path::new(TEAMSPACE).join("teamspace.ordo"),
        dir.join("a.ordo"),
    )
    .unwrap();
    std::fs::write(dir.join("b.ordo"), "op taskTitles = tasks[id, title]\n").unwrap();
    dir
}

#[test]
fn a_directory_of_files_shows_each_ops_file_and_filters_by_file_and_entity() {
    let dir = two_files();
    let d = dir.to_str().unwrap();
    let (code, out, err) = ordo(&["ops", d]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("  @a.ordo:70\n"), "{out}");
    assert!(
        out.contains("taskTitles  read  () -> rows {id, title}"),
        "{out}"
    );
    assert!(out.contains("  @b.ordo:1\n"), "{out}");

    let (_, out, _) = ordo(&["ops", d, "--file", "b.ordo"]);
    let lines: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("  read  ") || l.contains("  write  "))
        .collect();
    assert_eq!(lines.len(), 1, "{out}");
    assert!(lines[0].starts_with("taskTitles"));

    let (_, out, _) = ordo(&["ops", d, "--entity", "team"]);
    let groups: Vec<&str> = out.lines().filter(|l| l.starts_with("## ")).collect();
    assert_eq!(groups, ["## team"], "{out}");

    let (code, _, err) = ordo(&["ops", d, "--entity", "nope"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("The groups are: members, platform, projects, tasks, team"),
        "{err}"
    );
    let (code, _, err) = ordo(&["ops", d, "--file", "c.ordo"]);
    assert_eq!(code, 1);
    assert!(err.contains("The files are: a.ordo, b.ordo"), "{err}");
}

#[test]
fn the_platform_table_matches_the_react_package() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../clients/ordo-react/src/platform.ts"
    ))
    .unwrap();
    let table = text
        .split("export const PLATFORM_OPS")
        .nth(1)
        .expect("platform.ts has PLATFORM_OPS");
    let table = &table[..table.find("\n};").expect("the table ends")];
    let react: BTreeSet<&str> = table
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix('\'')?;
            let name = &rest[..rest.find('\'')?];
            rest[name.len()..].starts_with("':").then_some(name)
        })
        .collect();
    let ours: BTreeSet<&str> = ordo_cli::ops::APP_PLATFORM_OPS.iter().copied().collect();
    assert_eq!(
        ours, react,
        "ordo_cli::ops::APP_PLATFORM_OPS and platform.ts disagree"
    );

    // Every op either table names is one the door serves.
    let served: BTreeSet<String> = ordo_host::platform_ops::manifests()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for name in ordo_cli::ops::APP_PLATFORM_OPS
        .iter()
        .chain(ordo_cli::ops::APP_PLATFORM_EXTRAS)
    {
        assert!(
            served.contains(*name),
            "{name} is not a platform op this build serves"
        );
    }
}

#[test]
fn gen_ts_app_writes_package_imports_and_ops_md_equal_to_ordo_ops() {
    let dir = tempdir("gen-app");
    let src = dir.join("src");
    let (code, out, err) = ordo(&["gen", "ts", TEAMSPACE, "--app", src.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    for f in ["ids.ts", "specs.ts", "ops.ts", "index.ts", "OPS.md"] {
        assert!(
            src.join("generated").join(f).is_file(),
            "{f} missing; reported:\n{out}"
        );
    }
    for f in ["ids.ts", "specs.ts", "ops.ts", "index.ts"] {
        let text = std::fs::read_to_string(src.join("generated").join(f)).unwrap();
        assert!(!text.contains("'../core/"), "{f} still imports ../core/");
    }
    let ops_ts = std::fs::read_to_string(src.join("generated/ops.ts")).unwrap();
    assert!(ops_ts.contains("from '@hominemai/ordo-sdk/core/client.js'"));

    let (_, iface, _) = ordo(&["interface", TEAMSPACE]);
    assert_eq!(
        std::fs::read_to_string(src.join("interface.json")).unwrap(),
        iface
    );

    let md = std::fs::read_to_string(src.join("generated/OPS.md")).unwrap();
    let (_, index, _) = ordo(&["ops", TEAMSPACE]);
    let mut parts = md.splitn(4, '\n');
    let first = parts.next().unwrap();
    let second = parts.next().unwrap();
    assert!(first.contains("GENERATED"), "{first}");
    assert!(
        second.contains(&format!("ordo ops {TEAMSPACE} <query>")),
        "{second}"
    );
    assert_eq!(parts.next(), Some(""), "a blank line after the header");
    assert_eq!(parts.next().unwrap(), index);

    // Idempotent: a second run writes the same bytes.
    let before = std::fs::read(src.join("generated/ops.ts")).unwrap();
    ordo(&["gen", "ts", TEAMSPACE, "--app", src.to_str().unwrap()]);
    assert_eq!(before, std::fs::read(src.join("generated/ops.ts")).unwrap());
}
