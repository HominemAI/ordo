#!/usr/bin/env node
// Did the sandbox bite, and where? Reads a finished trial's transcripts and
// lists every tool result that looks like a sandbox denial.
//
//   node runner/sandbox-check.mjs runs/<id>
//
// Claude Code has no command that runs one shell command in its sandbox, and
// the dry stub never starts `claude`, so a real trial is the only proof. Read
// the list twice: a denial on a path the build needed (a cache, a socket, a
// host) means the allow-list in lib/sandbox.mjs is too tight; no denial at
// all on a trial that tried to look around means the sandbox was not on.

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

export const DENIAL = /operation not permitted|sandbox|read-only file system|EPERM|EACCES|blocked by|not in the allowlist|ENOTFOUND|connection refused by proxy/i;

/** Tool-result texts in a stream-json transcript that match DENIAL. */
export function denials(text) {
  const out = [];
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    let ev;
    try {
      ev = JSON.parse(line);
    } catch {
      continue;
    }
    const content = ev?.message?.content;
    if (!Array.isArray(content)) continue;
    for (const c of content) {
      if (c?.type !== 'tool_result') continue;
      const t = typeof c.content === 'string' ? c.content : JSON.stringify(c.content);
      if (DENIAL.test(t)) out.push(t.slice(0, 300).replace(/\s+/g, ' '));
    }
  }
  return out;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const dir = process.argv[2];
  if (!dir) {
    console.error('usage: node runner/sandbox-check.mjs runs/<id>');
    process.exit(2);
  }
  const tdir = join(dir, 'transcripts');
  for (const f of readdirSync(tdir).filter((n) => n.endsWith('.jsonl')).sort()) {
    const hits = denials(readFileSync(join(tdir, f), 'utf8'));
    console.log(`${f}: ${hits.length} possible denial${hits.length === 1 ? '' : 's'}`);
    for (const h of hits) console.log(`  - ${h}`);
  }
}
