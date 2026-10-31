//! Print the SQL the compiler emits for the executing corpus cases.
//!
//! `cargo run -p ordo-conformance --example dump_sql`
//!
//! Useful when a case fails and the question is whether the binder built the
//! wrong plan or the planner printed the right one badly. Reading the plan is
//! also the cheapest defence against a case that passes for the wrong reason.

fn main() {
    let f = ordo_conformance::Fixture::load().expect("fixture compiles");
    for (id, src) in [
        ("C001", "activeProducts[name, price]\nsort price"),
        ("C003", "doneOrders($from: 2026-02-01)[placed, total]\nsort placed"),
        (
            "C005",
            "doneOrders($from: 2026-01-01, total > 5000)[placed, total, customer.name]\nsort -total",
        ),
        ("C006", "spendByCustomer($from: 2026-01-01)\nsort -spent"),
        ("C007", "activeProducts[label: name, price]\nsort price"),
        ("C009", "top3Orders[*]"),
        ("C004 (compile only)", "top3Orders(total < 15000)[placed, total]"),
    ] {
        match f.compile(src) {
            Ok(p) => println!("── {id} ──\n{}\n", p.sql),
            Err(e) => println!("── {id} ──\nREFUSED: {e}\n"),
        }
    }
}
