// Tests for scripts/ci-affected.mjs: changed paths in, jobs out.
//
//   node scripts/ci-affected.test.mjs
//
// A small made-up workspace, so the answers do not move when a real
// crate's dependencies do. CI runs this in the `audit` job.

import assert from 'node:assert/strict';
import { affected, outputs, prefixes } from './ci-affected.mjs';

const packages = [
  { name: 'ordo-syntax', dir: 'crates/ordo-syntax', deps: [] },
  { name: 'ordo-fn', dir: 'crates/ordo-fn', deps: [{ name: 'ordo-syntax', kind: 'normal' }] },
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
  { name: 'ordo-host', dir: 'crates/ordo-host', deps: [{ name: 'ordo-fn', kind: 'normal' }] },
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

const run = (...files) => outputs(affected({ files, packages, readersOf }));

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
    want: { crates: ' ordo-cli ordo-conformance ordo-fn ordo-host ', host: 'true', notes_e2e: 'true' },
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
      crates: ' ordo-cli ordo-conformance ordo-fn ordo-host ',
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
];

const everything = ['Cargo.lock', 'Cargo.toml', '.github/workflows/ci.yml', 'scripts/ci-test-shard.sh', '.config/nextest.toml', 'docker/Dockerfile', 'rust-toolchain.toml', 'crates/ordo-wasm/src/lib.rs', 'clients/ordo-new/x.ts', 'a-new-top-level-thing/x'];

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

if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
