/**
 * The release rule: when a member may leave a tenant on their own, and what
 * leaving does to the rows that point at them (checklist 27aq).
 *
 * **Skeleton half**, like `guard.ts`. The conditions are guard walks with a
 * different subject: inside a release rule the comparison names `$member`,
 * the seat being released. The IR is the guard IR, so `g.eq('owner')` and
 * `g.eq('owner', MEMBER)` are the same node; the printer spells the subject
 * by position, as the parser reads it.
 *
 * ```ts
 * release.member.open();
 * release.member.locked();
 * release.member.when(
 *   rel.all(rel.none('invoices', g.all(g.eq('owner', MEMBER), g.eq('status', lit('open'))))),
 *   [rel.remove('drafts.author'), rel.reassign('tasks.assignee', rel.owner)],
 * );
 * ```
 *
 * With no release rule a program's members are **locked**: nobody leaves on
 * their own (owner ruling, 2026-10-05).
 */

import { CALLER } from './guard.js';
import type {
  GuardExprNode,
  ReassignTargetNode,
  ReleaseAtomNode,
  ReleaseCondNode,
  ReleaseEffectNode,
  ReleaseNode,
} from './ir.js';

/**
 * `$member`, the seat being released. The same value as `CALLER`: both mark
 * the guard's own subject, and the position decides how it is spelled.
 */
export const MEMBER = CALLER;

function atom(a: ReleaseAtomNode): ReleaseCondNode {
  return { disjuncts: [[a]] };
}

function split(path: string): [string, string] {
  const [entity, field, ...rest] = path.split('.');
  if (!entity || !field || rest.length > 0) {
    throw new Error(`expected \`entity.field\`, found \`${path}\``);
  }
  return [entity, field];
}

/** `entity(inner)`: some row of `entity` matches. */
export function has(entity: string, inner: GuardExprNode): ReleaseCondNode {
  return atom({ negated: false, entity, inner });
}

/** `not entity(inner)`: no row of `entity` matches. */
export function none(entity: string, inner: GuardExprNode): ReleaseCondNode {
  return atom({ negated: true, entity, inner });
}

/** Conjunction, distributed over disjunction, as the parser does. */
export function all(...parts: ReleaseCondNode[]): ReleaseCondNode {
  if (parts.length === 0) throw new Error('`and` needs at least one condition');
  return parts.reduce((acc, next) => ({
    disjuncts: acc.disjuncts.flatMap((a) => next.disjuncts.map((b) => [...a, ...b])),
  }));
}

/** Disjunction. */
export function any(...parts: ReleaseCondNode[]): ReleaseCondNode {
  if (parts.length === 0) throw new Error('`or` needs at least one condition');
  return { disjuncts: parts.flatMap((p) => p.disjuncts) };
}

/** `remove tasks.assignee`: the rows whose ref names the seat are deleted. */
export function remove(path: string): ReleaseEffectNode {
  const [entity, field] = split(path);
  return { k: 'remove', entity, field };
}

/** `reassign tasks.assignee to a or b`: the first target that names an active member. */
export function reassign(path: string, ...to: ReassignTargetNode[]): ReleaseEffectNode {
  if (to.length === 0) throw new Error('`reassign` needs at least one target');
  const [entity, field] = split(path);
  return { k: 'reassign', entity, field, to };
}

/** The tenant's longest-standing active owner other than the leaver. */
export const owner: ReassignTargetNode = { k: 'owner' };

/** `people(member == $member).manager`: a member named by one of the app's rows. */
export function lookup(entity: string, inner: GuardExprNode, field: string): ReassignTargetNode {
  return { k: 'lookup', entity, inner, field };
}

function node(
  mode: ReleaseNode['mode'],
  guard: ReleaseCondNode | null,
  effects: readonly ReleaseEffectNode[],
): ReleaseNode {
  return { kind: 'release', subject: 'member', mode, guard, effects };
}

/** `release member …`, the one release rule a program may carry. */
export const release = {
  member: {
    open: (effects: readonly ReleaseEffectNode[] = []): ReleaseNode => node('open', null, effects),
    locked: (effects: readonly ReleaseEffectNode[] = []): ReleaseNode =>
      node('locked', null, effects),
    when: (cond: ReleaseCondNode, effects: readonly ReleaseEffectNode[] = []): ReleaseNode =>
      node('when', cond, effects),
  },
} as const;

/** The release vocabulary as one namespace, for `rel.none(…)` at call sites. */
export const rel = { has, none, all, any, remove, reassign, owner, lookup, MEMBER } as const;
