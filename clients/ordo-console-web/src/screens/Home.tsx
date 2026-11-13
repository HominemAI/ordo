import { useEffect, useRef, useState, type MouseEvent } from 'react';
import { base, handle, type Session } from '../api';
import { identityMode } from '../identity';
import { Deployment } from './Deployment';
import { Deploys } from './Deploys';
import { Health } from './Health';
import { Admins } from './Admins';
import { Owners } from './Owners';
import { People } from './People';
import { Runtime } from './Runtime';
import { Settings } from './Settings';
import { Shares } from './Shares';
import { Tenants } from './Tenants';
import { Usage } from './Usage';
import { Windows } from './Windows';

/**
 * The signed-in console: a bar with the groups, a row of the open group's
 * pages, and one screen. Two groups (owner, 2026-11-13): System is the
 * process, Deployment is everything that manages the deployment. Inside
 * Deployment the pages sit in clusters: what is deployed and how it is
 * set, what the program holds, and who administers it.
 */
const GROUPS = [
  { id: 'system', name: 'System' },
  { id: 'deployment', name: 'Deployment' },
] as const;

type GroupId = (typeof GROUPS)[number]['id'];

const SCREENS = [
  { path: '', name: 'Health', group: 'system' },
  { path: 'runtime', name: 'Runtime', group: 'system' },
  { path: 'deployment', name: 'Connection', group: 'deployment' },
  { path: 'deploys', name: 'Deploys', group: 'deployment' },
  { path: 'settings', name: 'Settings', group: 'deployment' },
  { path: 'sign-in', name: 'Sign-in', group: 'deployment' },
  { path: 'windows', name: 'Maintenance', group: 'deployment' },
  { path: 'tenants', name: 'Tenants', group: 'deployment', cluster: true },
  { path: 'people', name: 'People', group: 'deployment' },
  { path: 'shares', name: 'Shares', group: 'deployment' },
  { path: 'usage', name: 'Usage', group: 'deployment' },
  { path: 'owners', name: 'Owners', group: 'deployment', cluster: true },
  { path: 'admins', name: 'Deployment people', group: 'deployment', cluster: true },
] as const satisfies readonly { path: string; name: string; group: GroupId; cluster?: true }[];

type ScreenPath = (typeof SCREENS)[number]['path'];

function current(): ScreenPath {
  const rest = window.location.pathname.slice(base.length).replace(/\/+$/, '');
  return SCREENS.find((s) => s.path === rest)?.path ?? '';
}

function groupOf(path: ScreenPath): GroupId {
  return SCREENS.find((s) => s.path === path)?.group ?? 'system';
}

/** The first page of a group: where its link in the bar leads. */
function firstOf(group: GroupId): ScreenPath {
  return SCREENS.find((s) => s.group === group)?.path ?? '';
}

export function Home(props: { session: Session; onSignedOut: () => void; onSignOut: () => Promise<void> }) {
  const [screen, setScreen] = useState<ScreenPath>(current);
  const pages = useRef<HTMLUListElement>(null);

  useEffect(() => {
    const back = () => setScreen(current());
    window.addEventListener('popstate', back);
    return () => window.removeEventListener('popstate', back);
  }, []);

  // The row of pages scrolls sideways when it does not fit; keep the open
  // page in view.
  useEffect(() => {
    pages.current
      ?.querySelector('a[aria-current="page"]')
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [screen]);

  const go = (path: ScreenPath) => {
    window.history.pushState(null, '', `${base}${path}`);
    setScreen(path);
  };

  const follow = (path: ScreenPath) => (e: MouseEvent<HTMLAnchorElement>) => {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    go(path);
  };

  const group = groupOf(screen);
  // With the identity service the console keeps no owners of its own; the
  // deployment's people live there. Without it, there are none.
  const shown = SCREENS.filter((s) => s.group === group && (identityMode() ? s.path !== 'owners' : s.path !== 'admins'));

  return (
    <>
      <header className="bar">
        <div className="bar-row">
          <span className="brand">Ordo console</span>
          <span className="muted handle">{handle}</span>
          <nav aria-label="Console">
            <ul className="groups">
              {GROUPS.map((g) => (
                <li key={g.id}>
                  <a
                    href={`${base}${firstOf(g.id)}`}
                    aria-current={group === g.id ? 'true' : undefined}
                    onClick={follow(firstOf(g.id))}
                  >
                    {g.name}
                  </a>
                </li>
              ))}
            </ul>
          </nav>
          <button type="button" className="signout" onClick={() => void props.onSignOut()}>
            Sign out
          </button>
        </div>
        <nav aria-label={`${GROUPS.find((g) => g.id === group)?.name} pages`}>
          <ul className="pages" ref={pages}>
            {shown.map((s) => (
              <li key={s.path} className={'cluster' in s && s.cluster ? 'cluster' : undefined}>
                <a href={`${base}${s.path}`} aria-current={screen === s.path ? 'page' : undefined} onClick={follow(s.path)}>
                  {s.name}
                </a>
              </li>
            ))}
          </ul>
        </nav>
      </header>
      <main className="wide">
        {screen === '' && (
          <Health
            onSignedOut={props.onSignedOut}
            onRuntime={() => go('runtime')}
            onDeployment={() => go('deployment')}
          />
        )}
        {screen === 'deployment' && <Deployment onSignedOut={props.onSignedOut} />}
        {screen === 'runtime' && <Runtime onSignedOut={props.onSignedOut} />}
        {screen === 'deploys' && <Deploys onSignedOut={props.onSignedOut} />}
        {screen === 'windows' && <Windows onSignedOut={props.onSignedOut} />}
        {screen === 'tenants' && <Tenants onSignedOut={props.onSignedOut} />}
        {screen === 'people' && <People onSignedOut={props.onSignedOut} />}
        {screen === 'sign-in' && <Settings key="sign-in" scope="signin" onSignedOut={props.onSignedOut} />}
        {screen === 'settings' && <Settings key="all" scope="all" onSignedOut={props.onSignedOut} />}
        {screen === 'usage' && <Usage onSignedOut={props.onSignedOut} />}
        {screen === 'shares' && <Shares onSignedOut={props.onSignedOut} />}
        {screen === 'owners' && !identityMode() && <Owners session={props.session} onSignedOut={props.onSignedOut} />}
        {screen === 'admins' && identityMode() && <Admins session={props.session} onSignedOut={props.onSignedOut} />}
      </main>
    </>
  );
}
