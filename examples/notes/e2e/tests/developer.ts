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
import { expect, type Browser, type Page } from '@playwright/test';
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

function env(dir = configDir()): NodeJS.ProcessEnv {
  // Nothing of the door's own settings: the CLI reaches the door over HTTP
  // with the session's token, and opens no database.
  const clean: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('ORDO_')) clean[k] = v;
  return { ...clean, ORDO_CONFIG_DIR: dir, ORDO_IDENTITY_URL: stack().identity };
}

/** `ordo <args>`, as the developer. Door commands get `--host` and `--space`. */
function run(args: readonly string[], dir = configDir(), input?: string): CliRun {
  const started = Date.now();
  const r = spawnSync(stack().ordo, [...args], { env: env(dir), encoding: 'utf8', timeout: 240_000, input });
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
    return { ordo: (args) => run(args), close };
  }

  // Join the identity service by its first invite, with a passkey.
  await join(page, readFileSync(`${s.identityData}/first-invite`, 'utf8').trim(), 'Dev');
  await login(page, configDir());
  return { ordo: (args) => run(args), close };
}

/** Join the identity service by an invite link, with a passkey. */
async function join(page: Page, link: string, name: string): Promise<void> {
  await page.goto(link);
  await expect(page.getByRole('heading', { name: 'Join this deployment' })).toBeVisible();
  await page.getByLabel('Your name').fill(name);
  await page.getByRole('button', { name: 'Create a passkey' }).click();
  await expect(page.getByRole('heading', { name: 'Save your recovery codes' })).toBeVisible();
  await page.getByLabel('I saved these codes somewhere safe').check();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByText(`Signed in as ${name}`)).toBeVisible();
}

/**
 * `ordo login`, with the session kept in `dir`, for the identity service's
 * own deployment or `--deployment <handle>`. It prints the link and waits
 * for the browser to come back to its loopback port; the page approves it
 * with its passkey.
 */
async function login(page: Page, dir: string, handle?: string): Promise<void> {
  const s = stack();
  const args = ['login', s.identity, '--no-browser', '--wait-secs', '120'];
  if (handle) args.push('--deployment', handle);
  const login = spawn(s.ordo, args, { env: env(dir) });
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
}

/**
 * The identity service's host command (`ordo-identity-service <args>`), run
 * with the service's own settings, as the deployment owner runs it on the
 * host. Answers what it printed.
 */
export function identityCommand(args: readonly string[]): string {
  const s = stack();
  const clean: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('ORDO_')) clean[k] = v;
  const r = spawnSync(s.identityBin, [...args], {
    env: {
      ...clean,
      ORDO_IDENTITY_DATABASE_URL: s.identityDb,
      ORDO_IDENTITY_ORIGIN: s.identity,
      ORDO_IDENTITY_LISTEN: '127.0.0.1:0',
      ORDO_IDENTITY_DATA_DIR: s.identityData,
      ORDO_IDENTITY_DEPLOYMENT: 'main',
    },
    encoding: 'utf8',
    timeout: 60_000,
  });
  console.log(`$ ordo-identity-service ${args.join(' ')}  (exit ${r.status})\n${r.stdout}${r.stderr}`);
  expect(r.status, `ordo-identity-service ${args.join(' ')}:\n${r.stdout}\n${r.stderr}`).toBe(0);
  return r.stdout;
}

/** A new developer, just signed in, and their token as `ordo login` keeps it. */
export interface FreshDeveloper extends Developer {
  readonly token: () => string;
}

/**
 * **A new developer, an admin of `main`, signed in now**: invited with the
 * host command, joined with a passkey, `ordo login` in a folder of its own.
 * Its touch is fresh, so the door lets it reach a tenant for the next 5
 * minutes (a dev branch, `tests/branch`).
 */
export async function freshDeveloper(browser: Browser, name: string): Promise<FreshDeveloper> {
  const { context, page } = await person(browser);
  const link = /https?:\/\/\S+/.exec(identityCommand(['invite', '--label', name]))?.[0] ?? '';
  expect(link, 'an invite link').not.toBe('');
  await join(page, link, name);
  const dir = `${stack().logs}/cli-config-${name}`;
  mkdirSync(dir, { recursive: true });
  await login(page, dir);
  return {
    ordo: (args) => run(args, dir),
    token: () => {
      const m = /"token"\s*:\s*"([^"]+)"/.exec(readFileSync(`${dir}/sessions.json`, 'utf8'));
      expect(m, 'a token in the session').not.toBeNull();
      return m![1]!;
    },
    close: () => context.close(),
  };
}

/** A developer signed in to two deployments at one identity service. */
export interface TwoDeployments {
  /** `ordo <args>` with a token for the identity service's own deployment. */
  readonly main: (args: readonly string[]) => CliRun;
  /** `ordo <args>` with a token for `handle`; `input` is its standard input. */
  readonly other: (args: readonly string[], input?: string) => CliRun;
  /** Their token for `handle`, as `ordo login` keeps it. */
  readonly otherToken: () => string;
  readonly close: () => Promise<void>;
}

/**
 * **A new developer, an admin of `main` and a member of `handle`**: invited
 * with the host command, joined with a passkey, made a member of `handle`
 * (`member add`), then `ordo login` twice in one browser profile, once per
 * deployment, each session in a folder of its own. `handle` must be
 * registered (`deployment add`).
 */
export async function developerOfTwo(browser: Browser, name: string, handle: string): Promise<TwoDeployments> {
  const { context, page } = await person(browser);
  const link = /https?:\/\/\S+/.exec(identityCommand(['invite', '--label', name]))?.[0] ?? '';
  expect(link, 'an invite link').not.toBe('');
  await join(page, link, name);
  const mainDir = `${stack().logs}/cli-config-${name}`;
  const otherDir = `${stack().logs}/cli-config-${name}-${handle}`;
  mkdirSync(mainDir, { recursive: true });
  mkdirSync(otherDir, { recursive: true });
  await login(page, mainDir);
  const who = run(['whoami'], mainDir);
  const person_ = /^who +(\S+)$/m.exec(who.out)?.[1] ?? '';
  expect(person_, who.out).toMatch(/^p_/);
  identityCommand(['member', 'add', handle, person_]);
  await login(page, otherDir, handle);
  return {
    main: (args) => run(args, mainDir),
    other: (args, input) => run(args, otherDir, input),
    otherToken: () => {
      const m = /"token"\s*:\s*"([^"]+)"/.exec(readFileSync(`${otherDir}/sessions.json`, 'utf8'));
      expect(m, `a token in the ${handle} session`).not.toBeNull();
      return m![1]!;
    },
    close: () => context.close(),
  };
}
