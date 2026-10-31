//! Money on the wire is an exact decimal string in major units (27ak).
//!
//! `api-engine-design.md` §5 has always said money travels as `"29.99"`,
//! and the generated SDK has always typed it as a string. The executor
//! answered with the JSON integer of minor units until places existed to
//! spell it by. These pin the spelling.

use ordo_exec::canonical;
use ordo_syntax::ScalarTy;

fn one(cell: &str, ty: ScalarTy) -> serde_json::Value {
    let rows = vec![vec![Some(cell.to_string())]];
    canonical(&rows, &[("v".into(), ty)])[0]["v"].clone()
}

#[test]
fn money_is_spelled_in_major_units_with_its_places() {
    assert_eq!(one("2999", ScalarTy::Money(2)), "29.99");
    assert_eq!(one("-5", ScalarTy::Money(2)), "-0.05");
    assert_eq!(one("2999", ScalarTy::Money(0)), "2999");
    assert_eq!(one("1", ScalarTy::Money(3)), "0.001");
    assert_eq!(one("42", ScalarTy::Int), 42, "an int stays a number");
}
