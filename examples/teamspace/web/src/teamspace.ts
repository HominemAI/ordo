/**
 * What the page knows about a team, with no React and no network: the shape
 * of the rows the ops answer, the invitation link, and the small rules the
 * screens share. The hooks (`useOpQuery`, `useCommit`) do the calling; this
 * file only builds what they send and reads what they answer, so it is tested
 * with no render.
 */

type Wire = Readonly<Record<string, unknown>>;

const str = (r: Wire, k: string): string => (typeof r[k] === 'string' ? r[k] : '');
const strOrNull = (r: Wire, k: string): string | null => (typeof r[k] === 'string' && r[k] !== '' ? r[k] : null);
const num = (r: Wire, k: string): number => (typeof r[k] === 'number' ? r[k] : Number(r[k] ?? 0) || 0);

export type Role = 'owner' | 'manager' | 'editor' | 'viewer';

export const STATUSES = ['todo', 'doing', 'review', 'done'] as const;
export type Status = (typeof STATUSES)[number];

export const STATUS_LABEL: Readonly<Record<Status, string>> = {
  todo: 'To do',
  doing: 'Doing',
  review: 'Review',
  done: 'Done',
};

/** Owners and managers run the team. */
export const isAdmin = (role: string | null | undefined): boolean => role === 'owner' || role === 'manager';

/** A person who can add and move tasks. A viewer can only read. */
export const canWrite = (role: string | null | undefined): boolean => role !== 'viewer';

/** The roles an invitation can name. Only an owner can invite an owner. */
export function invitableRoles(myRole: string | null | undefined): readonly Role[] {
  return myRole === 'owner' ? ['editor', 'manager', 'viewer', 'owner'] : ['editor', 'manager', 'viewer'];
}

/** The neighbouring column, or null at the edge. */
export function neighbour(status: string, step: -1 | 1): Status | null {
  const i = STATUSES.indexOf(status as Status);
  if (i < 0) return null;
  return STATUSES[i + step] ?? null;
}

/** What `datetime` parameters look like on the wire: seconds, and a `Z`. */
export function stamp(now: Date = new Date()): string {
  return `${now.toISOString().slice(0, 19)}Z`;
}

/** What `date` parameters look like on the wire: `YYYY-MM-DD`, by the local calendar. */
export function today(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

export interface Me {
  readonly id: string;
  readonly displayName: string;
  readonly role: string;
}

export const meOf = (r: Wire): Me => ({ id: str(r, 'id'), displayName: str(r, 'displayName'), role: str(r, 'role') });

export interface Person {
  readonly id: string;
  readonly displayName: string;
  readonly role: string;
  readonly status: string;
}

export const personOf = (r: Wire): Person => ({
  id: str(r, 'id'),
  displayName: str(r, 'displayName'),
  role: str(r, 'role'),
  status: str(r, 'status'),
});

export interface Project {
  readonly id: string;
  readonly rev: number;
  readonly name: string;
  readonly summary: string;
}

/** A row of `myProjects` (no `rev`) or of `std.projects.list`. */
export const projectOf = (r: Wire): Project => ({
  id: str(r, 'id'),
  rev: num(r, 'rev'),
  name: str(r, 'name'),
  summary: str(r, 'summary'),
});

export interface TeamSeat {
  /** The row of `team`, which `std.team.delete` takes. */
  readonly id: string;
  readonly member: string;
  readonly memberName: string;
  readonly memberRole: string;
}

export const seatOf = (r: Wire): TeamSeat => ({
  id: str(r, 'id'),
  member: str(r, 'member'),
  memberName: str(r, 'memberName'),
  memberRole: str(r, 'memberRole'),
});

export interface Task {
  readonly id: string;
  readonly rev: number;
  readonly title: string;
  readonly status: string;
  readonly assignee: string | null;
  readonly assigneeName: string | null;
  readonly authorName: string;
  readonly due: string | null;
}

export const taskOf = (r: Wire): Task => ({
  id: str(r, 'id'),
  rev: num(r, 'rev'),
  title: str(r, 'title'),
  status: str(r, 'status'),
  assignee: strOrNull(r, 'assignee'),
  assigneeName: strOrNull(r, 'assigneeName'),
  authorName: str(r, 'authorName'),
  due: strOrNull(r, 'due'),
});

/** The tasks of one column, in the order the door sent them. */
export const inColumn = (tasks: readonly Task[], status: Status): Task[] => tasks.filter((t) => t.status === status);

/** Past due and not done. Dates are `YYYY-MM-DD`, so they compare as text. */
export const overdue = (t: Task, now: string): boolean => t.due !== null && t.due < now && t.status !== 'done';

export interface Invitation {
  readonly id: string;
  readonly role: string;
  readonly label: string;
  readonly issuedBy: string;
  readonly expiresAt: number;
  readonly status: string;
}

export const invitationOf = (r: Wire): Invitation => ({
  id: str(r, 'id'),
  role: str(r, 'role'),
  label: str(r, 'label'),
  issuedBy: str(r, 'issuedBy'),
  expiresAt: num(r, 'expiresAt'),
  status: str(r, 'status'),
});

export interface ProjectReport {
  readonly id: string;
  readonly name: string;
  readonly total: number;
  readonly todo: number;
  readonly doing: number;
  readonly review: number;
  readonly done: number;
  readonly overdue: number;
}

export const projectReportOf = (r: Wire): ProjectReport => ({
  id: str(r, 'id'),
  name: str(r, 'name'),
  total: num(r, 'total'),
  todo: num(r, 'todo'),
  doing: num(r, 'doing'),
  review: num(r, 'review'),
  done: num(r, 'done'),
  overdue: num(r, 'overdue'),
});

export interface PersonReport {
  readonly id: string;
  readonly displayName: string;
  readonly role: string;
  readonly total: number;
  readonly open: number;
  readonly done: number;
  readonly overdue: number;
}

export const personReportOf = (r: Wire): PersonReport => ({
  id: str(r, 'id'),
  displayName: str(r, 'displayName'),
  role: str(r, 'role'),
  total: num(r, 'total'),
  open: num(r, 'open'),
  done: num(r, 'done'),
  overdue: num(r, 'overdue'),
});

// ── invitation links ─────────────────────────────────────────────────────

/** An invitation link's parts. */
export interface JoinRoute {
  readonly space: string;
  readonly ticket: string;
}

const PART = '[A-Za-z0-9._~-]+';

/** The team and ticket in `#/join/<space>/<ticket>`, or null. */
export function joinRoute(hash: string): JoinRoute | null {
  const m = new RegExp(`^#/join/(${PART})/(${PART})$`).exec(hash);
  return m && m[1] && m[2] ? { space: m[1], ticket: m[2] } : null;
}

/**
 * The address a colleague opens. The ticket rides in the fragment, so no
 * server ever receives it.
 */
export function inviteLink(location: { readonly origin: string; readonly pathname: string }, space: string, ticket: string): string {
  return `${location.origin}${location.pathname}#/join/${space}/${ticket}`;
}

/** A moment in seconds since the epoch, for people. */
export const when = (secs: number): string => new Date(secs * 1000).toLocaleString();
