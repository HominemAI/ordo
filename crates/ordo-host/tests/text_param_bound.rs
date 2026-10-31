//! `$name: text(64)` reaches the door as the parameter's length bound.
//!
//! DL §2 Parameters: "Bounds are enforced at the door, before planning."
//! `ordo-api` counts characters and answers `bound_violation` when a
//! `ParamTy::Text` carries a `max_len` (its own suite proves that). Before
//! 2026-10-04 nothing authored ever filled the slot, because `text(64)` did
//! not parse. This holds the wiring between them: the compiler's bound is
//! the door's bound, for an authored op and for a derived std op over a
//! bounded field.
//!
//! Nothing here touches a database.

use ordo_api::ParamTy;
use ordo_conformance::{Fixture, FIXTURE, SEED};
use ordo_host::build_specs;

#[test]
fn a_text_bound_is_the_doors_max_len() {
    let src = format!(
        "{FIXTURE}\n\
         entity labels (key: lbl) {{ name: text(8)\n note: text }}\n\
         op renameLabel($id: id, $name: text(8), $note: text) =\n  \
         set labels(id == $id) {{ name: $name, note: $note }}\n"
    );
    let f = Fixture::from_source(&src, SEED).expect("compiles");
    let specs = build_specs(&f);
    let ty_of = |op: &str, param: &str| -> ParamTy {
        let spec = specs
            .iter()
            .find(|s| s.name == op)
            .unwrap_or_else(|| panic!("`{op}` has a spec"));
        spec.params
            .iter()
            .find(|p| p.name == param)
            .unwrap_or_else(|| panic!("`{op}` takes `${param}`"))
            .ty
            .clone()
    };
    for op in ["renameLabel", "std.labels.create", "std.labels.update"] {
        assert_eq!(
            ty_of(op, "name"),
            ParamTy::Text { max_len: Some(8) },
            "{op}: the declared bound reaches the door"
        );
        assert_eq!(
            ty_of(op, "note"),
            ParamTy::Text { max_len: None },
            "{op}: an unbounded text stays unbounded"
        );
    }
}
