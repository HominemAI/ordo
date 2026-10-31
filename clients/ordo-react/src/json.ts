/**
 * The JSON value type and its check. `@hominemai/ordo-sdk` owns them; this
 * package re-exports them (owner ruling, 2026-10-23: ordo-react imports
 * ordo-sdk's code and carries no copy).
 */
export { isJsonValue, type JsonValue } from '@hominemai/ordo-sdk/core/json.js';
