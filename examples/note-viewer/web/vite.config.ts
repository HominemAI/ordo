import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The page imports the SDK from the repository, as notes v2's does: the
// generated half (`src/generated`, written by `npm run gen`) belongs to this
// deployment, and the alias lets it reach the SDK's hand-written core.
const sdk = fileURLToPath(new URL('../../../clients/ordo-sdk/src', import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [{ find: '@hominemai/ordo-sdk', replacement: sdk }],
    dedupe: ['react', 'react-dom'],
  },
  // notes v2's page takes 5173. The door's ORDO_AUTH_ORIGINS and
  // ORDO_AUTH_RETURN_URLS name this exact origin, so the port is fixed.
  server: { host: 'localhost', port: 5174, strictPort: true },
});
