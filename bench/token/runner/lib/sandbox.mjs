// The sandbox every trial's `claude -p` runs in: Claude Code's own, set by a
// settings file the runner writes outside the workspace and passes with
// `--settings`. Identical for both stacks. Roomy inside the run, shut outside.
//
// Keys used (https://code.claude.com/docs/en/settings-reference, "sandbox.*",
// and https://code.claude.com/docs/en/sandboxing):
//   sandbox.enabled, sandbox.failIfUnavailable, sandbox.autoAllowBashIfSandboxed,
//   sandbox.allowUnsandboxedCommands (false: no retry outside the sandbox),
//   sandbox.excludedCommands (empty),
//   sandbox.filesystem.{allowWrite,denyWrite,denyRead,allowRead},
//   sandbox.network.{allowedDomains,strictAllowlist,allowLocalBinding,allowUnixSockets},
//   permissions.blockReadsOutsideWorkingDirectories, permissions.deny.
// The sandbox covers Bash and what it starts. The Read, Edit and Write tools
// are not Bash, so the permission keys cover them.

import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';

/** The only hosts a trial may reach, besides localhost. Found from the stacks' own fetches. */
export const ALLOWED_DOMAINS = [
  // npm install: every `resolved` URL in the starter's lockfile is on this host.
  'registry.npmjs.org',
  // The dev servers and test stacks the agent starts.
  'localhost',
  '127.0.0.1',
];

/** Variables set so that every cache and temp file lands in the run's scratch folder. */
export function sandboxEnv({ scratch, dockerHost }) {
  const env = {
    TMPDIR: scratch,
    npm_config_cache: join(scratch, 'npm-cache'),
    XDG_CACHE_HOME: join(scratch, 'xdg-cache'),
    XDG_CONFIG_HOME: join(scratch, 'xdg-config'),
    DO_NOT_TRACK: '1',
  };
  if (dockerHost) env['DOCKER_HOST'] = dockerHost;
  return env;
}

/** The Docker endpoint the CLI would use, e.g. unix:///Users/me/.docker/run/docker.sock, or null. */
export function dockerHostOf(base = process.env) {
  if (base['DOCKER_HOST']) return base['DOCKER_HOST'];
  try {
    return (
      execFileSync('docker', ['context', 'inspect', '--format', '{{.Endpoints.docker.Host}}'], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
        timeout: 30_000,
      }).trim() || null
    );
  } catch {
    return null;
  }
}

/** A macOS per-user folder from `getconf`, or null elsewhere. */
function darwinDir(name) {
  if (process.platform !== 'darwin') return null;
  try {
    return execFileSync('getconf', [name], { encoding: 'utf8' }).trim().replace(/\/$/, '') || null;
  } catch {
    return null;
  }
}

const real = (p) => {
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
};
/** A path both as given and as resolved (macOS /var is /private/var). */
const both = (p) => [...new Set([p, real(p)])];

/**
 * The settings object. `ws` and `scratch` are the run's folders; `readable`
 * are files or folders outside them the agent may read and run (the Ordo and
 * Postgres binaries); `dockerHost` is the Docker endpoint, or null.
 */
export function sandboxSettings({ ws, scratch, readable = [], dockerHost = null, home = homedir() }) {
  const sock = dockerHost?.startsWith('unix://') ? dockerHost.slice('unix://'.length) : null;
  // macOS developer tools (git, strings, cc) write a cache straight into the
  // user's temp and cache folders, whatever TMPDIR says; closing them broke
  // git in the first real trial. Room to work, at the cost of reads there.
  const darwin = ['DARWIN_USER_TEMP_DIR', 'DARWIN_USER_CACHE_DIR'].map(darwinDir).filter(Boolean);
  const rw = [...both(ws), ...both(scratch), ...darwin.flatMap(both)];
  const reopen = [...rw, ...readable.flatMap(both), ...(sock ? both(sock) : [])];
  // Everything where the owner's files, other repositories and other
  // sessions' scratch live. The system folders stay readable: tools need them.
  const closed = [...both(home), '/Users', '/Volumes', '/private/tmp', '/tmp', '/private/var/folders'];
  return {
    sandbox: {
      enabled: true,
      // If the sandbox cannot start, the phase does not start: no silent fallback.
      failIfUnavailable: true,
      // Bash is allowed by --allowedTools anyway; this keeps it from asking.
      autoAllowBashIfSandboxed: true,
      // No escape hatch: `dangerouslyDisableSandbox` is ignored.
      allowUnsandboxedCommands: false,
      excludedCommands: [],
      filesystem: {
        allowWrite: rw,
        denyWrite: [],
        denyRead: [...new Set(closed)],
        // Narrower paths win, so these re-open only the run's own folders.
        allowRead: [...new Set([...reopen, '/private/tmp/claude', '/tmp/claude'])],
      },
      network: {
        allowedDomains: ALLOWED_DOMAINS,
        // A host off the list is refused in every mode, never asked about.
        strictAllowlist: true,
        allowLocalBinding: true,
        allowUnixSockets: sock ? both(sock) : [],
      },
    },
    permissions: {
      // The Read, Glob and Grep tools stay inside the workspace.
      blockReadsOutsideWorkingDirectories: true,
      // A permission rule writes an absolute path with a leading //.
      deny: [`Read(/${home}/**)`, `Edit(/${home}/**)`, `Write(/${home}/**)`,'Read(//private/tmp/**)', 'Read(//tmp/**)'],
    },
  };
}

/** Write the settings file; answers its sha256. */
export function writeSettings(file, settings) {
  const text = `${JSON.stringify(settings, null, 2)}\n`;
  writeFileSync(file, text);
  return createHash('sha256').update(text).digest('hex');
}

/**
 * Before each phase: the settings file must still be what the runner wrote
 * (it sits outside every folder the agent may write, so this is a check, not
 * an expectation), and the workspace must hold no settings file of its own
 * (lists in settings merge, so one could widen the allow-lists). Anything
 * found is deleted and answered, to go in the record.
 */
export function guardSettings({ file, sha, ws }) {
  const found = [];
  const now = existsSync(file) ? createHash('sha256').update(readFileSync(file)).digest('hex') : null;
  if (now !== sha) found.push(`runner settings file changed: ${file}`);
  for (const name of ['settings.json', 'settings.local.json']) {
    const p = join(ws, '.claude', name);
    if (existsSync(p)) {
      found.push(`workspace settings file removed: .claude/${name}`);
      rmSync(p, { force: true });
    }
  }
  return found;
}

export const makeScratch = (scratch) => {
  for (const d of ['', 'npm-cache', 'xdg-cache', 'xdg-config']) mkdirSync(join(scratch, d), { recursive: true });
};
