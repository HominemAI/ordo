//! `ordo skill install [<project dir>]` — the Ordo agent skill, into an app.
//!
//! Writes the files of the repository's `skills/ordo/` (embedded at build
//! time by `build.rs`, every file under the folder) into
//! `<dir>/.claude/skills/ordo/`, and keeps a marked block in
//! `<dir>/AGENTS.md` that points agents without skill loading at the same
//! files. The skill is the CLI's own version: installing again after an
//! upgrade replaces it.
//!
//! **It replaces what it wrote and nothing else.** A record beside the
//! files (`.ordo-installed`) holds each written file's SHA-256. A file whose
//! bytes still match the record, or already match the new content, is
//! replaced; a file of the same name that matches neither was written or
//! edited by someone else, and the install refuses before writing anything,
//! naming `--force`. A file the record lists that this version no longer
//! ships is removed if unchanged. Other files in the folder are untouched.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Every file under `skills/ordo/`, as `(relative path, bytes)`.
pub const FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/skill_files.rs"));

/// Where the skill goes, under the project directory.
pub const SKILL_DIR: &str = ".claude/skills/ordo";
/// The record of what the install wrote.
pub const RECORD: &str = ".ordo-installed";
pub const BLOCK_BEGIN: &str = "<!-- ordo skill: begin. Written by `ordo skill install`; \
                               edits inside this block are replaced. -->";
pub const BLOCK_END: &str = "<!-- ordo skill: end -->";

/// Why the install wrote nothing.
#[derive(Debug)]
pub enum SkillError {
    /// Files of the same names that the install did not write.
    Edited(Vec<PathBuf>),
    Io {
        path: PathBuf,
        err: std::io::Error,
    },
}

impl fmt::Display for SkillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillError::Edited(paths) => {
                writeln!(
                    f,
                    "ordo skill install: these files were changed since the last install, or \
                     were not written by it, so nothing was written:"
                )?;
                for p in paths {
                    writeln!(f, "  {}", p.display())?;
                }
                write!(
                    f,
                    "Keep a copy of your changes, then run again with --force to replace them"
                )
            }
            SkillError::Io { path, err } => write!(f, "{}: {err}", path.display()),
        }
    }
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> SkillError + '_ {
    move |err| SkillError::Io {
        path: path.to_path_buf(),
        err,
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The record: relative path to the SHA-256 written.
fn read_record(dir: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(dir.join(RECORD)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (hash, rel) = l.split_once("  ")?;
            Some((rel.to_string(), hash.to_string()))
        })
        .collect()
}

/// Install into `project`. Returns the report to print.
pub fn install(project: &Path, force: bool) -> Result<String, SkillError> {
    install_files(project, FILES, force)
}

/// [`install`] over a given file set, so the replace and refuse rules test
/// without rebuilding the binary.
pub fn install_files(
    project: &Path,
    files: &[(&str, &[u8])],
    force: bool,
) -> Result<String, SkillError> {
    let dir = project.join(SKILL_DIR);
    let record = read_record(&dir);

    // Decide everything before writing anything.
    let mut edited = Vec::new();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        let Ok(on_disk) = std::fs::read(&path) else {
            continue;
        };
        let current = digest(&on_disk);
        if current != digest(bytes) && record.get(*rel) != Some(&current) {
            edited.push(path);
        }
    }
    let mut stale = Vec::new();
    for (rel, hash) in &record {
        if files.iter().any(|(r, _)| r == rel) {
            continue;
        }
        let path = dir.join(rel);
        match std::fs::read(&path) {
            Ok(b) if digest(&b) == *hash => stale.push(path),
            Ok(_) => edited.push(path),
            Err(_) => {}
        }
    }
    if !edited.is_empty() && !force {
        return Err(SkillError::Edited(edited));
    }

    let mut report = String::new();
    let mut lines = Vec::new();
    for (rel, bytes) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, bytes).map_err(io(&path))?;
        lines.push(format!("{}  {rel}", digest(bytes)));
        report.push_str(&format!("{}\n", path.display()));
    }
    for path in stale
        .iter()
        .chain(if force { edited.iter() } else { [].iter() })
    {
        if !files.iter().any(|(r, _)| dir.join(r) == *path) {
            std::fs::remove_file(path).map_err(io(path))?;
            report.push_str(&format!("removed {}\n", path.display()));
        }
    }
    lines.push(String::new());
    let record_path = dir.join(RECORD);
    std::fs::write(&record_path, lines.join("\n")).map_err(io(&record_path))?;

    let agents = project.join("AGENTS.md");
    let old = match std::fs::read_to_string(&agents) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io(&agents)(e)),
    };
    let new = with_block(&old, &block(files));
    if new != old {
        std::fs::write(&agents, &new).map_err(io(&agents))?;
    }
    report.push_str(&format!("{}\n", agents.display()));
    Ok(report)
}

/// The AGENTS.md block, naming every installed file.
fn block(files: &[(&str, &[u8])]) -> String {
    let mut s = format!(
        "{BLOCK_BEGIN}\n## Ordo\n\n\
         This app is built on Ordo. Before writing code against it, read\n\
         `{SKILL_DIR}/SKILL.md` (installed by ordo {}). Its files:\n\n",
        env!("CARGO_PKG_VERSION")
    );
    for (rel, _) in files {
        s.push_str(&format!("- `{SKILL_DIR}/{rel}`\n"));
    }
    s.push_str(
        "\nLook up an op with `ordo ops <program dir> [op or word]`, not by reading the \
         generated client.\n",
    );
    s.push_str(BLOCK_END);
    s.push('\n');
    s
}

/// `text` with its marked block replaced, or the block appended.
fn with_block(text: &str, block: &str) -> String {
    if let (Some(b), Some(e)) = (text.find(BLOCK_BEGIN), text.find(BLOCK_END)) {
        if b < e {
            let mut end = e + BLOCK_END.len();
            if text[end..].starts_with('\n') {
                end += 1;
            }
            return format!("{}{block}{}", &text[..b], &text[end..]);
        }
    }
    if text.is_empty() {
        block.to_string()
    } else if text.ends_with('\n') {
        format!("{text}\n{block}")
    } else {
        format!("{text}\n\n{block}")
    }
}
