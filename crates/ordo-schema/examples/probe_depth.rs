fn main() {
    let p =
        ordo_syntax::parse_program("entity zzz { n: text }\nentity cats { parent -> cats? tree }")
            .unwrap();
    let s = ordo_schema::build(&p).unwrap();
    for e in &s.entities {
        println!(
            "{}: is_tree={} depth={}",
            e.name,
            e.is_tree,
            e.field("depth").is_some()
        );
    }
}
