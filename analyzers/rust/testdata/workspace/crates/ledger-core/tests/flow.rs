use ledger_core::Ledger;

#[test]
fn a_day_of_entries() {
    let mut ledger = Ledger::new();
    for amount in [10, -3, 7] {
        ledger.record(amount);
    }
    assert_eq!(ledger.balance(), 14);
}
