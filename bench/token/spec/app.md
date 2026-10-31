# Team tasks: the app to build

Build a small team task tracker as a web page over a backend. This file says
what it does, what the page must show, and how it is run. A hidden test suite
checks every rule here, through the page and through the backend's own API.
The stack notes at the end (`## Stack`) say how this maps onto your stack.

Build only what this file asks for. Nothing else is scored.

## 1 · What it does

- **People** sign in with an emailed code. There are no passwords.
- **A team** is a group of people with shared tasks.
  - The first time a person signs in, they get a team of their own. They are
    its **owner**.
  - A team has exactly one owner, the person it was made for.
  - Everyone else in a team is a **member**.
  - A person may belong to several teams: their own, and any they joined.
  - The team is made by the backend when the person first signs in. The
    hidden suite also signs people in through the backend's API alone,
    without the page, and expects their team to be there.
- **A task** belongs to one team. It has:
  - a title (text, not empty);
  - a done flag (starts false);
  - an assignee: nobody, or one person in the same team.

## 2 · Who may do what

| Action | Owner | Member | Anyone else, signed in | Signed out |
|---|---|---|---|---|
| See the team's tasks | yes | yes | no | no |
| Add, edit, mark done, assign, delete a task | yes | yes | no | no |
| See the team's people | yes | yes | no | no |
| Make an invitation link | yes | no | no | no |
| Remove a member | yes | no | no | no |
| Change anyone's role | no | no | no | no |
| Join without an invitation | n/a | n/a | no | no |

- These rules hold on the backend, not only on the page. The hidden suite
  calls the backend directly as each kind of person.
- A task can only be assigned to a person in the same team.
- A removed member loses access at once: their next request is refused.
- The owner cannot be removed.

## 3 · Joining a team

1. The owner presses **Invite member**. The page shows a link.
2. The owner gives the link to someone (the suite copies it).
3. That person opens the link while signed out. The sign-in screen shows the
   text `Sign in to join the team.`
4. They sign in with an emailed code, as anyone does. They are now a member
   of the inviting team (and owner of a team of their own, if they are new).
5. The page shows the team they just joined.

A link must work for at least one person. Whether it works more than once
is up to you. Opening a link while already signed in is not tested.

## 4 · The page

The page is a React app built with Vite. Labels, roles and words below are
the contract: the suite finds things by them. Use real form controls with
`<label>`s. Anything not named here is up to you.

### Signed out

| What | Exact contract |
|---|---|
| Email field | an input labelled `Email` |
| Send button | a button named `Email me a code` |
| Code field | an input labelled `Code`, shown once a code was sent |
| Sign-in button | a button named `Sign in` |
| Invited | the text `Sign in to join the team.` when opened from an invitation link |

### Signed in

| What | Exact contract |
|---|---|
| Who | the text `Signed in as <email>` |
| Sign out | a button named `Sign out`. Afterwards the signed-out screen shows |
| Team picker | a `<select>` labelled `Team`, one option per team the person belongs to. **Each option's text is the email of that team's owner**, exactly |
| Role | the text `Your role: owner` or `Your role: member`, for the team shown |

Everything below is for the team chosen in the picker.

**Tasks.** A region named `Tasks` (a `<section>` with a heading `Tasks` that
labels it).

| What | Exact contract |
|---|---|
| New task | an input labelled `New task` and a button named `Add task` |
| The list | a list (`<ul>` or `<ol>`), one list item per task |
| In each item | the task's title as text |
| | a checkbox named `Done`, checked when the task is done |
| | a `<select>` labelled `Assignee`: an option `Nobody`, then one option per person in the team, whose text is their email |
| | a button named `Edit`. It shows an input labelled `Title` and a button named `Save` in the same item |
| | a button named `Delete` |

**People.** A region named `People` (a `<section>` with a heading `People`).

| What | Exact contract |
|---|---|
| The list | a list, one item per person in the team |
| In each item | the person's email, and their role as the word `owner` or `member` |
| Remove | for the owner only: a button named `Remove` in each member's item |
| Invite | for the owner only: a button named `Invite member`. It shows a read-only input labelled `Invite link` whose value is the full link |

A member sees neither `Remove` nor `Invite member`.

### Behaviour

- Each change is saved on the backend before the page shows it as done.
- Changes made by other people need not appear live. A reload must show them.
- A reload keeps the person signed in, and shows the same team.
- After a reload, a team the person was removed from is gone from the picker,
  and none of its tasks show.
- An error the person can act on is shown as text on the page, not only in
  the console.

## 5 · How it is run

The suite starts your app with the stack's start script (see `## Stack`)
from this folder, then opens the page at `http://localhost:5310` in
Chromium. The page must work at that address.

- `web/` is the page: a Vite project. The suite runs `npm install` there if
  `node_modules` is missing, then `vite build` and `vite preview`.
- The backend is built from the files `## Stack` names. Nothing you start by
  hand survives into the test run.
- Emailed codes go to a local mail catcher. You can read them while you work
  (`## Stack` says where).

## 6 · What you are given

- this file;
- `guidance/`: your stack's documentation;
- a starter: an empty Vite React page in `web/`, and your stack's tools;
- `run-stack.sh`: the same start script the suite uses.

You may run the stack, the page and your own checks as often as you like.
