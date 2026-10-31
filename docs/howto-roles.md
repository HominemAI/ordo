# How to set who may call each op

For app developers. The tenant side is built in `crates/ordo-host/src/profile_ops.rs`.

## The idea

Who may call which op is runtime state. It is never written in the program.

- You ship a default for each op, the **template**.
- Each tenant's owner may change it for their own tenant.
- A tenant's changes never touch another tenant.

A tenant's **profile** is the template with that tenant's own lines laid over it, op by op.

## 1 · Roles

There are four: owner, manager, editor, viewer. They are fixed.

With no template, the built-in default applies:

- Every role calls authored reads.
- Editors and above call authored writes.
- Managers and owners call the `std.*` ops.

## 2 · Your template

Set `ORDO_ACCESS_TEMPLATE` beside the program. It is read once at boot.

```
ORDO_ACCESS_TEMPLATE="reportByProject=owner,manager; reportByPerson=owner,manager"
```

- Entries split on `;` or a newline.
- Each entry is `op=role,role`.
- `op=` with no roles closes the op to every role until a tenant opens it.
- An op not in the template keeps the built-in default.

The boot refuses on any of these:

- A platform op, such as `std.space.invite`.
- A pattern, such as `std.*`.
- An unknown role.
- The same op twice.

A name the program does not have does nothing. The boot census prints a `profiles` line that says what was read.

The template lists the program's own ops only: app ops and `std.<entity>.*`.

## 3 · A tenant's own profile

A tenant's manager or owner reads and changes it with platform ops. They ask for a fresh passkey touch, like other `std.space` ops. A delegated token cannot call the writes.

| Op | Does |
|---|---|
| `std.space.profile` | Reads the profile. `op` is optional |
| `std.space.setOpAccess` | Sets one op. `op`, `roles` (csv), `groups` (csv) |
| `std.space.resetOpAccess` | Drops the tenant's line, so the op follows the template again |
| `std.space.setMemberAccess` | Overrides one member. `member`, `op`, `effect` |
| `std.space.createGroup` | Makes a group. `name` |
| `std.space.addToGroup` | Adds a member. `group`, `member` |
| `std.space.removeFromGroup` | Removes a member. `group`, `member` |
| `std.space.deleteGroup` | Deletes a group. `group` |
| `std.space.groups` | Lists groups |

`setOpAccess` replaces the whole line for that op in this tenant. Open the report to a group:

```
POST /ops/std.space.createGroup     {"name": "finance"}
POST /ops/std.space.addToGroup      {"group": "finance", "member": "<member id>"}
POST /ops/std.space.setOpAccess     {"op": "reportByProject", "roles": "owner,manager", "groups": "finance"}
```

Hand it back to the template:

```
POST /ops/std.space.resetOpAccess   {"op": "reportByProject"}
```

### Deploys

- An op the tenant set is the tenant's from then on. A deploy never changes it.
- An op the tenant never set follows the template.
- A new op reaches every tenant at its template default.

### A manager hands out only what a manager holds

A manager may change the profile only to hand out ops the manager can call. Only an owner widens past that (owner ruling 2026-10-10). There is no manager self-escalation.

- The rule covers every path that widens: `setOpAccess`, `setMemberAccess` with `allow`, `resetOpAccess` when the default reaches more roles, and `addToGroup` when the group's ops exceed what the person holds now.
- Narrowing is always allowed. So is `removeFromGroup`.
- The refusal names the op and says only an owner can grant it. Nothing changes.
- An owner is not limited.

## 4 · Groups

A group is a named set of members. A tenant's own roles are groups.

- A group grants ops through the profile.
- A group does not confer a role.
- Groups are flat. A group inside a group is refused (`group_nesting`).

Deleting a group is one command group. It removes the group's links to its members and the group's row.

The delete is refused with `group_in_use` while the group still grants anything:

- a profile line names it. Take it out with `setOpAccess`, or `resetOpAccess`.
- an access grant on a record names it. Remove the grant.

A delete that silently revoked access would be the quietest authorization change there is. Nothing changes when it is refused.

### Tenants define their own roles over resources

An app developer can let each tenant give people different permissions on different resources. A tenant makes groups such as "Design" and "Finance", then grants one resource to a group or to one person. This is the same idea as naming a role and locking projects to it.

The app developer writes the resource once, with `acl`:

```
entity projects {
  name: text
  acl (read, write, admin)
}
```

`acl` derives `grantProjectsAccess` and a link entity, `projectsAccess`. The tenant does the rest at runtime:

```
POST /ops/std.space.createGroup     {"name": "Design"}
POST /ops/std.space.addToGroup      {"group": "<Design id>", "member": "<member id>"}
POST /ops/grantProjectsAccess       {"project": "<project id>", "member": "<Design id>", "level": "write"}
POST /ops/grantProjectsAccess       {"project": "<other project id>", "member": "<member id>", "level": "read"}
```

A member of Design opens the first project, and a person outside Design does not. The second grant names one person.

- The derived guards expand groups themselves. The program has no group walk to write.
- A caller's level is the highest over every row that applies, direct or through a group.
- Removing a person from the group removes their access on the next request.
- Who may grant a project is the project's own admins. Who may call `grantProjectsAccess` at all is the profile in section 3.

The full program is `examples/concepts/08-groups-and-project-access.ordo`. The conformance test is `a_group_member_opens_a_granted_resource_and_a_non_member_does_not` in `crates/ordo-conformance/tests/group_membership.rs`.

## 5 · Overrides

`setMemberAccess` allows or denies one op for one member. `effect` is `allow`, `deny` or `default`.

```
POST /ops/std.space.setMemberAccess  {"member": "<member id>", "op": "reportByPerson", "effect": "deny"}
```

The order of resolution:

1. The role allowlist, joined with the allowlists of the member's groups.
2. Then the member's own overrides.

Deny wins.

## 6 · What a tenant cannot change

- Platform ops and deployment surfaces cannot be listed. The refusal says so and points at `std.space.profile`.
- Owner-only capabilities stay the owner's.
- A delete closed to delegated tokens stays closed, whatever a tenant opens. See `ORDO_DELEGABLE_OPS` in `howto-workers.md`.
- Step-up stays the op's own.

## 7 · The record

Every change is one command group in the tenant's own chain. The actor is the caller. The row is imaged.

- Entity names: `$opaccess`, `$memberaccess`, `members`, `groupMembers`.
- A revert of a profile group is refused (`unknown_entity`). The way back is another profile command.
- The door is rebuilt after each change. Only that tenant's profile moves.

## 8 · Export and import

A space export carries the profile in `profile.json`:

- the tenant's op lines,
- the per-member overrides,
- the groups and who is in them.

The manifest counts each. Import restores them in the destination as one batch of profile commands by the importer, so they are on the destination's chain.

- Groups are made new in the destination. Their ids change. Op lines are rewritten to the new ids.
- People go through the people map, like every other row. A member the map does not place refuses the import (`profile_member_unmapped`). A profile never lands on the wrong person, and it never admits anyone.
- A line for an op the program does not have refuses the import (`profile_op_unknown`). Dropping it would leave that op at the template default, which can be wider than what the tenant set.
- A destination that already has groups or profile lines refuses (`profile_destination_not_empty`). Merging has no ruled policy.
- Re-running the same import name restores nothing twice.

An older bundle with no `profile.json` restores no profile. Its tenant starts at the template.

## 9 · What a caller sees

An op someone may not call answers `access_denied` (403). It is the same answer as an unknown op.

## 10 · Not built

- A group that holds a role.

Packages do not exist yet. When they do, the template moves into the package manifest (`domain-packages.md` §1).
