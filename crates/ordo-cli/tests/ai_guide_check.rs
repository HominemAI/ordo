//! The Ordo agent skill (`skills/ordo/`) is handed to agents as Ordo's whole
//! language and SDK. Every example in it must mean what it says, and
//! `docs/ai-guide.md` is the same files joined, never a second copy.
//!
//! How a block is checked:
//!
//! - Only fenced blocks tagged `ordo` or `ordo-refused` are read. `ts`,
//!   `tsx`, `text` and untagged blocks are not Ordo.
//! - The files are read in [`PARTS`] order, as one document.
//! - An `ordo` block whose first declaration is `domain` is a whole program.
//!   It must pass `ordo check`, and the blocks after it are added to it.
//! - Any other `ordo` block is appended to the last whole program, and the
//!   result must pass `ordo check`. It does not carry on to later blocks.
//! - An `ordo-refused` block starts `# refused: <code>`. It is appended the
//!   same way, and the result must fail with that code. So a trap the skill
//!   warns about is a real refusal, and stays one.
//!
//! Sizes: an agent reads `SKILL.md` on every task and opens a reference file
//! only when the task needs it. So `SKILL.md` has the tight budget, each
//! reference file its own, and the joined guide a ceiling.
//!
//! Regenerate the guide after editing a skill file:
//! `ORDO_BLESS_AI_GUIDE=1 cargo test -p ordo-cli --test ai_guide_check`
//! (or `scripts/ai-guide.sh`).

use std::path::PathBuf;

use ordo_cli::Sources;

/// The skill's files, in the order the guide joins them and the checks read
/// them. Paths are from the repository root.
const PARTS: [(&str, &str); 8] = [
    (
        "skills/ordo/SKILL.md",
        include_str!("../../../skills/ordo/SKILL.md"),
    ),
    (
        "skills/ordo/language.md",
        include_str!("../../../skills/ordo/language.md"),
    ),
    (
        "skills/ordo/traps.md",
        include_str!("../../../skills/ordo/traps.md"),
    ),
    (
        "skills/ordo/guards-and-roles.md",
        include_str!("../../../skills/ordo/guards-and-roles.md"),
    ),
    (
        "skills/ordo/react.md",
        include_str!("../../../skills/ordo/react.md"),
    ),
    (
        "skills/ordo/signin.md",
        include_str!("../../../skills/ordo/signin.md"),
    ),
    (
        "skills/ordo/client.md",
        include_str!("../../../skills/ordo/client.md"),
    ),
    (
        "skills/ordo/sdk-signatures.md",
        include_str!("../../../skills/ordo/sdk-signatures.md"),
    ),
];

/// Read on every task: the loop and the map.
const SKILL_MAX_BYTES: usize = 6_000;
/// One reference file, opened when its subject comes up.
const PART_MAX_BYTES: usize = 16_000;
/// The joined guide, for a reader who wants one file.
const GUIDE_MAX_BYTES: usize = 70_000;

const GUIDE_PATH: &str = "docs/ai-guide.md";
const BLESS: &str = "ORDO_BLESS_AI_GUIDE";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `SKILL.md` without its frontmatter: the guide is read by people too.
fn body(path: &str, text: &str) -> String {
    if path.ends_with("SKILL.md") {
        if let Some(rest) = text.strip_prefix("---\n") {
            if let Some(end) = rest.find("\n---\n") {
                return rest[end + "\n---\n".len()..].trim_start().to_string();
            }
        }
        panic!("{path} starts with `---` frontmatter");
    }
    text.to_string()
}

/// The guide: every part in order, each ending in one newline, with a blank
/// line between parts.
fn joined() -> String {
    let mut out = String::from(
        "<!-- Generated from skills/ordo/*.md by \
         `ORDO_BLESS_AI_GUIDE=1 cargo test -p ordo-cli --test ai_guide_check`. \
         Edit the skill files, never this one. -->\n\n",
    );
    for (i, (path, text)) in PARTS.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(body(path, text).trim_end());
        out.push('\n');
    }
    out
}

struct Block {
    at: String,
    tag: String,
    text: String,
}

fn blocks(path: &str, src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String, Vec<&str>)> = None;
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(fence) = trimmed.strip_prefix("```") {
            match open.take() {
                None => open = Some((i + 2, fence.trim().to_string(), Vec::new())),
                Some((start, tag, body)) => out.push(Block {
                    at: format!("{path}:{start}"),
                    tag,
                    text: body.join("\n"),
                }),
            }
        } else if let Some((_, _, body)) = open.as_mut() {
            body.push(line);
        }
    }
    assert!(open.is_none(), "{path} ends inside a code block");
    out
}

fn lead_word(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
}

/// `Err` with every code and message `ordo check` would print.
fn check(program: &str) -> Result<(), String> {
    let src = Sources::from_parts(vec![(String::new(), program.to_string())]);
    let failed = ordo_cli::uncompiled_ops(&src)?;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed
            .iter()
            .map(|(op, why)| format!("`{op}` does not compile [{why}]"))
            .collect::<Vec<_>>()
            .join("; "))
    }
}

#[test]
fn every_ordo_block_in_the_skill_passes_and_every_refused_block_fails() {
    let mut base: Option<String> = None;
    let mut passed = 0;
    let mut refused = 0;
    let mut failures = String::new();

    for b in PARTS.iter().flat_map(|(path, text)| blocks(path, text)) {
        let at = &b.at;
        match b.tag.as_str() {
            "ordo" => {
                let whole = lead_word(&b.text) == "domain";
                let program = if whole {
                    b.text.clone()
                } else {
                    match &base {
                        Some(p) => format!("{p}\n\n{}", b.text),
                        None => {
                            failures.push_str(&format!(
                                "\n  {at}: a fragment before any `domain` block"
                            ));
                            continue;
                        }
                    }
                };
                match check(&program) {
                    Ok(()) => passed += 1,
                    Err(e) => failures.push_str(&format!("\n  {at}: fails `ordo check`: {e}")),
                }
                if whole {
                    base = Some(b.text.clone());
                }
            }
            "ordo-refused" => {
                let first = b.text.lines().next().unwrap_or("").trim();
                let Some(code) = first.strip_prefix("# refused:").map(str::trim) else {
                    failures.push_str(&format!(
                        "\n  {at}: an `ordo-refused` block starts `# refused: <code>`"
                    ));
                    continue;
                };
                let Some(p) = &base else {
                    failures.push_str(&format!(
                        "\n  {at}: a refused block before any `domain` block"
                    ));
                    continue;
                };
                match check(&format!("{p}\n\n{}", b.text)) {
                    Ok(()) => failures.push_str(&format!(
                        "\n  {at}: compiles, but the skill says it is refused `{code}`"
                    )),
                    Err(e) if e.contains(&format!("[{code}]")) => refused += 1,
                    Err(e) => {
                        failures.push_str(&format!("\n  {at}: refused, but not `{code}`: {e}"))
                    }
                }
            }
            _ => {}
        }
    }

    assert!(failures.is_empty(), "skills/ordo examples:{failures}");
    // Floors, so a change that drops the examples cannot pass.
    assert!(passed >= 9, "only {passed} `ordo` blocks were checked");
    assert!(
        refused >= 6,
        "only {refused} `ordo-refused` blocks were checked"
    );
}

#[test]
fn the_ai_guide_is_the_skill_files_joined() {
    let want = joined();
    let path = repo_root().join(GUIDE_PATH);
    if std::env::var_os(BLESS).is_some() {
        std::fs::write(&path, &want).expect("write docs/ai-guide.md");
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "{GUIDE_PATH} is not skills/ordo/*.md joined. Edit the skill files, then run \
         `{BLESS}=1 cargo test -p ordo-cli --test ai_guide_check`"
    );
}

#[test]
fn the_skill_stays_short() {
    let mut over = String::new();
    for (path, text) in PARTS {
        let max = if path.ends_with("SKILL.md") {
            SKILL_MAX_BYTES
        } else {
            PART_MAX_BYTES
        };
        if text.len() > max {
            over.push_str(&format!("\n  {path} is {} bytes, over {max}", text.len()));
        }
    }
    let guide = joined().len();
    if guide > GUIDE_MAX_BYTES {
        over.push_str(&format!(
            "\n  {GUIDE_PATH} would be {guide} bytes, over {GUIDE_MAX_BYTES}"
        ));
    }
    assert!(over.is_empty(), "the skill's size budget:{over}");
}

#[test]
fn every_reference_file_is_on_the_map_and_in_the_guide() {
    let skill = PARTS[0].1;
    let dir = repo_root().join("skills/ordo");
    let mut missing = String::new();
    for entry in std::fs::read_dir(&dir).expect("read skills/ordo") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if !name.ends_with(".md") || name == "SKILL.md" {
            continue;
        }
        if !skill.contains(&format!("`{name}`")) {
            missing.push_str(&format!("\n  SKILL.md does not name `{name}`"));
        }
        if !PARTS.iter().any(|(p, _)| p.ends_with(&format!("/{name}"))) {
            missing.push_str(&format!("\n  {name} is not in PARTS, so not in the guide"));
        }
    }
    assert!(missing.is_empty(), "skills/ordo:{missing}");
}
