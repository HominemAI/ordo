//! 25bf — the field-scope manifest says who reads a field: the op, or only a
//! guard.
//!
//! The rule: a field the op's own pipeline mentions (projection, condition,
//! sort, group, a ref hop in any of them) is read by the op. Only an entity
//! that nothing but a derived guard examines carries the guard-only marker.
//! The consent ceremony discloses from this, so marking a returned field
//! "read by a guard, not by the op" told a person the op reads less than it
//! does.

use ordo_plan::artifact::{op_scope, render_scope_entry, EntityScope};
use ordo_schema::build;
use ordo_sema::Registry;
use ordo_syntax::parse_program;

const SRC: &str = r#"
entity people { displayName: text }
entity products { name: text }
entity orders {
  buyer   -> people
  product -> products
  meta: text
  access read  buyer == $caller
  access write buyer == $caller
}
entity docs {
  body: text
  access read  seats(member == $caller)
  access write seats(member == $caller)
}
entity seats {
  doc    -> docs
  member -> people
}

op orderSheet = orders[meta, who: buyer.displayName]
op byProduct($n: text) = orders(product.name == $n)[meta]
op sortedByProduct = orders[meta] sort product.name
op docBodies = docs[body]
op setMeta($n: text) = set orders(product.name == $n) { meta: "x" }
"#;

fn scope(op: &str) -> Vec<EntityScope> {
    let program = parse_program(SRC).expect("parses");
    let schema = build(&program).expect("schema");
    let registry = Registry::from_program(&program);
    op_scope(&schema, &registry, op)
}

fn entry<'a>(s: &'a [EntityScope], entity: &str) -> &'a EntityScope {
    s.iter()
        .find(|e| e.entity == entity)
        .unwrap_or_else(|| panic!("`{entity}` is in the manifest: {s:?}"))
}

#[test]
fn a_projection_through_a_ref_hop_is_read_by_the_op() {
    let s = scope("orderSheet");
    let people = entry(&s, "people");
    assert!(people.read.contains("displayName"), "{people:?}");
    assert_eq!(people.via, "body", "{people:?}");
    assert!(
        !render_scope_entry(people).contains("guard"),
        "{}",
        render_scope_entry(people)
    );
}

#[test]
fn a_condition_through_a_ref_hop_is_read_by_the_op() {
    let s = scope("byProduct");
    let products = entry(&s, "products");
    assert!(products.read.contains("name"), "{products:?}");
    assert_eq!(products.via, "body", "{products:?}");
    assert_eq!(render_scope_entry(products), "products.{name}");
}

#[test]
fn a_sort_through_a_ref_hop_is_read_by_the_op() {
    let s = scope("sortedByProduct");
    assert_eq!(entry(&s, "products").via, "body");
}

#[test]
fn a_write_condition_through_a_ref_hop_is_read_by_the_op() {
    let s = scope("setMeta");
    let products = entry(&s, "products");
    assert!(products.read.contains("name"), "{products:?}");
    assert_eq!(products.via, "body", "{products:?}");
}

#[test]
fn a_field_only_a_guard_examines_keeps_the_guard_marker() {
    let s = scope("docBodies");
    assert_eq!(entry(&s, "docs").via, "body");
    let seats = entry(&s, "seats");
    assert_eq!(seats.via, "guard", "{seats:?}");
    assert!(
        render_scope_entry(seats).ends_with("read by a guard, not by the op"),
        "{}",
        render_scope_entry(seats)
    );
}

#[test]
fn a_field_both_the_op_and_a_guard_read_is_read_by_the_op() {
    // `orders`' guard reads `buyer`; so does the projection's hop.
    let s = scope("orderSheet");
    let orders = entry(&s, "orders");
    assert!(orders.read.contains("buyer"), "{orders:?}");
    assert_eq!(orders.via, "body");
}
