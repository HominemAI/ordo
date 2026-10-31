import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The page imports the SDK and the hooks from the repository, as the notes
// page does (`examples/notes/web/vite.config.ts`).
const sdk = fileURLToPath(new URL('../../../clients/ordo-sdk/src', import.meta.url));
const hooks = fileURLToPath(new URL('../../../clients/ordo-react/src', import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      { find: '@hominemai/ordo-react', replacement: `${hooks}/index.ts` },
      { find: '@hominemai/ordo-sdk', replacement: sdk },
    ],
    dedupe: ['react', 'react-dom'],
  },
  // The door's CORS setting names this exact origin (`dev/serve.sh`).
  server: { host: 'localhost', port: 5183, strictPort: true },
});
