/**
 * ordo-react carries no copy of ordo-sdk's code (owner ruling, 2026-10-23;
 * audit-service-interfaces.md, fix order step 7).
 *
 * The test lists every name the SDK's hand-written halves (`core`, `auth`,
 * `replica`) export, then every name ordo-react declares in `src/` with its
 * own `export function | const | class | interface | type`. A name in both is
 * a copy. A re-export (`export { x } from '@hominemai/ordo-sdk/...'`) is not
 * a declaration and passes.
 *
 * Until step 7 a list of divergences (`OrdoRefusal` with `.refusal` against
 * `.detail`, two `decodeRefusal`s, two `checkRefine`s, the account-email
 * calls) was allowed here. It is empty now and stays empty.
 *
 * Three 0.3 names are the SDK's names for different things, so this
 * package's own were renamed and the old names kept as deprecated aliases
 * (`export { DescriptorClient as OrdoClient }`). {@link DEPRECATED} lists
 * them, and the test holds the list to the source.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const react = join(__dirname, '..', 'src');
const sdk = join(__dirname, '..', '..', 'ordo-sdk', 'src');

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? walk(p) : p.endsWith('.ts') || p.endsWith('.tsx') ? [p] : [];
  });
}

const DECL = /^export\s+(?:declare\s+)?(?:async\s+)?(?:abstract\s+)?(?:function\*?|const|class|interface|type|enum)\s+([A-Za-z_$][\w$]*)/gm;

function declared(files: string[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const f of files) {
    const text = readFileSync(f, 'utf8');
    for (const m of text.matchAll(DECL)) out.set(m[1]!, f);
  }
  return out;
}

/** `X as Y` inside an `export { ... }` with no `from`: a local name exported under another. */
function aliases(files: string[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const f of files) {
    const text = readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
    for (const block of text.matchAll(/export\s*\{([^}]*)\}\s*;/g)) {
      for (const m of block[1]!.matchAll(/(?:type\s+)?([A-Za-z_$][\w$]*)\s+as\s+([A-Za-z_$][\w$]*)/g)) {
        out.set(m[2]!, m[1]!);
      }
    }
  }
  return out;
}

/** The deprecated 0.3 names, each the new name it stands for. */
// Empty since the 0.3 aliases (OrdoClient, OrdoClientOptions, CallOptions) went, 2026-10-30.
const DEPRECATED: Record<string, string> = {};

describe('ordo-react carries no copy of an ordo-sdk export', () => {
  const sdkNames = declared(walk(sdk).filter((f) => !f.includes(`${join(sdk, 'generated')}`)));
  const ownNames = declared(walk(react));

  it('redefines nothing the SDK exports', () => {
    const copies = [...ownNames.keys()]
      .filter((n) => sdkNames.has(n))
      .map((n) => `${n} (in ${ownNames.get(n)!.replace(react, 'src')}, SDK ${sdkNames.get(n)!.replace(sdk, 'sdk/src')})`);
    expect(copies, 'import these from @hominemai/ordo-sdk instead of declaring them').toEqual([]);
  });

  it('exports an SDK name under its own meaning only as a listed deprecated alias', () => {
    const local = aliases(walk(react));
    const shadowing = [...local.keys()].filter((n) => sdkNames.has(n) && DEPRECATED[n] !== local.get(n));
    expect(shadowing, 'a new alias of an SDK name needs a line in DEPRECATED').toEqual([]);
    const stale = Object.entries(DEPRECATED).filter(([old, now]) => local.get(old) !== now);
    expect(stale, 'remove these from DEPRECATED').toEqual([]);
  });
});
