/**
 * Typed-id action discovery — `ui-gen.md` §2, verbatim.
 *
 * > A write op *applies to a row* iff it declares an id-typed parameter whose
 * > entity prefix matches the row's id type — typed ids make this statically
 * > detectable — **and** the caller's token can call it. No annotation, no
 * > registration flag; the catalog is the source.
 *
 * Both halves are already discharged by the time a descriptor reaches this
 * package. The emitter derives the index from `params[].id_entity`, and it
 * derives it **after** the catalog filter, so `id_types[].actions` names only
 * ops this token may call — a viewer cannot learn that a manager-only
 * `archiveProject` exists by noticing which id types offer actions. This
 * module is therefore a *lookup*, not a rule, and that is the point: the rule
 * lives in one place and the runtime cannot restate it differently.
 */

import type { IdTypeEntry, OpDescriptor } from './descriptor.js';
import { parseTypedId, type OrdoInterface } from './load.js';

export interface DiscoveredOp extends IdTypeEntry {
  readonly descriptor: OpDescriptor;
  /** Bind the row's id into this op's own param name. */
  readonly bind: (id: string) => Record<string, string>;
}

export type RowKnowledge =
  /** A prefix no entity in this catalog claims. */
  | { readonly known: false; readonly reason: 'unparseable' | 'unknown-prefix'; readonly id: string; readonly key: string | null }
  /** A prefix an entity claims, with whatever the catalog grants over it. */
  | {
      readonly known: true;
      readonly id: string;
      readonly key: string;
      readonly entity: string;
      readonly actions: readonly DiscoveredOp[];
      readonly detail: readonly DiscoveredOp[];
      /**
       * True when the entity is described but no op in this catalog accepts
       * its id. Distinct from an unknown prefix, and the distinction matters
       * to a renderer: this row is real and has no affordances, rather than
       * being an id from somewhere else. `groupMembers` in the market
       * fixture is exactly this case.
       */
      readonly inert: boolean;
    };

/**
 * Resolve any typed id against the catalog.
 *
 * Three outcomes, and they are three because collapsing them loses the one
 * fact a renderer acts on: unparseable (not an id), unknown prefix (an id
 * this catalog does not describe — which, given catalog scoping, may mean
 * *not yours* rather than *not real*, and the UI must not claim otherwise),
 * and known.
 */
export function discover(iface: OrdoInterface, id: string): RowKnowledge {
  const parsed = parseTypedId(id);
  if (!parsed) return { known: false, reason: 'unparseable', id, key: null };

  const entity = iface.entityForKey(parsed.key);
  const idType = iface.idTypeForKey(parsed.key);
  if (!entity && !idType) {
    return { known: false, reason: 'unknown-prefix', id, key: parsed.key };
  }

  const name = idType?.entity ?? entity?.entity ?? parsed.key;
  const actions = resolve(iface, idType?.actions ?? []);
  const detail = resolve(iface, idType?.detail ?? []);
  return {
    known: true,
    id,
    key: parsed.key,
    entity: name,
    actions,
    detail,
    inert: actions.length === 0 && detail.length === 0,
  };
}

function resolve(iface: OrdoInterface, entries: readonly IdTypeEntry[]): readonly DiscoveredOp[] {
  return entries.flatMap((e) => {
    // Belt and braces: the index is derived after the catalog filter, so an
    // entry naming an op that is not in `ops[]` should be impossible. If it
    // ever happens the answer is to drop it, never to offer a button that
    // cannot be pressed.
    const descriptor = iface.op(e.op);
    if (!descriptor) return [];
    return [{ ...e, descriptor, bind: (id: string) => ({ [e.param]: id }) }];
  });
}

/**
 * Actions grouped the way a row menu wants them: the destructive ones last
 * and marked. There is no `danger` annotation to read yet
 * (`definition-language.md` §10.3 marks it inert), so `kind: 'delete'` is
 * the derivation, and it is stated as a derivation rather than dressed up as
 * the annotation it is standing in for.
 */
export interface ActionMenu {
  readonly safe: readonly DiscoveredOp[];
  readonly destructive: readonly DiscoveredOp[];
}

export function actionMenu(row: RowKnowledge): ActionMenu {
  if (!row.known) return { safe: [], destructive: [] };
  return {
    safe: row.actions.filter((a) => a.kind !== 'delete'),
    destructive: row.actions.filter((a) => a.kind === 'delete'),
  };
}
