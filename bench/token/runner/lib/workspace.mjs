// A trial's workspace: a fresh folder holding only the spec, the stack's
// guidance and the starter, under git so every phase is a commit.

import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync, chmodSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { execFileSync } from 'node:child_process';
import { BENCH, REPO } from './paths.mjs';
import { packOrdoClients } from './sdk-pack.mjs';

const GIT = ['-c', 'user.name=bench', '-c', 'user.email=bench@example.com', '-c', 'commit.gpgsign=false'];

export const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');

/** Every file under `dir`, relative, sorted. Skips node_modules and .git. */
export function filesUnder(dir) {
  const out = [];
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      if (name === 'node_modules' || name === '.git') continue;
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else out.push(relative(dir, p));
    }
  };
  walk(dir);
  return out.sort();
}

const copy = (from, to) =>
  cpSync(from, to, { recursive: true, filter: (src) => !/(^|\/)(node_modules|\.stack|\.git)(\/|$)/.test(relative(from, src)) });

/** The guidance files for a stack: [repository-relative source, name in guidance/]. */
export function guidanceFiles(stack) {
  if (stack === 'ordo') {
    return readFileSync(join(BENCH, 'guidance/ordo/MANIFEST'), 'utf8')
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith('#'))
      .map((p) => [join(REPO, p), p.split('/').pop()]);
  }
  const dir = join(BENCH, 'guidance', stack);
  // SOURCE.md says where the files came from. It is the bench's note, not
  // the stack's guidance, so the agent does not get it.
  return readdirSync(dir)
    .filter((f) => f.endsWith('.md') && f !== 'SOURCE.md')
    .sort()
    .map((f) => [join(dir, f), f]);
}

/**
 * Why `parent` cannot hold a workspace, or null. Claude Code reads
 * CLAUDE.md files and `.claude/` folders from every folder above where it
 * runs, and a git repository above it adds its status to the prompt. So a
 * workspace must sit where no folder above it holds any of them: the
 * system temp folder does; the owner's home folder (`~/.claude`) does not.
 */
export function notIsolated(parent) {
  let d = realpathSync(parent);
  for (;;) {
    for (const name of ['.git', '.claude', 'CLAUDE.md', 'CLAUDE.local.md']) {
      if (existsSync(join(d, name))) return join(d, name);
    }
    const up = dirname(d);
    if (up === d) return null;
    d = up;
  }
}

/**
 * Build the workspace in `ws` (an empty folder) and commit it as "starter".
 * Answers the manifest: every file given to the agent, with its sha256.
 */
export function assemble(stack, ws) {
  mkdirSync(ws, { recursive: true });

  // The page: the common Vite React starter, then the stack's overlay.
  copy(join(BENCH, 'starter/common'), ws);
  copy(join(BENCH, 'starter', stack), ws);

  // The spec: the neutral app, then the stack's binding as `## Stack`.
  const spec = readFileSync(join(BENCH, 'spec/app.md'), 'utf8');
  const binding = readFileSync(join(BENCH, 'spec/bindings', `${stack}.md`), 'utf8');
  writeFileSync(join(ws, 'SPEC.md'), `${spec.trimEnd()}\n\n${binding}`);

  // The stack's own guidance.
  mkdirSync(join(ws, 'guidance'), { recursive: true });
  for (const [src, name] of guidanceFiles(stack)) cpSync(src, join(ws, 'guidance', name));

  // Ordo's SDK and React hooks arrive the way a published package does:
  // built tarballs that `npm install` puts in node_modules (web/package.json
  // names them). No source folder. They are not on npm yet.
  if (stack === 'ordo') {
    const pkgs = packOrdoClients();
    mkdirSync(join(ws, '.bench', 'pkgs'), { recursive: true });
    for (const [name, file] of Object.entries(pkgs)) cpSync(file, join(ws, '.bench', 'pkgs', `${name}.tgz`));
  }

  // The start script: the same up.sh the suite uses, copied in. The suite
  // runs its own pristine copy, so editing this one changes nothing scored.
  copy(join(BENCH, 'stacks', stack), join(ws, '.bench', 'stack'));
  writeFileSync(
    join(ws, 'run-stack.sh'),
    `#!/usr/bin/env bash
# Start this app the way the hidden suite does. Ctrl-C stops it.
# BENCH_NO_WEB=1 skips the page.
here=$(cd "$(dirname "$0")" && pwd)
exec bash "$here/.bench/stack/up.sh" "$here" "\${BENCH_STATE:-$here/.stack}"
`,
  );
  chmodSync(join(ws, 'run-stack.sh'), 0o755);
  writeFileSync(join(ws, '.gitignore'), 'node_modules/\n.stack/\ndist/\n');

  execFileSync('git', ['init', '-q', '-b', 'main'], { cwd: ws });
  commit(ws, 'starter');

  return filesUnder(ws).map((f) => ({ path: f, sha256: sha256(readFileSync(join(ws, f))) }));
}

/** Commit everything in the workspace (an empty commit if nothing changed). */
export function commit(ws, message) {
  execFileSync('git', [...GIT, 'add', '-A'], { cwd: ws });
  execFileSync('git', [...GIT, 'commit', '-q', '--allow-empty', '--no-verify', '-m', message], { cwd: ws });
}

/** The whole history as one file: `git clone <bundle>` gives it back. */
export function bundle(ws, file) {
  execFileSync('git', ['bundle', 'create', file, '--all'], { cwd: ws, stdio: 'ignore' });
}

/** Copy a reference build over the workspace (dry runs). */
export function copyReference(stack, ws) {
  const from = join(BENCH, 'reference', stack);
  if (!existsSync(from)) throw new Error(`no reference build at ${from}`);
  copy(from, ws);
}
