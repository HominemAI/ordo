// Each trial sees only its workspace: the flags, the environment and the
// folder check. Run: node --test runner/test/isolation.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { BUILD_TOOLS, ISOLATION_ENV, PLAN_TOOLS, claudeArgs, commandRecord, isolatedEnv } from '../lib/agent.mjs';
import { notIsolated } from '../lib/workspace.mjs';

test('every phase turns off the owner customisations', () => {
  for (const tools of [PLAN_TOOLS, BUILD_TOOLS]) {
    const a = claudeArgs({ model: 'm', maxTurns: 5, tools, resume: null, settingsFile: '/run/sandbox-settings.json' });
    assert.equal(a[a.indexOf('--settings') + 1], '/run/sandbox-settings.json');
    for (const flag of ['--safe-mode', '--disable-slash-commands', '--strict-mcp-config', '--no-chrome']) {
      assert.ok(a.includes(flag), flag);
    }
    assert.equal(a[a.indexOf('--setting-sources') + 1], 'project,local');
    assert.equal(a[a.indexOf('--mcp-config') + 1], '{"mcpServers":{}}');
    assert.ok(!a.includes('--bare'), '--bare needs an API key; the runs use the plan');
  }
});

test('the environment drops the session and API variables and sets the switches', () => {
  const { env, dropped } = isolatedEnv({
    PATH: '/bin',
    HOME: '/h',
    CLAUDECODE: '1',
    CLAUDE_CODE_ENTRYPOINT: 'cli',
    CLAUDE_CONFIG_DIR: '/h/.claude',
    ANTHROPIC_API_KEY: 'sk-test',
    CLAUDE_CODE_DISABLE_AUTO_MEMORY: '0',
  });
  assert.deepEqual(dropped, ['ANTHROPIC_API_KEY', 'CLAUDECODE', 'CLAUDE_CODE_ENTRYPOINT']);
  assert.equal(env.CLAUDE_CONFIG_DIR, '/h/.claude');
  assert.equal(env.PATH, '/bin');
  for (const [k, v] of Object.entries(ISOLATION_ENV)) assert.equal(env[k], v);
});

test('the recorded line names the variables, never their values', () => {
  const r = commandRecord({ bin: 'claude', cwd: '/w', args: ['-p', '--mcp-config', '{"a":1}'], dropped: ['ANTHROPIC_API_KEY'], promptFile: '/p.md' });
  assert.match(r.line, /-u ANTHROPIC_API_KEY/);
  assert.match(r.line, /'\{"a":1\}'/);
  assert.match(r.line, /< \/p\.md$/);
});

test('a folder under a repository or a .claude folder is refused', () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'iso-')));
  assert.equal(notIsolated(root), null);
  mkdirSync(join(root, 'a', 'b'), { recursive: true });
  writeFileSync(join(root, 'a', 'CLAUDE.md'), '');
  assert.equal(notIsolated(join(root, 'a', 'b')), join(root, 'a', 'CLAUDE.md'));
  assert.notEqual(notIsolated(homedir()), null);
});
