#!/usr/bin/env python3
"""The decisions board: a SQLite store, a local page, and a command line.

    python3 scripts/board.py serve            the page, at http://127.0.0.1:8737 (or a free port), opened in the browser
    python3 scripts/board.py answers          what the owner wrote since the last read
    python3 scripts/board.py ls --needs       tasks waiting on the owner
    python3 scripts/board.py marks            the owner's per-ask marks since the last read (--all, --mark questions)
    python3 scripts/board.py ls --blocked     tasks that are blocked (status=stop:Blocked)
    python3 scripts/board.py summary "two or three sentences" --look id:why id:why --tile "Tests green=1,914"
    python3 scripts/board.py ls --future      tasks for after the first deployment (set <id> --future | --now)
    python3 scripts/board.py docs [text]      the documents and what each is for
    python3 scripts/board.py docs missing     documents in docs/ that the list leaves out
    python3 scripts/board.py docs set <file> "<what it is for>" --kind plan
    python3 scripts/board.py reask <id> 3     put ask 3 to the owner again; the others keep their answers
    python3 scripts/board.py get <id>         one task
    python3 scripts/board.py set <id> sub="Built 2026-09-29" status=ok:Done
    python3 scripts/board.py task "<title>" <<'END'
    explain: what this is, in plain text
    detail: anything longer; the page folds it away
    ask: one decision
    suggest: what Claude would do about it, and why
    ask: the next decision
    suggest: ...
    END

`explain:` is held to 350 characters, an ask to 160 and a recommendation to
260, and `- ` lines become bullets.

The words. A TASK is one entry on the board. It carries any number of ASKS,
each a single question with a single ANSWER. A task also takes a COMMENT, the
owner's free text on the task as a whole, stored as the answer to ask 0.

Three files, one of them disposable:

    docs/board.jsonl           every task, one per line, in board order. In git.
    docs/board-answers.jsonl   every answer ever given, append-only. In git.
    ~/.board/ordo.db           the working store (ORDO_BOARD_DB moves it). Not in
                               git; rebuilt from the two files above whenever
                               they differ from it.

Every write exports both files before it commits, so the database never holds
anything git does not. The page text around the table lives in
docs/decisions.html, which the server sends as it is.
"""
import argparse, hashlib, html, json, math, os, re, sqlite3, sys
from collections import Counter
from contextlib import contextmanager
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DB = Path(os.environ.get("ORDO_BOARD_DB") or Path.home() / ".board/ordo.db")
ITEMS = ROOT / "docs/board.jsonl"
ANSWERS = ROOT / "docs/board-answers.jsonl"
PAGE = ROOT / "docs/decisions.html"
COMPARE = ROOT / "docs/supabase-comparison.html"  # the page's Grief Remediation tab
MARKS = ROOT / "docs/board-marks.json"  # each ask's mark: done, questions, close, or asking
MARK_KINDS = {"done": "done, nothing more", "questions": "has questions, talk it over",
              "close": "Claude may close it", "asking": "Claude has a question for the owner"}
GRIEF = ROOT / "docs/grief-notes.json"  # the owner's note on each of its rows, keyed g<n>
MARKS_READ_NAME = "marks-read.json"
GRIEF_READ = Path(os.environ.get("ORDO_BOARD_DB", Path.home() / ".board/ordo.db")).with_name("grief-read.json")
SUMMARY = ROOT / "docs/board-summary.json"
SUMMARY_MAX = 600
DOCS = ROOT / "docs/board-docs.jsonl"
# Where a task's work stands, apart from whether it was decided.
WORK = {
    "none": "a decision or a question that produced no work",
    "waiting": "undecided: the work waits on the owner's answer",
    "filed": "on the checklist with no go to build it",
    "queued": "decided and not started",
    "building": "started, with a named remainder",
    "blocked": "cannot proceed until something else does",
    "built": "complete; `where` says where",
}
KINDS = ("record", "spec", "plan", "guide", "audit", "review", "explainer")
PORT = 8737

TEXT = ("ref", "name", "sub", "decided", "explain", "ask", "suggest")
STATUSES = ("open", "decided", "deferred", "discuss")

SCHEMA = """
create table if not exists items(
  pos integer not null, id text unique, grp text,
  ref text, name text, sub text, decided text,
  status_cls text, status text, explain text, ask text, suggest text,
  asked_after integer not null default 0, asks text not null default '[]',
  future integer not null default 0, work text not null default '', where_ text not null default '');
create table if not exists answers(
  seq integer primary key, item text not null, at text not null,
  status text not null, text text not null, source text not null,
  read integer not null default 0, ask integer not null default 0);
create table if not exists meta(k text primary key, v text);
"""

# The same two tests the page applies, so `ls --needs` and the Needs me
# button cannot disagree about what is waiting.
NEEDS_LABEL = re.compile(
    r"^(decide|needs you|needs me|confirm|awaiting you|your call|blocked on you|unanswered|yours)$", re.I)
NEEDS_GROUP = re.compile(r"needs (you|me)", re.I)
# A blocked row carries this status class, which the page draws red:
# `status=stop:Blocked`. The page applies the same test for its Blocked button.
BLOCKED = "stop"


def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def digest():
    h = hashlib.sha256()
    for p in (ITEMS, ANSWERS):
        h.update(p.read_bytes() if p.exists() else b"")
        h.update(b"\0")
    return h.hexdigest()


def connect():
    DB.parent.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(DB, timeout=10, isolation_level=None)
    con.row_factory = sqlite3.Row
    con.executescript(SCHEMA)
    older = False
    for table, col, kind in (("items", "asks", "text not null default '[]'"),
                             ("items", "future", "integer not null default 0"),
                             ("items", "work", "text not null default ''"),
                             ("items", "where_", "text not null default ''"),
                             ("answers", "ask", "integer not null default 0")):
        if col not in [c[1] for c in con.execute(f"pragma table_info({table})")]:
            con.execute(f"alter table {table} add column {col} {kind}")
            older = True
    have = con.execute("select v from meta where k='snapshot'").fetchone()
    if ITEMS.exists() and (older or have is None or have[0] != digest()):
        load(con)
    return con


def load(con):
    con.execute("begin immediate")
    con.execute("delete from items")
    con.execute("delete from answers")
    for pos, line in enumerate(ITEMS.read_text().splitlines()):
        r = json.loads(line)
        if "group" in r:
            con.execute("insert into items(pos, grp) values(?, ?)", (pos, r["group"]))
            continue
        st = r.get("status") or ["", ""]
        # A task written before asks were line items carries one `ask`.
        asks = r.get("asks") or ([{"text": r["ask"]}] if (r.get("ask") or "").strip() else [])
        con.execute(
            "insert into items(pos,id,ref,name,sub,decided,status_cls,status,explain,ask,suggest,"
            "asked_after,asks,future,work,where_) values(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (pos, r["id"], *(str(r.get(k) or "") for k in ("ref", "name", "sub", "decided")),
             st[0], st[1], r.get("explain") or "", joined(asks), r.get("suggest") or "",
             r.get("asked_after", 0), json.dumps(asks, ensure_ascii=False), int(bool(r.get("future"))),
             r.get("work") or "", r.get("where") or ""))
    if ANSWERS.exists():
        for line in ANSWERS.read_text().splitlines():
            a = json.loads(line)
            # Before asks were line items, what the owner typed answered the one ask.
            ask = a.get("ask", 1 if a["source"] == "page" else 0)
            con.execute("insert into answers values(?,?,?,?,?,?,?,?)",
                        (a["seq"], a["item"], a["at"], a["status"], a["text"], a["source"],
                         int(a["read"]), ask))
    con.execute("insert or replace into meta values('snapshot', ?)", (digest(),))
    con.execute("commit")


def item_json(r):
    if r["grp"] is not None:
        return {"group": r["grp"]}
    out = {"id": r["id"]}
    out.update((k, r[k]) for k in ("ref", "name", "sub", "decided"))
    out["status"] = [r["status_cls"], r["status"]]
    out.update((k, r[k]) for k in ("explain", "suggest"))
    out["asks"] = json.loads(r["asks"])
    if r["asked_after"]:
        out["asked_after"] = r["asked_after"]
    if r["future"]:
        out["future"] = True
    out["work"], out["where"] = r["work"], r["where_"]
    return out


def answer_json(a):
    return {"seq": a["seq"], "item": a["item"], "ask": a["ask"], "at": a["at"], "status": a["status"],
            "text": a["text"], "source": a["source"], "read": bool(a["read"])}


def joined(asks):
    """A task's asks as one text: what is searched, and what `get <id> ask` prints."""
    return "\n".join(a["text"] for a in asks)


def set_asks(con, item, asks):
    con.execute("update items set asks=?, ask=? where id=?",
                (json.dumps(asks, ensure_ascii=False), joined(asks), item))


def replace(path, lines):
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text("".join(json.dumps(x, ensure_ascii=False) + "\n" for x in lines))
    os.replace(tmp, path)


def export(con):
    replace(ITEMS, [item_json(r) for r in con.execute("select * from items order by pos")])
    replace(ANSWERS, [answer_json(a) for a in con.execute("select * from answers order by seq")])
    con.execute("insert or replace into meta values('snapshot', ?)", (digest(),))


@contextmanager
def write(con):
    con.execute("begin immediate")
    try:
        yield
        export(con)
        con.execute("commit")
    except BaseException:
        con.execute("rollback")
        raise


def renumber(con, ordered):
    for pos, rowid in enumerate(ordered):
        con.execute("update items set pos=? where rowid=?", (pos, rowid))


def place(con, rowid, after=None, group=None):
    """Put a row after a named row, or at the end of a group."""
    order = [r[0] for r in con.execute("select rowid from items where rowid<>? order by pos", (rowid,))]
    if after:
        anchor = con.execute("select rowid from items where id=?", (after,)).fetchone()
        if not anchor:
            sys.exit(f"no task with id {after}")
        at = order.index(anchor[0]) + 1
    else:
        rows = con.execute("select rowid, grp from items where rowid<>? order by pos", (rowid,)).fetchall()
        hits = [i for i, r in enumerate(rows) if r["grp"] is not None and group.lower() in r["grp"].lower()]
        if len(hits) != 1:
            sys.exit(f"{len(hits)} groups match {group!r}")
        at = hits[0] + 1
        while at < len(rows) and rows[at]["grp"] is None:
            at += 1
    order.insert(at, rowid)
    renumber(con, order)


def needing(con):
    """Rows waiting on the owner, by the page's own rule."""
    heard = {}
    for a in con.execute("select item, ask, source, max(seq) seq from answers "
                         "where trim(text)<>'' group by item, ask, source"):
        heard.setdefault(a["item"], []).append(a)
    out, in_needs = [], False
    for r in con.execute("select * from items order by pos"):
        if r["grp"] is not None:
            in_needs = bool(NEEDS_GROUP.search(r["grp"]))
            continue
        # An unanswered ask waits on the owner whatever the status label says.
        asks = json.loads(r["asks"])
        # A future task's asks wait until its time comes (owner, 2026-10-03:
        # "it's for the future roadmap so it can be answered later").
        if r["future"]:
            continue
        # A blue mark is Claude's question, and waits on the owner even on a
        # settled task.
        if asking_on(r["id"], asks) or not settled(r, heard.get(r["id"], [])) and (
                in_needs or NEEDS_LABEL.match(r["status"]) or (asks and not r["decided"].strip())):
            out.append(r)
    return out


def settled(task, heard):
    """A task is settled when it is decided, a ruling was recorded on it, or
    every ask has an answer, all since it was last put to the owner. A comment
    settles a task only when the task has no asks. The page applies the same
    test in `answered`."""
    if task["decided"].strip():
        return True
    new = [a for a in heard if a["seq"] > task["asked_after"]]
    asks = json.loads(task["asks"])
    if not asks:
        return bool(new)
    # An ask put to the owner again on its own counts from its own mark.
    since = {n: max(task["asked_after"], a.get("after", 0)) for n, a in enumerate(asks, 1)}
    if any(a["source"] == "record" and a["ask"] == 0 and a["seq"] > max(since.values()) for a in new):
        return True
    return all(any(a["ask"] == n and a["seq"] > at for a in heard) for n, at in since.items())


def fields(pairs):
    out = {}
    for p in pairs:
        k, _, v = p.partition("=")
        if v.startswith("@"):
            v = Path(v[1:]).read_text().strip()
        elif v == "-":
            v = sys.stdin.read().strip()
        if k == "status":
            cls, _, label = v.partition(":")
            out["status_cls"], out["status"] = cls, label
        elif k in TEXT:
            out[k] = v
        elif k == "work":
            if v not in WORK:
                sys.exit("work is one of: " + "; ".join(f"{w} ({d})" for w, d in WORK.items()))
            out["work"] = v
        elif k == "where":
            out["where_"] = v
        else:
            sys.exit(f"unknown field {k}; fields are status, work, where, {', '.join(TEXT)}")
    return out


# ── the command line ─────────────────────────────────────────────────────────

def cmd_ls(con, a):
    if a.needs:
        rows = needing(con)
    elif a.blocked:
        rows = con.execute("select * from items where status_cls=? order by pos", (BLOCKED,)).fetchall()
    else:
        rows = con.execute("select * from items order by pos").fetchall()
    for r in rows:
        if r["grp"] is not None:
            if not a.grep and not a.future and a.work is None:
                print(f"\n# {r['grp']}")
            continue
        if a.future and not r["future"]:
            continue
        if a.work is not None and r["work"] != a.work:
            continue
        line = f"{r['id']}  [{r['ref']}]  {r['status']}{' / ' + r['work'] if r['work'] else ''}  {r['name']}"
        if not a.grep or a.grep.lower() in (line + " " + r["sub"]).lower():
            print(line)


def cmd_get(con, a):
    r = con.execute("select * from items where id=?", (a.id,)).fetchone()
    if not r:
        sys.exit(f"no task with id {a.id}")
    row = item_json(r)
    if a.fields:
        for f in a.fields:
            if f == "asks":
                for n, x in enumerate(row["asks"], 1):
                    print(f"{n}. {x['text']}" + (f"\n   suggest: {x['suggest']}" if x.get("suggest") else ""))
            else:
                print(r["ask"] if f == "ask" else row[f] if f != "status" else ":".join(row[f]))
        return
    row["answers"] = [answer_json(x) for x in
                      con.execute("select * from answers where item=? order by seq", (a.id,))]
    print(json.dumps(row, indent=1, ensure_ascii=False))


def cmd_set(con, a):
    f = fields(a.pairs)
    with write(con):
        if not con.execute("select 1 from items where id=?", (a.id,)).fetchone():
            sys.exit(f"no task with id {a.id}")
        for k, v in f.items():
            if k == "ask":
                set_asks(con, a.id, [{"text": v}] if v.strip() else [])
            else:
                con.execute(f"update items set {k}=? where id=?", (v, a.id))
        if a.future or a.now:
            con.execute("update items set future=? where id=?", (int(a.future), a.id))
        if a.reask:
            top = con.execute("select coalesce(max(seq),0) from answers").fetchone()[0]
            con.execute("update items set asked_after=?, decided='' where id=?", (top, a.id))
        if a.after or a.group:
            rowid = con.execute("select rowid from items where id=?", (a.id,)).fetchone()[0]
            place(con, rowid, a.after, a.group)
    index(con, a.id)


def cmd_add(con, a):
    f = {"ref": "new", "name": a.id, "sub": "", "decided": "", "status_cls": "warn",
         "status": "Yours", "explain": "", "ask": "", "suggest": ""}
    f.update(fields(a.pairs))
    with write(con):
        cur = con.execute(
            f"insert into items(pos,id,{','.join(f)}) values(-1,?,{','.join('?' * len(f))})",
            (a.id, *f.values()))
        set_asks(con, a.id, [{"text": f["ask"]}] if f["ask"].strip() else [])
        con.execute("update items set future=? where id=?", (int(a.future), a.id))
        place(con, cur.lastrowid, a.after, a.group)
    index(con, a.id)


def cmd_rm(con, a):
    with write(con):
        if not con.execute("delete from items where id=?", (a.id,)).rowcount:
            sys.exit(f"no task with id {a.id}")


def cmd_group(con, a):
    with write(con):
        if a.rename:
            rows = con.execute("select rowid from items where grp like ?", (f"%{a.title}%",)).fetchall()
            if len(rows) != 1:
                sys.exit(f"{len(rows)} groups match {a.title!r}")
            con.execute("update items set grp=? where rowid=?", (a.rename, rows[0][0]))
        else:
            cur = con.execute("insert into items(pos, grp) values(-1, ?)", (a.title,))
            order = [r[0] for r in con.execute("select rowid from items where pos>=0 order by pos")]
            before = a.before and con.execute(
                "select rowid from items where grp like ?", (f"%{a.before}%",)).fetchone()
            order.insert(order.index(before[0]) if before else len(order), cur.lastrowid)
            renumber(con, order)


def cmd_record(con, a):
    """Write a ruling into a row's history, and into `decided` unless told not to."""
    with write(con):
        if not con.execute("select 1 from items where id=?", (a.id,)).fetchone():
            sys.exit(f"no task with id {a.id}")
        con.execute("insert into answers(item,at,status,text,source,read,ask) values(?,?,?,?,'record',1,?)",
                    (a.id, a.when or now()[:10], "decided", a.text, a.ask))
        # A ruling on one ask answers that ask only; `decided` settles the
        # whole task, so it is written for a ruling on the task as a whole.
        if not a.keep_open and not a.ask:
            con.execute("update items set decided=? where id=?", (a.text, a.id))


def cmd_answers(con, a):
    q = "select a.*, i.ref, i.name, i.asks from answers a left join items i on i.id=a.item where "
    q += "a.source='page'" if a.all else "a.source='page' and a.read=0"
    rows = con.execute(q + " order by i.pos, a.ask=0, a.ask, a.seq").fetchall()
    if not rows:
        print("nothing new")
        new_marks = unread_marks(a.peek)
        if new_marks:
            print("\nmarks set since the last read:")
            print_marks(con, new_marks)
        return
    task = None
    for r in rows:
        if r["item"] != task:
            task = r["item"]
            print(f"\n{task}  [{r['ref']}]  {r['name']}")
        asks = json.loads(r["asks"] or "[]")
        if r["ask"] and r["ask"] <= len(asks):
            print(f"  ask {r['ask']}: {plain(asks[r['ask'] - 1]['text'])[:100]}")
        else:
            print(f"  comment ({r['status']})" if r["ask"] == 0 else f"  ask {r['ask']}, since removed")
        for line in r["text"].splitlines() or [""]:
            print(f"    > {line}")
    if not a.peek and not a.all:
        with write(con):
            con.executemany("update answers set read=1 where seq=?", [(r["seq"],) for r in rows])
    new_marks = unread_marks(a.peek)
    if new_marks:
        print("\nmarks set since the last read:")
        print_marks(con, new_marks)


def summary():
    if SUMMARY.exists():
        return json.loads(SUMMARY.read_text())
    return {"date": "", "text": "", "tiles": [], "look": []}


def cmd_summary(con, a):
    """The page's "Where we are": a few sentences, tiles, and rows to look at."""
    s = summary()
    if a.text is None and not (a.look or a.tile or a.drop_tile or a.no_look):
        print(s["date"], s["text"])
        print("tiles:", "  ".join(f"{k}={n}" for n, k, _ in s["tiles"]))
        for l in s["look"]:
            print(f"look: {l['id']}: {l['why']}")
        return
    if a.text is not None:
        text = sys.stdin.read().strip() if a.text == "-" else a.text.strip()
        if len(text) > SUMMARY_MAX:
            sys.exit(f"the summary is {len(text)} characters; the limit is {SUMMARY_MAX}. "
                     "Say less and point at rows with --look.")
        for rid in re.findall(r"\[\[([^\]]+)\]\]", text):
            if not con.execute("select 1 from items where id=?", (rid,)).fetchone():
                sys.exit(f"the summary names [[{rid}]] and no row has that id")
        s["text"] = text
        s["date"] = a.date or now()[:10]
    if a.no_look:
        s["look"] = []
    if a.look:
        s["look"] = []
        for entry in a.look:
            rid, _, why = entry.partition(":")
            if not con.execute("select 1 from items where id=?", (rid,)).fetchone():
                sys.exit(f"no task with id {rid}")
            s["look"].append({"id": rid, "why": why.strip()})
    for t in a.drop_tile or []:
        s["tiles"] = [x for x in s["tiles"] if x[1].lower() != t.lower()]
    for t in a.tile or []:
        label, _, value = t.partition("=")
        value, _, cls = value.partition(":")
        for x in s["tiles"]:
            if x[1].lower() == label.lower():
                x[0] = value
                x[2] = cls or x[2]
                break
        else:
            s["tiles"].append([value, label, cls])
    SUMMARY.write_text(json.dumps(s, indent=1, ensure_ascii=False) + "\n")


def documents():
    if not DOCS.exists():
        return []
    return [json.loads(l) for l in DOCS.read_text().splitlines() if l.strip()]


def on_disk():
    """Every document in docs/, leaving out the board's own files."""
    own = {ITEMS.name, ANSWERS.name, SUMMARY.name, DOCS.name, PAGE.name}
    return sorted(p.name for p in (ROOT / "docs").iterdir()
                  if p.is_file() and p.name not in own and not p.name.startswith(".")
                  and p.suffix in (".md", ".html", ".drawio"))


def cmd_docs(con, a):
    """The documents and what each is for: the page's collapsed list."""
    docs = documents()
    acts = a.args and a.args[0] in ("missing", "set", "rm")
    a.action = a.args[0] if acts else "ls"
    rest = (a.args[1:] if acts else a.args) + [None, None]
    a.file, a.what = rest[0], rest[1]
    if a.action in ("set", "rm"):
        if not a.file:
            sys.exit(f"docs {a.action} needs a file name")
        if a.action == "set":
            if not (ROOT / "docs" / a.file).exists():
                sys.exit(f"docs/{a.file} does not exist")
            old = next((d for d in docs if d["file"] == a.file), None)
            if not a.what and not old:
                sys.exit("a new document needs what it is for")
            if a.kind and a.kind not in KINDS:
                sys.exit(f"kinds are {', '.join(KINDS)}")
            new = {"file": a.file, "kind": a.kind or (old or {}).get("kind") or "record",
                   "what": a.what or old["what"]}
            docs = [d for d in docs if d["file"] != a.file] + [new]
        else:
            docs = [d for d in docs if d["file"] != a.file]
        docs.sort(key=lambda d: (KINDS.index(d["kind"]), d["file"]))
        replace(DOCS, docs)
        return
    have, disk = {d["file"] for d in docs}, on_disk()
    if a.action == "missing":
        for f in disk:
            if f not in have:
                print(f"not listed: {f}")
        for f in sorted(have - set(disk)):
            print(f"listed, gone from docs/: {f}")
        if have == set(disk):
            print(f"all {len(disk)} documents are listed")
        return
    for d in docs:
        if not a.file or a.file.lower() in (d["file"] + " " + d["what"] + " " + d["kind"]).lower():
            print(f"{d['file']}  [{d['kind']}]  {d['what']}")


def cmd_export(con, a):
    with write(con):
        pass
    print(f"{ITEMS.relative_to(ROOT)}, {ANSWERS.relative_to(ROOT)}")


# ── asking a question ────────────────────────────────────────────────────────

SECTION = re.compile(r"^(explain|detail|suggest|ask):[ \t]*", re.M)
TAG = re.compile(r"<[^>]+>")
WORD = re.compile(r"[a-z0-9]+(?:[._-][a-z0-9]+)*")
STOP = frozenset("""a an and are as at be by can do does for from has have how i if in is it its of on
    or should so that the their then there this to was we what when which who will with would you
    your not no one two""".split())
# A row is refused as the same question, unread, when it is alike in meaning
# and shares words as well. Both, because each alone has a false alarm on
# this board: an unrelated question reached 0.55 by meaning with 0.03 by
# words. Measured on six questions, which is few; `similar --calibrate` shows
# how rows already on the board score against each other. `words` alone is
# the line when the model is not installed.
ALIKE = {"meaning": 0.55, "shared": 0.25, "words": 0.45}


def prose(text):
    """Plain text to the HTML the page shows. Text that is already HTML passes through."""
    text = text.strip()
    if not text or text.startswith("<"):
        return text
    def inline(t):
        t = html.escape(" ".join(t.split()), quote=False)
        t = re.sub(r"`([^`]+)`", r"<code>\1</code>", t)
        return re.sub(r"\*\*([^*]+)\*\*", r"<b>\1</b>", t)

    out = []
    for para in re.split(r"\n\s*\n", text):
        # Lines that start with "- " are a list; anything before them is its lead.
        items = re.split(r"(?m)^[-*] +", para)
        if items[0].strip():
            out.append(f"<p>{inline(items[0])}</p>")
        if len(items) > 1:
            out.append("<ul>" + "".join(f"<li>{inline(i)}</li>" for i in items[1:]) + "</ul>")
    return "".join(out)


# The owner reads the board tired. What a task shows first is short, and
# anything longer sits folded under it (owner, 2026-09-29).
MORE = "<!--more-->"
LIMITS = {"explain": 350, "ask": 160, "suggest": 260}


def too_long(kind, text, what):
    n = len(plain(text))
    if n > LIMITS[kind]:
        sys.exit(f"{what} is {n} characters and the limit is {LIMITS[kind]}. Say it in fewer "
                 "words, use `- ` bullets, and put the rest under `detail:`, which the page folds away.")


def sections(body):
    """A task's body. Each `ask:` is one line item, answered on its own. A
    `suggest:` after an ask is the recommendation for that ask; one before
    the first ask is for the task as a whole."""
    parts, out, asks = SECTION.split(body), {}, []
    if parts[0].strip():
        out["explain"] = prose(parts[0])
    for k, v in zip(parts[1::2], parts[2::2]):
        if not v.strip():
            continue
        if k == "ask":
            asks.append({"text": prose(v)})
        elif k == "suggest" and asks:
            asks[-1]["suggest"] = asks[-1].get("suggest", "") + prose(v)
        else:
            out[k] = out.get(k, "") + prose(v)
    detail = out.pop("detail", "")
    if detail and "explain" not in out:
        sys.exit("`detail:` goes with an `explain:`; it is what the page folds under it")
    if "explain" in out:
        too_long("explain", out["explain"], "the explanation")
        out["explain"] += MORE + detail if detail else ""
    if "suggest" in out:
        too_long("suggest", out["suggest"], "the task's recommendation")
    for n, a in enumerate(asks, 1):
        too_long("ask", a["text"], f"ask {n}")
        if a.get("suggest"):
            too_long("suggest", a["suggest"], f"the recommendation for ask {n}")
    if asks:
        out["asks"] = asks
    return out


def words(*texts):
    text = html.unescape(TAG.sub(" ", " ".join(texts))).lower()
    return Counter(w for w in WORD.findall(text) if w not in STOP and len(w) > 1)


def plain(*texts):
    return " ".join(html.unescape(TAG.sub(" ", " ".join(texts))).split())


MODEL = "mxbai-embed-large-v1"      # 1024 dimensions, quantized, run by onnxruntime
QUERY = "Represent this sentence for searching relevant passages: "


class Embedder:
    """The local embedding model, with every vector it has made kept in the database.

    Set up once: `pip install --target ~/.board/lib tokenizers huggingface_hub`,
    onnxruntime and numpy in the Python that runs this, and the model's
    `onnx/model_quantized.onnx` and `tokenizer.json` under ~/.board/model-1024.
    It runs on this machine. A row's three texts are embedded once and again
    only when they change, so a search embeds the query and nothing else.
    """

    def __init__(self, con):
        import numpy, onnxruntime
        home = DB.parent
        sys.path.insert(0, str(home / "lib"))
        try:
            from tokenizers import Tokenizer
        finally:
            sys.path.remove(str(home / "lib"))
        self.np, self.con = numpy, con
        self.tok = Tokenizer.from_file(str(home / "model-1024/tokenizer.json"))
        self.tok.enable_truncation(max_length=512)
        self.tok.enable_padding()
        opts = onnxruntime.SessionOptions()
        opts.log_severity_level = 3
        self.run = onnxruntime.InferenceSession(
            str(home / "model-1024/onnx/model_quantized.onnx"), opts, providers=["CPUExecutionProvider"])
        self.wants = {i.name for i in self.run.get_inputs()}
        con.execute("create table if not exists vectors(hash text primary key, vec blob not null)")

    def fresh(self, texts):
        np, out = self.np, []
        for i in range(0, len(texts), 8):
            enc = self.tok.encode_batch(texts[i:i + 8])
            feed = {"input_ids": np.array([e.ids for e in enc], dtype=np.int64),
                    "attention_mask": np.array([e.attention_mask for e in enc], dtype=np.int64),
                    "token_type_ids": np.array([e.type_ids for e in enc], dtype=np.int64)}
            v = self.run.run(None, {k: x for k, x in feed.items() if k in self.wants})[0][:, 0]
            out.extend(v / (np.linalg.norm(v, axis=1, keepdims=True) + 1e-9))
        return out

    def encode(self, texts):
        np = self.np
        keys = [hashlib.sha256((MODEL + "\0" + t).encode()).hexdigest() for t in texts]
        have = {}
        for i in range(0, len(keys), 500):
            chunk = keys[i:i + 500]
            have.update(self.con.execute(
                f"select hash, vec from vectors where hash in ({','.join('?' * len(chunk))})", chunk))
        need = sorted({k: t for k, t in zip(keys, texts) if k not in have}.items())
        if len(need) > 20:
            print(f"embedding {len(need)} texts, once", file=sys.stderr)
        for (k, _), v in zip(need, self.fresh([t for _, t in need])):
            have[k] = v.astype(np.float32).tobytes()
            self.con.execute("insert or replace into vectors values(?, ?)", (k, have[k]))
        return np.stack([np.frombuffer(have[k], dtype=np.float32) for k in keys])


def embedder(con):
    if not (DB.parent / "model-1024/onnx/model_quantized.onnx").exists():
        return None
    try:
        return Embedder(con)
    except Exception as e:
        print(f"embedding model unavailable ({e}); comparing shared words only", file=sys.stderr)
        return None


PARTS = ("question", "explanation", "suggestion")


def row_parts(r):
    told = r["suggest"] + "".join(a.get("suggest") or "" for a in json.loads(r["asks"]))
    return parts(r["name"], r["ask"], r["explain"], told)


def parts(name, ask, explain, suggest):
    """A row or a new question as its three texts, each embedded on its own."""
    return [plain(name, ask)[:2000], plain(explain)[:2000], plain(suggest)[:2000]]


def by_meaning(con, rows, query, searching):
    """Each row's best match to the query, and the part of the row it was in.

    Every row on the board is about the same project, so every row resembles
    every other: uncentred, two unrelated rows score 0.78. What they share is
    their average, and it is subtracted from every vector before comparing,
    which leaves what is particular to each. Unrelated rows then score 0.47
    and a reworded duplicate 0.60, with its nearest rival at 0.28.

    A search compares the query with every part of every row. A new question
    is compared like with like, its question with theirs, its explanation
    with theirs, its suggestion with theirs.
    """
    model = embedder(con)
    if model is None:
        return None
    np = model.np
    texts = [t for r in rows for t in row_parts(r)]
    filled = [i for i, t in enumerate(texts) if t]
    asked = [k for k, t in enumerate(query) if t]
    v = model.encode([texts[i] for i in filled] + [(QUERY if searching else "") + query[k] for k in asked])
    v = v - v[:len(filled)].mean(axis=0)
    v = v / (np.linalg.norm(v, axis=1, keepdims=True) + 1e-9)
    sims = v[:len(filled)] @ v[len(filled):].T
    best = [(0.0, PARTS[0])] * len(rows)
    for n, i in enumerate(filled):
        cols = range(len(asked)) if searching else [c for c, k in enumerate(asked) if k == i % 3]
        sim = max((float(sims[n][c]) for c in cols), default=0.0)
        if sim > best[i // 3][0]:
            best[i // 3] = (sim, PARTS[i % 3])
    return best


def index(con, item):
    """Embed a task's three parts now, so the next search has nothing to catch up on."""
    r = con.execute("select * from items where id=?", (item,)).fetchone()
    model = r and embedder(con)
    if model:
        model.encode([t for t in row_parts(r) if t])


def by_words(rows, name, body):
    bags = [words(*[r["name"]] * 3, r["sub"], r["ask"], r["explain"], r["suggest"]) for r in rows]
    n = len(bags) + 1
    df = Counter(w for d in bags for w in d)

    def vec(c):
        v = {w: (1 + math.log(f)) * (math.log(n / (1 + df[w])) + 1) for w, f in c.items()}
        norm = math.sqrt(sum(x * x for x in v.values())) or 1
        return {w: x / norm for w, x in v.items()}

    q = vec(words(*[name] * 3, body))
    return [sum(x * v[w] for w, x in q.items() if w in v) for v in map(vec, bags)]


def nearest(con, query, skip=None, searching=False):
    """Rows most like a question, as (meaning, part, words, row), best first.

    `query` is the three texts from `parts`. Two measures, because each misses
    what the other finds: meaning catches a question asked in different words,
    shared words catch a name like `Google` that the small model blurs. Rows
    are ordered by their rank under both. `meaning` is None when the model is
    not installed.
    """
    rows = [r for r in con.execute("select * from items where id is not null") if r["id"] != skip]
    if not rows:
        return []
    w = by_words(rows, query[0], " ".join(query[1:]))
    m = by_meaning(con, rows, query, searching)
    rank = lambda xs: {i: n for n, i in enumerate(sorted(range(len(xs)), key=lambda i: -xs[i]))}
    rw, rm = rank(w), rank([x[0] for x in m]) if m else {}
    order = sorted(range(len(rows)), key=lambda i: -(1 / (20 + rw[i]) + (1 / (20 + rm[i]) if m else 0)))
    return [(*(m[i] if m else (None, "")), w[i], rows[i]) for i in order]


def same(m, w):
    if m is None:
        return w >= ALIKE["words"]
    return m >= ALIKE["meaning"] and w >= ALIKE["shared"]


def show(m, part, w, r):
    decided = ", decided" if r["decided"].strip() else ""
    score = f"words {w:.2f}" if m is None else f"{m:.2f} on its {part}, words {w:.2f}"
    return f"  {score}  {r['id']}  [{r['ref']}, {r['status']}{decided}]  {r['name'][:70]}"


def cmd_similar(con, a):
    if a.calibrate:
        rows = con.execute("select * from items where id is not null").fetchall()
        near = [nearest(con, row_parts(r), skip=r["id"])
                for r in rows]
        for label, k in (("meaning", 0), ("words", 2)):
            best = sorted((max(x[k] or 0 for x in n[:3]) for n in near), reverse=True)
            at = lambda q: best[int(len(best) * q)]
            print(f"{label}: nearest row across {len(best)}: top {best[0]:.2f}, "
                  f"5% {at(.05):.2f}, 25% {at(.25):.2f}, median {at(.5):.2f}")
        return
    # The keywords are searched as a text of their own, so a row that matches
    # them is found even when it does not match the question.
    found = nearest(con, [plain(a.text), plain(" ".join(a.keywords)), ""], searching=True)[:a.top]
    for x in found:
        print(show(*x))
    print("if one of these is the task: `task --update <id>`. If none is: `task \"<title>\"`.")


def reask(con, a, f):
    """Put an existing row to the owner again, with whatever was rewritten."""
    with write(con):
        r = con.execute("select * from items where id=?", (a.update,)).fetchone()
        if not r:
            sys.exit(f"no task with id {a.update}")
        if a.name:
            f["name"] = a.name
        if a.sub:
            f["sub"] = a.sub
        if a.ref != "new":
            f["ref"] = a.ref
        top = con.execute("select coalesce(max(seq),0) from answers").fetchone()[0]
        f.update(status_cls="warn", status="Yours", decided="", asked_after=top)
        if a.future:
            con.execute("update items set future=1 where id=?", (a.update,))
        asks = f.pop("asks", None)
        if asks is not None:
            set_asks(con, a.update, asks)
        for k, v in f.items():
            con.execute(f"update items set {k}=? where id=?", (v, a.update))
    index(con, a.update)
    was = f"was {r['status']}" + (", decided" if r["decided"].strip() else "")
    print(f"updated {a.update} and put it to the owner again ({was}); "
          f"rewrote: {', '.join([k for k in f if k in TEXT and k != 'decided'] + ([f'{len(asks)} asks'] if asks else [])) or 'nothing'}")


def cmd_reask(con, a):
    """Put some of a task's asks to the owner again and leave the rest closed.
    The body gives one `ask:` (with its `suggest:`) per number, in order. A
    number past the last ask adds one."""
    f = sections(sys.stdin.read() if not sys.stdin.isatty() else "")
    new = f.pop("asks", [])
    if len(new) != len(a.n):
        sys.exit(f"{len(a.n)} asks were named and the body holds {len(new)}")
    with write(con):
        r = con.execute("select * from items where id=?", (a.id,)).fetchone()
        if not r:
            sys.exit(f"no task with id {a.id}")
        asks = json.loads(r["asks"])
        top = con.execute("select coalesce(max(seq),0) from answers").fetchone()[0]
        for n, ask in zip(a.n, new):
            if not ask.get("suggest"):
                sys.exit(f"ask {n} needs a `suggest:` after it")
            if not 1 <= n <= len(asks) + 1:
                sys.exit(f"the task has {len(asks)} asks; {n} would leave a gap")
            ask["after"] = top
            if n == len(asks) + 1:
                asks.append(ask)
            else:
                asks[n - 1] = ask
        set_asks(con, a.id, asks)
        for k, v in f.items():
            con.execute(f"update items set {k}=? where id=?", (v, a.id))
        con.execute("update items set status_cls='warn', status='Yours', decided='', work='waiting' where id=?",
                    (a.id,))
        if a.sub:
            con.execute("update items set sub=? where id=?", (a.sub, a.id))
    index(con, a.id)
    print(f"put to the owner again on {a.id}: ask {', '.join(map(str, a.n))}; the others keep their answers")


def cmd_task(con, a):
    f = sections(sys.stdin.read() if not sys.stdin.isatty() else "")
    if a.update:
        return reask(con, a, f)
    if not a.name:
        sys.exit("a new task needs a title")
    asks = f.pop("asks", [])
    if not asks:
        sys.exit("a task for the owner needs at least one `ask:` on stdin")
    bare = [n for n, x in enumerate(asks, 1) if not x.get("suggest")]
    if bare and not f.get("suggest"):
        sys.exit(f"ask {bare[0]} has no `suggest:` after it, and the task has none of its own")
    item = a.id or "-".join(WORD.findall(a.name.lower())[:6])[:48]
    told = f.get("suggest", "") + "".join(x.get("suggest", "") for x in asks)
    ranked = nearest(con, parts(a.name, "\n".join(x["text"] for x in asks), f.get("explain", ""), told))
    alike = [x for x in ranked[:3] if same(x[0], x[2])]
    if alike and not a.force:
        print("not added: the board may already carry this.")
        for x in alike:
            print(show(*x))
        print("update that task with `task --update <id>`, or add this one anyway with --force")
        sys.exit(2)
    with write(con):
        if con.execute("select 1 from items where id=?", (item,)).fetchone():
            sys.exit(f"a task with id {item} exists; pass --id")
        last = con.execute("select id from items where status like 'yours' order by pos desc").fetchone()
        row = {"ref": a.ref, "name": a.name, "sub": a.sub or f"Raised {now()[:10]}", "decided": "",
               "status_cls": "warn", "status": "Yours", "explain": "", "suggest": "", **f}
        cur = con.execute(
            f"insert into items(pos,id,{','.join(row)}) values(-1,?,{','.join('?' * len(row))})",
            (item, *row.values()))
        set_asks(con, item, asks)
        con.execute("update items set future=? where id=?", (int(a.future), item))
        if a.after or last:
            place(con, cur.lastrowid, after=a.after or last["id"])
        else:
            first = con.execute("select rowid from items where grp is not null order by pos").fetchone()
            order = [r[0] for r in con.execute("select rowid from items where pos>=0 order by pos")]
            order.insert(order.index(first[0]) + 1 if first else 0, cur.lastrowid)
            renumber(con, order)
    index(con, item)
    print(f"added {item} with {len(asks)} asks. Nearest tasks, in case one is the same:")
    for x in ranked[:3]:
        print(show(*x))


# ── the page ─────────────────────────────────────────────────────────────────

def board(con):
    by = {}
    for x in con.execute("select * from answers order by seq"):
        by.setdefault(x["item"], []).append(answer_json(x))
    items = []
    for r in con.execute("select * from items order by pos"):
        row = item_json(r)
        if "id" in row:
            row["answers"] = by.get(row["id"], [])
        items.append(row)
    return {"items": items, "summary": summary(), "docs": documents()}


def answer(con, body):
    """One draft per ask, and one for the task's comment (ask 0), per reading:
    typing edits it until Claude reads it."""
    item, text = str(body["id"]), str(body.get("text") or "").strip()
    ask = int(body.get("ask") or 0)
    status = body.get("status") if body.get("status") in STATUSES and not ask else "open"
    with write(con):
        task = con.execute("select asks, decided, asked_after from items where id=?", (item,)).fetchone()
        if not task or not 0 <= ask <= len(json.loads(task["asks"])):
            return None
        # An ask that was answered and read, or settled by a ruling, is closed.
        # Only the comment stays open, until the task is put to the owner again.
        since = ask and max(task["asked_after"], json.loads(task["asks"])[ask - 1].get("after", 0))
        if ask and (task["decided"].strip() or con.execute(
                "select 1 from answers where item=? and ask=? and read=1 and trim(text)<>'' and seq>?",
                (item, ask, since)).fetchone()):
            return None
        draft = con.execute(
            "select seq from answers where item=? and ask=? and source='page' and read=0",
            (item, ask)).fetchone()
        if not text and status == "open":
            if draft:
                con.execute("delete from answers where seq=?", (draft[0],))
            return {}
        if draft:
            con.execute("update answers set at=?, status=?, text=? where seq=?",
                        (now(), status, text, draft[0]))
            seq = draft[0]
        else:
            seq = con.execute(
                "insert into answers(item,at,status,text,source,ask) values(?,?,?,?,'page',?)",
                (item, now(), status, text, ask)).lastrowid
        return answer_json(con.execute("select * from answers where seq=?", (seq,)).fetchone())


class Door(BaseHTTPRequestHandler):
    hosts = ()

    def log_message(self, *a):
        pass

    def send(self, code, body, kind="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", kind + "; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def local(self):
        """Refuse a request another site's page talked the browser into."""
        if self.headers.get("Host") not in self.hosts:
            return False
        origin = self.headers.get("Origin")
        return origin is None or origin in tuple("http://" + h for h in self.hosts)

    def do_GET(self):
        if not self.local():
            return self.send(403, {"error": "not local"})
        if self.path in ("/", "/index.html", "/decisions.html"):
            return self.send(200, PAGE.read_bytes(), "text/html")
        if self.path == "/compare":
            return self.send(200, COMPARE.read_bytes(), "text/html")
        if self.path == "/api/grief":
            return self.send(200, grief_notes())
        if self.path == "/api/marks":
            return self.send(200, marks())
        if self.path == "/api/board":
            con = connect()
            try:
                return self.send(200, board(con))
            finally:
                con.close()
        self.send(404, {"error": "no such path"})

    def do_POST(self):
        if self.local() and self.path == "/api/marks":
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)))
                at = mark_save(str(body["id"]), int(body["ask"]), body.get("mark"))
            except (ValueError, KeyError, TypeError):
                return self.send(400, {"error": "unreadable mark"})
            return self.send(200, {"saved": at})
        if self.local() and self.path == "/api/grief":
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)))
                at = grief_save(body["id"], body["text"])
            except (ValueError, KeyError, TypeError):
                return self.send(400, {"error": "unreadable note"})
            return self.send(200, {"saved": at})
        if not self.local() or self.path != "/api/answer":
            return self.send(403, {"error": "refused"})
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)))
            con = connect()
            try:
                saved = answer(con, body)
            finally:
                con.close()
        except (ValueError, KeyError, TypeError):
            return self.send(400, {"error": "unreadable answer"})
        if saved is None:
            return self.send(404, {"error": "no such row"})
        self.send(200, {"saved": saved})


# ── marks: the owner's per-ask state ───────────────────────────────────
# Green done, yellow questions, red "Claude may close it". Kept apart from
# answers because a mark changes at any time, on a closed ask as well. Keyed
# "<task>:<ask>", ask 0 being the task as a whole. What Claude has read is
# kept beside the database.

def marks():
    try:
        return json.loads(MARKS.read_text())
    except (FileNotFoundError, ValueError):
        return {}


def mark_save(item, ask, mark):
    if mark not in MARK_KINDS and mark not in (None, ""):
        raise ValueError(mark)
    con = connect()
    try:
        row = con.execute("select asks from items where id=?", (item,)).fetchone()
    finally:
        con.close()
    # Clearing is allowed past the last ask, so a mark left behind when a
    # rewrite dropped asks can still be taken off.
    if not row or not 0 <= ask <= len(json.loads(row["asks"])) and mark:
        raise ValueError(item)
    m, key = marks(), f"{item}:{ask}"
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    if mark:
        m[key] = {"mark": mark, "at": at}
    else:
        m.pop(key, None)
    tmp = MARKS.with_suffix(".tmp")
    tmp.write_text(json.dumps(dict(sorted(m.items())), ensure_ascii=False, indent=1) + "\n")
    tmp.replace(MARKS)
    return at


def asking_on(item, asks=None):
    """Whether any ask of this task carries Claude's blue question mark.

    A mark on an ask the task no longer has (a rewrite dropped it) does not
    count: it would hold the task in Needs you with nothing to answer."""
    for k, v in marks().items():
        task, _, n = k.partition(":")
        if task != item or v.get("mark") != "asking":
            continue
        if asks is not None and n.isdigit() and int(n) > len(asks):
            continue
        return True
    return False


def cmd_mark(con, a):
    """Set one ask's mark from the command line. Claude sets `asking` when it has
    a question for the owner, and clears it once he has answered."""
    try:
        mark_save(a.id, a.ask, "" if a.kind == "clear" else a.kind)
    except ValueError:
        sys.exit(f"no ask {a.ask} on {a.id}")
    if a.kind == "asking":
        # Claude's own mark is not news to Claude.
        seen = {}
        try:
            seen = json.loads(marks_read_path().read_text())
        except (FileNotFoundError, ValueError):
            pass
        key = f"{a.id}:{a.ask}"
        seen[key] = marks()[key]["at"]
        marks_read_path().parent.mkdir(parents=True, exist_ok=True)
        marks_read_path().write_text(json.dumps(seen))


def marks_read_path():
    return GRIEF_READ.with_name(MARKS_READ_NAME)


def unread_marks(peek):
    """Marks set or changed since the last read; marks them read unless peeking."""
    m = marks()
    try:
        seen = json.loads(marks_read_path().read_text())
    except (FileNotFoundError, ValueError):
        seen = {}
    new = {k: v for k, v in m.items() if seen.get(k) != v["at"]}
    if new and not peek:
        seen.update({k: v["at"] for k, v in new.items()})
        marks_read_path().parent.mkdir(parents=True, exist_ok=True)
        marks_read_path().write_text(json.dumps(seen))
    return new


def print_marks(con, ms):
    for key, v in sorted(ms.items()):
        item, _, ask = key.rpartition(":")
        what = "the task" if ask == "0" else f"ask {ask}"
        q = ""
        if ask != "0":
            r = con.execute("select asks from items where id=?", (item,)).fetchone()
            asks = json.loads(r["asks"]) if r else []
            if int(ask) <= len(asks):
                q = re.sub(r"<[^>]+>", "", asks[int(ask) - 1]["text"])[:80]
        print(f"  {item} {what}: {v['mark']} ({MARK_KINDS[v['mark']]})" + (f"  {q}" if q else ""))


def cmd_marks(con, a):
    ms = marks() if a.all else unread_marks(a.peek)
    if a.mark:
        ms = {k: v for k, v in ms.items() if v["mark"] == a.mark}
    if not ms:
        print("no marks" if a.all else "no new marks")
        return
    print_marks(con, ms)


# ── grief remediation notes ─────────────────────────────────────────────
# One note per row of docs/supabase-comparison.html, keyed by the row's
# permanent id (g<n>). The file is in git; what Claude has read is kept
# beside the database, so reading marks nothing in the repo.

def grief_notes():
    try:
        return json.loads(GRIEF.read_text())
    except (FileNotFoundError, ValueError):
        return {}


def grief_save(gid, text):
    if not re.fullmatch(r"g\d{1,4}", str(gid)) or not isinstance(text, str) or len(text) > 20000:
        raise ValueError(gid)
    notes = grief_notes()
    at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    if text.strip():
        notes[gid] = {"text": text, "at": at}
    else:
        notes.pop(gid, None)
    tmp = GRIEF.with_suffix(".tmp")
    tmp.write_text(json.dumps(dict(sorted(notes.items(), key=lambda kv: int(kv[0][1:]))),
                              ensure_ascii=False, indent=1) + "\n")
    tmp.replace(GRIEF)
    return at


def grief_rows():
    """Each row of the tab as {id: (group, grievance, ordo, verdict, todo)}, tags stripped."""
    src = COMPARE.read_text()
    out, group = {}, ""
    for m in re.finditer(r'<tr class="cmpgroup"><td[^>]*>(.*?)</td></tr>|<tr data-v="(\w+)" data-id="(g\d+)">(.*?)</tr>', src, re.S):
        if m.group(1):
            group = m.group(1)
            continue
        cells = [html.unescape(re.sub(r"<[^>]+>", "", c)).strip()
                 for c in re.findall(r"<td>(.*?)</td>", m.group(4), re.S)]
        out[m.group(3)] = (group, cells[1], cells[3], cells[4], cells[5], cells[2])
    return out


def cmd_grief(con, a):
    con.close()
    notes, rows = grief_notes(), grief_rows()
    if a.n:
        gid = a.n if a.n.startswith("g") else "g" + a.n
        if gid not in rows:
            sys.exit(f"no row {gid}")
        g, griev, ordo, verdict, todo, who = rows[gid]
        print(f"{gid}  [{g}]  {verdict}  ({who})\n  grievance: {griev}\n  ordo: {ordo}\n  to do: {todo}")
        n = notes.get(gid)
        print(f"  note ({n['at']}): {n['text']}" if n else "  no note")
        return
    try:
        seen = json.loads(GRIEF_READ.read_text())
    except (FileNotFoundError, ValueError):
        seen = {}
    shown = {k: v for k, v in notes.items() if a.all or seen.get(k) != v["at"]}
    if not shown:
        print("no new notes" if not a.all else "no notes")
        return
    for gid, n in shown.items():
        g, griev, _, verdict, _, _ = rows.get(gid, ("?", "(row no longer on the tab)", "", "?", "", ""))
        print(f"{gid}  {verdict}  {griev[:90]}\n  note ({n['at']}): {n['text']}\n")
    if not a.peek and not a.all:
        seen.update({k: v["at"] for k, v in shown.items()})
        GRIEF_READ.parent.mkdir(parents=True, exist_ok=True)
        GRIEF_READ.write_text(json.dumps(seen))


def cmd_serve(con, a):
    con.close()
    try:
        server = ThreadingHTTPServer(("127.0.0.1", a.port), Door)
    except OSError:
        # The port is taken, most often by a board left running: take any free one.
        server = ThreadingHTTPServer(("127.0.0.1", 0), Door)
        print(f"port {a.port} is in use; using {server.server_port}")
    port = server.server_port
    Door.hosts = (f"127.0.0.1:{port}", f"localhost:{port}")
    url = f"http://127.0.0.1:{port}"
    print(f"the board is at {url}  (ctrl-c stops it)")
    if not a.no_open:
        import webbrowser
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


def main():
    p = argparse.ArgumentParser(description="The decisions board.")
    s = p.add_subparsers(dest="cmd", required=True)

    def where(c):
        c.add_argument("--after", help="place after this row id")
        c.add_argument("--group", help="place at the end of the group whose title contains this")
        c.add_argument("--future", action="store_true", help="for after the first deployment")

    c = s.add_parser("serve"); c.add_argument("--port", type=int, default=PORT)
    c.add_argument("--no-open", action="store_true", help="do not open the page in a browser")
    c = s.add_parser("ls"); c.add_argument("--needs", action="store_true")
    c.add_argument("--future", action="store_true", help="only tasks for after the first deployment")
    c.add_argument("--blocked", action="store_true"); c.add_argument("--grep")
    c.add_argument("--work", metavar="STATE", help="only tasks whose work is in this state; '' for unaudited")
    c = s.add_parser("get"); c.add_argument("id"); c.add_argument("fields", nargs="*")
    c = s.add_parser("set"); c.add_argument("id"); c.add_argument("pairs", nargs="*"); where(c)
    c.add_argument("--now", action="store_true", help="no longer for after the first deployment")
    c.add_argument("--reask", action="store_true",
                   help="the row needs the owner again, whatever was answered before")
    c = s.add_parser("add"); c.add_argument("id"); c.add_argument("pairs", nargs="*"); where(c)
    c = s.add_parser("rm"); c.add_argument("id")
    c = s.add_parser("group"); c.add_argument("title"); c.add_argument("--before"); c.add_argument("--rename")
    c = s.add_parser("record"); c.add_argument("id"); c.add_argument("text")
    c.add_argument("--when"); c.add_argument("--keep-open", action="store_true")
    c.add_argument("--ask", type=int, default=0, metavar="N", help="the ruling answers this ask")
    c = s.add_parser("answers"); c.add_argument("--peek", action="store_true")
    c.add_argument("--all", action="store_true")
    s.add_parser("export")
    c = s.add_parser("docs", help="the documents and what each is for")
    c.add_argument("args", nargs="*", help="text to search for | missing | set <file> <what> | rm <file>")
    c.add_argument("--kind")
    c = s.add_parser("summary", help="the page's Where we are; no arguments prints it")
    c.add_argument("text", nargs="?", help="a few sentences, or - for stdin")
    c.add_argument("--look", nargs="+", metavar="ID[:WHY]", help="rows to look at; replaces the list")
    c.add_argument("--no-look", action="store_true")
    c.add_argument("--tile", action="append", metavar="LABEL=VALUE[:CLS]")
    c.add_argument("--drop-tile", action="append", metavar="LABEL"); c.add_argument("--date")
    c = s.add_parser("reask", help="put some asks of a task to the owner again; body on stdin")
    c.add_argument("id"); c.add_argument("n", type=int, nargs="+"); c.add_argument("--sub")
    c = s.add_parser("task", help="add a task with its asks; the body is read from stdin")
    c.add_argument("name", nargs="?"); c.add_argument("--id"); c.add_argument("--ref", default="new")
    c.add_argument("--update", metavar="ID",
                   help="rewrite this task from stdin and put it to the owner again")
    c.add_argument("--sub"); c.add_argument("--after")
    c.add_argument("--force", action="store_true", help="add it even if a similar task exists")
    c.add_argument("--future", action="store_true", help="for after the first deployment")
    c = s.add_parser("mark", help="set one ask's mark; Claude uses asking (blue ?) for a question")
    c.add_argument("id"); c.add_argument("ask", type=int)
    c.add_argument("kind", choices=sorted(MARK_KINDS) + ["clear"])
    c = s.add_parser("marks", help="the per-ask marks: done, questions, close, asking")
    c.add_argument("--all", action="store_true", help="every mark, read or not")
    c.add_argument("--peek", action="store_true", help="new marks, without marking them read")
    c.add_argument("--mark", choices=sorted(MARK_KINDS), help="only marks of this kind")
    c = s.add_parser("grief", help="the owner's notes on the Grief Remediation tab")
    c.add_argument("n", nargs="?", help="one row, by number: its grievance and its note")
    c.add_argument("--all", action="store_true", help="every note, read or not")
    c.add_argument("--peek", action="store_true", help="new notes, without marking them read")
    c = s.add_parser("similar", help="rows most like a piece of text")
    c.add_argument("text", nargs="?", default=""); c.add_argument("keywords", nargs="*")
    c.add_argument("--top", type=int, default=5)
    c.add_argument("--calibrate", action="store_true")
    a = p.parse_args()
    if a.cmd == "add" and not (a.after or a.group):
        sys.exit("add needs --after <id> or --group <title>")
    globals()["cmd_" + a.cmd](connect(), a)


if __name__ == "__main__":
    main()
