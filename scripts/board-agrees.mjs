#!/usr/bin/env node
// The board (docs/board.jsonl, written by scripts/board.py) and the checklist (docs/roadmap-checklist.md)
// must agree (owner ruling, 2026-09-18). Exit 1 if an open checklist item is
// not named on the board, or a row in a Done group points at an open item.
// Partial rows ("Landed: ..." outside the Done groups) are allowed.
import { readFileSync } from 'node:fs';
const items = readFileSync('docs/board.jsonl', 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l)),
  rows = items.filter(r => r.id);
const state = {};
for (const l of readFileSync('docs/roadmap-checklist.md', 'utf8').split('\n')) {
  const m = l.match(/^\s*- \[(.)\] \*\*(\S+)(?: ⚑)? · /); if (m) state[m[2]] = m[1]; }
const esc = s => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const text = rows.map(r => [r.ref, r.name, r.sub, r.explain, r.suggest, ...(r.asks || []).flatMap(a => [a.text, a.suggest || ''])].join(' ')).join('\n');
const problems = [];
for (const [id, s] of Object.entries(state))
  if (s === ' ' && !new RegExp('(^|[^0-9a-z-])' + esc(id) + '([^0-9a-z]|$)').test(text))
    problems.push(`open on the checklist, missing from the board: ${id}`);
let group = '';
for (const r of items) { if (r.group) { group = r.group; continue; }
  if (!/^Done/.test(group)) continue;
  // `18d · S4` names a finished PART of an open item, which is allowed.
  if (String(r.ref).includes('·')) continue;
  const ref = String(r.ref);
  for (const id of ref.match(/\b\d{1,2}(?:-\d+)?[a-z]?(?:-pre|-rev|′)?\b/g) || [])
    if (state[id] === ' ') problems.push(`board row "${r.id}" is Done but checklist ${id} is open`); }
console.log(`${Object.values(state).filter(s => s === ' ').length} open items, ${rows.length} rows, ${problems.length} disagreements`);
problems.forEach(p => console.log('  ' + p));
process.exit(problems.length ? 1 : 0);
