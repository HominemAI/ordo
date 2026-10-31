import { useCallback, useEffect, useState, type FormEvent } from 'react';
import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { addTask, deleteTask, listMembers, listTasks, updateTask, type ListMembersRow, type ListTasksRow } from './generated/ops';
import { describe, inviteLink, joinOf, Session, untilServed, type Join, type Team } from './session';

/** What one change sets. `assignee: null` means nobody. */
type Patch = { title?: string; done?: boolean; assignee?: string | null };

const TEAM_KEY = 'team-tasks.team';

function remembered(): string | null {
  try {
    return localStorage.getItem(TEAM_KEY);
  } catch {
    return null;
  }
}

function remember(team: string): void {
  try {
    localStorage.setItem(TEAM_KEY, team);
  } catch {
    // Private mode: the page still works, it just forgets the team.
  }
}

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [checking, setChecking] = useState(true);
  const [join] = useState<Join | null>(() => joinOf(window.location.hash));

  useEffect(() => {
    Session.resume()
      .then((s) => setSession(s))
      .catch(() => setSession(null))
      .finally(() => setChecking(false));
  }, []);

  if (checking) return <p>Loading…</p>;
  if (!session) {
    return (
      <SignIn
        join={join}
        onSignedIn={(s) => {
          if (s.joined) remember(s.joined);
          if (join) window.history.replaceState(null, '', window.location.pathname);
          setSession(s);
        }}
      />
    );
  }
  return <Home session={session} onSignedOut={() => setSession(null)} />;
}

function SignIn({ join, onSignedIn }: { join: Join | null; onSignedIn: (s: Session) => void }) {
  const [email, setEmail] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState<EmailCodeSent | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const send = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      setSent(await Session.sendCode(email.trim(), join));
      setError('');
    } catch (err) {
      setError(describe(err));
    } finally {
      setBusy(false);
    }
  };

  const verify = async (e: FormEvent) => {
    e.preventDefault();
    if (!sent) return;
    setBusy(true);
    try {
      onSignedIn(await Session.signIn(email.trim(), sent, code, join));
    } catch (err) {
      setError(`That code did not work. ${describe(err)}`);
      setBusy(false);
    }
  };

  return (
    <main>
      <h1>Team tasks</h1>
      {join && <p>Sign in to join the team.</p>}
      <form onSubmit={send}>
        <label htmlFor="email">Email</label>
        <input
          id="email"
          name="email"
          type="email"
          autoComplete="username"
          spellCheck={false}
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
        <button type="submit" disabled={busy}>
          Email me a code
        </button>
      </form>
      {sent && (
        <form onSubmit={verify}>
          <p>If {email} can sign in here, a code is on its way.</p>
          <label htmlFor="code">Code</label>
          <input
            id="code"
            name="code"
            inputMode="numeric"
            autoComplete="one-time-code"
            required
            value={code}
            onChange={(e) => setCode(e.target.value)}
          />
          <button type="submit" disabled={busy}>
            Sign in
          </button>
        </form>
      )}
      {error && <p role="alert">{error}</p>}
    </main>
  );
}

function Home({ session, onSignedOut }: { session: Session; onSignedOut: () => void }) {
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [team, setTeam] = useState<string | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    session
      .teams()
      .then((list) => {
        setTeams(list);
        const kept = remembered();
        const own = list.find((t) => t.role === 'owner');
        const chosen = list.find((t) => t.id === session.joined) ?? list.find((t) => t.id === kept) ?? own ?? list[0];
        setTeam(chosen?.id ?? null);
      })
      .catch((e) => setError(describe(e)));
  }, [session]);

  const choose = (id: string) => {
    remember(id);
    setTeam(id);
  };

  const signOut = async () => {
    try {
      await session.signOut();
    } finally {
      onSignedOut();
    }
  };

  const me = teams?.find((t) => t.role === 'owner')?.owner;
  const current = teams?.find((t) => t.id === team) ?? null;

  return (
    <main>
      <header>
        <h1>Team tasks</h1>
        {me && <p>Signed in as {me}</p>}
        <button type="button" onClick={() => void signOut()}>
          Sign out
        </button>
      </header>
      {error && <p role="alert">{error}</p>}
      {teams && (
        <p>
          <label htmlFor="team">Team</label>{' '}
          <select id="team" value={team ?? ''} onChange={(e) => choose(e.target.value)}>
            {teams.map((t) => (
              <option key={t.id} value={t.id}>
                {t.owner}
              </option>
            ))}
          </select>
        </p>
      )}
      {current && <TeamView key={current.id} session={session} team={current} />}
    </main>
  );
}

function TeamView({ session, team }: { session: Session; team: Team }) {
  const client = session.client(team.id);
  const [tasks, setTasks] = useState<readonly ListTasksRow[]>([]);
  const [people, setPeople] = useState<readonly ListMembersRow[]>([]);
  const [title, setTitle] = useState('');
  const [link, setLink] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [t, p] = await untilServed(() => Promise.all([listTasks(client).fetch(), listMembers(client).fetch()]));
      setTasks(t.rows);
      setPeople(p.rows);
    } catch (e) {
      setError(describe(e));
    }
    // `client` is rebuilt each render; the team id is what matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [team.id]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (work: () => Promise<unknown>) => {
    try {
      await work();
      setError('');
    } catch (e) {
      setError(describe(e));
    }
    await load();
  };

  const add = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    if (!t) return;
    void act(async () => {
      await addTask(client, { title: t }).commit();
      setTitle('');
    });
  };

  const change = (task: ListTasksRow, patch: Patch) => {
    // The row's assignee is null for nobody. Leaving the param out takes its
    // default, null, so nobody means "leave it out".
    const assignee = patch.assignee === undefined ? task.assignee : patch.assignee;
    return act(() =>
      updateTask(client, {
        id: task.id,
        rev: task.rev,
        title: patch.title ?? task.title,
        done: patch.done ?? task.done,
        ...(assignee ? { assignee: assignee as ListTasksRow['assignee'] } : {}),
      }).commit(),
    );
  };

  const invite = () =>
    act(async () => {
      const row = await session.platform(team.id, 'std.space.invite', { role: 'editor' });
      setLink(inviteLink(team.id, String(row['ticket'])));
    });

  const remove = (member: string) => act(() => session.platform(team.id, 'std.space.remove', { member }));

  const owner = team.role === 'owner';
  return (
    <>
      <p>Your role: {team.role}</p>
      {error && <p role="alert">{error}</p>}
      <section aria-labelledby="tasks-heading">
        <h2 id="tasks-heading">Tasks</h2>
        <form onSubmit={add}>
          <label htmlFor="new-task">New task</label>
          <input id="new-task" value={title} onChange={(e) => setTitle(e.target.value)} />
          <button type="submit">Add task</button>
        </form>
        <ul>
          {tasks.map((t) => (
            <TaskItem key={t.id} task={t} people={people} onChange={change} onDelete={() => act(() => deleteTask(client, { id: t.id }).commit())} />
          ))}
        </ul>
      </section>
      <section aria-labelledby="people-heading">
        <h2 id="people-heading">People</h2>
        <ul>
          {people.map((p) => (
            <li key={p.id}>
              {p.displayName} <span>{p.role === 'owner' ? 'owner' : 'member'}</span>{' '}
              {owner && p.role !== 'owner' && (
                <button type="button" onClick={() => void remove(p.id)}>
                  Remove
                </button>
              )}
            </li>
          ))}
        </ul>
        {owner && (
          <p>
            <button type="button" onClick={() => void invite()}>
              Invite member
            </button>{' '}
            {link && (
              <>
                <label htmlFor="invite-link">Invite link</label>
                <input id="invite-link" readOnly value={link} size={60} />
              </>
            )}
          </p>
        )}
      </section>
    </>
  );
}

function TaskItem({
  task,
  people,
  onChange,
  onDelete,
}: {
  task: ListTasksRow;
  people: readonly ListMembersRow[];
  onChange: (task: ListTasksRow, patch: Patch) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(task.title);
  const box = `done-${task.id}`;
  const who = `assignee-${task.id}`;
  const edit = `title-${task.id}`;

  const save = (e: FormEvent) => {
    e.preventDefault();
    const t = title.trim();
    if (!t) return;
    void onChange(task, { title: t }).then(() => setEditing(false));
  };

  return (
    <li>
      {editing ? (
        <form onSubmit={save}>
          <label htmlFor={edit}>Title</label>
          <input id={edit} value={title} onChange={(e) => setTitle(e.target.value)} />
          <button type="submit">Save</button>
        </form>
      ) : (
        <span>{task.title}</span>
      )}{' '}
      <input id={box} type="checkbox" checked={task.done} onChange={(e) => void onChange(task, { done: e.target.checked })} />
      <label htmlFor={box}>Done</label>{' '}
      <label htmlFor={who}>Assignee</label>
      <select id={who} value={task.assignee ?? ''} onChange={(e) => void onChange(task, { assignee: e.target.value || null })}>
        <option value="">Nobody</option>
        {people.map((p) => (
          <option key={p.id} value={p.id}>
            {p.displayName}
          </option>
        ))}
      </select>{' '}
      {!editing && (
        <button type="button" onClick={() => setEditing(true)}>
          Edit
        </button>
      )}{' '}
      <button type="button" onClick={() => void onDelete()}>
        Delete
      </button>
    </li>
  );
}
