//! Every route the served door mounts, read from the code that mounts it.
//!
//! `runtime::door` is the whole HTTP surface the binary binds: the op door
//! and blob routes (`ordo_api::router_with_blobs`), the download
//! (`ordo_api::download::router`) and the platform routes
//! (`serve::platform_router`, which merges `auth::auth_routes`). Each of
//! those mounts its routes with `.route("<path>", <method>(...))`. This
//! module reads those calls out of the sources, so a route added to any of
//! them appears here without anybody listing it, and the matrix's guard
//! fails until the policy table says what it expects of it.
//!
//! The op door is one route, `/op/{name}`, and many ops. The ops are
//! enumerated from the program's registry and from
//! `platform_ops::specs()`, again without a hand-written list.

use std::path::{Path, PathBuf};

/// One mounted route: a method and the path pattern as axum spells it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Route {
    pub method: String,
    pub path: String,
    /// The source file it was read from, for the guard's message.
    pub file: String,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace")
        .to_path_buf()
}

/// The files whose `.route(` calls make up `runtime::door`.
fn sources() -> Vec<PathBuf> {
    let w = workspace();
    let mut files = vec![
        w.join("crates/ordo-api/src/router.rs"),
        w.join("crates/ordo-api/src/download.rs"),
        w.join("crates/ordo-host/src/serve.rs"),
        w.join("crates/ordo-host/src/auth.rs"),
    ];
    let auth = w.join("crates/ordo-host/src/auth");
    let mut more: Vec<PathBuf> = std::fs::read_dir(&auth)
        .expect("src/auth")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    more.sort();
    files.extend(more);
    files
}

/// Code lines only: a comment can name a route without mounting it.
fn code(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A path given by a constant rather than a literal: its value, read from
/// the same file.
fn constant(src: &str, name: &str) -> Option<String> {
    let at = src.find(&format!("const {name}: &str = \""))?;
    let rest = &src[at..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')?;
    Some(rest[start..start + end].to_string())
}

/// The text of one call's arguments, from just after its `(`.
fn call_args(s: &str) -> &str {
    let mut depth = 1usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &s[..i];
                }
            }
            _ => {}
        }
    }
    s
}

/// The HTTP methods a route's handler expression names: `get(...)`,
/// `axum::routing::post(...)`, `get(a).post(b)`, `any(...)`.
fn methods(handler: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (word, method) in [
        ("get(", "GET"),
        ("post(", "POST"),
        ("put(", "PUT"),
        ("delete(", "DELETE"),
        ("patch(", "PATCH"),
        ("any(", "ANY"),
    ] {
        let mut rest = handler;
        while let Some(i) = rest.find(word) {
            let before = rest[..i].chars().last();
            // `routing::get(`, `.get(` or the start: not `forget(`.
            if before.is_none_or(|c| c == ':' || c == '.' || c.is_whitespace() || c == '(') {
                out.push(method.to_string());
            }
            rest = &rest[i + word.len()..];
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every route mounted in the door's sources.
pub fn mounted() -> Vec<Route> {
    let mut out = Vec::new();
    for file in sources() {
        let src =
            std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let c = code(&src);
        let mut rest = c.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let args = call_args(rest);
            let (first, handler) = args.split_once(',').unwrap_or((args, ""));
            let first = first.trim();
            let path = if let Some(lit) = first.strip_prefix('"') {
                lit.trim_end_matches('"').to_string()
            } else {
                let name = first.rsplit("::").next().unwrap_or(first);
                constant(&src, name).unwrap_or_else(|| {
                    panic!(
                        "{}: a route at `{first}`, a constant this file does not define",
                        file.display()
                    )
                })
            };
            let rel = file
                .strip_prefix(workspace())
                .unwrap_or(&file)
                .display()
                .to_string();
            for m in methods(handler) {
                out.push(Route {
                    method: m,
                    path: path.clone(),
                    file: rel.clone(),
                });
            }
        }
    }
    out.sort();
    out.dedup_by(|a, b| a.method == b.method && a.path == b.path);
    out
}

/// Every op name the door may serve: the program's, and every platform
/// op this build knows.
pub fn ops(program: &ordo_conformance::Fixture) -> Vec<(String, ordo_api::OpKind)> {
    let mut out: Vec<(String, ordo_api::OpKind)> = ordo_host::build_specs(program)
        .into_iter()
        .map(|s| (s.name, s.kind))
        .collect();
    out.extend(
        ordo_host::platform_ops::specs()
            .into_iter()
            .map(|s| (s.name, s.kind)),
    );
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// The platform ops this build serves: the names `platform_ops::specs()`
/// lists. A deployment token's realm is these, bound to the home space.
pub fn platform_op_names() -> &'static std::collections::BTreeSet<String> {
    static NAMES: std::sync::OnceLock<std::collections::BTreeSet<String>> =
        std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        ordo_host::platform_ops::specs()
            .into_iter()
            .map(|s| s.name)
            // The surfaces the door checks by name, outside the op catalog.
            .chain(
                [
                    ordo_host::serve::HEALTH_DEEP_OP,
                    ordo_host::serve::METRICS_OP,
                    ordo_host::serve::ALERTS_OP,
                    ordo_host::serve::SECEVENTS_OP,
                    ordo_host::serve::USAGE_OP,
                ]
                .map(String::from),
            )
            .collect()
    })
}
