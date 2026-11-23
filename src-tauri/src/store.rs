//! SQLite storage. It keeps only what git cannot tell: which folders were
//! registered and when, which branches are integration branches, when
//! DiffLens last fetched, the bases the user pinned, the state of each
//! review (`reviews`), who the user is on pull request hosts
//! (`connections`), which model reviews are read by (`model_settings`),
//! what it found in each review (`insights`), how it explained the
//! functions of each review (`explanations`) and why its tests failed
//! (`test_diagnoses`).

mod connections;
mod explanations;
mod insights;
mod jira;
mod model_settings;
mod overview;
mod reviews;
mod test_diagnoses;

pub use connections::ConnectionRecord;
pub use explanations::{BriefRecord, ExplanationRecord};
pub use insights::{AnalysisRecord, BatchRecord, FindingRecord};
pub use jira::JiraRecord;
pub use model_settings::ModelTaskRecord;
pub use overview::SummaryRecord;
pub use reviews::{CommentRecord, MarkKind, MarkRecord, PublishedAs, PulledUpdate, ReviewRecord};
pub use test_diagnoses::{confidence_of, fault_of, DiagnosisRecord, DiagnosisTranslationRecord};

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, ErrorCode as SqliteErrorCode, OptionalExtension, Row};

#[cfg(test)]
use crate::error::ErrorCode;
use crate::error::{app_error, AppResult};

/// Integration branches given to a project when it is added.
pub const DEFAULT_INTEGRATION_BRANCHES: &[&str] = &["main", "master", "develop", "release/*"];

/// Separates the entries of `projects.integration_branches`. Ref names and
/// patterns cannot contain control characters, so a newline is unambiguous.
const LIST_SEPARATOR: char = '\n';

/// Schema migrations, applied in order. Append new entries; never edit or
/// reorder the existing ones, because `PRAGMA user_version` records how many
/// of them a database has already applied.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        default_base TEXT NOT NULL,
        added_at TEXT NOT NULL
    );",
    // Bases are now found per branch among the integration branches, so the
    // single default base goes away and existing projects get the defaults.
    "ALTER TABLE projects ADD COLUMN integration_branches TEXT NOT NULL DEFAULT '';
    UPDATE projects SET integration_branches =
        'main' || char(10) || 'master' || char(10) || 'develop' || char(10) || 'release/*';
    ALTER TABLE projects DROP COLUMN default_base;
    ALTER TABLE projects ADD COLUMN last_fetched_at TEXT;
    CREATE TABLE branch_bases (
        project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
        branch TEXT NOT NULL,
        base TEXT NOT NULL,
        PRIMARY KEY (project_id, branch)
    );",
    // How the project's tests last ran ("process" or "sandbox"); NULL until
    // the first run.
    "ALTER TABLE projects ADD COLUMN test_isolation TEXT;",
    // Review state, per project and review target (see `store::reviews`).
    // `branch` and `base` are empty for the working tree, so the unique key
    // holds for it too (NULLs would all be distinct).
    "CREATE TABLE reviews (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
        target_kind TEXT NOT NULL,
        branch TEXT NOT NULL,
        base TEXT NOT NULL,
        checkpoint_sha TEXT,
        checkpoint_at TEXT,
        created_at TEXT NOT NULL,
        UNIQUE (project_id, target_kind, branch, base)
    );
    CREATE TABLE review_marks (
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        key TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        reviewed_at TEXT NOT NULL,
        PRIMARY KEY (review_id, kind, key)
    );
    CREATE TABLE review_comments (
        id TEXT PRIMARY KEY,
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        parent_id TEXT REFERENCES review_comments (id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        side TEXT NOT NULL,
        line INTEGER NOT NULL,
        line_text TEXT NOT NULL,
        commit_sha TEXT,
        body TEXT NOT NULL,
        state TEXT NOT NULL,
        remote_id TEXT,
        remote_author_id TEXT,
        remote_author_name TEXT,
        resolved INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX review_comments_by_review ON review_comments (review_id);",
    // Pull requests (see `store::connections` and `pull_requests`): who the
    // user is on each host, and what comments still owe the pull request.
    // `remote_pull_request` scopes a published comment to its pull request,
    // so one that disappears from it can be told from one of another PR.
    "CREATE TABLE connections (
        host TEXT PRIMARY KEY,
        username TEXT NOT NULL,
        account_id TEXT NOT NULL,
        account_name TEXT NOT NULL,
        connected_at TEXT NOT NULL
    );
    ALTER TABLE review_comments ADD COLUMN remote_pull_request TEXT;
    ALTER TABLE review_comments ADD COLUMN edited_locally INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE review_comments ADD COLUMN delete_pending INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE review_comments ADD COLUMN publish_error TEXT;
    CREATE UNIQUE INDEX review_comments_by_remote ON review_comments (review_id, remote_id)
        WHERE remote_id IS NOT NULL;",
    // The model (see `store::model_settings` and `ai`): app-wide settings by
    // key, such as the provider in use, and per provider the model chosen,
    // the effort and when its API key was saved. The key itself lives in the
    // Keychain only.
    "CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE model_providers (
        provider TEXT PRIMARY KEY,
        model TEXT,
        effort TEXT,
        key_saved_at TEXT
    );",
    // Insights (see `store::insights` and `insights`): each analysis of a
    // review with the head it read and who wrote it, its calls, and the
    // findings with the user's verdicts. Lists are JSON arrays of strings.
    "CREATE TABLE insight_analyses (
        id TEXT PRIMARY KEY,
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        head_sha TEXT,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX insight_analyses_by_review ON insight_analyses (review_id);
    CREATE TABLE insight_batches (
        analysis_id TEXT NOT NULL REFERENCES insight_analyses (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        paths TEXT NOT NULL,
        tokens INTEGER NOT NULL,
        status TEXT NOT NULL,
        error_code TEXT,
        error_message TEXT,
        PRIMARY KEY (analysis_id, position)
    );
    CREATE TABLE insight_findings (
        id TEXT PRIMARY KEY,
        analysis_id TEXT NOT NULL REFERENCES insight_analyses (id) ON DELETE CASCADE,
        batch INTEGER NOT NULL,
        ordinal INTEGER NOT NULL,
        title TEXT NOT NULL,
        severity TEXT NOT NULL,
        category TEXT NOT NULL,
        path TEXT NOT NULL,
        line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        explanation TEXT NOT NULL,
        suggested_direction TEXT NOT NULL,
        evidence TEXT NOT NULL,
        related TEXT NOT NULL,
        location_problem TEXT,
        cited_text TEXT NOT NULL,
        verdict TEXT NOT NULL DEFAULT 'pending',
        verdict_at TEXT,
        verdict_fingerprint TEXT
    );
    CREATE INDEX insight_findings_by_analysis ON insight_findings (analysis_id);",
    // Jira and the Overview (see `store::jira`, `store::overview`, `jira` and
    // `overview`): the one Jira Cloud connection (its token lives in the
    // Keychain only), and per review the latest summary the model wrote, with
    // the head it read. Criteria are a JSON array.
    "CREATE TABLE jira_connection (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        site TEXT NOT NULL,
        email TEXT NOT NULL,
        api_base TEXT NOT NULL,
        token_kind TEXT NOT NULL,
        account_id TEXT NOT NULL,
        account_name TEXT NOT NULL,
        connected_at TEXT NOT NULL
    );
    CREATE TABLE overview_summaries (
        review_id TEXT PRIMARY KEY REFERENCES reviews (id) ON DELETE CASCADE,
        head_sha TEXT,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL,
        condensed INTEGER NOT NULL,
        card_key TEXT,
        card_comments INTEGER NOT NULL,
        title TEXT NOT NULL,
        lead TEXT NOT NULL,
        card_summary TEXT NOT NULL,
        criteria TEXT NOT NULL
    );",
    // Languages (see `language`, `store::insights` and `store::overview`):
    // the language each analysis and summary was written in (everything
    // before was English), the translations of their prose the user asked
    // for, one per language, and the parts of a stored error the screen
    // words itself: the reason (a message key), its parameters as a JSON
    // object and the raw detail an external service gave. A translation of
    // a summary goes when a new summary replaces it.
    "ALTER TABLE insight_analyses ADD COLUMN language TEXT NOT NULL DEFAULT 'en';
    ALTER TABLE overview_summaries ADD COLUMN language TEXT NOT NULL DEFAULT 'en';
    CREATE TABLE insight_finding_translations (
        finding_id TEXT NOT NULL REFERENCES insight_findings (id) ON DELETE CASCADE,
        language TEXT NOT NULL,
        title TEXT NOT NULL,
        explanation TEXT NOT NULL,
        suggested_direction TEXT NOT NULL,
        evidence TEXT NOT NULL,
        related TEXT NOT NULL,
        PRIMARY KEY (finding_id, language)
    );
    CREATE TABLE overview_summary_translations (
        review_id TEXT NOT NULL REFERENCES overview_summaries (review_id) ON DELETE CASCADE,
        language TEXT NOT NULL,
        title TEXT NOT NULL,
        lead TEXT NOT NULL,
        card_summary TEXT NOT NULL,
        criteria TEXT NOT NULL,
        PRIMARY KEY (review_id, language)
    );
    ALTER TABLE insight_batches ADD COLUMN error_reason TEXT;
    ALTER TABLE insight_batches ADD COLUMN error_params TEXT;
    ALTER TABLE insight_batches ADD COLUMN error_detail TEXT;
    ALTER TABLE review_comments ADD COLUMN publish_error_code TEXT;
    ALTER TABLE review_comments ADD COLUMN publish_error_reason TEXT;
    ALTER TABLE review_comments ADD COLUMN publish_error_params TEXT;
    ALTER TABLE review_comments ADD COLUMN publish_error_detail TEXT;",
    // Explanations of functions (see `store::explanations` and
    // `explanations`): per review and function, the latest explanation a
    // model wrote, with a fingerprint of the function's lines when it was
    // written (to tell when its code changed since), who wrote it and in what
    // language, and the translations of its text the user asked for. A new
    // explanation of a function replaces its translations.
    "CREATE TABLE function_explanations (
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        function_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        head_sha TEXT,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL,
        language TEXT NOT NULL,
        what_changed TEXT NOT NULL,
        why TEXT NOT NULL,
        why_inferred INTEGER NOT NULL,
        purpose TEXT NOT NULL,
        PRIMARY KEY (review_id, function_id)
    );
    CREATE TABLE function_explanation_translations (
        review_id TEXT NOT NULL,
        function_id TEXT NOT NULL,
        language TEXT NOT NULL,
        what_changed TEXT NOT NULL,
        why TEXT NOT NULL,
        purpose TEXT NOT NULL,
        PRIMARY KEY (review_id, function_id, language),
        FOREIGN KEY (review_id, function_id)
            REFERENCES function_explanations (review_id, function_id) ON DELETE CASCADE
    );",
    // The structured Overview summary (see `store::overview`): besides the
    // title and a one-sentence lead, the points on how the change works and
    // the points that need the reviewer's attention, each a JSON array of
    // strings, in the summary and in each translation of it. A summary
    // written before has none: its lead is the whole paragraph.
    "ALTER TABLE overview_summaries ADD COLUMN points TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE overview_summaries ADD COLUMN attention TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE overview_summary_translations ADD COLUMN points TEXT NOT NULL DEFAULT '[]';
    ALTER TABLE overview_summary_translations ADD COLUMN attention TEXT NOT NULL DEFAULT '[]';",
    // Short explanations of functions (see `store::explanations`): beside
    // the detailed explanation in three parts kept in `function_explanations`
    // (where every explanation written before this one already is), the
    // short one a model wrote of each function, with the fingerprint of the
    // function's lines then, and the translations of it the user asked for.
    // A new short explanation of a function replaces its translations.
    "CREATE TABLE function_briefs (
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        function_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        head_sha TEXT,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL,
        language TEXT NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (review_id, function_id)
    );
    CREATE TABLE function_brief_translations (
        review_id TEXT NOT NULL,
        function_id TEXT NOT NULL,
        language TEXT NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (review_id, function_id, language),
        FOREIGN KEY (review_id, function_id)
            REFERENCES function_briefs (review_id, function_id) ON DELETE CASCADE
    );",
    // A model per task (see `store::model_settings`): the provider, model and
    // effort chosen for a task instead of the default model. A task without
    // a row, as every task of a database before this one, uses the default.
    "CREATE TABLE model_tasks (
        task TEXT PRIMARY KEY,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        effort TEXT
    );",
    // Explanations of failed tests (see `store::test_diagnoses` and
    // `test_diagnosis`): per review and failing test (its package and
    // top-level name, empty for a package that failed outside its tests),
    // the latest explanation a model wrote, with when the run it explained
    // started and fingerprints of the failure and of the code the model was
    // shown (to tell when either changed since), who wrote it and in what
    // language, and the translations of its prose the user asked for. A new
    // explanation of a failure replaces its translations.
    "CREATE TABLE test_diagnoses (
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        package TEXT NOT NULL,
        test TEXT NOT NULL,
        run_started_at TEXT NOT NULL,
        failure_fingerprint TEXT NOT NULL,
        code_fingerprint TEXT NOT NULL,
        head_sha TEXT,
        provider TEXT NOT NULL,
        model TEXT NOT NULL,
        created_at TEXT NOT NULL,
        language TEXT NOT NULL,
        cause TEXT NOT NULL,
        fault TEXT NOT NULL,
        confidence TEXT NOT NULL,
        location_path TEXT,
        location_line INTEGER,
        fix TEXT NOT NULL,
        fix_code TEXT NOT NULL,
        PRIMARY KEY (review_id, package, test)
    );
    CREATE TABLE test_diagnosis_translations (
        review_id TEXT NOT NULL,
        package TEXT NOT NULL,
        test TEXT NOT NULL,
        language TEXT NOT NULL,
        cause TEXT NOT NULL,
        fix TEXT NOT NULL,
        PRIMARY KEY (review_id, package, test, language),
        FOREIGN KEY (review_id, package, test)
            REFERENCES test_diagnoses (review_id, package, test) ON DELETE CASCADE
    );",
    // General comments (see `store::reviews`): a comment on the pull request
    // as a whole is of the kind 'general', with no file or line (an empty
    // path and line text, line 0); every comment kept before is on a line.
    // An explanation of a failure keeps the review comment written from it
    // until that comment is deleted, and, for writing one, the code around
    // the line it names as the model was shown it and the kind of failure
    // (both NULL for an explanation written before).
    "ALTER TABLE review_comments ADD COLUMN kind TEXT NOT NULL DEFAULT 'line';
    ALTER TABLE test_diagnoses ADD COLUMN comment_id TEXT
        REFERENCES review_comments (id) ON DELETE SET NULL;
    ALTER TABLE test_diagnoses ADD COLUMN code_excerpt TEXT;
    ALTER TABLE test_diagnoses ADD COLUMN failure_kind TEXT;",
    // Drafts written from a signal (see `store::reviews` and `signals`): the
    // review comment written from a model finding or a static check finding,
    // by the signal's key (`finding:<id>`, `check:<tool>:<rule>:<path>:
    // <line>:<message>`), until that comment is deleted.
    "CREATE TABLE signal_comments (
        review_id TEXT NOT NULL REFERENCES reviews (id) ON DELETE CASCADE,
        signal TEXT NOT NULL,
        comment_id TEXT NOT NULL REFERENCES review_comments (id) ON DELETE CASCADE,
        PRIMARY KEY (review_id, signal)
    );",
    // Fixed findings (see `store::insights` and `insights`): a finding a
    // later analysis re-checked and found fixed, or whose file left the
    // change, or that the user marked fixed, keeps how (`resolution`, NULL
    // while it is open), when, the model that said so and its reason. Every
    // finding kept before is open.
    "ALTER TABLE insight_findings ADD COLUMN resolution TEXT;
    ALTER TABLE insight_findings ADD COLUMN resolved_at TEXT;
    ALTER TABLE insight_findings ADD COLUMN resolved_model TEXT;
    ALTER TABLE insight_findings ADD COLUMN resolution_reason TEXT;",
];

const PROJECT_COLUMNS: &str = "id, path, integration_branches, last_fetched_at, added_at";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub path: String,
    /// Names or glob patterns, in the user's order of preference.
    pub integration_branches: Vec<String>,
    pub last_fetched_at: Option<String>,
    pub added_at: String,
}

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(file: &Path) -> AppResult<Self> {
        Self::from_connection(Connection::open(file)?)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> AppResult<Self> {
        // Off by default in SQLite; pinned bases go with their project.
        connection.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn insert(&self, record: &ProjectRecord) -> AppResult<()> {
        let result = self.lock()?.execute(
            "INSERT INTO projects (id, path, integration_branches, last_fetched_at, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.path,
                join_list(&record.integration_branches),
                record.last_fetched_at,
                record.added_at
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == SqliteErrorCode::ConstraintViolation =>
            {
                Err(app_error!(
                    AlreadyAdded,
                    AlreadyAdded,
                    "{path} is already a DiffLens project.",
                    path = &record.path
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Newest first.
    pub fn list(&self) -> AppResult<Vec<ProjectRecord>> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY added_at DESC, rowid DESC"
        ))?;
        let records = statement
            .query_map([], read_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn get(&self, id: &str) -> AppResult<Option<ProjectRecord>> {
        self.find_one("id", id)
    }

    pub fn find_by_path(&self, path: &str) -> AppResult<Option<ProjectRecord>> {
        self.find_one("path", path)
    }

    /// Returns false when no project has that id. Pinned bases go with it.
    pub fn delete(&self, id: &str) -> AppResult<bool> {
        let deleted = self
            .lock()?
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    pub fn set_last_fetched_at(&self, id: &str, fetched_at: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE projects SET last_fetched_at = ?2 WHERE id = ?1",
            params![id, fetched_at],
        )?;
        Ok(())
    }

    /// Bases the user pinned in a project, by branch name.
    pub fn branch_bases(&self, project_id: &str) -> AppResult<HashMap<String, String>> {
        let connection = self.lock()?;
        let mut statement =
            connection.prepare("SELECT branch, base FROM branch_bases WHERE project_id = ?1")?;
        let pins = statement
            .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(pins)
    }

    /// Pins the base of `branch`, or with `None` removes the pin.
    pub fn set_branch_base(
        &self,
        project_id: &str,
        branch: &str,
        base: Option<&str>,
    ) -> AppResult<()> {
        let connection = self.lock()?;
        match base {
            Some(base) => connection.execute(
                "INSERT INTO branch_bases (project_id, branch, base) VALUES (?1, ?2, ?3)
                 ON CONFLICT (project_id, branch) DO UPDATE SET base = excluded.base",
                params![project_id, branch, base],
            )?,
            None => connection.execute(
                "DELETE FROM branch_bases WHERE project_id = ?1 AND branch = ?2",
                params![project_id, branch],
            )?,
        };
        Ok(())
    }

    /// How the project's tests ran last, `None` before the first run or when
    /// no project has that id.
    pub fn test_isolation(&self, project_id: &str) -> AppResult<Option<String>> {
        let isolation = self
            .lock()?
            .query_row(
                "SELECT test_isolation FROM projects WHERE id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(isolation.flatten())
    }

    pub fn set_test_isolation(&self, project_id: &str, isolation: &str) -> AppResult<()> {
        self.lock()?.execute(
            "UPDATE projects SET test_isolation = ?2 WHERE id = ?1",
            params![project_id, isolation],
        )?;
        Ok(())
    }

    fn find_one(&self, column: &str, value: &str) -> AppResult<Option<ProjectRecord>> {
        let record = self
            .lock()?
            .query_row(
                &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE {column} = ?1"),
                params![value],
                read_record,
            )
            .optional()?;
        Ok(record)
    }

    fn lock(&self) -> AppResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| {
                app_error!(
                    Internal,
                    StorageUnavailable,
                    "Project storage is unavailable after a failure."
                )
            })
    }
}

fn read_record(row: &Row<'_>) -> rusqlite::Result<ProjectRecord> {
    let integration_branches: String = row.get(2)?;
    Ok(ProjectRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        integration_branches: split_list(&integration_branches),
        last_fetched_at: row.get(3)?,
        added_at: row.get(4)?,
    })
}

fn join_list(items: &[String]) -> String {
    items.join(&LIST_SEPARATOR.to_string())
}

fn split_list(text: &str) -> Vec<String> {
    text.split(LIST_SEPARATOR)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn migrate(connection: &mut Connection) -> AppResult<()> {
    let applied: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let applied = usize::try_from(applied).unwrap_or(0);
    if applied > MIGRATIONS.len() {
        return Err(app_error!(
            Internal,
            DatabaseTooNew,
            "The project database was created by a newer version of DiffLens."
        ));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", index + 1)?;
        transaction.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<String> {
        DEFAULT_INTEGRATION_BRANCHES
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    fn record(id: &str, path: &str, added_at: &str) -> ProjectRecord {
        ProjectRecord {
            id: id.to_string(),
            path: path.to_string(),
            integration_branches: defaults(),
            last_fetched_at: None,
            added_at: added_at.to_string(),
        }
    }

    #[test]
    fn stores_and_reads_back_a_project() {
        let store = Store::open_in_memory().unwrap();
        let mut project = record("a", "/repos/a", "2026-01-01T00:00:00Z");
        project.integration_branches = vec!["trunk".to_string(), "rel/*".to_string()];
        store.insert(&project).unwrap();

        assert_eq!(store.get("a").unwrap(), Some(project.clone()));
        assert_eq!(store.find_by_path("/repos/a").unwrap(), Some(project));
        assert_eq!(store.get("missing").unwrap(), None);
    }

    #[test]
    fn lists_newest_first_and_breaks_ties_by_insertion() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("old", "/repos/old", "2026-01-01T00:00:00Z"))
            .unwrap();
        store
            .insert(&record("tie-1", "/repos/tie-1", "2026-02-01T00:00:00Z"))
            .unwrap();
        store
            .insert(&record("tie-2", "/repos/tie-2", "2026-02-01T00:00:00Z"))
            .unwrap();

        let ids: Vec<String> = store.list().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["tie-2", "tie-1", "old"]);
    }

    #[test]
    fn rejects_a_path_that_is_already_stored() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
            .unwrap();

        let error = store
            .insert(&record("b", "/repos/a", "2026-01-02T00:00:00Z"))
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::AlreadyAdded);
    }

    #[test]
    fn delete_reports_whether_the_project_existed() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
            .unwrap();

        assert!(store.delete("a").unwrap());
        assert!(!store.delete("a").unwrap());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn records_the_last_fetch() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
            .unwrap();

        store
            .set_last_fetched_at("a", "2026-03-01T10:00:00Z")
            .unwrap();

        assert_eq!(
            store.get("a").unwrap().unwrap().last_fetched_at.as_deref(),
            Some("2026-03-01T10:00:00Z")
        );
    }

    #[test]
    fn pins_replaces_and_removes_branch_bases_per_project() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
            .unwrap();
        store
            .insert(&record("b", "/repos/b", "2026-01-01T00:00:00Z"))
            .unwrap();

        store
            .set_branch_base("a", "feature/x", Some("develop"))
            .unwrap();
        store
            .set_branch_base("a", "feature/x", Some("master"))
            .unwrap();
        store
            .set_branch_base("a", "feature/y", Some("develop"))
            .unwrap();
        store
            .set_branch_base("b", "feature/x", Some("main"))
            .unwrap();

        let pins = store.branch_bases("a").unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins["feature/x"], "master");
        assert_eq!(pins["feature/y"], "develop");

        store.set_branch_base("a", "feature/x", None).unwrap();
        store.set_branch_base("a", "never pinned", None).unwrap();
        assert_eq!(store.branch_bases("a").unwrap().len(), 1);

        store.delete("a").unwrap();
        assert!(store.branch_bases("a").unwrap().is_empty());
        assert_eq!(store.branch_bases("b").unwrap()["feature/x"], "main");
    }

    #[test]
    fn remembers_the_test_isolation_per_project() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
            .unwrap();
        store
            .insert(&record("b", "/repos/b", "2026-01-01T00:00:00Z"))
            .unwrap();

        assert_eq!(store.test_isolation("a").unwrap(), None);
        store.set_test_isolation("a", "process").unwrap();
        assert_eq!(
            store.test_isolation("a").unwrap().as_deref(),
            Some("process")
        );
        assert_eq!(store.test_isolation("b").unwrap(), None);
        assert_eq!(store.test_isolation("missing").unwrap(), None);
    }

    #[test]
    fn reopening_a_database_keeps_data_and_does_not_rerun_migrations() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        {
            let store = Store::open(&file).unwrap();
            store
                .insert(&record("a", "/repos/a", "2026-01-01T00:00:00Z"))
                .unwrap();
        }

        let store = Store::open(&file).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        let version: i64 = store
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn migrates_a_database_of_the_first_schema() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        {
            let connection = Connection::open(&file).unwrap();
            connection.execute_batch(MIGRATIONS[0]).unwrap();
            connection.pragma_update(None, "user_version", 1).unwrap();
            connection
                .execute(
                    "INSERT INTO projects (id, path, default_base, added_at)
                     VALUES ('old', '/repos/old', 'trunk', '2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();
        }

        let store = Store::open(&file).unwrap();

        assert_eq!(
            store.get("old").unwrap(),
            Some(ProjectRecord {
                id: "old".to_string(),
                path: "/repos/old".to_string(),
                integration_branches: defaults(),
                last_fetched_at: None,
                added_at: "2026-01-01T00:00:00Z".to_string(),
            })
        );
        store
            .set_branch_base("old", "feature/x", Some("develop"))
            .unwrap();
        assert_eq!(store.branch_bases("old").unwrap()["feature/x"], "develop");
        // A pin needs an existing project.
        assert!(store
            .set_branch_base("missing", "feature/x", Some("develop"))
            .is_err());
    }

    #[test]
    fn comments_of_the_fourth_schema_owe_the_pull_request_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        {
            let connection = Connection::open(&file).unwrap();
            for migration in &MIGRATIONS[..4] {
                connection.execute_batch(migration).unwrap();
            }
            connection.pragma_update(None, "user_version", 4).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO projects (id, path, added_at, integration_branches)
                         VALUES ('p', '/repos/p', 't', 'main');
                     INSERT INTO reviews (id, project_id, target_kind, branch, base, created_at)
                         VALUES ('r', 'p', 'branch', 'feature', 'main', 't');
                     INSERT INTO review_comments (id, review_id, path, side, line, line_text,
                         body, state, created_at, updated_at)
                         VALUES ('c', 'r', 'a.go', 'new', 1, 'x', 'Kept', 'draft', 't', 't');",
                )
                .unwrap();
        }

        let store = Store::open(&file).unwrap();
        let comment = store.comment("p", "c").unwrap().unwrap();
        assert_eq!(comment.body, "Kept");
        assert!(!comment.edited_locally && !comment.delete_pending);
        assert_eq!(
            (comment.remote_pull_request, comment.publish_error),
            (None, None)
        );
        assert_eq!(store.connection("bitbucket").unwrap(), None);
    }

    #[test]
    fn comments_and_explanations_kept_before_general_comments_stay_as_they_were() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        let before = MIGRATIONS.len() - 1;
        {
            let connection = Connection::open(&file).unwrap();
            for migration in &MIGRATIONS[..before] {
                connection.execute_batch(migration).unwrap();
            }
            connection.pragma_update(None, "user_version", before).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO projects (id, path, added_at, integration_branches)
                         VALUES ('p', '/repos/p', 't', 'main');
                     INSERT INTO reviews (id, project_id, target_kind, branch, base, created_at)
                         VALUES ('r', 'p', 'branch', 'feature', 'main', 't');
                     INSERT INTO review_comments (id, review_id, path, side, line, line_text,
                         body, state, created_at, updated_at)
                         VALUES ('c', 'r', 'a.go', 'new', 4, 'x', 'Kept', 'draft', 't', 't');
                     INSERT INTO test_diagnoses (review_id, package, test, run_started_at,
                         failure_fingerprint, code_fingerprint, provider, model, created_at,
                         language, cause, fault, confidence, fix, fix_code)
                         VALUES ('r', 'app/auth', 'TestSignUp', 't', 'f', 'c', 'anthropic',
                         'claude-opus-5-5', 't', 'en', 'A race.', 'code', 'high', '', '');",
                )
                .unwrap();
        }

        let store = Store::open(&file).unwrap();
        let comment = store.comment("p", "c").unwrap().unwrap();
        assert_eq!(comment.kind, crate::model::CommentKind::Line);
        assert_eq!((comment.path.as_str(), comment.line, comment.body.as_str()), ("a.go", 4, "Kept"));
        let diagnoses = store.diagnoses("r").unwrap();
        assert_eq!(diagnoses[0].cause, "A race.");
        assert_eq!(store.diagnosis_comment("r", "app/auth", "TestSignUp").unwrap(), None);
        assert_eq!((diagnoses[0].code_excerpt.as_deref(), diagnoses[0].failure_kind.as_deref()), (None, None));
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("difflens.db");
        {
            let connection = Connection::open(&file).unwrap();
            connection
                .pragma_update(None, "user_version", MIGRATIONS.len() + 1)
                .unwrap();
        }

        let error = Store::open(&file).err().unwrap();
        assert_eq!(error.code, ErrorCode::Internal);
    }
}
