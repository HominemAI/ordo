import { spawn } from 'node:child_process';
import { openSync, closeSync } from 'node:fs';
import { appendFile, mkdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { expect, type APIRequestContext, type TestInfo } from '@playwright/test';
import { stack, state } from './support.js';

async function freePort(): Promise<string> {
  const server = createServer();
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('No test port allocated');
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return String(address.port);
}

/** A failed deployment must not take down the other tests' live fixture. */
export async function withDeployment(request: APIRequestContext, info: TestInfo, use: (s: ReturnType<typeof stack>) => Promise<void>) {
  const here = fileURLToPath(new URL('..', import.meta.url));
  const isolated = `${state}-deploy`;
  await mkdir(isolated, { recursive: true });
  const ports = new Set<string>();
  while (ports.size < 3) ports.add(await freePort());
  const [door, doorB, page] = [...ports];
  const fd = openSync(`${isolated}/stack.log`, 'w');
  const child = spawn('bash', ['stack.sh'], { cwd: here, detached: true, stdio: ['ignore', fd, fd], env: {
    ...process.env, E2E_STATE: isolated, E2E_DOOR_PORT: door, E2E_DOOR_B_PORT: doorB, E2E_PAGE_PORT: page,
  } });
  closeSync(fd);
  const done = new Promise<void>((resolve, reject) => { child.once('exit', () => resolve()); child.once('error', reject); });
  try {
    await expect.poll(async () => {
      if (child.exitCode !== null) throw new Error(`Isolated stack exited ${child.exitCode}`);
      return request.get(`http://localhost:${page}/`, { timeout: 2000 }).then(r => r.status()).catch(() => 0);
    }, { timeout: 120_000, intervals: [200], message: 'Isolated deployment becomes ready' }).toBe(200);
    await use(JSON.parse(await readFile(`${isolated}/stack.json`, 'utf8')));
  } finally {
    if (child.pid && child.exitCode === null) process.kill(-child.pid, 'SIGTERM');
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([done, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Isolated stack did not stop cleanly')), 15_000); })]);
    } finally {
      clearTimeout(timer);
      // The normal report checks every started host, including these replicas.
      const pids = await readFile(`${isolated}/host-pids`, 'utf8').catch(() => '');
      await appendFile(`${state}/host-pids`, pids);
      for (const log of ['stack', 'apply', 'door', 'door-b']) {
        const body = await readFile(`${isolated}/${log}.log`).catch(() => undefined);
        if (body) await info.attach(`isolated-${log}`, { body, contentType: 'text/plain' });
      }
    }
  }
}
