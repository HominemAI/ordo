import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, type APIRequestContext, type APIResponse } from '@playwright/test';
export const state = process.env['E2E_STATE'] ?? fileURLToPath(new URL('../.stack', import.meta.url));
export const stack = (): { door: string; doorB: string; page: string; ordo: string; psql: string; program: string; pg: { host: string; port: string; user: string; db: string } } => JSON.parse(readFileSync(`${state}/stack.json`, 'utf8'));
export async function owner(request: APIRequestContext, account = 'owner') {
  const response = await request.post(`${stack().door}/auth/complete`, {
    headers: { Origin: stack().page },
    data: { method: 'guest', kind: 'resume', secret: readFileSync(`${state}/${account}-secret`, 'utf8').trim() },
  });
  expect(response.status(), await response.text()).toBe(200);
  return { Origin: stack().page, Authorization: `Bearer ${(await response.json()).token}`, 'X-Ordo-Space': 'system' };
}
export async function rows(response: APIResponse): Promise<Record<string, any>[]> {
  expect(response.status(), await response.text()).toBe(200);
  return (await response.json()).rows;
}
export async function refused(response: APIResponse, status: number, code: string) {
  expect(response.status(), await response.text()).toBe(status);
  expect((await response.json()).error.code).toBe(code);
}
export const normalize = (text: string) => text.replace(/("(?:request_id|requestId)"\s*:\s*)"[^"]*"/g, '$1"<request>"');
export async function sameRefusal(a: APIResponse, b: APIResponse) {
  await refused(a, 403, 'access_denied');
  await refused(b, 403, 'access_denied');
  expect.soft(normalize(await a.text())).toBe(normalize(await b.text()));
}
