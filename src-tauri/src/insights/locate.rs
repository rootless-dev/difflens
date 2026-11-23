//! Where a finding is in the code as the review shows it now: on its cited
//! lines while they read as they did, else wherever those lines moved to in
//! the file. And what the user's verdict was taken against, so a later look
//! can tell that the code around it changed.

use crate::fingerprint;
use crate::model::ExcerptLine;

/// Lines above and below the cited ones that a verdict's fingerprint covers.
pub const FINGERPRINT_AROUND: u32 = 3;

/// Lines shown above and below the cited ones in an excerpt.
const EXCERPT_AROUND: u32 = 2;

/// Longest excerpt, in lines.
const EXCERPT_MAX_LINES: u32 = 40;

/// Cited text with fewer letters and digits than this (a lone `}`) is too
/// common to be followed when it moves.
const DISTINCTIVE_CHARACTERS: usize = 4;

/// Where the cited lines are in `lines` now: `(line, end_line, moved)`, or
/// `None` when they no longer read as they did and were not found again.
pub fn relocate(
    lines: &[String],
    line: u32,
    end_line: u32,
    cited: &str,
) -> Option<(u32, u32, bool)> {
    let wanted: Vec<&str> = cited.split('\n').collect();
    let span = wanted.len();
    let reads_as_cited = |start: usize| {
        lines
            .get(start..start + span)
            .is_some_and(|window| window.iter().map(String::as_str).eq(wanted.iter().copied()))
    };
    let start = (line as usize).saturating_sub(1);
    if reads_as_cited(start) {
        return Some((line, end_line.max(line), false));
    }
    let distinctive = cited.chars().filter(|c| c.is_alphanumeric()).count();
    if distinctive < DISTINCTIVE_CHARACTERS || span > lines.len() {
        return None;
    }
    let found = (0..=lines.len() - span)
        .filter(|&candidate| reads_as_cited(candidate))
        .min_by_key(|&candidate| candidate.abs_diff(start))?;
    let line = count(found + 1);
    Some((line, count(found + span), true))
}

/// SHA-256 of the cited lines and `FINGERPRINT_AROUND` lines on each side,
/// as far as the file goes.
pub fn fingerprint_around(lines: &[String], line: u32, end_line: u32) -> String {
    let first = line.saturating_sub(FINGERPRINT_AROUND).max(1);
    let last = end_line.saturating_add(FINGERPRINT_AROUND);
    let start = (first as usize - 1).min(lines.len());
    let end = (last as usize).clamp(start, lines.len());
    let window = lines[start..end].join("\n");
    fingerprint::of_lines(&window, 1, count(end - start))
}

/// The cited lines and two around them, read from `lines`.
pub fn excerpt(lines: &[String], line: u32, end_line: u32) -> Vec<ExcerptLine> {
    let total = count(lines.len());
    let first = line.saturating_sub(EXCERPT_AROUND).max(1);
    let last = end_line
        .saturating_add(EXCERPT_AROUND)
        .min(total)
        .min(first + EXCERPT_MAX_LINES - 1);
    (first..=last)
        .filter_map(|number| {
            let text = lines.get(number as usize - 1)?;
            Some(ExcerptLine {
                line: number,
                text: text.clone(),
                cited: (line..=end_line).contains(&number),
            })
        })
        .collect()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_string).collect()
    }

    #[test]
    fn a_finding_stays_on_its_lines_or_follows_them_when_they_move() {
        let before = lines("a\nref, err := s.gateway.Refund(ctx)\nif err != nil {\n}\nz");
        let cited = "ref, err := s.gateway.Refund(ctx)\nif err != nil {";
        assert_eq!(relocate(&before, 2, 3, cited), Some((2, 3, false)));

        let moved =
            lines("// new\n// lines\na\nref, err := s.gateway.Refund(ctx)\nif err != nil {\n}\nz");
        assert_eq!(relocate(&moved, 2, 3, cited), Some((4, 5, true)));

        let changed = lines("a\nref, err := s.gateway.Refund(ctx, id)\nif err != nil {\n}\nz");
        assert_eq!(relocate(&changed, 2, 3, cited), None);
    }

    #[test]
    fn the_nearest_copy_wins_and_common_lines_are_not_followed() {
        let text = lines("x()\nsend()\nx()\nbar()\nbar()\nbar()\nsend()");
        // send() moved from line 6: lines 2 and 7 read the same; 7 is nearer.
        assert_eq!(relocate(&text, 6, 6, "send()"), Some((7, 7, true)));
        assert_eq!(relocate(&lines("a\n}\nb"), 1, 1, "}"), None);
        assert_eq!(relocate(&lines("a"), 1, 3, "a\nb\nc"), None);
    }

    #[test]
    fn the_fingerprint_covers_the_cited_lines_and_three_around_them() {
        let text: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
        let base = fingerprint_around(&text, 10, 11);
        let mut near = text.clone();
        near[6] = "changed line 7".to_string();
        assert_ne!(fingerprint_around(&near, 10, 11), base);
        let mut far = text.clone();
        far[5] = "changed line 6".to_string();
        far[14] = "changed line 15".to_string();
        assert_eq!(fingerprint_around(&far, 10, 11), base);
        // Moved down with what is around it: the same code.
        let mut shifted = vec!["new".to_string(), "lines".to_string()];
        shifted.extend(text.clone());
        assert_eq!(fingerprint_around(&shifted, 12, 13), base);
        // Near the ends it covers what there is.
        assert_eq!(
            fingerprint_around(&text, 1, 1),
            fingerprint_around(&text[..4], 1, 1)
        );
    }

    #[test]
    fn an_excerpt_is_the_cited_lines_and_two_around_them() {
        let text: Vec<String> = (1..=60).map(|n| format!("l{n}")).collect();
        let shown: Vec<(u32, bool)> = excerpt(&text, 52, 53)
            .iter()
            .map(|line| (line.line, line.cited))
            .collect();
        assert_eq!(
            shown,
            [
                (50, false),
                (51, false),
                (52, true),
                (53, true),
                (54, false),
                (55, false)
            ]
        );
        assert_eq!(excerpt(&text, 60, 60).last().unwrap().line, 60);
        assert_eq!(excerpt(&text, 1, 1)[0].line, 1);
        assert_eq!(excerpt(&text, 1, 60).len(), 40);
    }
}
