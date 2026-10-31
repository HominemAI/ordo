/**
 * Signing in by an invitation link, and choosing a team. The door is a stub.
 */

import { describe, expect, it } from 'vitest';
import { NoTeamError, Session } from '../src/session.js';
import { CREATION_OPTIONS, REQUEST_OPTIONS, credentials, door, signedIn } from './support.js';

const options = (d: ReturnType<typeof door>) => ({
  baseUrl: 'https://door.example/',
  fetch: d.fetch,
  credentials,
  now: () => 1000,
});

const join = { space: 'spc_team', invitation: 'tkt_1' };

describe('signing up by an invitation link', () => {
  it('sends the team and the ticket, and takes the team the door admitted', async () => {
    const d = door(
      { json: CREATION_OPTIONS },
      { json: signedIn('usr_bo', 'spc_bo', { admission: { space: 'spc_team', outcome: 'admitted' } }) },
    );
    const session = await Session.signUp(options(d), 'Bo', join);
    expect(d.sent[1]?.body).toMatchObject({ space: 'spc_team', invitation: 'tkt_1' });
    expect(session.admittedSpace).toBe('spc_team');
    expect(session.ownSpace).toBe('spc_bo');
  });

  it('counts an already-member answer as admitted, and a refusal as not', async () => {
    const again = door(
      { json: REQUEST_OPTIONS },
      { json: signedIn('usr_bo', 'spc_bo', { admission: { space: 'spc_team', outcome: 'already-member' } }) },
    );
    expect((await Session.signIn(options(again), join)).admittedSpace).toBe('spc_team');
    expect(again.sent[1]?.body).toMatchObject({ space: 'spc_team', invitation: 'tkt_1' });

    const refused = door(
      { json: REQUEST_OPTIONS },
      { json: signedIn('usr_bo', 'spc_bo', { admission: { space: 'spc_team', outcome: 'refused' } }) },
    );
    expect((await Session.signIn(options(refused), join)).admittedSpace).toBeNull();
  });

  it('sends no team with a plain sign-in', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn('usr_bo', 'spc_bo') });
    await Session.signIn(options(d));
    expect(d.sent[1]?.body).not.toHaveProperty('space');
    expect(d.sent[1]?.body).not.toHaveProperty('invitation');
  });
});

describe('choosing a team', () => {
  it('lists the teams the door says the person is in', async () => {
    const d = door(
      { json: REQUEST_OPTIONS },
      { json: signedIn('usr_bo', 'spc_bo') },
      {
        json: [
          { space: 'spc_bo', name: null, role: 'owner' },
          { space: 'spc_team', name: 'Acme', role: 'editor' },
        ],
      },
    );
    const session = await Session.signIn(options(d));
    const teams = await session.teams();
    expect(teams.map((t) => [t.space, t.role])).toEqual([
      ['spc_bo', 'owner'],
      ['spc_team', 'editor'],
    ]);
    expect(new URL(d.sent[2]?.url ?? '').pathname).toBe('/auth/spaces');
  });

  it('has no client until a team is chosen', async () => {
    const d = door({ json: REQUEST_OPTIONS }, { json: signedIn('usr_bo', 'spc_bo') });
    const session = await Session.signIn(options(d));
    expect(() => session.client()).toThrow(NoTeamError);
    session.useTeam('spc_team');
    expect(session.client().space).toBe('spc_team');
  });
});
