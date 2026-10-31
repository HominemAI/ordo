#!/usr/bin/env python3
"""Check the two SQL corpus docs against the compiler.

    cargo build -p ordo-cli
    python3 scripts/check-sql-corpus.py            # check both files
    python3 scripts/check-sql-corpus.py --counts   # also list ids per verdict
    python3 scripts/check-sql-corpus.py --show ID  # print the wrapped program
    python3 scripts/check-sql-corpus.py --try FILE # compile snippets (split by ====)

Entries are the lines `**X1. title**`. The verdict is the first mark in the
entry (direct, problematic, not possible). For not-possible entries the text
after the mark says "by design" or "gap".

Checks:
  - an entry marked direct or problematic must compile (every snippet);
  - a probe or snippet of a not-possible entry must be refused (a snippet that
    compiles means the verdict is stale);
  - every not-possible entry is tagged by design or gap.
Exit status is 1 on any failure.

Snippets: fenced blocks that are not ```sql, whole-line inline code, and inline
code after a mark that starts with an entity or `let`/`set`/`del`. Probes are
extra snippets in scripts/check-sql-corpus-probes.txt, for entries whose text
shows only a short fragment.

Wrapper rules (this is the whole list):
  1. The file is the reference schema (read from set 1), then EXTRA_SCHEMA
     (entities the entries use that the reference schema lacks), then the
     snippet.
  2. A snippet that starts with `op`, `routine`, `policy`, `entity` or `domain`
     is used as written (an `entity` replaces any extra entity of that name).
  3. Any other snippet becomes `op <id> unbounded =` with its lines indented.
  4. Every `$name` becomes an op parameter, typed from PARAM_TYPES, else text.
     `$today` and `$now` are `filled` parameters.
  5. A snippet with an ellipsis is a fragment and is skipped.
"""
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLI = os.path.join(ROOT, "target/debug/ordo")
FILES = ["docs/sql-translation-corpus.md", "docs/sql-translation-corpus-2.md"]

PARAM_TYPES = {
    "a": "date", "cutoff": "date", "curStart": "date", "curEnd": "date",
    "prevStart": "date", "prevEnd": "date", "pct": "int", "x": "id",
    "c": "id", "p": "id", "u": "id", "q": "text", "re": "text", "e": "text",
    "n": "text", "k": "text", "t": "money", "co": "id", "new": "id",
    "pass": "text", "since": "datetime", "d": "date", "i": "id",
}

EXTRA_SCHEMA = """
entity suppliers {
  name: text
  email: text?
}
entity lineitems {
  order -> orders
  returnflag: text
  linestatus: text
  qty: int
  price: money(2)
  disc: decimal(4)
  shipped: date
  commitDate: date
  receiptDate: date
}
entity parts {
  name: text
}
entity partSupp {
  part -> parts
  cost: money(2)
}
entity bookings {
  room: text
  start: datetime
  end: datetime
}
entity revisions (key: rvn) {
  doc: text
  created: datetime
}
entity accounts {
  status: text = "open"
  credit: money(2)
  limit: money(2)
  paid: money(2)
  total: money(2)
}
entity sessions {
  customer -> customers
  started: datetime
  ended: datetime?
}
entity events {
  session -> sessions
  customer -> customers
  kind: text
  seq: int
  at: datetime
  actor: text
}
entity notifications {
  user -> customers
  read: bool = false
}
entity posts {
  deleted: datetime?
}
entity logs {
  user -> customers
  at: datetime
  archived: bool = false
}
entity drafts {
  owner -> customers
}
entity prices {
  from: date
  to: date?
}
entity invoices {
  number: int
  total: money(2)
  currency: text
}
entity invoiceLines {
  invoice -> invoices owned
  currency: text
  amount: money(2)
}
entity counters {
  key: text unique
  n: int = 0
}
entity boards {
  name: text
}
entity boardsAccess {
  board -> boards
  member -> customers
  level: text
}
entity staff {
  org: text
}
"""

# Not-possible entries whose own code may compile (none today).
EXPECT_FAIL = set()

# Inline code counts as a whole query only when it starts with one of these.
SOURCES = {"let", "set", "del", "orders", "products", "customers", "employees",
           "categories", "sessions", "events", "notifications", "lineitems",
           "partSupp", "orderItems", "productTags", "shipments", "logs",
           "prices", "invoices", "accounts", "counters", "payments", "reviews"}

ENTRY = re.compile(r"^\*\*([A-Z]\d+[a-z]?)\. ")


def split_entries(text):
    """Yield (id, first_line_no, body_lines) for each entry."""
    lines = text.split("\n")
    cur = None
    in_fence = False
    for i, ln in enumerate(lines):
        if ln.startswith("```"):
            in_fence = not in_fence
        if not in_fence and (ln.startswith("## ") or ln.startswith("---")):
            if cur:
                yield cur
            cur = None
            continue
        m = ENTRY.match(ln) if not in_fence else None
        if m:
            if cur:
                yield cur
            cur = [m.group(1), i + 1, [ln]]
        elif cur:
            cur[2].append(ln)
    if cur:
        yield cur


def snippets(body):
    out = []
    in_fence = False
    lang = ""
    buf = []
    prose = []
    for ln in body:
        if ln.startswith("```"):
            if in_fence:
                if lang != "sql":
                    out.append("\n".join(buf))
                buf = []
                in_fence = False
            else:
                in_fence = True
                lang = ln[3:].strip()
            continue
        if in_fence:
            buf.append(ln)
        else:
            prose.append(ln)
            m = re.fullmatch(r"`([^`]+)`", ln.strip())
            if m:
                out.append(m.group(1))
    ptxt = "\n".join(prose)
    for m in re.finditer(r"(?:✅|⚠️|—|:|,)[^`\n]{0,40}`([a-zA-Z]+[\(\[ ][^`]*)`", ptxt):
        if m.group(1).split("(")[0].split("[")[0].split()[0] in SOURCES:
            out.append(m.group(1))
    return [s for s in out if s.strip() and not s.strip().startswith("--") and "→" not in s]


def wrap(eid, k, code, base):
    s = code.strip("\n")
    first = s.lstrip().split(None, 1)[0] if s.strip() else ""
    schema = base + EXTRA_SCHEMA
    if first == "entity":
        name = s.split()[1]
        extra = re.sub(r"entity %s \{.*?\n\}\n" % name, "", EXTRA_SCHEMA, flags=re.S)
        return base + extra + "\n" + s + "\n"
    if first in ("op", "domain", "routine", "policy"):
        return schema + "\n" + s + "\n"
    params = []
    for p in sorted(set(re.findall(r"\$([A-Za-z_]\w*)", s))):
        if p in ("caller", "agent"):
            continue
        if p in ("today", "now"):
            params.append("$%s: %s filled" % (p, "date" if p == "today" else "datetime"))
            continue
        params.append("$%s: %s" % (p, PARAM_TYPES.get(p, "text")))
    head = "op %s%s unbounded =\n" % (
        eid.lower() + ("x%d" % k if k else ""),
        "(" + ", ".join(params) + ")" if params else "",
    )
    body = "\n".join("  " + l for l in s.split("\n"))
    return schema + "\n" + head + body + "\n"


def run_check(src):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "corpus.ordo")
        with open(f, "w") as fh:
            fh.write(src)
        r = subprocess.run([CLI, "check", f], capture_output=True, text=True)
        msg = (r.stdout + r.stderr).strip().replace(f, "corpus.ordo")
        return r.returncode == 0, msg


def err_line(msg):
    for l in msg.split("\n"):
        if "DOES NOT COMPILE" in l or "error" in l:
            return l.strip()[:260]
    return msg.split("\n")[0][:230]


def base_schema():
    t = open(os.path.join(ROOT, FILES[0])).read()
    a = t.index("Reference schema used throughout")
    b = t.index("```\n", a) + 4
    c = t.index("\n```", b)
    return t[b:c] + "\n"


def verdict(body):
    t = " ".join(body)  # the first mark wins: "✅ ... (❌ for the tuple form)"
    marks = [(t.find(m), v) for m, v in (("❌", "no"), ("⚠️", "warn"), ("✅", "yes")) if m in t]
    return min(marks)[1] if marks else "?"


def load_probes():
    out = {}
    cur = None
    path = os.path.join(ROOT, "scripts/check-sql-corpus-probes.txt")
    for ln in open(path).read().split("\n"):
        if ln.startswith("## "):
            cur = []
            out.setdefault(ln[3:].strip(), []).append(cur)
        elif cur is not None:
            cur.append(ln)  # lines before the first `##` are the file's comment
    return {k: ["\n".join(v).strip("\n") for v in vs] for k, vs in out.items()}


def kind(body):
    """For a not-possible entry: 'design' or 'gap', read from the first words
    after the first cross mark."""
    t = " ".join(l for l in body if not l.startswith(("```", "SELECT", "--")))
    t = t[t.find("\u274c"):][:130].replace("**", "")
    if "by design" in t:
        return "design"
    return "gap" if "gap" in t else "?"


def main():
    if "--try" in sys.argv:  # --try FILE: snippets split by a line of ====
        src = open(sys.argv[sys.argv.index("--try") + 1]).read()
        for part in src.split("\n====\n"):
            ok, msg = run_check(wrap("t", 0, part, base_schema()))
            print("OK" if ok else "FAIL " + err_line(msg))
        return 0
    show = sys.argv[sys.argv.index("--show") + 1] if "--show" in sys.argv else None
    base = base_schema()
    probes = load_probes()
    seen = set()
    bad = 0
    for rel in FILES:
        text = open(os.path.join(ROOT, rel)).read()
        counts = {"yes": [], "warn": [], "no": [], "?": []}
        checked = 0
        unchecked = []
        kinds = {}
        problems = []
        for eid, line, body in split_entries(text):
            v = verdict(body)
            counts[v].append(eid)
            if v == "no":
                kinds.setdefault(kind(body), []).append(eid)
            seen.add(eid)
            doc = [x for x in snippets(body) if "…" not in x and "..." not in x]
            snips = doc + probes.get(eid, [])
            if not snips:
                unchecked.append((eid, v))
                continue
            checked += 1
            results = []
            for k, x in enumerate(snips):
                name = eid if len(snips) == 1 else "%s%s" % (eid, "abcdefghij"[k % 10])
                if show == eid:
                    print(wrap(name, 0, x, base).split("\n\nop ")[-1])
                ok, msg = run_check(wrap(name, 0, x, base))
                results.append((k >= len(doc), ok, msg))
            for is_probe, ok, msg in results:
                if v in ("yes", "warn") and not ok:
                    problems.append("FAIL   %s (line %d) says %s, does not compile%s: %s"
                                    % (eid, line, v, " [probe]" if is_probe else "", err_line(msg)))
                if v == "no" and ok and (is_probe or eid not in EXPECT_FAIL):
                    problems.append("STALE  %s (line %d) says not possible, %s compiles"
                                    % (eid, line, "a probe" if is_probe else "its code"))
        total = sum(map(len, counts.values()))
        print("%s: %d entries, %d checked against the compiler" % (rel, total, checked))
        print("  direct %d, problematic %d, not possible %d, unmarked %d"
              % (len(counts["yes"]), len(counts["warn"]), len(counts["no"]), len(counts["?"])))
        print("  not possible: by design %d, gap %d, unclassified %d"
              % (len(kinds.get("design", [])), len(kinds.get("gap", [])), len(kinds.get("?", []))))
        if "--counts" in sys.argv:
            print("  by design: %s" % " ".join(kinds.get("design", [])))
            print("  gap: %s" % " ".join(kinds.get("gap", [])))
            print("  unclassified: %s" % " ".join(kinds.get("?", [])))
        bad += len(kinds.get("?", []))
        print("  not run against the compiler: %s" % ", ".join(
            "%s(%s)" % u for u in unchecked if u[1] != "no"))
        if "--counts" in sys.argv:
            for k in ("yes", "warn", "no", "?"):
                print("  %s: %s" % (k, " ".join(counts[k])))
        for p in problems:
            print("  " + p)
        bad += len(problems)
    for k in probes:
        if k not in seen:
            print("probe for unknown entry", k)
            bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
