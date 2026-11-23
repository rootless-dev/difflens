//! Fixture of the `mutate` command: code whose tests miss a boundary, code
//! they verify, a loop a mutant makes endless, and code no test runs.

pub struct Account {
    pub balance: i64,
}

impl Account {
    /// The tests never withdraw the whole balance, so `>` into `>=` survives.
    pub fn withdraw(&mut self, amount: i64) -> bool {
        if amount > self.balance {
            return false;
        }
        self.balance -= amount;
        true
    }
}

/// Every mutant of it is caught.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// `i += 1` into `i *= 1` never ends.
pub fn steps(n: u32) -> u32 {
    let mut i = 0;
    while i < n {
        i += 1;
    }
    i * 2
}

/// No test runs it.
pub fn untested(x: i64) -> i64 {
    x * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdraws_less_than_the_balance() {
        let mut account = Account { balance: 10 };
        assert!(account.withdraw(4));
        assert_eq!(account.balance, 6);
        assert!(!account.withdraw(20));
        assert_eq!(account.balance, 6);
    }

    #[test]
    fn adds() {
        assert_eq!(add(2, 3), 5);
        assert_eq!(add(-1, 4), 3);
    }

    #[test]
    fn counts_steps() {
        assert_eq!(steps(3), 6);
        assert_eq!(steps(0), 0);
    }
}
