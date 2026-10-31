// Ordo's two client packages, built and packed like a published release.
// They are not on npm yet, so a trial's workspace gets them as tarballs and
// `npm install` unpacks them into node_modules: built `dist/` JavaScript and
// `.d.ts`, no source folder. Built from this repository at the run's commit,
// once per distinct source, and cached in the system temp folder.

import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { REPO } from './paths.mjs';

const PKGS = ['ordo-sdk', 'ordo-react'];
const SKIP = /(^|\/)(node_modules|dist)(\/|$)/;

function sourceHash() {
  const h = createHash('sha256');
  for (const pkg of PKGS) {
    const root = join(REPO, 'clients', pkg);
    const walk = (d) => {
      for (const name of readdirSync(d).sort()) {
        const p = join(d, name);
        if (SKIP.test(relative(root, p))) continue;
        if (statSync(p).isDirectory()) walk(p);
        else h.update(relative(root, p)).update(readFileSync(p));
      }
    };
    walk(root);
  }
  return h.digest('hex').slice(0, 16);
}

const run = (args, cwd) => execFileSync('npm', args, { cwd, stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' });

/** Answers { 'ordo-sdk': <tgz path>, 'ordo-react': <tgz path> }. */
export function packOrdoClients() {
  const cache = join(tmpdir(), `ordo-bench-clients-${sourceHash()}`);
  const out = Object.fromEntries(PKGS.map((p) => [p, join(cache, `${p}.tgz`)]));
  if (PKGS.every((p) => existsSync(out[p]))) return out;

  const tmp = mkdtempSync(join(tmpdir(), 'ordo-bench-build-'));
  try {
    mkdirSync(cache, { recursive: true });
    // Side by side, because ordo-react's dev dependency is file:../ordo-sdk.
    for (const pkg of PKGS) {
      const from = join(REPO, 'clients', pkg);
      const dir = join(tmp, pkg);
      cpSync(from, dir, { recursive: true, filter: (s) => !SKIP.test(relative(from, s)) });
      run(['ci', '--no-audit', '--no-fund'], dir);
      run(['run', 'build'], dir);
      const name = run(['pack', '--pack-destination', tmp, '--silent'], dir).trim().split('\n').pop();
      renameSync(join(tmp, name), out[pkg]);
    }
  } catch (e) {
    rmSync(cache, { recursive: true, force: true });
    throw new Error(`could not build the Ordo client packages: ${e.stderr || e.message}`);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  return out;
}
