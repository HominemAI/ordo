/**
 * A backend function call, `POST /fn/<name>` (`docs/howto-functions.md`,
 * "Calling a function").
 *
 * It rides the same request helper as an op (`core/door.ts`), so it has the
 * same deadline, the same naming of a call that got no answer and the same
 * single error shape: a refusal is an {@link OrdoRefusal}. The class is
 * `access_denied`, `space_paused`, `function_error`, `function_timeout`,
 * `function_oom`, `function_busy` or `egress_refused`, and the message says
 * what to change.
 *
 * Both clients (`OrdoClient` here, `DescriptorClient` in ordo-react) hand
 * this their own transport, so the wire shape is written once.
 */

import type { OnRetry } from './busy.js';
import { readBody, refusalOf } from './door.js';
import { isJsonValue } from './json.js';
import type { JsonValue } from './json.js';
import { OrdoLocalRefusal } from './refusal.js';

/** What one function call takes. */
export interface FunctionCallOptions {
  /**
   * Supply one to make a retry replay the first answer instead of running
   * the function again. None is sent when this is left out.
   */
  readonly idempotencyKey?: string | undefined;
  readonly signal?: AbortSignal | undefined;
  /** Told before each wait on a busy door, as for an op. */
  readonly onRetry?: OnRetry | undefined;
}

/** A function's answer: the JSON it returned, as the door sent it. */
export interface FunctionResponse<R = JsonValue> {
  readonly result: R;
  /** True when the door replayed the first answer for this idempotency key. */
  readonly replayed: boolean;
}

/** What the clients give the shared call: their own token, space and transport. */
export interface FunctionSite {
  readonly baseUrl: string;
  readonly space: string;
  readonly token: () => Promise<string | undefined>;
  readonly transport: (url: string, init: RequestInit, onRetry?: OnRetry) => Promise<Response>;
}

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Checked before the wire. */
function checked(name: string, params: Readonly<Record<string, unknown>>): void {
  if (!NAME.test(name)) {
    throw new OrdoLocalRefusal({
      class: 'access_denied',
      message: `${JSON.stringify(name)} is not a function name`,
      op: name,
      param: null,
    });
  }
  if (!isJsonValue(params)) {
    throw new OrdoLocalRefusal({
      class: 'invalid_param',
      message: `${name}: the parameters must be plain JSON (no undefined, Date, Map or non-finite number)`,
      op: name,
      param: null,
    });
  }
}

/** Call function `name` with `params`. */
export async function callFunction<R = JsonValue>(
  site: FunctionSite,
  name: string,
  params: Readonly<Record<string, unknown>> = {},
  options: FunctionCallOptions = {},
): Promise<FunctionResponse<R>> {
  checked(name, params);
  const headers: Record<string, string> = {
    'X-Ordo-Space': site.space,
    Accept: 'application/json',
    'Content-Type': 'application/json',
  };
  const token = await site.token();
  if (token) headers['Authorization'] = `Bearer ${token}`;
  if (options.idempotencyKey) headers['Idempotency-Key'] = options.idempotencyKey;

  const init: RequestInit = { method: 'POST', headers, body: JSON.stringify(params) };
  if (options.signal) init.signal = options.signal;

  const response = await site.transport(`${site.baseUrl}/fn/${encodeURIComponent(name)}`, init, options.onRetry);
  if (!response.ok) throw await refusalOf(response, name);
  const body = (await readBody(response)) as { result?: R } | null;
  return {
    result: (body !== null && typeof body === 'object' && 'result' in body ? body.result : null) as R,
    replayed: response.headers.get('X-Ordo-Idempotent-Replay') === 'true',
  };
}
