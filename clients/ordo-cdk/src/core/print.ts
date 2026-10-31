/**
 * The printer — IR to canonical `.ordo` text.
 *
 * **Skeleton half**, and the one place the package writes Ordo rather than
 * describes it. Every *word* it prints comes from the IR, which came from the
 * generated vocabulary; what is written here is layout: where the line breaks
 * are, where the parentheses go, and in which order an entity's clauses
 * appear.
 *
 * # Canonical, and what that word can honestly mean today
 *
 * `tooling-and-embedding-plan.md` Track C says `synth` emits canonical
 * `.ordo` *via `ordo fmt`*. There is no `ordo fmt` — `ordoc fmt` is Track A1
 * and unbuilt — so "canonical" here means *this printer's* one spelling, and
 * the isomorphism property is stated against the parser instead of against a
 * formatter: `parse(original)` and `parse(synth(...))` must be the same AST.
 * That is a weaker claim than byte equality and a stronger one than nothing,
 * and it is the claim `ordo-gen/tests/isomorphism.rs` actually checks. When
 * `ordo fmt` lands, this printer becomes its input and the claim tightens to
 * bytes.
 *
 * # Parenthesisation
 *
 * The two rules that matter, both from the grammar rather than from taste:
 *
 * - **Boolean mixing is parenthesized** (spec §1.2). `and` and `or` do not
 *   mix without parentheses, so a nested chain of the other connective is
 *   always wrapped. A nested chain of the *same* connective is wrapped too,
 *   because the parser flattens `a and b and c` into one node and would
 *   otherwise read a printed nesting as the flat form — a different tree.
 * - **Arithmetic follows conventional precedence** (`* /` over `+ -`), left
 *   associative, so a right operand of equal precedence under `-` or `/`
 *   needs parentheses and one under `+` or `*` does not.
 */

import type {
  AggNode,
  CondNode,
  Declaration,
  EntityNode,
  ExprNode,
  FieldNode,
  GuardAtomNode,
  GuardClauseNode,
  GuardExprNode,
  OpNode,
  ParamNode,
  RoutineNode,
  PipelineNode,
  ProjItemNode,
  ReassignTargetNode,
  ReleaseCondNode,
  ReleaseNode,
  ScalarType,
  SourceNode,
  StepNode,
  WriteStmtNode,
} from './ir.js';

/** Ordo string literal: double quotes, with the escapes the lexer resolves. */
export function strLit(s: string): string {
  let out = '"';
  for (const c of s) {
    if (c === '"') out += '\\"';
    else if (c === '\\') out += '\\\\';
    else if (c === '\n') out += '\\n';
    else if (c === '\t') out += '\\t';
    else if (c === '\r') out += '\\r';
    else out += c;
  }
  return out + '"';
}

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const POSITION = /^(0|[1-9][0-9]*)$/;

/**
 * A path. A segment that is a name is dotted. A segment that is not, which
 * only a key inside a `json` document can be, is bracketed: a position as
 * its digits (`tags[0]`), anything else as a string (`prefs["my key"]`).
 * The parser reads both spellings of a name-shaped key as one segment, so
 * the dotted one is the canonical one.
 */
export function printPath(segments: readonly string[]): string {
  return segments
    .map((s, i) => {
      if (i === 0) return s;
      if (NAME.test(s)) return `.${s}`;
      if (POSITION.test(s)) return `[${s}]`;
      return `[${strLit(s)}]`;
    })
    .join('');
}

const MUL_DIV = new Set(['*', '/']);

function precedence(op: string): number {
  return MUL_DIV.has(op) ? 2 : 1;
}

export function printExpr(e: ExprNode): string {
  switch (e.k) {
    case 'int':
      return e.v;
    case 'dec':
      return e.v;
    case 'str':
      return strLit(e.v);
    case 'date':
      return e.v;
    case 'datetime':
      return e.v;
    case 'dur':
      return `${e.n}${e.unit}`;
    case 'bool':
      return e.v ? 'true' : 'false';
    case 'null':
      return 'null';
    case 'param':
      return `$${e.name}`;
    case 'path':
      return printPath(e.segments);
    case 'neg':
      // `-1050` is one token to the lexer only when the operand is a bare
      // literal; anything else takes parentheses so `-a * b` cannot be
      // reparsed as `-(a * b)`.
      return e.e.k === 'int' || e.e.k === 'dec'
        ? `-${printExpr(e.e)}`
        : `-(${printExpr(e.e)})`;
    case 'bin': {
      const p = precedence(e.op);
      const l = e.l.k === 'bin' && precedence(e.l.op) < p ? `(${printExpr(e.l)})` : printExpr(e.l);
      const rNeedsParens =
        e.r.k === 'bin' &&
        (precedence(e.r.op) < p || (precedence(e.r.op) === p && (e.op === '-' || e.op === '/')));
      const r = rNeedsParens ? `(${printExpr(e.r)})` : printExpr(e.r);
      return `${l} ${e.op} ${r}`;
    }
    case 'call':
      return `${e.name}(${e.args.map(printExpr).join(', ')})`;
    case 'cond': {
      const arms = e.arms.map((a) => `${printCond(a.c)} -> ${printExpr(a.v)}`);
      return `cond(${[...arms, printExpr(e.else)].join(', ')})`;
    }
  }
}

/** Does this condition need wrapping when it sits inside a chain? */
function chainAtom(c: CondNode): string {
  if (c.k === 'and' || c.k === 'or') return `(${printCond(c)})`;
  return printCond(c);
}

export function printCond(c: CondNode): string {
  switch (c.k) {
    case 'cmp':
      return `${printExpr(c.l)} ${c.op} ${printExpr(c.r)}`;
    case 'in':
      return `${printExpr(c.e)} in (${c.list.map(printExpr).join(', ')})`;
    case 'text':
      return `${printExpr(c.l)} ${c.op} ${printExpr(c.r)}`;
    case 'isnull':
      return `${printExpr(c.e)} is ${c.negated ? 'not null' : 'null'}`;
    case 'since':
      return `${printExpr(c.e)} since ${printExpr(c.dur)}`;
    case 'under':
      return `${printExpr(c.e)} under ${printExpr(c.id)}`;
    case 'above':
      return `${printExpr(c.e)} above ${printExpr(c.id)}`;
    case 'bare':
      return printExpr(c.e);
    case 'and':
      return c.items.map(chainAtom).join(' and ');
    case 'or':
      return c.items.map(chainAtom).join(' or ');
    case 'not':
      // `not` binds to the following comparison or parenthesized group, so
      // anything with an operator of its own is wrapped.
      return c.inner.k === 'bare' ? `not ${printCond(c.inner)}` : `not (${printCond(c.inner)})`;
  }
}

// ── shapes ───────────────────────────────────────────────────────────────

/**
 * A declared type as source spells it: `duration(ms)` with its unit,
 * `money(2)` with its places, the bare word otherwise.
 */
function printType(ty: ScalarType): string {
  if (ty.unit) return `${ty.name}(${ty.unit})`;
  if (ty.places !== undefined) return `${ty.name}(${ty.places})`;
  return ty.name;
}

function printField(f: FieldNode): string {
  let s: string;
  if (f.kind.k === 'ref') {
    s = `${f.name} -> ${f.kind.target}${f.optional ? '?' : ''}`;
    if (f.kind.owned) s += ' owned';
    if (f.kind.tree) s += ' tree';
  } else {
    s = `${f.name}: ${printType(f.kind.ty)}${f.optional ? '?' : ''}`;
  }
  if (f.unique) s += ' unique';
  if (f.sealed) s += ' sealed';
  if (f.indexed) s += ' indexed';
  if (f.public) s += ' public';
  if (f.refinable) s += ' refinable';
  if (f.inline) s += ' inline';
  if (f.private) s += ' private';
  if (f.normalize) s += ` normalize(${f.normalize})`;
  if (f.default) s += ` = ${printExpr(f.default)}`;
  return s;
}

/**
 * `subject` is the parameter a guard's own comparison is spelled with:
 * `$caller` in an `access` clause, `$member` inside a release rule. The IR
 * is the same either way; the position decides the word, as the parser's does.
 */
function printGuardAtom(a: GuardAtomNode, subject = '$caller'): string {
  switch (a.k) {
    case 'cmp': {
      const rhs = a.caller ? subject : a.lit ? printExpr(a.lit) : subject;
      return `${a.path.join('.')} == ${rhs}`;
    }
    case 'bare':
      return a.path.join('.');
    case 'step':
      return `${a.path.join('.')}(${printGuardExpr(a.inner, subject)})`;
  }
}

function printGuardExpr(g: GuardExprNode, subject = '$caller'): string {
  // Boolean mixing is parenthesized, and a guard walk is the one place where
  // a whole clause can be a mix: the builder distributes into disjunctive
  // normal form (as the parser does), so a two-disjunct walk whose disjuncts
  // have more than one atom each is exactly `(a and b) or (c and d)` — which
  // does not parse without the parentheses.
  //
  // Found by the isomorphism corpus, in `syntax-examples.md`'s property
  // manager: `access write unit.property.propertiesAccess(member == $caller
  // and level == "write" or member == $caller and level == "admin")`. The
  // printer emitted the flattened form and the parser refused its own guard
  // back — the builder saying something `.ordo` cannot, caught in the
  // direction that matters most.
  const wrap = g.disjuncts.length > 1;
  return g.disjuncts
    .map((d) => {
      const conj = d.map((a) => printGuardAtom(a, subject)).join(' and ');
      return wrap && d.length > 1 ? `(${conj})` : conj;
    })
    .join(' or ');
}

function printGuardClause(g: GuardClauseNode): string {
  return g.k === 'open' ? 'open' : printGuardExpr(g.expr);
}

function printEntity(e: EntityNode): string {
  const head = e.key ? `entity ${e.name} (key: ${e.key}) {` : `entity ${e.name} {`;
  const lines: string[] = [head];
  for (const f of e.fields) lines.push(`  ${printField(f)}`);
  // Clause order is this printer's, and it is safe to fix because the AST
  // does not record how the author interleaved clauses with fields: `fields`,
  // `unique_keys`, `indexes` and `checks` are separate vectors, so any order
  // reparses to the same tree.
  for (const u of e.uniqueKeys) lines.push(`  unique (${u.join(', ')})`);
  for (const i of e.indexes) lines.push(`  index (${i.join(', ')})`);
  for (const c of e.checks) lines.push(`  check ${printCond(c)}`);
  if (e.acl) lines.push(`  acl (${e.acl.join(', ')})`);
  if (e.public) lines.push(e.public === 'cascade' ? '  public*' : '  public');
  if (e.accessRead) lines.push(`  access read ${printGuardClause(e.accessRead)}`);
  if (e.accessWrite) lines.push(`  access write ${printGuardClause(e.accessWrite)}`);
  lines.push('}');
  return lines.join('\n');
}

// ── pipelines ────────────────────────────────────────────────────────────

function printProjItem(p: ProjItemNode): string {
  switch (p.k) {
    case 'star':
      return '*';
    case 'exclude':
      return `-${p.name}`;
    case 'field':
      return p.path.join('.');
    case 'named':
      return `${p.out}: ${printExpr(p.value)}`;
  }
}

function printSource(s: SourceNode): string {
  let out = s.name;
  if (s.args.length > 0) {
    const args = s.args.map((a) =>
      a.k === 'named' ? `$${a.name}: ${printExpr(a.value)}` : printCond(a.cond),
    );
    out += `(${args.join(', ')})`;
  }
  if (s.projection !== null) {
    out += `[${s.projection.map(printProjItem).join(', ')}]`;
  }
  return out;
}

function printAgg(a: AggNode): string {
  if (a.cond) return `${a.out}: ${a.func}(${printCond(a.cond)})`;
  if (a.arg && a.arg2) return `${a.out}: ${a.func}(${printExpr(a.arg)}, ${printExpr(a.arg2)})`;
  if (a.arg) return `${a.out}: ${a.func} ${printExpr(a.arg)}`;
  return `${a.out}: ${a.func}`;
}

function printStep(s: StepNode): string {
  switch (s.k) {
    case 'filter':
      return `| ${printCond(s.cond)}`;
    case 'sort':
      return `sort ${s.keys.map((k) => (k.desc ? '-' : '') + printExpr(k.expr)).join(', ')}`;
    case 'take': {
      const keys = s.keys.length > 0 ? `[${s.keys.map(printExpr).join(', ')}]` : '';
      const n = s.n.k === 'lit' ? s.n.n : `$${s.n.name}`;
      return `take${keys} ${n}`;
    }
    case 'group':
      return `group ${s.keys.map(printExpr).join(', ')} { ${s.aggs.map(printAgg).join(', ')} }`;
    case 'reduce':
      return `reduce { ${s.aggs.map(printAgg).join(', ')} }`;
    case 'distinct':
      return 'distinct';
    case 'match':
      switch (s.kind.k) {
        case 'any':
          return `match any ${printSource(s.kind.source)}`;
        case 'none':
          return `match none ${printSource(s.kind.source)}`;
        case 'ref':
          return `match ${s.kind.path.join('.')}`;
        case 'on':
          return `match on ${s.kind.col}`;
      }
    // eslint-disable-next-line no-fallthrough -- every `match` arm returns
    case 'append':
      return `append ${printSource(s.source)}`;
  }
}

function printPipelineBody(p: PipelineNode, indent: string): string[] {
  const lines = [indent + p.sources.map(printSource).join('  ')];
  for (const s of p.steps) lines.push(indent + '  ' + printStep(s));
  return lines;
}

export function printPipeline(p: PipelineNode, indent: string): string[] {
  const lines: string[] = [];
  for (const l of p.lets) {
    lines.push(`${indent}let ${l.name} =`);
    lines.push(...printPipelineBody(l.pipeline, indent + '  '));
  }
  lines.push(...printPipelineBody(p, indent));
  return lines;
}

// ── writes and ops ───────────────────────────────────────────────────────

/**
 * The addressed-delete spelling, when the condition is exactly `id == <expr>`.
 *
 * The parser lowers `del E { id: $id }` into that condition — "one
 * representation is one behaviour, two could drift" — so the brace form is
 * *unrecoverable from the tree*, and printing the parenthesized form is
 * ordinarily correct: the spec calls both spellings addressed with identical
 * semantics.
 *
 * `subtree` is the exception, and it is not cosmetic. Rule 6 refuses
 * `subtree` after a ranged delete, and "ranged" is decided by which SPELLING
 * was written, not by what the condition happens to say — many anchors mean
 * overlapping subtrees with no coherent order over them. So a subtree delete
 * has to be printed back in the braces, or the parser refuses text the
 * builder produced from a program the parser accepted.
 *
 * Found by the isomorphism corpus (`fixture-market`, `syntax-0476`).
 */
function addressed(w: Extract<WriteStmtNode, { k: 'del' }>): string | null {
  const c = w.cond;
  if (c.k !== 'cmp' || c.op !== '==') return null;
  if (c.l.k !== 'path' || c.l.segments.length !== 1 || c.l.segments[0] !== 'id') return null;
  return `del ${w.entity} { id: ${printExpr(c.r)} }`;
}

function printWrite(w: WriteStmtNode): string {
  if (w.k === 'del') {
    if (w.subtree) {
      const brace = addressed(w);
      if (brace === null) {
        throw new Error(
          `del ${w.entity}: \`subtree\` cascades from ONE anchor, so it is legal only ` +
            'on an addressed delete (`del E { id: $id } subtree`). This condition ' +
            'addresses a range, and the parser refuses the pair — building it here ' +
            'would synthesize text that does not parse.',
        );
      }
      return `${brace} subtree`;
    }
    return `del ${w.entity}(${printCond(w.cond)})`;
  }
  const head = w.binding ? `let ${w.binding} = set ${w.entity}` : `set ${w.entity}`;
  const cond = w.cond ? `(${printCond(w.cond)})` : '';
  const fields = w.fields.map(([k, v]) => `${k}: ${printExpr(v)}`).join(', ');
  return `${head}${cond} { ${fields} }`;
}

function printParams(params: readonly ParamNode[]): string {
  if (params.length === 0) return '';
  const list = params
    .map((p) => {
      let s = `$${p.name}: ${printType(p.ty)}`;
      if (p.max) s += ` <= ${printExpr(p.max)}`;
      if (p.default) s += ` = ${printExpr(p.default)}`;
      return s;
    })
    .join(', ');
  return `(${list})`;
}

// Flag order is the parser's loop order, which accepts any order; this one
// is `OpFlags`' declaration order so two programs that differ only in how
// the author typed them print the same.
function printFlags(flags: OpNode['flags']): string {
  let s = '';
  if (flags.unbounded) s += ' unbounded';
  if (flags.unguarded) s += ' unguarded';
  if (flags.reproducible) s += ' reproducible';
  if (flags.nopause) s += ' nopause';
  if (flags.public) s += ' public';
  if (flags.download) s += ' download';
  return s;
}

/**
 * A write inside `each ->`. An addressed delete prints in its braces there:
 * `del e { id: id }` reads the row's `id`, where `del e(id == id)` reads as
 * a condition on `e` that every row meets. The parser takes both to the
 * same tree.
 */
function printEmission(w: WriteStmtNode): string {
  if (w.k === 'del' && !w.subtree) return addressed(w) ?? printWrite(w);
  return printWrite(w);
}

function printRoutine(r: RoutineNode): string {
  const head = r.domain ? `routine ${r.name} in ${r.domain} {` : `routine ${r.name} {`;
  const trigger =
    r.trigger.k === 'clock'
      ? `  trigger ${r.trigger.schedule}`
      : `  trigger when ${r.trigger.pattern}` +
        (r.trigger.modifiers ? ` ${r.trigger.modifiers}` : '');
  const run = `  run${printParams(r.params)}${printFlags(r.flags)}:`;
  if (r.each) {
    return [
      head,
      trigger,
      run,
      ...printPipeline(r.each, '    '),
      '    each ->',
      ...r.statements.map((w) => '      ' + printEmission(w)),
      '}',
    ].join('\n');
  }
  return [head, trigger, run, ...r.statements.map((w) => '    ' + printWrite(w)), '}'].join(
    '\n',
  );
}

function printOp(o: OpNode): string {
  const head = `op ${o.name}${printParams(o.params)}${printFlags(o.flags)} =`;

  if (o.body.k === 'write') {
    return [head, ...o.body.statements.map((w) => '  ' + printWrite(w))].join('\n');
  }
  return [head, ...printPipeline(o.body.pipeline, '  ')].join('\n');
}

function printReleaseCond(c: ReleaseCondNode): string {
  // Parenthesized when mixing, as a guard is: the parser refuses `a and b or c`.
  const wrap = c.disjuncts.length > 1;
  return c.disjuncts
    .map((d) => {
      const conj = d
        .map(
          (a) =>
            `${a.negated ? 'not ' : ''}${a.entity}(${printGuardExpr(a.inner, '$member')})`,
        )
        .join(' and ');
      return wrap && d.length > 1 ? `(${conj})` : conj;
    })
    .join(' or ');
}

function printReassignTarget(t: ReassignTargetNode): string {
  return t.k === 'owner'
    ? 'owner'
    : `${t.entity}(${printGuardExpr(t.inner, '$member')}).${t.field}`;
}

function printRelease(r: ReleaseNode): string {
  let head = `release ${r.subject} `;
  head += r.mode === 'when' && r.guard ? `when ${printReleaseCond(r.guard)}` : r.mode;
  if (r.effects.length === 0) return head;
  const lines = r.effects.map((e) =>
    e.k === 'remove'
      ? `  remove ${e.entity}.${e.field}`
      : `  reassign ${e.entity}.${e.field} to ${e.to.map(printReassignTarget).join(' or ')}`,
  );
  return [`${head} {`, ...lines, '}'].join('\n');
}

/** One declaration, printed. */
export function printDeclaration(d: Declaration): string {
  switch (d.kind) {
    case 'domain':
      return d.version ? `domain ${d.name} @ ${d.version}` : `domain ${d.name}`;
    case 'entity':
      return printEntity(d);
    case 'op':
      return printOp(d);
    case 'routine':
      return printRoutine(d);
    case 'release':
      return printRelease(d);
  }
}
