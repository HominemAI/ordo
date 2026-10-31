/**
 * Loading a descriptor, and the indexes every other module reads.
 *
 * The version gate is the only loud thing in this file, and it is loud on
 * purpose. `crates/ordo-plan/src/interface.rs` states the rule precisely:
 * `schema_version` moves on a **breaking** change — a removed key, a changed
 * meaning, a narrowed value set — and *adding a key is not breaking*. So this
 * package refuses an unknown **major** and accepts everything after it,
 * because a consumer that refused unknown minors would make every additive
 * server change a client outage, and one that accepted unknown majors would
 * read a document whose keys no longer mean what it thinks.
 */

import {
  SCHEMA_PREFIX,
  SUPPORTED_SCHEMA_MAJOR,
  type EntityDescriptor,
  type IdTypeDescriptor,
  type IdTypeEntry,
  type InterfaceDocument,
  type ListingEntry,
  type OpDescriptor,
} from './descriptor.js';

export class SchemaVersionError extends Error {
  readonly found: string;
  readonly supportedMajor = SUPPORTED_SCHEMA_MAJOR;

  constructor(found: string, why: string) {
    super(
      `@hominemai/ordo-react speaks ${SCHEMA_PREFIX}${SUPPORTED_SCHEMA_MAJOR} and this descriptor says ` +
        `${JSON.stringify(found)} — ${why}. Refusing to read it: a descriptor is derived truth, ` +
        `and guessing at a shape it does not have would hand a renderer facts nothing produced.`,
    );
    this.name = 'SchemaVersionError';
    this.found = found;
  }
}

export class MalformedDescriptorError extends Error {
  constructor(what: string) {
    super(`malformed interface descriptor: ${what}`);
    this.name = 'MalformedDescriptorError';
  }
}

/**
 * `<key>_<uuidv7>` (`typed-id-keys.md`). Keys are lower-case words joined by
 * `-` (`boar-acce`, `proj-memb`) and never contain `_`, so the *first*
 * underscore is the separator and splitting on the last would be a bug that
 * only shows up on multi-word entity names.
 */
export interface TypedId {
  readonly key: string;
  readonly body: string;
  readonly raw: string;
}

export function parseTypedId(id: string): TypedId | null {
  const at = id.indexOf('_');
  if (at <= 0 || at === id.length - 1) return null;
  return { key: id.slice(0, at), body: id.slice(at + 1), raw: id };
}

/**
 * A loaded descriptor plus the lookups a runtime needs. Immutable, cheap to
 * hold, and safe to share across a whole app — it describes shape, never
 * rows (`ui-gen.md` §1, "module ≠ access").
 */
export class OrdoInterface {
  readonly doc: InterfaceDocument;

  readonly #ops: ReadonlyMap<string, OpDescriptor>;
  readonly #entities: ReadonlyMap<string, EntityDescriptor>;
  readonly #idTypesByKey: ReadonlyMap<string, IdTypeDescriptor>;
  readonly #idTypesByEntity: ReadonlyMap<string, IdTypeDescriptor>;
  readonly #entityByKey: ReadonlyMap<string, EntityDescriptor>;

  constructor(doc: InterfaceDocument) {
    this.doc = doc;
    this.#ops = new Map(doc.ops.map((o) => [o.op, o]));
    this.#entities = new Map(doc.entities.map((e) => [e.entity, e]));
    this.#entityByKey = new Map(doc.entities.map((e) => [e.key, e]));
    this.#idTypesByKey = new Map(doc.id_types.map((t) => [t.key, t]));
    this.#idTypesByEntity = new Map(doc.id_types.map((t) => [t.entity, t]));
  }

  get domain(): string {
    return this.doc.domain;
  }

  get digest(): string {
    return this.doc.digest;
  }

  get ops(): readonly OpDescriptor[] {
    return this.doc.ops;
  }

  /**
   * The op, or `undefined`. Absence is not "no such op" — the descriptor is
   * catalog-scoped (`ui-gen.md` §1), so absence means *this token may not
   * call it*, and the two are deliberately indistinguishable here for the
   * same reason the door answers `access_denied` for an unregistered name.
   */
  op(name: string): OpDescriptor | undefined {
    return this.#ops.get(name);
  }

  /** Like {@link op}, but throws rather than letting `undefined` travel. */
  requireOp(name: string): OpDescriptor {
    const op = this.#ops.get(name);
    if (!op) {
      throw new MalformedDescriptorError(
        `no op ${JSON.stringify(name)} in this catalog — either it does not exist, or this token may not call it`,
      );
    }
    return op;
  }

  entity(name: string): EntityDescriptor | undefined {
    return this.#entities.get(name);
  }

  entityForKey(key: string): EntityDescriptor | undefined {
    return this.#entityByKey.get(key);
  }

  idTypeForKey(key: string): IdTypeDescriptor | undefined {
    return this.#idTypesByKey.get(key);
  }

  idTypeForEntity(entity: string): IdTypeDescriptor | undefined {
    return this.#idTypesByEntity.get(entity);
  }

  /**
   * Every read op that returns `entity`'s rows without being addressed by one
   * of its ids — `id_types[].list`, **read, not reconstructed**.
   *
   * Under `ordo.interface/1` this was a heuristic in a package whose whole
   * argument is that it reads facts: the descriptor indexed ops by the id
   * types they *accepted* and never by the entity they *listed*, so the only
   * route to a picker's candidates was `result.row_identity` plus a guess.
   * The guess could not tell "nothing lists this entity" from "my rule missed
   * one", and it ranked `std.E.list` first by NAME, which is the exact thing
   * `call` was stamped to make unnecessary. `/2` states the index; this reads
   * it, in the emitter's order.
   *
   * Note what is *not* filtered here: an op with required parameters is still
   * a listing of the entity. Whether it can stock a **blind** picker is a
   * question about `params[].required` — a fact this package already holds —
   * and {@link listOpsForBlindPicker} asks it separately, so a caller that can
   * supply the parameter is not denied the op.
   */
  listOpsFor(entity: string): readonly OpDescriptor[] {
    return this.#resolve(this.#idTypesByEntity.get(entity)?.list);
  }

  /**
   * The subset a picker with nothing in hand can call: no required parameter
   * at all. `[]` means genuinely blind, and the descriptor now says so
   * rather than leaving it indistinguishable from a failed reconstruction.
   */
  listOpsForBlindPicker(entity: string): readonly OpDescriptor[] {
    return this.listOpsFor(entity).filter((o) => o.params.every((p) => !p.required));
  }

  /**
   * Read ops that return **one row of `entity`, addressed by its id** —
   * `id_types[].get`.
   *
   * Deliberately not {@link detailOpsFor}. `detail` is every read that takes
   * an `entity` id whatever it returns, so it contains ops like a deals query
   * taking a category id and returning products. Reading a row back through
   * one of those and presenting it as "the current value" is the failure the
   * CAS path must not have, and under `/1` that path had nothing better to
   * pick from.
   */
  getOpsFor(entity: string): readonly OpDescriptor[] {
    return this.#resolve(this.#idTypesByEntity.get(entity)?.get);
  }

  /** The parameter `op` takes an `entity` id in, per the listing index. */
  getParamFor(entity: string, op: string): string | null {
    return this.#idTypesByEntity.get(entity)?.get.find((g) => g.op === op)?.param ?? null;
  }

  /** Read ops addressed by an id of `entity` — the detail half of `id_types`. */
  detailOpsFor(entity: string): readonly IdTypeEntryWithOp[] {
    const t = this.#idTypesByEntity.get(entity);
    if (!t) return [];
    return t.detail.flatMap((d) => {
      const op = this.#ops.get(d.op);
      return op ? [{ ...d, descriptor: op }] : [];
    });
  }

  /**
   * Listing entries to op descriptors. An entry naming an op that is not in
   * `ops` is dropped rather than thrown on: the two halves are emitted from
   * one scoped list, so it cannot happen, and if it ever did, a missing
   * picker beats a crash on load.
   */
  #resolve(entries: readonly ListingEntry[] | undefined): readonly OpDescriptor[] {
    return (entries ?? []).flatMap((e) => {
      const op = this.#ops.get(e.op);
      return op ? [op] : [];
    });
  }
}

export interface IdTypeEntryWithOp extends IdTypeEntry {
  readonly descriptor: OpDescriptor;
}

/**
 * Parse and gate a raw `interface.json`.
 *
 * Structural validation is deliberately shallow: the top-level shape and the
 * version, and nothing below them. The descriptor is *emitted* by the same
 * build that emits the schema — it is derived truth, not user input — so a
 * deep validator here would be a second, drifting statement of the contract,
 * which is the exact failure `interface.rs` refuses for itself. What this
 * package owes is the version gate, and it pays that in full.
 */
export function parseInterface(raw: unknown): OrdoInterface {
  if (typeof raw !== 'object' || raw === null) {
    throw new MalformedDescriptorError('expected a JSON object at the top level');
  }
  const doc = raw as Partial<InterfaceDocument>;

  const version = doc.schema_version;
  if (typeof version !== 'string') {
    throw new MalformedDescriptorError(
      'no `schema_version` — every emitted descriptor carries one, so a document without it is not one',
    );
  }
  if (!version.startsWith(SCHEMA_PREFIX)) {
    throw new SchemaVersionError(version, `it is not an ${SCHEMA_PREFIX}* document at all`);
  }
  const majorText = version.slice(SCHEMA_PREFIX.length).split('.')[0] ?? '';
  const major = Number.parseInt(majorText, 10);
  if (!Number.isInteger(major)) {
    throw new SchemaVersionError(version, 'its major version is not an integer');
  }
  if (major !== SUPPORTED_SCHEMA_MAJOR) {
    throw new SchemaVersionError(
      version,
      major > SUPPORTED_SCHEMA_MAJOR
        ? `major ${major} is newer than this package understands; upgrade @hominemai/ordo-react`
        : `major ${major} is older than this package understands; regenerate the descriptor`,
    );
  }

  for (const key of ['ops', 'entities', 'id_types'] as const) {
    if (!Array.isArray(doc[key])) {
      throw new MalformedDescriptorError(`\`${key}\` must be an array`);
    }
  }
  if (typeof doc.domain !== 'string') {
    throw new MalformedDescriptorError('`domain` must be a string');
  }

  return new OrdoInterface(doc as InterfaceDocument);
}
