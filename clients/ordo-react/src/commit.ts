/**
 * CAS-by-default commit — `ui-gen.md` §1, settled:
 *
 * > the generated commit path is **CAS-by-default** — the fetched row's `rev`
 * > rides the write; `cas_miss` is a typed conflict the runtime resolves by
 * > refetch + conflict presentation, never a silent overwrite.
 *
 * Three rules follow, and this module exists to make all three unavoidable
 * rather than merely available:
 *
 * 1. **A CAS op cannot be committed without a `rev`.** Not "defaults to CAS
 *    if you pass one" — the write is refused locally, before the wire, with
 *    the door's own `missing_param`. A runtime that let the `rev` be omitted
 *    would make last-write-wins the easy path, which is the thing the rule
 *    exists to prevent.
 * 2. **`cas_miss` never retries.** It becomes a conflict *state* the consumer
 *    must resolve. There is a refetch affordance and there is no automatic
 *    anything.
 * 3. **A retry after a conflict needs the refetched `rev`.** Re-submitting the
 *    stale one is refused locally, so "retry" cannot quietly mean "retry the
 *    losing write".
 */

import type { OpResponse, Params, Row } from './client.js';
import { DescriptorClient } from './client.js';
import type { OpDescriptor } from './descriptor.js';
import { deriveForm, type FormModel } from './form.js';
import { OrdoLocalRefusal, OrdoRefusal, type Refusal } from './refusal.js';

export interface CommitInput {
  /** Bound params, `rev` excluded — the CAS path supplies it. */
  readonly values: Params;
  /**
   * The row's `rev`, as fetched. Required for any op declaring a `rev`
   * parameter; refused locally when absent.
   */
  readonly rev?: number | undefined;
  /**
   * Reuse across retries of the *same* attempt so a transport retry replays
   * rather than re-executes. A conflict retry is a different attempt and
   * must mint a new one — the payload changed, and the same key with a
   * changed payload is `idempotency_mismatch` forever (§9).
   */
  readonly idempotencyKey?: string | undefined;
  readonly signal?: AbortSignal | undefined;
}

/** How the runtime found the row this write addresses, for the refetch. */
export interface CommitAddress {
  readonly param: string;
  readonly entity: string;
  readonly id: string;
  /** The read op that fetches it back. `null` when the catalog grants none. */
  readonly detailOp: string | null;
}

/**
 * What reading back the row a write lost against found. Three answers that
 * a bare `null` ran together:
 *
 * - `found`: the row as it is stored now.
 * - `deleted`: the catalog grants a read for it and the row is gone (or the
 *   door answered `not_found`), so the conflict is a delete, not an edit.
 * - `unavailable`: the catalog grants no read to fetch it through, so what is
 *   stored now cannot be known from here.
 */
export type ConflictLookup =
  | { readonly status: 'found'; readonly row: Row }
  | { readonly status: 'deleted' }
  | { readonly status: 'unavailable' };

export interface CasConflict {
  readonly kind: 'cas_miss';
  readonly refusal: Refusal;
  /** Exactly what was sent, so a merge UI can diff it against the fresh row. */
  readonly attempted: Params;
  readonly sentRev: number | null;
  readonly address: CommitAddress | null;
  /**
   * Refetch the row the write lost against. Returns `null` when the catalog
   * grants no detail op for the entity, or the row is gone — both of which a
   * merge UI must handle, and neither of which this runtime will paper over.
   */
  readonly refetch: () => Promise<Row | null>;
  /**
   * Like {@link refetch}, but says which of the three answers it is, so a
   * merge UI can tell "deleted elsewhere" from "cannot be read from here".
   */
  readonly lookup: () => Promise<ConflictLookup>;
}

export type CommitOutcome<R = Row> =
  | { readonly status: 'committed'; readonly response: OpResponse<R> }
  | { readonly status: 'conflict'; readonly conflict: CasConflict }
  | { readonly status: 'refused'; readonly refusal: Refusal };

/**
 * The address a CAS write is about: the id parameter pointing at an entity
 * the op actually writes.
 *
 * `statements[]` names the entities the op writes and `params[].id_entity`
 * names what each id addresses; their intersection is the row. Where the
 * intersection is not exactly one param the answer is `null` rather than a
 * guess — a refetch of the wrong row presented as "the current value" is a
 * worse failure than no refetch at all.
 */
export function commitAddress(op: OpDescriptor, values: Params, detailOp: string | null): CommitAddress | null {
  const written = new Set(op.statements.map((s) => s.entity));
  const hits = op.params.filter((p) => p.id_entity && written.has(p.id_entity));
  const only = hits.length === 1 ? hits[0] : undefined;
  if (!only?.id_entity) return null;
  const id = values[only.name];
  if (typeof id !== 'string') return null;
  return { param: only.name, entity: only.id_entity, id, detailOp };
}

/**
 * One commit attempt. Never retries, never widens a refusal, and refuses a
 * missing `rev` before the wire.
 */
export async function commitOnce<R = Row>(
  client: DescriptorClient,
  opName: string,
  input: CommitInput,
  /** The op's form model when the caller already holds it, so a commit does not derive it again (L6). */
  known?: FormModel,
): Promise<CommitOutcome<R>> {
  const op = client.iface.requireOp(opName);
  const model: FormModel = known ?? deriveForm(client.iface, opName);

  const params: Record<string, unknown> = { ...input.values };

  if (model.revField) {
    // Rule 1. A CAS op with no `rev` is not a write this runtime will make.
    if (input.rev === undefined || input.rev === null) {
      const refusal = new OrdoLocalRefusal({
        class: 'missing_param',
        message:
          `${op.op} is CAS-by-default: it declares \`rev\`, so the fetched row's revision must ride the ` +
          `write (ui-gen.md §1). Fetch the row, pass its \`rev\`, and handle the \`cas_miss\` conflict — ` +
          `there is no last-write-wins path through this runtime.`,
        op: op.op,
        param: model.revField.name,
      });
      return { status: 'refused', refusal: refusal.refusal };
    }
    params[model.revField.name] = input.rev;
  } else if (input.rev !== undefined) {
    // A `rev` on an op that declares none would be `unknown_param` at the
    // door. Say so here rather than let the caller learn it from a 400.
    const refusal = new OrdoLocalRefusal({
      class: 'unknown_param',
      message: `${op.op} declares no \`rev\` parameter (mode: ${op.mode ?? 'n/a'}), so there is no CAS to assert`,
      op: op.op,
      param: 'rev',
    });
    return { status: 'refused', refusal: refusal.refusal };
  }

  try {
    const response = await client.commit<R>(opName, {
      params: params as Params,
      idempotencyKey: input.idempotencyKey,
      signal: input.signal,
    });
    return { status: 'committed', response };
  } catch (e) {
    if (!(e instanceof OrdoRefusal)) throw e;
    const refusal = e.refusal as Refusal;

    if (refusal.class !== 'cas_miss') {
      return { status: 'refused', refusal };
    }

    // Rule 2. A CAS miss is a conflict state, not a retry.
    const detailOps = commitDetailOps(client, op, input.values);
    const address = commitAddress(op, input.values, detailOps);
    const lookup = async (): Promise<ConflictLookup> => {
      if (!address?.detailOp) return { status: 'unavailable' };
      // The parameter comes from the listing index, which recorded it when
      // it classified the op as a row fetch. Falling back to `'id'` was
      // fine while there was nothing better to read; it is not a name this
      // package needs to know any more.
      const param =
        client.iface.getParamFor(address.entity, address.detailOp) ??
        client.iface.requireOp(address.detailOp).params.find((p) => p.id_entity === address.entity)?.name;
      if (!param) return { status: 'unavailable' };
      try {
        const res = await client.fetchOp<Row>(address.detailOp, { params: { [param]: address.id } });
        const row = res.rows[0];
        return row ? { status: 'found', row } : { status: 'deleted' };
      } catch (e) {
        if (e instanceof OrdoRefusal && e.refusal.class === 'not_found') return { status: 'deleted' };
        throw e;
      }
    };
    const conflict: CasConflict = {
      kind: 'cas_miss',
      refusal,
      attempted: input.values,
      sentRev: input.rev ?? null,
      address,
      refetch: async () => {
        const found = await lookup();
        return found.status === 'found' ? found.row : null;
      },
      lookup,
    };
    return { status: 'conflict', conflict };
  }
}

/**
 * The read op that fetches back the row a conflicted write addressed.
 *
 * **`id_types[].get`, not `detail`.** `detail` is every read that *accepts*
 * an id of the entity, whatever it returns — a deals query taking a category
 * id and returning products is in it — so taking its first entry was a coin
 * flip that happened to land right in the market fixture. `get` is the ops
 * whose rows carry that entity's identity *and* are addressed by its id,
 * which is the definition of "read this row back". `/1` had no way to say it;
 * presenting another entity's row as "the current value" is precisely the
 * failure the CAS path exists to avoid.
 */
function commitDetailOps(client: DescriptorClient, op: OpDescriptor, values: Params): string | null {
  const written = new Set(op.statements.map((s) => s.entity));
  const hits = op.params.filter((p) => p.id_entity && written.has(p.id_entity));
  const only = hits.length === 1 ? hits[0] : undefined;
  if (!only?.id_entity || typeof values[only.name] !== 'string') return null;
  return client.iface.getOpsFor(only.id_entity)[0]?.op ?? null;
}

/**
 * Rule 3: retry a conflicted write against a freshly fetched row.
 *
 * The new `rev` must differ from the one that lost. Re-sending the stale rev
 * is the same write that already failed, and a runtime that let it through
 * would turn "retry" into a spin.
 */
export async function retryAfterConflict<R = Row>(
  client: DescriptorClient,
  opName: string,
  conflict: CasConflict,
  merged: CommitInput,
): Promise<CommitOutcome<R>> {
  if (merged.rev !== undefined && merged.rev === conflict.sentRev) {
    const refusal = new OrdoLocalRefusal({
      class: 'cas_miss',
      message:
        `retrying ${opName} with rev ${merged.rev} — the same revision the conflicted write asserted. ` +
        `Refetch the row (conflict.refetch()) and retry against its current \`rev\`.`,
      op: opName,
      param: 'rev',
    });
    return { status: 'refused', refusal: refusal.refusal };
  }
  // A new attempt is a new payload, so it needs a new idempotency key: the
  // same key with a changed payload is `idempotency_mismatch`, and for a
  // creating op that binding is permanent (§9).
  const next: CommitInput = { ...merged };
  return commitOnce<R>(client, opName, next);
}
