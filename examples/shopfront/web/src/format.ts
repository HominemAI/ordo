/**
 * What the pages show and send, with no React and no network.
 */

/** What `datetime` parameters look like on the wire: seconds, and a `Z`. */
export function stamp(now: Date = new Date()): string {
  return `${now.toISOString().slice(0, 19)}Z`;
}

/** A stamp as a short date and time for the reader's locale. */
export function when(value: unknown): string {
  if (typeof value !== 'string' || value === '') return '';
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? value : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** The product's attributes as `[name, value]` pairs, for tags. Anything but an object has none. */
export function tagsOf(attributes: unknown): Array<[string, string]> {
  if (attributes === null || typeof attributes !== 'object' || Array.isArray(attributes)) return [];
  return Object.entries(attributes as Record<string, unknown>).map(([k, v]) => [
    k,
    typeof v === 'string' ? v : JSON.stringify(v),
  ]);
}

/** The colour of a product, if its attributes name one in text. */
export function colorOf(attributes: unknown): string | null {
  if (attributes === null || typeof attributes !== 'object' || Array.isArray(attributes)) return null;
  const c = (attributes as Record<string, unknown>)['color'];
  return typeof c === 'string' && c !== '' ? c : null;
}

/** Group rows by category, categories in the order they first appear. */
export function byCategory<T extends { readonly category: string }>(rows: readonly T[]): Array<[string, T[]]> {
  const groups = new Map<string, T[]>();
  for (const row of rows) {
    const list = groups.get(row.category);
    if (list) list.push(row);
    else groups.set(row.category, [row]);
  }
  return [...groups];
}

/** Attributes typed as JSON: an object, or an error in words. */
export function parseAttributes(text: string): { ok: true; value: Record<string, unknown> } | { ok: false; error: string } {
  const source = text.trim() === '' ? '{}' : text;
  try {
    const value: unknown = JSON.parse(source);
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      return { ok: false, error: 'Attributes must be a JSON object, like {"color": "red"}.' };
    }
    return { ok: true, value: value as Record<string, unknown> };
  } catch {
    return { ok: false, error: 'Attributes are not valid JSON.' };
  }
}

/** Whether a row from `myOrders` or `allOrders` has shipped. */
export function shipped(row: { readonly shipped?: unknown }): boolean {
  return typeof row.shipped === 'string' && row.shipped !== '';
}
