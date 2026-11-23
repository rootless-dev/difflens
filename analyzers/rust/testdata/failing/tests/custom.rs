//! A test without libtest: it checks by itself and says so by its exit
//! status.

fn main() {
    assert_eq!(failing::add(1, 2), 3);
    println!("custom checks passed");
}
