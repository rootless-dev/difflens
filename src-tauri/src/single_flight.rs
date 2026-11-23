//! Concurrent requests for the same key share one computation: the first
//! caller runs it, the others wait for its result.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

pub struct SingleFlight<K, V> {
    flights: Mutex<HashMap<K, Arc<Flight<V>>>>,
}

struct Flight<V> {
    result: Mutex<Option<V>>,
    done: Condvar,
}

impl<K: Eq + Hash + Clone, V: Clone> SingleFlight<K, V> {
    pub fn new() -> Self {
        Self {
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// Runs `work` unless a run for `key` is already in progress, in which
    /// case its result is returned once ready. If the running `work` panics,
    /// those waiting get `if_abandoned`.
    pub fn run(&self, key: K, if_abandoned: V, work: impl FnOnce() -> V) -> V {
        let (flight, leader) = {
            let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
            match flights.get(&key) {
                Some(flight) => (Arc::clone(flight), false),
                None => {
                    let flight = Arc::new(Flight {
                        result: Mutex::new(None),
                        done: Condvar::new(),
                    });
                    flights.insert(key.clone(), Arc::clone(&flight));
                    (flight, true)
                }
            }
        };
        if !leader {
            return flight.wait();
        }
        let landing = Landing {
            flights: &self.flights,
            key,
            flight,
            if_abandoned: Some(if_abandoned),
        };
        let value = work();
        landing.finish(value.clone());
        value
    }
}

impl<V: Clone> Flight<V> {
    fn wait(&self) -> V {
        let mut result = self.result.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(value) = result.as_ref() {
                return value.clone();
            }
            result = self
                .done
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Publishes the result of a flight and forgets it, also when the leader
/// unwinds.
struct Landing<'a, K: Eq + Hash, V> {
    flights: &'a Mutex<HashMap<K, Arc<Flight<V>>>>,
    key: K,
    flight: Arc<Flight<V>>,
    if_abandoned: Option<V>,
}

impl<K: Eq + Hash, V> Landing<'_, K, V> {
    fn finish(mut self, value: V) {
        self.if_abandoned = None;
        self.publish(value);
    }

    fn publish(&mut self, value: V) {
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        let mut result = self
            .flight
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if result.is_none() {
            *result = Some(value);
        }
        self.flight.done.notify_all();
    }
}

impl<K: Eq + Hash, V> Drop for Landing<'_, K, V> {
    fn drop(&mut self) {
        // Only set when the leader unwound before `finish`.
        if let Some(value) = self.if_abandoned.take() {
            self.publish(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn concurrent_callers_share_one_run() {
        let flights = SingleFlight::<&str, u32>::new();
        let runs = AtomicUsize::new(0);
        let barrier = Barrier::new(4);
        let results: Vec<u32> = thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        flights.run("key", 0, || {
                            runs.fetch_add(1, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(300));
                            42
                        })
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        assert_eq!(results, vec![42; 4]);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_finished_flight_is_forgotten() {
        let flights = SingleFlight::<u8, u8>::new();
        assert_eq!(flights.run(1, 0, || 5), 5);
        assert_eq!(flights.run(1, 0, || 6), 6);
    }

    #[test]
    fn waiters_get_the_fallback_when_the_leader_panics() {
        let flights = SingleFlight::<u8, i32>::new();
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            let leader = scope.spawn(|| {
                flights.run(1, -1, || {
                    barrier.wait();
                    thread::sleep(Duration::from_millis(200));
                    panic!("leader failed");
                })
            });
            barrier.wait();
            let follower = flights.run(1, -2, || 7);
            assert!(follower == -1 || follower == 7);
            assert!(leader.join().is_err());
        });
        assert_eq!(flights.run(1, 0, || 9), 9);
    }
}
