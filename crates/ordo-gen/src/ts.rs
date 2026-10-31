//! TypeScript emission primitives, and the file model both backends share.
//!
//! Nothing here knows what Ordo is. It knows how to spell a string literal
//! that `tsc` will accept, how to turn an Ordo name into a legal TypeScript
//! identifier without losing which name it was, and what the header on a
//! generated file says.

use std::collections::BTreeSet;

/// One emitted file: a repository-relative path and its bytes.
///
/// A `Vec<GenFile>` is the whole output of a backend, which is what lets the
/// CLI write them and a test compare them against what is checked in without
/// either knowing anything about TypeScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenFile {
    pub path: String,
    pub contents: String,
}

/// The header every generated file carries.
///
/// It is not decoration. `tooling-and-embedding-plan.md`'s split between a
/// small hand-written runtime core and a generated surface is only real if a
/// reader opening one file can tell which half they are in — otherwise the
/// first person to fix a bug fixes it in the generated half and loses the fix
/// on the next `ordo gen ts`.
pub fn header(what: &str, from: &str) -> String {
    format!(
        "// GENERATED — do not edit; regenerate with `ordo gen ts`.\n\
         //\n\
         // {what}\n\
         //\n\
         // Source of truth: {from}\n\
         //\n\
         // This file is the GENERATED half of this package. The hand-written\n\
         // half is under `src/core/` and is versioned by hand; the boundary\n\
         // between them is documented in the package README. An edit here is\n\
         // lost the next time the generator runs.\n\n"
    )
}

/// A TypeScript string literal — single-quoted, matching the repository's
/// existing TS (`clients/ordo-react`), with the four characters that can end
/// a literal or a line escaped.
pub fn str_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// An object key: bare when it is a plain identifier, quoted otherwise.
///
/// Op names are dotted (`std.tasks.update`), so this is load-bearing rather
/// than cosmetic — a bare `std.tasks.update:` is a syntax error.
pub fn key(name: &str) -> String {
    if is_plain_ident(name) {
        name.to_string()
    } else {
        str_lit(name)
    }
}

/// A property access: `.name` when it can be, `['name']` when it cannot.
pub fn access(name: &str) -> String {
    if is_plain_ident(name) {
        format!(".{name}")
    } else {
        format!("[{}]", str_lit(name))
    }
}

/// Shape only — a reserved word is a perfectly good *property* name, and
/// `.in(…)` and `.delete` both depend on that. What a reserved word cannot be
/// is a binding, which [`binding`] handles separately.
fn is_plain_ident(name: &str) -> bool {
    let mut cs = name.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The TypeScript/JavaScript reserved words that cannot be a bare binding.
///
/// Deliberately the *binding* list and not the property list: a property may
/// be called `in` or `delete`, and the CDK's `.in(…)` depends on that. What
/// this list protects is a generated `const <name> = …`, where an Ordo entity
/// called `class` or an op called `delete` would otherwise emit a file that
/// does not parse.
const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    // Contextual, but a top-level `const let` / `const await` is still an
    // error and a generated file must never be the place that is discovered.
    "await",
    "let",
    "static",
    "yield",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
];

fn is_reserved_word(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// A legal TypeScript binding name for an Ordo name.
///
/// Dots become `$` (op names are dotted; `std.tasks.update` becomes
/// `std$tasks$update`) and a reserved word gains a trailing `_`. Neither is
/// lossy in the direction that matters: the Ordo name always rides beside the
/// binding as a string literal, and the wire only ever sees that string.
pub fn binding(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                c
            } else {
                '$'
            }
        })
        .collect();
    if out
        .chars()
        .next()
        .is_none_or(|c| c.is_ascii_digit() || c == '$')
    {
        out.insert(0, '_');
    }
    if is_reserved_word(&out) {
        out.push('_');
    }
    out
}

/// `vendorSpend` → `VendorSpend`, for a generated type name.
pub fn pascal(name: &str) -> String {
    let cleaned = binding(name);
    let mut out = String::with_capacity(cleaned.len());
    let mut upper_next = true;
    for c in cleaned.chars() {
        if c == '$' || c == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    if out
        .chars()
        .next()
        .is_none_or(|c| !c.is_ascii_alphabetic() && c != '_')
    {
        out.insert(0, '_');
    }
    out
}

/// A `/** … */` block, wrapped, with `*/` neutralised so a doc string
/// carrying one cannot end the comment early.
pub fn doc(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("/**\n");
    for line in lines {
        for physical in line.replace("*/", "*\u{200b}/").split('\n') {
            if physical.is_empty() {
                out.push_str(" *\n");
            } else {
                out.push_str(" * ");
                out.push_str(physical);
                out.push('\n');
            }
        }
    }
    out.push_str(" */\n");
    out
}

/// A TypeScript union of string literals, or `never` for an empty one.
///
/// `never` is the honest empty: a union of nothing is a type nothing
/// inhabits, and an op that declares no refusals really cannot be refused in
/// a way its own descriptor names.
pub fn string_union(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        return "never".to_string();
    }
    values
        .iter()
        .map(|v| str_lit(v))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_op_names_survive_both_spellings() {
        assert_eq!(binding("std.tasks.update"), "std$tasks$update");
        assert_eq!(key("std.tasks.update"), "'std.tasks.update'");
        assert_eq!(access("std.tasks.update"), "['std.tasks.update']");
        assert_eq!(pascal("std.tasks.update"), "StdTasksUpdate");
    }

    #[test]
    fn a_reserved_word_cannot_become_a_binding() {
        assert_eq!(binding("delete"), "delete_");
        assert_eq!(binding("in"), "in_");
        // …but it stays a legal property, which `.in(…)` needs.
        assert_eq!(access("in"), ".in");
    }

    #[test]
    fn string_literals_cannot_escape_their_quotes() {
        assert_eq!(str_lit("it's"), "'it\\'s'");
        assert_eq!(str_lit("a\\b"), "'a\\\\b'");
        assert_eq!(str_lit("a\nb"), "'a\\nb'");
    }

    #[test]
    fn a_doc_comment_cannot_end_early() {
        let d = doc(&["ends here */ and then code".to_string()]);
        assert!(!d.contains("*/ and then"));
        assert!(d.ends_with(" */\n"));
    }
}
