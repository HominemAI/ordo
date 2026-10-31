//! `std.space.import` — the door that makes the importer reachable.
//!
//! `ordo_exec::import` has been built since 11-10a, and until this op nothing
//! in any shipped binary could call it. `crates/ordo-exec/tests/import.rs`
//! proves what the engine does; this file proves the three things only the
//! op can be wrong about.
//!
//! **1. Whose keys the rows are sealed under.** `Fixture::compile_source`
//! mints a `TestProvider` over the conformance space, because a compiler has
//! no business deciding custody (R2a, ruled 2026-09-07), and the Stage-5
//! write protocol seals through `Fixture::keys`. So an import that threaded
//! custody into the `Executor` alone — which is the obvious way to write it,
//! and covers every read — would write every sealed value in the bundle
//! under **a constant in this repository**, in a space whose real key was
//! never consulted. Nothing about the resulting rows would look wrong:
//! they exist, they are in the chain, and the space's own reads open them,
//! because the same wrong key opens them back. `a_sealed_value_is_sealed_
//! under_the_destinations_own_custody` is the test that fails when the
//! assignment goes missing, and it fails by asking the *key provider* to open
//! the envelope rather than by asking the importer.
//!
//! **2. Which bundle a caller can name.** The staging root is one directory
//! for a deployment with many tenants, so the space component of the path is
//! an authorization boundary: without it, an operator staging one tenant's
//! bundle would have made it loadable by every other tenant's owner, each of
//! whom passes every gate this op has.
//!
//! **3. That an absent bundle and an unspellable one are one answer**, so a
//! caller cannot map the staging directory a guess at a time.
//!
//! **4. Who the people in the destination are.** Every entity here declares
//! `owner -> members`, which almost every real application does and which
//! could not be imported at all before 2026-09-26: `members` is a platform
//! entity, the importer will not create one, and nothing mapped their ids, so
//! the first row refused. The owner ruled a **supplied map** with a refusal
//! for any name it does not cover (`platform-ref-import`). The tests below
//! are that ruling in both directions: with a map the rows land and point at
//! the person the operator named, and with no map the import still refuses
//! and writes nothing.
//!
//! # What is deliberately proved elsewhere
//!
//! The **gate**. `std.space.import` is owner-only and step-up gated by
//! sitting in `platform_ops::owner_only`, which is one list shared with the
//! two exit ops, and `tests/exit_ops.rs` drives that list through the real
//! router for `std.space.export` and `std.space.delete`. What this file adds
//! is the assertion that the name is *in* the list and in the registration
//! gate, because a handler reached through the wrong gate is the one failure
//! the tests below could not see.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::PathBuf;
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::cadence::LogPolicy;
use ordo_exec::export::{export_space, DirectoryBundle, ExportRequest, SealedMode};
use ordo_exec::registry::{self, Artifact};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::exit_ops::{ExitOps, IMPORT};
use serde_json::{json, Value};

/// The source space: the deployment's own, which the fixture's custody
/// covers.
const HERE: &str = ordo_host::DEFAULT_SPACE;
/// The destination, provisioned through the one entry point that mints
/// custody, so its keys are **different keys** from the source's.
const TWIN: &str = "spc_import_dest";
/// `TWIN`'s founding owner, and the principal the import runs as.
const IMPORTER: &str = "mem_dest";
/// Who writes the source row.
const AUTHOR: &str = "mem_author";
/// A second member of the destination, so a changed map has somewhere else to
/// point that is still legitimate.
const SECOND: &str = "mem_second";

const NOW: u64 = 1_700_000_000;

/// One entity, one **sealed** field, no guard, and no reference to
/// `members`.
///
/// Sealed because that is the field whose custody is observable: a plaintext
/// column would be byte-identical however the seal was keyed, so a program
/// without one cannot fail the first test in this file.
///
/// No `-> members` ref because import skips platform entities and nothing
/// maps their ids, so an entity with one cannot be imported at all today.
/// That gap is on the board as `platform-ref-import` and is not worked
/// around here.
const PROGRAM: &str = "\
entity notes (key: nte) {
  owner -> members
  title: text
  memo:  text = \"\"  sealed
}
";

const MEMO: &str = "the sealed memo, keyed twice";

struct Harness {
    pool: Pool,
    pg: Pg,
    /// **The deployment's** custody, and a different provider instance from
    /// the fixture's on purpose. `Fixture::compile_source` mints its own
    /// `TestProvider` with its own `MemoryKeyStore`, so a space created in
    /// one provider does not exist in the other. That is what makes the
    /// custody assignment observable: this provider knows `TWIN`, the
    /// fixture's does not, and an import that sealed through the fixture's
    /// would be sealing under a space its own key store has never heard of.
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    ops: ExitOps,
    staging: PathBuf,
    exports: PathBuf,
    artifact: Artifact,
    fixture: Arc<Fixture>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.staging).ok();
        std::fs::remove_dir_all(&self.exports).ok();
    }
}

async fn harness(db: &str) -> Harness {
    let fixture = Arc::new(Fixture::from_source(PROGRAM, "").expect("the program compiles"));
    let pg = Pg::scoped(db);
    fixture.reset(&pg).expect("the schema applies");

    let pool = Pool::connect(PoolConfig {
        db: ordo_conformance::test_db_name(db),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");

    // The op reads the **deployed artifact**, and imports against the
    // program those stored bytes describe, so the registry carries the same
    // source the harness compiled.
    registry::ensure_tables(&pool).await.expect("registry");
    let artifact = Artifact {
        version: "v_import".into(),
        artifact: "{\"stand-in\": true}\n".into(),
        source: PROGRAM.to_string(),
        ddl: fixture.ddl.clone(),
        applied_at: ordo_commandlog::Timestamp(NOW),
        actor: "test".into(),
    };
    registry::store(&pool, &artifact).await.expect("store");
    registry::activate(&pool, "v_import", "test", ordo_commandlog::Timestamp(NOW))
        .await
        .expect("activate");

    // The deployment's own custody: a provider that knows `TWIN` and knows
    // nothing about the conformance space the compiler's provider was minted
    // over.
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(TWIN));

    // **The source space, with a person in it.** `owner -> members` is a
    // foreign key on `($space, id)`, so a source row cannot exist until the
    // source space has a member. Provisioned under the FIXTURE's custody,
    // which is the provider the export reads with.
    ordo_host::provision::provision_space(
        &pool,
        fixture.keys.as_ref(),
        HERE,
        &ordo_host::provision::FoundingOwner {
            member_id: AUTHOR,
            principal: AUTHOR,
            display_name: "The source's author",
        },
    )
    .await
    .expect("the source provisions");

    // The destination, provisioned before anything is exported, through the
    // one entry point that mints custody for a space.
    ordo_host::provision::provision_space(
        &pool,
        keys.as_ref(),
        TWIN,
        &ordo_host::provision::FoundingOwner {
            member_id: IMPORTER,
            principal: IMPORTER,
            display_name: "The destination's owner",
        },
    )
    .await
    .expect("the destination provisions");

    // A second legitimate member of the destination, so the changed-map test
    // changes the map to somebody who exists: a refusal for a non-member
    // would prove the wrong thing.
    // Inserted directly, because a platform entity has no derived writer and
    // must not grow one: `provision_space` writes its founding owner the same
    // way, and there is no second legitimate path to a membership.
    pool.rows(&format!(
        "INSERT INTO \"members\" \
         (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \"kind\", \
          \"memberCount\", \"role\", \"status\", \"joined\") \
         VALUES ('{TWIN}', '{SECOND}', 1, '{SECOND}', 'The other one', 'principal', 1, \
                 'editor', 'active', CURRENT_DATE) ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("a second destination member");

    let stem = format!("{db}-{}", std::process::id());
    let staging = std::env::temp_dir().join(format!("ordo-import-staging-{stem}"));
    let exports = std::env::temp_dir().join(format!("ordo-import-exports-{stem}"));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::create_dir_all(&exports).unwrap();

    let ops = ExitOps {
        pool: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        keys: keys.clone(),
        policy: LogPolicy::DEFAULT,
        bundles: exports.clone(),
        imports: Some(staging.clone()),
        // The program declares no blob field, so a bundle from it carries no
        // objects and `blob_door_missing` is unreachable here. The blob leg
        // is `tests/import_blobs.rs`.
        door: None,
        stream: Some(Arc::new(ordo_secevent::Recorder::default())),
        blobs: None,
        stores: None,
        now: Arc::new(|| NOW),
    };

    Harness {
        pool,
        pg,
        keys,
        ops,
        staging,
        exports,
        artifact,
        fixture,
    }
}

impl Harness {
    /// One row in the source space, written through the ordinary write
    /// protocol so its sealed value is a real envelope.
    async fn seed_source(&self) {
        let f = self.fixture.in_space(HERE);
        let mut log = ordo_conformance::WriteLog::default();
        f.run_write_as(
            &self.pg,
            &mut log,
            &format!("std.notes.create($owner: \"{AUTHOR}\", $title: \"kept\", $memo: \"{MEMO}\")"),
            Some(AUTHOR),
        )
        .expect("the source row is written");
    }

    /// The destination's row, read back through a program bound to `TWIN`
    /// and to the **deployment's** custody, which is the pair an ordinary
    /// request runs under.
    fn read_destination(&self) -> Value {
        let mut f = self.fixture.in_space(TWIN);
        f.keys = self.keys.clone();
        f.run_case_params(&self.pg, "notes[id, title, memo]", &[])
            .expect("the destination's row reads back")
    }

    /// Export the source space into a bundle, and stage it for `space` under
    /// `name`.
    async fn stage(&self, space: &str, name: &str) {
        let dir = self.exports.join("out");
        let mut sink = DirectoryBundle::new(&dir);
        export_space(
            &self.pool,
            &ExportRequest {
                space: &SpaceId(HERE.to_string()),
                schema: &self.fixture.schema,
                artifact: &self.artifact,
                // The SOURCE space's custody, which is the compiler's
                // provider: `HERE` was created there and nowhere else.
                keys: self.fixture.keys.as_ref(),
                blobs: None,
                sealed: SealedMode::Open,
                correlation: "exp_test",
            },
            &mut sink,
        )
        .await
        .expect("the source exports");
        let into = self.staging.join(space).join(name);
        std::fs::create_dir_all(into.parent().unwrap()).unwrap();
        copy_tree(&dir, &into);
    }

    /// Stage a bundle **flat** under the root, which is where an
    /// implementation with no space component in its path would look for it.
    ///
    /// This exists so the boundary test can fail. Staging under some other
    /// space's directory does not test the boundary at all: neither a correct
    /// implementation nor a broken one finds a bundle there, so the refusal
    /// arrives either way and the assertion proves nothing. Putting it where
    /// the broken one *would* find it is the only version that bites.
    async fn stage_flat(&self, name: &str) {
        let dir = self.exports.join("out");
        let mut sink = DirectoryBundle::new(&dir);
        export_space(
            &self.pool,
            &ExportRequest {
                space: &SpaceId(HERE.to_string()),
                schema: &self.fixture.schema,
                artifact: &self.artifact,
                keys: self.fixture.keys.as_ref(),
                blobs: None,
                sealed: SealedMode::Open,
                correlation: "exp_flat",
            },
            &mut sink,
        )
        .await
        .expect("the source exports");
        copy_tree(&dir, &self.staging.join(name));
    }

    /// Stage a people map beside a bundle, as the operator would.
    ///
    /// Beside and not inside: the bundle's manifest carries a digest per
    /// member and the importer refuses one it does not describe, so a file
    /// added inside the bundle would make it incomplete.
    fn stage_people(&self, space: &str, bundle: &str, pairs: &[(&str, &str)]) {
        let map: serde_json::Map<String, Value> = pairs
            .iter()
            .map(|(s, d)| ((*s).to_string(), json!(d)))
            .collect();
        let dir = self.staging.join(space);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{bundle}.people")),
            serde_json::to_string(&Value::Object(map)).unwrap(),
        )
        .unwrap();
    }

    /// Whatever is staged as the map, written verbatim, so a test can stage
    /// something that is not a map at all.
    fn stage_people_raw(&self, space: &str, bundle: &str, raw: &str) {
        let dir = self.staging.join(space);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{bundle}.people")), raw).unwrap();
    }

    async fn import(&self, bundle: &str, name: &str) -> Result<Value, ordo_api::ExecError> {
        let bound = BoundParams(
            [
                ("bundle".to_string(), json!(bundle)),
                ("name".to_string(), json!(name)),
            ]
            .into_iter()
            .collect(),
        );
        self.ops
            .import(TWIN, &bound, IMPORTER)
            .await
            .map(|r| r.rows.into_iter().next().unwrap_or(Value::Null))
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.pool
            .rows(sql)
            .await
            .expect("query")
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next())
            .flatten()
    }
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// **The test the custody assignment exists for.**
///
/// The two providers are the whole mechanism. `Fixture::compile_source`
/// mints its own `TestProvider` over its own `MemoryKeyStore`, and the
/// harness gives the deployment a **different** provider instance in which
/// `TWIN` was created. So:
///
/// - with the assignment, the import seals through the deployment's
///   provider, `TWIN`'s key is there, the rows land and read back;
/// - without it, the import seals through the compiler's provider, which has
///   never heard of `TWIN`, and it fails instead of quietly writing rows
///   under a key nobody chose.
///
/// A single shared provider would have made this test pass either way, which
/// is why the harness does not use one. That is the vacuous version of this
/// assertion and it is the version I wrote first.
#[tokio::test]
async fn a_sealed_value_is_sealed_under_the_destinations_own_custody() {
    let h = harness("import_op_custody").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    // Every row names a person, so the import needs to be told who that
    // person is in the destination. `a_row_that_names_a_person_...` below is
    // the test for the map itself.
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER)]);

    let report = h.import("first", "run1").await.expect("the import lands");
    assert_eq!(report["rows"], json!(1), "one row arrived: {report}");

    // Read back through the destination's space under the deployment's
    // custody, which is the pair every ordinary request runs under. A sealed
    // column is opened host-side on projection, so this fails if the seal and
    // the reader disagree about whose key it is.
    let rows = h.read_destination();
    let rows = rows.as_array().expect("an array of rows");
    assert_eq!(rows.len(), 1, "one row in the destination: {rows:?}");
    assert_eq!(
        rows[0]["memo"],
        json!(MEMO),
        "the sealed value must open under the destination's own custody"
    );
    assert_eq!(rows[0]["title"], json!("kept"));
}

/// A bundle the caller's own space has no directory for is not nameable,
/// **even when it is standing exactly where a flat layout would find it**.
///
/// The space component of the staging path is an authorization boundary: one
/// root serves a deployment with many tenants, and every tenant's owner
/// passes every gate this op has. Remove the component and this test loads
/// somebody else's whole space into the caller's own, under the caller's
/// keys.
#[tokio::test]
async fn a_bundle_outside_the_bound_spaces_directory_cannot_be_named() {
    let h = harness("import_op_boundary").await;
    h.seed_source().await;
    // Flat under the root: the path a boundary-less resolver builds.
    h.stage_flat("theirs").await;

    let refused = h.import("theirs", "run1").await.expect_err("refused");
    assert_eq!(refused.code, ErrorCode::ImportRefused);
    assert!(
        refused.message.starts_with("bundle_unknown:"),
        "{}",
        refused.message
    );
    // And nothing arrived, which is the claim that matters: a refusal that
    // fired after the first batch would have leaked most of the rows.
    assert_eq!(
        h.one(&format!(
            "SELECT count(*) FROM \"notes\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .as_deref(),
        Some("0"),
        "a refused import must write nothing"
    );
}

/// An unspellable name and an absent bundle are **one** answer, byte for
/// byte, so the staging directory cannot be mapped a guess at a time.
#[tokio::test]
async fn an_illegal_name_and_an_absent_bundle_are_indistinguishable() {
    let h = harness("import_op_oracle").await;
    let absent = h.import("nothinghere", "run1").await.expect_err("refused");
    let traversal = h.import("../../etc", "run1").await.expect_err("refused");
    assert_eq!(absent.code, traversal.code);
    assert_eq!(
        absent.message, traversal.message,
        "a caller must not be able to tell an illegal name from a missing bundle"
    );
}

/// The op's name is in the gate lists, which is the one thing the tests above
/// cannot see: a handler that works and is reachable by a viewer is worse
/// than one that does not work.
#[test]
fn the_import_op_is_owner_only() {
    assert!(ordo_host::platform_ops::owner_only(IMPORT));
}

/// **The ruling, forwards.** A row that names a person lands on the person the
/// operator named, and the mapping is stored where anyone can read it later.
#[tokio::test]
async fn a_row_that_names_a_person_lands_on_the_person_the_operator_named() {
    let h = harness("import_op_people").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER)]);

    let report = h.import("first", "run1").await.expect("the import lands");
    assert_eq!(report["rows"], json!(1));
    assert_eq!(
        report["peopleMapped"],
        json!(1),
        "the map's size is reported, because an import with the wrong map \
         produces a space whose history names the wrong people: {report}"
    );

    // The row points at the DESTINATION's member, not the source's id.
    assert_eq!(
        h.one(&format!(
            "SELECT \"owner\" FROM \"notes\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .as_deref(),
        Some(IMPORTER),
        "the ref must be rewritten to the mapped member"
    );

    // And the mapping is stored under this import's name, so what an import
    // attributed is answerable afterwards rather than only at the time.
    assert_eq!(
        h.one(&format!(
            "SELECT \"row_id\" FROM \"$importmap\" WHERE \"$space\" = '{TWIN}' \
             AND \"import\" = 'run1' AND \"entity\" = 'members' \
             AND \"source_id\" = '{AUTHOR}'"
        ))
        .await
        .as_deref(),
        Some(IMPORTER),
    );
}

/// **The ruling, backwards.** No map is not a licence to guess. The import
/// refuses by name and writes nothing, which is what it did before the map
/// existed and must keep doing.
#[tokio::test]
async fn an_import_with_no_people_map_refuses_and_writes_nothing() {
    let h = harness("import_op_nomap").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    // No `stage_people`.

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert_eq!(refused.code, ErrorCode::ImportRefused);
    assert!(
        refused.message.contains("unresolved_ref"),
        "{}",
        refused.message
    );
    assert_eq!(
        h.one(&format!(
            "SELECT count(*) FROM \"notes\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .as_deref(),
        Some("0"),
    );
}

/// A map may not admit anybody. Pointing a row at a principal who is not a
/// member of the destination is refused, which is the half of the ruling that
/// refuses option (c) outright: a file that could add members would be a file
/// that grants access to the space it is loaded into.
#[tokio::test]
async fn a_map_naming_someone_who_is_not_a_member_is_refused() {
    let h = harness("import_op_nonmember").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, "mem_never_admitted")]);

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert_eq!(refused.code, ErrorCode::ImportRefused);
    assert!(
        refused.message.starts_with("people_unknown:"),
        "{}",
        refused.message
    );
    // Refused before the destination was touched, not part way through.
    assert_eq!(
        h.one(&format!(
            "SELECT count(*) FROM \"notes\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .as_deref(),
        Some("0"),
    );
}

/// Two source people onto one destination member is a **merge**, and it is
/// refused rather than performed: every reference to either source id would
/// resolve to the same row with nothing to show it had happened.
#[tokio::test]
async fn a_map_that_merges_two_people_is_refused() {
    let h = harness("import_op_merge").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    h.stage_people(
        TWIN,
        "first",
        &[(AUTHOR, IMPORTER), ("mem_other", IMPORTER)],
    );

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert!(
        refused.message.starts_with("people_ambiguous:"),
        "{}",
        refused.message
    );
}

/// A second run under the same name with a **different** map is refused. The
/// rows already loaded were attributed under the stored mapping, so honouring
/// a new one would split one import's authorship between two answers.
#[tokio::test]
async fn a_changed_map_under_a_used_import_name_is_refused() {
    let h = harness("import_op_changed").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER)]);
    h.import("first", "run1")
        .await
        .expect("the first run lands");

    // The same import name, a different answer about who the author is.
    h.stage_people(TWIN, "first", &[(AUTHOR, SECOND)]);
    let refused = h.import("first", "run1").await.expect_err("refused");
    assert!(
        refused.message.starts_with("people_changed:"),
        "{}",
        refused.message
    );

    // The stored mapping is untouched, which is what makes the refusal safe
    // to retry from.
    assert_eq!(
        h.one(&format!(
            "SELECT \"row_id\" FROM \"$importmap\" WHERE \"$space\" = '{TWIN}' \
             AND \"import\" = 'run1' AND \"entity\" = 'members' \
             AND \"source_id\" = '{AUTHOR}'"
        ))
        .await
        .as_deref(),
        Some(IMPORTER),
    );
}

/// A map that is not JSON is refused by name rather than read as absent. The
/// distinction matters: absent means *nobody is mapped and every person-row
/// refuses*, and a typo in a staged file must not read as that.
#[tokio::test]
async fn a_map_that_is_not_json_is_refused_rather_than_treated_as_absent() {
    let h = harness("import_op_badmap").await;
    h.seed_source().await;
    h.stage(TWIN, "first").await;
    h.stage_people_raw(TWIN, "first", "mem_author = mem_dest\n");

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert!(
        refused.message.starts_with("people_unreadable:"),
        "{}",
        refused.message
    );
}

// ── The tenant's access profile ──────────────────────────────────────────

/// A second person in the source space.
const SRC2: &str = "mem_src2";

impl Harness {
    /// Seed the source tenant's profile: a second person, two groups (one
    /// with both people, one empty), a line that names a role and a group,
    /// a line of roles alone, and an override for each person.
    async fn seed_profile(&self) {
        for ddl in [
            ordo_conformance::access_profile::OPACCESS_DDL,
            ordo_conformance::access_profile::MEMBERACCESS_DDL,
        ] {
            self.pool.rows(ddl).await.expect("profile tables");
        }
        for sql in [
            format!(
                "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \
                 \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
                 VALUES ('{HERE}', '{SRC2}', 1, '{SRC2}', 'Second', 'principal', 1, 'editor', \
                 'active', CURRENT_DATE)"
            ),
            format!(
                "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \
                 \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
                 VALUES ('{HERE}', 'grp_fin', 1, 'grp_grp_fin', 'Finance', 'group', 2, NULL, \
                 'active', CURRENT_DATE), \
                 ('{HERE}', 'grp_ops', 1, 'grp_grp_ops', 'Ops', 'group', 0, NULL, \
                 'active', CURRENT_DATE)"
            ),
            format!(
                "INSERT INTO \"groupMembers\" (\"$space\", \"id\", \"group\", \"member\") \
                 VALUES ('{HERE}', 'gm_1', 'grp_fin', '{AUTHOR}'), \
                 ('{HERE}', 'gm_2', 'grp_fin', '{SRC2}')"
            ),
            format!(
                "INSERT INTO \"$opaccess\" (\"$space\", \"op\", \"roles\", \"groups\", \"rev\") \
                 VALUES ('{HERE}', 'std.notes.get', 'owner,manager', 'grp_fin', 1), \
                 ('{HERE}', 'std.notes.create', 'owner', '', 3)"
            ),
            format!(
                "INSERT INTO \"$memberaccess\" (\"$space\", \"member\", \"op\", \"effect\") \
                 VALUES ('{HERE}', '{AUTHOR}', 'std.notes.create', 'deny'), \
                 ('{HERE}', '{SRC2}', 'std.notes.get', 'allow')"
            ),
        ] {
            self.pool.rows(&sql).await.expect("profile seeds");
        }
    }

    /// One space's profile with every id replaced by a label, so the source's
    /// and the destination's can be compared although every id differs.
    async fn profile_view(&self, space: &str, people: &[(&str, &str)]) -> Value {
        let label = |id: &str| -> String {
            people
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, l)| (*l).to_string())
                .unwrap_or_else(|| format!("?{id}"))
        };
        let q = |sql: String| {
            let pool = self.pool.clone();
            async move { pool.rows(&sql).await.expect("query") }
        };
        let cell = |r: &Vec<Option<String>>, i: usize| r[i].clone().unwrap_or_default();
        let names: std::collections::BTreeMap<String, String> = q(format!(
            "SELECT \"id\", \"displayName\" FROM \"members\" WHERE \"$space\" = '{space}' \
             AND \"kind\" = 'group'"
        ))
        .await
        .iter()
        .map(|r| (cell(r, 0), cell(r, 1)))
        .collect();
        let lines: Vec<Value> = q(format!(
            "SELECT \"op\", \"roles\", \"groups\" FROM \"$opaccess\" WHERE \"$space\" = '{space}' \
             ORDER BY \"op\""
        ))
        .await
        .iter()
        .map(|r| {
            let mut groups: Vec<String> = cell(r, 2)
                .split(',')
                .filter(|g| !g.is_empty())
                .map(|g| names.get(g).cloned().unwrap_or_else(|| format!("?{g}")))
                .collect();
            groups.sort();
            json!({ "op": cell(r, 0), "roles": cell(r, 1), "groups": groups })
        })
        .collect();
        let mut overrides: Vec<Value> = q(format!(
            "SELECT \"member\", \"op\", \"effect\" FROM \"$memberaccess\" \
             WHERE \"$space\" = '{space}'"
        ))
        .await
        .iter()
        .map(|r| json!({ "member": label(&cell(r, 0)), "op": cell(r, 1), "effect": cell(r, 2) }))
        .collect();
        overrides.sort_by_key(|v| v.to_string());
        let mut groups = serde_json::Map::new();
        for (id, name) in &names {
            let mut held: Vec<String> = q(format!(
                "SELECT \"member\" FROM \"groupMembers\" WHERE \"$space\" = '{space}' \
                 AND \"group\" = '{id}'"
            ))
            .await
            .iter()
            .map(|r| label(&cell(r, 0)))
            .collect();
            held.sort();
            groups.insert(name.clone(), json!(held));
        }
        json!({ "lines": lines, "overrides": overrides, "groups": groups })
    }
}

/// **The profile travels with the tenant.** Export then import into a fresh
/// tenant reproduces the op lines, the overrides, the groups and who is in
/// them. Ids differ (the groups are minted in the destination, the people
/// are the map's), so the two profiles are compared by name.
#[tokio::test]
async fn an_exported_profile_is_restored_exactly_in_a_fresh_tenant() {
    let h = harness("import_op_profile").await;
    h.seed_profile().await;
    h.stage(TWIN, "first").await;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(h.staging.join(TWIN).join("first").join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["profile"]["file"], "profile.json",
        "the manifest names the member: {manifest}"
    );
    assert_eq!(
        (
            manifest["profile"]["lines"].as_u64(),
            manifest["profile"]["overrides"].as_u64(),
            manifest["profile"]["groups"].as_u64(),
            manifest["profile"]["links"].as_u64(),
        ),
        (Some(2), Some(2), Some(2), Some(2))
    );
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER), (SRC2, SECOND)]);

    let report = h.import("first", "run1").await.expect("the import lands");
    assert_eq!(report["profileRows"], json!(8), "{report}");

    let people_src = [(AUTHOR, "author"), (SRC2, "second")];
    let people_dst = [(IMPORTER, "author"), (SECOND, "second")];
    let source = h.profile_view(HERE, &people_src).await;
    let dest = h.profile_view(TWIN, &people_dst).await;
    assert_eq!(dest, source, "the profile is the same profile");
    assert_eq!(
        dest["groups"],
        json!({ "Finance": ["author", "second"], "Ops": [] }),
        "{dest}"
    );

    // It is in the destination's chain.
    let n = h
        .one(&format!(
            "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = '{TWIN}'"
        ))
        .await;
    assert!(
        n.as_deref().is_some_and(|n| n != "0"),
        "the profile commands are on the chain: {n:?}"
    );

    // Run again under the same name: nothing is written twice.
    let again = h.import("first", "run1").await.expect("the resume lands");
    assert_eq!(again["profileRows"], json!(0), "{again}");
    assert_eq!(h.profile_view(TWIN, &people_dst).await, source);
}

/// **A line the program cannot honour is refused, not dropped.** A dropped
/// line leaves its op at the app developer's default, which can be wider than
/// what the tenant set. Nothing is written.
#[tokio::test]
async fn a_profile_naming_a_removed_op_is_refused_and_writes_nothing() {
    let h = harness("import_op_profile_gone").await;
    h.seed_profile().await;
    h.pool
        .rows(&format!(
            "INSERT INTO \"$opaccess\" (\"$space\", \"op\", \"roles\", \"groups\", \"rev\") \
             VALUES ('{HERE}', 'std.notes.removedLongAgo', 'owner', '', 1)"
        ))
        .await
        .unwrap();
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER), (SRC2, SECOND)]);

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert_eq!(refused.code, ErrorCode::ImportRefused);
    assert!(
        refused.message.starts_with("profile_op_unknown:")
            && refused.message.contains("std.notes.removedLongAgo")
            && refused.message.contains("default"),
        "{}",
        refused.message
    );
    for table in ["members", "groupMembers", "$opaccess", "$memberaccess"] {
        let extra = if table == "members" {
            " AND \"kind\" = 'group'"
        } else {
            ""
        };
        assert_eq!(
            h.one(&format!(
                "SELECT count(*) FROM \"{table}\" WHERE \"$space\" = '{TWIN}'{extra}"
            ))
            .await
            .as_deref(),
            Some("0"),
            "{table} stays empty"
        );
    }
}

/// A member the people map does not place is refused by name: the profile
/// never lands on the wrong person.
#[tokio::test]
async fn a_profile_naming_a_member_the_map_does_not_place_is_refused() {
    let h = harness("import_op_profile_unmapped").await;
    h.seed_profile().await;
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER)]);

    let refused = h.import("first", "run1").await.expect_err("refused");
    assert_eq!(refused.code, ErrorCode::ImportRefused);
    assert!(
        refused.message.starts_with("profile_member_unmapped:") && refused.message.contains(SRC2),
        "{}",
        refused.message
    );
}

/// Merging into a tenant that already has groups or lines has no ruled
/// policy, so it is refused.
#[tokio::test]
async fn a_profile_is_not_merged_into_a_tenant_that_has_one() {
    let h = harness("import_op_profile_busy").await;
    h.seed_profile().await;
    h.pool
        .rows(&format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \
             \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
             VALUES ('{TWIN}', 'grp_there', 1, 'grp_grp_there', 'Already', 'group', 0, NULL, \
             'active', CURRENT_DATE)"
        ))
        .await
        .unwrap();
    h.stage(TWIN, "first").await;
    h.stage_people(TWIN, "first", &[(AUTHOR, IMPORTER), (SRC2, SECOND)]);
    let refused = h.import("first", "run1").await.expect_err("refused");
    assert!(
        refused
            .message
            .starts_with("profile_destination_not_empty:"),
        "{}",
        refused.message
    );
}
