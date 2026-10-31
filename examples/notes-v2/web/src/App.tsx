import { OrdoProvider, type DescriptorClient as HooksClient } from '@hominemai/ordo-react';
import type { AcceptedShare } from '@hominemai/ordo-sdk/auth/share.js';
import type { EmailCodeSent } from '@hominemai/ordo-sdk/auth/email.js';
import { beginEmailSignIn } from '@hominemai/ordo-sdk/auth/email.js';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AcceptView } from './components/AcceptView.js';
import { AddPasskey, wayIn } from './components/AddPasskey.js';
import { EmailForReminders } from './components/EmailForReminders.js';
import { useHumanCheck } from './components/HumanCheck.js';
import { LinkView } from './components/LinkView.js';
import { FilesProvider } from './components/Files.js';
import { NotesView } from './components/NotesView.js';
import { SearchView } from './components/SearchView.js';
import { SharedView } from './components/SharedView.js';
import { SignedOut } from './components/SignedOut.js';
import { DOOR, returnUrl } from './config.js';
import { friendly, returnedErrorText, type Way } from './errors.js';
import { acceptTicket, linkSecret, useHash } from './hooks.js';
import { loadOffer, type Offer } from './methods.js';
import { openNotesDevice } from './device.js';
import type { OrdoDevice } from '@hominemai/ordo-sdk/replica/device.js';
import type { TenantStatus } from '@hominemai/ordo-sdk/auth/provisioning.js';
import { Session, untilServed } from './session.js';
import { sharedDevices } from './shared.js';
import { forget, openOfflineNotes, recall, remember, resumeOutcome, whenOnline, type OfflineNotes } from './resume.js';
import { beginOutside, deviceMarker, guestSecret, returnedFrom } from './signin.js';

interface In {
  readonly session: Session;
  readonly device: OrdoDevice;
  /** One device per shared tenant, opened by the shared view. */
  readonly shared: ReturnType<typeof sharedDevices>;
  /** The hooks' client for this person's tenant. */
  readonly hooks: HooksClient;
  /** Asks for a fresh passkey touch, which the platform's sharing ops want. */
  readonly stepUp: () => Promise<void>;
}

export function App() {
  const hash = useHash();
  const ticket = acceptTicket(hash);
  const secret = linkSecret(hash);
  const [signedIn, setSignedIn] = useState<In | null>(null);
  // On load the page asks the door whether this browser is still signed in.
  // Until it answers, the sign-in screen waits, so it never flashes.
  const [resuming, setResuming] = useState(true);
  // The door could not be asked on load: the notes held on this device, shown
  // read-only until the network is back and the resume can be asked again.
  const [offline, setOffline] = useState<OfflineNotes | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<'notes' | 'shared' | 'search'>('notes');
  const [accepted, setAccepted] = useState<readonly { space: string; name: string | null }[]>([]);
  // While a new person's tenant is made: the step it has reached.
  const [settingUp, setSettingUp] = useState<TenantStatus | null>(null);
  const options = useMemo(() => ({ baseUrl: DOOR, onSettingUp: setSettingUp }), []);
  // The ways in come from the door. Until they arrive the sign-in screen waits.
  const [offer, setOffer] = useState<Offer | null>(null);
  useEffect(() => {
    let alive = true;
    void loadOffer(options).then((o) => alive && setOffer(o));
    return () => {
      alive = false;
    };
  }, [options]);
  const human = useHumanCheck(offer?.siteKey ?? null);

  /** Erase everything this device holds for the person who is leaving. */
  const purgeAll = useCallback(async (own: In) => {
    await Promise.allSettled([own.device.purge(), own.shared.purge()]);
  }, []);

  const enter = async (make: () => Promise<Session>, way: Way = 'passkey') => {
    setBusy(true);
    setError(null);
    try {
      const session = await make();
      const client = session.client();
      // The door learns of a new tenant a moment after the answer, so the
      // first call can be refused. Wait for it before showing anything.
      await untilServed(client);
      const device = await openNotesDevice(client, session.space, session.principal);
      const next: In = {
        session,
        device,
        shared: sharedDevices(session),
        hooks: session.hooksClient(),
        stepUp: () => session.stepUp(),
      };
      session.onEnd(() => {
        forget();
        void purgeAll(next).then(() => setSignedIn(null));
      });
      remember({ space: session.space, principal: session.principal });
      setSignedIn(next);
    } catch (e) {
      setError(friendly(e, DOOR, way));
    } finally {
      setBusy(false);
      setSettingUp(null);
      // Each human-check token is good for one try.
      human.reset();
    }
  };

  /**
   * Ask the door to continue the sign-in. A refusal goes to sign-in. A network
   * failure opens the device copy when this browser holds one, and leaves the
   * person on sign-in, with the failure said, when it holds none.
   */
  const resume = async () => {
    try {
      const out = await resumeOutcome(options);
      if (out.kind === 'session') {
        const { session } = out;
        await enter(async () => session);
      } else if (out.kind === 'offline') {
        const who = recall();
        if (who === null) {
          setError(friendly(out.error, DOOR));
          return;
        }
        setOffline(await openOfflineNotes(options, who));
      } else {
        // Refused: nothing on this device belongs to a sign-in that is gone.
        forget();
      }
    } catch (e) {
      setError(friendly(e, DOOR));
    }
  };

  // Offline from a failed resume: ask again when the network returns.
  const retrying = useRef(false);
  const retryResume = useCallback(async () => {
    if (offline === null || retrying.current) return;
    retrying.current = true;
    try {
      const out = await resumeOutcome(options);
      if (out.kind === 'offline') return;
      const held = offline;
      setOffline(null);
      if (out.kind === 'session') {
        // The same person: the device copy closes and the real one opens.
        await held.device.close();
        await enter(async () => out.session);
      } else {
        forget();
        await held.device.purge();
      }
    } catch (e) {
      setError(friendly(e, DOOR));
    } finally {
      retrying.current = false;
    }
  }, [offline, options]);
  useEffect(() => (offline === null ? undefined : whenOnline(window, () => void retryResume())), [offline, retryResume]);

  // Back from an outside provider: `?handoff=` or `?error=` in the address.
  // Read once, then the address is put back as it was before leaving.
  const returned = useRef(false);
  useEffect(() => {
    if (returned.current) return;
    returned.current = true;
    const back = returnedFrom(window.location.search);
    if (back.kind === 'none') {
      // A reload, or a browser opened again: resume the sign-in it holds.
      void resume().finally(() => setResuming(false));
      return;
    }
    setResuming(false);
    window.history.replaceState(null, '', `${window.location.pathname}${back.hash}`);
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    if (back.carried) {
      // The trip was to add a passkey, so the person is still signed in and
      // the provider's code is the claim's first proof. It is not a sign-in.
      const carried = back.carried;
      const proof = back.kind === 'handoff' ? back : null;
      void enter(async () => Session.resumeForClaim(options, carried, proof), 'outside').then(() => {
        if (back.kind === 'error') setError(returnedErrorText(back.error));
      });
      return;
    }
    if (back.kind === 'error') {
      setError(returnedErrorText(back.error));
      return;
    }
    void enter(() => Session.fromOutside(options, back), 'outside');
    // `enter` is stable enough for a run-once effect.
  }, []);

  const signUp = (display: string) => void enter(() => Session.signUp(options, display, human.token));
  const signIn = () => void enter(() => Session.signIn(options, human.token));
  const guest = () => void enter(() => Session.asGuest(options, human.token), 'guest');
  const emailCode = (sent: EmailCodeSent, code: string, address: string) =>
    void enter(() => Session.withEmail(options, sent, code, address), 'email');
  const emailBegin = async (address: string): Promise<EmailCodeSent | null> => {
    setBusy(true);
    setError(null);
    try {
      const device = deviceMarker();
      return await beginEmailSignIn(options, {
        address,
        ...(human.token ? { human: human.token } : {}),
        ...(device ? { device } : {}),
      });
    } catch (e) {
      setError(friendly(e, DOOR, 'email'));
      return null;
    } finally {
      setBusy(false);
      human.reset();
    }
  };
  const outside = async (connection: string) => {
    setBusy(true);
    setError(null);
    try {
      const redirect = await beginOutside(options, connection, returnUrl(window.location), human.token, window.location.hash);
      window.location.assign(redirect);
    } catch (e) {
      setError(friendly(e, DOOR, 'outside'));
      setBusy(false);
      human.reset();
    }
  };
  const useToken = (token: string, space: string) =>
    void enter(async () => Session.fromToken(options, token, space));

  const signOut = async () => {
    if (!signedIn) return;
    try {
      await signedIn.session.signOut();
    } catch (e) {
      setError(friendly(e));
      return;
    }
    forget();
    await purgeAll(signedIn);
    setSignedIn(null);
    setTab('notes');
  };

  /** Every way in this deployment serves (`config.ts`), passkey first. */
  function signedOutScreen(onToken?: (token: string, space: string) => void) {
    if (resuming) return <p className="hint">Checking whether you are signed in...</p>;
    if (offer === null) return <p className="hint">Checking how you can sign in...</p>;
    if (settingUp !== null) return <SettingUp status={settingUp} />;
    return (
      <SignedOut
        methods={offer.methods}
        human={human}
        busy={busy}
        error={error}
        onSignUp={signUp}
        onSignIn={signIn}
        onEmailBegin={emailBegin}
        onEmailCode={emailCode}
        onGuest={guest}
        guestHere={guestSecret() !== null}
        onOutside={(c) => void outside(c)}
        onToken={onToken}
      />
    );
  }

  const onAccepted = (a: AcceptedShare) => {
    setAccepted((prev) => [...prev, { space: a.space, name: a.name }]);
    window.location.hash = '';
    setTab('shared');
  };

  return (
    <main>
      <header>
        <h1>Notes</h1>
        {signedIn && (
          <nav aria-label="Sections">
            <button aria-pressed={tab === 'notes'} onClick={() => setTab('notes')}>
              Mine
            </button>
            <button aria-pressed={tab === 'shared'} onClick={() => setTab('shared')}>
              Shared with me
            </button>
            <button aria-pressed={tab === 'search'} onClick={() => setTab('search')}>
              Search
            </button>
            <button onClick={() => void signOut()}>Sign out</button>
          </nav>
        )}
      </header>

      {secret !== null ? (
        // A share link is read with no account, so this comes before the
        // sign-in screen and never waits on a session.
        <LinkView secret={secret} endpoint={options} />
      ) : ticket !== null ? (
        <AcceptView
          ticket={ticket}
          session={signedIn?.session ?? null}
          baseUrl={DOOR}
          signedOut={signedOutScreen()}
          onAccepted={onAccepted}
          onCancel={() => {
            window.location.hash = '';
          }}
        />
      ) : signedIn === null && offline !== null ? (
        <>
          <p role="status" className="hint" data-testid="offline-state">
            You are offline. Showing the notes kept on this device. You can read them, but changes need the network.
            This page asks again when the network returns.{' '}
            <button onClick={() => void retryResume()}>Try again now</button>
          </p>
          <OrdoProvider client={offline.hooks} device={offline.device}>
            <FilesProvider>
              <NotesView />
            </FilesProvider>
          </OrdoProvider>
        </>
      ) : signedIn === null ? (
        signedOutScreen(useToken)
      ) : (
        // The signed-in part runs on the React building blocks: live reads,
        // and the device copy for the ops the descriptor says are syncable.
        <OrdoProvider client={signedIn.hooks} live device={signedIn.device} stepUp={signedIn.stepUp}>
          <AddPasskey
            session={signedIn.session}
            human={human}
            connections={offer?.methods.connections ?? []}
          />
          {tab === 'notes' ? (
            <FilesProvider>
              <NotesView />
            </FilesProvider>
          ) : tab === 'search' ? (
            <FilesProvider>
              <SearchView session={signedIn.session} accepted={accepted} devices={signedIn.shared} />
            </FilesProvider>
          ) : (
            <SharedView session={signedIn.session} accepted={accepted} devices={signedIn.shared} />
          )}
        </OrdoProvider>
      )}

      {signedIn && (
        <footer>
          <details>
            <summary>About this account</summary>
            <p className="hint">
              Your account id is <code>{signedIn.session.principal}</code>. You signed in with {wayIn(signedIn.session.via)}.
            </p>
            <p className="hint" data-testid="page-version">
              Page version: v2
            </p>
          </details>
          <EmailForReminders baseUrl={signedIn.session.endpoint.baseUrl} token={() => signedIn.session.token()} />
        </footer>
      )}
    </main>
  );
}

/** What a step of making a person's tenant is called on the page. */
const STEPS: Readonly<Record<string, string>> = {
  reserved: 'Getting started',
  targeted: 'Making your keys',
  custody: 'Making your database',
  database: 'Connecting your database',
  attached: 'Making you its owner',
  founded: 'Opening it',
  serving: 'Opening it',
};

/** Shown while a new person's tenant is made, between sign-up and the notes. */
function SettingUp({ status }: { readonly status: TenantStatus }) {
  const step = (status.step && STEPS[status.step]) || 'Getting started';
  return (
    <section aria-labelledby="setting-up">
      <h2 id="setting-up">Setting up your account</h2>
      <p role="status" aria-live="polite">
        {step}. This can take a little while the first time.
      </p>
    </section>
  );
}
