import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('external coverage rejects a missing or empty host profile before invoking the reporter', () => {
  const root = mkdtempSync(join(tmpdir(), 'ordo-e2e-coverage-'));
  try {
    for (const path of ['scripts', 'bin', 'profiles', 'examples/notes/e2e/.stack']) mkdirSync(join(root, path), { recursive: true });
    copyFileSync('scripts/e2e-coverage.sh', join(root, 'scripts/e2e-coverage.sh'));
    writeFileSync(join(root, 'bin/cargo'), '#!/bin/sh\nprintf "cargo %s\\n" "$*"\n', { mode: 0o755 });
    writeFileSync(join(root, 'examples/notes/e2e/.stack/host-pids'), '123\n456\n');
    const run = () => spawnSync('bash', [join(root, 'scripts/e2e-coverage.sh'), 'report', 'notes'], {
      encoding: 'utf8',
      env: { ...process.env, PATH: `${root}/bin:${process.env.PATH}`, LLVM_PROFILE_FILE: `${root}/profiles/ordo-%p-%m.profraw` },
    });
    // A profile from the CLI does not prove either host flushed its counters.
    writeFileSync(join(root, 'profiles/ordo-789-signature.profraw'), 'cli');
    assert.match(run().stderr, /No LLVM profile for host 123/);
    writeFileSync(join(root, 'profiles/ordo-123-signature.profraw'), 'host one');
    writeFileSync(join(root, 'profiles/ordo-456-signature.profraw'), '');
    assert.match(run().stderr, /No LLVM profile for host 456/);
    writeFileSync(join(root, 'profiles/ordo-456-signature.profraw'), 'host two');
    const result = run();
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /cargo llvm-cov report --lcov --output-path coverage-out\/lcov-notes.info/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
