/**
 * The adapter for the stack under test, picked by BENCH_STACK, pointed at the
 * running stack by the stack.json its up.sh wrote (BENCH_STACK_JSON).
 */

import { readFileSync } from 'node:fs';
import { ordo } from './ordo.js';
import { supabase } from './supabase.js';
import type { Adapter } from './types.js';

export interface StackInfo {
  readonly stack: string;
  readonly page: string;
  readonly mail: string;
  /** Ordo: the door. */
  readonly door?: string;
  /** Supabase: the API and its public key. */
  readonly url?: string;
  readonly anonKey?: string;
}

let chosen: Adapter | null = null;

export function stackInfo(): StackInfo {
  const path = process.env['BENCH_STACK_JSON'];
  if (!path) throw new Error('BENCH_STACK_JSON is not set: point it at the stack.json that up.sh wrote');
  return JSON.parse(readFileSync(path, 'utf8')) as StackInfo;
}

export function adapter(): Adapter {
  if (chosen) return chosen;
  const info = stackInfo();
  const name = process.env['BENCH_STACK'] ?? info.stack;
  if (name === 'ordo') chosen = ordo(info);
  else if (name === 'supabase') chosen = supabase(info);
  else throw new Error(`BENCH_STACK=${name}: the suite has adapters for ordo and supabase`);
  return chosen;
}
