import { DescriptorClient, parseInterface, withPlatformOps, OrdoRefusal } from '@hominemai/ordo-react';
import { doorRequest, readBody, refusalOf } from '@hominemai/ordo-sdk/core/door.js';
import type { Params, Row } from '@hominemai/ordo-sdk/core/client.js';
import { DOOR } from './config.js';
import type { Session } from './session.js';

export async function studioClient(session: Session): Promise<DescriptorClient> {
  const response = await doorRequest({ baseUrl: DOOR, timeoutOption: 'callTimeoutMs' }, `${DOOR}/interface`, {
    headers: { Authorization: `Bearer ${await session.token()}`, 'X-Ordo-Space': session.space },
  });
  if (!response.ok) throw await refusalOf(response, 'interface');
  const body = await readBody(response);
  return new DescriptorClient({
    baseUrl: DOOR,
    space: session.space,
    token: () => session.token(),
    iface: withPlatformOps(
      parseInterface(body),
      'std.space.invite',
      'std.space.invitations',
      'std.space.revokeInvitation',
      'std.space.setRole',
      'std.space.suspend',
      'std.space.reinstate',
      'std.space.leave',
    ),
  });
}

export async function commit(client: DescriptorClient, session: Session, name: string, params: Params) {
  try {
    return await client.commit(name, { params });
  } catch (error) {
    if (!(error instanceof OrdoRefusal) || error.refusal.code !== 'presence_required') throw error;
    await session.stepUp();
    return client.commit(name, { params });
  }
}

export function message(error: unknown): string {
  if (error instanceof OrdoRefusal) {
    if (error.refusal.code === 'cas_miss')
      return 'Someone changed this record. Close the editor, refresh, and review the latest version before saving again.';
    if (error.refusal.code === 'access_denied' || error.refusal.code === 'access_guard')
      return 'Your account does not have access to this action. Ask a studio owner for access.';
  }
  return error instanceof Error ? error.message : 'The action could not be completed. Please try again.';
}

export interface WorkspaceChoice { space: string; name: string | null; role: string | null; status: string }
export async function workspaces(session: Session): Promise<WorkspaceChoice[]> {
  const response = await doorRequest({ baseUrl: DOOR, timeoutOption: 'callTimeoutMs' }, `${DOOR}/op/std.person.tenants`, {
    headers: { Authorization: `Bearer ${await session.token()}`, 'X-Ordo-Space': session.space },
  });
  if (!response.ok) throw await refusalOf(response, 'std.person.tenants');
  const body = await readBody(response) as { rows: WorkspaceChoice[] };
  return body.rows;
}

/** Only the tenant portability operations used by the data panel. */
export async function portability(session: Session, action: 'export' | 'import', params: Params) {
  const name = action === 'export' ? 'std.space.export' : 'std.space.import';
  const idempotency = crypto.randomUUID();
  async function send(): Promise<Row[]> {
    const response = await doorRequest({ baseUrl: DOOR, timeoutOption: 'callTimeoutMs' }, `${DOOR}/op/${name}`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${await session.token()}`, 'X-Ordo-Space': session.space,
        'Content-Type': 'application/json', 'Idempotency-Key': idempotency },
      body: JSON.stringify(params),
    });
    if (!response.ok) throw await refusalOf(response, name);
    return (await readBody(response) as { rows: Row[] }).rows;
  }
  try { return await send(); }
  catch (error) {
    if (!(error instanceof OrdoRefusal) || error.refusal.code !== 'presence_required') throw error;
    await session.stepUp();
    return send();
  }
}
