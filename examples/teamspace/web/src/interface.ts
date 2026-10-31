/**
 * The descriptor the React hooks read at run time: `interface.json`, written
 * by `ordo interface` (`npm run gen`) and committed beside the generated client.
 *
 * The invitation and role ops are the platform's, not the program's, so the
 * descriptor does not carry them. `@hominemai/ordo-react`'s platform table
 * covers the sharing ops only, so this file describes the four this page
 * needs (`std.space.invite`, `invitations`, `revokeInvitation`, `setRole`) the
 * same way the table does, from one of its entries. Marked `platform`, a
 * refused write asks the provider's `stepUp` for a fresh passkey touch and
 * tries once more.
 */

import {
  OrdoInterface,
  PLATFORM_OPS,
  parseInterface,
  type OpDescriptor,
  type OrdoType,
  type ParamDescriptor,
} from '@hominemai/ordo-react';
import descriptor from './interface.json';

type P = readonly [name: string, type: OrdoType, required: boolean];

/** A platform op shaped like `like`, with its own name, parameters and columns. */
function platformOpLike(like: OpDescriptor, op: string, params: readonly P[], outputs: readonly string[]): OpDescriptor {
  const template: ParamDescriptor | undefined = PLATFORM_OPS['std.space.share'].params[0];
  if (!template) throw new Error('the platform table changed: no parameter to copy');
  return {
    ...like,
    op,
    params: params.map(([name, type, required]) => ({ ...template, name, type, required })),
    result: {
      ...like.result,
      outputs: outputs.map((name) => ({ name, type: null, sealed: false, refinable: false, id_entity: null, align: 'left' as const })),
    },
  };
}

const TEXT: OrdoType = 'text';
const DURATION: OrdoType = 'duration';
const write = PLATFORM_OPS['std.space.share'];
const read = PLATFORM_OPS['std.space.shares'];

const EXTRA: readonly OpDescriptor[] = [
  platformOpLike(write, 'std.space.invite', [['role', TEXT, false], ['expires', DURATION, false], ['label', TEXT, false]], [
    'ticket',
    'expiresAt',
    'role',
    'outcome',
  ]),
  platformOpLike(read, 'std.space.invitations', [], [
    'id',
    'role',
    'for',
    'label',
    'issuedBy',
    'issuedAt',
    'expiresAt',
    'status',
    'declinedAt',
  ]),
  platformOpLike(write, 'std.space.revokeInvitation', [['id', TEXT, true]], ['id', 'outcome']),
  platformOpLike(write, 'std.space.setRole', [['member', TEXT, true], ['role', TEXT, true]], [
    'member',
    'principal',
    'role',
    'status',
    'outcome',
    'door',
  ]),
];

const base = parseInterface(descriptor);
export const INTERFACE = new OrdoInterface({ ...base.doc, ops: [...base.doc.ops, ...EXTRA] });
