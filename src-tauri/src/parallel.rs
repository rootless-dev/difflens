//! Work on a list with a few threads at once, as the model calls of one piece
//! of work are asked: at most as many at a time as the provider allows, in
//! the list's order, and none started once the work is stopped.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

/// Runs `work` on each of `items`, at most `workers` at once, taking them in
/// order; once `stopped` says so, no other item is started. Returns each
/// item's result in the items' order, `None` for an item never started.
pub fn run<T, R>(
    items: &[T],
    workers: usize,
    stopped: impl Fn() -> bool + Sync,
    work: impl Fn(usize, &T) -> R + Sync,
) -> Vec<Option<R>>
where
    T: Sync,
    R: Send,
{
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let workers = workers.clamp(1, items.len().max(1));
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if stopped() {
                    return;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    return;
                };
                let result = work(index, item);
                if let Ok(mut results) = results.lock() {
                    results[index] = Some(result);
                }
            });
        }
    });
    results.into_inner().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::*;

    #[test]
    fn runs_every_item_with_at_most_the_workers_asked_for() {
        let at_once = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let items: Vec<u32> = (1..=7).collect();
        let results = run(
            &items,
            3,
            || false,
            |_, item| {
                let now = at_once.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(10));
                at_once.fetch_sub(1, Ordering::SeqCst);
                item * 2
            },
        );
        assert_eq!(
            results,
            items.iter().map(|item| Some(item * 2)).collect::<Vec<_>>()
        );
        assert!(most.load(Ordering::SeqCst) <= 3);
    }

    #[test]
    fn a_stop_leaves_the_items_not_started() {
        let stop = AtomicBool::new(false);
        let items = [1, 2, 3];
        let results = run(
            &items,
            1,
            || stop.load(Ordering::SeqCst),
            |index, item| {
                if index == 0 {
                    stop.store(true, Ordering::SeqCst);
                }
                *item
            },
        );
        assert_eq!(results, [Some(1), None, None]);
        assert!(run(&[] as &[u32], 4, || false, |_, item| *item).is_empty());
    }
}
