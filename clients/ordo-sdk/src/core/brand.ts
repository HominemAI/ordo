/**
 * Branded ids.
 *
 * Every id in Ordo is spelled `id` by the type system — that is why
 * `params[].id_entity` had to be added to the descriptor at all, and why
 * `ui-gen.md` §2's action discovery exists. The brand puts the missing fact
 * back where a caller can use it: passing a `CategoriesId` where a
 * `VendorsId` is wanted is a compile error in the caller's own language.
 *
 * The brand is a phantom property. It costs nothing at run time, and a value
 * crossing the wire is the plain `prefix_body` string it always was — the
 * cast below is the only place the two views meet.
 */

declare const brand: unique symbol;

export type Branded<T, B extends string> = T & { readonly [brand]: B };

/**
 * Assert that a string is an id of a particular entity.
 *
 * There is no validation here and there should not be: the typed-id prefix
 * is the *deployment's* fact and the door is the authority on it. A client
 * that pre-validated would be a second answer, and it would be the one that
 * went stale after a `key:` override.
 */
export function asId<B extends string>(value: string): Branded<string, B> {
  return value as Branded<string, B>;
}
