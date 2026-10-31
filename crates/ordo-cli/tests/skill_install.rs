//! `ordo skill install`: it writes the embedded skill and an AGENTS.md
//! block, replaces what it wrote and nothing else, and refuses a file of
//! the same name someone changed.

use std::path::PathBuf;

use ordo_cli::skill::{install, install_files, BLOCK_BEGIN, BLOCK_END, FILES, SKILL_DIR};

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ordo-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp directory");
    dir
}

#[test]
fn every_file_under_skills_ordo_is_embedded() {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/ordo"));
    let mut on_disk = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                on_disk.push(
                    p.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    on_disk.sort();
    let embedded: Vec<String> = FILES.iter().map(|(r, _)| r.to_string()).collect();
    assert_eq!(embedded, on_disk);
    let skill = FILES
        .iter()
        .find(|(r, _)| *r == "SKILL.md")
        .expect("SKILL.md");
    let text = std::str::from_utf8(skill.1).unwrap();
    assert!(text.starts_with("---\nname: ordo\n"), "{text}");
}

#[test]
fn installing_twice_is_idempotent_and_keeps_the_rest_of_agents_md() {
    let dir = tempdir("skill-twice");
    std::fs::write(dir.join("AGENTS.md"), "# My app\n\nOur own rules.\n").unwrap();
    install(&dir, false).expect("first install");
    let skill = std::fs::read(dir.join(SKILL_DIR).join("SKILL.md")).unwrap();
    assert_eq!(
        skill,
        FILES.iter().find(|(r, _)| *r == "SKILL.md").unwrap().1
    );
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(
        agents.starts_with("# My app\n\nOur own rules.\n"),
        "{agents}"
    );
    assert!(agents.contains(BLOCK_BEGIN) && agents.contains(BLOCK_END));
    assert!(agents.contains(".claude/skills/ordo/SKILL.md"));

    install(&dir, false).expect("second install");
    assert_eq!(
        std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
        agents
    );
    assert_eq!(agents.matches(BLOCK_BEGIN).count(), 1);
}

#[test]
fn an_edited_file_is_refused_naming_force_and_force_replaces_it() {
    let dir = tempdir("skill-edited");
    install(&dir, false).unwrap();
    let path = dir.join(SKILL_DIR).join("SKILL.md");
    std::fs::write(&path, "my own notes\n").unwrap();
    let err = install(&dir, false)
        .expect_err("an edited file is refused")
        .to_string();
    assert!(err.contains("--force"), "{err}");
    assert!(err.contains("SKILL.md"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "my own notes\n");

    install(&dir, true).expect("--force replaces it");
    assert_ne!(std::fs::read_to_string(&path).unwrap(), "my own notes\n");
}

#[test]
fn a_file_of_the_same_name_it_did_not_write_is_refused() {
    let dir = tempdir("skill-foreign");
    let path = dir.join(SKILL_DIR).join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "someone else's skill\n").unwrap();
    assert!(install(&dir, false).is_err());
    assert!(
        !dir.join("AGENTS.md").exists(),
        "nothing is written on a refusal"
    );
}

#[test]
fn a_new_version_replaces_its_own_files_and_leaves_others() {
    let dir = tempdir("skill-versions");
    let v1: &[(&str, &[u8])] = &[("SKILL.md", b"v1\n"), ("old.md", b"old\n")];
    let v2: &[(&str, &[u8])] = &[("SKILL.md", b"v2\n"), ("new/ref.md", b"new\n")];
    install_files(&dir, v1, false).unwrap();
    let skill = dir.join(SKILL_DIR);
    std::fs::write(skill.join("mine.md"), "kept\n").unwrap();

    install_files(&dir, v2, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "v2\n"
    );
    assert_eq!(
        std::fs::read_to_string(skill.join("new/ref.md")).unwrap(),
        "new\n"
    );
    assert!(
        !skill.join("old.md").exists(),
        "a file it wrote and no longer ships is removed"
    );
    assert_eq!(
        std::fs::read_to_string(skill.join("mine.md")).unwrap(),
        "kept\n"
    );
    let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(
        agents.contains("new/ref.md") && !agents.contains("old.md"),
        "{agents}"
    );
}
