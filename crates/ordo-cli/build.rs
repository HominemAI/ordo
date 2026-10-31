//! Embeds every file under the repository's `skills/ordo/` into the `ordo`
//! binary, for `ordo skill install` (`src/skill.rs`).
//!
//! The list is generated, never written by hand: a file added to the folder
//! is in the next build, so the installed skill always matches the CLI that
//! installs it. The output is `$OUT_DIR/skill_files.rs`, one
//! `(relative path, include_bytes!(absolute path))` pair per file, in
//! relative-path order.

use std::path::{Path, PathBuf};

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    // A directory's own entry, so a file added to it reruns this script.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => return,
    };
    entries.sort();
    // Hidden files (`.DS_Store`, an editor's swap file) are not the skill.
    entries.retain(|p| {
        !p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    });
    for p in entries {
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            println!("cargo:rerun-if-changed={}", p.display());
            let rel = p
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, p));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let root = manifest.join("../../skills/ordo");
    let root = root.canonicalize().unwrap_or(root);
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::from("&[\n");
    for (rel, full) in files {
        s.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?}) as &[u8]),\n",
            full.display().to_string()
        ));
    }
    s.push_str("]\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));
    std::fs::write(out.join("skill_files.rs"), s).expect("write skill_files.rs");
}
