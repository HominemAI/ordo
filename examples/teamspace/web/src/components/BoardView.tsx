import { useCommit, useOpQuery } from '@hominemai/ordo-react';
import { useMemo, useState, type FormEvent } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import {
  STATUSES,
  STATUS_LABEL,
  canWrite,
  inColumn,
  neighbour,
  overdue,
  projectOf,
  seatOf,
  stamp,
  taskOf,
  today,
  type Me,
  type Task,
} from '../teamspace.js';

interface Props {
  readonly me: Me;
}

/** The tasks of one project in four columns, and the form to add one. */
export function BoardView({ me }: Props) {
  const projects = useOpQuery('myProjects');
  const list = useMemo(() => projects.rows.map((r) => projectOf({ ...r })), [projects.rows]);
  const [chosen, setChosen] = useState<string | null>(null);
  const project = list.find((p) => p.id === chosen) ?? list[0];
  const params = { project: project?.id ?? '' };
  const enabled = project !== undefined;
  const board = useOpQuery('board', { params, enabled, live: true });
  const team = useOpQuery('projectTeam', { params, enabled });
  const tasks = useMemo(() => board.rows.map((r) => taskOf({ ...r })), [board.rows]);
  const seats = useMemo(() => team.rows.map((r) => seatOf({ ...r })), [team.rows]);
  const create = useCommit('createTask');
  const move = useCommit('moveTask');
  const [title, setTitle] = useState('');
  const [assignee, setAssignee] = useState('');
  const [due, setDue] = useState('');
  const [note, setNote] = useState<string | null>(null);

  const writable = canWrite(me.role);
  const now = today();
  const refusal = projects.refusal ?? board.refusal ?? team.refusal ?? create.refusal ?? move.refusal;
  const shown = note ?? (refusal ? refusalText(refusal) : null);

  const add = async (e: FormEvent) => {
    e.preventDefault();
    if (!project || title.trim() === '') return;
    setNote(null);
    move.reset();
    try {
      const out = await create.commit({
        project: project.id,
        title: title.trim(),
        ...(assignee !== '' ? { assignee } : {}),
        ...(due !== '' ? { due } : {}),
        added: stamp(),
      });
      if (out.status === 'committed') {
        setTitle('');
        setDue('');
        board.refetch();
      }
    } catch (err) {
      setNote(message(err));
    }
  };

  const step = async (t: Task, to: string) => {
    setNote(null);
    create.reset();
    try {
      const out = await move.commit({ id: t.id, status: to }, t.rev);
      if (out.status === 'committed') board.refetch();
      if (out.status === 'conflict') {
        setNote('Changed by someone else, reloaded.');
        board.refetch();
      }
    } catch (err) {
      setNote(message(err));
    }
  };

  if (projects.loading) return <p className="hint">Loading your projects.</p>;
  if (!project) {
    return (
      <section className="card">
        <h2>No projects yet</h2>
        <p className="hint">
          {me.role === 'owner' || me.role === 'manager'
            ? 'Make a project on the Projects tab, then add people to its team.'
            : 'You are not on a project team yet. Ask a manager to add you.'}
        </p>
      </section>
    );
  }

  return (
    <section aria-label="Board">
      <div className="tabs" role="tablist" aria-label="Projects">
        {list.map((p) => (
          <button key={p.id} role="tab" aria-selected={p.id === project.id} onClick={() => setChosen(p.id)}>
            {p.name}
          </button>
        ))}
      </div>
      {project.summary !== '' && <p className="hint">{project.summary}</p>}

      {writable && (
        <form className="card add" onSubmit={(e) => void add(e)}>
          <label htmlFor="task-title">New task</label>
          <input id="task-title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What needs doing?" />
          <div className="row">
            <div className="grow">
              <label htmlFor="task-assignee">Assignee</label>
              <select id="task-assignee" value={assignee} onChange={(e) => setAssignee(e.target.value)}>
                <option value="">Nobody</option>
                {seats.map((s) => (
                  <option key={s.id} value={s.member}>
                    {s.memberName}
                  </option>
                ))}
              </select>
            </div>
            <div className="grow">
              <label htmlFor="task-due">Due</label>
              <input id="task-due" type="date" value={due} onChange={(e) => setDue(e.target.value)} />
            </div>
          </div>
          <div className="row">
            <button type="submit" className="primary" disabled={create.committing || title.trim() === ''}>
              Add task
            </button>
          </div>
        </form>
      )}

      {shown !== null && (
        <p role="alert" className="error">
          {shown}
        </p>
      )}

      <div className="columns">
        {STATUSES.map((status) => {
          const here = inColumn(tasks, status);
          return (
            <section key={status} className="column" aria-label={STATUS_LABEL[status]}>
              <h3>
                {STATUS_LABEL[status]} <span className="hint">{here.length}</span>
              </h3>
              {here.map((t) => {
                const left = neighbour(t.status, -1);
                const right = neighbour(t.status, 1);
                return (
                  <article key={t.id} className="task">
                    <p className="title">{t.title}</p>
                    <p className="hint">
                      {t.assigneeName ?? 'Nobody'}
                      {t.due !== null && (
                        <span className={overdue(t, now) ? 'late' : ''}>{` · due ${t.due}${overdue(t, now) ? ' (late)' : ''}`}</span>
                      )}
                    </p>
                    {writable && (
                      <div className="row">
                        <button aria-label={`Move ${t.title} left`} disabled={left === null || move.committing} onClick={() => left && void step(t, left)}>
                          &lt;
                        </button>
                        <button aria-label={`Move ${t.title} right`} disabled={right === null || move.committing} onClick={() => right && void step(t, right)}>
                          &gt;
                        </button>
                      </div>
                    )}
                  </article>
                );
              })}
            </section>
          );
        })}
      </div>
    </section>
  );
}
