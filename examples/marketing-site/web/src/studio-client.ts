import { DescriptorClient, parseInterface, withPlatformOps, OrdoRefusal } from '@hominemai/ordo-react';
import { doorRequest, readBody, refusalOf } from '@hominemai/ordo-sdk/core/door.js';
import type { Params } from '@hominemai/ordo-sdk/core/client.js';
import { DOOR } from './config.js';
import type { Session } from './session.js';

export async function studioClient(session: Session): Promise<DescriptorClient> {
  const response = await doorRequest({ baseUrl: DOOR, timeoutOption: "callTimeoutMs" }, `${DOOR}/interface`, {
    headers: { Authorization: `Bearer ${await session.token()}`, 'X-Ordo-Space': 'system' },
  });
  if (!response.ok) throw await refusalOf(response, 'interface');
  const body = await readBody(response);
  return new DescriptorClient({ baseUrl: DOOR, space: 'system', token: () => session.token(),
    iface: withPlatformOps(parseInterface(body), 'std.space.invite', 'std.space.invitations',
      'std.space.revokeInvitation', 'std.space.setRole', 'std.space.suspend', 'std.space.reinstate'),
  });
}

export async function commit(client: DescriptorClient, session: Session, name: string, params: Params) {
  try { return await client.commit(name, { params }); }
  catch (error) {
    if (!(error instanceof OrdoRefusal) || error.refusal.code !== 'presence_required') throw error;
    await session.stepUp();
    return client.commit(name, { params });
  }
}

export function message(error: unknown): string {
  if (error instanceof OrdoRefusal) {
    if (error.refusal.code === 'cas_miss') return 'Someone changed this record. Close the editor, refresh, and review the latest version before saving again.';
    if (error.refusal.code === 'access_denied' || error.refusal.code === 'access_guard') return 'Your account does not have access to this action. Ask a studio owner for access.';
  }
  return error instanceof Error ? error.message : 'The action could not be completed. Please try again.';
}
