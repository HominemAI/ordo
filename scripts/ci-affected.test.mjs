// Tests for scripts/ci-affected.mjs: changed paths in, jobs out.
//
//   node scripts/ci-affected.test.mjs
//
// A small made-up workspace, so the answers do not move when a real
// crate's dependencies do. Then the closure check on the real ordo-host
// test map (.config/ci-test-map.json). CI runs this in the `audit` job.

import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { affected, checkMap, outputs, prefixes, testFilter } from './ci-affected.mjs';

const packages = [
  { name: 'ordo-syntax', dir: 'crates/ordo-syntax', deps: [] },
  {
    name: 'ordo-fn',
    dir: 'crates/ordo-fn',
    deps: [{ name: 'ordo-syntax', kind: 'normal' }],
    tests: [
      { name: 'run', path: 'tests/run.rs' },
      { name: 'parse', path: 'tests/parse.rs' },
    ],
  },
  { name: 'ordo-blob', dir: 'crates/ordo-blob', deps: [] },
  { name: 'ordo-gen', dir: 'crates/ordo-gen', deps: [{ name: 'ordo-syntax', kind: 'normal' }] },
  {
    name: 'ordo-cli',
    dir: 'crates/ordo-cli',
    deps: [
      { name: 'ordo-fn', kind: 'normal' },
      { name: 'ordo-gen', kind: 'normal' },
    ],
  },
  {
    name: 'ordo-host',
    dir: 'crates/ordo-host',
    deps: [{ name: 'ordo-fn', kind: 'normal' }],
    tests: ['usage', 'functions', 'notes_app', 'mail_api', 'authz_matrix', 'upgrade_continuity', 'old_name'].map((name) => ({
      name,
      path: `tests/${name}.rs`,
    })),
  },
  // Depends on ordo-host: runs whole whenever ordo-host's code changes.
  { name: 'ordo-console', dir: 'crates/ordo-console', deps: [{ name: 'ordo-host', kind: 'normal' }] },
  { name: 'ordo-identity-service', dir: 'crates/ordo-identity-service', deps: [] },
  // Dev-depends on ordo-host: its tests run when ordo-host's code changes,
  // and nothing follows from it.
  { name: 'ordo-conformance', dir: 'crates/ordo-conformance', deps: [{ name: 'ordo-host', kind: 'dev' }] },
  { name: 'ordo-playground', dir: 'crates/ordo-playground', deps: [{ name: 'ordo-conformance', kind: 'normal' }] },
];

// Who names which path, as `git grep` would find it.
const READERS = {
  'docs/ai-guide.md': [{ file: 'crates/ordo-cli/tests/ai_guide_check.rs', include: false }],
  'skills/ordo/react.md': [{ file: 'crates/ordo-cli/tests/ai_guide_check.rs', include: true }],
  'docs/wire.md': [{ file: 'crates/ordo-fn/src/wire.rs', include: true }],
};
const readersOf = (p) => READERS[p] ?? [];

// The files testBinaries reads.
const FILES = {
  'crates/ordo-fn/tests/run.rs': 'mod support;\n',
  'crates/ordo-fn/tests/parse.rs': '#[test] fn p() {}\n',
  'crates/ordo-fn/tests/support/mod.rs': '',
  'crates/ordo-fn/tests/golden/a.rs': '// named by nobody\n',
  'crates/ordo-host/tests/usage.rs': '',
  'crates/ordo-host/tests/functions.rs': '',
  'crates/ordo-host/tests/notes_app.rs': 'mod mail_support;\n',
  'crates/ordo-host/tests/mail_api.rs': 'mod mail_support;\n',
  'crates/ordo-host/tests/authz_matrix.rs': 'mod authz_support;\n',
  'crates/ordo-host/tests/upgrade_continuity.rs': 'join("tests/upgrade_fixture")\n',
  // Pulls usage.rs in by path.
  'crates/ordo-host/tests/old_name.rs': '#[path = "usage.rs"] mod usage;\n',
  'crates/ordo-host/tests/mail_support/mod.rs': '',
  'crates/ordo-host/tests/authz_support/mod.rs': '#[path = "../mail_support/mod.rs"] mod mail_support;\n',
};
const io = {
  read: (p) => FILES[p] ?? null,
  list: (d) => Object.keys(FILES).filter((f) => f.startsWith(d + '/')).sort(),
};

const P = 'crates/ordo-host/src/';
const hostMap = {
  files: {
    [`${P}lib.rs`]: 'all',
    [`${P}functions.rs`]: ['functions', 'notes_app'],
    [`${P}tickets.rs`]: ['usage'],
  },
};

const run = (...files) => outputs(affected({ files, packages, readersOf, io, hostMap }));

const cases = [
  {
    name: 'docs only: nothing runs',
    files: ['docs/howto-e2e.md', 'AGENTS.md', 'docs/board.jsonl'],
    want: { full: 'false', crates: '', rest: 'false', host: 'false', clients: '', notes_e2e: 'false' },
  },
  {
    name: 'a doc a test reads: that crate alone',
    files: ['docs/ai-guide.md'],
    want: { crates: ' ordo-cli ', rest: 'true', host: 'false', upgrade: 'false', notes_e2e: 'false' },
  },
  {
    name: 'a doc compiled into a crate: its dependents too',
    files: ['docs/wire.md'],
    want: { crates: ' ordo-cli ordo-conformance ordo-console ordo-fn ordo-host ', host: 'true', notes_e2e: 'true' },
  },
  {
    name: 'a skill file: the skill examples and the ai guide test',
    files: ['skills/ordo/react.md'],
    want: { crates: ' ordo-cli ', clients: ' ordo-react ' },
  },
  {
    name: 'ordo-fn code: it and every dependent, dev-dependents once',
    files: ['crates/ordo-fn/src/lib.rs'],
    want: {
      crates: ' ordo-cli ordo-conformance ordo-console ordo-fn ordo-host ',
      host: 'true',
      upgrade: 'true',
      authz: 'true',
      notes_e2e: 'true',
      console_e2e: 'true',
      identity_e2e: 'false',
    },
  },
  {
    name: "a crate's test file: that crate alone",
    files: ['crates/ordo-fn/tests/run.rs'],
    want: { crates: ' ordo-fn ', host: 'false', notes_e2e: 'false', upgrade: 'false' },
  },
  {
    name: 'ordo-blob: the S3 contract',
    files: ['crates/ordo-blob/src/s3.rs'],
    want: { crates: ' ordo-blob ', s3: 'true', host: 'false' },
  },
  {
    name: 'ordo-gen: every client, for the generated halves',
    files: ['crates/ordo-gen/src/ts.rs'],
    want: { crates: ' ordo-cli ordo-gen ', clients: ' ordo-cdk ordo-react ordo-sdk ' },
  },
  {
    name: 'clients/ordo-sdk: the SDK, ordo-react, every example, notes e2e',
    files: ['clients/ordo-sdk/src/index.ts'],
    want: {
      crates: ' ordo-host ',
      clients: ' ordo-react ordo-sdk ',
      examples: ' notes notes-v2 shopfront teamspace ',
      notes_e2e: 'true',
      console_e2e: 'false',
      audit: 'false',
    },
  },
  {
    name: 'clients/ordo-cdk: that client alone',
    files: ['clients/ordo-cdk/src/synth.ts'],
    want: { crates: '', clients: ' ordo-cdk ', examples: '', notes_e2e: 'false' },
  },
  {
    name: 'the console page: console e2e',
    files: ['clients/ordo-console-web/src/app.ts'],
    want: { console_e2e: 'true', clients: '', crates: '' },
  },
  {
    name: 'an example: its job and its e2e',
    files: ['examples/notes-v2/web/src/App.tsx'],
    want: { examples: ' notes-v2 ', notes_e2e: 'true', crates: '' },
  },
  {
    name: 'teamspace: its job, no e2e',
    files: ['examples/teamspace/teamspace.ordo'],
    want: { examples: ' teamspace ', notes_e2e: 'false' },
  },
  {
    name: 'a lockfile: the audit',
    files: ['clients/ordo-cdk/package-lock.json'],
    want: { audit: 'true', clients: ' ordo-cdk ' },
  },
  {
    name: 'the identity e2e folder',
    files: ['crates/ordo-identity-service/e2e/tests/signin.spec.ts'],
    want: { identity_e2e: 'true', crates: ' ordo-identity-service ', console_e2e: 'false' },
  },
  {
    name: 'the upgrade script',
    files: ['scripts/upgrade-continuity.sh'],
    want: { upgrade: 'true', rest: 'false' },
  },

  // Rule 6: a test file runs only its binary.
  {
    name: 'one ordo-host test file: that binary alone, no unit or doc tests',
    files: ['crates/ordo-host/tests/functions.rs'],
    want: {
      crates: ' ordo-host ',
      test_filter: 'binary_id(ordo-host::functions)',
      tests_only: ' ordo-host ',
      host: 'true',
      upgrade: 'false',
      authz: 'false',
      notes_e2e: 'false',
    },
  },
  {
    name: 'a test file another pulls in by #[path]: both binaries',
    files: ['crates/ordo-host/tests/usage.rs'],
    want: { test_filter: 'binary_id(ordo-host::old_name) | binary_id(ordo-host::usage)' },
  },
  {
    name: 'another crate: its test file alone',
    files: ['crates/ordo-fn/tests/parse.rs'],
    want: { crates: ' ordo-fn ', test_filter: 'binary_id(ordo-fn::parse)', tests_only: ' ordo-fn ', host: 'false' },
  },
  {
    name: 'a helper: the binaries that name it, through other helpers too',
    files: ['crates/ordo-host/tests/mail_support/mod.rs'],
    want: {
      test_filter: 'binary_id(ordo-host::authz_matrix) | binary_id(ordo-host::mail_api) | binary_id(ordo-host::notes_app)',
      authz: 'true',
      upgrade: 'false',
    },
  },
  {
    name: 'a fixture directory a test names: that test, and the upgrade job',
    files: ['crates/ordo-host/tests/upgrade_fixture/v1.ordo'],
    want: { test_filter: 'binary_id(ordo-host::upgrade_continuity)', upgrade: 'true' },
  },
  {
    name: 'a helper no test names: every test binary of the crate',
    files: ['crates/ordo-fn/tests/golden/a.rs'],
    want: { crates: ' ordo-fn ', test_filter: '(package(ordo-fn) & kind(test))', tests_only: ' ordo-fn ' },
  },
  {
    name: 'two test files and a helper of one crate: the union',
    files: ['crates/ordo-fn/tests/parse.rs', 'crates/ordo-fn/tests/support/mod.rs'],
    want: { test_filter: 'binary_id(ordo-fn::parse) | binary_id(ordo-fn::run)' },
  },
  {
    name: "a test file and the crate's code: the whole crate",
    files: ['crates/ordo-fn/tests/parse.rs', 'crates/ordo-fn/src/lib.rs'],
    want: { test_filter: 'package(ordo-cli) | package(ordo-conformance) | package(ordo-console) | package(ordo-fn) | package(ordo-host)', tests_only: '' },
  },

  // Rule 7: ordo-host's code, through the map.
  {
    name: 'a mapped ordo-host file: its binaries and the unit tests; dependents whole',
    files: [`${P}functions.rs`],
    want: {
      crates: ' ordo-conformance ordo-console ordo-host ',
      test_filter: 'package(ordo-conformance) | package(ordo-console) | (package(ordo-host) & (kind(lib) | kind(bin))) | binary_id(ordo-host::functions) | binary_id(ordo-host::notes_app)',
      tests_only: '',
      upgrade: 'true',
      authz: 'false',
      notes_e2e: 'true',
    },
  },
  {
    name: 'a mapped file and a test file: the union',
    files: [`${P}tickets.rs`, 'crates/ordo-host/tests/functions.rs'],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | (package(ordo-host) & (kind(lib) | kind(bin))) | binary_id(ordo-host::functions) | binary_id(ordo-host::usage)' },
  },
  {
    name: 'a file mapped to "all": every ordo-host test',
    files: [`${P}lib.rs`],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | package(ordo-host)', authz: 'true' },
  },
  {
    name: 'an unmapped ordo-host file: every ordo-host test',
    files: [`${P}runtime.rs`],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | package(ordo-host)' },
  },
  {
    name: 'a mapped file beside an unmapped one: every ordo-host test',
    files: [`${P}functions.rs`, `${P}runtime.rs`],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | package(ordo-host)' },
  },
  {
    name: 'a mapped file and a helper no test names: every ordo-host test binary',
    files: [`${P}functions.rs`, 'crates/ordo-host/tests/nobody/x.rs'],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | (package(ordo-host) & (kind(lib) | kind(bin))) | (package(ordo-host) & kind(test))' },
  },
  {
    name: 'ordo-host reached through a dependency: whole, the map aside',
    files: [`${P}functions.rs`, 'crates/ordo-fn/src/lib.rs'],
    want: { test_filter: 'package(ordo-cli) | package(ordo-conformance) | package(ordo-console) | package(ordo-fn) | package(ordo-host)' },
  },
  {
    name: 'ordo-host build files: every ordo-host test',
    files: ['crates/ordo-host/build.rs'],
    want: { test_filter: 'package(ordo-conformance) | package(ordo-console) | package(ordo-host)' },
  },
  {
    name: 'other crates keep the crate rule',
    files: ['crates/ordo-fn/src/lib.rs'],
    want: { test_filter: 'package(ordo-cli) | package(ordo-conformance) | package(ordo-console) | package(ordo-fn) | package(ordo-host)', tests_only: '' },
  },
  {
    name: 'docs only: a filter that matches nothing',
    files: ['docs/howto-e2e.md'],
    want: { test_filter: 'none()', unit: 'false' },
  },
];

const everything = ['.config/ci-test-map.json', 'Cargo.lock', 'Cargo.toml', '.github/workflows/ci.yml', 'scripts/ci-test-shard.sh', '.config/nextest.toml', 'docker/Dockerfile', 'rust-toolchain.toml', 'crates/ordo-wasm/src/lib.rs', 'clients/ordo-new/x.ts', 'a-new-top-level-thing/x'];

let failed = 0;
const check = (name, got, want) => {
  for (const [k, v] of Object.entries(want)) {
    if (got[k] !== v) {
      failed++;
      console.log(`FAIL ${name}: ${k} is ${JSON.stringify(got[k])}, want ${JSON.stringify(v)}`);
      return;
    }
  }
  console.log(`ok   ${name}`);
};

for (const c of cases) check(c.name, run(...c.files), c.want);
for (const f of everything) {
  check(`${f}: everything`, run('docs/x.md', f), {
    full: 'true',
    test_filter: '',
    tests_only: '',
    unit: 'true',
    crates: 'all',
    host: 'true',
    console_e2e: 'true',
    identity_e2e: 'true',
    examples: ' notes notes-v2 shopfront teamspace ',
  });
}

assert.deepEqual(prefixes('crates/ordo-host/tests/x.rs'), [
  'crates/ordo-host/tests/x.rs',
  'crates/ordo-host/tests',
  'crates/ordo-host',
  'ordo-host/tests/x.rs',
  'ordo-host/tests',
]);
assert.deepEqual(prefixes('docs/x.md'), ['docs/x.md']);
console.log('ok   prefixes');

assert.equal(testFilter({ full: true }), '');
console.log('ok   testFilter');

// checkMap itself.
{
  const p = checkMap({ files: { 'a.rs': ['x'], 'gone.rs': 'all', 'b.rs': ['nope'], 'c.rs': [] } }, ['a.rs', 'b.rs', 'c.rs', 'new.rs'], ['x']);
  assert.equal(p.length, 4);
  assert.match(p.find((m) => m.includes('new.rs')), /Add "new\.rs": "all"/);
  assert.match(p.find((m) => m.includes('gone.rs')), /does not exist/);
  assert.match(p.find((m) => m.includes('nope')), /test binary nope, which does not exist/);
  assert.match(p.find((m) => m.includes('c.rs')), /not empty/);
  assert.deepEqual(checkMap({ files: { 'a.rs': 'all' } }, ['a.rs'], []), []);
  console.log('ok   checkMap');
}

// The closure check on the real map: every ordo-host source file is in
// it, and everything it names exists. Read from disk, so it needs no cargo.
{
  const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
  const host = join(repo, 'crates/ordo-host');
  const walk = (d) =>
    readdirSync(join(host, d), { withFileTypes: true }).flatMap((e) =>
      e.isDirectory() ? walk(`${d}/${e.name}`) : e.name.endsWith('.rs') ? [`crates/ordo-host/${d}/${e.name}`] : [],
    );
  const src = walk('src');
  // ordo-host declares no [[test]] targets: each tests/*.rs and
  // tests/*/main.rs is one binary.
  assert.doesNotMatch(
    readFileSync(join(host, 'Cargo.toml'), 'utf8'),
    /^\[\[test\]\]|^autotests/m,
    'ordo-host now declares its test targets: read them here',
  );
  const bins = readdirSync(join(host, 'tests'), { withFileTypes: true }).flatMap((e) => {
    if (e.isFile() && e.name.endsWith('.rs')) return [e.name.slice(0, -3)];
    if (e.isDirectory() && readdirSync(join(host, 'tests', e.name)).includes('main.rs')) return [e.name];
    return [];
  });
  const map = JSON.parse(readFileSync(join(repo, '.config/ci-test-map.json'), 'utf8'));
  const problems = checkMap(map, src, bins);
  if (problems.length) {
    failed += problems.length;
    for (const m of problems) console.log(`FAIL test map: ${m}`);
  } else console.log(`ok   test map: ${src.length} ordo-host source files, ${bins.length} test binaries`);
}

if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
