import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// The page imports the SDK from the repository, not from npm: the generated
// half belongs to one program, and this app carries its own (`src/generated`,
// written by `npm run gen`). The alias lets that generated code reach the
// SDK's hand-written core by the same path everywhere.
const sdk = fileURLToPath(new URL('../../../clients/ordo-sdk/src', import.meta.url));
const hooks = fileURLToPath(new URL('../../../clients/ordo-react/src', import.meta.url));

// One build serves three origins (sign-in, shop, staff console). The ports
// are fixed by `dev.mjs` or the `dev:*` scripts, because the passkey relying
// party and the door's origin lists name each origin exactly.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      { find: '@hominemai/ordo-react', replacement: `${hooks}/index.ts` },
      { find: '@hominemai/ordo-sdk', replacement: sdk },
    ],
    dedupe: ['react', 'react-dom'],
  },
  server: {
    host: 'localhost',
    strictPort: true,
  },
  test: {
    globals: true,
    environment: 'node',
    include: ['test/**/*.test.{ts,tsx}'],
  },
});
