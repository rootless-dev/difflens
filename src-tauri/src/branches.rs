//! Which branches are integration branches, which of them each branch to
//! review is compared with, and the card key in a branch name. Pure logic over
//! what git reported; no process runs here.

use std::collections::{BTreeSet, HashMap};

use crate::git::{AheadBehind, BranchRef, RefKind};
use crate::model::{BaseSource, BranchBase};

/// Ahead/behind of refs against one base commit, keyed by full ref name.
pub type Distances = HashMap<String, AheadBehind>;

/// The local branches and the branches of origin of one repository, most
/// recently committed first.
pub struct Inventory {
    refs: Vec<BranchRef>,
}

impl Inventory {
    pub fn new(refs: Vec<BranchRef>) -> Self {
        Self { refs }
    }

    /// The ref a branch to review stands for: the local branch when there is
    /// one, otherwise the branch of origin. The local branch is what the user
    /// works on, with commits not pushed yet.
    pub fn resolve(&self, name: &str) -> Option<&BranchRef> {
        self.find(RefKind::Local, name)
            .or_else(|| self.find(RefKind::Origin, name))
    }

    /// The ref an integration branch stands for as a base: the branch of
    /// origin when there is one, otherwise the local branch. Origin is what a
    /// pull request compares with, so a local base that is out of date, or
    /// has commits never pushed, does not change what the review shows. Every
    /// base, pinned or found from history, is resolved here.
    pub fn resolve_base(&self, name: &str) -> Option<&BranchRef> {
        self.find(RefKind::Origin, name)
            .or_else(|| self.find(RefKind::Local, name))
    }

    /// Integration branches that exist, locally or on origin, as names. They
    /// follow the order of `patterns`; the names one glob matches are sorted
    /// by name. Each name appears once.
    pub fn bases(&self, patterns: &[String]) -> Vec<String> {
        let names: BTreeSet<&str> = self.refs.iter().map(|r| r.name.as_str()).collect();
        let mut bases: Vec<String> = Vec::new();
        for pattern in patterns {
            for name in names.iter().filter(|name| glob_matches(pattern, name)) {
                if !bases.iter().any(|base| base == name) {
                    bases.push((*name).to_string());
                }
            }
        }
        bases
    }

    /// Branches to review: local branches, and branches of origin with no
    /// local branch of the same name, that are not integration branches.
    pub fn review_branches<'a: 'p, 'p>(
        &'a self,
        patterns: &'p [String],
    ) -> impl Iterator<Item = &'a BranchRef> + 'p {
        self.refs.iter().filter(move |branch| {
            let shadowed =
                branch.kind == RefKind::Origin && self.find(RefKind::Local, &branch.name).is_some();
            !shadowed && !is_integration(&branch.name, patterns)
        })
    }

    fn find(&self, kind: RefKind, name: &str) -> Option<&BranchRef> {
        self.refs
            .iter()
            .find(|branch| branch.kind == kind && branch.name == name)
    }
}

/// How the user would write a ref: `origin/develop` for a branch of origin,
/// `develop` for a local branch.
pub fn ref_name(branch: &BranchRef) -> String {
    match branch.kind {
        RefKind::Local => branch.name.clone(),
        RefKind::Origin => format!("origin/{}", branch.name),
    }
}

/// An integration branch a branch can be compared with.
pub struct Candidate<'a> {
    pub name: &'a str,
    /// The ref the base resolves to; see `ref_name`.
    pub ref_name: &'a str,
    /// Commit the base resolves to; see `Inventory::resolve_base`.
    pub sha: &'a str,
    pub distances: &'a Distances,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenBase {
    pub base: BranchBase,
    pub distance: AheadBehind,
}

/// The base of the branch whose full ref is `branch_ref`. A pinned base wins
/// while it is still a candidate. Otherwise the candidate with the fewest
/// commits on the branch that are not in it (ahead), then the fewest commits
/// in it that are not on the branch (behind), then the earliest in
/// `candidates`, which follow the order of the integration branches.
pub fn choose_base(
    branch_ref: &str,
    pinned: Option<&str>,
    candidates: &[Candidate<'_>],
) -> Option<ChosenBase> {
    let measured = candidates.iter().filter_map(|candidate| {
        candidate
            .distances
            .get(branch_ref)
            .map(|distance| (candidate, *distance))
    });
    let pinned_choice = pinned.and_then(|pinned| {
        measured
            .clone()
            .find(|(candidate, _)| candidate.name == pinned)
            .map(|(candidate, distance)| (candidate, distance, BaseSource::User))
    });
    // min_by_key keeps the first of equal elements, which is the tie-break.
    let choice = pinned_choice.or_else(|| {
        measured
            .min_by_key(|(_, distance)| (distance.ahead, distance.behind))
            .map(|(candidate, distance)| (candidate, distance, BaseSource::History))
    });
    choice.map(|(candidate, distance, source)| ChosenBase {
        base: BranchBase {
            name: candidate.name.to_string(),
            source,
            ref_name: candidate.ref_name.to_string(),
            sha: candidate.sha.to_string(),
        },
        distance,
    })
}

pub fn is_integration(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| glob_matches(pattern, name))
}

/// Matches a branch name against a pattern where `*` stands for any run of
/// characters and `?` for one character, neither crossing a `/`, as git's
/// own ref patterns do. Anything else matches itself.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    glob_matches_from(&pattern, &name)
}

fn glob_matches_from(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => {
            for taken in 0..=name.len() {
                if glob_matches_from(rest, &name[taken..]) {
                    return true;
                }
                if name.get(taken) == Some(&'/') {
                    break;
                }
            }
            false
        }
        Some(('?', rest)) => match name.split_first() {
            Some((first, remaining)) if *first != '/' => glob_matches_from(rest, remaining),
            _ => false,
        },
        Some((literal, rest)) => match name.split_first() {
            Some((first, remaining)) if first == literal => glob_matches_from(rest, remaining),
            _ => false,
        },
    }
}

/// The first card key in a branch name: what `[A-Z][A-Z0-9]+-\d+` matches,
/// such as `ABC-123` in `feature/ABC-123-login`.
pub fn card(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    (0..bytes.len()).find_map(|start| {
        card_end(bytes, start).map(|end| String::from_utf8_lossy(&bytes[start..end]).into_owned())
    })
}

/// End of a card key starting at `start`. Every byte it accepts is ASCII, so
/// the slice is valid UTF-8.
fn card_end(bytes: &[u8], start: usize) -> Option<usize> {
    let run = |from: usize, accept: fn(&u8) -> bool| {
        bytes
            .get(from..)
            .map_or(0, |rest| rest.iter().take_while(|b| accept(b)).count())
    };
    if !bytes.get(start)?.is_ascii_uppercase() {
        return None;
    }
    let prefix_end = start + 1 + run(start + 1, |b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if prefix_end == start + 1 || bytes.get(prefix_end) != Some(&b'-') {
        return None;
    }
    let digits = run(prefix_end + 1, u8::is_ascii_digit);
    (digits > 0).then_some(prefix_end + 1 + digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Commit;

    fn patterns(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn branch(kind: RefKind, name: &str) -> BranchRef {
        let prefix = match kind {
            RefKind::Local => "refs/heads/",
            RefKind::Origin => "refs/remotes/origin/",
        };
        BranchRef {
            kind,
            name: name.to_string(),
            full_ref: format!("{prefix}{name}"),
            is_current: false,
            head: Commit {
                sha: format!("sha of {prefix}{name}"),
                subject: String::new(),
                authored_at: String::new(),
            },
            upstream: None,
        }
    }

    #[test]
    fn globs_match_within_one_path_component() {
        assert!(glob_matches("release/*", "release/2.4"));
        assert!(glob_matches("release/*", "release/"));
        assert!(!glob_matches("release/*", "release/2.4/rc"));
        assert!(!glob_matches("release/*", "release"));
        assert!(!glob_matches("release/*", "prerelease/2.4"));
        assert!(glob_matches("*/main", "team/main"));
        assert!(glob_matches("v?.x", "v2.x"));
        assert!(!glob_matches("v?.x", "v/.x"));
        assert!(glob_matches("develop", "develop"));
        assert!(!glob_matches("develop", "develop2"));
        assert!(!glob_matches("main", "feature/main"));
        assert!(glob_matches("rel*/*-*", "release/2.4-hotfix"));
    }

    #[test]
    fn cards_are_found_anywhere_in_the_name() {
        assert_eq!(card("feature/ABC-178").as_deref(), Some("ABC-178"));
        assert_eq!(card("ABC-178-login-page").as_deref(), Some("ABC-178"));
        assert_eq!(card("fix/xAB2-17_typo").as_deref(), Some("AB2-17"));
        assert_eq!(card("hotfix/PAY-1-and-OPS-2").as_deref(), Some("PAY-1"));
        assert_eq!(card("é/ÜX-PAY-77").as_deref(), Some("PAY-77"));
        assert_eq!(card("feature/abc-178"), None);
        assert_eq!(card("feature/A-178"), None);
        assert_eq!(card("feature/ABC-"), None);
        assert_eq!(card("feature/ABC178"), None);
        assert_eq!(card(""), None);
    }

    #[test]
    fn bases_follow_the_pattern_order_and_prefer_nothing_twice() {
        let inventory = Inventory::new(vec![
            branch(RefKind::Local, "feature/x"),
            branch(RefKind::Local, "release/2.10"),
            branch(RefKind::Origin, "release/2.9"),
            branch(RefKind::Local, "master"),
            branch(RefKind::Origin, "master"),
            branch(RefKind::Origin, "develop"),
        ]);

        assert_eq!(
            inventory.bases(&patterns(&[
                "main",
                "master",
                "develop",
                "release/*",
                "develop"
            ])),
            vec!["master", "develop", "release/2.10", "release/2.9"]
        );
        assert!(inventory.bases(&[]).is_empty());
    }

    #[test]
    fn review_branches_skip_integration_branches_and_shadowed_remotes() {
        let inventory = Inventory::new(vec![
            branch(RefKind::Origin, "feature/remote-only"),
            branch(RefKind::Local, "feature/both"),
            branch(RefKind::Origin, "feature/both"),
            branch(RefKind::Local, "develop"),
            branch(RefKind::Origin, "release/1.0"),
            branch(RefKind::Local, "release-notes"),
        ]);

        let names: Vec<&str> = inventory
            .review_branches(&patterns(&["develop", "release/*"]))
            .map(|b| b.full_ref.as_str())
            .collect();

        assert_eq!(
            names,
            vec![
                "refs/remotes/origin/feature/remote-only",
                "refs/heads/feature/both",
                "refs/heads/release-notes"
            ]
        );
    }

    #[test]
    fn resolve_prefers_the_local_branch_over_origin() {
        let inventory = Inventory::new(vec![
            branch(RefKind::Origin, "develop"),
            branch(RefKind::Local, "develop"),
            branch(RefKind::Origin, "master"),
        ]);

        assert_eq!(
            inventory.resolve("develop").map(|b| b.full_ref.as_str()),
            Some("refs/heads/develop")
        );
        assert_eq!(
            inventory.resolve("master").map(|b| b.full_ref.as_str()),
            Some("refs/remotes/origin/master")
        );
        assert!(inventory.resolve("origin/master").is_none());
        assert!(inventory.resolve("missing").is_none());
    }

    #[test]
    fn a_base_resolves_to_origin_before_the_local_branch() {
        let inventory = Inventory::new(vec![
            branch(RefKind::Local, "develop"),
            branch(RefKind::Origin, "develop"),
            branch(RefKind::Local, "trunk"),
        ]);

        assert_eq!(
            inventory.resolve_base("develop").map(|b| b.full_ref.as_str()),
            Some("refs/remotes/origin/develop")
        );
        assert_eq!(
            inventory.resolve_base("trunk").map(|b| b.full_ref.as_str()),
            Some("refs/heads/trunk")
        );
        assert!(inventory.resolve_base("origin/develop").is_none());
        assert!(inventory.resolve_base("missing").is_none());
        assert_eq!(
            inventory.resolve_base("develop").map(ref_name).as_deref(),
            Some("origin/develop")
        );
        assert_eq!(
            inventory.resolve_base("trunk").map(ref_name).as_deref(),
            Some("trunk")
        );
    }

    fn distances(ahead: u32, behind: u32) -> Distances {
        HashMap::from([("refs/heads/x".to_string(), AheadBehind { ahead, behind })])
    }

    fn chosen(choice: Option<ChosenBase>) -> Option<(String, BaseSource, u32, u32)> {
        choice.map(|c| {
            (
                c.base.name,
                c.base.source,
                c.distance.ahead,
                c.distance.behind,
            )
        })
    }

    #[test]
    fn the_base_is_the_closest_candidate_in_history() {
        let main = distances(5, 0);
        let develop = distances(2, 9);
        let release = distances(2, 3);
        let candidates = [
            Candidate {
                name: "main",
                ref_name: "origin/main",
                sha: "sha of main",
                distances: &main,
            },
            Candidate {
                name: "develop",
                ref_name: "origin/develop",
                sha: "sha of develop",
                distances: &develop,
            },
            Candidate {
                name: "release/1",
                ref_name: "release/1",
                sha: "sha of release/1",
                distances: &release,
            },
        ];

        let choice = choose_base("refs/heads/x", None, &candidates);

        assert_eq!(
            choice.as_ref().map(|c| c.base.sha.as_str()),
            Some("sha of release/1")
        );
        assert_eq!(
            chosen(choice),
            Some(("release/1".to_string(), BaseSource::History, 2, 3))
        );
    }

    #[test]
    fn equal_distances_fall_back_to_the_order_of_the_candidates() {
        let same = distances(1, 1);
        let candidates = [
            Candidate {
                name: "master",
                ref_name: "master",
                sha: "sha of master",
                distances: &same,
            },
            Candidate {
                name: "develop",
                ref_name: "origin/develop",
                sha: "sha of develop",
                distances: &same,
            },
        ];

        assert_eq!(
            chosen(choose_base("refs/heads/x", None, &candidates)).map(|c| c.0),
            Some("master".to_string())
        );
    }

    #[test]
    fn a_pinned_base_wins_while_it_is_a_candidate() {
        let near = distances(1, 0);
        let far = distances(30, 400);
        let candidates = [
            Candidate {
                name: "develop",
                ref_name: "origin/develop",
                sha: "sha of develop",
                distances: &near,
            },
            Candidate {
                name: "master",
                ref_name: "master",
                sha: "sha of master",
                distances: &far,
            },
        ];

        let pinned = choose_base("refs/heads/x", Some("master"), &candidates);

        assert_eq!(
            pinned
                .as_ref()
                .map(|c| (c.base.sha.as_str(), c.base.ref_name.as_str())),
            Some(("sha of master", "master"))
        );
        assert_eq!(
            chosen(pinned),
            Some(("master".to_string(), BaseSource::User, 30, 400))
        );
        assert_eq!(
            chosen(choose_base("refs/heads/x", Some("gone"), &candidates)),
            Some(("develop".to_string(), BaseSource::History, 1, 0))
        );
        assert_eq!(choose_base("refs/heads/x", Some("master"), &[]), None);
        assert_eq!(choose_base("refs/heads/other", None, &candidates), None);
    }
}
