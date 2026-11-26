import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect } from '@playwright/test';
import { stack } from './support.js';
const exec = promisify(execFile);

/** New psql connections avoid a transaction's cached statistics snapshot. */
async function sample(table: string): Promise<number> {
  if (!/^[a-z][a-z0-9_]*$/.test(table)) throw new Error('Invalid entity table name');
  const { pg, psql } = stack();
  const { stdout } = await exec(psql, ['-X', '-A', '-t', '-v', 'ON_ERROR_STOP=1',
    '-h', pg.host, '-p', pg.port, '-U', pg.user, '-d', pg.db, '-c',
    `SELECT count(*), coalesce(sum(seq_scan + coalesce(idx_scan, 0)), 0) FROM pg_stat_user_tables WHERE relname = '${table}'`], {
    env: { ...process.env, PGPASSWORD: process.env['ORDO_PG_PASSWORD'] ?? process.env['PGPASSWORD'] ?? '' },
    timeout: 10_000,
  });
  const [tables, scans] = stdout.trim().split('|').map(Number);
  expect(tables, `No pg_stat_user_tables entry for ${table}`).toBeGreaterThan(0);
  expect(Number.isSafeInteger(scans), `Invalid scan count: ${stdout}`).toBe(true);
  return scans;
}

/** Allow backend statistics to flush: require equal samples 11s apart. */
export async function tableReads(table: string): Promise<number> {
  let previous: number | undefined;
  let settled = 0;
  await expect.poll(async () => {
    settled = await sample(table);
    const stable = previous !== undefined && previous === settled;
    previous = settled;
    return stable;
  }, { intervals: [11_000], timeout: 75_000, message: `${table} scan counters settle` }).toBe(true);
  return settled;
}
