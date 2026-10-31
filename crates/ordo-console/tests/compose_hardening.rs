//! The compose file is hardened (audit F6): published ports bind loopback,
//! no secret is made world-readable, and the one image's root is
//! read-only.
//! The files are plain, regular YAML, so this reads them line by line
//! rather than pulling in a YAML parser.

use std::path::PathBuf;

fn read(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docker")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The quoted entries of every one-line `ports: [...]` list.
fn published_ports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("ports:") {
            out.extend(rest.split('"').skip(1).step_by(2).map(str::to_owned));
        }
    }
    assert!(!out.is_empty(), "no published ports found");
    out
}

/// Service name to its body lines, for the top-level `services:` block.
fn services(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_services = false;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.starts_with('#') && !line.is_empty() {
            in_services = line.starts_with("services:");
            continue;
        }
        if !in_services {
            continue;
        }
        if line.starts_with("  ") && !line.starts_with("   ") && line.trim_end().ends_with(':') {
            out.push((line.trim().trim_end_matches(':').to_owned(), Vec::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push(line.to_owned());
        }
    }
    out
}

#[test]
fn published_ports_bind_loopback() {
    for port in published_ports(&read("compose.one.yml")) {
        assert!(
            port.starts_with("127.0.0.1:"),
            "compose.one.yml: port {port:?} must bind 127.0.0.1 (put a TLS proxy in front)"
        );
    }
}

#[test]
fn no_secret_is_made_world_readable() {
    // The compose file points at `scripts/make-secrets.sh`, which is where
    // the modes are set; it may not suggest a world-readable mode.
    let text = read("compose.one.yml");
    assert!(
        !text.contains("0644"),
        "compose.one.yml: mentions mode 0644"
    );
    assert!(
        text.contains("scripts/make-secrets.sh"),
        "compose.one.yml: setup must point at scripts/make-secrets.sh"
    );
    let script = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/make-secrets.sh"),
    )
    .expect("scripts/make-secrets.sh");
    assert!(
        !script.contains("0644"),
        "make-secrets.sh: mentions mode 0644"
    );
    assert!(
        script.contains("chmod 0440"),
        "make-secrets.sh: secrets must be 0440"
    );
}

/// The separate images and their compose file are gone (step 7).
#[test]
fn only_the_one_image_is_provided() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docker");
    assert!(!dir.join("compose.split.yml").exists());
    let dockerfile = read("Dockerfile");
    for stage in [" AS host", " AS console", " AS console-builder"] {
        assert!(!dockerfile.contains(stage), "Dockerfile still has{stage}");
    }
}

#[test]
fn one_image_is_read_only_but_keeps_its_privileges() {
    let text = read("compose.one.yml");
    let all = services(&text);
    let (_, ordo) = all.iter().find(|(n, _)| n == "ordo").expect("ordo service");
    assert!(
        ordo.iter().any(|l| l.trim() == "read_only: true"),
        "ordo read_only"
    );
    assert!(
        !ordo.iter().any(|l| l.trim().starts_with("security_opt")),
        "the supervisor needs file capabilities, so no-new-privileges stays off"
    );
    // Review, step 7: /tmp is writable by every child, so nothing there
    // may run; and shutdown's worst case, 3 children x 2 grace periods of
    // 30 s, fits before Docker's own kill.
    assert!(
        ordo.iter().any(|l| {
            let t = l.trim();
            t.starts_with("- /tmp:") && t.contains("noexec")
        }),
        "ordo: /tmp must be mounted noexec"
    );
    let grace: u64 = ordo
        .iter()
        .find_map(|l| l.trim().strip_prefix("stop_grace_period:"))
        .and_then(|v| v.trim().trim_end_matches('s').parse().ok())
        .expect("ordo: stop_grace_period in seconds");
    assert!(
        grace >= 180,
        "ordo: stop_grace_period {grace}s is under 3 x 60 s"
    );
    let caps = ordo
        .iter()
        .find_map(|l| l.trim().strip_prefix("cap_add:"))
        .expect("ordo: cap_add");
    assert!(
        !caps.contains("KILL"),
        "ordo: the supervisor needs no KILL: {caps}"
    );
}
