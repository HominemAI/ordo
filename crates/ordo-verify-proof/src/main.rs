//! `ordo-verify-proof` — the published offline verifier.
//!
//! ```text
//! ordo-verify-proof proof       [--file F | -] [--key HEX --key-id ID] [--head HEX]
//! ordo-verify-proof attestation [--file F | -] [--key HEX --key-id ID]
//! ordo-verify-proof consent     --space S --url U
//! ```
//!
//! Exit codes, and they are `ordo verify`'s so an operator's cron reads one
//! convention:
//!
//! | code | meaning |
//! |---|---|
//! | 0 | every check passed |
//! | 1 | the input could not be read or parsed |
//! | 2 | usage |
//! | 3 | **verification failed** |
//!
//! 3 is separate from 1 on purpose. "This chain does not link at index 41"
//! and "I could not open the file" must page different people.

use std::io::Read;

use ordo_commandlog::CheckpointVerifier;
use ordo_verify_proof::{proof, verify_attestation, verify_proof};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    let Some(mode) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let flags = match parse(&args[1..]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return 2;
        }
    };

    match mode {
        "consent" => {
            let (Some(space), Some(url)) = (flags.get("--space"), flags.get("--url")) else {
                eprintln!("`consent` needs --space and --url\n\n{USAGE}");
                return 2;
            };
            // The URL must be the **canonical** form — lowercase host, the
            // default port omitted, a path that always begins with `/`. This
            // subcommand does not canonicalise it, and that is deliberate:
            // guessing at a URL the owner is about to commit to would be the
            // one place a helpful tool could silently produce a signature over
            // bytes the platform will not reproduce.
            println!("{}", proof::consent_hex(space, url));
            println!();
            eprintln!(
                "Sign those bytes with your Ed25519 key -- they are the raw message, not a \
                 digest -- and pass the signature to `std.space.attestTo` together with the \
                 same URL. The URL must be the canonical form: lowercase host, `:443` \
                 omitted, path present and beginning with `/`."
            );
            0
        }
        "proof" | "attestation" => {
            let doc = match read_input(&flags) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let trusted = match trusted_key(&flags) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{e}");
                    return 2;
                }
            };
            if mode == "proof" {
                match verify_proof(
                    &doc,
                    trusted.as_ref(),
                    flags.get("--head").map(String::as_str),
                ) {
                    Ok(v) => {
                        println!("{}", v.summary());
                        for f in &v.findings {
                            println!("  [{}] {}", f.step, f.detail);
                        }
                        println!(
                            "\n  verifiable from index {} (unsigned; a feasibility hint, \
                                  not a claim)",
                            v.verifiable_from
                        );
                        for c in v.caveats() {
                            println!("\n  NOTE: {c}");
                        }
                        v.code()
                    }
                    Err(e) => {
                        eprintln!("could not read the proof: {e}");
                        1
                    }
                }
            } else {
                match verify_attestation(&doc, trusted.as_ref()) {
                    Ok(v) => {
                        println!("{}", v.summary());
                        for f in &v.findings {
                            println!("  [{}] {}", f.step, f.detail);
                        }
                        println!(
                            "\n  destination key `{}` -- compare with what you recorded on the \
                             first delivery",
                            v.destination_key_id
                        );
                        println!(
                            "  cadence: every {} commands or {}s, whichever lands first; the \
                             next delivery is due by unix {}. Silence past that is the signal.",
                            v.cadence_every,
                            v.cadence_or_secs,
                            v.next_due_by()
                        );
                        v.code()
                    }
                    Err(e) => {
                        eprintln!("could not read the attestation: {e}");
                        1
                    }
                }
            }
        }
        other => {
            eprintln!("unknown mode `{other}`\n\n{USAGE}");
            2
        }
    }
}

type Flags = std::collections::BTreeMap<String, String>;

/// Strict: an unknown flag is a usage error, never something ignored. A
/// typo'd `--ky` that silently verified structure-only, and printed
/// "UNVERIFIED" in a line the operator skimmed, is the worst failure this
/// tool has available to it.
fn parse(args: &[String]) -> Result<Flags, String> {
    const KNOWN: &[&str] = &["--file", "--key", "--key-id", "--head", "--space", "--url"];
    let mut out = Flags::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-" {
            out.insert("--file".into(), "-".into());
            continue;
        }
        if !KNOWN.contains(&a.as_str()) {
            return Err(format!("unknown flag `{a}`"));
        }
        let v = it.next().ok_or_else(|| format!("`{a}` needs a value"))?;
        out.insert(a.clone(), v.clone());
    }
    Ok(out)
}

fn read_input(flags: &Flags) -> Result<String, String> {
    match flags.get("--file").map(String::as_str) {
        None | Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| format!("stdin: {e}"))?;
            Ok(s)
        }
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}")),
    }
}

fn trusted_key(flags: &Flags) -> Result<Option<CheckpointVerifier>, String> {
    match (flags.get("--key"), flags.get("--key-id")) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(
            "--key and --key-id go together. A key with no id cannot refuse a record signed \
             by a *different* key, which is half of what checking a signature is for."
                .into(),
        ),
        (Some(hex), Some(id)) => CheckpointVerifier::from_public_hex(id, hex)
            .map(Some)
            .map_err(|e| format!("--key is not an Ed25519 public key: {e:?}")),
    }
}

const USAGE: &str = "\
ordo-verify-proof -- verify an Ordo chain proof or attestation, offline.

  ordo-verify-proof proof       [--file F | -] [--key HEX --key-id ID] [--head HEX]
  ordo-verify-proof attestation [--file F | -] [--key HEX --key-id ID]
  ordo-verify-proof consent     --space S --url U

  --file    the document; `-` or omitted reads stdin
  --key     the Ed25519 public key you trust, 64 hex characters
  --key-id  its id, which must match the record's
  --head    a head you pinned earlier; supplying it is what turns a
            consistency check into a rewrite check

This binary has no network, no database driver and no configuration. It
reads what you give it and does arithmetic. Without --key it checks
structure only and says so.";
