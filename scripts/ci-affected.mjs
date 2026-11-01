#!/usr/bin/env node
// Which CI jobs a push to `bleeding` reaches (owner ruling, 2026-11-01).
//
// A push to `bleeding` runs only what its change reaches. A pull request to
// `main`, the nightly run on `main` and a manual run run everything. So
// does a push whose commits carry the trailer line `ci: full`.
//
//   node scripts/ci-affected.mjs                  in CI (reads the env below)
//   node scripts/ci-affected.mjs --files list.txt one changed path per line
//   node scripts/ci-affected.mjs --diff <base>    diff <base> against HEAD
//
// CI env: EVENT (github.event_name), BEFORE (github.event.before), AFTER
// (github.sha). Writes its answer to $GITHUB_OUTPUT when that is set, and
// prints it either way.
//
// How a path is read:
//
// 1. Cargo.lock, the root Cargo.toml, rust-toolchain, .github/, .cargo/
//    config, docker/, scripts/ci-* and any path it does not know: run
//    everything.
// 2. A path in a workspace crate (from `cargo metadata`): that crate's
//    tests. Outside tests/, benches/, examples/ and e2e/ it is the crate's
//    code, so every crate that depends on it runs too, transitively.
// 3. clients/<pkg>/ and examples/<app>/: that package's job, and the e2e
//    suites and examples that import it.
// 4. Docs (docs/, *.md and other inert files): nothing, unless a crate
//    reads that file.
// 5. Every path: a crate whose sources name it in a string literal reads
//    it (`include_str!`, a fixture path), so that crate's tests run. Found
//    with `git grep` on each prefix of the path of 2 or more parts.
//
// When unsure it runs. The pull request to `main` runs everything anyway.

import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export const CLIENT_JOBS = ['ordo-sdk', 'ordo-cdk', 'ordo-react'];
export const EXAMPLE_JOBS = ['notes', 'notes-v2', 'teamspace', 'shopfront'];

// What each e2e suite builds (its `cargo build -p` line).
const NOTES_E2E_BUILDS = ['ordo-host', 'ordo-cli'];
const CONSOLE_E2E_BUILDS = ['ordo-console', 'ordo-cli', 'ordo-identity-service', 'ordo-supervisor'];
const IDENTITY_E2E_BUILDS = ['ordo-identity-service'];
// scripts/upgrade-continuity.sh builds these and runs an ordo-host test.
const UPGRADE_BUILDS = ['ordo-host', 'ordo-cli'];

const RUN_EVERYTHING = [
  /^Cargo\.lock$/,
  /^Cargo\.toml$/,
  /^rust-toolchain/,
  /^\.github\//,
  /^\.cargo\/config/,
  /^docker\//,
  /^scripts\/ci-/,
  // The Postgres the suites and e2e stacks start when CI gives none.
  /^scripts\/conformance-pg\.sh$/,
];

// Paths that run nothing of their own. A crate that reads one still runs
// (rule 5).
const INERT = [
  /^docs\//,
  /^review\//,
  /^security\//,
  /^site-animations\//,
  /^editors\//,
  /^bench\//,
  /^\.claude\//,
  /\.md$/,
  /^(LICENSE[^/]*|NOTICE|\.gitignore|clean-build-artifacts\.sh)$/,
  /^scripts\/(board\.py|board-agrees\.mjs|ai-guide\.sh|watch-ci\.sh|clean-test-clusters\.sh|reap-test-dbs\.sh|stress-store-pool\.sh|two-instances\.sh|guard-index-bench\.sh|make-secrets-test\.sh|make-secrets\.sh|check-sql-corpus\.py|check-sql-corpus-probes\.txt|build-fn-engine\.sh|build-wasm\.sh)$/,
  /^scripts\/rehearse\//,
];

const TEST_DIRS = /^(tests|benches|examples|e2e)\//;

/** The answer for a run that covers everything. */
export function everything(reason, packages) {
  return {
    full: true,
    reason,
    crates: new Set(packages.map((p) => p.name)),
    clients: new Set(CLIENT_JOBS),
    examples: new Set(EXAMPLE_JOBS),
    upgrade: true,
    audit: true,
    notesE2e: true,
    authz: true,
    consoleE2e: true,
    identityE2e: true,
  };
}

/** The crate a path belongs to: the longest package dir that holds it. */
function owner(path, packages) {
  let best = null;
  for (const p of packages) {
    if (path.startsWith(p.dir + '/') && (!best || p.dir.length > best.dir.length)) best = p;
  }
  return best;
}

/** Each prefix of 2 or more parts, longest first, also without `crates/`. */
export function prefixes(path) {
  const parts = path.split('/');
  const out = [];
  for (let i = parts.length; i >= 2; i--) out.push(parts.slice(0, i).join('/'));
  if (parts[0] === 'crates') {
    for (let i = parts.length; i >= 3; i--) out.push(parts.slice(1, i).join('/'));
  }
  return out;
}

/**
 * Work out what a list of changed paths reaches.
 *
 * packages: [{ name, dir, deps: [{ name, kind }] }], dir relative to the
 * repository root. readersOf(path): [{ file, include }], each source
 * file under crates/ that names the path, and whether it compiles the
 * file in (include_str!, include_bytes!, include!).
 */
export function affected({ files, packages, readersOf = () => [] }) {
  const code = new Set(); // crates whose code changed: they and their dependents
  const tests = new Set(); // crates whose tests alone changed
  const clients = new Set();
  const examples = new Set();
  const flags = {
    upgrade: false,
    audit: false,
    notesE2e: false,
    authz: false,
    consoleE2e: false,
    identityE2e: false,
  };

  const reader = (path) => {
    for (const { file, include } of readersOf(path)) {
      const p = owner(file, packages);
      if (!p) continue;
      // Compiled into the crate (include_str! in its code): the crate's
      // code changed. Read at run time or by a test: its tests alone.
      const rest = file.slice(p.dir.length + 1);
      (include && !TEST_DIRS.test(rest) ? code : tests).add(p.name);
    }
  };

  for (const f of files) {
    if (RUN_EVERYTHING.some((r) => r.test(f))) return everything(`${f} changed`, packages);
    if (/(^|\/)package(-lock)?\.json$/.test(f) || f === '.cargo/audit.toml') flags.audit = true;

    if (f.startsWith('crates/')) {
      const p = owner(f, packages);
      if (!p) return everything(`${f} is in no workspace crate`, packages);
      const rest = f.slice(p.dir.length + 1);
      if (rest === 'Cargo.toml') flags.audit = true;
      // A crate's own .md reaches its code only through include_str!,
      // which rule 5 finds; otherwise it is the crate's doc tests at most.
      if (TEST_DIRS.test(rest) || rest.endsWith('.md')) tests.add(p.name);
      else code.add(p.name);
      if (p.name === 'ordo-identity-service' && rest.startsWith('e2e/')) flags.identityE2e = true;
      if (p.name === 'ordo-console' && rest.startsWith('examples/')) flags.consoleE2e = true;
      reader(f);
      continue;
    }

    let m;
    if ((m = f.match(/^clients\/([^/]+)\//))) {
      const pkg = m[1];
      if (pkg === 'ordo-console-web') flags.consoleE2e = true;
      else if (CLIENT_JOBS.includes(pkg)) {
        clients.add(pkg);
        // ordo-react imports ordo-sdk; the example pages and the notes e2e
        // import both from clients/*/src.
        if (pkg === 'ordo-sdk') clients.add('ordo-react');
        // ordo-host's notes_app test runs the notes page's vitest.
        if (pkg === 'ordo-sdk' || pkg === 'ordo-react') {
          EXAMPLE_JOBS.forEach((x) => examples.add(x));
          flags.notesE2e = true;
          tests.add('ordo-host');
        }
      } else return everything(`${f} is in a client CI does not know`, packages);
      reader(f);
      continue;
    }

    if ((m = f.match(/^examples\/([^/]+)\//))) {
      const app = m[1];
      if (EXAMPLE_JOBS.includes(app)) examples.add(app);
      if (app === 'notes' || app === 'notes-v2') flags.notesE2e = true;
      reader(f);
      continue;
    }

    if (f.startsWith('skills/')) {
      // scripts/check-skill-examples.mjs, in the clients (ordo-react) job.
      clients.add('ordo-react');
      reader(f);
      continue;
    }

    if (f === 'scripts/check-skill-examples.mjs') {
      clients.add('ordo-react');
      continue;
    }
    if (f === 'scripts/upgrade-continuity.sh') {
      flags.upgrade = true;
      continue;
    }
    if (f === 'scripts/authz-matrix.sh') {
      flags.authz = true;
      continue;
    }
    if (f === 'scripts/pg-snapshot.sh') {
      flags.notesE2e = true;
      flags.consoleE2e = true;
      continue;
    }

    if (INERT.some((r) => r.test(f))) {
      reader(f);
      continue;
    }

    return everything(`${f} is a path this filter does not know`, packages);
  }

  // Dependents: normal and build dependencies carry a change on, so they
  // are followed transitively. A dev-dependency changes only that crate's
  // tests, so its dependents are added once and not followed.
  const built = new Set(code);
  for (let grew = true; grew; ) {
    grew = false;
    for (const p of packages) {
      if (built.has(p.name)) continue;
      if (p.deps.some((d) => d.kind !== 'dev' && built.has(d.name))) {
        built.add(p.name);
        grew = true;
      }
    }
  }
  const crates = new Set([...built, ...tests]);
  for (const p of packages) {
    if (p.deps.some((d) => d.kind === 'dev' && built.has(d.name))) crates.add(p.name);
  }

  const any = (set, names) => names.some((n) => set.has(n));
  // The upgrade job builds both and runs an ordo-host test file.
  if (any(built, UPGRADE_BUILDS) || tests.has('ordo-host')) flags.upgrade = true;
  if (crates.has('ordo-host')) flags.authz = true;
  if (any(built, NOTES_E2E_BUILDS)) flags.notesE2e = true;
  if (any(built, CONSOLE_E2E_BUILDS)) flags.consoleE2e = true;
  if (any(built, IDENTITY_E2E_BUILDS)) flags.identityE2e = true;
  // ordo-gen writes the clients' generated halves.
  if (built.has('ordo-gen')) CLIENT_JOBS.forEach((c) => clients.add(c));

  return { full: false, reason: `${files.length} changed paths`, crates, clients, examples, ...flags };
}

/** The workflow outputs. Lists are space-padded so `contains` matches whole names. */
export function outputs(r) {
  const list = (s) => (s.size ? ` ${[...s].sort().join(' ')} ` : '');
  const b = (x) => (x ? 'true' : 'false');
  return {
    full: b(r.full),
    reason: r.reason,
    crates: r.full ? 'all' : list(r.crates),
    rest: b(r.full || r.crates.size > 0),
    host: b(r.full || r.crates.has('ordo-host')),
    s3: b(r.full || r.crates.has('ordo-blob')),
    upgrade: b(r.upgrade),
    clients: list(r.clients),
    examples: list(r.examples),
    audit: b(r.audit),
    notes_e2e: b(r.notesE2e),
    authz: b(r.authz),
    console_e2e: b(r.consoleE2e),
    identity_e2e: b(r.identityE2e),
  };
}

// ---- reading the repository ----

const git = (args) => execFileSync('git', args, { encoding: 'utf8' }).trim();
const gitOk = (args) => spawnSync('git', args, { stdio: 'ignore' }).status === 0;

function workspacePackages() {
  const m = JSON.parse(
    execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], {
      encoding: 'utf8',
      maxBuffer: 64 << 20,
    }),
  );
  const root = m.workspace_root + '/';
  return m.packages.map((p) => ({
    name: p.name,
    dir: p.manifest_path.slice(root.length).replace(/\/Cargo\.toml$/, ''),
    deps: p.dependencies.filter((d) => d.path).map((d) => ({ name: d.name, kind: d.kind ?? 'normal' })),
  }));
}

const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function gitReaders(path) {
  const args = ['grep', '-nE'];
  for (const p of prefixes(path)) args.push('-e', `"[^"]*${escape(p)}["/]`);
  args.push('--', 'crates/*.rs');
  const r = spawnSync('git', args, { encoding: 'utf8', maxBuffer: 64 << 20 });
  if (r.status !== 0) return [];
  const found = new Map();
  for (const line of r.stdout.split('\n')) {
    const m = line.match(/^([^:]+):\d+:(.*)$/);
    if (!m) continue;
    // A line that is only the literal is a macro argument on its own
    // line, as rustfmt writes a long include_str!.
    const include = /include(_str|_bytes)?!/.test(m[2]) || /^\s*(concat!\()?"[^"]*"\)?,?\s*$/.test(m[2]);
    found.set(m[1], (found.get(m[1]) ?? false) || include);
  }
  return [...found].map(([file, include]) => ({ file, include }));
}

/** The base to diff against, or null when there is none to trust. */
function pushBase(before, after) {
  if (before && !/^0+$/.test(before) && gitOk(['cat-file', '-e', `${before}^{commit}`])) {
    if (gitOk(['merge-base', '--is-ancestor', before, after])) return before;
  }
  // A force-push or a new branch: what the branch holds beyond main.
  if (gitOk(['rev-parse', '--verify', '-q', 'origin/main'])) {
    try {
      return git(['merge-base', 'origin/main', after]);
    } catch {
      return null;
    }
  }
  return null;
}

function decide(argv, env) {
  const local = argv.includes('--files') || argv.includes('--diff');
  if (!local && env.EVENT !== 'push') {
    // Needs neither git history nor cargo.
    return everything(`a ${env.EVENT || 'local'} run runs everything`, []);
  }
  let packages;
  try {
    packages = workspacePackages();
  } catch (e) {
    return everything(`cargo metadata failed (${e.message.split('\n')[0]})`, []);
  }
  const i = argv.indexOf('--files');
  if (i >= 0) {
    const files = readFileSync(argv[i + 1], 'utf8').split('\n').map((s) => s.trim()).filter(Boolean);
    return affected({ files, packages, readersOf: gitReaders });
  }
  const d = argv.indexOf('--diff');
  let base;
  const after = env.AFTER || 'HEAD';
  if (d >= 0) {
    base = argv[d + 1];
  } else {
    base = pushBase(env.BEFORE, after);
    if (!base) return everything('no base commit to diff against', packages);
    const messages = git(['log', '--format=%B', `${base}..${after}`]);
    if (/^ci: full\s*$/m.test(messages)) return everything('a commit asks for it (ci: full)', packages);
  }
  let files;
  try {
    files = git(['diff', '--name-only', base, after]).split('\n').filter(Boolean);
  } catch {
    return everything(`git diff ${base} failed`, packages);
  }
  const r = affected({ files, packages, readersOf: gitReaders });
  if (!r.full) r.reason = `${files.length} changed paths since ${base.slice(0, 12)}`;
  return r;
}

function main() {
  let r;
  try {
    r = decide(process.argv.slice(2), process.env);
  } catch (e) {
    // A filter that cannot decide runs everything rather than nothing.
    r = everything(`the filter failed (${String(e.message).split('\n')[0]})`, []);
  }
  const out = outputs(r);
  const text = Object.entries(out)
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');
  console.log(text);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, text + '\n');
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `### What this change reaches\n\n\`\`\`\n${text}\n\`\`\`\n`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main();
