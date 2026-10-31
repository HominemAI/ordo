import { OrdoProvider, type DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import type { ListedSpace } from '@hominemai/ordo-sdk/auth/share.js';
import { useEffect, useMemo, useRef, useState } from 'react';
import { BoardView } from './components/BoardView.js';
import { MembersView } from './components/MembersView.js';
import { ProjectsView } from './components/ProjectsView.js';
import { ReportView } from './components/ReportView.js';
import { SignedOut } from './components/SignedOut.js';
import { TeamPicker } from './components/TeamPicker.js';
import { DOOR, returnUrl } from './config.js';
import { friendly, returnedErrorText, type Way } from './errors.js';
import { useHash } from './hooks.js';
import { loadMethods } from './methods.js';
import type { SignInMethods } from './config.js';
import { NoTeamError, Session, untilServed, type Join } from './session.js';
import { beginOutside, deviceMarker, guestSecret, keepLastTeam, lastTeam, returnedFrom } from './signin.js';
import { isAdmin, joinRoute, meOf, type Me } from './teamspace.js';

type Tab = 'board' | 'projects' | 'members' | 'report';

interface In {
  readonly session: Session;
  readonly space: string;
  readonly hooks: HooksClient;
  readonly me: Me;
  readonly teams: readonly ListedSpace[];
}

/** The invitation link in the address, in the form the sign-in options take. */
const joinOf = (hash: string): Join | undefined => {
  const route = joinRoute(hash);
  return route ? { space: route.space, invitation: route.ticket } : undefined;
};

export function App() {
  const hash = useHash();
  const join = joinOf(hash);
  const [signedIn, setSignedIn] = useState<In | null>(null);
  const [picking, setPicking] = useState<{ session: Session; teams: readonly ListedSpace[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('board');
  const options = useMemo(() => ({ baseUrl: DOOR }), []);
  const origin = window.location.origin;
  // The ways in come from the door. Until they arrive the sign-in screen waits.
  const [methods, setMethods] = useState<SignInMethods | null>(null);
  useEffect(() => {
    let alive = true;
    void loadMethods(options).then((m) => alive && setMethods(m));
    return () => {
      alive = false;
    };
  }, [options]);

  /** Bind the session to a team, check the door serves it, and show the team. */
  const open = async (session: Session, space: string, teams: readonly ListedSpace[]) => {
    session.useTeam(space);
    const seat = await untilServed(session.client());
    if (!seat) throw new NoTeamError('you have no seat in that team');
    keepLastTeam(space);
    session.onEnd(() => setSignedIn(null));
    setPicking(null);
    setTab('board');
    setSignedIn({ session, space, hooks: session.hooksClient(), me: meOf({ ...seat }), teams });
  };

  /**
   * Choose the team to work in. An invitation that admitted the person names
   * it. Otherwise the door lists the person's teams, and a person in more
   * than one picks (the last pick is remembered).
   */
  const settle = async (session: Session) => {
    if (session.admittedSpace !== null) {
      const teams = await session.teams().catch(() => []);
      return open(session, session.admittedSpace, teams);
    }
    const teams = await session.teams();
    const ids = teams.map((t) => t.space);
    if (session.ownSpace !== null && !ids.includes(session.ownSpace)) {
      ids.push(session.ownSpace);
    }
    const remembered = lastTeam();
    const first = ids[0];
    if (remembered !== null && ids.includes(remembered)) return open(session, remembered, teams);
    if (ids.length > 1) {
      setPicking({ session, teams });
      return;
    }
    if (first === undefined) throw new NoTeamError('not provisioned');
    return open(session, first, teams);
  };

  const enter = async (make: () => Promise<Session>, way: Way = 'passkey') => {
    setBusy(true);
    setError(null);
    try {
      await settle(await make());
    } catch (e) {
      setError(friendly(e, origin, DOOR, way));
    } finally {
      setBusy(false);
    }
  };

  // Back from an outside provider: `?handoff=` or `?error=` in the address.
  // Read once, then the address is put back as it was before leaving.
  const returned = useRef(false);
  useEffect(() => {
    if (returned.current) return;
    returned.current = true;
    const back = returnedFrom(window.location.search);
    if (back.kind === 'none') return;
    window.history.replaceState(null, '', `${window.location.pathname}${back.hash}`);
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    if (back.kind === 'error') {
      setError(returnedErrorText(back.error));
      return;
    }
    void enter(() => Session.fromOutside(options, back, joinOf(back.hash)), 'outside');
    // `enter` is stable enough for a run-once effect.
  }, []);

  // Once in, the invitation link has done its work.
  useEffect(() => {
    if (signedIn && join) window.location.hash = '';
  }, [signedIn, join]);

  const signUp = (display: string) => void enter(() => Session.signUp(options, display, join));
  const signIn = () => void enter(() => Session.signIn(options, join));
  const guest = () => void enter(() => Session.asGuest(options, join), 'guest');
  const emailCode = (sent: EmailCodeSent, code: string) => void enter(() => Session.withEmail(options, sent, code, join), 'email');
  const emailBegin = async (address: string): Promise<EmailCodeSent | null> => {
    setBusy(true);
    setError(null);
    try {
      return await Session.beginEmail(options, address, deviceMarker(), join);
    } catch (e) {
      setError(friendly(e, origin, DOOR, 'email'));
      return null;
    } finally {
      setBusy(false);
    }
  };
  const outside = async (connection: string) => {
    setBusy(true);
    setError(null);
    try {
      const redirect = await beginOutside(options, connection, returnUrl(window.location), window.location.hash);
      window.location.assign(redirect);
    } catch (e) {
      setError(friendly(e, origin, DOOR, 'outside'));
      setBusy(false);
    }
  };

  const signOut = async () => {
    const session = signedIn?.session ?? picking?.session;
    if (!session) return;
    try {
      await session.signOut();
    } catch (e) {
      setError(friendly(e, origin));
      return;
    }
    setPicking(null);
    setSignedIn(null);
  };

  const pick = (session: Session, space: string, teams: readonly ListedSpace[]) => {
    setBusy(true);
    setError(null);
    open(session, space, teams)
      .catch((e: unknown) => setError(friendly(e, origin)))
      .finally(() => setBusy(false));
  };

  const admin = signedIn !== null && isAdmin(signedIn.me.role);
  const view: Tab = admin ? tab : 'board';
  const tabs: readonly [Tab, string][] = [
    ['board', 'Board'],
    ['projects', 'Projects'],
    ['members', 'Members'],
    ['report', 'Report'],
  ];

  return (
    <main>
      <header>
        <h1>Teamspace</h1>
        {signedIn && (
          <div className="who">
            <span>
              {signedIn.me.displayName} <span className="hint">({signedIn.me.role})</span>
            </span>
            {signedIn.teams.length > 1 && (
              <button onClick={() => (setPicking({ session: signedIn.session, teams: signedIn.teams }), setSignedIn(null))}>
                Switch team
              </button>
            )}
            <button onClick={() => void signOut()}>Sign out</button>
          </div>
        )}
      </header>

      {signedIn !== null ? (
        <>
          {admin && (
            <nav aria-label="Sections">
              {tabs.map(([id, label]) => (
                <button key={id} aria-pressed={view === id} onClick={() => setTab(id)}>
                  {label}
                </button>
              ))}
            </nav>
          )}
          <OrdoProvider key={signedIn.space} client={signedIn.hooks} live stepUp={() => signedIn.session.stepUp()}>
            {view === 'board' && <BoardView me={signedIn.me} />}
            {admin && view === 'projects' && <ProjectsView />}
            {admin && view === 'members' && <MembersView me={signedIn.me} space={signedIn.space} />}
            {admin && view === 'report' && <ReportView />}
          </OrdoProvider>
        </>
      ) : picking !== null ? (
        <>
          <TeamPicker teams={picking.teams} onPick={(space) => pick(picking.session, space, picking.teams)} />
          {error !== null && (
            <p role="alert" className="error">
              {error}
            </p>
          )}
          <p>
            <button onClick={() => void signOut()}>Sign out</button>
          </p>
        </>
      ) : methods === null ? (
        <p className="hint">Checking how you can sign in...</p>
      ) : (
        <SignedOut
          methods={methods}
          invited={join !== undefined}
          busy={busy}
          error={error}
          onSignUp={signUp}
          onSignIn={signIn}
          onEmailBegin={emailBegin}
          onEmailCode={emailCode}
          onGuest={guest}
          guestHere={guestSecret() !== null}
          onOutside={(c) => void outside(c)}
        />
      )}
    </main>
  );
}
