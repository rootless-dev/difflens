//! Language detection by marker files at the repository root and one level down.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::analyzers;
use crate::model::AnalysisSupport;

struct LanguageRule {
    language: &'static str,
    /// The language is detected when any of these files is present...
    markers: &'static [&'static str],
    /// ...unless one of these is present too.
    unless: &'static [&'static str],
}

/// Add a language by adding a rule here.
const RULES: &[LanguageRule] = &[
    LanguageRule {
        language: "go",
        markers: &["go.mod"],
        unless: &[],
    },
    LanguageRule {
        language: "javascript",
        markers: &["package.json"],
        unless: &["tsconfig.json"],
    },
    LanguageRule {
        language: "python",
        markers: &["pyproject.toml", "requirements.txt"],
        unless: &[],
    },
    LanguageRule {
        language: "rust",
        markers: &["Cargo.toml"],
        unless: &[],
    },
    LanguageRule {
        language: "typescript",
        markers: &["tsconfig.json"],
        unless: &[],
    },
];

/// Dependency and build output folders whose marker files describe third-party
/// code, not the project.
const SKIPPED_DIRECTORIES: &[&str] = &["node_modules", "vendor", "target", "dist", "build"];

/// Returns the detected language ids, sorted. Unreadable folders are skipped:
/// detection is best effort and never fails.
pub fn detect(root: &Path) -> Vec<String> {
    let mut found = HashSet::new();
    for directory in directories_to_scan(root) {
        collect_markers(&directory, &mut found);
    }

    let mut languages: Vec<String> = RULES
        .iter()
        .filter(|rule| rule.markers.iter().any(|marker| found.contains(marker)))
        .filter(|rule| !rule.unless.iter().any(|marker| found.contains(marker)))
        .map(|rule| rule.language.to_string())
        .collect();
    languages.sort();
    languages
}

/// Full when at least one of `languages` has an analyzer in the registry,
/// whether a sidecar or one prepared the first time it is needed.
pub fn analysis_support(languages: &[String]) -> AnalysisSupport {
    let analyzed = languages
        .iter()
        .any(|language| analyzers::for_language(language).is_some());
    if analyzed {
        AnalysisSupport::Full
    } else {
        AnalysisSupport::DiffOnly
    }
}

fn directories_to_scan(root: &Path) -> Vec<std::path::PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let Ok(entries) = fs::read_dir(root) else {
        return directories;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || SKIPPED_DIRECTORIES.contains(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        }
    }
    directories
}

fn collect_markers(directory: &Path, found: &mut HashSet<&'static str>) {
    let known = RULES
        .iter()
        .flat_map(|rule| rule.markers.iter().chain(rule.unless.iter()));
    for marker in known {
        if directory.join(marker).is_file() {
            found.insert(*marker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tree(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for file in files {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        dir
    }

    #[test]
    fn detects_nothing_in_an_empty_folder() {
        let dir = tree(&[]);
        assert!(detect(dir.path()).is_empty());
    }

    #[test]
    fn detects_each_marker_at_the_root() {
        let cases = [
            ("go.mod", "go"),
            ("package.json", "javascript"),
            ("tsconfig.json", "typescript"),
            ("pyproject.toml", "python"),
            ("requirements.txt", "python"),
            ("Cargo.toml", "rust"),
        ];
        for (marker, language) in cases {
            let dir = tree(&[marker]);
            assert_eq!(detect(dir.path()), vec![language], "marker {marker}");
        }
    }

    #[test]
    fn package_json_with_tsconfig_is_typescript_only() {
        let dir = tree(&["package.json", "tsconfig.json"]);
        assert_eq!(detect(dir.path()), vec!["typescript"]);
    }

    #[test]
    fn detects_markers_one_level_down_but_not_deeper() {
        let dir = tree(&["backend/go.mod", "web/package.json", "a/b/Cargo.toml"]);
        assert_eq!(detect(dir.path()), vec!["go", "javascript"]);
    }

    #[test]
    fn ignores_dependency_and_hidden_folders() {
        let dir = tree(&[
            "node_modules/package.json",
            "vendor/go.mod",
            ".cache/Cargo.toml",
        ]);
        assert!(detect(dir.path()).is_empty());
    }

    #[test]
    fn returns_several_languages_sorted() {
        let dir = tree(&["Cargo.toml", "go.mod", "scripts/requirements.txt"]);
        assert_eq!(detect(dir.path()), vec!["go", "python", "rust"]);
    }

    #[test]
    fn a_missing_folder_detects_nothing() {
        let dir = tree(&[]);
        assert!(detect(&dir.path().join("gone")).is_empty());
    }

    #[test]
    fn analysis_is_full_only_with_an_analyzed_language() {
        for language in ["go", "python", "javascript", "typescript", "rust"] {
            assert_eq!(
                analysis_support(&[language.to_string()]),
                AnalysisSupport::Full,
                "{language}"
            );
        }
        // Every language detected has an analyzer, so only a project with
        // none found is diff only.
        for rule in RULES {
            assert!(analyzers::for_language(rule.language).is_some(), "{}", rule.language);
        }
        assert_eq!(analysis_support(&["haskell".to_string()]), AnalysisSupport::DiffOnly);
        assert_eq!(analysis_support(&[]), AnalysisSupport::DiffOnly);
    }
}
