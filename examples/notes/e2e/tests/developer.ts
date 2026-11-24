/**
 * The deployment's developer: the person who owns the deployment and runs
 * the `ordo` command against its door (plan-versions.md increment 8).
 *
 * They come in as a real developer does, with no backdoor. They join the
 * identity service by its first invite link, with a passkey, then run
 * `ordo login`, open the link it prints, and approve it with that passkey.
 * The CLI keeps the session in a config folder of its own and sends the
 * token it gets for every door command after that.
 */

import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { expect, type Browser } from '@playwright/test';
import { person, stack } from './support.js';

export interface CliRun {
  readonly code: number;
  readonly out: string;
  readonly err: string;
}

export interface Developer {
  /** Run `ordo <args>` as the developer, against the stack's door. */
  readonly ordo: (args: readonly string[]) => CliRun;
  readonly close: () => Promise<void>;
}

/** Where the developer's CLI keeps its session (ORDO_CONFIG_DIR). */
function configDir(): string {
  return `${stack().logs}/cli-config`;
}

function env(): NodeJS.ProcessEnv {
  // Nothing of the door's own settings: the CLI reaches the door over HTTP
  // with the session's token, and opens no database.
  const clean: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('ORDO_')) clean[k] = v;
  return { ...clean, ORDO_CONFIG_DIR: configDir(), ORDO_IDENTITY_URL: stack().identity };
}

/** `ordo <args>`, as the developer. Door commands get `--host` and `--space`. */
function run(args: readonly string[]): CliRun {
  const started = Date.now();
  const r = spawnSync(stack().ordo, [...args], { env: env(), encoding: 'utf8', timeout: 240_000 });
  const run = { code: r.status ?? -1, out: r.stdout ?? '', err: r.stderr ?? '' };
  // In the run's log, so a failure shows what each command answered.
  console.log(`$ ordo ${args.join(' ')}  (exit ${run.code}, ${Date.now() - started} ms)\n${run.out}${run.err}`);
  return run;
}

/** The door flags of every deployment command: the deployment's own space. */
export function door(): string[] {
  return ['--host', stack().door, '--space', 'system'];
}

/**
 * The developer, signed in. The first invite works once, so a stack that is
 * reused (locally) keeps the session from the first sign-in and signs in no
 * second time.
 */
export async function developer(browser: Browser): Promise<Developer> {
  const s = stack();
  mkdirSync(configDir(), { recursive: true });
  const { context, page } = await person(browser);
  const close = () => context.close();
  if (existsSync(`${configDir()}/sessions.json`) && run(['whoami']).code === 0) {
    return { ordo: run, close };
  }

  // Join the identity service by its first invite, with a passkey.
  await page.goto(readFileSync(`${s.identityData}/first-invite`, 'utf8').trim());
  await expect(page.getByRole('heading', { name: 'Join this deployment' })).toBeVisible();
  await page.getByLabel('Your name').fill('Dev');
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Save your recovery codes' })).toBeVisible();
  await page.getByLabel('I saved these codes somewhere safe').check();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByText('Signed in as Dev')).toBeVisible();

  // `ordo login`: it prints the link and waits for the browser to come back
  // to its loopback port.
  const login = spawn(s.ordo, ['login', s.identity, '--no-browser', '--wait-secs', '120'], { env: env() });
  let out = '';
  let err = '';
  login.stdout.on('data', (d: Buffer) => (out += d.toString()));
  login.stderr.on('data', (d: Buffer) => (err += d.toString()));
  const done = new Promise<number>((resolve) => login.on('close', (c) => resolve(c ?? -1)));
  await expect.poll(() => /https?:\/\/\S+\/cli\?\S+/.exec(err)?.[0] ?? '', { timeout: 30_000 }).not.toBe('');
  const link = /https?:\/\/\S+\/cli\?\S+/.exec(err)![0];
  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Sign in the ordo command?' })).toBeVisible();
  await page.getByRole('button', { name: 'Approve with my passkey' }).click();
  const code = await done;
  expect(code, `ordo login:\n${out}\n${err}`).toBe(0);
  expect(out).toContain('Signed in to');
  return { ordo: run, close };
}
