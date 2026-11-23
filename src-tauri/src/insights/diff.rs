//! A line diff of the two sides of a file, rendered as unified hunks whose
//! lines carry their new-side number, so the model can cite them.
//!
//! The diff is computed here rather than asked of git: the new side can be
//! the working tree, an untracked file or a blob, and the review already has
//! both texts in hand.

use std::fmt::Write;

/// One line of the diff, by its 0-based index on each side it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Same { old: usize, new: usize },
    Removed { old: usize },
    Added { new: usize },
}

impl Line {
    fn changed(self) -> bool {
        !matches!(self, Line::Same { .. })
    }
}

/// Past this many edits the middle of the file is shown as removed, then
/// added, instead of being matched line by line: memory grows with the
/// square of the edits.
const MAX_EDITS: usize = 1500;

/// The lines of a text as `str::lines` splits them (no `\r`, no empty line
/// after a final newline), numbered as the Diff tab numbers them.
pub fn lines_of(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// A shortest edit script from `old` to `new` (Myers), after the lines both
/// share at the start and at the end.
pub fn diff(old: &[&str], new: &[&str]) -> Vec<Line> {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];

    let mut lines: Vec<Line> = (0..prefix)
        .map(|index| Line::Same {
            old: index,
            new: index,
        })
        .collect();
    match myers(old_middle, new_middle) {
        Some(middle) => lines.extend(middle.into_iter().map(|line| shift(line, prefix))),
        None => {
            lines.extend((0..old_middle.len()).map(|index| Line::Removed {
                old: prefix + index,
            }));
            lines.extend((0..new_middle.len()).map(|index| Line::Added {
                new: prefix + index,
            }));
        }
    }
    let old_tail = old.len() - suffix;
    let new_tail = new.len() - suffix;
    lines.extend((0..suffix).map(|index| Line::Same {
        old: old_tail + index,
        new: new_tail + index,
    }));
    lines
}

fn shift(line: Line, by: usize) -> Line {
    match line {
        Line::Same { old, new } => Line::Same {
            old: old + by,
            new: new + by,
        },
        Line::Removed { old } => Line::Removed { old: old + by },
        Line::Added { new } => Line::Added { new: new + by },
    }
}

/// Myers' greedy algorithm, keeping for each round only the diagonals it can
/// reach. `None` past `MAX_EDITS`.
fn myers(old: &[&str], new: &[&str]) -> Option<Vec<Line>> {
    let n = old.len() as isize;
    let m = new.len() as isize;
    let max = (old.len() + new.len()).min(MAX_EDITS) as isize;
    // Furthest x on each diagonal k = x - y, at index k + offset.
    let offset = max + 1;
    let mut v = vec![0isize; (2 * offset + 1) as usize];
    // trace[d] holds diagonals -d..=d as they were before round d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=max {
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let at = |k: isize| v[(k + offset) as usize];
            let mut x = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
                at(k + 1)
            } else {
                at(k - 1) + 1
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Line> {
    let mut lines = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        if d == 0 {
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                lines.push(Line::Same {
                    old: x as usize,
                    new: y as usize,
                });
            }
            break;
        }
        let at = |k: isize| v[(k + d) as usize];
        let k = x - y;
        let previous_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let previous_x = at(previous_k);
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            lines.push(Line::Same {
                old: x as usize,
                new: y as usize,
            });
        }
        if x == previous_x {
            lines.push(Line::Added {
                new: previous_y as usize,
            });
        } else {
            lines.push(Line::Removed {
                old: previous_x as usize,
            });
        }
        x = previous_x;
        y = previous_y;
    }
    lines.reverse();
    lines
}

/// The hunks of a diff with `context` unchanged lines around each change,
/// rendered one line per diff line: a marker (`+`, `-` or a space), the
/// new-side line number (blank for a removed line), `|` and the code.
pub struct Hunks {
    pub text: String,
    /// Changed lines left out to keep within `max_chars`.
    pub left_out: usize,
}

pub fn render(
    old: &[&str],
    new: &[&str],
    lines: &[Line],
    context: usize,
    max_chars: usize,
) -> Hunks {
    let width = new.len().max(1).to_string().len().max(4);
    let mut text = String::new();
    let mut left_out = 0;
    for (start, end) in hunk_ranges(lines, context) {
        let hunk = render_hunk(old, new, &lines[start..end], width);
        if left_out > 0 || text.len() + hunk.len() > max_chars {
            left_out += lines[start..end]
                .iter()
                .filter(|line| line.changed())
                .count();
            continue;
        }
        text.push_str(&hunk);
    }
    Hunks { text, left_out }
}

/// Ranges of `lines` (end excluded) holding each change and `context` lines
/// around it, merged where they touch.
fn hunk_ranges(lines: &[Line], context: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.changed() {
            continue;
        }
        let start = index.saturating_sub(context);
        let end = (index + context + 1).min(lines.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

fn render_hunk(old: &[&str], new: &[&str], lines: &[Line], width: usize) -> String {
    let first_old = lines.iter().find_map(|line| match line {
        Line::Same { old, .. } | Line::Removed { old } => Some(*old),
        Line::Added { .. } => None,
    });
    let first_new = lines.iter().find_map(|line| match line {
        Line::Same { new, .. } | Line::Added { new } => Some(*new),
        Line::Removed { .. } => None,
    });
    let old_count = lines
        .iter()
        .filter(|line| !matches!(line, Line::Added { .. }))
        .count();
    let new_count = lines
        .iter()
        .filter(|line| !matches!(line, Line::Removed { .. }))
        .count();
    let start = |first: Option<usize>, count: usize| match first {
        Some(index) if count > 0 => index + 1,
        _ => 0,
    };
    let mut text = format!(
        "@@ -{},{} +{},{} @@\n",
        start(first_old, old_count),
        old_count,
        start(first_new, new_count),
        new_count
    );
    for line in lines {
        let _ = match *line {
            Line::Same { new: index, .. } => {
                writeln!(text, "  {:>width$} | {}", index + 1, new[index])
            }
            Line::Added { new: index } => {
                writeln!(text, "+ {:>width$} | {}", index + 1, new[index])
            }
            Line::Removed { old: index } => writeln!(text, "- {:>width$} | {}", "", old[index]),
        };
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diff as text: ` a`, `-b`, `+c`.
    fn script(old: &str, new: &str) -> Vec<String> {
        let (old, new) = (lines_of(old), lines_of(new));
        diff(&old, &new)
            .into_iter()
            .map(|line| match line {
                Line::Same {
                    old: index,
                    new: other,
                } => {
                    assert_eq!(old[index], new[other]);
                    format!(" {}", old[index])
                }
                Line::Removed { old: index } => format!("-{}", old[index]),
                Line::Added { new: index } => format!("+{}", new[index]),
            })
            .collect()
    }

    #[test]
    fn finds_a_shortest_script_that_keeps_the_shared_lines() {
        assert_eq!(
            script("a\nb\nc\nd\n", "a\nc\nx\nd\n"),
            [" a", "-b", " c", "+x", " d"]
        );
        assert_eq!(script("", "a\nb\n"), ["+a", "+b"]);
        assert_eq!(script("a\nb\n", ""), ["-a", "-b"]);
        assert_eq!(script("same\n", "same\n"), [" same"]);
        // The classic example: 5 edits.
        let edits = script("A\nB\nC\nA\nB\nB\nA\n", "C\nB\nA\nB\nA\nC\n")
            .iter()
            .filter(|line| !line.starts_with(' '))
            .count();
        assert_eq!(edits, 5);
    }

    #[test]
    fn every_line_of_both_sides_appears_once_in_order() {
        let old: Vec<String> = (0..300).map(|n| format!("line {}", n % 17)).collect();
        let new: Vec<String> = (0..280).map(|n| format!("line {}", (n * 7) % 19)).collect();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let lines = diff(&old, &new);
        let olds: Vec<usize> = lines
            .iter()
            .filter_map(|line| match line {
                Line::Same { old, .. } | Line::Removed { old } => Some(*old),
                Line::Added { .. } => None,
            })
            .collect();
        let news: Vec<usize> = lines
            .iter()
            .filter_map(|line| match line {
                Line::Same { new, .. } | Line::Added { new } => Some(*new),
                Line::Removed { .. } => None,
            })
            .collect();
        assert_eq!(olds, (0..300).collect::<Vec<_>>());
        assert_eq!(news, (0..280).collect::<Vec<_>>());
    }

    #[test]
    fn a_change_too_large_to_match_reads_as_removed_then_added() {
        let old: Vec<String> = (0..2000).map(|n| format!("old {n}")).collect();
        let new: Vec<String> = (0..2000).map(|n| format!("new {n}")).collect();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let lines = diff(&old, &new);
        assert_eq!(lines.len(), 4000);
        assert_eq!(lines[0], Line::Removed { old: 0 });
        assert_eq!(lines[2000], Line::Added { new: 0 });
    }

    #[test]
    fn renders_numbered_hunks_with_context_merged_where_they_touch() {
        let old: Vec<String> = (1..=40).map(|n| format!("l{n}")).collect();
        let mut new = old.clone();
        new[4] = "changed 5".to_string();
        new.insert(30, "added".to_string());
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        let lines = diff(&old, &new);

        let far = render(&old, &new, &lines, 2, usize::MAX);
        assert_eq!(
            far.text,
            "@@ -3,5 +3,5 @@\n\
             \x20    3 | l3\n\
             \x20    4 | l4\n\
             -      | l5\n\
             +    5 | changed 5\n\
             \x20    6 | l6\n\
             \x20    7 | l7\n\
             @@ -29,4 +29,5 @@\n\
             \x20   29 | l29\n\
             \x20   30 | l30\n\
             +   31 | added\n\
             \x20   32 | l31\n\
             \x20   33 | l32\n"
        );
        assert_eq!(far.left_out, 0);

        let near = render(&old, &new, &lines, 20, usize::MAX);
        assert_eq!(near.text.matches("@@ -").count(), 1);
        assert!(near.text.starts_with("@@ -1,40 +1,41 @@\n"));
    }

    #[test]
    fn hunks_past_the_limit_are_left_out_and_counted() {
        let old = lines_of("a\nb\nc\nd\ne\nf\ng\nh\n");
        let new = lines_of("A\nb\nc\nd\ne\nf\ng\nH\n");
        let lines = diff(&old, &new);
        let first = render(&old, &new, &lines, 0, usize::MAX).text;
        let first_hunk = &first[..first.rfind("@@ -").unwrap()];
        let cut = render(&old, &new, &lines, 0, first_hunk.len());
        assert_eq!(cut.text, first_hunk);
        assert_eq!(cut.left_out, 2);
    }

    #[test]
    fn an_added_file_is_one_hunk_of_added_lines() {
        let new = lines_of("package a\n\nfunc A() {}\n");
        let lines = diff(&[], &new);
        let rendered = render(&[], &new, &lines, 20, usize::MAX);
        assert_eq!(
            rendered.text,
            "@@ -0,0 +1,3 @@\n+    1 | package a\n+    2 | \n+    3 | func A() {}\n"
        );
    }
}
