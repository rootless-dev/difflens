//! Two identical existing functions: a duplicate the review does not touch.

pub fn first_twin(items: &[u32], step: u32) -> Vec<u32> {
    let mut kept = Vec::new();
    for item in items {
        if item % step == 0 && *item > 10 {
            kept.push(item * 2 + 1);
        } else if *item < 3 {
            kept.push(*item);
        }
    }
    kept.sort();
    kept
}

pub fn second_twin(items: &[u32], step: u32) -> Vec<u32> {
    let mut kept = Vec::new();
    for item in items {
        if item % step == 0 && *item > 10 {
            kept.push(item * 2 + 1);
        } else if *item < 3 {
            kept.push(*item);
        }
    }
    kept.sort();
    kept
}
