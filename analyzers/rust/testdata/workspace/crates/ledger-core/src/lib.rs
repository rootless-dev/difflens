//! A ledger of signed amounts.

#[derive(Debug, Default)]
pub struct Ledger {
    entries: Vec<i64>,
}

impl Ledger {
    pub fn new() -> Self {
        Ledger { entries: Vec::new() }
    }

    pub fn record(&mut self, amount: i64) {
        if amount != 0 {
            self.entries.push(amount);
        }
    }

    pub fn balance(&self) -> i64 {
        self.entries.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balances() {
        let mut ledger = Ledger::new();
        ledger.record(5);
        ledger.record(-2);
        assert_eq!(ledger.balance(), 3);
    }

    #[test]
    fn ignores_zero() {
        let mut ledger = Ledger::new();
        ledger.record(0);
        assert_eq!(ledger.balance(), 0);
    }
}
