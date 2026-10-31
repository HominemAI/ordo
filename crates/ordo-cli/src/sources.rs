//! A program is a **directory**, not a file.
//!
//! P7 says `ordo check <dir>`, and it says it because a schema and the ops
//! over it are normally more than one file: the market fixture is one file
//! only because it is a fixture. The compiler has one entry point that
//! takes one `&str`, so the directory has to become one source text
//! somewhere — and the cost of doing that naively is that every span in
//! every diagnostic points into a buffer the author never wrote.
//!
//! [`Sources`] is that concatenation plus the line map that undoes it. A
//! diagnostic comes back as `orders.ordo:12:5`, which is the only form an
//! editor can jump to.
//!
//! **Ordering is deterministic and it matters.** Files are visited in
//! sorted order of their path *relative to the root*, compared as bytes,
//! and directories are walked before their contents are emitted so the
//! order is a property of the tree and not of the filesystem's readdir.
//! Two machines with the same directory produce the same buffer, so they
//! produce the same spans, the same op order, and the same artifact bytes.

use std::path::{Path, PathBuf};

/// One source text assembled from one or more files, with the map back.
#[derive(Debug)]
pub struct Sources {
    text: String,
    /// `(first line of this file in `text`, label)`, in buffer order.
    /// Lines are 1-based, matching [`ordo_syntax::Span`].
    files: Vec<(u32, String)>,
}

impl Sources {
    /// Read a path: a single `.ordo` file, or a directory of them.
    ///
    /// A directory with no `.ordo` file in it is an error rather than an
    /// empty program — "nothing to check" is almost always a wrong path,
    /// and reporting success for it is how a CI step passes by checking
    /// nothing.
    pub fn read(path: &Path) -> Result<Sources, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_file() {
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            return Ok(Sources::from_parts(vec![(label_of(path), text)]));
        }
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        collect(path, path, &mut found)?;
        // Sort by the RELATIVE path's bytes: the absolute prefix differs
        // per machine and per checkout, and sorting by it would make the
        // op order depend on where the repository lives.
        found.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        if found.is_empty() {
            return Err(format!("{}: no .ordo files here", path.display()));
        }
        let mut parts = Vec::new();
        for (rel, full) in found {
            let text = std::fs::read_to_string(&full).map_err(|e| format!("{rel}: {e}"))?;
            parts.push((rel, text));
        }
        Ok(Sources::from_parts(parts))
    }

    /// Build directly from `(label, text)` pairs — the seam tests use, so
    /// the line map is testable without touching a filesystem.
    pub fn from_parts(parts: Vec<(String, String)>) -> Sources {
        let mut text = String::new();
        let mut files = Vec::new();
        let mut line = 1u32;
        for (label, part) in parts {
            // Each file starts on its own line whatever the last one ended
            // with, so a file that forgets its trailing newline cannot
            // fuse its last declaration onto the next file's first one.
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
                line += 1;
            }
            files.push((line, label));
            text.push_str(&part);
            line += part.matches('\n').count() as u32;
            if !part.ends_with('\n') {
                text.push('\n');
                line += 1;
            }
        }
        Sources { text, files }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The files that went in, in buffer order — what `ordo check` prints
    /// so the reader knows what was checked and in which order.
    pub fn labels(&self) -> Vec<&str> {
        self.files.iter().map(|(_, l)| l.as_str()).collect()
    }

    /// The file a buffer line came from and the line within it, `None` for
    /// an anonymous source. `ordo ops` shows and filters ops by it.
    pub fn file_of(&self, line: u32) -> Option<(&str, u32)> {
        let mut hit = None;
        for (first, label) in &self.files {
            if *first <= line {
                hit = Some((label.as_str(), line - first + 1));
            } else {
                break;
            }
        }
        hit.filter(|(l, _)| !l.is_empty())
    }

    /// Map a buffer position back to `file:line:col` — or to a bare
    /// `line:col` when the source is anonymous (an empty label: a test, a
    /// playground buffer). An anonymous source must not grow a phantom
    /// filename; a reader who follows one wastes the trip.
    pub fn at(&self, span: ordo_syntax::Span) -> String {
        let mut hit = self.files.first();
        for f in &self.files {
            if f.0 <= span.line {
                hit = Some(f);
            } else {
                break;
            }
        }
        match hit {
            None => format!("{}:{}", span.line, span.col),
            Some((_, label)) if label.is_empty() => format!("{}:{}", span.line, span.col),
            Some((first, label)) => {
                format!("{label}:{}:{}", span.line - first + 1, span.col)
            }
        }
    }
}

/// A single file is labelled by the path the caller gave, not by its base
/// name: `ordo check src/orders.ordo` should report `src/orders.ordo:12:5`,
/// which an editor can open. (A directory's files are labelled relative to
/// the directory, which is the form that stays stable across checkouts.)
fn label_of(path: &Path) -> String {
    path.display().to_string()
}

/// Recursive walk, collecting `(relative path, full path)`. Entries are
/// sorted at every level so a nested tree is walked in one order on every
/// machine; the caller sorts the flattened result again, which is what
/// actually pins the order.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| {
            e.map(|e| e.path())
                .map_err(|e| format!("{}: {e}", dir.display()))
        })
        .collect::<Result<_, _>>()?;
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if p.extension().is_some_and(|x| x == "ordo") {
            let rel = p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| p.to_string_lossy().into_owned());
            out.push((rel, p));
        }
    }
    Ok(())
}
