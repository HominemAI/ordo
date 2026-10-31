//! The editor grammar and the lexer must agree about what a word is.
//!
//! `editors/ordo.tmLanguage.json` is a second statement of the language's
//! closed vocabularies — one that ships to VS Code, to JetBrains' TextMate
//! importer, and to the docs site's shiki renderer. A second statement that
//! nothing checks is a second answer waiting to differ: a keyword added to
//! the lexer would go uncoloured (an author never learns the word is
//! reserved), and a word coloured as reserved that the lexer treats as an
//! ordinary name would teach the opposite of the truth — that `count` cannot
//! be a field name, when the two-tier design exists precisely so it can.
//!
//! So the grammar carries a manifest (`ordoInventory`) and this file checks
//! it in **both** directions, in the same spirit as `docs_parse.rs`: nothing
//! in the lexer is missing from the grammar, and nothing in the grammar is
//! absent from the lexer. Three further checks stop the manifest from
//! becoming a fiction of its own:
//!
//! - every manifest word must actually appear inside a capturing group of
//!   some pattern (a manifest that lists a word the grammar never matches is
//!   a lie the first test could not catch);
//! - every word a pattern colours as vocabulary must be in the manifest (an
//!   unlisted word could otherwise be coloured as a keyword forever);
//! - the four closed sets the grammar borrows from elsewhere — event kinds,
//!   expression functions, aggregates, allocation modes, duration units —
//!   are pinned against their producers, not retyped.

use std::collections::{BTreeMap, BTreeSet};

use ordo_syntax::token::{DurationUnit, Keyword};
use ordo_syntax::{function_names, EventKind, ScalarTy, AGGREGATE_NAMES, ALLOCATE_MODES};
use serde_json::Value;

const GRAMMAR_SRC: &str = include_str!("../../../editors/ordo.tmLanguage.json");

fn grammar() -> Value {
    serde_json::from_str(GRAMMAR_SRC).expect("editors/ordo.tmLanguage.json is not valid JSON")
}

/// A family map (`scope name -> words`) out of one half of the manifest.
fn families(g: &Value, half: &str) -> BTreeMap<String, Vec<String>> {
    g["ordoInventory"][half]
        .as_object()
        .unwrap_or_else(|| panic!("ordoInventory.{half} is missing or not an object"))
        .iter()
        .map(|(scope, words)| {
            let words = words
                .as_array()
                .unwrap_or_else(|| panic!("ordoInventory.{half}.{scope} is not an array"))
                .iter()
                .map(|w| w.as_str().expect("a word is a string").to_string())
                .collect();
            (scope.clone(), words)
        })
        .collect()
}

fn flatten(fams: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    fams.values().flatten().cloned().collect()
}

/// A word that looks like an Ordo identifier — the shape a vocabulary entry
/// has. Used to tell `(read|write|admin)` (words) from `([a-z][a-zA-Z0-9]*)`
/// (a character class) without parsing the regex.
fn is_word(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase()) && cs.all(|c| c.is_ascii_alphanumeric())
}

/// Every capturing group's contents in a regex, ignoring `(?:…)`, `(?=…)`
/// and friends. Bracket-depth aware so `[(]` inside a character class is not
/// mistaken for a group.
fn capture_groups(re: &str) -> Vec<String> {
    let bytes: Vec<char> = re.chars().collect();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut in_class = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            '\\' => i += 1,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => {
                let capturing = bytes.get(i + 1) != Some(&'?');
                stack.push(if capturing { i + 1 } else { usize::MAX });
            }
            ')' if !in_class => {
                if let Some(start) = stack.pop() {
                    if start != usize::MAX {
                        out.push(bytes[start..i].iter().collect());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The scope-name prefixes that mean "this is vocabulary, not punctuation".
/// A word coloured under one of these is making a claim about the language's
/// closed sets, so it owes the manifest an entry.
const VOCABULARY_PREFIXES: &[&str] = &[
    "keyword.",
    "storage.modifier.",
    "constant.language.",
    "support.type.",
    "support.function.",
    "support.constant.",
];

/// Walk every rule object in the document, yielding
/// `(all scope names it assigns, all regexes it uses)`.
fn rules(value: &Value, out: &mut Vec<(Vec<String>, Vec<String>)>) {
    match value {
        Value::Object(map) => {
            let mut scopes = Vec::new();
            let mut regexes = Vec::new();
            if let Some(Value::String(n)) = map.get("name") {
                scopes.push(n.clone());
            }
            for key in ["captures", "beginCaptures", "endCaptures"] {
                if let Some(Value::Object(caps)) = map.get(key) {
                    for cap in caps.values() {
                        if let Some(Value::String(n)) = cap.get("name") {
                            scopes.push(n.clone());
                        }
                    }
                }
            }
            for key in ["match", "begin", "end"] {
                if let Some(Value::String(r)) = map.get(key) {
                    regexes.push(r.clone());
                }
            }
            if !scopes.is_empty() && !regexes.is_empty() {
                out.push((scopes, regexes));
            }
            for v in map.values() {
                rules(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| rules(v, out)),
        _ => {}
    }
}

fn diff(label: &str, a: &BTreeSet<String>, b: &BTreeSet<String>) -> String {
    let only: Vec<&str> = a.difference(b).map(String::as_str).collect();
    if only.is_empty() {
        String::new()
    } else {
        format!("\n{label}: {}", only.join(" "))
    }
}

/// The headline: the grammar's reserved list IS `Keyword::ALL`, both ways.
#[test]
fn reserved_inventory_equals_the_lexer() {
    let g = grammar();
    let reserved = families(&g, "reserved");
    let listed = flatten(&reserved);
    let lexed: BTreeSet<String> = Keyword::ALL
        .iter()
        .map(|k| k.as_str().to_string())
        .collect();

    assert_eq!(
        listed,
        lexed,
        "the editor grammar and the lexer disagree about the reserved words.{}{}",
        diff(
            "in editors/ordo.tmLanguage.json and NOT a Keyword (remove it, or add \
             the keyword to token.rs)",
            &listed,
            &lexed
        ),
        diff(
            "a Keyword and NOT in editors/ordo.tmLanguage.json (an author who \
             writes it will see it uncoloured and never learn it is reserved)",
            &lexed,
            &listed
        ),
    );
}

/// One word, one semantic family. The families exist so a theme can colour
/// declarations differently from word-operators; a word in two of them would
/// have two colours and the reader would learn nothing from either.
#[test]
fn each_reserved_word_has_exactly_one_family() {
    let g = grammar();
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (scope, words) in families(&g, "reserved") {
        for w in words {
            seen.entry(w).or_default().push(scope.clone());
        }
    }
    let doubled: Vec<String> = seen
        .iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(w, s)| format!("`{w}` in {}", s.join(" and ")))
        .collect();
    assert!(
        doubled.is_empty(),
        "a reserved word belongs to one family: {}",
        doubled.join("; ")
    );
}

/// The other half of the two-tier discipline. `read write admin`, `open`,
/// `count sum avg`, `run` and the rest stay `Ident` in the lexer so they
/// remain usable as field names; the grammar colours them by POSITION only.
/// If one of them ever became reserved, colouring it contextually would be
/// wrong in the other direction — so the manifest names the intersection
/// (`dual`) explicitly rather than letting it grow silently.
#[test]
fn contextual_words_are_not_keywords_unless_declared_dual() {
    let g = grammar();
    let contextual = flatten(&families(&g, "contextual"));
    let dual: BTreeSet<String> = g["ordoInventory"]["dual"]
        .as_array()
        .expect("ordoInventory.dual is an array")
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();

    let reserved = flatten(&families(&g, "reserved"));
    let actual_intersection: BTreeSet<String> =
        contextual.intersection(&reserved).cloned().collect();

    assert_eq!(
        actual_intersection, dual,
        "`dual` must name exactly the words that are both reserved and \
         contextual. `distinct` is one (a pipeline step and an aggregate, \
         which declparse::agg_decl handles by hand); anything else appearing \
         here is a word whose tier changed and whose colouring needs a decision."
    );

    for w in &contextual {
        if dual.contains(w) {
            continue;
        }
        assert!(
            Keyword::parse(w).is_none(),
            "`{w}` is listed as contextual but the lexer reserves it — the \
             grammar would teach an author it can be a field name when it cannot"
        );
    }
    for w in &dual {
        assert!(
            Keyword::parse(w).is_some(),
            "`{w}` is declared dual but is not a keyword at all"
        );
    }
}

/// A manifest that lists words the grammar never matches would pass every
/// test above while colouring nothing. So every listed word must appear
/// inside a capturing group somewhere in the file.
#[test]
fn every_manifest_word_is_matched_by_a_pattern() {
    let g = grammar();
    let mut all_rules = Vec::new();
    rules(&g["patterns"], &mut all_rules);
    rules(&g["repository"], &mut all_rules);

    let matched: BTreeSet<String> = all_rules
        .iter()
        .flat_map(|(_, regexes)| regexes)
        .flat_map(|re| capture_groups(re))
        .flat_map(|group| {
            group
                .split('|')
                .filter(|w| is_word(w) || w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();

    let mut listed = flatten(&families(&g, "reserved"));
    listed.extend(flatten(&families(&g, "contextual")));

    let unmatched: Vec<&str> = listed.difference(&matched).map(String::as_str).collect();
    assert!(
        unmatched.is_empty(),
        "these words are in ordoInventory but no capturing group matches them, \
         so the grammar colours nothing for them: {}\n\
         (remember: vocabulary is written `\\\\b(word)\\\\b`, inside a group)",
        unmatched.join(" ")
    );
}

/// And the reverse: a pattern that colours a word as vocabulary is making a
/// claim about a closed set, so the word owes the manifest an entry — which
/// is what drags it in front of the two tests above.
#[test]
fn every_vocabulary_word_a_pattern_colours_is_in_the_manifest() {
    let g = grammar();
    let mut all_rules = Vec::new();
    rules(&g["patterns"], &mut all_rules);
    rules(&g["repository"], &mut all_rules);

    let mut listed = flatten(&families(&g, "reserved"));
    listed.extend(flatten(&families(&g, "contextual")));

    let mut stray: BTreeSet<String> = BTreeSet::new();
    for (scopes, regexes) in &all_rules {
        let vocabulary = scopes.iter().any(|s| {
            VOCABULARY_PREFIXES
                .iter()
                .any(|p| s.starts_with(p) && s.ends_with(".ordo"))
        });
        if !vocabulary {
            continue;
        }
        for re in regexes {
            for group in capture_groups(re) {
                let alternatives: Vec<&str> = group.split('|').collect();
                if alternatives.iter().all(|a| is_word(a)) {
                    for a in alternatives {
                        if !listed.contains(a) {
                            stray.insert(a.to_string());
                        }
                    }
                }
            }
        }
    }
    assert!(
        stray.is_empty(),
        "these words are coloured as vocabulary but are not in ordoInventory, \
         so nothing checks them against the compiler: {}",
        stray.iter().cloned().collect::<Vec<_>>().join(" ")
    );
}

/// The five closed sets the grammar borrows. Each is owned by a producer in
/// this crate; the grammar restates it for Oniguruma's benefit, and the
/// restatement is checked rather than trusted.
#[test]
fn borrowed_closed_sets_match_their_producers() {
    let g = grammar();
    let contextual = families(&g, "contextual");
    let get = |scope: &str| -> BTreeSet<String> {
        contextual
            .get(scope)
            .unwrap_or_else(|| panic!("ordoInventory.contextual.{scope} is missing"))
            .iter()
            .cloned()
            .collect()
    };
    let from = |words: &mut dyn Iterator<Item = &str>| -> BTreeSet<String> {
        words.map(str::to_string).collect()
    };

    assert_eq!(
        get("constant.language.event.ordo"),
        from(&mut EventKind::ALL.iter().map(|k| k.as_str())),
        "the trigger vocabulary is seven words and no eighth (trigger.rs); the \
         grammar marks anything else `invalid.illegal.event-kind`, which is only \
         true while this list is"
    );
    assert_eq!(
        get("support.function.ordo"),
        from(&mut function_names()),
        "the expression-function set is closed (parser.rs FUNCTIONS)"
    );
    assert_eq!(
        get("support.function.aggregate.ordo"),
        from(&mut AGGREGATE_NAMES.iter().copied()),
        "the aggregate set is closed (declparse.rs AGGREGATE_NAMES)"
    );
    assert_eq!(
        get("support.constant.allocate-mode.ordo"),
        from(&mut ALLOCATE_MODES.iter().copied()),
        "the allocation modes are closed and an unknown one is a compile error, \
         never a silent default (parser.rs ALLOCATE_MODES)"
    );
    assert_eq!(
        get("support.type.unit.ordo"),
        from(&mut DurationUnit::ALL.iter().map(|u| u.as_str())),
        "duration units are `ms s m h d w mo y` and nothing else (token.rs)"
    );
    // **The inventory this test did not check, and the gap that hid.**
    //
    // Five closed sets above are held to their producers, and the type set
    // was not — because `ScalarTy` had no `ALL` to hold it to, and the
    // reason recorded for that (`Duration(DurationUnit)` is not a unit
    // variant, so `word_enum!` cannot generate one) was a reason a MACRO
    // could not, mistaken for a reason the enum could not.
    //
    // So a type added to the language and forgotten in the grammar stayed
    // uncoloured, and every editor user learned an inventory one word
    // shorter than the real one, with nothing failing. `ScalarTy::ALL` and
    // `::DECLARABLE` exist now and this is the assertion they were added
    // for.
    //
    // Since 27ak `decimal(p)` is declarable, so the grammar's list and the
    // declarable set are the same set.
    let grammar_types = get("support.type.ordo");
    let expected = from(&mut ScalarTy::DECLARABLE.iter().map(|t| t.as_str()));
    assert_eq!(
        grammar_types, expected,
        "the grammar's type list must be `ScalarTy::DECLARABLE` — a type added to \
         the language and missing here is a type every editor user is told does \
         not exist"
    );
}

/// The parser's closed-set rejection names the same words the grammar
/// colours, because both are derived from `ScalarTy::DECLARABLE`.
///
/// This message had already drifted once: it named eight types while the
/// language admitted nine, because `datetime` was added and the sentence
/// was not. A hand-written list inside an error string is the one kind of
/// inventory nothing tests, so it is derived now and this holds the
/// derivation to the grammar.
#[test]
fn the_closed_set_message_names_every_declarable_type() {
    let words = ordo_syntax::declparse::declarable_type_words();
    for t in ScalarTy::DECLARABLE {
        assert!(
            words.split(", ").any(|w| w == t.as_str()),
            "the closed-set rejection does not name `{}`: {words}",
            t.as_str()
        );
    }
    // And the two halves of the inventory agree, so a variant added to
    // `ALL` and forgotten in `DECLARABLE` is caught here rather than by an
    // author who cannot spell their own type. Every member is declarable
    // since `decimal(p)` became storable (27ak).
    assert_eq!(
        ScalarTy::ALL.len(),
        ScalarTy::DECLARABLE.len(),
        "every member of `ALL` has a declaration form"
    );
}

/// `ScalarTy::ALL` is total over the enum.
///
/// The list is written out rather than derived, because `Duration` carries
/// a unit and there is no `Self::VARIANTS`. This exhaustive match is what
/// keeps it honest: a new variant fails to compile here, and the author has
/// to say whether it belongs in `ALL`, in `DECLARABLE`, or in neither.
#[test]
fn the_scalar_inventory_is_total_over_the_enum() {
    for t in ScalarTy::ALL {
        // Every member reaches a spelling, and no two share one.
        let listed = ScalarTy::ALL
            .iter()
            .filter(|x| x.as_str() == t.as_str())
            .count();
        assert_eq!(listed, 1, "`{}` appears twice in ALL", t.as_str());
    }
    // The exhaustive half. Adding a variant breaks this match, which is the
    // whole mechanism.
    let in_all = |t: ScalarTy| -> bool { ScalarTy::ALL.iter().any(|x| x.as_str() == t.as_str()) };
    let every: bool = match ScalarTy::Text {
        ScalarTy::Text
        | ScalarTy::Int
        | ScalarTy::Money(_)
        | ScalarTy::Decimal(_)
        | ScalarTy::Bool
        | ScalarTy::Date
        | ScalarTy::Datetime
        | ScalarTy::Duration(_)
        | ScalarTy::Id
        | ScalarTy::Blob
        | ScalarTy::Uri
        | ScalarTy::Json => [
            ScalarTy::Text,
            ScalarTy::Int,
            ScalarTy::Money(2),
            ScalarTy::Decimal(4),
            ScalarTy::Bool,
            ScalarTy::Date,
            ScalarTy::Datetime,
            ScalarTy::Duration(DurationUnit::Ms),
            ScalarTy::Id,
            ScalarTy::Blob,
            ScalarTy::Uri,
            ScalarTy::Json,
        ]
        .into_iter()
        .all(in_all),
    };
    assert!(every, "a ScalarTy variant is missing from ALL");
    assert_eq!(ScalarTy::ALL.len(), 12);
}

/// The grammar says which language version it tracks. Without this the
/// artifact that ships to four hosts has no identity, and "which release of
/// Ordo does my editor understand?" has no answer.
#[test]
fn the_grammar_names_the_language_version_it_tracks() {
    let g = grammar();
    assert_eq!(
        g["ordoInventory"]["languageVersion"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "editors/ordo.tmLanguage.json tracks a different version of the language \
         than the crate that defines it"
    );
}

/// Sanity on the surface every host reads: the identifiers a TextMate host
/// keys off must be present and stable.
#[test]
fn the_grammar_declares_the_identifiers_hosts_key_off() {
    let g = grammar();
    assert_eq!(g["scopeName"].as_str(), Some("source.ordo"));
    assert_eq!(g["name"].as_str(), Some("Ordo"));
    assert_eq!(
        g["fileTypes"].as_array().map(Vec::as_slice),
        Some([Value::String("ordo".into())].as_slice())
    );
    assert!(
        g["repository"].as_object().is_some_and(|r| !r.is_empty()),
        "the repository is where every rule lives"
    );
}
