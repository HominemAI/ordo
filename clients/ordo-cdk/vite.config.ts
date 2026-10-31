import { defineConfig } from 'vitest/config';

// The package ships as a library and depends on nothing: it builds an IR and
// prints text.
const entry = new URL('./src/index.ts', import.meta.url).pathname;

export default defineConfig({
  build: {
    lib: { entry, formats: ['es'], fileName: () => 'index.js' },
    sourcemap: true,
    emptyOutDir: true,
  },
  test: {
    globals: true,
    environment: 'node',
    // `test/corpus/*.gen.ts` are generated fixtures, not tests; the corpus
    // suite imports them.
    include: ['test/**/*.test.ts'],
  },
});
