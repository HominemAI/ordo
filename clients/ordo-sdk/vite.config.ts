import { readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The package ships as a library and depends on nothing: the door client is
// one `fetch` wrapper, and the generated half is data and types.
//
// Every module under `src/` is its own file in `dist/`, with the same path
// (`src/core/client.ts` becomes `dist/core/client.js`). Code that `ordo gen ts`
// emits imports `@hominemai/ordo-sdk/core/client.js` and its siblings by those
// paths, and `package.json` `exports` serves them, so an app outside the
// repository resolves them. One file per module also means a module imported
// through `.` and through a subpath is the same file, so `instanceof
// OrdoRefusal` holds across both.
const src = fileURLToPath(new URL('./src', import.meta.url));
const entries: Record<string, string> = {};
for (const dir of ['', 'auth', 'core', 'generated', 'replica']) {
  const prefix = dir ? `${dir}/` : '';
  for (const f of readdirSync(`${src}/${prefix}`)) {
    if (f.endsWith('.ts')) {
      entries[`${prefix}${f.slice(0, -3)}`] = `${src}/${prefix}${f}`;
    }
  }
}

export default defineConfig({
  build: {
    lib: { entry: entries, formats: ['es'] },
    rollupOptions: {
      output: {
        preserveModules: true,
        preserveModulesRoot: 'src',
        entryFileNames: '[name].js',
      },
    },
    sourcemap: true,
    emptyOutDir: true,
  },
  test: {
    globals: true,
    // Every test runs against the generated surface and a mock `fetch`.
    // No DOM, no server, no network.
    environment: 'node',
    include: ['test/**/*.test.ts'],
  },
});
