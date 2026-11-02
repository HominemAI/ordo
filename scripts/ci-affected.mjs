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
//    config, .config/nextest.toml, .config/ci-test-map.json, docker/,
//    scripts/ci-* and any path it does not know: run everything.
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
// Test in proportion to the change (owner ruling 2026-11-02, board task
// `test-in-proportion-to-the-change` asks 1 and 2):
//
// 6. A test file under crates/<X>/tests/ that is a test target (from
//    `cargo metadata`) runs only that test binary. Another file there (a
//    helper such as tests/fixture/ or tests/mail_support/) runs the test
//    binaries whose source names it; when none does, all of X's test
//    binaries. Either way, X's unit and doc tests do not run.
// 7. A file under crates/ordo-host/src/ runs the ordo-host test binaries
//    .config/ci-test-map.json lists for it, plus ordo-host's unit and doc
//    tests. A file mapped to "all", or not in the map, runs every one.
//    Crates that depend on ordo-host run whole, as in rule 2.
//
// The narrowing goes out as `test_filter`, a nextest filterset that
// scripts/ci-test-shard.sh joins to each tier's own. Empty means every
// test.
//
// When unsure it runs. The pull request to `main` runs everything anyway.

import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, readdirSync, readFileSync } from 'node:fs';
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
  // Which tests take turns under cargo-nextest.
  /^\.config\/nextest\.toml$/,
  // Which ordo-host tests each of its source files reaches (rule 7).
  /^\.config\/ci-test-map\.json$/,
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

export const HOST = 'ordo-host';
const HOST_SRC = /^crates\/ordo-host\/src\/.+\.rs$/;
const word = (k) => new RegExp(`\\b${k.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`);

/**
 * The test binaries of crate `p` that a changed file under its tests/ can
 * reach (rule 6), or null for all of them.
 *
 * p.tests: [{ name, path }], its test targets, path relative to p.dir.
 * io.read(path): a file's text, or null. io.list(dir): the .rs files under
 * a directory, relative to the repository root.
 */
export function testBinaries(p, rest, io) {
  const roots = p.tests ?? [];
  const root = roots.find((t) => t.path === rest);
  const stem = rest.split('/')[1].replace(/\.rs$/, '');
  const text = (t) => io.read(`${p.dir}/${t.path}`);
  if (root) {
    // A root another root pulls in with #[path = "..."] reaches that one too.
    const out = new Set([root.name]);
    for (const t of roots) {
      const s = text(t);
      if (s === null) return null;
      if (t !== root && s.includes(`${stem}.rs"`)) out.add(t.name);
    }
    return [...out].sort();
  }
  // A helper: named by its first part under tests/, a directory or a file.
  // A helper that names it (authz_support names mail_support) passes it on.
  const rootPaths = new Set(roots.map((t) => `${p.dir}/${t.path}`));
  const helpers = io.list(`${p.dir}/tests`).filter((f) => !rootPaths.has(f));
  const keyOf = (f) => f.slice(p.dir.length + 'tests/'.length + 1).split('/')[0].replace(/\.rs$/, '');
  const keys = new Set([stem]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const h of helpers) {
      const k = keyOf(h);
      if (keys.has(k)) continue;
      const s = io.read(h);
      if (s !== null && [...keys].some((x) => word(x).test(s))) {
        keys.add(k);
        grew = true;
      }
    }
  }
  const out = [];
  for (const t of roots) {
    const s = text(t);
    if (s === null) return null;
    if ([...keys].some((x) => word(x).test(s))) out.push(t.name);
  }
  return out.length ? out.sort() : null;
}

/**
 * What is wrong with the ordo-host test map, as messages that say how to
 * fix it. srcFiles: every crates/ordo-host/src/**.rs; binaries: every
 * ordo-host test binary.
 */
export function checkMap(map, srcFiles, binaries) {
  const problems = [];
  const files = map?.files;
  if (!files || typeof files !== 'object') {
    return ['.config/ci-test-map.json has no "files" object. Restore it from git.'];
  }
  const bins = new Set(binaries);
  for (const f of srcFiles) {
    if (!(f in files)) {
      problems.push(
        `${f} is not in .config/ci-test-map.json. Add "${f}": "all" under "files", ` +
          'or the list of ordo-host test binaries it reaches. "all" is always safe.',
      );
    }
  }
  const src = new Set(srcFiles);
  for (const [f, v] of Object.entries(files)) {
    if (!src.has(f)) {
      problems.push(`.config/ci-test-map.json names ${f}, which does not exist. Remove it, or rename it to the file's new path.`);
      continue;
    }
    if (v === 'all') continue;
    if (!Array.isArray(v) || v.length === 0) {
      problems.push(`.config/ci-test-map.json: ${f} must be "all" or a list of test binaries that is not empty. "all" is always safe.`);
      continue;
    }
    for (const b of v) {
      if (!bins.has(b)) {
        problems.push(
          `.config/ci-test-map.json: ${f} names test binary ${b}, which does not exist ` +
            `(no crates/ordo-host/tests/${b}.rs). Fix the name or remove it; "all" is always safe.`,
        );
      }
    }
  }
  return problems;
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
 *
 * Rules 6 and 7: packages[].tests lists each crate's test targets; io
 * reads the files they name (testBinaries); hostMap is
 * .config/ci-test-map.json. Without them every test of a reached crate
 * runs, as before.
 */
export function affected({ files, packages, readersOf = () => [], io = { read: () => null, list: () => [] }, hostMap = null }) {
  const code = new Set(); // crates whose code changed: they and their dependents
  const tests = new Set(); // crates whose tests run whole, unit and doc tests too
  const bins = new Map(); // crate -> Set of test binaries, or null for all of them
  const hostSrc = new Set(); // ordo-host binaries its mapped source files reach
  let hostAll = false; // an ordo-host code change the map does not narrow
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

  // A file under a crate's tests/: the test binaries it reaches (rule 6).
  const testFile = (p, rest) => {
    const found = testBinaries(p, rest, io);
    const had = bins.get(p.name);
    if (had === null) return;
    if (found === null) bins.set(p.name, null);
    else bins.set(p.name, new Set([...(had ?? []), ...found]));
  };

  const reader = (path) => {
    for (const { file, include } of readersOf(path)) {
      const p = owner(file, packages);
      if (!p) continue;
      // Compiled into the crate (include_str! in its code): the crate's
      // code changed. Read at run time or by a test: its tests alone.
      const rest = file.slice(p.dir.length + 1);
      if (include && !TEST_DIRS.test(rest)) {
        code.add(p.name);
        if (p.name === HOST) hostAll = true;
      } else if (rest.startsWith('tests/')) testFile(p, rest);
      else tests.add(p.name);
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
      if (rest.startsWith('tests/')) testFile(p, rest);
      else if (TEST_DIRS.test(rest) || rest.endsWith('.md')) tests.add(p.name);
      else {
        code.add(p.name);
        if (p.name === HOST) {
          const mapped = HOST_SRC.test(f) ? hostMap?.files?.[f] : undefined;
          if (Array.isArray(mapped) && mapped.length) mapped.forEach((b) => hostSrc.add(b));
          else hostAll = true;
        }
      }
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
  const closure = (start) => {
    const built = new Set(start);
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
    const devs = packages.filter((p) => p.deps.some((d) => d.kind === 'dev' && built.has(d.name))).map((p) => p.name);
    return { built, devs };
  };
  const { built, devs } = closure(code);
  // Crates whose every test runs: changed code, its dependents, its
  // dev-dependents once, and crates whose tests run whole.
  const whole = new Set([...built, ...devs, ...tests]);
  // ordo-host narrows by the map only when its own mapped files are all
  // that reach it (rule 7).
  if (code.has(HOST) && !hostAll && !tests.has(HOST)) {
    const others = closure([...code].filter((c) => c !== HOST));
    if (!others.built.has(HOST) && !others.devs.includes(HOST)) {
      whole.delete(HOST);
      const had = bins.get(HOST);
      if (had !== null) bins.set(HOST, new Set([...(had ?? []), ...hostSrc]));
    }
  }
  const crates = new Set([...whole, ...bins.keys()]);

  // Per crate: null when every test runs, else { unit, bins } with bins
  // null for every test binary.
  const scope = new Map();
  for (const c of crates) {
    if (whole.has(c)) scope.set(c, null);
    else scope.set(c, { unit: code.has(c), bins: bins.get(c) });
  }
  // Whether crate c's test binary b runs.
  const runs = (c, b) => crates.has(c) && (scope.get(c) === null || scope.get(c).bins === null || scope.get(c).bins.has(b));

  const any = (set, names) => names.some((n) => set.has(n));
  // The upgrade job builds both and runs an ordo-host test file.
  if (any(built, UPGRADE_BUILDS) || runs(HOST, 'upgrade_continuity')) flags.upgrade = true;
  // scripts/authz-matrix.sh runs ordo-host's authz_matrix.
  if (runs(HOST, 'authz_matrix')) flags.authz = true;
  if (any(built, NOTES_E2E_BUILDS)) flags.notesE2e = true;
  if (any(built, CONSOLE_E2E_BUILDS)) flags.consoleE2e = true;
  if (any(built, IDENTITY_E2E_BUILDS)) flags.identityE2e = true;
  // ordo-gen writes the clients' generated halves.
  if (built.has('ordo-gen')) CLIENT_JOBS.forEach((c) => clients.add(c));

  return { full: false, reason: `${files.length} changed paths`, crates, scope, clients, examples, ...flags };
}

/**
 * The nextest filterset for the tests a change reaches, '' for every test.
 * scripts/ci-test-shard.sh joins it to each tier's own filter.
 */
export function testFilter(r) {
  if (r.full) return '';
  if (r.crates.size === 0) return 'none()';
  const parts = [];
  for (const c of [...r.crates].sort()) {
    const s = r.scope?.get(c) ?? null;
    if (s === null) {
      parts.push(`package(${c})`);
      continue;
    }
    if (s.unit) parts.push(`(package(${c}) & (kind(lib) | kind(bin)))`);
    if (s.bins === null || s.bins === undefined) parts.push(`(package(${c}) & kind(test))`);
    else for (const b of [...s.bins].sort()) parts.push(`binary_id(${c}::${b})`);
  }
  return parts.join(' | ');
}

/** The workflow outputs. Lists are space-padded so `contains` matches whole names. */
export function outputs(r) {
  const list = (s) => (s.size ? ` ${[...s].sort().join(' ')} ` : '');
  const b = (x) => (x ? 'true' : 'false');
  // Crates whose code did not change: their doc tests stay out.
  const testsOnly = r.full ? new Set() : new Set([...r.crates].filter((c) => r.scope?.get(c) && !r.scope.get(c).unit));
  return {
    full: b(r.full),
    reason: r.reason,
    crates: r.full ? 'all' : list(r.crates),
    test_filter: testFilter(r),
    tests_only: list(testsOnly),
    unit: b(r.full || r.crates.size > 0),
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
  repoRoot = root;
  return m.packages.map((p) => {
    const dir = p.manifest_path.slice(root.length).replace(/\/Cargo\.toml$/, '');
    return {
      name: p.name,
      dir,
      deps: p.dependencies.filter((d) => d.path).map((d) => ({ name: d.name, kind: d.kind ?? 'normal' })),
      // Its test targets (rule 6), path relative to dir.
      tests: p.targets
        .filter((t) => t.kind.includes('test'))
        .map((t) => ({ name: t.name, path: t.src_path.slice(root.length + dir.length + 1) })),
    };
  });
}

let repoRoot = '';

/** Reading files for testBinaries, relative to the repository root. */
export const fsIo = (base) => ({
  read: (path) => {
    try {
      return readFileSync(base + path, 'utf8');
    } catch {
      return null;
    }
  },
  list: (dir) => {
    const out = [];
    const walk = (d) => {
      let names;
      try {
        names = readdirSync(base + d, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of names) {
        if (e.isDirectory()) walk(`${d}/${e.name}`);
        else if (e.name.endsWith('.rs')) out.push(`${d}/${e.name}`);
      }
    };
    walk(dir);
    return out.sort();
  },
});

/** .config/ci-test-map.json, or null when it cannot be read (rule 7 then narrows nothing). */
function hostMapAt(base) {
  try {
    return JSON.parse(readFileSync(base + '.config/ci-test-map.json', 'utf8'));
  } catch {
    return null;
  }
}

const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function gitReaders(path) {
  const args = ['grep', '-nE'];
  // The literal ends at the prefix (a path or a directory that holds it),
  // or goes on as a pattern ("ordo-host/tests/{test}", "tests/*"). A
  // literal that goes on to some other file under the prefix
  // ("crates/ordo-host/src/blob.rs" for a test file) does not read it.
  for (const p of prefixes(path)) args.push('-e', `"[^"]*${escape(p)}(/?"|/[{*])`);
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
    return affected({ files, packages, readersOf: gitReaders, io: fsIo(repoRoot), hostMap: hostMapAt(repoRoot) });
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
  const r = affected({ files, packages, readersOf: gitReaders, io: fsIo(repoRoot), hostMap: hostMapAt(repoRoot) });
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
