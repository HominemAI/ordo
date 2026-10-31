import { useCommit, useOpQuery } from '@hominemai/ordo-react';
import { useMemo, useState, type FormEvent } from 'react';
import { refusalText } from '../errors.js';
import { message } from '../hooks.js';
import { personOf, projectOf, seatOf, stamp, type Project } from '../teamspace.js';

/** Admins only: make, rename and delete projects, and say who is on each team. */
export function ProjectsView() {
  const projects = useOpQuery('std.projects.list');
  const list = useMemo(
    () =>
      projects.rows.map((r) => ({
        project: projectOf({ ...r }),
        added: typeof r['added'] === 'string' ? r['added'] : stamp(),
      })),
    [projects.rows],
  );
  const create = useCommit('std.projects.create');
  const update = useCommit('std.projects.update');
  const remove = useCommit('std.projects.delete');
  const [name, setName] = useState('');
  const [summary, setSummary] = useState('');
  const [open, setOpen] = useState<string | null>(null);
  const [thrown, setThrown] = useState<string | null>(null);

  const refusal = create.refusal ?? update.refusal ?? remove.refusal ?? projects.refusal;
  const shown = thrown ?? (refusal ? refusalText(refusal) : null);
  const busy = create.committing || update.committing || remove.committing;

  const run = async (act: () => Promise<{ status: string }>) => {
    for (const hook of [create, update, remove]) hook.reset();
    setThrown(null);
    try {
      const out = await act();
      if (out.status === 'committed') projects.refetch();
      if (out.status === 'conflict') {
        setThrown('Changed by someone else, reloaded.');
        projects.refetch();
      }
    } catch (e) {
      setThrown(message(e));
    }
  };

  const add = (e: FormEvent) => {
    e.preventDefault();
    if (name.trim() === '') return;
    void run(async () => {
      const out = await create.commit({ name: name.trim(), summary: summary.trim(), added: stamp() });
      if (out.status === 'committed') {
        setName('');
        setSummary('');
      }
      return out;
    });
  };

  return (
    <section aria-label="Projects">
      <form className="card" onSubmit={add}>
        <h2>New project</h2>
        <label htmlFor="project-name">Name</label>
        <input id="project-name" value={name} onChange={(e) => setName(e.target.value)} />
        <label htmlFor="project-summary">Summary</label>
        <input id="project-summary" value={summary} onChange={(e) => setSummary(e.target.value)} />
        <div className="row">
          <button type="submit" className="primary" disabled={busy || name.trim() === ''}>
            Make project
          </button>
        </div>
      </form>

      {shown !== null && (
        <p role="alert" className="error">
          {shown}
        </p>
      )}

      {!projects.loading && list.length === 0 && <p className="hint">No projects yet.</p>}
      {list.map(({ project, added }) => (
        <ProjectCard
          key={project.id}
          project={project}
          busy={busy}
          isOpen={open === project.id}
          onToggle={() => setOpen(open === project.id ? null : project.id)}
          onRename={(next) =>
            void run(() =>
              update.commit({ id: project.id, name: next, summary: project.summary, added }, project.rev),
            )
          }
          onDelete={() => void run(() => remove.commit({ id: project.id }))}
        />
      ))}
    </section>
  );
}

interface CardProps {
  readonly project: Project;
  readonly busy: boolean;
  readonly isOpen: boolean;
  readonly onToggle: () => void;
  readonly onRename: (name: string) => void;
  readonly onDelete: () => void;
}

function ProjectCard({ project, busy, isOpen, onToggle, onRename, onDelete }: CardProps) {
  const [name, setName] = useState(project.name);
  return (
    <article className="card">
      <div className="row">
        <input aria-label={`Name of ${project.name}`} value={name} onChange={(e) => setName(e.target.value)} />
        <button disabled={busy || name.trim() === '' || name.trim() === project.name} onClick={() => onRename(name.trim())}>
          Rename
        </button>
        <button onClick={onToggle} aria-expanded={isOpen}>
          Team
        </button>
        <button disabled={busy} onClick={onDelete}>
          Delete
        </button>
      </div>
      {project.summary !== '' && <p className="hint">{project.summary}</p>}
      {isOpen && <TeamPanel project={project} />}
    </article>
  );
}

/** Who is on one project's team, and the people who could be. */
function TeamPanel({ project }: { readonly project: Project }) {
  const team = useOpQuery('projectTeam', { params: { project: project.id } });
  const people = useOpQuery('people');
  const add = useCommit('std.team.create');
  const drop = useCommit('std.team.delete');
  const seats = useMemo(() => team.rows.map((r) => seatOf({ ...r })), [team.rows]);
  const everyone = useMemo(() => people.rows.map((r) => personOf({ ...r })), [people.rows]);
  const free = everyone.filter((p) => !seats.some((s) => s.member === p.id));
  const [pick, setPick] = useState('');
  const [thrown, setThrown] = useState<string | null>(null);
  const refusal = add.refusal ?? drop.refusal ?? team.refusal ?? people.refusal;
  const shown = thrown ?? (refusal ? refusalText(refusal) : null);

  const run = async (act: () => Promise<{ status: string }>) => {
    add.reset();
    drop.reset();
    setThrown(null);
    try {
      const out = await act();
      if (out.status === 'committed') team.refetch();
    } catch (e) {
      setThrown(message(e));
    }
  };

  return (
    <div className="team">
      <h3>Team of {project.name}</h3>
      {seats.length === 0 && <p className="hint">Nobody is on this team yet.</p>}
      <ul className="plain">
        {seats.map((s) => (
          <li key={s.id} className="between">
            <span>
              {s.memberName} <span className="hint">({s.memberRole})</span>
            </span>
            <button disabled={drop.committing} onClick={() => void run(() => drop.commit({ id: s.id }))}>
              Remove
            </button>
          </li>
        ))}
      </ul>
      <div className="row">
        <select aria-label="Person to add" value={pick} onChange={(e) => setPick(e.target.value)}>
          <option value="">Choose a person</option>
          {free.map((p) => (
            <option key={p.id} value={p.id}>
              {p.displayName} ({p.role})
            </option>
          ))}
        </select>
        <button
          disabled={add.committing || pick === ''}
          onClick={() =>
            void run(async () => {
              const out = await add.commit({ project: project.id, member: pick });
              if (out.status === 'committed') setPick('');
              return out;
            })
          }
        >
          Add to team
        </button>
      </div>
      {shown !== null && (
        <p role="alert" className="error">
          {shown}
        </p>
      )}
    </div>
  );
}
