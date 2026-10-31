// One agent phase: `claude -p` in the workspace, its stream-json transcript
// saved line by line, its usage read back. Or, in a dry run, a stub that
// stands in for the agent and spends nothing.

import { spawn } from 'node:child_process';
import { createWriteStream, readFileSync, writeFileSync } from 'node:fs';
import { readUsage } from './usage.mjs';
import { track, untrack } from './live.mjs';

/** Tools the planning pass gets: read the folder, write the plan. */
export const PLAN_TOOLS = ['Read', 'Glob', 'Grep', 'Write'];
/** Tools the build gets without asking. Anything else is denied. */
export const BUILD_TOOLS = ['Bash', 'Read', 'Write', 'Edit', 'MultiEdit', 'Glob', 'Grep', 'TodoWrite', 'Task'];
/** Never: the stack's docs are the pinned ones in guidance/. */
export const DENIED_TOOLS = ['WebFetch', 'WebSearch'];

/**
 * The argv for one phase. The prompt goes on stdin, so the variadic tool
 * lists cannot swallow it. Nothing here depends on the stack: both stacks
 * run the same command but for the model, turns and resume.
 *
 * `--bare` would isolate more, but it takes only an API key, and the runs
 * use the owner's Claude Code plan. So each of the owner's customisations
 * is turned off by its own flag (and `isolatedEnv` below):
 */
export function claudeArgs({ model, maxTurns, tools, resume, settingsFile }) {
  if (!settingsFile) throw new Error('every phase needs the sandbox settings file');
  const args = [
    '-p',
    '--model', model,
    '--output-format', 'stream-json',
    '--verbose',
    '--max-turns', String(maxTurns),
    // Only the workspace's own settings files, and it is given none: no user
    // settings, so none of the owner's hooks, permissions or plugins.
    '--setting-sources', 'project,local',
    // The sandbox (lib/sandbox.mjs): the runner's file, which outranks the
    // workspace's and which the agent cannot write.
    '--settings', settingsFile,
    // CLAUDE.md files, skills, installed plugins, hooks, MCP servers, custom
    // commands and agents, output styles and workflows: all off. Auth, the
    // model, built-in tools and permissions work as usual.
    '--safe-mode',
    // No skills or slash commands, built-in ones included.
    '--disable-slash-commands',
    // MCP servers: exactly the empty list given here.
    '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
    '--no-chrome',
    '--permission-mode', 'dontAsk',
    '--allowedTools', ...tools,
    '--disallowedTools', ...DENIED_TOOLS,
  ];
  if (resume) args.push('--resume', resume);
  return args;
}

/** Set for every phase, on top of the flags. */
export const ISOLATION_ENV = {
  // No memory directory read or written, and no CLAUDE.md from anywhere.
  CLAUDE_CODE_DISABLE_AUTO_MEMORY: '1',
  CLAUDE_CODE_DISABLE_CLAUDE_MDS: '1',
  // The connectors on the owner's claude.ai account (mail, drive, docs).
  ENABLE_CLAUDEAI_MCP_SERVERS: 'false',
  // No update checks or other calls beside the model's.
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
};

/**
 * Inherited variables a phase must not see. A runner started from inside a
 * Claude Code session inherits that session's `CLAUDECODE` and
 * `CLAUDE_CODE_*`. An API key would bill the API instead of the owner's
 * plan. `CLAUDE_CONFIG_DIR` is kept: it says where the plan's sign-in lives.
 */
const DROPPED = (name) =>
  name === 'CLAUDECODE' ||
  (name.startsWith('CLAUDE_CODE_') && !(name in ISOLATION_ENV)) ||
  name === 'ANTHROPIC_API_KEY' ||
  name === 'ANTHROPIC_AUTH_TOKEN';

/** The environment for a phase: `base` less the dropped names, plus ISOLATION_ENV. */
export function isolatedEnv(base) {
  const env = {};
  const dropped = [];
  for (const [k, v] of Object.entries(base)) {
    if (DROPPED(k)) dropped.push(k);
    else env[k] = v;
  }
  return { env: { ...env, ...ISOLATION_ENV }, dropped: dropped.sort() };
}

const shq = (s) => (/^[A-Za-z0-9_./:=,@+-]+$/.test(s) ? s : `'${s.replaceAll("'", `'\\''`)}'`);

/**
 * What a phase runs, for the record and for `--dry`: the argv, the folder,
 * the variables set and dropped (names only, never values), and one line
 * a person can paste. The prompt goes on stdin from `prompt_file`.
 */
export function commandRecord({ bin, cwd, args, dropped, promptFile, extraEnv = {} }) {
  const all = { ...ISOLATION_ENV, ...extraEnv };
  const set = Object.entries(all).map(([k, v]) => `${k}=${v}`);
  const unset = dropped.map((k) => `-u ${k}`);
  const line = `cd ${shq(cwd)} && env ${[...unset, ...set].join(' ')} ${[bin, ...args].map(shq).join(' ')} < ${shq(promptFile)}`;
  return { bin, args, cwd, env_set: all, env_dropped: dropped, prompt_file: promptFile, line };
}

/**
 * Run one phase. Stops it at `deadline` (ms since epoch). Every stdout line
 * goes to `transcript` as it arrives; stderr to `transcript.stderr`.
 */
export function runClaude({ bin, cwd, env, args, prompt, transcript, deadline }) {
  return new Promise((resolve) => {
    const out = createWriteStream(transcript);
    const err = createWriteStream(`${transcript}.stderr`);
    const started = Date.now();
    const child = spawn(bin, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'], detached: true });
    track(child.pid);
    let timedOut = false;
    const left = Math.max(0, deadline - Date.now());
    const timer = setTimeout(() => {
      timedOut = true;
      try {
        process.kill(-child.pid, 'SIGTERM');
      } catch {}
      setTimeout(() => {
        try {
          process.kill(-child.pid, 'SIGKILL');
        } catch {}
      }, 10_000).unref();
    }, left);
    child.stdout.pipe(out);
    child.stderr.pipe(err);
    child.stdin.end(prompt);
    child.on('close', (code, signal) => {
      untrack(child.pid);
      clearTimeout(timer);
      out.end(() => {
        err.end();
        const usage = readUsage(readFileSync(transcript, 'utf8'));
        resolve({ exit: code, signal, timed_out: timedOut, started, ended: Date.now(), usage });
      });
    });
    child.on('error', (e) => {
      clearTimeout(timer);
      writeFileSync(`${transcript}.stderr`, String(e));
    });
  });
}

/** A dry phase: `work` stands in for the agent; nothing is spent. */
export async function runDry({ transcript, work, note }) {
  const started = Date.now();
  await work();
  const ended = Date.now();
  writeFileSync(transcript, `${JSON.stringify({ type: 'bench-dry', note, started, ended })}\n`);
  return { exit: 0, signal: null, timed_out: false, started, ended, usage: readUsage('') };
}
