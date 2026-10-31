import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// The console serves this build itself: index.html for every page route
// under /d/<handle>/, and the hashed files under /assets/ (crates/ordo-console
// src/page.rs). So the base is the site root, whatever the handle.
//
// The page runs under CSP `default-src 'self'`: no inline script or style.
// Vite's build emits only <script src> and <link rel=stylesheet>; the
// modulepreload polyfill is off so nothing else is injected.
//
// `npm run dev` forwards the console's routes to a console running on
// ORDO_CONSOLE_DEV (default http://localhost:8081), whose
// ORDO_CONSOLE_ORIGIN must then be http://localhost:5174.
const console = process.env['ORDO_CONSOLE_DEV'] ?? 'http://localhost:8081';

export default defineConfig({
  plugins: [react()],
  base: '/',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    modulePreload: { polyfill: false },
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  server: {
    host: 'localhost',
    port: 5174,
    strictPort: true,
    proxy: { '^/d/[^/]+/api/': console, '/health': console },
  },
});
