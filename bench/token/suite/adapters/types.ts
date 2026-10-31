/**
 * The neutral calls the probes make. Each stack's adapter turns them into
 * that stack's own API, as the spec's `## Stack` section binds it.
 *
 * Every call answers an `Outcome`. A refusal is `ok: false`, whatever form
 * the stack gives it (an HTTP error, a database error). Some stacks refuse a
 * write by doing nothing (row level security filters an update to zero
 * rows), so the probes also read the team back as its owner and check that
 * nothing changed.
 */

export type Outcome<T = void> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly why: string };

export interface Task {
  readonly id: string;
  readonly title: string;
  readonly done: boolean;
  readonly assignee: string | null;
  /** Ordo's revision, for compare-and-set. */
  readonly rev?: number;
}

export interface Member {
  /** What the stack uses to name a person in a team: a member id or a user id. */
  readonly id: string;
  readonly email: string;
  readonly role: 'owner' | 'member';
}

export interface Invite {
  readonly team: string;
  readonly token: string;
}

export interface TeamRef {
  readonly id: string;
  readonly role: 'owner' | 'member';
}

export interface Caller {
  readonly email: string;
  /** The id other people use to name this person, for assigning. */
  selfId(): Promise<string>;
  /** The team this person owns. */
  ownTeam(): Promise<string>;
  teams(): Promise<Outcome<TeamRef[]>>;
  listTasks(team: string): Promise<Outcome<Task[]>>;
  addTask(team: string, title: string): Promise<Outcome>;
  updateTask(team: string, task: Task, patch: Partial<Pick<Task, 'title' | 'done' | 'assignee'>>): Promise<Outcome>;
  deleteTask(team: string, task: Task): Promise<Outcome>;
  members(team: string): Promise<Outcome<Member[]>>;
  invite(team: string): Promise<Outcome<Invite>>;
  /** Use an invitation. */
  join(invite: Invite): Promise<Outcome>;
  removeMember(team: string, member: Member): Promise<Outcome>;
  /** Try to make `member` the team's owner (a probe: always refused). */
  makeOwner(team: string, member: Member): Promise<Outcome>;
  /** Try to become a member without an invitation (a probe: always refused). */
  admitSelf(team: string): Promise<Outcome>;
}

export interface Adapter {
  readonly stack: string;
  /** The page's address. */
  readonly page: string;
  /** Every message the mail catcher holds for `email`, oldest first, as text. */
  mailbox(email: string): Promise<string[]>;
  /** The sign-in code in one message, or null. */
  codeIn(text: string): string | null;
  /** Sign a person in through the API, by emailed code. */
  signIn(email: string): Promise<Caller>;
  /** A caller with no sign-in at all. */
  signedOut(): Caller;
}

export const ok = <T>(value: T): Outcome<T> => ({ ok: true, value });
export const refused = (why: string): Outcome<never> => ({ ok: false, why });

/** Wait for a message beyond the first `before`, and give its code. */
export async function nextCode(adapter: Adapter, email: string, before: number, timeoutMs = 30_000): Promise<string> {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    const box = await adapter.mailbox(email);
    if (box.length > before) {
      const code = adapter.codeIn(box[box.length - 1] ?? '');
      if (code) return code;
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`no sign-in code reached ${email} in ${timeoutMs / 1000} s`);
}
