// The sandbox settings: closed to the owner's files, open to the run.
// Run: node --test runner/test/sandbox.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ALLOWED_DOMAINS, guardSettings, sandboxEnv, sandboxSettings, writeSettings } from '../lib/sandbox.mjs';

const home = '/Users/someone';
const mk = () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'sbx-')));
  const ws = join(root, 'ws');
  const scratch = join(root, 'bt');
  mkdirSync(ws);
  mkdirSync(scratch);
  return { root, ws, scratch };
};

test('the home folder is denied for reads and the workspace is writable and readable', () => {
  const { ws, scratch } = mk();
  const s = sandboxSettings({ ws, scratch, readable: ['/bin/ordo'], dockerHost: 'unix:///Users/someone/.docker/run/docker.sock', home });
  const fs = s.sandbox.filesystem;
  assert.ok(fs.denyRead.includes(home), '~ denied');
  assert.ok(fs.denyRead.includes('/Users'));
  assert.ok(fs.denyRead.includes('/private/tmp'));
  // ~/.ssh and ~/.claude sit under the denied home and nothing re-opens them.
  for (const secret of ['/Users/someone/.ssh', '/Users/someone/.claude']) {
    assert.ok(fs.denyRead.some((d) => secret.startsWith(`${d}/`)), secret);
    assert.ok(!fs.allowRead.some((a) => secret === a || secret.startsWith(`${a}/`) || a.startsWith(`${secret}/`)), `${secret} is not re-opened`);
    assert.ok(!fs.allowWrite.some((a) => secret.startsWith(`${a}/`)));
  }
  assert.ok(fs.allowWrite.includes(ws) && fs.allowRead.includes(ws));
  assert.ok(fs.allowWrite.includes(scratch));
  assert.ok(fs.allowRead.includes('/bin/ordo'));
  assert.deepEqual(s.permissions.deny.slice(0, 2), ['Read(//Users/someone/**)', 'Edit(//Users/someone/**)']);
  assert.equal(s.permissions.blockReadsOutsideWorkingDirectories, true);
});

test('no escape: the sandbox cannot be skipped and the network is a closed list', () => {
  const { ws, scratch } = mk();
  const s = sandboxSettings({ ws, scratch, dockerHost: null, home }).sandbox;
  assert.equal(s.enabled, true);
  assert.equal(s.failIfUnavailable, true);
  assert.equal(s.allowUnsandboxedCommands, false);
  assert.deepEqual(s.excludedCommands, []);
  assert.equal(s.network.strictAllowlist, true);
  assert.deepEqual(s.network.allowedDomains, ALLOWED_DOMAINS);
  assert.ok(ALLOWED_DOMAINS.includes('registry.npmjs.org'));
  assert.ok(ALLOWED_DOMAINS.every((d) => !d.includes('*')));
});

test('the Docker socket is the only socket opened, and only when there is one', () => {
  const { ws, scratch } = mk();
  const on = sandboxSettings({ ws, scratch, dockerHost: 'unix:///Users/someone/.docker/run/docker.sock', home });
  assert.deepEqual(on.sandbox.network.allowUnixSockets, ['/Users/someone/.docker/run/docker.sock']);
  const off = sandboxSettings({ ws, scratch, dockerHost: null, home });
  assert.deepEqual(off.sandbox.network.allowUnixSockets, []);
  assert.equal(sandboxEnv({ scratch, dockerHost: 'unix:///x.sock' }).DOCKER_HOST, 'unix:///x.sock');
});

test('a changed settings file, or a workspace settings file, is caught and removed', () => {
  const { root, ws, scratch } = mk();
  const file = join(root, 'sandbox-settings.json');
  const sha = writeSettings(file, sandboxSettings({ ws, scratch, home }));
  assert.deepEqual(guardSettings({ file, sha, ws }), []);
  mkdirSync(join(ws, '.claude'));
  writeFileSync(join(ws, '.claude', 'settings.local.json'), '{"sandbox":{"enabled":false}}');
  writeFileSync(file, '{}');
  const found = guardSettings({ file, sha, ws });
  assert.equal(found.length, 2);
  assert.throws(() => readFileSync(join(ws, '.claude', 'settings.local.json')));
});
