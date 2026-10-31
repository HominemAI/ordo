#!/usr/bin/env node
// One benchmark trial, start to finish. README.md says how to run it.
//
//   node runner/trial.mjs --stack ordo|supabase [--dry | --dry=broken]
//
// 1. A fresh workspace: SPEC.md, guidance/, the starter. Nothing else.
// 2. Plan: Fable reads the spec and guidance and writes PLAN.md.
// 3. Build: Sonnet builds from the plan.
// 4. Score: the hidden suite against the app, started by the bench's up.sh.
// 5. Repair: up to 3 rounds, each told only the failing test names.
//
// Caps over the agent's phases together: 150 turns, 60 minutes. A cap hit
// is a fail, and its tokens count.
//
// --dry stands a stub in for the agent: no plan, and the reference build
// copied in as the "build". --dry=broken copies it in with one label wrong,
// so the first round fails and the repair loop runs (the stub repair copies
// the reference again). Nothing is spent in a dry run.

import { execFileSync } from 'node:child_process';
import { appendFileSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir, platform, release, arch } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { BUILD_TOOLS, PLAN_TOOLS, claudeArgs, commandRecord, isolatedEnv, runClaude, runDry } from './lib/agent.mjs';
import { dockerHostOf, guardSettings, makeScratch, sandboxEnv, sandboxSettings, writeSettings } from './lib/sandbox.mjs';
import { BENCH, REPO } from './lib/paths.mjs';
import { stopAll } from './lib/live.mjs';
import { scoreRound, sweep } from './lib/suite.mjs';
import { add, total, zero } from './lib/usage.mjs';
import { assemble, bundle, commit, copyReference, guidanceFiles, notIsolated, sha256 } from './lib/workspace.mjs';

const { values: opt } = parseArgs({
  options: {
    stack: { type: 'string' },
    dry: { type: 'string' },
    out: { type: 'string', default: join(BENCH, 'runs') },
    work: { type: 'string', default: tmpdir() },
    'plan-model': { type: 'string', default: 'claude-fable-5-1' },
    'build-model': { type: 'string', default: 'claude-sonnet-5-5' },
    'max-turns': { type: 'string', default: '150' },
    minutes: { type: 'string', default: '60' },
    repairs: { type: 'string', default: '3' },
    'keep-workspace': { type: 'boolean', default: false },
    claude: { type: 'string', default: process.env['CLAUDE_BIN'] ?? 'claude' },
  },
  // `--dry` alone means a passing dry run.
  args: process.argv.slice(2).map((a) => (a === '--dry' ? '--dry=pass' : a)),
});

const STACKS = { ordo: 'Ordo', supabase: 'Supabase' };
const stack = opt.stack ?? '';
if (!(stack in STACKS)) {
  console.error('usage: node runner/trial.mjs --stack ordo|supabase [--dry | --dry=broken]');
  process.exit(2);
}
const dry = opt.dry ?? null;
if (dry && dry !== 'pass' && dry !== 'broken') {
  console.error('--dry takes no value, or =broken');
  process.exit(2);
}
const caps = { turns: Number(opt['max-turns']), minutes: Number(opt.minutes), repairs: Number(opt.repairs) };
const models = { plan: opt['plan-model'], build: opt['build-model'] };

// ── The machine ────────────────────────────────────────────────────────────

const which = (bin) => {
  try {
    return execFileSync('/usr/bin/which', [bin], { encoding: 'utf8' }).trim();
  } catch {
    return '';
  }
};
const quiet = (bin, args) => {
  try {
    return execFileSync(bin, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: 30_000 }).trim();
  } catch {
    return null;
  }
};

const env = { ...process.env };
const extraPath = [];
if (stack === 'ordo') {
  const ordo = resolve(env['ORDO'] ?? which('ordo'));
  const host = resolve(env['ORDO_HOST_BIN'] ?? which('ordo-host'));
  if (!existsSync(ordo) || !existsSync(host)) {
    console.error('set ORDO and ORDO_HOST_BIN to the ordo and ordo-host binaries, or put them on PATH');
    process.exit(2);
  }
  env['ORDO'] = ordo;
  env['ORDO_HOST_BIN'] = host;
  extraPath.push(dirname(ordo), dirname(host));
}
env['PATH'] = [...new Set([...extraPath, ...(env['PATH'] ?? '').split(delimiter)])].join(delimiter);
// A machine fault is not the agent's: refuse to start rather than feed it
// back as a failing round.
if (stack === 'supabase' && (!which('supabase') || quiet('docker', ['info']) === null)) {
  console.error('Supabase needs the supabase CLI on PATH and Docker running (docker info fails)');
  process.exit(2);
}
if (stack === 'ordo' && !existsSync(join(env['ORDO_PGBIN'] ?? '/opt/homebrew/opt/postgresql@17/bin', 'initdb'))) {
  console.error('Ordo needs PostgreSQL binaries: set ORDO_PGBIN to the folder holding initdb');
  process.exit(2);
}
if (!existsSync(join(BENCH, 'suite', 'node_modules', '@playwright', 'test'))) {
  console.error(`the hidden suite is not installed: run npm install in ${join(BENCH, 'suite')}`);
  process.exit(2);
}

const fileSha = (p) => (p && existsSync(p) ? sha256(readFileSync(p)) : null);
const versions = {
  claude: dry ? null : quiet(opt.claude, ['--version']),
  node: process.version,
  npm: quiet('npm', ['--version']),
  git_commit: quiet('git', ['-C', REPO, 'rev-parse', 'HEAD']),
  git_dirty: (quiet('git', ['-C', REPO, 'status', '--porcelain']) ?? '') !== '',
  playwright: JSON.parse(readFileSync(join(BENCH, 'suite/node_modules/@playwright/test/package.json'), 'utf8')).version,
  os: `${platform()} ${release()} ${arch()}`,
  ...(stack === 'ordo'
    ? { ordo_sha256: fileSha(env['ORDO']), ordo_host_sha256: fileSha(env['ORDO_HOST_BIN']) }
    : { supabase: quiet('supabase', ['--version']), docker: quiet('docker', ['version', '--format', '{{.Server.Version}}']) }),
};

// ── The trial ──────────────────────────────────────────────────────────────

const t0 = Date.now();
const stamp = new Date(t0).toISOString().replace(/[:.]/g, '-');
const id = `${stamp}-${stack}${dry ? (dry === 'broken' ? '-dry-broken' : '-dry') : ''}`;
const dir = join(resolve(opt.out), id);
mkdirSync(join(dir, 'transcripts'), { recursive: true });
// The agent must see only its workspace: nothing above it may hold a
// repository, a CLAUDE.md or a .claude folder.
const leak = notIsolated(resolve(opt.work));
if (leak) {
  console.error(`--work ${opt.work} is not isolated: ${leak} sits above it. Use a folder outside any repository and outside your home folder (the default is the system temp folder).`);
  process.exit(2);
}
const ws = mkdtempSync(join(resolve(opt.work), `bench-ws-${stack}-`));
const log = (msg) => console.log(`[${((Date.now() - t0) / 1000).toFixed(0).padStart(5)} s] ${msg}`);

// Stopped by hand: stop the agent and the stack with it. The trial has no
// record; its folder keeps what was written so far.
for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, async () => {
    console.error(`\n${sig}: stopping the agent and the stack`);
    await stopAll();
    sweep(stack, ws, env);
    process.exit(130);
  });
}

log(`trial ${id}`);
log(`workspace ${ws}`);
const manifest = assemble(stack, ws);
writeFileSync(join(dir, 'manifest.json'), JSON.stringify(manifest, null, 2));

const prompt = (name, vars) => {
  let text = readFileSync(join(BENCH, 'runner/prompts', `${name}.md`), 'utf8');
  for (const [k, v] of Object.entries(vars)) text = text.replaceAll(`{{${k}}}`, v);
  return text;
};

const deadline = t0 + caps.minutes * 60_000;
let turnsUsed = 0;
let capHit = null;
let buildSession = null;
const phases = [];
const rounds = [];

// The same environment for every phase of both stacks: the runner's, less
// the owner's Claude Code and API variables, plus the isolation switches.
const agent = isolatedEnv(env);

// The sandbox: one settings file per trial, written outside the workspace,
// the same rules for both stacks (lib/sandbox.mjs). Everything the agent may
// write is the workspace and this scratch folder.
const scratch = mkdtempSync(join(resolve(opt.work), 'bt-'));
makeScratch(scratch);
const dockerHost = dockerHostOf(env);
const extraEnv = sandboxEnv({ scratch, dockerHost });
agent.env = { ...agent.env, ...extraEnv };
const settingsFile = join(dir, 'sandbox-settings.json');
const readable = [env['ORDO'], env['ORDO_HOST_BIN'], env['ORDO_PGBIN'] ?? '/opt/homebrew/opt/postgresql@17/bin'].filter(Boolean);
const settingsSha = writeSettings(settingsFile, sandboxSettings({ ws, scratch, readable, dockerHost }));
log(`sandbox settings ${settingsFile}`);

async function phase(name, model, text, tools, resume, stub) {
  const transcript = join(dir, 'transcripts', `${name}.jsonl`);
  const promptFile = join(dir, 'transcripts', `${name}.prompt.md`);
  writeFileSync(promptFile, text);
  const guard = guardSettings({ file: settingsFile, sha: settingsSha, ws });
  if (guard.length) log(`${name}: ${guard.join('; ')}`);
  const args = claudeArgs({ model, maxTurns: Math.max(1, caps.turns - turnsUsed), tools, resume, settingsFile });
  const command = commandRecord({ bin: opt.claude, cwd: ws, args, dropped: agent.dropped, promptFile, extraEnv });
  log(`${name}: ${dry ? 'dry stub' : model}`);
  // A dry run prints the command a real one would run, so it can be checked
  // without spending anything.
  if (dry) log(`${name}: a real trial runs\n  ${command.line}`);
  const r = dry
    ? await runDry({ transcript, work: stub, note: `${name} (dry ${dry})` })
    : await runClaude({ bin: opt.claude, cwd: ws, env: agent.env, args, prompt: text, transcript, deadline });
  turnsUsed += r.usage.num_turns;
  if (r.timed_out) capHit = 'time';
  else if (r.usage.subtype === 'error_max_turns' || turnsUsed >= caps.turns) capHit = capHit ?? 'turns';
  commit(ws, `after ${name}`);
  const record = {
    name,
    model: dry ? null : model,
    started: new Date(r.started).toISOString(),
    ended: new Date(r.ended).toISOString(),
    wall_ms: r.ended - r.started,
    exit: r.exit,
    signal: r.signal,
    timed_out: r.timed_out,
    resumed: resume ?? null,
    // What ran, exactly; in a dry run, what a real trial would have run.
    command: { ...command, prompt_file: `transcripts/${name}.prompt.md`, ran: !dry },
    sandbox: { settings_file: 'sandbox-settings.json', sha256: settingsSha, tampering: guard },
    transcript: `transcripts/${name}.jsonl`,
    ...r.usage,
  };
  phases.push(record);
  log(`${name}: ${record.num_turns} turns, ${total(record.counts)} tokens${capHit ? `, cap hit: ${capHit}` : ''}`);
  return record;
}

const breakIt = () => {
  const app = join(ws, 'web/src/App.tsx');
  writeFileSync(app, readFileSync(app, 'utf8').replace('>Add task<', '>Add<').replace('Add task\n', 'Add\n'));
};

try {
  // 2. Plan.
  await phase('plan', models.plan, prompt('plan', { STACK_NAME: STACKS[stack] }), PLAN_TOOLS, null, async () =>
    writeFileSync(join(ws, 'PLAN.md'), '# Plan\n\nDry run: no plan was made.\n'),
  );
  if (existsSync(join(ws, 'PLAN.md'))) copyFileSync(join(ws, 'PLAN.md'), join(dir, 'PLAN.md'));

  // 3. Build.
  if (!capHit) {
    const b = await phase('build', models.build, prompt('build', { STACK_NAME: STACKS[stack] }), BUILD_TOOLS, null, async () => {
      copyReference(stack, ws);
      if (dry === 'broken') breakIt();
    });
    buildSession = b.session_id;
  }

  // 4 and 5. Score, then repair while it fails and caps allow.
  for (let round = 0; ; round++) {
    log(`round ${round}: scoring`);
    const r = await scoreRound({ stack, ws, dir: join(dir, 'tests', `round-${round}`), env });
    rounds.push({ round, ...r, started: new Date(r.started).toISOString(), ended: new Date(r.ended).toISOString() });
    log(`round ${round}: ${r.passed}/${r.total} passed, @perm ${r.perm_passed}/${r.perm_total}${r.stack_started ? '' : ` (${r.stack_error})`}`);
    if (r.pass || capHit || round >= caps.repairs || Date.now() >= deadline) break;
    const failing = r.failing.map((f) => `- ${f}`).join('\n');
    await phase(`repair-${round + 1}`, models.build, prompt('repair', { FAILING: failing }), BUILD_TOOLS, buildSession, async () =>
      copyReference(stack, ws),
    );
  }
} finally {
  // ── The record ───────────────────────────────────────────────────────────
  bundle(ws, join(dir, 'workspace.bundle'));
  const last = rounds[rounds.length - 1];
  let all = zero();
  let afterFail = zero();
  for (const p of phases) {
    all = add(all, p.counts);
    if (p.name.startsWith('repair-')) afterFail = add(afterFail, p.counts);
  }
  const record = {
    schema: 'bench.token.trial/1',
    id,
    stack,
    dry,
    started: new Date(t0).toISOString(),
    ended: new Date().toISOString(),
    wall_ms: Date.now() - t0,
    models: dry ? null : models,
    caps,
    cap_hit: capHit,
    versions,
    inputs: {
      spec_sha256: sha256(readFileSync(join(ws, 'SPEC.md'))),
      guidance: guidanceFiles(stack).map(([src, name]) => ({ name, sha256: fileSha(src) })),
      manifest: 'manifest.json',
    },
    phases,
    rounds,
    // A trial passes when its last round passes every test and no cap was hit.
    pass: !!last?.pass && !capHit,
    tests: last ? { passed: last.passed, total: last.total } : null,
    perm: last ? { passed: last.perm_passed, total: last.perm_total } : null,
    repair_rounds: Math.max(0, rounds.length - 1),
    turns: turnsUsed,
    tokens: { ...all, total: total(all) },
    tokens_after_first_failure: { ...afterFail, total: total(afterFail) },
    cost_usd_list_price: phases.reduce((s, p) => s + (p.cost_usd_list_price ?? 0), 0),
    artifacts: { plan: 'PLAN.md', bundle: 'workspace.bundle', transcripts: 'transcripts/', tests: 'tests/' },
  };
  writeFileSync(join(dir, 'record.json'), JSON.stringify(record, null, 2));
  appendFileSync(join(resolve(opt.out), 'results.jsonl'), `${JSON.stringify(record)}\n`);
  if (!opt['keep-workspace']) {
    rmSync(ws, { recursive: true, force: true });
    rmSync(scratch, { recursive: true, force: true });
  }
  log(`${record.pass ? 'PASS' : 'FAIL'}: ${record.tests ? `${record.tests.passed}/${record.tests.total}` : 'not scored'}, ${record.tokens.total} tokens. Record: ${dir}`);
}
