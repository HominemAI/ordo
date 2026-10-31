---
name: decisions
description: The official roadmap, a local web app over SQLite (scripts/board.py) where Adam answers decisions. Use whenever project state changes and the board must be updated in the same push, when he says he has answered, asks what is outstanding, or a decision closes.
---

# The decisions file

**The board is the official roadmap** (ruled by Adam, 2026-09-18).
It is his gateway of visibility into Ordo, not only the place he answers
questions, so it must always be up to date. `docs/roadmap-checklist.md` is the
detailed engineering record underneath it; a board that lags the checklist is a
defect.

**The board and the checklist must agree** (Adam, same day). Every item's state
(open, in progress, built, ruled, blocked) reads the same in both, so a checked
box in one and an open task in the other is a defect whichever side is right.

The rule: **every push that changes project state updates the board and the
checklist in the same push.** Before every such push run `node scripts/board-agrees.mjs`: it
exits 1 if an open checklist item is not named on the board, or a task in a Done
group points at an open item. A task that finishes part of an open item names
the part in its ref (`18d · S4`). That means an increment built or started, a ruling made, a test count
moved, work blocked or unblocked, or a new question. Update the affected task's
status and sub-line, and the summary with `board.py summary` (the text, the
Tests green tile, the tasks to look at). Work is not finished until the board says it is.

## The words

A **task** is one entry on the board. It carries **asks**, numbered from 1,
each one question with its own recommendation and one **answer**. The owner's
**comment** is free text on the task as a whole, stored as the answer to ask
0. Adam, 2026-09-29: "1 question one answer and then a comment box", and "an
entry is a task not a question because a task can have multiple asks".

One ask is one question that stands alone: it does not say "that rule" or
"the above". Two questions are two asks. A task is answered when every ask
has an answer, or a ruling is recorded on it. A comment alone answers a task
only when the task has no asks.

## Where it lives

The board is a small local web app (Adam, 2026-09-29), in three parts:

- `scripts/board.py`: the server and the command line.
- `docs/board.jsonl`: every task, one per line, in board order. In git.
- `docs/board-answers.jsonl`: every answer ever given, append-only. In git.

`~/.board/ordo.db` is the SQLite working store. It is outside the repo and rebuilt
from the two files whenever they differ from it, so after a checkout or a merge
there is nothing to do. Every write exports both files before it commits.
`docs/decisions.html` is the page and its fixed prose. It holds no tasks and no
summary. `docs/board-summary.json` holds "Where we are" and is written by
`summary`.

Adam runs `~/scripts/board serve`, a launcher for `scripts/board.py`, and opens `http://127.0.0.1:8737`.
What he types is saved as he types it. There is no save step and no file on
his Desktop.

**Never read `docs/board.jsonl` whole.** It is 420 KB. Use the command line,
which touches one task at a time:

```
python3 scripts/board.py answers            what Adam wrote since the last read
python3 scripts/board.py answers --peek     the same, without marking it read
python3 scripts/board.py ls --needs         tasks waiting on him
python3 scripts/board.py ls --blocked       tasks that are blocked
python3 scripts/board.py ls --grep passkey  find a task
python3 scripts/board.py get <id> [field]   one task, or one field of it
python3 scripts/board.py set <id> sub="..." status=ok:Done explain=@/path/to/file.html
python3 scripts/board.py set <id> --reask   put a task to him again
python3 scripts/board.py add <id> name="..." ask=@file suggest=@file --group "Needs you"
python3 scripts/board.py record <id> "his ruling, in his words" [--when 2026-09-29] [--ask N]
python3 scripts/board.py rm <id>
python3 scripts/board.py group "New section" --before "Done"
```

Long values go in a scratch file and are passed as `field=@path`. Options
(`--after`, `--group`, `--reask`) go after the fields, not before them.

### Where we are

```
python3 scripts/board.py summary            print it
python3 scripts/board.py summary "What changed, in two or three sentences, with [[task-id]] links" \
    --look enrolment-questions:"five decisions" auth-bootstrap:"blocked on the above" \
    --tile "Tests green=1,914"
```

Adam ruled 2026-09-29 that the summary stays short. The command refuses more
than 600 characters. Say what changed since the last summary and point at
tasks; the detail belongs on the task. `[[task-id]]` in the text and every
`--look` entry become links that open the task, and both are refused when the
task does not exist. `--look` replaces the whole list, `--no-look` empties it,
`--tile` sets one tile by its label (`Label=value:class`, class `ok`, `warn`
or `stop`) and `--drop-tile` removes one. The Needs you and Blocked tiles are
counted from the tasks.

**An answered ask is closed.** Once the owner's answer has been read, or a
ruling is recorded on the task, the page shows the answer in place of the box
and only the comment stays open. To ask one ask again and leave the rest closed,
`reask <id> <n>` with the new `ask:` and `suggest:` on stdin; a number past
the last adds an ask. `task --update <id>` reopens every ask. An `ask:` is always a question: a task that needs nothing
from the owner has no asks, and notes go in `explain:` or `detail:`.

**Write for a tired reader.** The owner reads the board at night (2026-09-29:
"lots of words it feels draining and hard to comprehend"). `task` refuses
text over these limits:

| Part | Limit | How to write it |
|---|---|---|
| `explain:` | 350 characters | One line of what this is, then up to four `- ` bullets |
| `ask:` | 160 characters | One question that stands alone |
| `suggest:` | 260 characters | The answer in bold first, then one sentence of why |
| `detail:` | none | Everything else. The page folds it under "More detail" |

Short sentences. One idea per bullet. Numbers as digits. No paragraph longer
than two sentences, in `detail:` as well.

### Where the work stands

Every task says where its work stands and where it was done. The status
says what was decided; `work` and `where` say what became of it (owner,
2026-09-29). Set both whenever a task's state changes:

```
python3 scripts/board.py set <id> work=built where="crates/ordo-host/src/apply_op.rs, commit f1d19f9, test crates/ordo-host/tests/deploy_apply.rs"
python3 scripts/board.py ls --work queued
```

| `work` | Meaning |
|---|---|
| `none` | A decision or a question that produced no work |
| `waiting` | Undecided: the work waits on the owner's answer |
| `filed` | On the checklist with no go to build it |
| `queued` | Decided and not started |
| `building` | Started, with a named remainder |
| `blocked` | Cannot proceed until something else does |
| `built` | Complete |

`where` names things a reader can open: commits, files, tests, a plan. For
`built` it is where the work is. For `building` it is what is done and what
remains. For `none` it is where the decision is recorded. A task that is
decided and unbuilt is `queued`, never left reading Ruled alone.

### Blocked work

A task whose work cannot proceed is `status=stop:Blocked`, and its sub-line
names the tasks it waits on. The page draws the status red and its Blocked
button shows those tasks alone; `ls --blocked` is the same list. When the
blocker clears, set the status back in the same commit.

### Adding a task

Three steps, each a script, and none of them reads the board into context.

**1. Search.** The question, then any extra keywords:

```
python3 scripts/board.py similar "what I need decided" "extra keywords" --top 5
```

It prints the nearest tasks, one line each, with the part of the task that
matched. Read them. As a guide: unrelated tasks score about 0.45 by meaning,
a reworded duplicate 0.60 or more. If a title alone does not settle it,
`get <id> asks` shows that task's asks, numbered.

**2a. It exists: update it and flag it.**

```
python3 scripts/board.py task --update <id> <<'END'
explain: only the parts that changed
ask: the first question as it stands now
suggest: what you would do about it
ask: the second
suggest: ...
END
```

Parts left out are kept. Asks are replaced as a set: give all of them or none. The task is marked `Yours`, its earlier answers stay
in its history, and it shows under Needs me until the owner answers again.

**2b. It does not exist: create it and flag it.**

```
python3 scripts/board.py task "What the task is, as a title" --ref 20b <<'END'
explain: what this is, with the specifics that make it decidable
ask: one decision
suggest: what you would do about it, and why
ask: the next decision
suggest: ...
END
```

Each `ask:` is one line item. A `suggest:` after an ask belongs to that ask;
one before the first ask is for the task as a whole. At least one ask is
required, and every ask needs a recommendation. The task lands after the last one waiting
on the owner. As a backstop `task` runs the search itself and refuses (exit 2)
when a task is alike in meaning (0.55 or more) and shares words as well (0.25
or more); `--force` overrides.

The body is plain text: blank lines make paragraphs, and `backticks` and
`**bold**` are understood.

**How the search compares.** Each task is three texts, its question, its
explanation and its suggestion, each embedded on its own, and every part of
a search is compared with every part of every task. A new question is compared
like with like: its question with theirs, its explanation with theirs, its
suggestion with theirs. Shared words are compared as well, and tasks are
ordered by their rank under both.

Every task is about Ordo, so every task resembles every other. The average of
all the tasks' vectors is what they have in common, and it is subtracted from
each vector before comparing, which leaves what is particular to each.

The model is `mxbai-embed-large-v1`, 1024 dimensions, in `~/.board/model-1024`.
It runs on this machine and sends nothing anywhere. A task's parts are
embedded when it is added or changed, by `task`, `task --update`, `set` and
`add`, and the vectors are kept in the database. A search embeds the query
alone and takes under a second. Rebuilding every vector takes about two minutes.
Without the model, only shared words are compared.

The loop is:

1. Claude adds or updates tasks with the command line.
2. Adam answers in the page and says so.
3. Claude runs `answers`, which prints only what is new, grouped by task
   with each answer under its ask, and marks it read.
   The page then shows those answers under the task's history, and the box
   above is empty for his next one.

## When to use this

- Adam says he has answered or updated the board.
- Adam asks what is outstanding, what is blocked, or what needs him.
- A decision closes through conversation rather than the page. Write it in
  with `record`, or the board stops being the source of truth.
- New blocked or partial work appears and needs a question.

## Marks

Each ask has four buttons, saved to `docs/board-marks.json` apart from the
answers, so the owner can change one at any time, on a closed ask too.
`answers` prints new marks after the answers; `marks` prints them alone
(`--all`, `--mark questions`, `--peek`).

- **green `done`**: he is finished with that ask. Nothing to do.
- **yellow `questions`**: he wants to talk about it. Treat it like a
  `discuss`: answer on the task (`reask` with a clearer ask) or raise it in
  conversation. Never treat it as agreement.
- **red `close`**: he hands the decision to Claude. Decide it on the
  recommendation, `record` it on that ask with the reasoning, and tell him
  in the report.
- **blue `asking` (?)**: Claude's own mark: Claude has a question for him on
  that ask. Set it with `board.py mark <id> <n> asking` and put the question
  in the ask itself (`reask`). The task then counts as Needs you, even when
  settled. Clear it with `mark <id> <n> clear` once he has answered.

## Reading an answer

Answers to asks are text alone. The comment carries a status for the task:
`open`, `decided`, `deferred` or `discuss`.

**Read the text, not just the status.** A `decided` whose text says "do the
opposite of your recommendation" is a decision against you. A `discuss` is a
request for a conversation, not a deferral: answer it on the task (rewrite
`explain` or `suggest`, then `set <id> --reask`) rather than silently leaving
it.

## Acting on it

Work in this order:

1. **`decided`**: implement. The answer is authority; it outranks your own
   recommendation and any prior ruling in `corpus-rulings.md`. If a decision
   contradicts a settled document, that is a real conflict: say so on the
   board rather than quietly doing one or the other. Then `record` the ruling
   on the task, which sets `decided` and adds it to the history.
2. **`discuss`**: answer it on the task and `--reask`.
3. **`deferred`**: leave the work and leave the task, with a sub-line saying
   it was deferred. Deferred is not closed.
4. **`open` with text**: Adam is thinking aloud. Treat the text as input, not
   instruction, and reflect it back in the task.

When a decision leads to a ruling, record it in `docs/corpus-rulings.md` in the
established form, reasoning and not just outcome, and cite the decision date.

## What goes on the board

- Every **blocked** item, as a question.
- Every **partial** item whose remainder needs a judgement rather than just work.
- Any **confirmation** where you are proceeding on a reading Adam has not
  explicitly endorsed. These are cheap to ask and expensive to get wrong.

**Remove tasks that are closed and no longer informative.** A board that only
grows stops being read. The answers stay in `docs/board-answers.jsonl`.

Each task carries a `ref` (checklist item, or `new`/`audit` for something that
has no item yet), a status (`warn:Yours` is what puts it under Needs me), an
`explain` with the specifics that make it decidable, and its asks, each with a
`suggest`: **what you would do and why**, never a bare question. Adam is deciding, not
researching; give him the argument so he can disagree with it. The three long
fields are HTML.

**A defect found in use gets its own task** in the board group "Reported
fixes" (owner, 2026-09-29: "so we keep it as a reported fix and can be aware
of potential regressions in the future"). Each says what was seen, the
cause, the fix, what a regression would look like, and the names of the
tests that guard it. Add the test names when the fix lands.

## The documents list and the future roadmap

`python3 scripts/board.py docs` prints every document in `docs/` with what it
is for, and `docs <text>` searches it. It is how a session with no memory of
this work orients. **A commit that adds a document adds it to the list**:
`docs set <file> "<what it is for>" --kind plan`. `docs missing` names what
the list leaves out, and should print that all are listed before a push.

Work placed after the first deployment carries `--future` and shows under the
page's Future tab. `ls --future` lists it; `set <id> --now` brings a task
back when its turn comes.

## Answer history

A task's history is every answer on it in `docs/board-answers.jsonl`, oldest
first, and the page shows it under the answer box. What Adam types in the page
joins the history when `answers` reads it. **A ruling given in conversation
goes in with `record`**, verbatim, with the date it was given; `record` also
sets `decided` to it. Rulings from before 2026-09-29 whose date was not kept
show as undated entries.

Each task opens as **one block**: explain across the top, then the asks as
numbered lines each with its answer box, then the comment box and the history (owner's
layout, 2026-09-29). Do not split a task's fields back into separate panels.

## What not to do

- **Do not hand-edit the two `.jsonl` files or the database.** Use
  `scripts/board.py`, which keeps all three in step.
- **Do not treat silence as agreement.** An unanswered question stays open, and
  proceeding on your own recommendation without an answer is exactly what the
  board exists to prevent.
- **Do not let it drift from reality.** If work lands that closes a question,
  update the task before the next report, or the board starts lying, which is
  worse than not having it.
- **Do not start the server for Adam or open a browser to check it.** He runs
  it. If the page might be broken, ask him what he sees.

## What each command costs

The board is 425,000 characters on disk. The commands exist so that none of
it is read. Sizes measured 2026-09-29:

| To do this | Run | Comes back |
|---|---|---|
| Read the owner's new answers | `answers` | only what is unread |
| Read the summary | `summary` | about 800 characters |
| See what waits on the owner | `ls --needs` | about 400 characters |
| Find a task | `ls --grep <text>` | one line per match |
| Check a task exists | `similar "<text>"` | about 470 characters |
| Read one field | `get <id> asks` | that field alone |
| Read a task | `get <id>` | about 2,000 characters |
| Change a task | `set <id> field="..."` | nothing; send the new value only |

- **Ask for the field, not the task**, when one field is what is needed.
- **Never run `ls` bare.** It returns 16,000 characters. Use `--needs` or
  `--grep`.
- **Never read a task in order to change it.** `set` and `task --update`
  replace the parts given and keep the rest.
- **`docs/decisions.html` is prose only**, 30,000 characters. Find the
  paragraph with a search and edit that paragraph; do not read the page.

## Where each part lives

| Part | Location | In git |
|---|---|---|
| Tasks | `docs/board.jsonl` | yes |
| Answers and comments, append-only | `docs/board-answers.jsonl` | yes |
| Where we are: summary, tiles, tasks to look at | `docs/board-summary.json` | yes |
| The documents and what each is for | `docs/board-docs.jsonl` | yes |
| The page and its fixed prose | `docs/decisions.html` | yes |
| The Grief Remediation tab's rows, served at `/compare` | `docs/supabase-comparison.html` | yes |
| Each ask's mark: done, questions, close, asking (`board.py marks`) | `docs/board-marks.json` | yes |
| Adam's note on each of those rows (`board.py grief`, skill `grief-remediation`) | `docs/grief-notes.json` | yes |
| Server and command line | `scripts/board.py` | yes |
| Working database | `~/.board/ordo.db` | no |
| Launcher | `~/scripts/board` | no |
| Embedding model, for finding similar questions | `~/.board/model-1024`, `~/.board/lib` | no |
