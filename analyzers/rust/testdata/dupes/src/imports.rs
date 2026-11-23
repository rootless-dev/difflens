//! Existing code with a long block of imports.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::{self, Debug, Display, Formatter};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
pub use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
pub(crate) use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

pub fn queue_of(names: &[&str]) -> VecDeque<String> {
    names.iter().map(|name| name.to_string()).collect()
}
