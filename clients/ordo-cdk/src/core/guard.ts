/**
 * Guard walks — the closed sub-grammar of `access read` / `access write`.
 *
 * **Skeleton half.** The guard grammar (DL G1) has no functions, no
 * arithmetic, no `!=`, no ordering and no `in`; it is disjuncts of conjuncts
 * of three atom shapes. There is no vocabulary to pour in, which is why this
 * file is entirely hand-written and the generator emits nothing for it.
 *
 * # Distribution, and why it is not an optimisation
 *
 * `decl.rs` distributes a parenthesized guard into disjunctive normal form at
 * parse time rather than keeping a nested node, and gives the reason: three
 * separate rules are stated over "a disjunct" — at least one must reference
 * `$caller` (G2); a write guard is create-exempt only when *every* disjunct
 * contains a reverse step rooted at the new row; the lowering unions one
 * semi-join per disjunct. A nested node would make all three read past the
 * parentheses and answer about a walk the author did not write.
 *
 * So this builder distributes too, with the same algebra. It is sound for the
 * same reason: the guard grammar has no `not`, so there is no negation for De
 * Morgan to flip. If it did not distribute, a builder-authored guard and the
 * identical hand-written one would produce different manifests — which is
 * exactly the "second semantics" Track C forbids.
 */

import type { ExprNode, GuardAtomNode, GuardClauseNode, GuardExprNode } from './ir.js';

/** `$caller` — the only non-literal a guard comparison may name. */
export const CALLER = Symbol('$caller');
export type GuardRhs = typeof CALLER | { readonly node: ExprNode };

function atom(a: GuardAtomNode): GuardExprNode {
  return { disjuncts: [[a]] };
}

function segments(path: string | readonly string[]): string[] {
  return typeof path === 'string' ? path.split('.') : [...path];
}

/**
 * `path == $caller`, or `path == "literal"`.
 *
 * `==` and nothing else: G1 admits no other operator, so the builder has no
 * other method — the absent `.ne()` here is the grammar's refusal, one layer
 * earlier.
 */
export function eq(path: string | readonly string[], rhs: GuardRhs = CALLER): GuardExprNode {
  return atom(
    rhs === CALLER
      ? { k: 'cmp', path: segments(path), caller: true, lit: null }
      : { k: 'cmp', path: segments(path), caller: false, lit: rhs.node },
  );
}

/** A bare bool field, or a path to one. */
export function bare(path: string | readonly string[]): GuardExprNode {
  return atom({ k: 'bare', path: segments(path) });
}

/**
 * A reverse collection step: `projectMembers(member == $caller)`, optionally
 * behind forward hops (`project.projectMembers(…)`).
 */
export function step(path: string | readonly string[], inner: GuardExprNode): GuardExprNode {
  return atom({ k: 'step', path: segments(path), inner });
}

/** Conjunction, distributed over disjunction — the parser's own algebra. */
export function all(...parts: GuardExprNode[]): GuardExprNode {
  if (parts.length === 0) throw new Error('`and` needs at least one guard atom');
  return parts.reduce((acc, next) => ({
    disjuncts: acc.disjuncts.flatMap((a) => next.disjuncts.map((b) => [...a, ...b])),
  }));
}

/** Disjunction: the independent authorization paths, concatenated. */
export function any(...parts: GuardExprNode[]): GuardExprNode {
  if (parts.length === 0) throw new Error('`or` needs at least one guard atom');
  return { disjuncts: parts.flatMap((p) => p.disjuncts) };
}

/**
 * `access read open` — open by declaration (G9).
 *
 * A separate value rather than a walk that happens to be always-true,
 * because the language spells it as a separate thing on purpose: bypass is
 * "explicit and greppable", and a builder that let `open` be constructed
 * from combinators would let it be constructed by accident.
 */
export const OPEN: GuardClauseNode = { k: 'open' };

/** Wrap a walk as an `access` clause. */
export function walk(g: GuardExprNode): GuardClauseNode {
  return { k: 'walk', expr: g };
}

/** The whole guard vocabulary as one namespace, for `g.eq(…)` at call sites. */
export const g = { eq, bare, step, all, any, walk, OPEN, CALLER } as const;
