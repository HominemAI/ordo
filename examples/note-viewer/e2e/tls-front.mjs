// HTTPS in front of the stack's plain-http servers, for the federation e2e.
//
// An OIDC issuer, its endpoints and an app's callback address must be https
// (`ordo-identity` O15). The identity service and the two doors serve plain
// http on loopback, so this process listens on one https port for each and
// forwards every request as it came, with `x-forwarded-proto: https`.
//
// The certificate is the test fixture `localhost` leaf, signed by the test CA
// in `crates/ordo-host/tests/fixture/ca.pem`. The doors trust that CA through
// `ORDO_AUTH_PROVIDER_CA_FILE`. The browser is told to trust the leaf's key
// (playwright.config.ts). Nothing here is fit for anything but a test.
//
// Usage: node tls-front.mjs <https port>=<http port> ...

import { readFileSync } from 'node:fs';
import { request } from 'node:http';
import { createServer } from 'node:https';

const fixture = (name) => readFileSync(new URL(`../../../crates/ordo-host/tests/fixture/${name}`, import.meta.url));
const tls = { cert: fixture('localhost.pem'), key: fixture('localhost-key.pem') };

for (const pair of process.argv.slice(2)) {
  const [front, back] = pair.split('=').map(Number);
  if (!front || !back) throw new Error(`not <https port>=<http port>: ${pair}`);
  const server = createServer(tls, (req, res) => {
    const out = request(
      {
        host: '127.0.0.1',
        port: back,
        method: req.method,
        path: req.url,
        headers: { ...req.headers, 'x-forwarded-proto': 'https' },
      },
      (up) => {
        res.writeHead(up.statusCode ?? 502, up.headers);
        up.pipe(res);
      },
    );
    out.on('error', (e) => {
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end(`nothing answered on port ${back}: ${e.message}`);
    });
    req.pipe(out);
  });
  // Every address, so `localhost` reaches it as 127.0.0.1 and as ::1.
  server.listen(front, () => console.log(`https://localhost:${front} -> http://127.0.0.1:${back}`));
}
