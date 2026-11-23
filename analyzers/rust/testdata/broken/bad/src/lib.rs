//! Compiles, but its unit tests do not.

pub fn triple(value: i32) -> i32 {
    value * 3
}

#[cfg(test)]
mod tests {
    #[test]
    fn triples() {
        let text: &str = super::triple(3);
        assert_eq!(text, "9");
    }
}
