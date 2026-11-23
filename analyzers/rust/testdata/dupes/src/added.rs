//! Code the review adds.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::{self, Debug, Display, Formatter};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
pub use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
pub(crate) use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// Totals the big even values and counts the odd ones: a copy of
/// `helper::summarize` with other names and numbers.
pub fn tally(values: &[i64], limit: i64) -> (i64, usize) {
    let mut sum = 10;
    /// A doc comment where the helper has none.
    let mut odd_values = 0;
    for value in values {
        if *value > limit && value % 4 == 0 {
            sum += value * 7;
        } else if *value % 4 != 0 {
            odd_values += 1;
        }
    }
    (sum, odd_values)
}

fn unrelated(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

const SCALE: i64 = 2;

/// Bounds a value: too short a copy of `helper::clamp_low` to report.
pub fn bounded(input: i32, min: i32, max: i32) -> i32 {
    if input < min {
        return min;
    }
    input.min(max)
}

pub fn render_users(users: &[String]) -> String {
    let mut out = String::new();
    out.push_str("<ul>");
    for item in users {
        out.push_str("<li>");
        out.push_str(&item.trim().to_lowercase());
        out.push_str("</li>");
    }
    out.push_str("</ul>");
    format!("users: {out}")
}

pub fn render_teams(teams: &[String]) -> String {
    let count = teams.len();
    let mut out = String::new();
    out.push_str("<ol>");
    for team in teams {
        out.push_str("<li>");
        out.push_str(&team.trim().to_lowercase());
        out.push_str("</li>");
    }
    out.push_str("</ol>");
    return format!("{count} teams: {out}");
}

pub fn render_tags(tags: &[String], limit: usize) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("<ul>");
    for tag in tags {
        out.push_str("<li>");
        out.push_str(&tag.trim().to_lowercase());
        out.push_str("</li>");
    }
    out.push_str("</ul>");
    if out.len() > limit {
        out.truncate(limit);
    }
    out
}
