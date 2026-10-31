import { useEffect, useState } from 'react';
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

/**
 * The signed-in console: a bar with the nav, and one screen. The nav lists
 * the screens built so far; later steps add Deploys, Tenants and the rest (docs/plan-console.md section 4) as one entry each here.
 */
const SCREENS = [
  { path: '', name: 'Health' },
  { path: 'runtime', name: 'Runtime' },
  { path: 'deploys', name: 'Deploys' },
  { path: 'deployment', name: 'Deployment' },
  { path: 'tenants', name: 'Tenants' },
  { path: 'people', name: 'People' },
  { path: 'sign-in', name: 'Sign-in' },
  { path: 'settings', name: 'Settings' },
  { path: 'usage', name: 'Usage' },
  { path: 'shares', name: 'Shares' },
  { path: 'owners', name: 'Owners' },
  { path: 'admins', name: 'Deployment people' },
] as const;

type ScreenPath = (typeof SCREENS)[number]['path'];

function current(): ScreenPath {
  const rest = window.location.pathname.slice(base.length).replace(/\/+$/, '');
  return SCREENS.find((s) => s.path === rest)?.path ?? '';
}

export function Home(props: { session: Session; onSignedOut: () => void; onSignOut: () => Promise<void> }) {
  const [screen, setScreen] = useState<ScreenPath>(current);

  useEffect(() => {
    const back = () => setScreen(current());
    window.addEventListener('popstate', back);
    return () => window.removeEventListener('popstate', back);
  }, []);

  const go = (path: ScreenPath) => {
    window.history.pushState(null, '', `${base}${path}`);
    setScreen(path);
  };

  return (
    <>
      <header className="bar">
        <span className="brand">Ordo console</span>
        <span className="muted handle">{handle}</span>
        <nav aria-label="Console">
          <ul>
            {/* With the identity service the console keeps no owners of its own;
                the deployment's people live there. Without it, there are none. */}
            {SCREENS.filter((s) => (identityMode() ? s.path !== 'owners' : s.path !== 'admins')).map((s) => (
              <li key={s.path}>
                <a
                  href={`${base}${s.path}`}
                  aria-current={screen === s.path ? 'page' : undefined}
                  onClick={(e) => {
                    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
                    e.preventDefault();
                    go(s.path);
                  }}
                >
                  {s.name}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <button type="button" className="signout" onClick={() => void props.onSignOut()}>
          Sign out
        </button>
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
