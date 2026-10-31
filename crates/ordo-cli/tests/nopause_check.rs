//! `ordo check` and the `nopause` op word (checklist 27ar).

use ordo_cli::{check_sources, Sources};

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("app.ordo".into(), text.to_string())])
}

const PROGRAM: &str = "\
entity invoices {
  number: text unique
  status: text = \"open\"
}

op openInvoices nopause =
  invoices(status == \"open\")[number]
  take 50

op addInvoice($number: text) =
  set invoices { number: $number }
";

#[test]
fn check_names_the_ops_that_answer_while_paused() {
    let out = check_sources(&one(PROGRAM)).expect("checks");
    assert!(
        out.contains("  openInvoices  (nopause: answers while the tenant is paused)\n"),
        "{out}"
    );
    assert!(out.contains("  addInvoice\n"), "{out}");
}
