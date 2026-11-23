use shapes::{Circle, Shape};

#[test]
fn circles_have_area() {
    let circle = Circle::new(2.0);
    assert!(circle.area() > 12.0);
}
