//! `std.space.export` and `std.space.import` under pins
//! (`plan-versions.md` §1.3, §2.9 V2).
//!
//! **Reported fix (2026-11-24).** Both ops read the active version's schema
//! (`ExitOps::deployed`), never the tenant's pin. A tenant pinned to an
//! older version exported under the active version's name and columns, so
//! its bundle would not import into a tenant on its own version, and on a
//! database of its own the read failed on a column it never had. Both ops
//! now read the version the tenant is pinned to.
//!
//! **V2.** A bundle exported on version X imports into an empty tenant
//! served another version: the tenant is pinned to X, cause `restore`, and
//! the rows load under X. X not stored, revoked, or a tenant holding rows
//! are refused, naming the fix, with nothing written and the pin unmoved.
//! A bundle newer than every stored version stays `artifact_mismatch`.
//!
//! A regression looks like a bundle that names the active version for a
//! tenant pinned elsewhere, an import refused across stored versions, or a
//! pin moved by a refused import.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_crypto::{KeyProvider, LocalKeyProvider, MemoryKeyStore};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::exit_ops::ExitOps;
use ordo_host::provision::{self, FoundingOwner};
use ordo_host::tenants;
use ordo_host::versions::Versions;
use serde_json::{json, Value};

const ADA: &str = "p_ada";
const NOW: u64 = 1_700_000_000;

const PROGRAM: &str = "domain notes @ 0.1.0\n\n\
    entity notes {\n  title: text\n  legacy: text = \"\"\n}\n\n\
    op allNotes unbounded =\n  notes[title]\n";

fn v2() -> String {
    // 0.2.0 adds `starred` and stops declaring `legacy`, which stays as an
    // orphaned column until it is pruned.
    PROGRAM.replace("@ 0.1.0", "@ 0.2.0").replace(
        "  title: text\n  legacy: text = \"\"\n",
        "  title: text\n  starred: bool = false\n",
    )
}

struct H {
    pool: Pool,
    _pg: Pg,
    ops: ExitOps,
    root: PathBuf,
}

impl Drop for H {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

async fn harness(stem: &str) -> H {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.unwrap();
    for source in [PROGRAM.to_string(), v2()] {
        apply_op::run(
            &pool,
            &SinglePool::new(pool.clone()),
            &source,
            &apply_op::CodeFiles::new(),
            ADA,
            "t",
            true,
        )
        .await
        .expect("applied");
    }
    let keys: Arc<dyn KeyProvider> = Arc::new(LocalKeyProvider::new(
        b"exit-ops-versions",
        Box::new(MemoryKeyStore::new()),
    ));
    let root =
        std::env::temp_dir().join(format!("ordo-exit-versions-{stem}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("exports")).unwrap();
    std::fs::create_dir_all(root.join("staged")).unwrap();
    let ops = ExitOps {
        pool: pool.clone(),
        pools: Arc::new(SinglePool::new(pool.clone())),
        keys,
        policy: LogPolicy::DEFAULT,
        bundles: root.join("exports"),
        imports: Some(root.join("staged")),
        door: None,
        stream: None,
        blobs: None,
        stores: None,
        now: Arc::new(|| NOW),
    };
    H {
        pool,
        _pg: pg,
        ops,
        root,
    }
}

impl H {
    /// A tenant pinned to `version`, its owner `mem_<name>`.
    async fn tenant(&self, name: &str, version: &str) -> String {
        let id = tenants::mint_space_id();
        tenants::reserve(&self.pool, &id, name, ADA, 100, None)
            .await
            .unwrap();
        provision::provision_space(
            &self.pool,
            self.ops.keys.as_ref(),
            &id,
            &FoundingOwner {
                member_id: &format!("mem_{name}"),
                principal: &format!("p_{name}"),
                display_name: name,
            },
        )
        .await
        .expect("provisioned");
        Versions::on(&self.pool)
            .pin(&id, version, ADA, 100)
            .await
            .expect("pinned");
        tenants::activate(&self.pool, &id, 100).await.unwrap();
        id
    }

    async fn notes(&self, space: &str, titles: &[&str]) {
        for (i, t) in titles.iter().enumerate() {
            self.pool
                .rows(&format!(
                    "INSERT INTO \"notes\" (\"$space\", \"id\", \"rev\", \"title\") \
                     VALUES ('{space}', 'nte_{space}_{i}', 1, '{t}')"
                ))
                .await
                .unwrap();
        }
    }

    async fn titles(&self, space: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .pool
            .rows(&format!(
                "SELECT \"title\" FROM \"notes\" WHERE \"$space\" = '{space}'"
            ))
            .await
            .unwrap()
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        out.sort();
        out
    }

    async fn pin(&self, space: &str) -> String {
        Versions::on(&self.pool)
            .pin_of(space)
            .await
            .unwrap()
            .unwrap()
            .version
            .unwrap()
    }

    /// Export `space`; answer the bundle's directory and its answer.
    async fn export(&self, space: &str) -> (PathBuf, Value) {
        let r = self
            .ops
            .export(space, &BoundParams::default(), &format!("mem_{space}"))
            .await
            .expect("exported");
        let row = r.rows.into_iter().next().unwrap();
        (PathBuf::from(row["bundle"].as_str().unwrap()), row)
    }

    /// Stage `bundle` for `space` under `name`, with every source member
    /// mapped to `owner`.
    fn stage(&self, bundle: &Path, space: &str, name: &str, source_owner: &str, owner: &str) {
        let into = self.root.join("staged").join(space).join(name);
        copy_tree(bundle, &into);
        std::fs::write(
            self.root
                .join("staged")
                .join(space)
                .join(format!("{name}.people")),
            json!({ source_owner: owner }).to_string(),
        )
        .unwrap();
    }

    async fn import(&self, space: &str, bundle: &str, owner: &str) -> Result<Value, String> {
        let bound = BoundParams(
            [
                ("bundle".to_string(), json!(bundle)),
                ("name".to_string(), json!(format!("imp_{bundle}"))),
            ]
            .into_iter()
            .collect(),
        );
        self.ops
            .import(space, &bound, owner)
            .await
            .map(|r| r.rows.into_iter().next().unwrap())
            .map_err(|e| e.message)
    }
}

fn copy_tree(from: &Path, to: &Path) {
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

/// Rewrite the version a staged bundle's manifest names.
fn rename_version(bundle: &Path, version: &str) {
    let path = bundle.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["artifact"]["version"] = json!(version);
    std::fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_pinned_to_an_older_version_exports_every_row_and_imports_back() {
    let h = harness("ordo_exit_versions_1").await;
    let old = h.tenant("old", "0.1.0").await;
    h.notes(&old, &["first", "second", "third"]).await;

    let (bundle, row) = h.export(&old).await;
    assert_eq!(row["version"], "0.1.0", "the tenant's own version: {row}");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["artifact"]["version"], "0.1.0");
    let notes_rows = row["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity"] == "notes")
        .map(|e| e["rows"].clone());
    assert_eq!(notes_rows, Some(json!(3)), "every row: {row}");
    let notes = std::fs::read_to_string(bundle.join("data/notes.jsonl")).unwrap();
    assert!(!notes.contains("starred"), "only 0.1.0's columns: {notes}");

    // Back into a tenant on the same version.
    let twin = h.tenant("twin", "0.1.0").await;
    h.stage(&bundle, &twin, "back", "mem_old", "mem_twin");
    let done = h.import(&twin, "back", "mem_twin").await.expect("imported");
    assert_eq!(done["rows"].as_u64(), Some(3), "{done}");
    assert_eq!(
        done["pinned"],
        Value::Null,
        "no pin moves on its own version"
    );
    assert_eq!(h.titles(&twin).await, ["first", "second", "third"]);
    assert_eq!(h.pin(&twin).await, "0.1.0");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_from_another_stored_version_pins_an_empty_tenant_to_it() {
    let h = harness("ordo_exit_versions_2").await;
    let old = h.tenant("old", "0.1.0").await;
    h.notes(&old, &["kept", "also kept"]).await;
    let (bundle, _) = h.export(&old).await;

    let new = h.tenant("new", "0.2.0").await;
    h.stage(&bundle, &new, "across", "mem_old", "mem_new");
    let done = h.import(&new, "across", "mem_new").await.expect("imported");
    assert_eq!(done["rows"].as_u64(), Some(2), "{done}");
    assert_eq!(done["pinned"]["from"], "0.2.0", "{done}");
    assert_eq!(done["pinned"]["to"], "0.1.0", "{done}");
    assert_eq!(done["pinned"]["cause"], "restore", "{done}");
    assert_eq!(h.pin(&new).await, "0.1.0");
    assert_eq!(h.titles(&new).await, ["also kept", "kept"]);
    let last = Versions::on(&h.pool)
        .pin_history(&new)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        (last.cause.as_str(), last.actor.as_str()),
        ("restore", "mem_new")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_across_versions_is_refused_by_name_and_moves_nothing() {
    let h = harness("ordo_exit_versions_3").await;
    let old = h.tenant("old", "0.1.0").await;
    h.notes(&old, &["one"]).await;
    let (bundle, _) = h.export(&old).await;
    let new = h.tenant("new", "0.2.0").await;

    // A version this deployment does not store.
    h.stage(&bundle, &new, "unstored", "mem_old", "mem_new");
    rename_version(&h.root.join("staged").join(&new).join("unstored"), "0.1.5");
    let e = h.import(&new, "unstored", "mem_new").await.unwrap_err();
    assert!(e.contains("version_not_stored"), "{e}");
    assert!(e.contains("ordo promote 0.1.5"), "{e}");

    // Newer than every stored version: the import's own version check.
    h.stage(&bundle, &new, "newer", "mem_old", "mem_new");
    rename_version(&h.root.join("staged").join(&new).join("newer"), "9.0.0");
    let e = h.import(&new, "newer", "mem_new").await.unwrap_err();
    assert!(e.contains("artifact_mismatch"), "{e}");

    // A tenant that holds rows.
    h.stage(&bundle, &new, "full", "mem_old", "mem_new");
    h.notes(&new, &["mine"]).await;
    let e = h.import(&new, "full", "mem_new").await.unwrap_err();
    assert!(e.contains("space_not_empty"), "{e}");
    assert!(e.contains("Nothing was written"), "{e}");
    assert_eq!(h.titles(&new).await, ["mine"]);

    // A revoked version.
    let empty = h.tenant("empty", "0.2.0").await;
    Versions::on(&h.pool)
        .revoke("0.1.0", "a data bug", None, ADA, 200)
        .await
        .expect("revoked");
    h.stage(&bundle, &empty, "revoked", "mem_old", "mem_empty");
    let e = h.import(&empty, "revoked", "mem_empty").await.unwrap_err();
    assert!(e.contains("version_not_served"), "{e}");
    assert!(e.contains("ordo versions unrevoke 0.1.0"), "{e}");

    for space in [&new, &empty] {
        assert_eq!(h.pin(space).await, "0.2.0", "a refused import moves no pin");
    }
    assert!(h.titles(&empty).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_into_a_database_not_of_the_bundles_shape_is_refused_by_name() {
    let h = harness("ordo_exit_versions_4").await;
    let old = h.tenant("old", "0.1.0").await;
    h.notes(&old, &["one"]).await;
    let (bundle, _) = h.export(&old).await;
    let new = h.tenant("new", "0.2.0").await;
    // `legacy`, orphaned by 0.2.0, is pruned: the database no longer holds
    // a column 0.1.0 writes.
    h.pool
        .rows("ALTER TABLE \"notes\" DROP COLUMN \"legacy\"")
        .await
        .unwrap();
    h.stage(&bundle, &new, "unfit", "mem_old", "mem_new");
    let e = h.import(&new, "unfit", "mem_new").await.unwrap_err();
    assert!(e.contains("version_unfit"), "{e}");
    assert!(e.contains("legacy"), "names what is missing: {e}");
    assert!(e.contains("ordo restore --record --tenant"), "{e}");
    assert!(e.contains("Nothing was written"), "{e}");
    assert_eq!(h.pin(&new).await, "0.2.0", "no pin moved");
    assert!(h.titles(&new).await.is_empty());
}
