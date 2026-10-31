import type { OrdoClient } from '@hominemai/ordo-sdk/core/client.js';
import type { JsonValue } from '@hominemai/ordo-sdk/core/json.js';
import { useState } from 'react';
import { parseAttributes } from '../format.js';
import type { ShopsId } from '../generated/ids.js';
import { addProduct, editProduct, restock, std$products$list, type StdProductsListRow } from '../generated/ops.js';
import { useAct, useLoad } from '../hooks.js';
import { Problem } from './Account.js';

/**
 * Every product, active or not (`std.products.list` is open to anyone who may
 * read, and `catalogue` shows only the active ones). Staff edit, restock and
 * add; a guard refuses anyone else.
 */
export function ProductsTab({ client, shop }: { client: OrdoClient; shop: ShopsId }) {
  const products = useLoad(() => std$products$list(client).rows(), [client]);
  return (
    <section aria-labelledby="products-title">
      <h2 id="products-title">Products</h2>
      <Problem text={products.error} />
      {products.loading && <p className="hint">Loading.</p>}
      <ul className="plain">
        {(products.data ?? []).map((p) => (
          // `rev` is part of the key so a saved row starts its form again.
          <ProductRow key={`${p.id}:${p.rev}:${p.stock}`} product={p} client={client} onChanged={products.reload} />
        ))}
      </ul>
      <AddProduct client={client} shop={shop} onAdded={products.reload} />
    </section>
  );
}

function ProductRow(props: { product: StdProductsListRow; client: OrdoClient; onChanged: () => void }) {
  const { product, client } = props;
  const [name, setName] = useState(product.name);
  const [category, setCategory] = useState(product.category);
  const [price, setPrice] = useState(product.price);
  const [active, setActive] = useState(product.active);
  const [attributes, setAttributes] = useState(JSON.stringify(product.attributes ?? {}));
  const [by, setBy] = useState('1');
  const act = useAct();
  const [local, setLocal] = useState<string | null>(null);
  const id = `p-${product.id}`;

  const save = async () => {
    const parsed = parseAttributes(attributes);
    if (!parsed.ok) {
      setLocal(parsed.error);
      return;
    }
    setLocal(null);
    const ok = await act.run(() =>
      editProduct(client, {
        id: product.id,
        rev: product.rev,
        name: name.trim(),
        category: category.trim(),
        price: price.trim(),
        active,
        attributes: parsed.value as JsonValue,
      }).commit(),
    );
    if (ok) props.onChanged();
  };

  const add = async () => {
    const n = Number(by);
    if (!Number.isInteger(n) || n === 0) {
      setLocal('Restock by a whole number, not 0.');
      return;
    }
    setLocal(null);
    const ok = await act.run(() => restock(client, { product: product.id, by: n }).commit());
    if (ok) props.onChanged();
  };

  return (
    <li className="card">
      <h3>
        {product.name} <span className="hint">{product.sku}</span>
      </h3>
      <div className="hint">
        {product.stock} in stock{product.active ? '' : ', not shown in the shop'}
      </div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <label htmlFor={`${id}-name`}>Name</label>
        <input id={`${id}-name`} value={name} onChange={(e) => setName(e.target.value)} />
        <label htmlFor={`${id}-cat`}>Category</label>
        <input id={`${id}-cat`} value={category} onChange={(e) => setCategory(e.target.value)} />
        <label htmlFor={`${id}-price`}>Price</label>
        <input id={`${id}-price`} inputMode="decimal" value={price} onChange={(e) => setPrice(e.target.value)} />
        <label htmlFor={`${id}-attrs`}>Attributes (JSON)</label>
        <input id={`${id}-attrs`} value={attributes} onChange={(e) => setAttributes(e.target.value)} />
        <label className="check">
          <input type="checkbox" checked={active} onChange={(e) => setActive(e.target.checked)} /> Shown in the shop
        </label>
        <div className="row">
          <button type="submit" className="primary" disabled={act.busy}>
            Save
          </button>
        </div>
      </form>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <label htmlFor={`${id}-by`}>Restock by</label>
        <input id={`${id}-by`} type="number" step={1} value={by} onChange={(e) => setBy(e.target.value)} />
        <div className="row">
          <button type="submit" disabled={act.busy}>
            Restock
          </button>
        </div>
      </form>
      <Problem text={local ?? act.error} />
    </li>
  );
}

function AddProduct(props: { client: OrdoClient; shop: ShopsId; onAdded: () => void }) {
  const [sku, setSku] = useState('');
  const [name, setName] = useState('');
  const [category, setCategory] = useState('');
  const [price, setPrice] = useState('');
  const [stock, setStock] = useState('0');
  const [attributes, setAttributes] = useState('{}');
  const act = useAct();
  const [local, setLocal] = useState<string | null>(null);

  const submit = async () => {
    const parsed = parseAttributes(attributes);
    if (!parsed.ok) {
      setLocal(parsed.error);
      return;
    }
    const n = Number(stock);
    if (!Number.isInteger(n) || n < 0) {
      setLocal('Stock must be a whole number, 0 or more.');
      return;
    }
    setLocal(null);
    const ok = await act.run(() =>
      addProduct(props.client, {
        shop: props.shop,
        sku: sku.trim(),
        name: name.trim(),
        category: category.trim(),
        price: price.trim(),
        stock: n,
        attributes: parsed.value as JsonValue,
      }).commit(),
    );
    if (ok) {
      setSku('');
      setName('');
      setCategory('');
      setPrice('');
      setStock('0');
      setAttributes('{}');
      props.onAdded();
    }
  };

  const ready = sku.trim() !== '' && name.trim() !== '' && category.trim() !== '' && price.trim() !== '';
  return (
    <form
      className="card"
      aria-labelledby="add-title"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <h3 id="add-title">Add a product</h3>
      <label htmlFor="add-sku">SKU</label>
      <input id="add-sku" value={sku} onChange={(e) => setSku(e.target.value)} />
      <label htmlFor="add-name">Name</label>
      <input id="add-name" value={name} onChange={(e) => setName(e.target.value)} />
      <label htmlFor="add-cat">Category</label>
      <input id="add-cat" value={category} onChange={(e) => setCategory(e.target.value)} />
      <label htmlFor="add-price">Price</label>
      <input id="add-price" inputMode="decimal" value={price} onChange={(e) => setPrice(e.target.value)} />
      <label htmlFor="add-stock">Stock</label>
      <input id="add-stock" type="number" min={0} step={1} value={stock} onChange={(e) => setStock(e.target.value)} />
      <label htmlFor="add-attrs">Attributes (JSON)</label>
      <input id="add-attrs" value={attributes} onChange={(e) => setAttributes(e.target.value)} />
      <div className="row">
        <button type="submit" className="primary" disabled={!ready || act.busy}>
          Add product
        </button>
      </div>
      <p className="hint">A SKU that exists is changed in place.</p>
      <Problem text={local ?? act.error} />
    </form>
  );
}
