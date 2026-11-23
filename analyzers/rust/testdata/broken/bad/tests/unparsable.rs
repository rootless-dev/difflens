// A file that does not even parse: analyze reports it as a diagnostic.

#[test]
fn never_runs() {
    let value = (1 + ;
}
