//! Existing code the review copies.

/// Sums the even scores above a floor and counts the odd ones.
pub fn summarize(scores: &[i64], floor: i64) -> (i64, usize) {
    let mut total = 0;
    let mut odd = 0;
    for score in scores {
        if *score > floor && score % 2 == 0 {
            total += score * 3;
        } else if *score % 2 != 0 {
            odd += 1;
        }
    }
    (total, odd)
}

/// Clamps a value to a range.
pub fn clamp_low(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        return low;
    }
    value.min(high)
}
