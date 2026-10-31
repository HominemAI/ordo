//! G12's guard (`plan-console.md` 7, step 9): **every console write through
//! the door leaves a record.**
//!
//! It lists every door op the console calls, from the console's own
//! source, and holds each to one of two rules:
//!
//! - a **read** is a read in `ordo-host`'s own spec (`OpKind::Read`);
//! - a **write** names the act it records, the `ordo-host` source that
//!   records it, and the `ordo-host` test that drives the op and asserts the
//!   record. The act must appear in both.
//!
//! A new op the console calls fails here until it is placed in [`CALLS`].
//!
//! Settings changes are runtime-side (the supervisor's revisions and the
//! console's audit log), not door ops, so they are not listed here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ordo_api::OpKind;

/// How a door op the console calls is held to the rule.
enum Rule {
    Read,
    /// The act word the record carries, the `ordo-host/src` file that
    /// records it and the `ordo-host/tests` file that asserts it.
    Write {
        act: &'static str,
        src: &'static str,
        test: &'static str,
    },
}

use Rule::{Read, Write};

/// Every door op the console calls.
const CALLS: &[(&str, Rule)] = &[
    // Console key (step 3).
    ("std.deployment.consoleKeys", Read),
    (
        "std.deployment.addConsoleKey",
        Write {
            act: "added",
            src: "console_keys.rs",
            test: "console_key.rs",
        },
    ),
    (
        "std.deployment.revokeConsoleKey",
        Write {
            act: "revoked",
            src: "console_keys.rs",
            test: "console_key.rs",
        },
    ),
    // Deploys (step 4).
    ("std.deployment.versions", Read),
    // A plan stores nothing: it is declared a write so it asks for a touch,
    // and it has no act to record.
    ("std.deployment.plan", Read),
    (
        "std.deployment.apply",
        Write {
            act: "applied",
            src: "apply_op.rs",
            test: "deploy_apply.rs",
        },
    ),
    (
        "std.deployment.activate",
        Write {
            act: "activated",
            src: "deploy_ops.rs",
            test: "deploy_apply.rs",
        },
    ),
    // Tenants (step 5).
    ("std.deployment.tenants", Read),
    (
        "std.space.create",
        Write {
            act: "tenant-created",
            src: "tenant_ops.rs",
            test: "deployment_tenants.rs",
        },
    ),
    (
        "std.space.reap",
        Write {
            act: "space_reaped",
            src: "reap_op.rs",
            test: "deployment_tenants.rs",
        },
    ),
    (
        "std.deployment.pauseTenant",
        Write {
            act: "tenant-paused",
            src: "pause.rs",
            test: "deployment_tenants.rs",
        },
    ),
    (
        "std.deployment.resumeTenant",
        Write {
            act: "tenant-resumed",
            src: "pause.rs",
            test: "deployment_tenants.rs",
        },
    ),
    (
        "std.space.export",
        Write {
            act: "space_exported",
            src: "exit_ops.rs",
            test: "exit_ops.rs",
        },
    ),
    (
        "std.space.delete",
        Write {
            act: "space_erased",
            src: "exit_ops.rs",
            test: "exit_ops.rs",
        },
    ),
    (
        "std.space.attachBucket",
        Write {
            act: "bucket_attached",
            src: "bucket_ops.rs",
            test: "attach_bucket.rs",
        },
    ),
    (
        "std.space.detachBucket",
        Write {
            act: "bucket_detached",
            src: "bucket_ops.rs",
            test: "attach_bucket.rs",
        },
    ),
    // Settings (step 6): a read; changes go through the supervisor.
    ("std.deployment.settings", Read),
    // Usage (step 7).
    ("std.deployment.quotas", Read),
    ("std.deployment.observedCost", Read),
    (
        "std.deployment.setQuota",
        Write {
            act: "quota-set",
            src: "quotas.rs",
            test: "deployment_quotas.rs",
        },
    ),
    // Shares, links, security log (step 9).
    ("std.space.links", Read),
    ("std.space.shares", Read),
    ("std.deployment.securityEvents", Read),
    (
        "std.space.revokeLink",
        Write {
            act: "link-revoked",
            src: "share_links.rs",
            test: "share_links.rs",
        },
    ),
    (
        "std.space.revokeShare",
        Write {
            act: "share-revoked",
            src: "share_ops.rs",
            test: "sharing.rs",
        },
    ),
    // People (step 8).
    ("std.space.members", Read),
    ("std.space.grants", Read),
    ("std.space.groupMembers", Read),
    ("std.space.callableBy", Read),
    ("std.space.groups", Read),
    ("std.space.callersOf", Read),
    (
        "std.space.invite",
        Write {
            act: "invitation-issued",
            src: "invite_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.setRole",
        Write {
            act: "member-role-changed",
            src: "member_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.remove",
        Write {
            act: "member-removed",
            src: "member_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.addToGroup",
        Write {
            act: "group-member-added",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    // People, the rest (step 8b).
    ("std.space.invitations", Read),
    ("std.space.profile", Read),
    ("std.space.admission", Read),
    (
        "std.space.suspend",
        Write {
            act: "MembershipSuspended",
            src: "member_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.reinstate",
        Write {
            act: "MembershipReactivated",
            src: "member_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.createGroup",
        Write {
            act: "group-created",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.deleteGroup",
        Write {
            act: "group-deleted",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.removeFromGroup",
        Write {
            act: "group-member-removed",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.setOpAccess",
        Write {
            act: "profile-op-set",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.resetOpAccess",
        Write {
            act: "profile-op-reset",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.setMemberAccess",
        Write {
            act: "profile-member-set",
            src: "profile_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.revokeInvitation",
        Write {
            act: "invitation-revoked",
            src: "invite_ops.rs",
            test: "people_reads.rs",
        },
    ),
    (
        "std.space.setAdmission",
        Write {
            act: "admission-policy-set",
            src: "admission.rs",
            test: "admission.rs",
        },
    ),
    // Recovery: the act is recorded by `Recovery::record_split`.
    (
        "std.space.recoverMember",
        Write {
            act: "opened",
            src: "recovery.rs",
            test: "recovery_ops.rs",
        },
    ),
    (
        "std.space.approveMemberRecovery",
        Write {
            act: "approved",
            src: "recovery.rs",
            test: "recovery_ops.rs",
        },
    ),
    (
        "std.space.setRecoveryPolicy",
        Write {
            act: "policy-set",
            src: "identity_ops.rs",
            test: "recovery_ops.rs",
        },
    ),
    // Owner succession (owner, 2026-10-24), in the deployment's own space.
    ("std.identity.openCases", Read),
    (
        "std.space.succeedOwner",
        Write {
            act: "owner-succeeded",
            src: "recovery.rs",
            test: "recovery_ops.rs",
        },
    ),
    (
        "std.identity.resolveObjection",
        Write {
            act: "objection-resolved",
            src: "recovery.rs",
            test: "recovery_ops.rs",
        },
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("readable").flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every `"std.<...>"` literal in the console's own source, outside its
/// unit tests.
fn ops_the_console_calls() -> BTreeSet<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut ops = BTreeSet::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("readable");
        let code = text.split("#[cfg(test)]").next().unwrap_or("");
        for (i, _) in code.match_indices("\"std.") {
            let rest = &code[i + 1..];
            let end = rest.find('"').unwrap_or(0);
            let lit = &rest[..end];
            let op_shaped = lit.split('.').count() == 3
                && lit
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_');
            if op_shaped {
                ops.insert(lit.to_string());
            }
        }
    }
    ops
}

fn host(path: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ordo-host")
        .join(path);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn every_door_op_the_console_calls_is_placed() {
    let placed: BTreeSet<&str> = CALLS.iter().map(|(op, _)| *op).collect();
    let unplaced: Vec<String> = ops_the_console_calls()
        .into_iter()
        .filter(|op| !placed.contains(op.as_str()))
        .collect();
    assert!(
        unplaced.is_empty(),
        "the console calls door ops G12's guard has not placed: {unplaced:?}. Add each to \
         CALLS as a read, or as a write with the act it records and the ordo-host test that \
         asserts the record"
    );
}

#[test]
fn every_read_is_a_read_at_the_door_and_every_write_is_a_change() {
    let specs = ordo_host::platform_ops::specs();
    for (op, rule) in CALLS {
        let spec = specs
            .iter()
            .find(|s| s.name == *op)
            .unwrap_or_else(|| panic!("{op} is not a door op"));
        match rule {
            // `plan` is declared a write so that it asks for a touch, and
            // stores nothing.
            Read if *op == "std.deployment.plan" => {}
            Read => assert!(
                matches!(spec.kind, OpKind::Read),
                "{op} is placed as a read and the door declares it a change: it needs a record"
            ),
            Write { .. } => assert!(
                matches!(spec.kind, OpKind::Write | OpKind::Delete),
                "{op} is placed as a write and the door declares it a read"
            ),
        }
    }
}

#[test]
fn every_write_records_its_act_and_a_test_asserts_it() {
    let mut missing = Vec::new();
    for (op, rule) in CALLS {
        let Write { act, src, test } = rule else {
            continue;
        };
        let code = host(&format!("src/{src}"));
        // An act built from two words (`bucket_{verb}`) is found by both.
        let built = act.split_once('_').is_some_and(|(stem, word)| {
            code.contains(&format!("\"{stem}_")) && code.contains(&format!("\"{word}\""))
        });
        if !code.contains(act) && !built {
            missing.push(format!("{op}: ordo-host/src/{src} records no `{act}`"));
        }
        if !host(&format!("tests/{test}")).contains(act) {
            missing.push(format!(
                "{op}: ordo-host/tests/{test} does not assert the `{act}` record"
            ));
        }
    }
    assert!(missing.is_empty(), "G12 gaps: {missing:#?}");
}
