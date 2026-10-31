# Working on Ordo

## The board is the source of truth

The decisions board is the official roadmap and the record of what the owner
has ruled (owner rulings, 2026-09-18 and 2026-09-29). Where the project
stands, what is open, what is blocked and what has been decided are read from
the board and written to it. A claim about project state that the board does
not carry is not yet true.

**Load the `decisions` skill before touching the board**
(`.claude/skills/decisions/SKILL.md`). It holds every command, the text
limits, the marks, the work states and what each command costs.

| Word | Meaning |
|---|---|
| **Task** | One entry on the board. It has an id, a status, an explanation and any number of asks. |
| **Ask** | One question inside a task, numbered from 1, with its own recommendation. One ask takes one answer. |
| **Answer** | What the owner wrote against one ask. |
| **Comment** | What the owner wrote about the task as a whole. Stored as the answer to ask 0. |

### Rules

1. **Everything goes through `scripts/board.py`.** Never read
   `docs/board.jsonl` or `docs/board-answers.jsonl` whole, and never edit
   them by hand. Never run `ls` bare; use `--needs`, `--grep` or `get <id> <field>`.
2. **Questions for the owner go on the board** as asks, each with a
   recommendation. They are not asked in chat. Search with `similar` first.
3. **The owner's answers outrank everything else**: a recommendation, a plan,
   and any earlier ruling in `docs/corpus-rulings.md`. Read the text of an
   answer, not only its status. An unanswered ask is open.
4. **The board and `docs/roadmap-checklist.md` must agree.** A commit that
   changes project state updates both, and `node scripts/board-agrees.mjs`
   passes before it is made.
5. **A ruling is recorded twice**: on its task with `record`, and in
   `docs/corpus-rulings.md` with the reasoning and the date.
6. **Every task says where its work stands**: set `work` and `where`
   whenever its state changes.
7. **A defect found in use gets its own task** in the group "Reported fixes",
   with what was seen, the cause, the fix, what a regression looks like and
   the tests that guard it.
8. **Write for a tired reader.** Short sentences, one idea per bullet, numbers
   as digits. `task` refuses text over its limits.
9. **Each service is reached through one interface** (owner, 2026-10-23). The
   database, mail, file storage, outside sign-in providers, and the door seen
   from the SDK each get one client with one typed error, never their own
   handling and custom errors at every call site. Callers use the client;
   errors keep their cause and say what a caller can do (retry, refuse, fix
   input); one place at the edge turns them into an HTTP answer.

## Web and design work

The project lists the `modern-web-guidance` skills by name only
(`.claude/settings.json`), because most Ordo work is Rust. An agent writing
page, CSS or browser code invokes `modern-web-guidance:modern-web-guidance`
first, and its brief says so.

## Sizing work

Size work in agent sessions and CI rounds, never in a person's days or weeks
(owner, 2026-10-16: "Why do you measure in human weeks vs claude weeks?").

| Size | Means |
|---|---|
| S | part of one agent session |
| M | about 1 session (30 to 60 minutes) and 1 to 2 CI rounds |
| L | 2 to 3 sessions, with a review between them |

What this was measured on, 2026-10-11: public path phase 2 took one Opus
session of about 48 minutes, 14 audit fixes about 43, the Supabase import
design about 6. Each of sign-in steps 1 to 10, sized M and L, was one
session. Name what really sets the pace beside the size: a CI run is about
12 minutes, the owner's answers, rehearsals against real data, and research
whose outcome is unknown (say how many tries it may take).
