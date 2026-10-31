/**
 * `useRow(id)` — typed-id action discovery for any id, from the id alone.
 *
 * There is no fetch here and there is deliberately none: discovery is a
 * property of the *descriptor*, and a descriptor describes shape, never rows
 * (`ui-gen.md` §1). A row menu can be drawn before, or without, ever reading
 * the row it belongs to.
 */

import { useMemo } from 'react';
import { actionMenu, discover, type ActionMenu, type RowKnowledge } from '../actions.js';
import { useInterface } from './context.js';

export interface UseRow {
  readonly row: RowKnowledge;
  readonly menu: ActionMenu;
  /** Params for calling one of the discovered ops against this row. */
  readonly bind: (op: string) => Record<string, string> | null;
}

export function useRow(id: string | null | undefined): UseRow {
  const iface = useInterface();
  return useMemo(() => {
    const row = discover(iface, id ?? '');
    const menu = actionMenu(row);
    const bind = (op: string): Record<string, string> | null => {
      if (!row.known) return null;
      const found = [...row.actions, ...row.detail].find((a) => a.op === op);
      return found ? found.bind(row.id) : null;
    };
    return { row, menu, bind };
  }, [iface, id]);
}
