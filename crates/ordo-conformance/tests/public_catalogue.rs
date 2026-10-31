//! A public catalogue read anonymously, against Postgres
//! (`design-public-path.md` §2): a public op compiled under the public gate
//! answers a caller with no identity, and a private field of the same row is
//! never served, because no public op can name it.

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

const PROGRAM: &str = "
entity plans {
  name:   text  public
  price:  money public
  margin: money
}

op pricing public =
  plans(price > 0)[name, price]
  sort name
  take 50
";

const SEED: &str = r#"
INSERT INTO "plans" ("$space", "id", "rev", "name", "price", "margin") VALUES
  (@SPACE@, 'plan_1', 1, 'basic', 500, 100),
  (@SPACE@, 'plan_2', 1, 'pro', 2000, 900),
  (@SPACE@, 'plan_3', 1, 'retired', 0, 0);
"#;

#[test]
fn a_public_catalogue_is_read_anonymously() {
    let f = Fixture::from_source(PROGRAM, SEED).expect("the catalogue compiles");
    let pg = Pg::scoped("ordo_public_catalogue");
    f.reset(&pg).expect("seeded");
    let v: Value = f
        .run_case_as(&pg, "pricing", &[], None)
        .expect("an anonymous caller reads a public op");
    let rows = v.as_array().expect("rows");
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["basic", "pro"]);
    for r in rows {
        assert!(
            r.get("margin").is_none(),
            "a private field is never served: {r}"
        );
    }
}

#[test]
fn a_private_field_cannot_be_reached_by_a_public_op() {
    for body in [
        "plans[name, margin]\n  take 50",
        "plans(margin > 100)[name]\n  take 50",
        "plans[name]\n  sort margin\n  take 50",
    ] {
        let src = format!("{PROGRAM}\nop leak public =\n  {body}\n");
        let err = Fixture::from_source(&src, SEED).err().unwrap_or_default();
        assert!(err.contains("public_reads_private"), "{body}: {err}");
    }
}
