/**
 * `useOpForm(op)` — form state derived from `params[]`, and nothing rendered.
 *
 * The hook is a `useState` around `form.ts`. Everything that decides what a
 * field *is* lives there and is pure, which is why the field derivation is
 * tested against all 165 ops of the golden descriptor without React.
 */

import { useCallback, useMemo, useState } from 'react';
import type { ParamValue, Row } from '../client.js';
import {
  deriveForm,
  toParams,
  validateForm,
  type FormField,
  type FormIssue,
  type FormModel,
  type FormValues,
} from '../form.js';
import { useOrdo } from './context.js';

export interface UseOpForm {
  readonly model: FormModel;
  /** Every field, including the runtime-owned `rev`. */
  readonly fields: readonly FormField[];
  /** What the consumer draws — `model.fields` minus the hidden ones. */
  readonly visible: readonly FormField[];
  readonly values: FormValues;
  readonly setValue: (name: string, value: ParamValue | undefined) => void;
  readonly setValues: (next: FormValues) => void;
  readonly reset: () => void;
  /** Typed, per-param, using the door's own classes. */
  readonly issues: readonly FormIssue[];
  readonly issueFor: (name: string) => FormIssue | undefined;
  readonly valid: boolean;
  /** The bound params, ready for `useCommit().commit()`. */
  readonly params: Record<string, ParamValue>;
  /**
   * Candidate search for an `id_entity` param: run the entity's list op and
   * hand back rows. The consumer renders the picker; this is the data behind
   * it. Resolves to `[]` when the catalog grants no list op for the entity —
   * a blind reference, which `field.candidates.blind` announces up front.
   */
  readonly searchCandidates: (field: string, limit?: number) => Promise<readonly Row[]>;
}

export function useOpForm(opName: string, initial: FormValues = {}): UseOpForm {
  const client = useOrdo();
  const model = useMemo(() => deriveForm(client.iface, opName), [client, opName]);
  const [values, setValuesState] = useState<FormValues>(initial);

  const setValue = useCallback((name: string, value: ParamValue | undefined) => {
    setValuesState((prev) => ({ ...prev, [name]: value }));
  }, []);

  const reset = useCallback(() => setValuesState(initial), [initial]);

  const issues = useMemo(() => validateForm(model, values, client.iface), [model, values, client]);

  const searchCandidates = useCallback(
    async (fieldName: string, limit = 50): Promise<readonly Row[]> => {
      const field = model.fields.find((f) => f.name === fieldName);
      const listOp = field?.candidates?.listOps[0];
      if (!listOp) return [];
      // One page is the right default: a picker is a picker, and paging to
      // exhaustion behind a dropdown is how a runtime walks an entity by
      // accident. A consumer that wants more owns the loop.
      const res = await client.fetchPage<Row>(listOp);
      return res.rows.slice(0, limit);
    },
    [client, model],
  );

  return {
    model,
    fields: model.fields,
    visible: model.visible,
    values,
    setValue,
    setValues: setValuesState,
    reset,
    issues,
    issueFor: (name) => issues.find((i) => i.param === name),
    valid: issues.length === 0,
    params: toParams(model, values),
    searchCandidates,
  };
}
