#!/usr/bin/env node
// Typecheck every ```ts / ```tsx block in skills/ordo/*.md.
//
// Agents copy these examples, so each import path, function, field and type
// must be real. Each block is written as its own module in a temp dir and
// `tsc --noEmit` runs over all of them (strict, jsx react-jsx, bundler
// resolution) against the real sources of clients/ordo-sdk and
// clients/ordo-react (mapped with `paths`, no build needed). Errors print as
// `skills/ordo/<file>:<line>`, the line in the markdown file.
//
// Needs `npm ci` in clients/ordo-react (typescript, react types).
//
// A block that is deliberately not compilable (signature listings, pseudo
// code) is marked with an info string `ts skip` or `tsx skip`. Use it for real
// pseudo-code only.
//
// Names a fragment uses but does not declare (DOOR, space, token, ...) come
// from PRELUDE below. The generated surface (`./generated/ops.js`) and the
// app files the docs refer to (`./auth`, `./interface.json`) are stub
// modules in STUBS, typed like the real generated code.
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const skillDir = join(root, 'skills/ordo');
const sdkSrc = join(root, 'clients/ordo-sdk/src');
const reactSrc = join(root, 'clients/ordo-react/src');
const reactMods = join(root, 'clients/ordo-react/node_modules');
const tsc = join(reactMods, 'typescript/bin/tsc');
if (!existsSync(tsc)) {
  console.error('typescript not found: run `npm ci` in clients/ordo-react first');
  process.exit(2);
}

const PRELUDE = `
import type { PasskeySession } from '@hominemai/ordo-sdk/auth/passkey.js';
import type { ReactElement } from 'react';
declare global {
  const DOOR: string;
  const space: string;
  const member: string;
  const project: string;
  const body: string;
  const newName: string;
  const p: { id: string; rev: number };
  const session: PasskeySession;
  function showLink(url: string): void;
  function SignIn(props: Record<string, never>): ReactElement;
  function Workspace(props: { space: string }): ReactElement;
}
export {};
`;

const STUBS = {
  // Shaped like \`ordo gen ts\` output for a notes app.
  'generated/ops.ts': `
import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import type { RefusalClass } from '@hominemai/ordo-sdk/core/refusal.js';
import type { ReadCall, WriteCall } from '@hominemai/ordo-sdk/core/call.js';
export interface ProjectNotesRow { id: string; body: string; pinned: boolean; posted: string }
export interface ProjectNotesFields { pinned: import('@hominemai/ordo-sdk/core/refine.js').OpenField<'pinned', boolean>; posted: import('@hominemai/ordo-sdk/core/refine.js').OpenField<'posted', string> }
export interface ProjectNotesParams { project: string }
export declare function projectNotes(client: OrdoClient, params: ProjectNotesParams): ReadCall<ProjectNotesRow, RefusalClass, ProjectNotesFields>;
export declare function addNote(client: OrdoClient, params: { project: string; body: string; today: string }): WriteCall;
export declare function renameProject(client: OrdoClient, params: { id: string; rev: number; name: string }): WriteCall;
export declare function setLead(client: OrdoClient, params: { id: string; lead: string | null }): WriteCall;
`,
  // The app's one session object, as signin.md has the app write it.
  'auth.ts': `
import { createOrdoAuth } from '@hominemai/ordo-react';
export const DOOR = 'http://127.0.0.1:4000';
export const auth = createOrdoAuth({ baseUrl: DOOR });
`,
  'interface.json': '{}',
};

// Extract blocks.
const blocks = [];
for (const f of readdirSync(skillDir).filter((n) => n.endsWith('.md')).sort()) {
  const lines = readFileSync(join(skillDir, f), 'utf8').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const m = /^```(tsx?)(?:\s+(\S+))?\s*$/.exec(lines[i]);
    if (!m) continue;
    const start = i + 2; // 1-based line of the first code line
    const code = [];
    for (i++; i < lines.length && !lines[i].startsWith('```'); i++) code.push(lines[i]);
    blocks.push({ file: f, start, lang: m[1], skip: m[2] === 'skip', code: code.join('\n') });
  }
}

const tmp = mkdtempSync(join(tmpdir(), 'skill-examples-'));
for (const [p, text] of Object.entries(STUBS)) {
  mkdirSync(dirname(join(tmp, 'blocks', p)), { recursive: true });
  writeFileSync(join(tmp, 'blocks', p), text);
}
writeFileSync(join(tmp, 'blocks', 'prelude.ts'), PRELUDE);
const checked = [];
const skipped = [];
for (const b of blocks) {
  if (b.skip) {
    skipped.push(b);
    continue;
  }
  const name = `${b.file.replace(/\.md$/, '')}__L${b.start}.${b.lang}`;
  // Appended, not prepended, so a line in the file is the line in the block.
  writeFileSync(join(tmp, 'blocks', name), `${b.code}\nexport {};\n`);
  checked.push({ name, b });
}
writeFileSync(
  join(tmp, 'tsconfig.json'),
  JSON.stringify({
    compilerOptions: {
      target: 'ES2022', lib: ['ES2022', 'DOM', 'DOM.Iterable'], module: 'ESNext',
      moduleResolution: 'Bundler', jsx: 'react-jsx', strict: true, noEmit: true,
      skipLibCheck: true, resolveJsonModule: true, esModuleInterop: true,
      types: [], baseUrl: tmp,
      paths: {
        '@hominemai/ordo-react': [join(reactSrc, 'index.ts')],
        '@hominemai/ordo-sdk': [join(sdkSrc, 'index.ts')],
        '@hominemai/ordo-sdk/*': [join(sdkSrc, '*')],
        react: [join(reactMods, '@types/react')],
        'react/*': [join(reactMods, '@types/react/*')],
      },
    },
    include: ['blocks/**/*'],
  }),
);

const r = spawnSync(process.execPath, [tsc, '-p', join(tmp, 'tsconfig.json'), '--pretty', 'false'], { encoding: 'utf8' });
const out = (r.stdout ?? '') + (r.stderr ?? '');
let failed = 0;
for (const line of out.split('\n')) {
  const m = /^(?:.*\/)?(?:blocks\/)?([a-z-]+)__L(\d+)\.tsx?\((\d+),\d+\): (.*)$/.exec(line);
  if (m) {
    failed++;
    console.error(`skills/ordo/${m[1]}.md:${Number(m[2]) + Number(m[3]) - 1}: ${m[4]}`);
  } else if (/error TS/.test(line)) {
    failed++;
    console.error(line);
  }
}
console.log(`${checked.length} blocks checked, ${skipped.length} skipped`);
for (const b of skipped) console.log(`  skipped skills/ordo/${b.file}:${b.start - 1}`);
if (failed) {
  console.error(`${failed} error(s)`);
  process.exit(1);
}
