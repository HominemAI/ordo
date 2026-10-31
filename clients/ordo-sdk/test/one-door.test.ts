/**
 * One request helper, one retry policy, one error shape, across ordo-sdk,
 * ordo-react and ordo-cdk (`docs/audit-service-interfaces.md`, fix order
 * step 7).
 *
 * The audit found 3 request helpers, 4 retry policies and about 10 error
 * shapes. This test reads every hand-written source file of the three
 * packages and holds them to one of each:
 *
 * - **One request helper.** Only `src/core/door.ts` calls `fetch`. Every
 *   other file reaches the door through `doorRequest` (or `authRequest`,
 *   which is it with the sign-in defaults). The one exception is an upload
 *   with progress, which only `XMLHttpRequest` reports (`core/files.ts`).
 * - **One retry loop.** Only `door.ts` defines the loop; only `busy.ts`
 *   computes a backoff or reads `Retry-After`; only `refusal.ts` lists the
 *   retry classes.
 * - **One error shape.** Every error class a door answer becomes extends
 *   `OrdoRefusal`. The few other `Error` classes are listed below with why.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';

const clients = join(__dirname, '..', '..');
const ROOTS = ['ordo-sdk/src', 'ordo-react/src', 'ordo-cdk/src'];

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return f === 'generated' ? [] : walk(p);
    return /\.tsx?$/.test(p) ? [p] : [];
  });
}

/** The source without its comments, so a doc that names `fetch(` is not a call. */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`])\/\/.*$/gm, '$1');
}

const files = ROOTS.flatMap((r) => walk(join(clients, r))).map((p) => ({
  path: relative(clients, p),
  code: code(readFileSync(p, 'utf8')),
}));

/** Every file but `allowed` whose code matches `pattern`, with the line. */
function offenders(pattern: RegExp, allowed: readonly string[]): string[] {
  const out: string[] = [];
  for (const f of files) {
    if (allowed.includes(f.path)) continue;
    const flags = pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`;
    for (const m of f.code.matchAll(new RegExp(pattern.source, flags))) {
      const line = f.code.slice(0, m.index).split('\n').length;
      out.push(`${f.path}:${line}: ${m[0].trim()}`);
    }
  }
  return out;
}

const DOOR = 'ordo-sdk/src/core/door.ts';
const BUSY = 'ordo-sdk/src/core/busy.ts';
const REFUSAL = 'ordo-sdk/src/core/refusal.ts';
const FILES = 'ordo-sdk/src/core/files.ts';

/**
 * A call of a fetch: `fetch(`, `x.fetch(`, `#fetch(`, or the helper's own
 * names for one (`f(`, `send(`) given a URL. `Call.fetch(options)`, the
 * generated read verb, is not one.
 */
const FETCH_CALL = /(?:(?<![\w$])|[.#])fetch\s*\(\s*(?!options\b|\))|(?<![\w$.#])(?:f|send)\s*\(\s*(?:url\b|u\b|`|'|")/;

describe('one request helper', () => {
  it('reads the three packages', () => {
    expect(files.length).toBeGreaterThan(60);
    expect(files.some((f) => f.path === DOOR)).toBe(true);
  });

  it('the pattern catches the forms a second helper would take', () => {
    const hit = (s: string) => new RegExp(FETCH_CALL.source).test(code(s));
    for (const bad of [
      'await fetch(url, init)',
      'await this.#fetch(`${base}/op`, init)',
      'endpoint.fetch(url(door, path), init)',
      'const r = await f(`${base}/auth`, {})',
      'return send(url, init)',
    ]) {
      expect(hit(bad), bad).toBe(true);
    }
    for (const fine of ['await call.fetch(options)', 'async fetch(options = {})', 'await this.fetch()', 'req.send(file)']) {
      expect(hit(fine), fine).toBe(false);
    }
  });

  it('only core/door.ts calls fetch', () => {
    expect(offenders(FETCH_CALL, [DOOR]), 'send it through doorRequest (core/door.ts)').toEqual([]);
  });

  it('only core/door.ts takes the global fetch', () => {
    expect(offenders(/(?<!typeof\s+)globalThis\.fetch\b/, [DOOR]), 'use fetchOf (core/door.ts)').toEqual([]);
  });

  it('only an upload with progress opens an XMLHttpRequest, and nothing opens another transport', () => {
    expect(offenders(/new\s+XMLHttpRequest\b/, [FILES])).toEqual([]);
    expect(offenders(/new\s+(?:EventSource|WebSocket)\b|sendBeacon\s*\(/, [])).toEqual([]);
  });
});

describe('one retry policy', () => {
  it('only core/door.ts holds the retry loop', () => {
    expect(offenders(/function\s+fetchRetrying\b/, [DOOR])).toEqual([]);
    expect(offenders(/\bfetchRetrying\s*\(/, [DOOR]), 'pass a RetryPolicy to doorRequest').toEqual([]);
  });

  it('only core/busy.ts computes a backoff', () => {
    // An exponent that is not a power of ten (decimals use those) is a backoff.
    expect(offenders(/(?<!\b10n?\s*)\*\*(?!\/)/, [BUSY]), 'use reconnectDelayMs or the retry options').toEqual([]);
  });

  it("only core/busy.ts reads the door's Retry-After", () => {
    expect(offenders(/['"]Retry-After['"]/i, [BUSY]), 'use retryAfterMs, or OrdoRefusal.retryAfterMs').toEqual([]);
  });

  it('only core/refusal.ts lists the retry classes', () => {
    expect(offenders(/new\s+Set[^;]*['"]server_busy['"]/, [REFUSAL]), 'use RETRY_CLASSES or RECONNECT_CLASSES').toEqual(
      [],
    );
  });
});

describe('one error shape', () => {
  /**
   * The `Error` classes that are not a door's refusal, and why each stands
   * apart. Every other error class extends `OrdoRefusal`.
   */
  const NOT_A_REFUSAL: Record<string, string> = {
    OrdoRefusal: 'the one shape itself',
    OrdoNetworkError: 'no answer came: there is no code or status to carry',
    CallTimeout: 'the deadline, inside the helper; it reaches a caller as OrdoNetworkError',
    SchemaVersionError: "ordo-react: a descriptor file this package cannot read; the door said nothing",
    MalformedDescriptorError: 'ordo-react: as SchemaVersionError',
  };

  const classes = files.flatMap((f) =>
    [...f.code.matchAll(/class\s+([A-Za-z_$][\w$]*)(?:<[^>{]*>)?\s+extends\s+([A-Za-z_$][\w$]*)/g)].map((m) => ({
      name: m[1]!,
      base: m[2]!,
      path: f.path,
    })),
  );

  it('declares OrdoRefusal once', () => {
    expect(classes.filter((c) => c.name === 'OrdoRefusal').map((c) => c.path)).toEqual([REFUSAL]);
  });

  it('every other error class is an OrdoRefusal', () => {
    const refusals = new Set(['OrdoRefusal']);
    // A subclass of a subclass counts: walk until nothing new joins.
    for (let grew = true; grew; ) {
      grew = false;
      for (const c of classes) {
        if (refusals.has(c.base) && !refusals.has(c.name)) {
          refusals.add(c.name);
          grew = true;
        }
      }
    }
    const apart = classes
      .filter((c) => c.base === 'Error' && !(c.name in NOT_A_REFUSAL))
      .map((c) => `${c.path}: ${c.name}`);
    expect(apart, 'extend OrdoRefusal, or say in NOT_A_REFUSAL why it is not one').toEqual([]);
    for (const name of ['PasskeyError', 'ServerBusyError', 'AccessError', 'ConnectionError', 'DelegationError', 'TenantError', 'AccountEmailError', 'TenantStatusError', 'OrdoLocalRefusal']) {
      expect(refusals.has(name), name).toBe(true);
    }
  });

  it('keeps NOT_A_REFUSAL honest', () => {
    const stale = Object.keys(NOT_A_REFUSAL).filter((n) => !classes.some((c) => c.name === n && c.base === 'Error'));
    expect(stale, 'remove these from NOT_A_REFUSAL').toEqual([]);
  });
});
