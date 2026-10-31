import { useMemo, useState, type FormEvent } from 'react';
import { AccountStrip, Problem, SignedOut, Starting } from './components/Account.js';
import { byCategory, colorOf, shipped, stamp, tagsOf, when } from './format.js';
import { catalogue, me, myOrders, placeOrder, productsWhere, type CatalogueRow } from './generated/ops.js';
import type { ProductsId } from './generated/ids.js';
import { useAct, useLoad } from './hooks.js';
import type { Session } from './session.js';
import { useSubdomain } from './subdomain.js';

export const SHOP_KEY = 'ordo-shopfront.shop';

/** The shop: customers browse the catalogue, order, and see their orders. */
export function ShopPage() {
  const sub = useSubdomain(SHOP_KEY);
  if (sub.starting) return <Starting />;
  if (sub.session === null) {
    return <SignedOut title="Shop" text="Sign in to order." sub={sub} />;
  }
  return <Shop session={sub.session} onSignOut={sub.forget} notice={sub.error} />;
}

function Shop(props: { session: Session; onSignOut: () => void; notice: string | null }) {
  const { session } = props;
  const client = useMemo(() => session.client(), [session]);
  const who = useLoad(() => me(client).fetch().then((p) => p.rows[0] ?? null), [client]);
  const orders = useLoad(() => myOrders(client).rows(), [client]);
  const [color, setColor] = useState('');
  const all = useLoad(() => catalogue(client).rows(), [client]);
  const shown = useLoad(
    () => productsWhere(client, { color }).rows(),
    [client, color],
    color !== '',
  );

  const products: readonly CatalogueRow[] = (color === '' ? all.data : shown.data) ?? [];
  const colors = useMemo(
    () => [...new Set((all.data ?? []).map((p) => colorOf(p.attributes)).filter((c): c is string => c !== null))].sort(),
    [all.data],
  );

  return (
    <main>
      <header>
        <h1>Shop</h1>
        <AccountStrip name={who.data?.displayName ?? null} memberId={who.data?.id ?? null} onSignOut={props.onSignOut} />
      </header>
      <Problem text={props.notice} />
      <Problem text={who.error} />

      <section aria-labelledby="catalogue-title">
        <h2 id="catalogue-title">Catalogue</h2>
        <div className="row">
          <label htmlFor="color">Colour</label>
          <select id="color" value={color} onChange={(e) => setColor(e.target.value)}>
            <option value="">All colours</option>
            {colors.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </div>
        <Problem text={color === '' ? all.error : shown.error} />
        {(color === '' ? all.loading : shown.loading) && <p className="hint">Loading.</p>}
        {!all.loading && products.length === 0 && <p className="hint">Nothing to show.</p>}
        {byCategory(products).map(([category, rows]) => (
          <div key={category}>
            <h3>{category}</h3>
            <ul className="grid">
              {rows.map((p) => (
                <ProductCard key={p.id} product={p} client={client} onOrdered={orders.reload} />
              ))}
            </ul>
          </div>
        ))}
      </section>

      <section aria-labelledby="orders-title">
        <h2 id="orders-title">My orders</h2>
        <Problem text={orders.error} />
        {orders.data?.length === 0 && <p className="hint">No orders yet.</p>}
        <ul className="plain">
          {(orders.data ?? []).map((o) => (
            <li key={o.id} className="card">
              <strong>
                {o.quantity} x {o.productName}
              </strong>{' '}
              <span>total {o.total}</span>
              <div className="hint">Placed {when(o.placed)}</div>
              {o.note !== '' && o.note !== null && <div className="hint">Note: {o.note}</div>}
              <div>
                {shipped(o)
                  ? `Shipped ${when(o.shipped)}${o.carrier ? ` with ${o.carrier}` : ''}`
                  : 'Waiting to ship'}
              </div>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}

function ProductCard(props: {
  product: CatalogueRow;
  client: ReturnType<Session['client']>;
  onOrdered: () => void;
}) {
  const { product, client } = props;
  const [quantity, setQuantity] = useState('1');
  const [note, setNote] = useState('');
  const act = useAct();
  const [done, setDone] = useState(false);
  const out = product.stock <= 0;
  const qty = Number(quantity);
  const valid = Number.isInteger(qty) && qty >= 1;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setDone(false);
    const ok = await act.run(() =>
      placeOrder(client, { product: product.id as ProductsId, quantity: qty, placed: stamp(), note }).commit(),
    );
    if (ok) {
      setDone(true);
      setNote('');
      setQuantity('1');
      props.onOrdered();
    }
  };

  const id = `order-${product.id}`;
  return (
    <li className="card">
      <h4>{product.name}</h4>
      <div className="price">{product.price}</div>
      <div className={out ? 'error' : 'hint'}>{out ? 'Out of stock' : `${product.stock} in stock`}</div>
      <div className="tags">
        {tagsOf(product.attributes).map(([k, v]) => (
          <span key={k} className="tag">
            {k}: {v}
          </span>
        ))}
      </div>
      <form onSubmit={(e) => void submit(e)}>
        <label htmlFor={`${id}-qty`}>Quantity</label>
        <input
          id={`${id}-qty`}
          type="number"
          min={1}
          step={1}
          value={quantity}
          onChange={(e) => setQuantity(e.target.value)}
          disabled={out || act.busy}
        />
        <label htmlFor={`${id}-note`}>Note (optional)</label>
        <input id={`${id}-note`} value={note} onChange={(e) => setNote(e.target.value)} disabled={out || act.busy} />
        <div className="row">
          <button type="submit" className="primary" disabled={out || !valid || act.busy}>
            Order
          </button>
        </div>
        {done && <p className="hint">Ordered.</p>}
        <Problem text={act.error} />
      </form>
    </li>
  );
}
