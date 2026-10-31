import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import { useState } from 'react';
import { shipped, stamp, when } from '../format.js';
import { allOrders, markShipped, type AllOrdersRow } from '../generated/ops.js';
import { useAct, useLoad } from '../hooks.js';
import { Problem } from './Account.js';

type Filter = 'all' | 'waiting' | 'shipped';

/** Every order the caller may read, with a way to mark one shipped. */
export function OrdersTab({ client }: { client: OrdoClient }) {
  const orders = useLoad(() => allOrders(client).rows(), [client]);
  const [filter, setFilter] = useState<Filter>('waiting');
  const rows = (orders.data ?? []).filter((o) => filter === 'all' || (filter === 'shipped') === shipped(o));

  return (
    <section aria-labelledby="orders-title">
      <h2 id="orders-title">Orders</h2>
      <div className="row">
        <label htmlFor="order-filter">Show</label>
        <select id="order-filter" value={filter} onChange={(e) => setFilter(e.target.value as Filter)}>
          <option value="waiting">Waiting to ship</option>
          <option value="shipped">Shipped</option>
          <option value="all">All</option>
        </select>
        <button type="button" onClick={orders.reload}>
          Reload
        </button>
      </div>
      <Problem text={orders.error} />
      {orders.loading && <p className="hint">Loading.</p>}
      {!orders.loading && rows.length === 0 && <p className="hint">No orders here.</p>}
      <ul className="plain">
        {rows.map((o) => (
          <OrderRow key={o.id} order={o} client={client} onShipped={orders.reload} />
        ))}
      </ul>
    </section>
  );
}

function OrderRow(props: { order: AllOrdersRow; client: OrdoClient; onShipped: () => void }) {
  const { order, client } = props;
  const [carrier, setCarrier] = useState('');
  const act = useAct();
  const id = `ship-${order.id}`;

  const ship = async () => {
    const ok = await act.run(() =>
      markShipped(client, {
        order: order.id,
        product: order.product,
        quantity: order.quantity,
        shipped: stamp(),
        carrier: carrier.trim(),
      }).commit(),
    );
    if (ok) props.onShipped();
  };

  return (
    <li className="card">
      <strong>
        {order.quantity} x {order.productName}
      </strong>{' '}
      <span>total {order.total}</span>
      <div className="hint">
        {order.customer}, placed {when(order.placed)}
      </div>
      {order.note !== '' && order.note !== null && <div className="hint">Note: {order.note}</div>}
      {shipped(order) ? (
        <div>
          Shipped {when(order.shipped)}
          {order.carrier ? ` with ${order.carrier}` : ''}
        </div>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void ship();
          }}
        >
          <label htmlFor={id}>Carrier</label>
          <input id={id} value={carrier} onChange={(e) => setCarrier(e.target.value)} disabled={act.busy} />
          <div className="row">
            <button type="submit" className="primary" disabled={act.busy}>
              Mark shipped
            </button>
          </div>
          <Problem text={act.error} />
        </form>
      )}
    </li>
  );
}
