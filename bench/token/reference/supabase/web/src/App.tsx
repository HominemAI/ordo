import { useCallback, useEffect, useState, type FormEvent } from 'react';
import type { Session } from '@supabase/supabase-js';
import {
  acceptInvite,
  addTask,
  changeTask,
  db,
  deleteTask,
  invite,
  inviteLink,
  joinOf,
  myTeams,
  people as listPeople,
  removePerson,
  sendCode,
  tasks as listTasks,
  verifyCode,
  type Person,
  type Task,
  type Team,
} from './db';

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

const describe = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [checking, setChecking] = useState(true);
  const [join] = useState<string | null>(() => joinOf(window.location.hash));
  // The team joined from an invitation link. Joining finishes just after the
  // session arrives, so Home is drawn again once it has.
  const [joined, setJoined] = useState('');

  useEffect(() => {
    db.auth
      .getSession()
      .then(({ data }) => setSession(data.session))
      .finally(() => setChecking(false));
    const { data } = db.auth.onAuthStateChange((_event, s) => setSession(s));
    return () => data.subscription.unsubscribe();
  }, []);

  if (checking) return <p>Loading…</p>;
  if (!session) {
    return (
      <SignIn
        join={join}
        onJoined={(team) => {
          remember(team);
          window.history.replaceState(null, '', window.location.pathname);
          setJoined(team);
        }}
      />
    );
  }
  return <Home key={joined} session={session} />;
}

function SignIn({ join, onJoined }: { join: string | null; onJoined: (team: string) => void }) {
  const [email, setEmail] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const send = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      await sendCode(email.trim());
      setSent(true);
      setError('');
    } catch (err) {
      setError(describe(err));
    } finally {
      setBusy(false);
    }
  };

  const verify = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      // Join before the session reaches the page, so the first read shows
      // the team. The session is already held by the client at this point.
      await verifyCode(email.trim(), code);
      if (join) onJoined(await acceptInvite(join));
    } catch (err) {
      setError(`That did not work. ${describe(err)}`);
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

function Home({ session }: { session: Session }) {
  const me = session.user.id;
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [team, setTeam] = useState<string | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    myTeams(me)
      .then((list) => {
        setTeams(list);
        const kept = remembered();
        const own = list.find((t) => t.role === 'owner');
        setTeam((list.find((t) => t.id === kept) ?? own ?? list[0])?.id ?? null);
      })
      .catch((e) => setError(describe(e)));
  }, [me]);

  const choose = (id: string) => {
    remember(id);
    setTeam(id);
  };

  const current = teams?.find((t) => t.id === team) ?? null;

  return (
    <main>
      <header>
        <h1>Team tasks</h1>
        <p>Signed in as {session.user.email}</p>
        <button type="button" onClick={() => void db.auth.signOut()}>
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
      {current && <TeamView key={current.id} team={current} />}
    </main>
  );
}

function TeamView({ team }: { team: Team }) {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [people, setPeople] = useState<Person[]>([]);
  const [title, setTitle] = useState('');
  const [link, setLink] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [t, p] = await Promise.all([listTasks(team.id), listPeople(team.id)]);
      setTasks(t);
      setPeople(p);
    } catch (e) {
      setError(describe(e));
    }
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
      await addTask(team.id, t);
      setTitle('');
    });
  };

  const change = (task: Task, patch: Partial<Omit<Task, 'id'>>) => act(() => changeTask(task.id, patch));

  const makeLink = () =>
    act(async () => {
      setLink(inviteLink(await invite(team.id)));
    });

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
            <TaskItem key={t.id} task={t} people={people} onChange={change} onDelete={() => act(() => deleteTask(t.id))} />
          ))}
        </ul>
      </section>
      <section aria-labelledby="people-heading">
        <h2 id="people-heading">People</h2>
        <ul>
          {people.map((p) => (
            <li key={p.id}>
              {p.email} <span>{p.role}</span>{' '}
              {owner && p.role !== 'owner' && (
                <button type="button" onClick={() => void act(() => removePerson(team.id, p.id))}>
                  Remove
                </button>
              )}
            </li>
          ))}
        </ul>
        {owner && (
          <p>
            <button type="button" onClick={() => void makeLink()}>
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
  task: Task;
  people: readonly Person[];
  onChange: (task: Task, patch: Partial<Omit<Task, 'id'>>) => Promise<void>;
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
            {p.email}
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
