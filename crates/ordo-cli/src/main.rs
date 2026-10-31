use std::path::Path;
use std::process::exit;

use ordo_cli::Sources;

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         ordo check <dir|file.ordo> [--expect-fail <op>]...\n      \
         human report: entities, guards, ops. Exits 1 when an op does not\n      \
         compile, unless named by --expect-fail (which fails once it compiles)\n  \
         ordo plan  <dir|file.ordo> [op]     artifact JSON (byte-stable), all ops or one\n  \
         ordo interface <dir|file.ordo> [--out <dir>]\n      \
         the Interface Descriptor (interface.json): the one data contract\n      \
         the generated SDK, the LSP and generated UI share. A projection of\n      \
         the same artifact `ordo plan` prints — entities with their types,\n      \
         enum values and refs; ops with params, result shape, mode label,\n      \
         cost, disclosures, terminal verbs and typed-id action discovery.\n      \
         Byte-stable, versioned and digested. `--out <dir>` writes\n      \
         <dir>/interface.json instead of printing.\n  \
         ordo gen ts <dir|file.ordo> [--out <root>]\n      \
         regenerate the generated half of both TypeScript packages:\n      \
         clients/ordo-sdk (the typed client, a pure function of the\n      \
         interface descriptor) and clients/ordo-cdk (the typed builder —\n      \
         its language layer from the parser's own closed vocabularies, its\n      \
         schema layer from the same descriptor). Neither package's\n      \
         generated half is ever hand-edited; language evolution is this\n      \
         command. `--out` defaults to the current directory, which is the\n      \
         repository root in the ordinary case.\n  \
         ordo gen ts <dir|file.ordo> --app <srcdir>\n      \
         an app's typed client: <srcdir>/generated/*.ts importing the SDK's\n      \
         core from @hominemai/ordo-sdk/core/, <srcdir>/interface.json for\n      \
         the React hooks, and <srcdir>/generated/OPS.md, the `ordo ops`\n      \
         index. Run it after every program change.\n  \
         ordo ops   <dir|file.ordo> [op|word] [--entity <e>] [--file <f>]\n      \
         what an app may call, from the same descriptor as `ordo interface`.\n      \
         No query: one line per op (params, returns, who may call), grouped\n      \
         by entity, with the platform ops (std.space.*) last. An op's name:\n      \
         that op in full, with a call to paste for the generated client and\n      \
         for @hominemai/ordo-react. Any other word: the ops whose name,\n      \
         params, returned fields or entity contain it. --entity and --file\n      \
         narrow the list to one group or one source file. Exits 1 when an op\n      \
         does not compile, as `check` does, or when nothing matches.\n  \
         ordo skill install [<project dir>] [--force]\n      \
         write the Ordo agent skill, the one this binary carries, into\n      \
         <dir>/.claude/skills/ordo/ and a marked block into <dir>/AGENTS.md\n      \
         pointing other agents at it. The directory defaults to the current\n      \
         one. Running it again replaces what it wrote and nothing else; a\n      \
         file of the same name changed since is refused unless --force.\n  \
         ordo dev   <dir|file.ordo> [--fresh] [--out <dir>] [--gen <cmd>]\n      \
         the sub-second hot loop (`plan-dev-loop.md`): watches the `.ordo`\n      \
         set, recompiles on save, brings up its own Postgres (or connects\n      \
         to ORDO_PG_* when set), classifies every save's schema delta,\n      \
         applies an additive one through the same class-A path `ordo\n      \
         migrate --apply` runs and resets and reseeds (from `seed.sql`)\n      \
         on anything else (§4.3), then gates and swaps\n      \
         through the SAME assembly a server gets, serving on ORDO_DEV_ADDR\n      \
         (default 127.0.0.1:4000) with no restart between saves.\n      \
         `--fresh` destroys an owned cluster's data directory before\n      \
         starting. `--out <dir>` writes interface.json there whenever its\n      \
         digest moves; `--gen <cmd>` runs after a digest change, with the\n      \
         descriptor's path in ORDO_INTERFACE_PATH — the seam, not a\n      \
         generator, and a failing one prints its output without stopping\n      \
         the loop.\n  \
         ordo console recover [--owner <id>]\n      \
         the console's lockout failsafe, run inside the image: prints a\n      \
         one-time setup code that enrols a new console passkey, and writes\n      \
         the time and unix user to the console's audit log. When the code\n      \
         is used, every console session ends.\n  \
         ordo plan  <dir|file.ordo> --against <db>\n      \
         the migration report: every change between the program and a LIVE\n      \
         database, classified, probed and costed. Read-only — it applies no\n      \
         DDL and writes nothing. Refuses to print an appliable plan when any\n      \
         probe comes back non-empty.\n  \
         ordo migrate <dir|file.ordo> --apply --db <db> [--as <actor>] [--places ...]\n      \
         the plan, then the additive apply (schema-evolution increment 2):\n      \
         a first apply against an empty database creates the whole schema;\n      \
         over a deployed one it sends class A (new entity, optional or\n      \
         constant-defaulted field, index) in one transaction with the index\n      \
         concurrently after it, plus a constraint that constrains no\n      \
         existing data. Everything else is refused by name with the\n      \
         statement to run by hand where one exists. Recorded in\n      \
         `$migrations` and on the deployment's security stream. Then\n      \
         `ordo apply` stores and activates the artifact.\n      \
         --places <entity>.<field>=rescale|keep[,...] (also on `plan`)\n      \
         states what a change of places on a stored money or decimal\n      \
         field does to its amounts: rescale keeps each value (money is\n      \
         multiplied or divided by 10^n); keep leaves the stored numbers\n      \
         as written. Refused until stated.\n\
         \n\
         A path may be a single `.ordo` file or a directory; a directory is\n\
         read recursively in sorted relative-path order, and diagnostics name\n\
         the file they came from.\n\
         \n\
         signing in: deployment people, at the identity service\n  \
         ordo login [<identity url>] [--touch] [--deployment <h>] [--no-browser]\n      \
         sign this computer in through your browser. The session is kept in\n      \
         ~/.config/ordo/sessions.json (ORDO_CONFIG_DIR moves it), readable by\n      \
         you only. `--touch` confirms with your passkey again: writes need a\n      \
         touch in the last 5 minutes. The url defaults to ORDO_IDENTITY_URL.\n  \
         ordo logout [<identity url>]    end the session and delete it here.\n  \
         ordo whoami [<identity url>]    who the next command acts as.\n  \
         ordo people list | invite [--label <t>] | end <p_id>\n  \
         ordo people machine-key add --name <n> | list | end <m_id>\n      \
         the deployment's admins and machine keys. A machine key's secret is\n      \
         shown once.\n      \
         Commands that call a deployment send, in order: --token, ORDO_TOKEN,\n      \
         a machine key (ORDO_MACHINE_KEY or ORDO_MACHINE_KEY_FILE, with\n      \
         ORDO_IDENTITY_URL; for CI, no browser), or the session from login.\n\
         \n\
         operator commands — these talk to a deployment's database\n\
         (ORDO_PG_HOST/PORT/USER/DB, or --db <name>). `apply` also speaks to\n\
         a deployment over HTTP; see its --host below. The rest stay on the\n\
         database deliberately: verify, checkpoint, prune, revert and keys\n\
         must work when the door does not, and a verification that can only\n\
         be performed by the thing under suspicion is not a verification.\n  \
         ordo apply     <dir|file.ordo> [--as <actor>] [--no-activate]\n      \
         compile the program, store its artifact under its version hash,\n      \
         point the deployment at it, and nudge every replica. On an empty\n      \
         database it creates the schema first; on one the load gate would\n      \
         refuse it stores and does not activate, naming `ordo migrate\n      \
         --apply`. An op that does not compile refuses the whole apply.\n      \
         `--no-activate` stores without re-pointing, for a staged deploy.\n      \
         It then waits until every running node serves the new version and\n      \
         has brought the tenants' databases to it, and names any tenant\n      \
         held closed. `--wait-secs <n>` (or ORDO_APPLY_WAIT_SECS, default\n      \
         300) bounds the wait; past it the apply exits 1 and says what each\n      \
         node is still doing. `--no-wait` returns once the version is active.\n      \
         `--host <h> --token <t> --space <s>` sends the program to a running\n      \
         deployment's own door (`std.deployment.apply`) instead of opening its\n      \
         database. A bare host gets https; spell `http://` for a local `ordo\n      \
         dev` (`--host http://127.0.0.1:4000`). An empty `--host`\n      \
         selects that address. ORDO_HOST, ORDO_TOKEN and ORDO_SPACE are the environment forms.\n      \
         Without `--token`, the token comes from a machine key or from\n      \
         `ordo login` (see signing in, below). A capability token can never\n      \
         apply, because applying is step-up gated.\n      \
         `--as` is refused with `--host`: over the door the actor\n      \
         recorded is the principal the token proved, not one you assert.\n  \
         ordo space create --name <n> --owner <o> --host <h> --token <t> --space <s>\n      \
         mint a tenant through the deployment's own door (`std.space.create`,\n      \
         17aa). Prints the minted space id and whether it is reachable now or\n      \
         at the next restart. No `--db` mode exists: creating a tenant is the\n      \
         deployment's act, gated the same way `apply` is (owner, step-up, the\n      \
         bound space must be the deployment's own), never a row an operator\n      \
         writes by hand. `--host`, `--token` and `--space` are `apply`'s own\n      \
         flags and resolve the same way; see `apply` above for the defaults,\n      \
         the environment forms and where the token comes from.\n  \
         ordo console-key revoke --id <ck-...> --host <h> --token <t> --space <s>\n      \
         revoke a console's key at the deployment\n      \
         (`std.deployment.revokeConsoleKey`). Every session the key signed in\n      \
         ends at once; the console must be linked again. The token is the\n      \
         deployment owner's own sign-in with a passkey touch: this op is\n      \
         never exempt from step-up. `--host`, `--token` and `--space` resolve\n      \
         as for `apply`.\n  \
         ordo secret set <name> --host <h> --token <t> --space <s>\n      \
         set an app secret for backend functions (`std.deployment.setSecret`).\n      \
         The value is read from standard input, never from the command line:\n      \
         `ordo secret set openaiKey < key.txt`. It is sealed under the\n      \
         deployment's keys and never read back. Each set is a new version.\n  \
         ordo secret ls --host <h> --token <t> --space <s>\n      \
         list the app secrets: name, bound hosts, version, who set it and\n      \
         when. Never a value.\n  \
         ordo secret rm <name> --host <h> --token <t> --space <s>\n      \
         remove an app secret's value. `--host`, `--token` and `--space`\n      \
         resolve as for `apply`.\n  \
         ordo fn runs [<name>] [--limit <n>] --host <h> --token <t> --space <s>\n      \
         list backend function runs, newest first (`std.deployment.functionRuns`):\n      \
         run number, function, version, tenant, when, who, outcome and\n      \
         duration. 50 by default, at most 500. Runs are kept\n      \
         ORDO_FN_RUN_RETENTION_DAYS days, 14 by default.\n  \
         ordo fn log <seq> --host <h> --token <t> --space <s>\n      \
         show one run and the lines it logged (`std.deployment.functionRun`).\n      \
         A secret's value shows as [secret]. The deployment owner's only;\n      \
         `--host`, `--token` and `--space` resolve as for `apply`.\n  \
         ordo import --bundle <b> [--name <n>] --host <h> --token <t> --space <s>\n      \
         load a bundle an operator has staged for this space through the\n      \
         deployment's own door (`std.space.import`, 11-10a). The bundle is\n      \
         named, not pathed: it is read from ORDO_IMPORT_BUNDLE_DIR under the\n      \
         BOUND space's own directory, so a bundle staged for one tenant is\n      \
         not nameable by another. `--name` defaults to the bundle's name and\n      \
         is the resume key: re-running under the same name resumes whatever\n      \
         landed and will not import twice. A bundle whose row ids changed\n      \
         under a used name refuses; one whose ids are the same and whose\n      \
         VALUES changed does not, so use a new name for a changed bundle.\n      \
         No `--db` mode exists, because every sealed field\n      \
         is re-sealed under the destination space's key and this tool holds\n      \
         no custody.\n  \
         ordo identity recover --principal <id> --as <who> [--evidence <class>] [--ref <r>]\n      \
         open a recovery for a person who has lost every way in, from the host,\n      \
         and print the recovery ticket once. The deployment owner's way back in,\n      \
         and the only one when their own last passkey is gone. The person spends\n      \
         the ticket by registering a new passkey; nothing else accepts it. Their\n      \
         new passkey cannot satisfy step-up for a day (adjustable), every session\n      \
         they had ends, and the passkeys they had can contest it for three days.\n      \
         Evidence is one of vouched-by-members, verified-out-of-band,\n      \
         contract-holder, host-break-glass (the default).\n  \
         ordo rollback  --version <hash> [--as <actor>]\n      \
         re-point the registry at a version it already holds. The same\n      \
         mechanism as apply's activation, backwards; one more row in the\n      \
         activation log, never a rewrite of it.\n  \
         ordo deploys   [--limit <n>]\n      \
         what is stored, what is serving, and every re-point in order.\n  \
         ordo verify     --identity [--security-key-id <id> --security-public-key <hex>]\n      \
         verify the deployment's security stream, then reconcile every link in\n      \
         `$authenticator` against it: a link no event explains (a sign-up, a\n      \
         ticket, a link ceremony, a recovery, a password given) was written by\n      \
         hand, and is reported by id and principal, exit 3.\n  \
         ordo verify     --space <spc> [--key-id <id> --public-key <hex>]\n                  \
         [--from-genesis] [--control-db <name>]\n                  \
         [--attestation <file>]…\n      \
         recompute the space's chain and name the FIRST bad index and what\n      \
         kind of tamper it was. Anchors at the newest checkpoint whose\n      \
         signature verifies under the supplied key; otherwise genesis. The\n      \
         anchor used is always printed, because \"the whole chain verifies\"\n      \
         and \"the suffix since a marker verifies\" are different claims.\n      \
         --control-db reads the control database's $chainwitness, which a\n      \
         restore of the space cannot roll back; without it the output says\n      \
         the witness was not read. --attestation takes a receiver's\n      \
         ordo.attestation/1 document, repeatably. A head a receiver holds\n      \
         that this chain has moved away from, with no restore marker\n      \
         attesting a pre-restore head at or above it, is a restore nobody\n      \
         recorded, and it exits 3.\n      \
         It also verifies the space's SECURITY stream and reports it as its\n      \
         own paragraph, never merged: a single verdict would hide which of\n      \
         the two artifacts moved, and which one moved is the diagnosis. That\n      \
         stream is signed under its OWN key, so it takes its own flags —\n      \
         --security-key-id/--security-public-key, or\n      \
         ORDO_SECURITY_CHECKPOINT_KEY_ID/ORDO_SECURITY_CHECKPOINT_PUBKEY.\n      \
         Without them the entries are still recomputed against each other,\n      \
         but nothing proves none was removed from the END of the stream, and\n      \
         the output says so rather than printing a bare green line.\n  \
         ordo verify     --deployment [--control-db <name>]\n                  \
         [--security-key-id <id> --security-public-key <hex>]\n      \
         the deployment's own security stream, which belongs to no space and\n      \
         has no command chain beside it: unverified callers, non-members,\n      \
         tokens naming a space they have nothing to do with, and the\n      \
         platform's own custody events. It is the only place a CROSS-SPACE\n      \
         sweep is visible, because a principal probing a hundred spaces\n      \
         appears in none of their per-space streams by design.\n  \
         ordo verify     <dir|file.ordo> --space <spc> --objects\n                  \
         [--master-key-file <f> --passphrase-file <p>]\n      \
         the third, independent mode (`plan-blob-chain.md` §4.5): walk every\n      \
         image the chain has ever stored and compare its recorded digest\n      \
         against the object that is there, rather than recomputing hashes.\n      \
         Needs the program (only the schema says which columns are blob\n      \
         columns) and a key release to open a recorded digest — the master\n      \
         key from --master-key-file/--passphrase-file or ORDO_MASTER_KEY, and\n      \
         the object store from ORDO_BLOB_STORE, the same two sources `ordo\n      \
         keys` and the server itself read. Reports each handle's outcome and\n      \
         exits 3 on the first one the chain and the object disagree about.\n  \
         ordo checkpoint --space <spc> [--control-db <name>]\n      \
         verify the suffix and sign a marker at the current head, under\n      \
         ORDO_CHECKPOINT_KEY / ORDO_CHECKPOINT_KEY_ID. Prints the verifying\n      \
         key to publish; receivers need nothing else. Every signed head is\n      \
         also recorded in the control database's $chainwitness, which a\n      \
         restore of the space cannot roll back; --control-db names that\n      \
         database and defaults to --db. A witness that cannot be written\n      \
         signs nothing.\n  \
         ordo checkpoint --security (--space <spc> | --deployment)\n      \
         the same act for the SECURITY stream, under its own key\n      \
         (ORDO_SECURITY_CHECKPOINT_KEY / _KEY_ID). --security selects the\n      \
         artifact rather than adding one: reading both in one command is\n      \
         free, signing both under two keys is not, and an operator holding\n      \
         one of the two keys must be able to sign only its artifact.\n      \
         This is the marker `ordo verify` needs for its fourth check —\n      \
         nothing was removed from the END of the stream — which is the one\n      \
         property a chain walk cannot establish, because deleting the last\n      \
         N entries leaves a perfectly self-consistent chain. A deployment\n      \
         with the key set signs one every hour on its own\n      \
         (`secevent.checkpoint`); this is the same pass by hand.\n      \
         It verifies the stream before signing and REFUSES (exit 3) rather\n      \
         than anchor history that does not recompute, countersign a stream\n      \
         entries have already been removed from the end of, or extend a\n      \
         retained tail nothing signed vouches for.\n  \
         ordo prune      --space <spc> [--key-id <id> --public-key <hex>] [--dry-run]\n      \
         cut the chain back to its floor: the greatest CHECKPOINT at or\n      \
         below both the undo horizon and the slowest registered consumer.\n      \
         `--dry-run` needs no key and shows where the floor is and who is\n      \
         holding it; a real prune verifies the cut point and the prefix it\n      \
         is about to destroy, so it requires the key. This is the one\n      \
         command that destroys audit evidence.\n  \
         ordo triggers ls --space <spc> [--host <h> --token <t>]\n  \
         ordo triggers deregister --space <spc> --listener <name> [--as <who>]\n                          \
         [--force] [--host <h> --token <t>]\n      \
         list a space's consumer cursors, or retire a dead listener's so it\n      \
         stops pinning the log floor. With --host the running deployment says\n      \
         which listeners it no longer serves; without it, or if the listener\n      \
         is still served, deregister refuses unless --force. The act and the\n      \
         position it released are recorded in the space's security stream.\n  \
         ordo restore   --record --space <spc> --as <who>\n                  \
         (--source pitr --at <secs> | --source bundle --digest <hex>)\n                  \
         [--pre-restore-head <idx>:<hex> | --unattested] [--again]\n                  \
         [--restored-from <idx>]\n                  \
         [--control-db <name> | --tenant]\n      \
         record a restore that has already happened: a `system:restore`\n      \
         marker in the chain saying where history resumed, joined by one\n      \
         correlation to a `custody-action` in the security-event stream and\n      \
         to a row in the control database. Without it the platform refuses\n      \
         to serve the space. Restores are recorded by the operator who ran\n      \
         them; this tool attributes, it does not authenticate. `--tenant`\n      \
         resolves --space's database through $spacedb instead of trusting\n      \
         --db to name it, and prints which database it resolved; refused\n      \
         together with --db, which is the manual, break-glass form.\n  \
         ordo fleet      [--db <name>]\n      \
         every target's schema version and when it was last confirmed, its\n      \
         custody, and which tenants are on it, each tenant printed with its\n      \
         name beside its id. From the control database alone: quarantine,\n      \
         the connection budget and installed credential labels are a\n      \
         replica's own state and are not here — see /health/ready.\n  \
         ordo revert     <dir|file.ordo> --space <spc> --group <id> --as <principal>\n                  \
         [--dry-run]\n      \
         apply the inverse of a command group as NEW history, in one\n      \
         transaction, with every touched row checked against `--as`'s WRITE\n      \
         guard before and after. History is never rewritten.\n      \
         `--as` is asserted, not authenticated: this is an operator tool\n      \
         holding database credentials. It checks authorization, not\n      \
         identity. See the `operator` module docs.\n\
         \n\
         limits and metering:\n  \
         ordo quota set  --space <spc> --quantity <requests|rows_returned>\n                  \
         --limit <n> --grace <n> --period-secs <n>\n      \
         set a per-space quota. Changes without a restart. Over the limit\n      \
         alone warns; over limit plus grace refuses with quota_exceeded.\n  \
         ordo quota show --space <spc>\n      \
         the quotas set on a space.\n  \
         ordo usage --space <spc> --from <secs> --to <secs> [--db <name>]\n      \
         a space's usage over [from, to) as JSONL, one record per period.\n\
         \n\
         key ceremony — custody that only a running server can create is\n\
         custody nobody performed:\n  \
         ordo keys seal-master --out <path> --passphrase-file <p> [--key-id <id>]\n      \
         mint a deployment master key and write it sealed under a\n      \
         passphrase-derived key (argon2id, the OWASP profile). Prints the\n      \
         key id and a fingerprint; never the key. Refuses to overwrite.\n      \
         The passphrase comes from a FILE: a flag is in the shell history\n      \
         and in `ps`, and a prompt cannot be scripted.\n  \
         ordo keys init --space <spc> --as <who>\n                 \
         [--master-key-file <f> --passphrase-file <p>]\n      \
         create `$keycustody` if absent and mint the space's SMK and epoch\n      \
         1's DEK. Idempotent. Records a `custody-action` in the space's\n      \
         security-event stream. Without the file flags the master comes\n      \
         from ORDO_MASTER_KEY. **A deployment will not mint custody for a\n      \
         space it was merely told about** — this command is why.\n  \
         ordo keys status  --space <spc> --as <who> [--db <name>]\n                   \
         [--master-key-file <f> --passphrase-file <p>]\n      \
         the current epoch, each live epoch with its counts and standing,\n      \
         and every re-seal pass with its cursor and attempts.\n  \
         ordo keys rotate  --space <spc> --as <who> --reason <text>\n                   \
         [--db <name>] [--master-key-file <f> --passphrase-file <p>]\n      \
         mint the next DEK and queue a re-seal pass for the epoch it replaces.\n  \
         ordo keys reseal  --space <spc> --as <who> [--db <name>]\n                   \
         [--master-key-file <f> --passphrase-file <p>]\n      \
         queue a pass for every retired epoch that still holds anything.\n  \
         ordo keys destroy --space <spc> --epoch <n> --as <who> [--confirm]\n                   \
         [--db <name>] [--master-key-file <f> --passphrase-file <p>]\n      \
         destroy a retired DEK once nothing is sealed under it. Refuses by\n      \
         name while any count is non-zero.\n  \
         ordo keys rewrap-master --from <f> --from-passphrase-file <p>\n                          \
         --to <f> --to-passphrase-file <p> --as <who> [--dry-run]\n      \
         re-wrap every space's SMK from one master to another, in one\n      \
         transaction. NOT an SMK rotation: no envelope and no blind index\n      \
         moves, so it is one row per space rather than a full re-index.\n\
         \n\
         There is no escrow and no recovery. Back up the sealed file and the\n\
         passphrase separately, to somewhere that is not this deployment's\n\
         database: the property that makes crypto-erasure real is the\n\
         property that makes losing key material unrecoverable.\n\
         \n\
         exit codes: 0 ok · 1 failed or refused · 2 usage · 3 verification failed\n"
    );
    exit(2);
}

/// `--out <dir>`, wherever it appears in the trailing arguments.
fn out_dir(rest: &[String]) -> Option<String> {
    let i = rest.iter().position(|a| a == "--out")?;
    match rest.get(i + 1) {
        Some(dir) => Some(dir.clone()),
        None => {
            eprintln!("--out needs a directory");
            exit(2);
        }
    }
}

/// `<flag> <value>`, wherever it appears in the trailing arguments.
fn flag_value(rest: &[String], flag: &str) -> Option<String> {
    let i = rest.iter().position(|a| a == flag)?;
    match rest.get(i + 1) {
        Some(v) => Some(v.clone()),
        None => {
            eprintln!("{flag} needs a value");
            exit(2);
        }
    }
}

/// The first argument that is neither a flag nor the value of one of
/// `valued`.
fn positional(rest: &[String], valued: &[&str]) -> Option<String> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if valued.contains(&a.as_str()) {
            it.next();
        } else if !a.starts_with("--") {
            return Some(a.clone());
        }
    }
    None
}

/// `ordo dev --gen <command>` (D4): the generator seam, not the generator.
fn gen_command(rest: &[String]) -> Option<String> {
    let i = rest.iter().position(|a| a == "--gen")?;
    match rest.get(i + 1) {
        Some(cmd) => Some(cmd.clone()),
        None => {
            eprintln!("--gen needs a command");
            exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        usage()
    };

    // The operator commands take flags rather than a bare path, so they are
    // dispatched before the source-loading the compiler commands share.
    if matches!(
        cmd.as_str(),
        "verify"
            | "checkpoint"
            | "revert"
            | "apply"
            | "rollback"
            | "deploys"
            | "prune"
            | "restore"
            | "keys"
            | "migrate"
            | "quota"
            | "usage"
            | "space"
            | "console-key"
            | "secret"
            | "fn"
            | "identity"
            | "import"
            | "fleet"
            | "triggers"
    ) {
        let out = ordo_cli::operator::run(cmd, rest);
        if out.code == 0 {
            print!("{}", out.report);
        } else {
            eprint!("{}", out.report);
        }
        exit(out.code);
    }

    // `ordo login`, `logout`, `whoami`, `people`: deployment people at the
    // identity service (`plan-identity-service.md` 5.5 to 5.7).
    if matches!(cmd.as_str(), "login" | "logout" | "whoami" | "people") {
        let out = ordo_cli::signin::run(cmd, rest);
        if out.code == 0 {
            print!("{}", out.report);
        } else {
            eprint!("{}", out.report);
        }
        exit(out.code);
    }

    // `ordo console recover`: the console's lockout failsafe
    // (`plan-console.md` 5.3 item 7). Run inside the image.
    if cmd == "console" {
        let (code, report) = ordo_console::recover::run_cli(rest);
        if code == 0 {
            print!("{report}");
        } else {
            eprint!("{report}");
        }
        exit(code);
    }

    // `plan --against <db>` is a DEPLOYMENT command wearing `plan`'s name,
    // and that is deliberate: §9's engine is `plan` then `apply`, and an
    // operator asking "what will this deploy do to my database" should not
    // have to learn a second verb to ask it. It dispatches before the
    // source-loading path because it needs flags, a database and a runtime.
    if cmd == "plan" && rest.iter().any(|a| a == "--against") {
        let out = ordo_cli::operator::run("plan", rest);
        if out.code == 0 {
            print!("{}", out.report);
        } else {
            eprint!("{}", out.report);
        }
        exit(out.code);
    }

    // `ordo dev <dir>` — the sub-second hot loop (`plan-dev-loop.md`).
    // Dispatched before the source-loading path below because it does its
    // own reading, over and over, rather than once.
    if cmd == "dev" {
        let [path, flags @ ..] = rest else { usage() };
        let opts = ordo_cli::dev::DevOptions {
            fresh: flags.iter().any(|a| a == "--fresh"),
            out: out_dir(flags).map(std::path::PathBuf::from),
            gen: gen_command(flags),
        };
        if let Err(e) = ordo_cli::dev::run(Path::new(path), opts) {
            eprintln!("ordo dev: {e}");
            exit(1);
        }
        exit(0);
    }

    // `ordo ops <dir> [query]`: the derived lookup of what an app may call.
    if cmd == "ops" {
        let [path, flags @ ..] = rest else { usage() };
        let query = ordo_cli::ops::Query {
            text: positional(flags, &["--entity", "--file"]),
            entity: flag_value(flags, "--entity"),
            file: flag_value(flags, "--file"),
        };
        let src = match Sources::read(Path::new(path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                exit(1);
            }
        };
        let (code, out, err) = ordo_cli::ops::run(&src, &query, path);
        print!("{out}");
        eprint!("{err}");
        exit(code);
    }

    // `ordo skill install [<dir>]`: the agent skill this binary carries.
    if cmd == "skill" {
        let [sub, flags @ ..] = rest else { usage() };
        if sub != "install" {
            eprintln!("`ordo skill {sub}`: the only subcommand is `install`");
            exit(2);
        }
        let dir = positional(flags, &[]).unwrap_or_else(|| ".".to_string());
        let force = flags.iter().any(|a| a == "--force");
        match ordo_cli::skill::install(Path::new(&dir), force) {
            Ok(report) => print!("{report}"),
            Err(e) => {
                eprintln!("{e}");
                exit(1);
            }
        }
        exit(0);
    }

    // `gen` takes a backend name before its path, because there will be more
    // than one backend (`tooling-and-embedding-plan.md` §B6 sequences Python,
    // Go and Java behind TypeScript) and a flagless `ordo gen <dir>` would
    // have to grow one later, breaking every script that had learned it.
    let (cmd, rest) = if cmd == "gen" {
        let [backend, rest @ ..] = rest else { usage() };
        if backend != "ts" {
            eprintln!(
                "`ordo gen {backend}` — the only backend is `ts`. Python, Go and \
                 Java are planned (tooling-and-embedding-plan.md §B6) and each is \
                 a pure consumer of the same descriptor; none is built."
            );
            exit(2);
        }
        (&"gen ts".to_string(), rest)
    } else {
        (cmd, rest)
    };

    let [path, rest @ ..] = rest else { usage() };
    let src = match Sources::read(Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };
    let out_root = out_dir(rest).unwrap_or_else(|| ".".to_string());
    let result = match cmd.as_str() {
        "check" => ordo_cli::check_sources(&src),
        "plan" => ordo_cli::plan_artifact(&src, rest.first().map(|s| s.as_str())),
        "interface" => ordo_cli::interface_json(&src),
        "gen ts" => match flag_value(rest, "--app") {
            Some(app) => ordo_cli::generate_app(&src, path, Path::new(&app)),
            None => ordo_cli::generate_ts(&src, Path::new(&out_root)),
        },
        _ => usage(),
    };
    let out = match result {
        Ok(out) => out,
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    };

    // `--out <dir>` writes the file `tooling-and-embedding-plan.md` §A1 names
    // (`out/interface.json`), rather than asking every build script to
    // redirect stdout and get the filename right. Stdout stays the default,
    // because that is what every other compiler command here does.
    if cmd == "interface" {
        if let Some(dir) = out_dir(rest) {
            let path = Path::new(&dir).join("interface.json");
            if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, &out))
            {
                eprintln!("{}: {e}", path.display());
                exit(1);
            }
            println!("{}", path.display());
            return;
        }
    }
    print!("{out}");
    if cmd == "check" {
        // On stderr, beside the refusals, so the report on stdout stays the
        // program's shape. A warning never changes the exit code.
        for w in ordo_cli::check_warnings(&src) {
            eprintln!("{w}");
        }
        exit(check_exit(&src, &expected_failures(rest)));
    }
}

/// `--expect-fail <op>`, repeatable: an op the caller knows does not compile
/// yet, such as one waiting on an unbuilt feature. Named one at a time so the
/// allowance is greppable, and it fails the check once the op compiles, so it
/// cannot outlive its reason.
fn expected_failures(rest: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--expect-fail" {
            match it.next() {
                Some(op) => out.push(op.clone()),
                None => {
                    eprintln!("--expect-fail needs an op name");
                    exit(2);
                }
            }
        }
    }
    out
}

/// `ordo check`'s exit code (reported fix 25p): 1 when an op does not compile
/// and was not expected to fail, or was expected to fail and now compiles.
fn check_exit(src: &Sources, expected: &[String]) -> i32 {
    let failed = match ordo_cli::uncompiled_ops(src) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut code = 0;
    for (op, why) in &failed {
        if !expected.contains(op) {
            eprintln!("ordo check: `{op}` does not compile [{why}]");
            code = 1;
        }
    }
    for op in expected {
        if !failed.iter().any(|(f, _)| f == op) {
            eprintln!(
                "ordo check: `{op}` was expected to fail and did not: it compiles, or no op \
                 by that name is in this program; drop --expect-fail {op}"
            );
            code = 1;
        }
    }
    code
}
