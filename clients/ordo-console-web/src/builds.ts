/**
 * The Tenant builds screen's calls: the console's `builds` routes, which
 * reach the deployment's `std.deployment.tenantBuilds`, `setTenantBuild`,
 * `joinTenantBuild`, `leaveTenantBuild` and `removeTenantBuild`
 * (docs/plan-versions.md 3.8, increment 16).
 *
 * A build pins named tenants to its version, apart from the active one.
 */

import { get, post } from './api';

export interface BuildMember {
  tenant: string;
  joinedBy: string;
  joinedAt: number;
}

export interface Build {
  name: string;
  version: string;
  /** Its version's order number, `#n`. */
  ordinal: number | null;
  reason: string;
  setBy: string;
  setAt: number;
  tenants: BuildMember[];
}

/** What set, join and leave answer: the build, its version and the tenants moved. */
export interface Moved {
  build: string;
  version: string;
  ordinal: number | null;
  tenants: string[];
}

export const listBuilds = (): Promise<{ builds: Build[] }> => get<{ builds: Build[] }>('builds');
export const setBuild = (name: string, version: string, reason: string): Promise<Moved> =>
  post<Moved>('builds/set', { name, version, reason });
export const joinBuild = (name: string, tenant: string): Promise<Moved> => post<Moved>('builds/join', { name, tenant });
export const leaveBuild = (tenant: string): Promise<Moved> => post<Moved>('builds/leave', { tenant });
export const removeBuild = (name: string, reason: string) =>
  post<{ build: string; version: string; removed: boolean }>('builds/remove', { name, reason });
