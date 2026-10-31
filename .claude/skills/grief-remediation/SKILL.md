---
name: grief-remediation
description: The board's Grief Remediation tab - Supabase's schema and migration grievances set beside what Ordo does, with Adam's note on each row. Use when Adam says he has written notes on the grief tab, asks what he thought about a grievance, wants a row added or changed, or when migration, schema-evolution, dev/prod or inter-system-table work touches a row.
---

# Grief Remediation

A tab on the decisions board. Each row is one grievance with Supabase's
schema and migration system: who raised it (`Reported`, `Found`, or `Adam`),
what Ordo does today, a verdict, and what to do. Adam opens a row and types
his thoughts under it; they save as he types.

## Where it lives

| Part | Location | In git |
|---|---|---|
| The rows | `docs/supabase-comparison.html`, served at `/compare` | yes |
| Adam's notes, keyed `g<n>` | `docs/grief-notes.json`, written by the page through `/api/grief` | yes |
| Which notes Claude has read | `~/.board/grief-read.json` | no |

## Reading Adam's notes

```
python3 scripts/board.py grief           notes new or changed since the last read; marks them read
python3 scripts/board.py grief --peek    the same, without marking them read
python3 scripts/board.py grief --all     every note
python3 scripts/board.py grief 16        one row: its grievance, Ordo's answer, the verdict, and the note
```

Never read `docs/grief-notes.json` or the rows file whole when one row is
what is needed. `grief <n>` prints one.

## What a note is

A note is Adam's thinking about one row. It is input, not a ruling:

- **It does not decide anything on the board.** A note that reads like a
  decision becomes a task, or a ruling recorded with `record`, through the
  `decisions` skill. Questions back to him go on the board as asks, never in
  chat.
- **It outranks the row.** If a note disagrees with a verdict or a "what to
  do", change the row to match and say so in your reply.
- **A note naming a defect** in Ordo gets its own task under Reported fixes.

## Changing the rows

Edit `docs/supabase-comparison.html` directly. The page needs no change and
no server restart.

- **A row's number is its permanent id.** Notes are keyed by `data-id="g<n>"`.
  Never renumber. A new row takes the next unused number, wherever it sits.
- **Each row is** `<tr data-v="VERDICT" data-id="gN">` with 6 cells: number,
  grievance, who, Ordo today, verdict pill, what to do. `board.py grief`
  parses that shape, so keep it.
- **Verdicts:** `ok` Addressed, `part` Partial, `gap` Gap, `prob` Problem (a
  defect in Ordo today), `opp` Opportunity. The pill is
  `<span class="v ok">Addressed</span>`.
- **Who:** `Reported` (public complaints, with a source at the bottom),
  `Found` (Claude found it in Ordo), `<b>Adam</b>` (his own grievance).
- **When work lands** that changes a verdict, update the row in the same
  commit as the board task.
- **Write for a tired reader:** one or two short sentences per cell, numbers
  as digits, no em dashes.
