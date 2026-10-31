//! Backend functions, checked against the program
//! (`plan-backend-functions.md` §4.1, increment 1).
//!
//! A function is the app developer's own code. The program declares what it
//! may reach: the hosts in `egress`, the secrets in `secrets`, and its
//! limits. What is checked here is what the declaration alone can answer.
//! The code file itself is read, size-checked and hashed by `ordo apply`.
//!
//! | Class | Refused |
//! |---|---|
//! | `invalid_function` | a duplicate name, a name an op or routine has, a domain that is not the program's, an answer other than `json`, a caller other than `member`, a code path that is absolute, climbs out with `..` or is not `.js`/`.mjs`, a host that is not a DNS name or is listed twice, a secret listed twice, a limit outside the deployment's ceilings |

use ordo_syntax::decl::{FunctionDecl, Program};

use crate::{Schema, SchemaError};

/// The ceilings a function's `limit` may reach by default (§9). A
/// deployment's own ceilings are applied again when it runs.
pub const MAX_WALL_MS: u64 = 15 * 60 * 1000;
pub const MAX_CPU_MS: u64 = 60 * 1000;
pub const MAX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
/// Below this the engine itself does not fit.
pub const MIN_MEMORY_BYTES: u64 = 8 * 1024 * 1024;

fn refuse(span: ordo_syntax::Span, message: String) -> SchemaError {
    SchemaError {
        code: "invalid_function",
        message,
        span,
    }
}

/// Check every function in `program`.
pub fn check(schema: &Schema, program: &Program) -> Result<(), SchemaError> {
    let mut seen: Vec<&str> = Vec::new();
    for f in &program.functions {
        let name = f.name.name.as_str();
        if seen.contains(&name) {
            return Err(refuse(
                f.name.span,
                format!("function `{name}` is declared twice"),
            ));
        }
        seen.push(name);
        if program.ops.iter().any(|o| o.name.name == name) {
            return Err(refuse(
                f.name.span,
                format!("`{name}` names both an op and a function; give the function its own name"),
            ));
        }
        if program.routines.iter().any(|r| r.name.name == name) {
            return Err(refuse(
                f.name.span,
                format!("`{name}` names both a routine and a function; give one its own name"),
            ));
        }
        if let (Some(d), Some(prog)) = (&f.domain, &schema.domain) {
            if &d.name != prog {
                return Err(refuse(
                    d.span,
                    format!(
                        "function `{name}` says `in {}`, and this program is domain `{prog}`",
                        d.name
                    ),
                ));
            }
        }
        check_call(f)?;
        check_code(f)?;
        check_egress(f)?;
        check_secrets(schema, f)?;
        check_limits(f)?;
    }
    Ok(())
}

fn check_call(f: &FunctionDecl) -> Result<(), SchemaError> {
    if f.returns.name != "json" {
        return Err(refuse(
            f.returns.span,
            format!(
                "function `{}` answers `{}`; a function answers `json` (a stream is a later \
                 increment)",
                f.name.name, f.returns.name
            ),
        ));
    }
    match f.by.name.as_str() {
        "member" => {}
        "public" => {
            return Err(refuse(
                f.by.span,
                format!(
                    "function `{}` is `by public`: a public function runs with the visitor's \
                     public grants and is not built yet; use `by member`",
                    f.name.name
                ),
            ))
        }
        other => {
            return Err(refuse(
                f.by.span,
                format!(
                    "function `{}` is `by {other}`; increment 1 builds `by member`: a signed-in \
                     member of the tenant",
                    f.name.name
                ),
            ))
        }
    }
    let mut params: Vec<&str> = Vec::new();
    for p in &f.params {
        if params.contains(&p.name.name.as_str()) {
            return Err(refuse(
                p.name.span,
                format!("`${}` is declared twice", p.name.name),
            ));
        }
        params.push(&p.name.name);
    }
    Ok(())
}

/// The code path is relative to the program's root and stays inside it.
fn check_code(f: &FunctionDecl) -> Result<(), SchemaError> {
    let path = f.code.as_str();
    let bad = |why: &str| {
        Err(refuse(
            f.code_span,
            format!("function `{}`: `code \"{path}\"` {why}", f.name.name),
        ))
    };
    if path.is_empty() {
        return bad("is empty");
    }
    if path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
        return bad("must be relative to the program's directory");
    }
    if path.split(['/', '\\']).any(|s| s == "..") {
        return bad("must stay inside the program's directory (no `..`)");
    }
    if !(path.ends_with(".js") || path.ends_with(".mjs")) {
        return bad("must name a bundled JavaScript module, `.js` or `.mjs`");
    }
    Ok(())
}

fn check_egress(f: &FunctionDecl) -> Result<(), SchemaError> {
    let mut seen: Vec<&str> = Vec::new();
    for h in &f.egress {
        if !is_dns_name(&h.name) {
            return Err(refuse(
                h.span,
                format!(
                    "`{}` is not a host name: write a DNS name such as `api.openai.com` \
                     (an address or a scheme is refused)",
                    h.name
                ),
            ));
        }
        if seen.contains(&h.name.as_str()) {
            return Err(refuse(h.span, format!("`{}` is listed twice", h.name)));
        }
        seen.push(&h.name);
    }
    Ok(())
}

/// Two or more labels of letters, digits and inner hyphens, and a last
/// label that is not all digits (so an IPv4 address is refused).
pub fn is_dns_name(h: &str) -> bool {
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 || h.len() > 253 {
        return false;
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    labels.iter().all(|l| label_ok(l))
        && !labels
            .last()
            .is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()))
}

fn check_secrets(schema: &Schema, f: &FunctionDecl) -> Result<(), SchemaError> {
    let mut seen: Vec<&str> = Vec::new();
    for s in &f.secrets {
        if seen.contains(&s.name.as_str()) {
            return Err(refuse(
                s.span,
                format!("secret `{}` is listed twice", s.name),
            ));
        }
        seen.push(&s.name);
    }
    // A signing key (`secret hookKey`, no host) is used only as an HMAC key
    // in the host. A secret with hosts is sent, so it needs one of them.
    let sent = |name: &str| {
        schema
            .secrets
            .iter()
            .any(|d| d.name.name == name && !d.hosts.is_empty())
    };
    if f.egress.is_empty() && f.secrets.iter().any(|s| sent(&s.name)) {
        return Err(refuse(
            f.secrets
                .iter()
                .find(|s| sent(&s.name))
                .map_or(f.secrets[0].span, |s| s.span),
            format!(
                "function `{}` names secrets bound to hosts and no `egress` host: such a \
                 secret is only ever sent to a host, so list the hosts it calls",
                f.name.name
            ),
        ));
    }
    for s in &f.secrets {
        let Some(decl) = schema.secrets.iter().find(|d| d.name.name == s.name) else {
            return Err(refuse(
                s.span,
                format!(
                    "function `{}` uses secret `{}`, and the program declares no such secret: \
                     add `secret {} for <host>`, or `secret {}` for a signing key",
                    f.name.name, s.name, s.name, s.name
                ),
            ));
        };
        // A secret goes only to its bound hosts. One the function cannot
        // reach under any of them could never be sent: a mistake in one of
        // the two lists.
        if !decl.hosts.is_empty() && !f.egress.iter().any(|h| decl.hosts.contains(&h.name)) {
            let egress: Vec<&str> = f.egress.iter().map(|h| h.name.as_str()).collect();
            return Err(refuse(
                s.span,
                format!(
                    "function `{}` uses secret `{}`, which is bound to {}, and its egress is {}: \
                     no host it may call receives the secret",
                    f.name.name,
                    s.name,
                    decl.hosts.join(", "),
                    egress.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

fn check_limits(f: &FunctionDecl) -> Result<(), SchemaError> {
    let Some(span) = f.limits.span else {
        return Ok(());
    };
    let over = |what: &str, max: String| {
        Err(refuse(
            span,
            format!(
                "function `{}`: the `{what}` limit is over the ceiling of {max}",
                f.name.name
            ),
        ))
    };
    if let Some(ms) = f.limits.wall_ms {
        if ms == 0 {
            return Err(refuse(span, "a `wall` limit of 0 lets nothing run".into()));
        }
        if ms > MAX_WALL_MS {
            return over("wall", "15m".into());
        }
    }
    if let Some(ms) = f.limits.cpu_ms {
        if ms == 0 {
            return Err(refuse(span, "a `cpu` limit of 0 lets nothing run".into()));
        }
        if ms > MAX_CPU_MS {
            return over("cpu", "60s".into());
        }
    }
    if let Some(b) = f.limits.memory_bytes {
        if b > MAX_MEMORY_BYTES {
            return over("memory", "512 mb".into());
        }
        if b < MIN_MEMORY_BYTES {
            return Err(refuse(
                span,
                format!(
                    "function `{}`: a `memory` limit under 8 mb leaves no room for the engine",
                    f.name.name
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_dns_name;

    fn check(src: &str) -> Result<(), String> {
        let p = ordo_syntax::parse_program(src).map_err(|e| e.message)?;
        crate::build(&p).map(|_| ()).map_err(|e| e.message)
    }

    const OK: &str = "domain notes\nentity notes { title: text }\n\
        secret llmKey for api.example.com\n\
        function suggestTitle in notes {\n  call($text: text) -> json\n  code \"fn/s.js\"\n  \
        egress api.example.com\n  secrets llmKey\n  limit wall 30s, memory 64 mb\n}\n";

    #[test]
    fn a_well_formed_function_is_accepted() {
        check(OK).unwrap();
    }

    #[test]
    fn a_signing_key_needs_no_egress() {
        let src = "domain notes\nentity notes { title: text }\n\
            secret hookKey\n\
            function checkHook in notes {\n  call($body: text) -> json\n  code \"fn/h.js\"\n  \
            secrets hookKey\n}\n";
        check(src).unwrap();
    }

    #[test]
    fn each_refusal_names_its_reason() {
        let cases = [
            (
                OK.replace("Title in notes", "Title in other"),
                "this program is domain",
            ),
            (OK.replace("-> json", "-> text"), "answers `json`"),
            (OK.replace("-> json", "-> json by public"), "not built yet"),
            (OK.replace("fn/s.js", "../s.js"), "no `..`"),
            (OK.replace("fn/s.js", "/abs/s.js"), "relative"),
            (OK.replace("fn/s.js", "fn/s.ts"), "`.js` or `.mjs`"),
            (
                OK.replace(
                    "egress api.example.com",
                    "egress api.example.com, api.example.com",
                ),
                "listed twice",
            ),
            (
                OK.replace("egress api.example.com", "egress \"10.0.0.1\""),
                "not a host name",
            ),
            (OK.replace("limit wall 30s", "limit wall 20m"), "ceiling"),
            (
                OK.replace("secrets llmKey", "secrets otherKey"),
                "declares no such secret",
            ),
            (
                OK.replace("egress api.example.com", "egress api.elsewhere.com"),
                "no host it may call receives the secret",
            ),
            (OK.replace("memory 64 mb", "memory 2 mb"), "under 8 mb"),
            (OK.replace("  egress api.example.com\n", ""), "no `egress`"),
            (
                format!("{OK}op suggestTitle = notes[title]\n"),
                "both an op and a function",
            ),
        ];
        for (src, want) in cases {
            let e = check(&src).expect_err(&src);
            assert!(e.contains(want), "{want:?} not in {e:?}\n{src}");
        }
    }

    #[test]
    fn host_names() {
        assert!(is_dns_name("api.openai.com"));
        assert!(is_dns_name("my-model.example.com"));
        assert!(!is_dns_name("localhost"));
        assert!(!is_dns_name("1.2.3.4"));
        assert!(!is_dns_name("-x.example.com"));
        assert!(!is_dns_name("a..b"));
    }
}
