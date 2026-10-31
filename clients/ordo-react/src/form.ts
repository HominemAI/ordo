/**
 * Form derivation — `params[]` in, field model out.
 *
 * Pure, synchronous, and deliberately the whole of `useOpForm`'s brain: the
 * hook is a `useState` around these functions, so everything interesting is
 * testable against the golden descriptor with no React and no server.
 *
 * As of `ordo.interface/2` **nothing here is derived**. The two things that
 * were — a parameter's option list, and "which op lists entity E" — are
 * facts the descriptor now carries (`params[].enum_values` through
 * `field_ref`, and `id_types[].list`/`.get`), so the joins that stood in for
 * them are gone rather than kept as a fallback. There is no fallback to keep
 * them for: the major moved, `parseInterface` refuses a `/1` document
 * outright, and a heuristic that can never run is a heuristic nobody
 * maintains.
 */

import type { OpDescriptor, OrdoType, ParamDescriptor } from './descriptor.js';
import type { ParamValue } from './client.js';
import { parseTypedId, type OrdoInterface } from './load.js';
import type { Refusal } from './refusal.js';
import { isJsonValue } from './json.js';

/**
 * How a form field is filled. `json` (28b) is a document: its value is any
 * JSON value, so a renderer offers a structured or JSON-text editor rather
 * than a one-line input, and parses before setting the value.
 */
export type FieldKind = 'scalar' | 'reference' | 'enum' | 'rev' | 'json';

/** Where a picker gets its candidates. Op *names*; the hook runs them. */
export interface CandidateSource {
  readonly entity: string;
  /** The typed-id prefix an accepted value must carry. */
  readonly key: string;
  /**
   * Read ops returning this entity's rows, in the descriptor's own order.
   * May be empty, and empty is now a stated fact rather than a failed guess.
   */
  readonly listOps: readonly string[];
  /** The subset callable with no arguments — what a cold picker can use. */
  readonly unparameterizedListOps: readonly string[];
  /**
   * Read ops that resolve one chosen id **to that row**, for the picker's
   * label. `id_types[].get`, not `detail`: an op that merely accepts the id
   * may return something else entirely.
   */
  readonly detailOps: readonly string[];
  /** True when nothing in the catalog can stock this picker cold. */
  readonly blind: boolean;
}

export interface FormField {
  readonly name: string;
  readonly type: OrdoType;
  readonly required: boolean;
  /** Canonical digits, never a number — see `ParamDescriptor.max`. */
  readonly max: string | null;
  /** Canonical digits. `"0"` on a `take` count, `null` otherwise. */
  readonly min: string | null;
  readonly kind: FieldKind;
  /** `reference` fields only. */
  readonly candidates: CandidateSource | null;
  /** `enum` fields only — the option list, in declaration order. */
  readonly options: readonly string[] | null;
  /**
   * A field the consumer must not draw: the runtime owns its value. Today
   * that is exactly the `rev` field, which the CAS path fills from the
   * fetched row.
   */
  readonly hidden: boolean;
}

export interface FormModel {
  readonly op: string;
  readonly kind: OpDescriptor['kind'];
  readonly mode: string | null;
  readonly fields: readonly FormField[];
  /** The CAS field, when the op declares one. */
  readonly revField: FormField | null;
  /** Everything the consumer draws. */
  readonly visible: readonly FormField[];
  /**
   * `mode` is the compiler's static label. A skin shows it verbatim rather
   * than guessing at a verb: "creates", "converges", "deletes subtree".
   */
  readonly danger: boolean;
}

/** Build the field model for one op. */
export function deriveForm(iface: OrdoInterface, opName: string): FormModel {
  const op = iface.requireOp(opName);
  const fields = op.params.map((p) => deriveField(iface, op, p));
  const revField = fields.find((f) => f.kind === 'rev') ?? null;
  return {
    op: op.op,
    kind: op.kind,
    mode: op.mode,
    fields,
    revField,
    visible: fields.filter((f) => !f.hidden),
    // No `danger` annotation exists yet: `definition-language.md` §10.3
    // marks it inert and the emitter deliberately omits it rather than ship
    // an always-null field a renderer would learn to trust. Until it lands,
    // the honest proxy is the kind — a delete is destructive by derivation.
    danger: op.kind === 'delete',
  };
}

function deriveField(iface: OrdoInterface, op: OpDescriptor, p: ParamDescriptor): FormField {
  if (isRevParam(op, p)) {
    return {
      name: p.name,
      type: p.type,
      required: p.required,
      max: p.max,
      min: p.min,
      kind: 'rev',
      candidates: null,
      options: null,
      hidden: true,
    };
  }

  if (p.id_entity) {
    return {
      name: p.name,
      type: p.type,
      required: p.required,
      max: p.max,
      min: p.min,
      kind: 'reference',
      candidates: candidateSource(iface, p.id_entity),
      options: null,
      hidden: false,
    };
  }

  if (p.type === 'json') {
    return {
      name: p.name,
      type: p.type,
      required: p.required,
      max: p.max,
      min: p.min,
      kind: 'json',
      candidates: null,
      options: null,
      hidden: false,
    };
  }

  const options = enumOptionsFor(p);
  return {
    name: p.name,
    type: p.type,
    required: p.required,
    max: p.max,
    min: p.min,
    kind: options ? 'enum' : 'scalar',
    candidates: null,
    options,
    hidden: false,
  };
}

/**
 * Is this the CAS parameter — the one carrying the fetched row's revision?
 *
 * **By declaration, not by name.** `statements[].cas_param` names the
 * parameter each statement asserts as the row's `rev`, read off the braces by
 * the compiler, so this is a membership test over a stated fact. Under
 * `ordo.interface/1` the descriptor said only "this op is a mutation", so the
 * package matched `name === 'rev' && type === 'int'` and treated `call.cas`
 * as corroboration — which worked, because the language reserves `rev`, and
 * which was still a name heuristic in a package that claims to have none.
 */
export function isRevParam(op: OpDescriptor, p: ParamDescriptor): boolean {
  return op.statements.some((s) => s.cas_param === p.name);
}

export function candidateSource(iface: OrdoInterface, entity: string): CandidateSource {
  const listOps = iface.listOpsFor(entity).map((o) => o.op);
  const cold = iface.listOpsForBlindPicker(entity).map((o) => o.op);
  const detailOps = iface.getOpsFor(entity).map((o) => o.op);
  const key = iface.entity(entity)?.key ?? iface.idTypeForEntity(entity)?.key ?? '';
  return {
    entity,
    key,
    listOps,
    unparameterizedListOps: cold,
    detailOps,
    blind: cold.length === 0,
  };
}

/**
 * Options for an enum-valued parameter — **read, not joined**.
 *
 * Under `ordo.interface/1` this was the package's second heuristic:
 * `entities[].fields[].enum_values` existed and `interface.rs` said of it "a
 * dropdown is derivable from this and nothing else", but `params[]` carried
 * no link back, so the bridge was the op's `statements[]` joined to a field
 * of the same *name*, refusing when two written entities both had one.
 *
 * `/2` derives the link in the producer, where the AST says exactly which
 * field each parameter fills — through ref paths and read source conditions
 * a statement list cannot see — resolves it against the entity half, and
 * emits the value set on the parameter. So this is one property access now,
 * and the ambiguity refusal lives where the ambiguity actually is.
 */
export function enumOptionsFor(p: ParamDescriptor): readonly string[] | null {
  return p.enum_values ?? null;
}

// ── validation ───────────────────────────────────────────────────────────

export type FormValues = Readonly<Record<string, ParamValue | undefined>>;

export interface FormIssue extends Pick<Refusal, 'class' | 'message' | 'param'> {
  readonly class: 'missing_param' | 'invalid_param' | 'bound_violation';
}

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const MONEY = /^-?\d+(\.\d+)?$/;
const DURATION = /^-?\d+(ms|s|m|h|d|w|mo|y)$/;

/**
 * The door's own checks (`api-engine-design.md` §5), run locally.
 *
 * This is not a second contract: every class raised here is one the door
 * would raise for the same value, and the point of running them client-side
 * is that the user learns before the round trip. Where a check *cannot* be
 * made honestly on the client it is not made — nothing here guesses at
 * guards, access, or uniqueness.
 */
export function validateForm(model: FormModel, values: FormValues, iface: OrdoInterface): readonly FormIssue[] {
  const issues: FormIssue[] = [];
  for (const f of model.fields) {
    const v = values[f.name];

    // Explicit `null` counts as absent (§4, `missing_param`).
    if (v === undefined || v === null || v === '') {
      if (f.required && !f.hidden) {
        issues.push({ class: 'missing_param', message: `${f.name} is required`, param: f.name });
      }
      continue;
    }
    const issue = checkValue(f, v, iface);
    if (issue) issues.push(issue);
  }
  return issues;
}

function checkValue(f: FormField, v: ParamValue, iface: OrdoInterface): FormIssue | null {
  const bad = (cls: FormIssue['class'], message: string): FormIssue => ({ class: cls, message, param: f.name });

  switch (f.type) {
    case 'int': {
      if (typeof v !== 'number' || !Number.isInteger(v)) {
        return bad('invalid_param', `${f.name} is \`int\`: a JSON integer, not ${JSON.stringify(v)}`);
      }
      // `max` and `min` are canonical DIGIT STRINGS, always. This branch used
      // to read `typeof f.max === 'number'` and was therefore dead: the
      // emitter has never written a JSON number here, and no descriptor in
      // the golden populated `max` at all, so nothing failed to notice.
      if (f.max !== null && v > Number(f.max)) {
        return bad('bound_violation', `${f.name} <= ${f.max}`);
      }
      // The derived floor on a `take` count. `LIMIT -1` is a Postgres error
      // and `take[keys] -1` is a silently empty page, so the door refuses it
      // and so does this.
      if (f.min !== null && v < Number(f.min)) {
        return bad('bound_violation', `${f.name} >= ${f.min}`);
      }
      return null;
    }
    case 'money': {
      // Exact decimal string, never a float (§5). A number here is the bug
      // that silently rounds money, so it is refused rather than coerced.
      if (typeof v !== 'string' || !MONEY.test(v)) {
        return bad('invalid_param', `${f.name} is \`money\`: an exact decimal string like "-12.50", never a float`);
      }
      if (f.max !== null && compareDecimal(v, f.max) > 0) {
        return bad('bound_violation', `${f.name} <= ${f.max}`);
      }
      return null;
    }
    case 'text': {
      if (typeof v !== 'string') return bad('invalid_param', `${f.name} is \`text\``);
      // A `max` on a `text` parameter can only ever be a character count:
      // `declparse::param_bound` refuses `<=` on `text` outright, so the only
      // producer is a platform op's `ParamTy::Text { max_len }`.
      // Characters (code points), as the door and Postgres count them; a
      // UTF-16 `.length` would refuse what the door accepts.
      const chars = Array.from(v).length;
      if (f.max !== null && chars > Number(f.max)) {
        return bad('bound_violation', `${f.name} is text(${f.max}); got ${chars} characters`);
      }
      if (f.options && !f.options.includes(v)) {
        return bad('invalid_param', `${f.name} is one of [${f.options.join(', ')}]`);
      }
      return null;
    }
    case 'date': {
      if (typeof v !== 'string' || !isCalendarDate(v)) {
        return bad('invalid_param', `${f.name} is \`date\`: "YYYY-MM-DD"`);
      }
      // Bounds on `date` compare lexicographically, which is chronological
      // for this format — the door's own rule, not a coincidence.
      if (f.max !== null && v > f.max) return bad('bound_violation', `${f.name} <= ${f.max}`);
      return null;
    }
    case 'bool':
      return typeof v === 'boolean' ? null : bad('invalid_param', `${f.name} is \`bool\``);
    case 'duration':
      return typeof v === 'string' && DURATION.test(v)
        ? null
        : bad('invalid_param', `${f.name} is \`duration\`: "<int><unit>", units ms s m h d w mo y`);
    case 'id': {
      if (typeof v !== 'string') return bad('invalid_param', `${f.name} is an \`id\``);
      const parsed = parseTypedId(v);
      if (!parsed) {
        return bad('invalid_param', `${f.name} is a typed id: "<prefix>_<body>"`);
      }
      // The typed-id check the door cannot skip and a UI should not either:
      // an id param declares its entity, so a value carrying another
      // entity's prefix is wrong before it is sent.
      const expected = f.candidates?.key;
      if (expected && parsed.key !== expected) {
        const actual = iface.entityForKey(parsed.key)?.entity;
        return bad(
          'invalid_param',
          `${f.name} addresses ${f.candidates?.entity} (prefix "${expected}_"), but got "${parsed.key}_"` +
            (actual ? ` — that is a ${actual} id` : ''),
        );
      }
      return null;
    }
    case 'datetime': {
      // Until this case existed, `datetime` fell through the `default`
      // below and the validator waved it through — the mirror image of the
      // engine's own defect, where `ordo_host::param_ty`'s wildcard bound
      // it as unbounded text. Both halves are now exhaustive.
      if (typeof v !== 'string' || !isDatetime(v)) {
        return bad('invalid_param', `${f.name} is \`datetime\`: "YYYY-MM-DDTHH:MM:SSZ" (UTC, "Z" required)`);
      }
      return null;
    }
    case 'uri': {
      // The same syntactic check the door makes, and deliberately no more:
      // the platform references a `uri` and does not fetch it, so neither
      // does a form. What is refused is the shape that is never an address
      // — no scheme, whitespace, control characters, or unbounded length.
      if (typeof v !== 'string' || !isUri(v)) {
        return bad(
          'invalid_param',
          `${f.name} is \`uri\`: a scheme then a body, no spaces or control characters, at most 2048 bytes`,
        );
      }
      return null;
    }
    case 'json':
      // Any JSON value, as the door takes it (28b). Refused here is only
      // what the wire would change: a non-finite number, a class instance,
      // `undefined` inside. Size and depth are the deployment's limits and
      // the door's to check.
      return isJsonValue(v)
        ? null
        : bad('invalid_param', `${f.name} is \`json\`: a JSON value (plain objects, arrays, finite numbers)`);
    case 'blob':
      // Unreachable through a well-formed descriptor: `blob` is refused as
      // a parameter type by the compiler (class `blob_parameter`), so no
      // op can declare one and no form can contain one. Named rather than
      // left to `default`, because the failure mode of the wildcard here is
      // a form that renders a file picker and posts the file as JSON.
      return bad(
        'invalid_param',
        `${f.name} is \`blob\`: bytes never ride a JSON parameter — upload through the blob door`,
      );
    // `decimal` is expression-only (DL §2) and never a parameter, and an
    // `enum` is a `text` with an option list, already handled by the `text`
    // case reading `f.options`. The `default` is therefore reached only by
    // a descriptor from a newer engine, and waving that through is right:
    // a client must not refuse a value it merely does not recognise.
    default:
      return null;
  }
}

/**
 * A real calendar date, not merely a parseable one.
 *
 * `Date.parse("2026-02-30")` succeeds and silently means March 2nd, which is
 * exactly the class of bug a client-side check exists to catch — the door
 * validates the date properly and would refuse it, so accepting it here would
 * mean the local check said yes to something the server says no to.
 */
function isCalendarDate(v: string): boolean {
  if (!DATE.test(v)) return false;
  const [y, m, d] = v.split('-').map((n) => Number.parseInt(n, 10)) as [number, number, number];
  const at = new Date(Date.UTC(y, m - 1, d));
  return at.getUTCFullYear() === y && at.getUTCMonth() === m - 1 && at.getUTCDate() === d;
}

/**
 * `YYYY-MM-DDTHH:MM:SSZ`, and nothing else — the door's own rule
 * (`ordo_api::bind::is_datetime`), mirrored so the local check and the
 * remote one agree.
 *
 * No offsets and no fractional seconds: accepting `+02:00` would mean this
 * client deciding a timezone conversion the platform has deliberately left
 * nowhere to happen. `60` seconds is accepted, because a leap second is a
 * real instant.
 */
function isDatetime(v: string): boolean {
  const m = /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2}):(\d{2})Z$/.exec(v);
  if (!m) return false;
  const [, date = '', hh = '', mm = '', ss = ''] = m;
  if (!isCalendarDate(date)) return false;
  return Number(hh) <= 23 && Number(mm) <= 59 && Number(ss) <= 60;
}

/**
 * A syntactic address, matching `ordo_api::bind::is_uri`. Deliberately not
 * an RFC 3986 parser: what a form owes a `uri` is the difference between an
 * address and a string pasted into the wrong box.
 */
function isUri(v: string): boolean {
  if (v.length === 0 || v.length > 2048) return false;
  // eslint-disable-next-line no-control-regex
  if (/[\s\u0000-\u001f\u007f]/.test(v)) return false;
  return /^[A-Za-z][A-Za-z0-9+.-]*:.+$/.test(v);
}

/** Exact comparison of two decimal strings, with no float in the middle. */
function compareDecimal(a: string, b: string): number {
  const [ai = '0', af = ''] = a.replace('-', '').split('.');
  const [bi = '0', bf = ''] = b.replace('-', '').split('.');
  const negA = a.startsWith('-');
  const negB = b.startsWith('-');
  if (negA !== negB) return negA ? -1 : 1;
  const sign = negA ? -1 : 1;
  if (ai.length !== bi.length) return sign * (ai.length - bi.length);
  if (ai !== bi) return sign * (ai < bi ? -1 : 1);
  const width = Math.max(af.length, bf.length);
  const ap = af.padEnd(width, '0');
  const bp = bf.padEnd(width, '0');
  if (ap === bp) return 0;
  return sign * (ap < bp ? -1 : 1);
}

/** Strip absent values and the runtime-owned ones; what actually goes on the wire. */
export function toParams(model: FormModel, values: FormValues): Record<string, ParamValue> {
  const out: Record<string, ParamValue> = {};
  for (const f of model.fields) {
    const v = values[f.name];
    if (v === undefined || v === null) continue;
    out[f.name] = v;
  }
  return out;
}
