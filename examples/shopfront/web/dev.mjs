// Serves the one build at the three origins `../dev/serve.sh` names:
// sign-in on 5180, shop on 5181, staff console on 5182. Each is its own Vite
// dev server on `localhost` with a fixed port. `src/main.tsx` picks the page
// from the origin it was loaded at.
//
// The ports follow VITE_LOGIN_URL, VITE_SHOP_URL and VITE_ADMIN_URL when set.
import { createServer } from 'vite';

const urls = [
  process.env.VITE_LOGIN_URL ?? 'http://localhost:5180',
  process.env.VITE_SHOP_URL ?? 'http://localhost:5181',
  process.env.VITE_ADMIN_URL ?? 'http://localhost:5182',
];

for (const url of urls) {
  const port = Number(new URL(url).port);
  const server = await createServer({ server: { host: 'localhost', port, strictPort: true } });
  await server.listen();
  server.printUrls();
}
