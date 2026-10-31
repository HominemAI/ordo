# Environments: dev, staging and production

Status: **ruled, 2026-10-01** (board `environments`, all five asks). From the owner's notes on the Grief Remediation tab
(2026-10-01, rows g33, g35, g36, g37, g38, g39). Board: `environments`.
Checklist 27e to 27h.

The owner (g33): *"dev staging prod should be all things Ordo supports."*

---

## 1 · Where we are

- **`ordo dev`** exists (`plan-dev-loop.md`, `deploying.md` §1b). It watches a
  directory, recompiles on save, and serves through the same gate and apply
  path a server uses. It runs one database and one space, with test keys.
- **No staging concept exists.** Nothing names an environment, and nothing
  promotes a version from one to the next.
- **Production** is any deployment `ordo apply` points at.

## 2 · The model

The owner's picture, made concrete:

| Environment | What it is | Data |
|---|---|---|
| **dev** | Created by applying. Because the schema is declarative, it is identical to production's shape. | Seed or generated data. Never real. |
| **staging** | The same program run against a **clone of named production tenants**. | A copy of those tenants. |
| **production** | Per tenant. Each tenant moves on its own. | Real. |

### 2.1 Apply moves up one environment at a time

```
ordo apply              to dev (the default)
ordo apply --staging    refused until dev runs this version
ordo apply --prod       refused until staging runs this version
```

"Runs this version" is checked by hash. The same source compiles to the same
hash only if compiling is byte-stable across machines, which is unproven. A
test for that comes first (27e).

With stored plans (`plan-evolution-plans.md` §3.1) the step is: render once
in dev, then run the **same plan** in staging and production. The plan staging
ran is the plan production runs.

### 2.2 Dev branches are dev tenants (g35)

Schema is per tenant, so a branch can be a tenant in the dev deployment:

```
ordo apply --branch feature-x     a dev tenant named for the branch
```

It costs one tenant database, not one project. It is removed when the branch
is, and only on an explicit command: a merge never deletes it (the Supabase
UAT loss, row g35).

**Ruled 2026-10-01**, with a later step: *"it may make sense to have custom
builds / branches for 'special' tenants, public tests of beta builds etc. so
maybe a branch could be promoted. This would mean serving more versions."*
Promoting a branch to serve chosen production tenants depends on serving
more than one version (`plan-evolution-plans.md` §3.5) and is filed for after
first deployment (`tenant-builds`).

### 2.3 Staging clones named tenants (g37)

The deployment owner names which production tenants staging copies. Each is
exported and imported through the existing bundle path, so every row is
re-sealed under staging's own keys. Production's keys never leave production.

**Ruled 2026-10-01: masked by default.** The owner: *"that's a dev to end
user contract."* Whether a clone may keep real personal data is the app
developer's decision, made under their agreement with their users, so it is
the app developer's setting, default masked.

### 2.4 Blue/green per tenant (g33)

Production moves tenant by tenant. **Ruled 2026-10-01**, a program setting
with these strategies:

| Strategy | What it does |
|---|---|
| **rolling** (default) | A batch every 5 minutes, all tenants within an hour. |
| **all** | Every tenant at once. |
| **canary** | A random n tenants first, the rest after a wait. |
| **blue/green** | Two named groups; the second moves when the first is healthy. |
| **by query** | The tenants a query selects move first, in its order. |

The owner's examples of a query, each a selection over tenant facts the
platform already keeps:

- the tenants with the least usage (metering, 17n);
- the tenants with the most rows of a given entity;
- the tenants with some text in their name or metadata;
- the tenants with the fewest members;
- every tenant except those on a given version.

The batch size, interval and window are settings. Each tenant's move is
seamless (`plan-evolution-plans.md` §3.6), so a batch that fails leaves its
tenants serving the old version. Needs `settings-in-code` (18ac).

## 3 · Config travels with the program (g36)

Auth settings, origins and limits are environment variables today. The
owner: *"we'll make this declarative with our ticket on putting all
configuration in Ordo lang."* That ticket is `settings-in-code` (18ac). Each
environment then differs only in secrets and in values the program marks as
per-environment.

## 4 · Destructive commands name their environment (g38)

Every command that can lose data (`rollback` past a drop, a plan with a drop,
`ordo dev --fresh`, a tenant delete) prints the environment and the
deployment, and in production asks for the deployment's name typed back.
Detail comes with the rollout build.

## 5 · Seeding and generated data (g37)

The owner asked to start from a list of when seeding is needed:

| When | What it needs |
|---|---|
| A new dev environment or branch | Enough rows to click through every screen |
| Tests | Exact, repeatable rows |
| Staging | Production's shape and volume, without real people |
| Demos | Plausible, safe content |

Two pieces: `seed.sql` learns sealed fields (today it cannot seal), and a
**data generation module** produces rows from the schema. The generator is a
future item.

## 6 · Permissions (g33)

The owner asked how this plays into permissions and security. Proposed:

- **Dev:** anyone with the repository. Test keys. No real data.
- **Staging:** a deployment owner role. Holds cloned data, so it is treated
  as production for access, with its own keys.
- **Production:** deployment owners, through the console's separate sign-in
  (ruled 2026-10-01, `deployment-owner-identity`).
- **Promotion** to staging or production is an act by a named deployment
  owner, recorded on the deployment's stream.

## 7 · Later

- **A GitHub integration** (g33): connect a repository; a push applies to dev,
  a merge to main renders a plan for staging. Future.
- **The data generation module** (§5). Future.

## 8 · Open for the owner

On the board as `environments`:

All five ruled 2026-10-01: dev by default with promotion gates; dev branches
as tenants (promotion to special tenants later); masking by default as the
app developer's contract; rollout strategies as above; staging treated as
production for access. Recorded in `corpus-rulings.md`.
