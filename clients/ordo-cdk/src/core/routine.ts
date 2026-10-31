/**
 * The routine: a named body of writes that runs on a clock or on a change
 * (checklist 12a).
 *
 * **Skeleton half**, like `release.ts`. A routine's body is a write op's
 * body, so it is built with the same calls: `b.set`, `b.del`, `.where()`,
 * `.let_()`. Only the trigger is new, and it is three small constructors.
 *
 * ```ts
 * routine('staleAlerts', { in: 'estate', trigger: every('15m'), params: { today: t.date() } },
 *   (b, p) => [
 *     b.set('alerts', { status: lit('dismissed') })
 *       .where(f.status.eq(lit('active')), f.raised.lt(p.today.sub(durationLit(30, 'd')))),
 *   ],
 * );
 * routine('motionSeen', { trigger: when('estate.events.created', { debounce: '30s', coalesce: true }) },
 *   (b) => [b.del('events', f.kind.eq(lit('noise')))],
 * ).unguarded().nopause();
 * ```
 *
 * A body that reads first is `each(pipeline, writes)`: the writes run once
 * for each row the pipeline produces, and inside their braces the row's
 * fields are named as the pipeline named them (`f.id`, `path('unit', 'name')`).
 *
 * ```ts
 * routine('expire', { trigger: daily('08:00Z'), params: { today: t.date() } },
 *   (b, p) => each(
 *     b.source(src('leases').where(f.end.lt(p.today)).pick('id')),
 *     [b.set('leases', { id: f.id, status: lit('expired') })],
 *   ),
 * );
 * ```
 *
 * Event kinds (`created`, `written`, …) are written inside the `when`
 * pattern string, `<domain>.<entity>.<kind>`, and the parser checks them
 * against its closed set. The builder carries the string and offers no second
 * list of kinds that could drift from the parser's.
 */

import { Body, Q, paramNode, type ParamsOf, type TypeSpec, type WriteLike } from './build.js';
import type { PipelineNode, RoutineNode, RoutineTriggerNode, WriteStmtNode } from './ir.js';
import { makeParams } from './value.js';

/** A routine's trigger, as the clause body prints it. */
export type RoutineTrigger = RoutineTriggerNode;

const PERIOD = /^[0-9]+[smhdw]$/;

/** `trigger every 5m`: a fixed period, `<n>` followed by `s`, `m`, `h`, `d` or `w`. */
export function every(period: string): RoutineTrigger {
  if (!PERIOD.test(period)) {
    throw new Error(`every: expected \`<n><s|m|h|d|w>\` such as \`5m\`, found \`${period}\``);
  }
  return { k: 'clock', schedule: `every ${period}` };
}

/** `trigger every day 09:00Z`: once a day at a UTC time, which must carry the `Z`. */
export function daily(time: string): RoutineTrigger {
  if (!/^[0-9]{2}:[0-9]{2}Z$/.test(time)) {
    throw new Error(`daily: expected \`HH:MMZ\` (UTC is written out), found \`${time}\``);
  }
  return { k: 'clock', schedule: `every day ${time}` };
}

export interface WhenOptions {
  /** `debounce 60s`: wait for the burst to end. */
  readonly debounce?: string;
  /** `coalesce`: at most one pending run. */
  readonly coalesce?: boolean;
}

/** `trigger when shop.orders.created debounce 60s coalesce`: a change in the data. */
export function when(pattern: string, options: WhenOptions = {}): RoutineTrigger {
  const parts: string[] = [];
  if (options.debounce !== undefined) {
    if (!PERIOD.test(options.debounce)) {
      throw new Error(
        `when: debounce expects \`<n><unit>\` such as \`60s\`, found \`${options.debounce}\``,
      );
    }
    parts.push(`debounce ${options.debounce}`);
  }
  if (options.coalesce) parts.push('coalesce');
  return { k: 'change', pattern, modifiers: parts.join(' ') };
}

export interface RoutineOptions<P extends Record<string, TypeSpec>> {
  /** `routine name in domain`. */
  readonly in?: string;
  readonly trigger: RoutineTrigger;
  /** The `run(…)` parameters, declared exactly as an op's. */
  readonly params?: P;
}

/** `<pipeline> each -> <writes>`: a body that reads, then writes per row. */
export class Each {
  readonly pipeline: PipelineNode;
  readonly statements: readonly WriteStmtNode[];

  constructor(pipeline: PipelineNode, statements: readonly WriteStmtNode[]) {
    this.pipeline = pipeline;
    this.statements = statements;
  }
}

/**
 * `each ->`: run `writes` once for each row `q` produces. Inside the writes'
 * braces, and in an addressed delete's `id`, the names are the row's fields.
 */
export function each(q: Q, writes: WriteLike | WriteLike[]): Each {
  const statements = (Array.isArray(writes) ? writes : [writes]).map((w) => w.node());
  if (statements.length === 0) {
    throw new Error('each: `each ->` needs at least one write to run for each row');
  }
  return new Each(q.node(), statements);
}

/** A routine declaration. Flags chain after it, as on an op. */
export class Routine {
  readonly kind = 'routine' as const;
  readonly name: string;
  readonly domain: string | null;
  readonly trigger: RoutineTrigger;
  readonly params: RoutineNode['params'];
  readonly each: RoutineNode['each'];
  readonly statements: RoutineNode['statements'];
  unboundedFlag = false;
  unguardedFlag = false;
  reproducibleFlag = false;
  nopauseFlag = false;

  constructor(
    name: string,
    domain: string | null,
    trigger: RoutineTrigger,
    params: RoutineNode['params'],
    statements: RoutineNode['statements'],
    each: RoutineNode['each'] = null,
  ) {
    this.name = name;
    this.domain = domain;
    this.trigger = trigger;
    this.params = params;
    this.each = each;
    this.statements = statements;
  }

  unbounded(): this {
    this.unboundedFlag = true;
    return this;
  }

  /** `unguarded`: bypass, which the language makes explicit and greppable. */
  unguarded(): this {
    this.unguardedFlag = true;
    return this;
  }

  reproducible(): this {
    this.reproducibleFlag = true;
    return this;
  }

  /** `nopause`: the routine keeps running while its tenant is paused. */
  nopause(): this {
    this.nopauseFlag = true;
    return this;
  }

  node(): RoutineNode {
    return {
      kind: 'routine',
      name: this.name,
      domain: this.domain,
      trigger: this.trigger,
      params: this.params,
      flags: {
        unbounded: this.unboundedFlag,
        unguarded: this.unguardedFlag,
        reproducible: this.reproducibleFlag,
        nopause: this.nopauseFlag,
        public: false,
      },
      each: this.each,
      statements: this.statements,
    };
  }
}

/**
 * `routine(name, options, body)`.
 *
 * The body callback is handed the same {@link Body} builder and parameter
 * namespace as a write op's, and returns one or more write statements, or
 * an {@link each} over a pipeline.
 */
export function routine<P extends Record<string, TypeSpec> = Record<string, never>>(
  name: string,
  options: RoutineOptions<P>,
  build: (b: Body, p: ParamsOf<P>) => WriteLike | WriteLike[] | Each,
): Routine {
  const params = options.params ?? ({} as P);
  const paramNodes = Object.entries(params).map(([n, s]) => paramNode(n, s));
  const p = makeParams(Object.keys(params)) as ParamsOf<P>;
  const built = build(new Body(), p);
  if (built instanceof Each) {
    return new Routine(
      name,
      options.in ?? null,
      options.trigger,
      paramNodes,
      built.statements,
      built.pipeline,
    );
  }
  const statements = (Array.isArray(built) ? built : [built]).map((w) => w.node());
  if (statements.length === 0) {
    throw new Error(`routine ${name}: the body needs at least one statement`);
  }
  return new Routine(name, options.in ?? null, options.trigger, paramNodes, statements);
}
