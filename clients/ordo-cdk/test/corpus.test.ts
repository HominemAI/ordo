/**
 * **Leg B of the isomorphism loop.**
 *
 * `crates/ordo-gen/tests/isomorphism.rs` owns the argument; this file is the
 * half of it that needs a JavaScript runtime. For every `corpus/<name>.gen.ts`
 * — a corpus program machine-translated from its own parsed AST into builder
 * calls — it imports the module, takes the `synth()` output, and compares it
 * against the checked-in `corpus/<name>.synth.ordo`.
 *
 * Cargo then parses that same file and asserts it comes back as the original
 * tree. So the round trip is checked end to end by the two suites together,
 * and `cargo test --workspace` stays green on a machine with no node without
 * the property going unverified there.
 *
 * Regenerate the `.synth.ordo` files with `npm run gen:corpus` — which is
 * this suite with `ORDO_UPDATE_GOLDEN=1`, the same variable the Rust goldens
 * use, so one habit covers both.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const corpus = join(here, 'corpus');
const update = process.env['ORDO_UPDATE_GOLDEN'] !== undefined;

// `import.meta.glob` is resolved by vite at build time, so the module graph is
// static and the fixtures are typechecked with everything else — which is
// half the point of emitting them as TypeScript rather than as data.
const modules = import.meta.glob<{ ordo: string }>('./corpus/*.gen.ts');

const entries = Object.entries(modules).sort(([a], [b]) => a.localeCompare(b));

describe('the corpus synthesizes canonical .ordo', () => {
  it('has fixtures at all', () => {
    // Non-vacuity. A glob that matched nothing would make every assertion
    // below vacuously true and the suite would pass while proving nothing —
    // the failure mode a corpus exists to prevent.
    expect(entries.length).toBeGreaterThan(15);
  });

  for (const [path, load] of entries) {
    const name = path.replace('./corpus/', '').replace('.gen.ts', '');
    it(name, async () => {
      const mod = await load();
      const actual = mod.ordo;
      expect(typeof actual).toBe('string');
      expect(actual.length).toBeGreaterThan(0);

      const golden = join(corpus, `${name}.synth.ordo`);
      if (update) {
        mkdirSync(corpus, { recursive: true });
        writeFileSync(golden, actual);
        return;
      }
      expect(
        existsSync(golden),
        `${name}.synth.ordo is missing — run \`npm run gen:corpus\``,
      ).toBe(true);
      expect(readFileSync(golden, 'utf8')).toBe(actual);
    });
  }
});
