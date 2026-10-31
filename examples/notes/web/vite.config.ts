import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// The page imports the SDK from the repository, not from npm: `@hominemai/ordo-sdk`'s
// generated half belongs to one deployment, and this app carries its own
// (`src/generated`, written by `npm run gen`). The alias lets that generated
// code reach the SDK's hand-written core by the same path everywhere.
const sdk = fileURLToPath(new URL('../../../clients/ordo-sdk/src', import.meta.url));

// `@hominemai/ordo-react` is aliased the same way. Its source imports `react`,
// and no `node_modules` sits beside it in this repository, so `dedupe` makes
// Vite resolve `react` from this app and not from the package's folder.
const hooks = fileURLToPath(new URL('../../../clients/ordo-react/src', import.meta.url));

// `ordo dev` sends CORS headers only when `ORDO_CORS_ORIGINS` is set, and it
// has no self-serve provisioning, so sign-up cannot finish against it.
// `ORDO_DEV_DOOR` makes Vite forward the door's routes from this origin, so a
// page that uses a development token needs no CORS setting. The page is then
// pointed at itself with `VITE_ORDO_URL=http://localhost:5173`. Unset, nothing
// is proxied.
const door = process.env['ORDO_DEV_DOOR'];

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      { find: '@hominemai/ordo-react', replacement: `${hooks}/index.ts` },
      { find: '@hominemai/ordo-sdk', replacement: sdk },
    ],
    dedupe: ['react', 'react-dom'],
  },
  // The passkey relying party is `localhost` and `ORDO_AUTH_ORIGINS` names
  // this exact origin, so the port is fixed: another port is another origin
  // and the door would refuse the ceremony and the calls.
  server: {
    host: 'localhost',
    port: 5173,
    strictPort: true,
    ...(door
      ? { proxy: { '/op': door, '/auth': door, '/health': door } }
      : {}),
  },
  test: {
    globals: true,
    environment: 'node',
    include: ['test/**/*.test.{ts,tsx}'],
    // The live tests wait on a real feed.
    testTimeout: 40_000,
  },
});
