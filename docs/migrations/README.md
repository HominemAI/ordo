# Migration logs

One file per try at moving a real app into Ordo, named
`<app>-<yyyy-mm-dd>.md`. The method in `docs/plan-supabase-import.md` comes
from these logs, and then becomes a skill (owner, 2026-10-16).

Each log records every step, in order:

| Field | What to write |
|---|---|
| Step | What was run, the command or the file |
| Assumed | What we took as true without checking |
| Saw | What happened, with counts |
| Broke | What failed, and the cause once known |
| Decided | What a person had to choose, and who chose it |
| Time | Agent time and CI rounds |

End each log with what the next try should do differently.
