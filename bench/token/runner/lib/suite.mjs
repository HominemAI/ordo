// One scoring round: start the workspace's app with the bench's own up.sh,
// run the hidden suite against it, stop it, keep the logs.

import { spawn, execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BENCH } from './paths.mjs';
import { track, untrack } from './live.mjs';

/** Ports each stack's up.sh holds. Anything still on them is from an earlier run. */
export const PORTS = {
  ordo: [4710, 5310, 4725, 4726, 55510],
  supabase: [5310, 54321, 54322, 54324],
};

/** The name a failed start is fed back under, in place of test names. */
export const NO_START = 'the app did not start with run-stack.sh';

/**
 * Stop whatever the agent left running: processes started from the
 * workspace, anything on the stack's ports, and a local Supabase.
 */
export function sweep(stack, ws, env) {
  try {
    execFileSync('pkill', ['-f', ws], { stdio: 'ignore' });
  } catch {}
  if (stack === 'supabase') {
    try {
      execFileSync('supabase', ['stop', '--no-backup', '--project-id', 'bench-team-tasks'], { env, stdio: 'ignore', timeout: 120_000 });
    } catch {}
  }
  for (const port of PORTS[stack]) {
    let pids = '';
    try {
      pids = execFileSync('lsof', ['-ti', `tcp:${port}`, '-sTCP:LISTEN'], { encoding: 'utf8' }).trim();
    } catch {}
    for (const pid of pids.split('\n').filter(Boolean)) {
      try {
        process.kill(Number(pid), 'SIGKILL');
      } catch {}
    }
  }
}

function startStack({ stack, ws, state, env, log }) {
  return new Promise((resolve) => {
    const child = spawn('bash', [join(BENCH, 'stacks', stack, 'up.sh'), ws, state], {
      env,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    track(child.pid);
    child.on('close', () => untrack(child.pid));
    let text = '';
    let settled = false;
    const done = (ok, why) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ ok, why, child });
    };
    const take = (b) => {
      text += b.toString();
      writeFileSync(log, text);
      if (text.includes('stack: up.')) done(true, '');
    };
    child.stdout.on('data', take);
    child.stderr.on('data', take);
    child.on('close', () => done(false, text.trim().split('\n').slice(-3).join(' | ')));
    // A first `supabase start` pulls its images: allow 10 minutes.
    const timer = setTimeout(() => done(false, 'the stack did not come up in 10 minutes'), 600_000);
  });
}

async function stopStack(child) {
  if (child.exitCode !== null) return;
  try {
    process.kill(-child.pid, 'SIGINT');
  } catch {}
  const until = Date.now() + 90_000;
  while (child.exitCode === null && Date.now() < until) await new Promise((r) => setTimeout(r, 250));
  if (child.exitCode === null) {
    try {
      process.kill(-child.pid, 'SIGKILL');
    } catch {}
  }
}

/** Every test in a Playwright JSON report: its full name and status. */
export function testsIn(report) {
  const out = [];
  const walk = (suite, path) => {
    for (const spec of suite.specs ?? []) {
      for (const t of spec.tests ?? []) {
        const last = t.results?.[t.results.length - 1];
        const status = last?.status ?? 'skipped';
        out.push({ name: [...path, spec.title].filter(Boolean).join(' › '), status, ok: status === 'passed', perm: spec.title.includes('@perm') });
      }
    }
    for (const s of suite.suites ?? []) walk(s, [...path, s.title]);
  };
  for (const s of report.suites ?? []) walk(s, []);
  return out;
}

/**
 * Score the workspace once. `dir` gets: stack.log, the stack's own logs,
 * suite.log, results.json and the failure traces.
 */
export async function scoreRound({ stack, ws, dir, env }) {
  mkdirSync(dir, { recursive: true });
  const started = Date.now();
  // A short path: the stack keeps a Postgres data folder and sockets here.
  const state = mkdtempSync(join(tmpdir(), `bench-${stack}-`));
  sweep(stack, ws, env);
  const up = await startStack({ stack, ws, state, env, log: join(dir, 'stack.log') });
  let tests = [];
  let suiteExit = null;
  if (up.ok) {
    suiteExit = await new Promise((resolve) => {
      const child = spawn('npx', ['--no-install', 'playwright', 'test'], {
        cwd: join(BENCH, 'suite'),
        env: { ...env, BENCH_STACK: stack, BENCH_STACK_JSON: join(state, 'stack.json'), BENCH_REPORT_DIR: dir },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      let text = '';
      child.stdout.on('data', (b) => (text += b));
      child.stderr.on('data', (b) => (text += b));
      child.on('close', (code) => {
        writeFileSync(join(dir, 'suite.log'), text);
        resolve(code);
      });
    });
    const file = join(dir, 'results.json');
    if (existsSync(file)) tests = testsIn(JSON.parse(readFileSync(file, 'utf8')));
  }
  await stopStack(up.child);
  sweep(stack, ws, env);
  keepLogs(state, join(dir, 'stack'));
  rmSync(state, { recursive: true, force: true });

  const failing = up.ok ? tests.filter((t) => !t.ok).map((t) => t.name) : [NO_START];
  const perm = tests.filter((t) => t.perm);
  return {
    started,
    ended: Date.now(),
    stack_started: up.ok,
    stack_error: up.ok ? null : up.why,
    suite_exit: suiteExit,
    total: tests.length,
    passed: tests.filter((t) => t.ok).length,
    perm_total: perm.length,
    perm_passed: perm.filter((t) => t.ok).length,
    failing,
    // A run with no tests at all is a fail, whatever the exit code.
    pass: up.ok && tests.length > 0 && failing.length === 0,
  };
}

/** Logs and small JSON files only: no database, no built page. */
function keepLogs(from, to) {
  mkdirSync(to, { recursive: true });
  for (const name of readdirSync(from)) {
    const p = join(from, name);
    if (statSync(p).isFile() && name !== 'status.json' && /\.(log|json|env|err)$/.test(name)) cpSync(p, join(to, name));
  }
}
