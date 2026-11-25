//! **Removing landed `prev` lines** (ruled 2026-10-04, `syntax-additions`
//! ask 1). A clean `ordo apply` names each `prev` mark whose rename has
//! landed in every environment (`ordo_host::prev_offer`). `ordo apply
//! --remove-landed-prev` then cuts those marks out of the source, and only
//! those: the parser gives each mark's exact place, so nothing else on the
//! line moves.

use std::path::{Path, PathBuf};

/// The line `render_apply` prints for each landed mark, which `apply` reads
/// back to remove it.
pub const LANDED: &str = "prev landed ";

/// The marks a rendered apply names as landed.
pub fn read_landed(report: &str) -> Vec<String> {
    report
        .lines()
        .filter_map(|l| l.strip_prefix(LANDED))
        .filter_map(|l| l.split('`').nth(1))
        .map(str::to_string)
        .collect()
}

/// One cut: the file, and the bytes to remove from it.
struct Cut {
    file: PathBuf,
    text: String,
    line: u32,
}

/// Remove each mark in `marks` (as `PrevMark::text` writes them) from the
/// source under `path`. Answers one line per mark removed.
pub fn remove(path: &Path, marks: &[String]) -> Result<Vec<String>, String> {
    let src = crate::Sources::read(path)?;
    let program = ordo_syntax::parse_program(src.text()).map_err(|e| e.to_string())?;
    let mut cuts = Vec::new();
    for e in &program.entities {
        if let (Some(p), Some(span)) = (&e.prev, e.prev_span) {
            let text = format!("{} (prev: {})", e.name.name, p.name);
            if marks.contains(&text) {
                cuts.push(cut(path, &src, span)?);
            }
        }
        for f in &e.fields {
            let Some(span) = f.prev_span else { continue };
            let names: Vec<String> = f.prev.iter().map(|n| n.text()).collect();
            let text = format!("{}.{} prev({})", e.name.name, f.name.name, names.join(", "));
            if marks.contains(&text) {
                cuts.push(cut(path, &src, span)?);
            }
        }
    }
    let mut done = Vec::new();
    for c in cuts {
        let body =
            std::fs::read_to_string(&c.file).map_err(|e| format!("{}: {e}", c.file.display()))?;
        let mut lines: Vec<String> = body.split('\n').map(str::to_string).collect();
        let at = c.line.saturating_sub(1) as usize;
        let Some(l) = lines.get_mut(at) else {
            return Err(format!("{}: no line {}", c.file.display(), c.line));
        };
        let Some(i) = l.find(&c.text) else {
            return Err(format!(
                "{}:{}: `{}` was not found where the parser put it",
                c.file.display(),
                c.line,
                c.text
            ));
        };
        l.replace_range(i..i + c.text.len(), "");
        std::fs::write(&c.file, lines.join("\n"))
            .map_err(|e| format!("{}: {e}", c.file.display()))?;
        done.push(format!("{}:{}", c.file.display(), c.line));
    }
    Ok(done)
}

fn cut(path: &Path, src: &crate::Sources, span: ordo_syntax::Span) -> Result<Cut, String> {
    let text = src
        .text()
        .get(span.start..span.end)
        .ok_or_else(|| "a `prev` mark's place is outside the source".to_string())?
        .to_string();
    let (label, line) = src
        .file_of(span.line)
        .map(|(l, n)| (l.to_string(), n))
        .unwrap_or_default();
    let file = if path.is_dir() {
        path.join(&label)
    } else {
        path.to_path_buf()
    };
    Ok(Cut { file, text, line })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_landed_field_mark_and_an_entity_mark_are_cut_and_nothing_else_moves() {
        let dir = std::env::temp_dir().join(format!("ordo-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("app.ordo");
        std::fs::write(
            &file,
            "domain notes @ 1.0.0\n\nentity notes (key: note, prev: memos) {\n  heading: text prev(title)\n  body: text?\n}\n",
        )
        .unwrap();
        let report = format!(
            "{LANDED}`notes.heading prev(title)` has landed\n{LANDED}`notes (prev: memos)` has landed\n"
        );
        let marks = read_landed(&report);
        assert_eq!(marks.len(), 2);
        let done = remove(&dir, &marks).unwrap();
        assert_eq!(done.len(), 2);
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            after,
            "domain notes @ 1.0.0\n\nentity notes (key: note) {\n  heading: text\n  body: text?\n}\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entity_mark_alone_takes_its_parenthesis() {
        let dir = std::env::temp_dir().join(format!("ordo-prev1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("app.ordo");
        std::fs::write(
            &file,
            "domain notes @ 1.0.0\n\nentity notes (prev: memos) {\n  body: text?\n}\n",
        )
        .unwrap();
        remove(&file, &["notes (prev: memos)".to_string()]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "domain notes @ 1.0.0\n\nentity notes {\n  body: text?\n}\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
