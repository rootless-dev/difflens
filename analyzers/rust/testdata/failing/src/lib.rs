//! A library whose tests pass, fail, panic and are ignored, and with a
//! clippy finding.

pub fn add(left: u64, right: u64) -> u64 {
    return left + right;
}

pub fn halve(value: u64) -> u64 {
    if value.is_multiple_of(2) {
        value / 2
    } else {
        value / 2 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 2), 4);
    }

    #[test]
    fn halves_wrongly() {
        println!("about to halve");
        assert_eq!(halve(3), 1, "halve rounds up");
    }

    #[test]
    #[ignore = "slow"]
    fn ignored() {
        assert_eq!(add(1, 1), 2);
    }

    #[test]
    fn panics() {
        let values: Vec<u64> = Vec::new();
        let _ = values[3];
    }
}
