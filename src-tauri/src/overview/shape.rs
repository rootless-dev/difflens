//! The shape of a change, from its files alone: files added and deleted,
//! directories it creates or touches, and the dependency manifests and
//! database migrations it changes, told apart by their names in the ways
//! the common ecosystems name them.

use std::collections::BTreeSet;

use crate::error::AppResult;
use crate::model::{ChangeShape, FileStatus, TouchedFile};
use crate::review_state::ChangedSides;

/// Manifests and lock files, by exact file name.
const MANIFESTS: &[&str] = &[
    // Go
    "go.mod",
    "go.sum",
    "go.work",
    // JavaScript and TypeScript
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "bun.lock",
    "bun.lockb",
    "deno.json",
    "deno.lock",
    // Rust
    "Cargo.toml",
    "Cargo.lock",
    // Python
    "requirements.txt",
    "pyproject.toml",
    "Pipfile",
    "Pipfile.lock",
    "poetry.lock",
    "uv.lock",
    "setup.py",
    "setup.cfg",
    // Ruby
    "Gemfile",
    "Gemfile.lock",
    // JVM
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle.lockfile",
    "libs.versions.toml",
    "build.sbt",
    // PHP
    "composer.json",
    "composer.lock",
    // .NET
    "packages.config",
    "packages.lock.json",
    "Directory.Packages.props",
    // Swift and Apple platforms
    "Package.swift",
    "Package.resolved",
    "Podfile",
    "Podfile.lock",
    "Cartfile",
    // Elixir, Dart, C and C++
    "mix.exs",
    "mix.lock",
    "pubspec.yaml",
    "pubspec.lock",
    "vcpkg.json",
    "conanfile.txt",
    "conanfile.py",
];

/// Manifests by extension: `.csproj`, `.gemspec` and the like.
const MANIFEST_EXTENSIONS: &[&str] = &[".csproj", ".fsproj", ".vbproj", ".gemspec", ".cabal"];

/// Directory names that hold migrations.
const MIGRATION_DIRECTORIES: &[&str] = &["migrations", "migration", "migrate", "changelog"];

/// The shape of `files`. `old_directories` answers, for each directory
/// asked, whether the old side has it.
pub fn shape(
    files: &[ChangedSides],
    old_directories: impl FnOnce(&[&str]) -> AppResult<Vec<bool>>,
) -> AppResult<ChangeShape> {
    let count = |status: FileStatus| {
        u32::try_from(files.iter().filter(|file| file.status == status).count()).unwrap_or(u32::MAX)
    };
    let directories: BTreeSet<&str> = files.iter().map(|file| directory_of(&file.path)).collect();
    let directories: Vec<&str> = directories.into_iter().collect();
    let existed = old_directories(&directories)?;
    let mut directories_new = Vec::new();
    let mut directories_modified = Vec::new();
    for (directory, existed) in directories.iter().zip(existed) {
        let shown = if directory.is_empty() { "." } else { directory };
        if existed {
            directories_modified.push(shown.to_string());
        } else {
            directories_new.push(shown.to_string());
        }
    }
    let touched = |is: fn(&str) -> bool| -> Vec<TouchedFile> {
        files
            .iter()
            .filter(|file| is(&file.path))
            .map(|file| TouchedFile {
                path: file.path.clone(),
                status: file.status,
            })
            .collect()
    };
    Ok(ChangeShape {
        files_added: count(FileStatus::Added),
        files_deleted: count(FileStatus::Deleted),
        directories_new,
        directories_modified,
        manifests: touched(is_manifest),
        migrations: touched(is_migration),
    })
}

/// The directory a path is in, "" for the root.
fn directory_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub fn is_manifest(path: &str) -> bool {
    let name = file_name(path);
    if MANIFESTS.contains(&name) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    // requirements-dev.txt, requirements/base.txt
    let requirements = (lower.starts_with("requirements") && lower.ends_with(".txt"))
        || (directory_of(path).rsplit('/').next() == Some("requirements")
            && lower.ends_with(".txt"));
    requirements
        || MANIFEST_EXTENSIONS
            .iter()
            .any(|extension| lower.ends_with(extension))
}

pub fn is_migration(path: &str) -> bool {
    let name = file_name(path);
    let lower = name.to_ascii_lowercase();
    let directories: Vec<String> = directory_of(path)
        .split('/')
        .map(str::to_ascii_lowercase)
        .collect();
    let in_migrations = directories
        .iter()
        .any(|directory| MIGRATION_DIRECTORIES.contains(&directory.as_str()));
    // Alembic keeps them in alembic/versions.
    let alembic = directories
        .windows(2)
        .any(|pair| pair[0] == "alembic" && pair[1] == "versions");
    // Flyway's V1__name.sql and R__name.sql; golang-migrate's 1_name.up.sql;
    // Liquibase's db.changelog-1.0.xml.
    let flyway = (name.starts_with('V') || name.starts_with('R'))
        && name.contains("__")
        && lower.ends_with(".sql");
    let up_down = lower.ends_with(".up.sql") || lower.ends_with(".down.sql");
    let liquibase = lower.starts_with("db.changelog");
    in_migrations || alembic || flyway || up_down || liquibase
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileSide;

    fn file(path: &str, status: FileStatus) -> ChangedSides {
        let side = FileSide {
            text: None,
            reason: None,
            size: 0,
        };
        ChangedSides {
            path: path.to_string(),
            old_path: None,
            status,
            old: side.clone(),
            new: side,
        }
    }

    #[test]
    fn counts_files_and_tells_new_directories_from_modified_ones() {
        let files = [
            file("README.md", FileStatus::Modified),
            file("internal/refund/service.go", FileStatus::Added),
            file("internal/refund/service_test.go", FileStatus::Added),
            file("internal/payment/repo.go", FileStatus::Modified),
            file("old/gone.go", FileStatus::Deleted),
        ];
        let mut asked = Vec::new();
        let shape = shape(&files, |directories| {
            asked = directories.iter().map(|d| d.to_string()).collect();
            Ok(directories
                .iter()
                .map(|directory| *directory != "internal/refund")
                .collect())
        })
        .unwrap();
        assert_eq!(asked, ["", "internal/payment", "internal/refund", "old"]);
        assert_eq!(shape.files_added, 2);
        assert_eq!(shape.files_deleted, 1);
        assert_eq!(shape.directories_new, ["internal/refund"]);
        assert_eq!(shape.directories_modified, [".", "internal/payment", "old"]);
        assert!(shape.manifests.is_empty());
        assert!(shape.migrations.is_empty());
    }

    #[test]
    fn finds_manifests_across_languages() {
        for path in [
            "go.mod",
            "services/api/go.sum",
            "web/package.json",
            "pnpm-lock.yaml",
            "Cargo.lock",
            "requirements-dev.txt",
            "requirements/base.txt",
            "pyproject.toml",
            "Gemfile.lock",
            "app.gemspec",
            "pom.xml",
            "build.gradle.kts",
            "src/Api/Api.csproj",
            "composer.json",
            "Package.swift",
            "mix.exs",
            "pubspec.yaml",
        ] {
            assert!(is_manifest(path), "{path}");
        }
        for path in [
            "src/package.go",
            "docs/requirements.md",
            "cargo.go",
            "gomod.txt",
        ] {
            assert!(!is_manifest(path), "{path}");
        }
    }

    #[test]
    fn finds_migrations_by_where_they_live_or_how_they_are_named() {
        for path in [
            "db/migrations/0003_refunds.sql",
            "internal/storage/migrations/20261119_add_refunds.go",
            "db/migrate/20261119120000_create_refunds.rb",
            "alembic/versions/3f2a_add_refunds.py",
            "src/main/resources/db/V3__add_refunds.sql",
            "sql/000003_refunds.up.sql",
            "db/changelog/db.changelog-1.1.xml",
            "app/Migrations/Version20261119.php",
        ] {
            assert!(is_migration(path), "{path}");
        }
        for path in [
            "internal/migrator.go",
            "docs/Vision__notes.md",
            "refund/service.go",
        ] {
            assert!(!is_migration(path), "{path}");
        }
    }

    #[test]
    fn lists_the_manifests_and_migrations_touched() {
        let files = [
            file("go.mod", FileStatus::Modified),
            file("migrations/0003_refunds.sql", FileStatus::Added),
        ];
        let shape = shape(&files, |directories| Ok(vec![true; directories.len()])).unwrap();
        assert_eq!(
            shape.manifests,
            [TouchedFile {
                path: "go.mod".to_string(),
                status: FileStatus::Modified
            }]
        );
        assert_eq!(shape.migrations[0].status, FileStatus::Added);
    }
}
