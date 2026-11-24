//! **`ordo fix <dir|file.ordo> [--write]`**: rewrite a program's source
//! from an old language edition to the current one (`plan-versions.md`
//! §2.6, §2.8 step R4).
//!
//! It works on the developer's machine. It needs no door and no database.
//! Without `--write` it is a dry run: it prints the diff and writes
//! nothing. With `--write` it checks that the rewritten program compiles,
//! then writes each changed file.
//!
//! The rewrite keeps the program's meaning, so the fixed source compiles
//! to the same program. Applied, it is the same version, with the new
//! edition recorded on it (§1.1).

use std::path::Path;

use ordo_plan::edition::Editions;

use crate::sources::{self, Sources};

/// What a run printed, and its exit code.
pub struct Outcome {
    pub code: i32,
    pub report: String,
}

/// `ordo fix <path> [--write]`.
pub fn run(path: &Path, write: bool) -> Outcome {
    match fix(path, write) {
        Ok(report) => Outcome { code: 0, report },
        Err(report) => Outcome {
            code: 1,
            report: format!("ordo fix: {report}\n"),
        },
    }
}

fn fix(path: &Path, write: bool) -> Result<String, String> {
    let files = sources::files(path)?;
    if files.is_empty() {
        return Err(format!("{}: no .ordo files here", path.display()));
    }
    let mut parts = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        parts.push((f.display().to_string(), text));
    }
    let editions = Editions::this_release();
    let fixed = editions.fix(&parts)?;
    if fixed.from == fixed.to {
        return Ok(format!(
            "{} is at edition {}, the current edition. Nothing to change.\n",
            path.display(),
            fixed.to
        ));
    }
    let mut out = format!(
        "{}: edition {} to edition {}\n",
        path.display(),
        fixed.from,
        fixed.to
    );
    for c in &fixed.changes {
        out.push_str(&format!("  {c}\n"));
    }
    out.push_str(&format!("  the source says `edition {}`\n\n", fixed.to));
    out.push_str(&fixed.diff());
    if !write {
        out.push_str(&format!(
            "\ndry run: nothing written. Run `ordo fix {} --write` to write it.\n",
            path.display()
        ));
        return Ok(out);
    }
    // The rewritten program must compile before a byte is written.
    let after = Sources::from_parts(
        fixed
            .files
            .iter()
            .map(|(label, _, new)| (label.clone(), new.clone()))
            .collect(),
    );
    crate::load_sources(&after).map_err(|e| {
        format!(
            "the rewritten source does not compile, so nothing was written. This is a fault \
             in `ordo fix`; please report it.\n{e}"
        )
    })?;
    out.push('\n');
    for (label, _, new) in fixed.changed() {
        std::fs::write(label, new).map_err(|e| format!("{label}: {e}"))?;
        out.push_str(&format!("wrote {label}\n"));
    }
    out.push_str(
        "Apply it as any change: it compiles to the same program, so it is the same version.\n",
    );
    Ok(out)
}
