/**
 * The deployment's developer, shared by the e2e suites that run the
 * identity service (examples/notes/e2e, examples/note-viewer/e2e).
 *
 * The developer owns the deployment and runs the `ordo` command against its
 * door (plan-versions.md increment 8). They come in as a real developer
 * does, with no backdoor. They join the identity service by its first
 * invite link, with a passkey, then run `ordo login`, open the link it
 * prints, and approve it with that passkey. The CLI keeps the session in a
 * config folder of its own and sends the token it gets for every door
 * command after that.
 *
 * Each suite hands in its own stack and its own way to make a person with
 * a passkey device. This file imports nothing from `@playwright/test`: it
 * lies outside both suites' packages, and a second copy of Playwright's
 * runner refuses to load. It drives the page by the methods below, which
 * Playwright's `Page` and `Locator` have, and checks with `node:assert`.
 */

import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';

/** The part of a Playwright `Locator` the kit uses. */
export interface LocatorLike {
  waitFor(options?: { state?: 'visible'; timeout?: number }): Promise<void>;
  click(): Promise<void>;
  fill(value: string): Promise<void>;
  check(): Promise<void>;
}

/** The part of a Playwright `Page` the kit uses. */
export interface PageLike {
  goto(url: string): Promise<unknown>;
  getByRole(role: 'heading' | 'button', options: { name: string }): LocatorLike;
  getByLabel(text: string): LocatorLike;
  getByText(text: string): LocatorLike;
}

/** One person: a browser profile with its own passkey device. */
export interface PersonLike {
  readonly context: { close(): Promise<void> };
  readonly page: PageLike;
}

const SHOWN = { state: 'visible', timeout: 15_000 } as const;

/** What the kit needs to know about a suite's stack. */
export interface DeveloperStack {
  /** The `ordo` CLI. */
  readonly ordo: string;
  /** The identity service's address as the CLI reaches it (`ordo login <it>`). */
  readonly identity: string;
  /**
   * The identity service's address as the browser reaches it, when the CLI
   * reaches it another way (a TLS front the CLI does not trust). The link
   * `ordo login` prints is opened there.
   */
  readonly identityPage?: string;
  /** The identity service's data folder, which holds `first-invite`. */
  readonly identityData: string;
  /** The identity service's binary, for its host commands. */
  readonly identityBin: string;
  /** Its database URL, for those commands. */
  readonly identityDb: string;
  /** Its public origin (`ORDO_IDENTITY_ORIGIN`), for those commands. */
  readonly identityOrigin: string;
  /** Where the stack keeps its logs; the CLI sessions go beside them. */
  readonly logs: string;
}

export interface CliRun {
  readonly code: number;
  readonly out: string;
  readonly err: string;
}

export interface Developer {
  /** Run `ordo <args>` as the developer. */
  readonly ordo: (args: readonly string[]) => CliRun;
  /** Their current token, as `ordo login` keeps it. */
  readonly token: () => string;
  readonly close: () => Promise<void>;
}

/** A new developer, just signed in, and their token as `ordo login` keeps it. */
export type FreshDeveloper = Developer;

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

/** What the kit gives a suite. */
export interface DeveloperKit<Browser> {
  /**
   * The developer, signed in. The first invite works once, so a stack that
   * is reused (locally) keeps the session from the first sign-in and signs
   * in no second time.
   */
  readonly developer: (browser: Browser) => Promise<Developer>;
  /**
   * **A new developer, an admin of the service's own deployment, signed in
   * now**: invited with the host command, joined with a passkey, `ordo
   * login` in a folder of its own. Its touch is fresh.
   */
  readonly freshDeveloper: (browser: Browser, name: string) => Promise<FreshDeveloper>;
  /**
   * **A new developer, an admin of the service's own deployment and a
   * member of `handle`**: `ordo login` twice in one browser profile, once
   * per deployment, each session in a folder of its own. `handle` must be
   * registered (`deployment add`).
   */
  readonly developerOfTwo: (browser: Browser, name: string, handle: string) => Promise<TwoDeployments>;
  /** The identity service's host command, run with the service's own settings. */
  readonly identityCommand: (args: readonly string[]) => string;
}

/** The kit over one suite's stack and its way to make a person. */
export function developerKit<Browser>(
  stack: () => DeveloperStack,
  person: (browser: Browser) => Promise<PersonLike>,
): DeveloperKit<Browser> {
  /** Where the developer's CLI keeps its session (ORDO_CONFIG_DIR). */
  const configDir = () => `${stack().logs}/cli-config`;

  function env(dir: string): NodeJS.ProcessEnv {
    // Nothing of the door's own settings: the CLI reaches the door over
    // HTTP with the session's token, and opens no database.
    const clean: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('ORDO_')) clean[k] = v;
    return { ...clean, ORDO_CONFIG_DIR: dir, ORDO_IDENTITY_URL: stack().identity };
  }

  /** `ordo <args>`, as the developer whose session is in `dir`. */
  function run(args: readonly string[], dir: string, input?: string): CliRun {
    const started = Date.now();
    const r = spawnSync(stack().ordo, [...args], { env: env(dir), encoding: 'utf8', timeout: 240_000, input });
    const out = { code: r.status ?? -1, out: r.stdout ?? '', err: r.stderr ?? '' };
    // In the run's log, so a failure shows what each command answered.
    console.log(`$ ordo ${args.join(' ')}  (exit ${out.code}, ${Date.now() - started} ms)\n${out.out}${out.err}`);
    return out;
  }

  function tokenIn(dir: string): string {
    // `whoami` renews a token near its end, so the one read is current.
    run(['whoami'], dir);
    const m = /"token"\s*:\s*"([^"]+)"/.exec(readFileSync(`${dir}/sessions.json`, 'utf8'));
    assert.ok(m, 'a token in the session');
    return m[1]!;
  }

  /** Join the identity service by an invite link, with a passkey. */
  async function join(page: PageLike, link: string, name: string): Promise<void> {
    await page.goto(link);
    await page.getByRole('heading', { name: 'Join this deployment' }).waitFor(SHOWN);
    await page.getByLabel('Your name').fill(name);
    await page.getByRole('button', { name: 'Create a passkey' }).click();
    await page.getByRole('heading', { name: 'Save your recovery codes' }).waitFor(SHOWN);
    await page.getByLabel('I saved these codes somewhere safe').check();
    await page.getByRole('button', { name: 'Continue' }).click();
    await page.getByText(`Signed in as ${name}`).waitFor(SHOWN);
  }

  /**
   * `ordo login`, with the session kept in `dir`, for the identity
   * service's own deployment or `--deployment <handle>`. It prints the link
   * and waits for the browser to come back to its loopback port; the page
   * approves it with its passkey.
   */
  async function login(page: PageLike, dir: string, handle?: string): Promise<void> {
    const s = stack();
    const args = ['login', s.identity, '--no-browser', '--wait-secs', '120'];
    if (handle) args.push('--deployment', handle);
    const cli = spawn(s.ordo, args, { env: env(dir) });
    let out = '';
    let err = '';
    cli.stdout.on('data', (d: Buffer) => (out += d.toString()));
    cli.stderr.on('data', (d: Buffer) => (err += d.toString()));
    const done = new Promise<number>((resolve) => cli.on('close', (c) => resolve(c ?? -1)));
    const linkIn = () => /https?:\/\/\S+\/cli\?\S+/.exec(err)?.[0] ?? '';
    for (let waited = 0; !linkIn() && waited < 30_000; waited += 200) {
      await new Promise((r) => setTimeout(r, 200));
    }
    let link = linkIn();
    assert.ok(link, `ordo login printed no link:\n${out}\n${err}`);
    if (s.identityPage && link.startsWith(s.identity)) link = s.identityPage + link.slice(s.identity.length);
    await page.goto(link);
    await page.getByRole('heading', { name: 'Sign in the ordo command?' }).waitFor(SHOWN);
    await page.getByRole('button', { name: 'Approve with my passkey' }).click();
    const code = await done;
    assert.equal(code, 0, `ordo login:\n${out}\n${err}`);
    assert.ok(out.includes('Signed in to'), out);
  }

  function identityCommand(args: readonly string[]): string {
    const s = stack();
    const clean: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('ORDO_')) clean[k] = v;
    const r = spawnSync(s.identityBin, [...args], {
      env: {
        ...clean,
        ORDO_IDENTITY_DATABASE_URL: s.identityDb,
        ORDO_IDENTITY_ORIGIN: s.identityOrigin,
        ORDO_IDENTITY_LISTEN: '127.0.0.1:0',
        ORDO_IDENTITY_DATA_DIR: s.identityData,
        ORDO_IDENTITY_DEPLOYMENT: 'main',
      },
      encoding: 'utf8',
      timeout: 60_000,
    });
    console.log(`$ ordo-identity-service ${args.join(' ')}  (exit ${r.status})\n${r.stdout}${r.stderr}`);
    assert.equal(r.status, 0, `ordo-identity-service ${args.join(' ')}:\n${r.stdout}\n${r.stderr}`);
    return r.stdout;
  }

  async function developer(browser: Browser): Promise<Developer> {
    const dir = configDir();
    mkdirSync(dir, { recursive: true });
    const { context, page } = await person(browser);
    const close = () => context.close();
    const signedIn = { ordo: (args: readonly string[]) => run(args, dir), token: () => tokenIn(dir), close };
    if (existsSync(`${dir}/sessions.json`) && run(['whoami'], dir).code === 0) return signedIn;
    // Join the identity service by its first invite, with a passkey.
    await join(page, readFileSync(`${stack().identityData}/first-invite`, 'utf8').trim(), 'Dev');
    await login(page, dir);
    return signedIn;
  }

  async function freshDeveloper(browser: Browser, name: string): Promise<FreshDeveloper> {
    const { context, page } = await person(browser);
    const link = /https?:\/\/\S+/.exec(identityCommand(['invite', '--label', name]))?.[0] ?? '';
    assert.ok(link, 'an invite link');
    await join(page, link, name);
    const dir = `${stack().logs}/cli-config-${name}`;
    mkdirSync(dir, { recursive: true });
    await login(page, dir);
    return { ordo: (args) => run(args, dir), token: () => tokenIn(dir), close: () => context.close() };
  }

  async function developerOfTwo(browser: Browser, name: string, handle: string): Promise<TwoDeployments> {
    const { context, page } = await person(browser);
    const link = /https?:\/\/\S+/.exec(identityCommand(['invite', '--label', name]))?.[0] ?? '';
    assert.ok(link, 'an invite link');
    await join(page, link, name);
    const mainDir = `${stack().logs}/cli-config-${name}`;
    const otherDir = `${stack().logs}/cli-config-${name}-${handle}`;
    mkdirSync(mainDir, { recursive: true });
    mkdirSync(otherDir, { recursive: true });
    await login(page, mainDir);
    const who = run(['whoami'], mainDir);
    const id = /^who +(\S+)$/m.exec(who.out)?.[1] ?? '';
    assert.match(id, /^p_/, who.out);
    identityCommand(['member', 'add', handle, id]);
    await login(page, otherDir, handle);
    return {
      main: (args) => run(args, mainDir),
      other: (args, input) => run(args, otherDir, input),
      otherToken: () => {
        const m = /"token"\s*:\s*"([^"]+)"/.exec(readFileSync(`${otherDir}/sessions.json`, 'utf8'));
        assert.ok(m, `a token in the ${handle} session`);
        return m[1]!;
      },
      close: () => context.close(),
    };
  }

  return { developer, freshDeveloper, developerOfTwo, identityCommand };
}
