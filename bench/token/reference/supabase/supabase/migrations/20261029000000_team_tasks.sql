-- The bench's team task tracker: the reference build on Supabase.
--
-- A team is a `teams` row. Its people are `team_members` rows, one owner and
-- any number of members. A person gets a team of their own when they sign up
-- (a trigger on auth.users). Others join through an invitation token
-- (create_invite, accept_invite). Row level security keeps every read and
-- write inside the caller's own teams.

-- Helpers live outside the API's schemas, so PostgREST does not expose them.
create schema if not exists private;
grant usage on schema private to authenticated;

-- ── Tables ─────────────────────────────────────────────────────────────────

create table public.teams (
  id uuid primary key default gen_random_uuid(),
  owner_id uuid not null references auth.users (id) on delete cascade,
  created_at timestamptz not null default now()
);
comment on table public.teams is 'A group of people with shared tasks. Made for its owner at sign-up.';

create table public.team_members (
  team_id uuid not null references public.teams (id) on delete cascade,
  user_id uuid not null references auth.users (id) on delete cascade,
  email text not null,
  role text not null check (role in ('owner', 'member')),
  joined_at timestamptz not null default now(),
  primary key (team_id, user_id)
);
comment on table public.team_members is 'Who belongs to which team, and as what.';
create index team_members_user_id_idx on public.team_members (user_id);

create table public.tasks (
  id uuid primary key default gen_random_uuid(),
  team_id uuid not null references public.teams (id) on delete cascade,
  title text not null check (length(btrim(title)) > 0),
  done boolean not null default false,
  assignee uuid,
  created_at timestamptz not null default now(),
  -- The assignee must be a person in the same team. Removing them leaves
  -- the task with nobody.
  foreign key (team_id, assignee) references public.team_members (team_id, user_id) on delete set null (assignee)
);
comment on table public.tasks is 'A team task: a title, a done flag and an optional assignee from the same team.';
create index tasks_team_id_idx on public.tasks (team_id);

create table public.invites (
  token text primary key default encode(extensions.gen_random_bytes(18), 'hex'),
  team_id uuid not null references public.teams (id) on delete cascade,
  created_by uuid not null references auth.users (id) on delete cascade,
  expires_at timestamptz not null default now() + interval '7 days'
);
comment on table public.invites is 'Invitation tokens. Reached only through create_invite and accept_invite.';

-- ── Who is in which team ───────────────────────────────────────────────────
-- Security definer, so the policies below can ask without recursing into
-- team_members' own policy.

create or replace function private.is_member(p_team uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1 from public.team_members m
    where m.team_id = p_team and m.user_id = (select auth.uid())
  );
$$;

create or replace function private.is_owner(p_team uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1 from public.team_members m
    where m.team_id = p_team and m.user_id = (select auth.uid()) and m.role = 'owner'
  );
$$;

grant execute on function private.is_member(uuid), private.is_owner(uuid) to authenticated;

-- ── Row level security ─────────────────────────────────────────────────────

alter table public.teams enable row level security;
alter table public.team_members enable row level security;
alter table public.tasks enable row level security;
alter table public.invites enable row level security;

create policy "Members can see their teams"
on public.teams for select to authenticated
using ((select private.is_member(id)));

create policy "Members can see the people of their teams"
on public.team_members for select to authenticated
using ((select private.is_member(team_id)));

-- No insert or update policy: nobody joins except through accept_invite,
-- and nobody changes a role.
create policy "Owners can remove members, never the owner"
on public.team_members for delete to authenticated
using ((select private.is_owner(team_id)) and role = 'member');

create policy "Members can see their team tasks"
on public.tasks for select to authenticated
using ((select private.is_member(team_id)));

create policy "Members can add team tasks"
on public.tasks for insert to authenticated
with check ((select private.is_member(team_id)));

create policy "Members can change team tasks"
on public.tasks for update to authenticated
using ((select private.is_member(team_id)))
with check ((select private.is_member(team_id)));

create policy "Members can delete team tasks"
on public.tasks for delete to authenticated
using ((select private.is_member(team_id)));

-- invites has no policy: only the two functions below reach it.

-- ── A team of one's own, at sign-up ────────────────────────────────────────

create or replace function private.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_team uuid;
begin
  insert into public.teams (owner_id) values (new.id) returning id into v_team;
  insert into public.team_members (team_id, user_id, email, role)
  values (v_team, new.id, coalesce(new.email, ''), 'owner');
  return new;
end;
$$;

create trigger on_auth_user_created
after insert on auth.users
for each row execute function private.handle_new_user();

-- ── Invitations ────────────────────────────────────────────────────────────

create or replace function public.create_invite(p_team_id uuid)
returns text
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_token text;
begin
  if not private.is_owner(p_team_id) then
    raise exception 'Only the team owner can invite people.' using errcode = '42501';
  end if;
  insert into public.invites (team_id, created_by)
  values (p_team_id, (select auth.uid()))
  returning token into v_token;
  return v_token;
end;
$$;

create or replace function public.accept_invite(p_token text)
returns uuid
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_team uuid;
  v_user uuid := (select auth.uid());
  v_email text;
begin
  if v_user is null then
    raise exception 'Sign in to join a team.' using errcode = '42501';
  end if;
  select i.team_id into v_team
  from public.invites i
  where i.token = p_token and i.expires_at > now();
  if v_team is null then
    raise exception 'That invitation link is not valid any more.' using errcode = 'P0002';
  end if;
  select u.email into v_email from auth.users u where u.id = v_user;
  insert into public.team_members (team_id, user_id, email, role)
  values (v_team, v_user, coalesce(v_email, ''), 'member')
  on conflict (team_id, user_id) do nothing;
  return v_team;
end;
$$;

revoke execute on function public.create_invite(uuid), public.accept_invite(text) from public, anon;
grant execute on function public.create_invite(uuid), public.accept_invite(text) to authenticated;
