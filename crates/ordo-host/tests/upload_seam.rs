//! The upload seam, as assertions over the workspace's own source.
//!
//! `docs/plan-blob-chain.md` §1 pins four properties that a type signature
//! cannot state, and this file states them the way `ordo-egress`'s
//! `holds_nothing.rs` states pin 5: by reading the source, so the failure
//! lands on the line that causes it.
//!
//! # Why a scan and not a type
//!
//! `PinnedUpload` gets the whole of the type-level pin already: private
//! fields, one private constructor, no derives, consumed by move. What the
//! type cannot say is *how many* places construct it, and the trait it
//! crosses on cannot say who else implements it: sealing `PlatformValue`
//! would exclude `ordo-host`, three crates above where the trait is
//! declared, which is the one crate that has to implement it (§7.1, ruled
//! by the owner on 2026-09-09). Those two residues are what this file
//! closes.
//!
//! # The scan's own hazard
//!
//! A scan that cannot fail is worse than no test, in two ways. It can be
//! looking at nothing (a moved directory, a renamed file), and it can be
//! finding itself (a test whose needle appears in its own source matches
//! forever). So every scan below asserts a floor on how much it read, and
//! every needle is assembled from pieces so that this file never contains
//! one literally.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `crates/*/src`, which is what ships. Test
/// directories are read separately where a test asks for them, and where
/// one does it says why.
fn shipped_sources() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("crates/").flatten() {
        walk(&entry.path().join("src"), &mut out);
    }
    out
}

/// Every `.rs` file in the workspace, shipped or not: `crates/*/src` plus
/// `crates/*/tests`. A test that implements a platform value is not shipped
/// and is still a second answer to "what is a platform value", which is the
/// hazard, so the trait's scan reads this set.
fn all_sources() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("crates/").flatten() {
        walk(&entry.path().join("src"), &mut out);
        walk(&entry.path().join("tests"), &mut out);
    }
    out
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let body = std::fs::read_to_string(&p).expect("a source file");
            out.push((p, body));
        }
    }
}

/// The path a failure names, relative to the workspace, so the message is
/// something a reader can open.
fn short(p: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root");
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

// ── the ruling's invariant ───────────────────────────────────────────────

/// **Only the write protocol writes entity rows.**
///
/// This is the invariant candidate (c) was chosen for. The owner's reason
/// is recorded in `corpus-rulings.md`: (c) yields a rule a test can
/// enforce, where (a) and (b) yield a discipline to remember. This is that
/// test, and until C4 it failed: `ordo_exec::blob::attach` held an
/// `"UPDATE {} SET {} = {value} …"` against an entity table, in the blob
/// door's own transaction, appending no command group.
///
/// # What the needle actually finds
///
/// An entity table is never named by a literal in this tree: its name comes
/// from the schema, so the statement is built by interpolation, and the
/// four shapes below are what that looks like in a format string. The scan
/// is over the statement's *shape*, not over a resolved table name, so it
/// also catches statements naming a platform table by variable (`$blobref`,
/// `$blobtomb`). That is why the allowance list has more entries than
/// the two write paths, and each entry says which it is.
///
/// A file not on this list containing one of these shapes fails, and adding
/// it to the list is how a reviewer says they meant it. That is the same
/// ratchet `the_dependency_set_is_the_one_that_was_argued_for` uses in
/// `ordo-egress`, and it works for the same reason: it does not stop the
/// change, it stops the change from arriving as a side effect of something
/// else.
#[test]
fn only_the_write_protocol_updates_entity_rows() {
    // Assembled, so this file does not match its own scan.
    let needles = [
        format!("UPDATE {}", '{'),
        format!("INSERT INTO {}", '{'),
        format!("DELETE FROM {}", '{'),
    ];
    let allowed: &[(&str, &str)] = &[
        (
            "ordo-conformance/src/write.rs",
            "the write protocol itself: the one place an op's create, converge or delete \
             becomes SQL",
        ),
        (
            "ordo-exec/src/undo.rs",
            "the revert applier: it re-applies a before-image, which is the write protocol \
             run backwards, and it images and appends a group for it",
        ),
        (
            "ordo-conformance/src/membership.rs",
            "the membership surface: `members` and `groupMembers` are entity tables and this \
             writes them outside `write.rs`. It is not the blob door's old shape, because it \
             images before and after and stages a command group for every change (`append`), \
             but it IS a second implementation of the protocol and the plan's claim that \
             `attach` was the only one was not exact",
        ),
        (
            "ordo-exec/src/erase.rs",
            "crypto-erasure: one data-modifying CTE per space-scoped table, deleting a whole \
             space. No group, deliberately, because the chain it would append to is being \
             destroyed in the same statement",
        ),
        (
            "ordo-exec/src/blobgc.rs",
            "the collector, over `$blobref` and `$blobtomb` alone. Platform tables named by \
             variable, never an entity table",
        ),
        (
            "ordo-host/src/schema_apply.rs",
            "a money or decimal places change the app developer stated with \
             `--places <entity>.<field>=rescale` (25aa): one statement multiplies or divides \
             every stored amount so each keeps its value in the new unit. Like the re-seal \
             pass, no value changes meaning, so no group is appended; it runs inside the \
             migration's own transaction and is refused when a division would lose digits",
        ),
        (
            "ordo-exec/src/reseal.rs",
            "the re-seal pass (K1): it rewrites a sealed column's envelope to the same value \
             under a new epoch key, and moves a blob column from an old handle to a new \
             object holding the same bytes. No value changes, `rev` does not move, and no \
             group is appended, by the 2026-09-08 ruling: forging a converge with equal \
             images would put a lie in the chain. Every update is a compare-and-set on the \
             old envelope bytes or the old handle, so a concurrent write is skipped, never \
             overwritten",
        ),
    ];

    let mut offenders: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    let sources = shipped_sources();
    assert!(
        sources.len() > 60,
        "the scan found only {} shipped source files, so it is looking at the wrong place",
        sources.len()
    );
    for (path, body) in &sources {
        if !needles.iter().any(|n| body.contains(n.as_str())) {
            continue;
        }
        let name = short(path);
        match allowed.iter().find(|(p, _)| name.ends_with(p)) {
            Some((p, _)) => seen.push(p),
            None => offenders.push(name),
        }
    }
    assert!(
        offenders.is_empty(),
        "these files build a DML statement against an interpolated table name, and are not \
         on the list of places allowed to:\n  {}\n\nAn entity row is written by the write \
         protocol, which locks it, evaluates its guards before and after, images it, and \
         appends a command group. A write outside that path is a change no revert can undo, \
         no trigger sees, and no version number reflects, which is exactly what \
         `ordo_exec::blob::attach` was before `docs/plan-blob-chain.md` C4 deleted it. If a \
         new one is deliberate, add it here with the reason.",
        offenders.join("\n  ")
    );
    for (path, why) in allowed {
        assert!(
            seen.contains(path),
            "`{path}` is on the allowance list ({why}) and no longer matches the scan. Either \
             the file moved, in which case this list is stale and the scan is weaker than it \
             reads, or the statement is gone, in which case delete the entry."
        );
    }
}

// ── the seam's three source-level pins ───────────────────────────────────

/// **One mint site.** `PinnedUpload` is constructed in exactly one place,
/// and that place is the blob door.
///
/// The type cannot say this: a private type is constructible anywhere in
/// its own module, and a module can grow. What the property is worth is
/// what the token stands for, which is bytes that were sealed and stored
/// and bound to the row this upload named. A second construction site is a
/// second definition of that claim, which is how a token comes to exist
/// for bytes nobody wrote.
#[test]
fn the_seam_has_one_mint_site() {
    let literal = format!("PinnedUpload {}", '{');
    let mut sites: Vec<(String, usize)> = Vec::new();
    let sources = shipped_sources();
    assert!(sources.len() > 60, "the scan is looking at the wrong place");
    for (path, body) in &sources {
        // `PinnedUpload {` appears three ways: the declaration, the `impl
        // … for` block, and the return type of `mint`. None of those is a
        // construction; what is left is. Counted per occurrence rather than
        // per line, so a one-line function returning a literal is caught
        // too.
        let mut built = 0;
        for (at, _) in body.match_indices(literal.as_str()) {
            let before = &body[at.saturating_sub(16)..at];
            if before.ends_with("struct ") || before.ends_with("for ") || before.ends_with("-> ") {
                continue;
            }
            built += 1;
        }
        if built > 0 {
            sites.push((short(path), built));
        }
    }
    assert_eq!(
        sites,
        vec![("crates/ordo-host/src/blob.rs".to_string(), 1)],
        "a `PinnedUpload` is evidence that these bytes were sealed by the pipeline and \
         written to the store, and that evidence is only worth anything while `mint` in the \
         blob door is the one way to produce it. Found: {sites:?}"
    );
}

/// **One implementor of `PlatformValue`.**
///
/// The residue §1.2 names and cannot close with a type. Sealing the trait
/// would exclude `ordo-host`, since a sealed trait's private supertrait
/// lives in `ordo-conformance` and `ordo-host` is three crates above it, so
/// the trait is implementable in principle by anything above the protocol.
///
/// The hazard a second implementor is: two callers deciding what a platform
/// value is. The write protocol takes the column literal, the reference
/// statement, the image value and the target *from the value*, so an
/// implementor is not a data holder, it is an author of SQL that runs
/// inside the protocol's transaction with the protocol's guards already
/// passed. One implementor means one place where that authority lives, and
/// it is the place that just wrote the bytes.
///
/// Read over tests as well as shipped source, because a test double is a
/// second answer to the same question and would be the obvious way to get
/// one written by accident.
#[test]
fn the_platform_value_trait_has_one_implementor() {
    let needle = format!("impl {} for", "PlatformValue");
    let mut impls: Vec<String> = Vec::new();
    let sources = all_sources();
    assert!(
        sources.len() > 120,
        "the scan found only {} source files across src and tests",
        sources.len()
    );
    for (path, body) in &sources {
        for _ in 0..body.matches(needle.as_str()).count() {
            impls.push(short(path));
        }
    }
    assert_eq!(
        impls,
        vec!["crates/ordo-host/src/blob.rs".to_string()],
        "two implementors of `PlatformValue` are two callers deciding what a platform value \
         is, and each of them authors SQL the write protocol runs with its guards already \
         passed. If a second one is genuinely wanted, the trait needs an argument for why \
         before it needs an implementation. Found: {impls:?}"
    );
}

/// **The token is not serialisable, and cannot become so here.**
///
/// Two assertions, because the module scan alone would be weak: a manifest
/// that carried `serde` would let a derive appear in one line, and a scan
/// for the word `serde` would then be a scan for whether somebody wrote a
/// comment. So the stronger half is the manifest: `ordo-host` depends on
/// `serde_json` and not on `serde`, so a `Serialize` derive does not
/// compile in this crate at all. The module half asserts the token
/// carries no derive of any kind and is the subject of exactly one `impl`.
///
/// What this is protecting: a serialisable token is a token that can be
/// written down, sent somewhere, and presented again. The whole reason
/// `blob_parameter` is refused (`ordo-sema`, `check_content_params`) is
/// that a caller-supplied handle names an object the caller did not upload,
/// and a token with a wire form re-opens exactly that door with a bigger
/// value in it.
#[test]
fn the_seam_is_not_serialisable() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("this crate's own manifest");
    for line in manifest.lines() {
        let t = line.trim();
        assert!(
            !t.starts_with("serde ") && !t.starts_with("serde="),
            "`ordo-host` must not depend on `serde` itself. It has `serde_json` for \
             rendering values, which cannot derive anything; adding `serde` would make a \
             wire form for `PinnedUpload` one attribute away."
        );
    }

    let door = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/blob.rs"))
        .expect("the door");
    let derive = format!("#[{}", "derive");
    assert_eq!(
        door.matches(derive.as_str()).count(),
        0,
        "nothing in the blob door derives anything. `PinnedUpload` derives nothing by \
         design (no `Clone`, no `Copy`, no `Debug`, no `Default`), and a derive appearing \
         anywhere in this file is the diff where that would be lost."
    );
    for word in ["Serialize", "Deserialize", "to_json", "serde_json"] {
        assert!(
            !door.contains(word),
            "`{word}` in the blob door is a way for a token to acquire a form that can be \
             written down and presented again"
        );
    }
    assert_eq!(
        door.matches("for PinnedUpload").count(),
        1,
        "the token implements exactly one trait, `PlatformValue`, and that trait is how the \
         write protocol reads it. A second impl is a second thing it can be used as."
    );
}

/// **No op body can reach the platform converge.**
///
/// `a_blob_parameter_is_still_refused` (`ordo-sema`) says an op author
/// cannot ask for a blob-typed parameter. This says the other half: even if
/// one could, there is no path from a parsed op to the entry point that
/// writes a blob column.
///
/// Asserted three ways, because "not called from `run_write_body`" alone
/// would pass if some other function in the protocol called it:
///
/// 1. `run_platform_converge` appears in `write.rs` exactly once, on the
///    line that defines it. So nothing inside the write protocol calls it,
///    and no statement, no op body and no parsed call can reach it.
/// 2. It is called from exactly one file in the workspace,
///    `ordo-exec/src/lib.rs`, which is `Executor::attach_write`.
/// 3. `attach_write` is called from exactly one file, the blob door, which
///    is reached by the `PUT /blob/...` route and by nothing that parses
///    `.ordo` source.
#[test]
fn no_op_body_can_reach_the_platform_converge() {
    let sources = shipped_sources();
    let protocol = sources
        .iter()
        .find(|(p, _)| short(p).ends_with("ordo-conformance/src/write.rs"))
        .map(|(_, b)| b.clone())
        .expect("the write protocol");
    // The trailing parenthesis matters: a doc comment naming the function
    // is a reference and not a call, and a scan that counted those would
    // fail on the first cross-reference somebody wrote.
    let entry = "run_platform_converge(";
    assert_eq!(
        protocol.matches(entry).count(),
        1,
        "`{entry}` occurs more than once in the write protocol. Exactly one of those is its \
         own definition; any other is a call from inside the protocol, and a call from \
         inside the protocol is a path a parsed op can reach."
    );
    assert!(
        protocol.contains(&format!("pub fn {entry}")),
        "the one occurrence is supposed to be the definition"
    );

    let callers: Vec<String> = sources
        .iter()
        .filter(|(p, b)| b.contains(entry) && !short(p).ends_with("ordo-conformance/src/write.rs"))
        .map(|(p, _)| short(p))
        .collect();
    assert_eq!(
        callers,
        vec!["crates/ordo-exec/src/lib.rs".to_string()],
        "the platform converge is reached from `Executor::attach_write` and nowhere else. \
         Found: {callers:?}"
    );

    let door: Vec<String> = sources
        .iter()
        .filter(|(p, b)| b.contains("attach_write(") && !short(p).ends_with("ordo-exec/src/lib.rs"))
        .map(|(p, _)| short(p))
        .collect();
    assert_eq!(
        door,
        vec!["crates/ordo-host/src/blob.rs".to_string()],
        "`attach_write` is the blob door's, and a second caller is a second way to write a \
         blob column without an upload behind it. Found: {door:?}"
    );
}

// ── the door's wiring, which C4 left as a note and nothing asserted ──────

/// **One blob door, wired by the runtime, holding no chain of its own.**
///
/// `plan-blob-chain.md`'s C4 left this as a hazard with no test: *"a real
/// deployment must hand the door the same `Arc` the executor holds: a
/// second `Arc` is a second chain for one space, which is the one thing a
/// chain cannot be."* The note also says the door carries a
/// `log: Arc<Mutex<WriteLog>>`, and by the time the increment landed it
/// did not. That is the more interesting half of what this pins.
///
/// **The hazard was closed by removing the field rather than by wiring it
/// carefully**, which is the stronger fix and the reason this test reads
/// the struct rather than the call. `PlatformBlobs` holds an
/// `Arc<Spaces>` — the resolver the op path already holds — and a
/// `BlobPipeline`, and nothing else. A second chain is not something the
/// runtime must remember to avoid; it is not expressible, because there is
/// no field to put one in. The same sentence covers the pool: the door
/// finds a space's database through the resolver, so it cannot answer
/// "where are this space's rows" differently from the executor, which for
/// a space that had been moved would mean reading references out of the
/// database the space left.
///
/// So two assertions. The struct admits no second log and no second pool,
/// and exactly one shipped file builds the door.
#[test]
fn the_blob_door_is_wired_once_and_holds_no_chain_of_its_own() {
    let sources = shipped_sources();
    assert!(sources.len() > 60, "the scan is looking at the wrong place");

    // The declaration, read field by field. Assembled rather than written
    // literally, for the reason this file's own doc gives.
    let decl_needle = format!("pub struct PlatformBlobs {}", '{');
    let (_, door) = sources
        .iter()
        .find(|(p, body)| short(p) == "crates/ordo-host/src/blob.rs" && body.contains(&decl_needle))
        .expect("the blob door's declaration");
    let at = door.find(&decl_needle).expect("the declaration");
    let body = &door[at..at + door[at..].find("\n}").expect("the closing brace")];
    // Field lines only. A doc comment inside the declaration explains why
    // the door holds no pool, and a scan that read prose would fail on the
    // sentence that says the property holds.
    let fields: String = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && t.contains(':')
        })
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["WriteLog", "Pool", "CommandLog"] {
        assert!(
            !fields.contains(forbidden),
            "`PlatformBlobs` grew a `{forbidden}`. An upload's group joins the same per-space \
             chain an op's group joins, and the door reaches a space's rows through the \
             resolver the op path holds. A field of its own here is a second answer to a \
             question that must have one, and nothing above would fail loudly when the two \
             disagreed. The fields now read:\n{fields}"
        );
    }

    // And one construction site, which is the runtime. A second would be a
    // second door, and the pin above only holds per door.
    let build_needle = format!("PlatformBlobs {}", '{');
    let mut sites: Vec<(String, usize)> = Vec::new();
    for (path, text) in &sources {
        let mut built = 0;
        for (at, _) in text.match_indices(build_needle.as_str()) {
            let before = &text[at.saturating_sub(16)..at];
            if before.ends_with("struct ")
                || before.ends_with("for ")
                || before.ends_with("impl ")
                || before.ends_with("-> ")
            {
                continue;
            }
            built += 1;
        }
        if built > 0 {
            sites.push((short(path), built));
        }
    }
    assert_eq!(
        sites,
        vec![("crates/ordo-host/src/runtime.rs".to_string(), 1)],
        "the blob door is built somewhere other than the runtime that owns the deployment's \
         one `Spaces`. Found: {sites:?}"
    );
}
