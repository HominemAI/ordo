import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The package ships as a library: React and `@hominemai/ordo-sdk` are peers,
// and nothing else is bundled because nothing else is depended on. Tests and
// type checks read the SDK from this repository's source; a build leaves it
// external, so the package imports the installed one.
const sdk = fileURLToPath(new URL('../ordo-sdk/src', import.meta.url));
const entry = new URL('./src/index.ts', import.meta.url).pathname;

export default defineConfig(({ command }) => ({
  resolve: command === 'build' ? {} : { alias: [{ find: '@hominemai/ordo-sdk', replacement: sdk }] },
  build: {
    lib: { entry, formats: ['es'], fileName: () => 'index.js' },
    rollupOptions: { external: ['react', 'react/jsx-runtime', /^@hominemai\/ordo-sdk(\/|$)/] },
    sourcemap: true,
    emptyOutDir: true,
  },
  test: {
    globals: true,
    // Every test in this package runs against the golden descriptor and a
    // mock `fetch`. No DOM, no server, no network.
    environment: 'node',
    include: ['test/**/*.test.ts'],
  },
}));
