#!/usr/bin/env node
// Discovery, not reachability proof. Runtime registration must be reviewed separately.
import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs';
import { join, relative } from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
export const hash = text => createHash('sha256').update(text).digest('hex');
export function sourceFiles(root) {
  const out = [];
  function walk(dir) {
    for (const item of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, item.name);
      if (item.isDirectory()) walk(p);
      else if (item.isFile() && p.endsWith('.rs')) out.push(relative(root, p).replaceAll('\\', '/'));
    }
  }
  for (const item of readdirSync(join(root, 'crates'), { withFileTypes: true })) {
    if (item.isDirectory() && existsSync(join(root, 'crates', item.name, 'src'))) walk(join(root, 'crates', item.name, 'src'));
  }
  return out.sort();
}
// Mask comments and literals without shifting offsets. Nested Rust comments and
// raw strings matter: examples in documentation are not route registrations.
export function maskRust(source) {
  const chars = source.split('');
  const blank = (a, b) => { for (let k = a; k < b; k++) if (source[k] !== '\n') chars[k] = ' '; };
  for (let i = 0; i < source.length;) {
    if (source.startsWith('//', i)) {
      const end = source.indexOf('\n', i); const stop = end < 0 ? source.length : end;
      blank(i, stop); i = stop;
    } else if (source.startsWith('/*', i)) {
      let j = i + 2, depth = 1;
      while (j < source.length && depth) {
        if (source.startsWith('/*', j)) { depth++; j += 2; }
        else if (source.startsWith('*/', j)) { depth--; j += 2; }
        else j++;
      }
      if (depth) throw new Error('Unclosed Rust comment');
      blank(i, j); i = j;
    } else {
      const raw = source.slice(i).match(/^(?:br|r)(#*)"/);
      if (raw) {
        const endToken = '"' + raw[1];
        const end = source.indexOf(endToken, i + raw[0].length);
        if (end < 0) throw new Error('Unclosed raw string');
        const stop = end + endToken.length; blank(i, stop); i = stop;
      } else if (source[i] === '"') {
        let j = i + 1;
        while (j < source.length && source[j] !== '"') j += source[j] === '\\' ? 2 : 1;
        if (j >= source.length) throw new Error('Unclosed string');
        blank(i, j + 1); i = j + 1;
      } else if (source[i] === "'" && /^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'/.test(source.slice(i))) {
        const char = source.slice(i).match(/^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'/)[0];
        blank(i, i + char.length); i += char.length;
      } else i++;
    }
  }
  return chars.join('');
}
export function discover(file, source) {
  const masked = maskRust(source), entries = [];
  const add = (kind, offset, expression, extra = {}) => entries.push({
    kind, file, line: source.slice(0, offset).split('\n').length,
    expression: expression.replace(/\s+/g, ' ').trim(), ...extra,
  });
  for (const match of masked.matchAll(/\.\s*route\s*\(/g)) {
    const start = match.index + match[0].length;
    let depth = 1, end = start, comma = -1;
    for (; end < masked.length && depth; end++) {
      if (masked[end] === '(') depth++;
      if (masked[end] === ')') depth--;
      if (masked[end] === ',' && depth === 1 && comma < 0) comma = end;
    }
    if (depth) throw new Error(`Unparsed route in ${file}`);
    if (comma < 0) continue; // Not the two-argument HTTP router method.
    const path = source.slice(start, comma).trim();
    const handler = source.slice(comma + 1, end - 1).trim().replace(/,$/, '').trim();
    add('route-candidate', match.index, source.slice(start, end - 1), { path, handler });
  }
  for (const match of masked.matchAll(/\bpub(?:\([^)]*\))?\s+const\s+(\w+)\s*:\s*&(?:'static\s+)?str\s*=/g)) {
    const value = source.slice(match.index + match[0].length).match(/^\s*"(std\.[^"\n]+)"/);
    if (value) add('operation-candidate', match.index, value[1], { symbol: match[1] });
  }
  return entries;
}
export function inventory(root) {
  const files = sourceFiles(root).map(file => {
    const bytes = readFileSync(join(root, file));
    return { file, sha256: hash(bytes), lines: bytes.toString('utf8').split('\n').length };
  });
  const buildInputs = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'rust-toolchain', '.cargo/config.toml',
    ...readdirSync(join(root, 'crates')).map(name => `crates/${name}/Cargo.toml`)]
    .filter(file => existsSync(join(root, file))).sort().map(file => ({ file, sha256: hash(readFileSync(join(root, file))) }));
  const entries = files.flatMap(({ file }) => discover(file, readFileSync(join(root, file), 'utf8')));
  // Includes tests embedded in src and dynamic route expressions deliberately.
  // They remain candidates until caller/registration review resolves them.
  for (const e of entries) e.id = `${e.file}:${e.line}:${e.kind}`;
  return { version: 1, status: 'discovery-only', sourceDigest: hash(JSON.stringify({ files, buildInputs })), files, buildInputs, entries };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [mode, target = 'coverage/web-inventory.json'] = process.argv.slice(2);
    if (!['write', 'check'].includes(mode)) throw new Error('Usage: web-coverage-inventory.mjs write|check [inventory.json]');
    const result = inventory(process.cwd());
    const encoded = JSON.stringify(result, null, 2) + '\n';
    if (mode === 'write') writeFileSync(target, encoded);
    else if (readFileSync(target, 'utf8') !== encoded) throw new Error('Source inventory is stale; regenerate and review affected entries');
    console.log(`${result.files.length} Rust source files; ${result.entries.length} candidates. Registration and reachability remain unproven.`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
