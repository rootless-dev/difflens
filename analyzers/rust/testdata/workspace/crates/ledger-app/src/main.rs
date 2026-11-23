use ledger_core::Ledger;

fn main() {
    let mut ledger = Ledger::new();
    ledger.record(10);
    println!("{}", format_balance(ledger.balance()));
}

fn format_balance(balance: i64) -> String {
    if balance < 0 {
        format!("-${}", -balance)
    } else {
        format!("${balance}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_debts() {
        assert_eq!(super::format_balance(-5), "-$5");
    }
}
