use failing::add;

#[test]
fn adds_through_the_api() {
    assert_eq!(add(40, 2), 42);
}
