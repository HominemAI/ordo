import { useMemo, useState } from 'react';
import { AccountStrip, Problem, SignedOut, Starting } from './components/Account.js';
import { OrdersTab } from './components/OrdersTab.js';
import { ProductsTab } from './components/ProductsTab.js';
import { StaffTab } from './components/StaffTab.js';
import { me, myShops, staffOf } from './generated/ops.js';
import type { ShopsId } from './generated/ids.js';
import { useLoad } from './hooks.js';
import type { Session } from './session.js';
import { useSubdomain } from './subdomain.js';

export const ADMIN_KEY = 'ordo-shopfront.admin';

type Tab = 'orders' | 'products' | 'staff';
const TABS: ReadonlyArray<readonly [Tab, string]> = [
  ['orders', 'Orders'],
  ['products', 'Products'],
  ['staff', 'Staff'],
];

/** The staff console. */
export function AdminPage() {
  const sub = useSubdomain(ADMIN_KEY);
  if (sub.starting) return <Starting />;
  if (sub.session === null) {
    return <SignedOut title="Staff console" text="Sign in with your staff account." sub={sub} />;
  }
  return <Console session={sub.session} onSignOut={sub.forget} notice={sub.error} />;
}

function Console(props: { session: Session; onSignOut: () => void; notice: string | null }) {
  const client = useMemo(() => props.session.client(), [props.session]);
  const who = useLoad(() => me(client).fetch().then((p) => p.rows[0] ?? null), [client]);
  const shops = useLoad(() => myShops(client).rows(), [client]);
  const [tab, setTab] = useState<Tab>('orders');
  const shop = shops.data?.[0];
  const staff = useLoad(
    () => staffOf(client, { shop: (shop?.id ?? '') as ShopsId }).rows(),
    [client, shop?.id],
    shop !== undefined,
  );
  const level = staff.data?.find((s) => s.member === who.data?.id)?.level ?? null;

  const strip = (
    <header>
      <h1>Staff console</h1>
      <AccountStrip name={who.data?.displayName ?? null} memberId={who.data?.id ?? null} onSignOut={props.onSignOut} />
    </header>
  );

  if (shops.loading || who.loading) {
    return (
      <main>
        {strip}
        <Problem text={props.notice} />
        <Problem text={shops.error ?? who.error} />
        {shops.error === null && <p className="hint">Loading.</p>}
      </main>
    );
  }

  if (shop === undefined) {
    return (
      <main>
        {strip}
        <Problem text={props.notice} />
        <Problem text={shops.error ?? who.error} />
        {shops.error === null && (
          <section className="card">
            <p>This console is for the shop's staff.</p>
            {who.data && (
              <p>
                Your member id is <code>{who.data.id}</code>. An admin can add you.
              </p>
            )}
          </section>
        )}
      </main>
    );
  }

  const shopId = shop.id;
  return (
    <main>
      {strip}
      <Problem text={props.notice} />
      <p className="hint">
        {shop.name}
        {level !== null && ` (${level})`}
      </p>
      <nav aria-label="Console sections">
        {TABS.map(([name, label]) => (
          <button
            key={name}
            type="button"
            className={tab === name ? 'tab on' : 'tab'}
            aria-current={tab === name ? 'page' : undefined}
            onClick={() => setTab(name)}
          >
            {label}
          </button>
        ))}
      </nav>
      {tab === 'orders' && <OrdersTab client={client} />}
      {tab === 'products' && <ProductsTab client={client} shop={shopId} />}
      {tab === 'staff' && (
        <StaffTab
          client={client}
          shop={shopId}
          rows={staff.data ?? []}
          error={staff.error}
          isAdmin={level === 'admin'}
          self={who.data?.id ?? null}
          reload={staff.reload}
        />
      )}
    </main>
  );
}
