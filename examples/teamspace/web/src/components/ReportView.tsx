import { useOpQuery } from '@hominemai/ordo-react';
import { useMemo } from 'react';
import { refusalText } from '../errors.js';
import { personReportOf, projectReportOf, today } from '../teamspace.js';

/**
 * Admins only: the team's tasks by project and by person. A count that is
 * missing comes back as 0 (`personReportOf` and `projectReportOf` read it so).
 */
export function ReportView() {
  const now = today();
  const byProject = useOpQuery('reportByProject', { params: { today: now } });
  const byPerson = useOpQuery('reportByPerson', { params: { today: now } });
  const projects = useMemo(() => byProject.rows.map((r) => projectReportOf({ ...r })), [byProject.rows]);
  const people = useMemo(() => byPerson.rows.map((r) => personReportOf({ ...r })), [byPerson.rows]);
  const refusal = byProject.refusal ?? byPerson.refusal;

  return (
    <section aria-label="Report">
      {refusal && (
        <p role="alert" className="error">
          {refusalText(refusal)}
        </p>
      )}
      <div className="card">
        <h2>By project</h2>
        <table>
          <thead>
            <tr>
              <th>Project</th>
              <th>Total</th>
              <th>To do</th>
              <th>Doing</th>
              <th>Review</th>
              <th>Done</th>
              <th>Late</th>
            </tr>
          </thead>
          <tbody>
            {projects.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
                <td>{p.total}</td>
                <td>{p.todo}</td>
                <td>{p.doing}</td>
                <td>{p.review}</td>
                <td>{p.done}</td>
                <td className={p.overdue > 0 ? 'late' : ''}>{p.overdue}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {!byProject.loading && projects.length === 0 && <p className="hint">No projects.</p>}
      </div>
      <div className="card">
        <h2>By person</h2>
        <table>
          <thead>
            <tr>
              <th>Person</th>
              <th>Role</th>
              <th>Assigned</th>
              <th>Open</th>
              <th>Done</th>
              <th>Late</th>
            </tr>
          </thead>
          <tbody>
            {people.map((p) => (
              <tr key={p.id}>
                <td>{p.displayName}</td>
                <td>{p.role}</td>
                <td>{p.total}</td>
                <td>{p.open}</td>
                <td>{p.done}</td>
                <td className={p.overdue > 0 ? 'late' : ''}>{p.overdue}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {!byPerson.loading && people.length === 0 && <p className="hint">No people.</p>}
      </div>
    </section>
  );
}
