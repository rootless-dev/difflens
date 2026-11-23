use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

mod reasons;

pub use reasons::Reason;

/// Error codes of the frontend contract (`ErrorCode` in `src/lib/api.ts`).
/// Read back too, from the error a failed Insights call was stored with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PathNotFound,
    NotAGitRepository,
    AlreadyAdded,
    DestinationExists,
    CloneFailed,
    FetchFailed,
    NoRemote,
    GitNotFound,
    UnknownProject,
    UnknownBranch,
    NoCommonHistory,
    UnknownFile,
    UnknownComment,
    /// The finding, or the analysis or call it belongs to, is gone: a newer
    /// analysis replaced it.
    UnknownFinding,
    /// No credential is saved for the pull request host.
    NotConnected,
    /// The host refused the credential: wrong, expired or revoked token.
    AuthFailed,
    /// The credential works but lacks a permission (API token scope).
    MissingScope,
    /// The host does not know the repository, pull request or comment, or the
    /// account cannot see it.
    RemoteNotFound,
    /// The host is limiting requests.
    RateLimited,
    /// The host could not be reached at all.
    NetworkFailed,
    /// The host answered with an error of its own.
    RemoteFailed,
    /// Publishing is not possible now; the message says why.
    PublishBlocked,
    /// The chosen model provider lacks what it needs to be asked: an API key
    /// or a model.
    ModelNotConfigured,
    /// The model declined to answer; the message says why when it knows.
    ModelRefused,
    /// The answer was cut off before it was complete (the output limit, or
    /// the model's context window).
    ModelTruncated,
    /// The model answered, but not with JSON matching the schema it was
    /// given.
    ModelInvalidOutput,
    /// The `claude` command of Claude Code was not found on this machine.
    ClaudeCliNotFound,
    /// The `claude` command ran but failed; the message carries its reason.
    ClaudeCliFailed,
    /// The user stopped the operation.
    Cancelled,
    /// The branch of a review is checked out in no folder yet, so a terminal
    /// has nowhere to open until a worktree is created for it.
    NoFolder,
    /// The path is not a worktree DiffLens created in the project.
    UnknownWorktree,
    /// The worktree has uncommitted changes; removing it needs `force`.
    WorktreeDirty,
    /// A terminal session is open in the worktree.
    TerminalsOpen,
    /// No terminal session has that id: it was closed.
    UnknownTerminal,
    Internal,
}

/// What fills a message: a value shown as it is (a path, a branch, text a
/// service wrote), a number (a count, a status code), or a sentence of its
/// own that the frontend words too. Serialised as a string, a number or a
/// `{ message, reason?, params? }` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Param {
    Number(i64),
    Text(String),
    Message(Box<CoreText>),
}

pub type Params = BTreeMap<String, Param>;

impl From<String> for Param {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Param {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<&String> for Param {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<&&str> for Param {
    fn from(text: &&str) -> Self {
        Self::Text((*text).to_string())
    }
}

impl From<Cow<'_, str>> for Param {
    fn from(text: Cow<'_, str>) -> Self {
        Self::Text(text.into_owned())
    }
}

impl From<std::path::Display<'_>> for Param {
    fn from(path: std::path::Display<'_>) -> Self {
        Self::Text(path.to_string())
    }
}

/// Text from elsewhere is kept as a plain value.
impl From<CoreText> for Param {
    fn from(text: CoreText) -> Self {
        if text.reason.is_none() {
            Self::Text(text.message)
        } else {
            Self::Message(Box::new(text))
        }
    }
}

macro_rules! number_params {
    ($($number:ty),*) => {
        $(impl From<$number> for Param {
            fn from(number: $number) -> Self {
                Self::Number(i64::try_from(number).unwrap_or(i64::MAX))
            }
        })*
    };
}

number_params!(u8, u16, u32, u64, usize, i32, i64);

/// Errors of other libraries, kept as their own text.
macro_rules! text_params {
    ($($error:ty),*) => {
        $(impl From<$error> for Param {
            fn from(error: $error) -> Self {
                Self::Text(error.to_string())
            }
        }
        impl From<&$error> for Param {
            fn from(error: &$error) -> Self {
                Self::Text(error.to_string())
            }
        })*
    };
}

text_params!(
    std::io::Error,
    serde_json::Error,
    rusqlite::Error,
    keyring::Error,
    reqwest::Error,
    time::error::Format,
    tauri::Error
);

/// A sentence the core composes for the user, worded by the frontend in the
/// language shown: `reason` names which sentence, `params` fill it, and
/// `message` is the English (logs, tests, and what is shown when the
/// frontend does not know the reason). Without a reason it is text from
/// elsewhere, shown as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreText {
    pub message: String,
    pub reason: Option<Reason>,
    pub params: Params,
}

/// Text from elsewhere is a plain string, as such fields were before they
/// could be worded; a worded one is `{ message, reason, params? }`.
impl Serialize for CoreText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        if self.reason.is_none() && self.params.is_empty() {
            return serializer.serialize_str(&self.message);
        }
        let fields = 1 + usize::from(self.reason.is_some()) + usize::from(!self.params.is_empty());
        let mut text = serializer.serialize_struct("CoreText", fields)?;
        text.serialize_field("message", &self.message)?;
        if let Some(reason) = &self.reason {
            text.serialize_field("reason", reason)?;
        }
        if !self.params.is_empty() {
            text.serialize_field("params", &self.params)?;
        }
        text.end()
    }
}

/// Read back from its object, or from a plain string (text kept before it
/// had a reason, such as the message of a stored test run).
impl<'de> Deserialize<'de> for CoreText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Worded {
            message: String,
            #[serde(default)]
            reason: Option<Reason>,
            #[serde(default)]
            params: Params,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Worded(Worded),
            Plain(String),
        }

        Ok(match Stored::deserialize(deserializer)? {
            Stored::Worded(text) => Self {
                message: text.message,
                reason: text.reason,
                params: text.params,
            },
            Stored::Plain(message) => Self::raw(message),
        })
    }
}

impl CoreText {
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: Some(reason),
            params: Params::new(),
        }
    }

    /// Text the core did not write (a service's or a tool's own words).
    pub fn raw(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            params: Params::new(),
        }
    }

    #[must_use]
    pub fn param(mut self, name: &str, value: impl Into<Param>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }

    /// Used by `core_text!`, which fills every named argument the same way.
    #[must_use]
    pub fn arg(self, name: &str, value: impl Into<Param>) -> Self {
        self.param(name, value)
    }

    /// The same text with a full stop at the end when it has no closing
    /// punctuation, so a detail reads as a sentence. Text from elsewhere
    /// gets the full stop itself; a worded text is wrapped in the
    /// `sentence` reason, which adds it in every language.
    #[must_use]
    pub fn sentence(self) -> Self {
        let trimmed = self.message.trim();
        if trimmed.ends_with(['.', '!', '?']) {
            if trimmed.len() == self.message.len() {
                return self;
            }
            return Self {
                message: trimmed.to_string(),
                ..self
            };
        }
        if self.reason.is_none() {
            return Self::raw(format!("{trimmed}."));
        }
        let message = format!("{trimmed}.");
        Self::new(Reason::Sentence, message)
            .param("text", self)
            .verified()
    }

    /// Checked against the English catalog in tests; nothing otherwise.
    #[must_use]
    pub fn verified(self) -> Self {
        #[cfg(test)]
        catalog::check(self.reason, &self.params, None, &self.message);
        self
    }
}

/// Reads as its English message, as tests and logs read it.
impl std::ops::Deref for CoreText {
    type Target = str;

    fn deref(&self) -> &str {
        &self.message
    }
}

/// Compares the English message, as tests and logs read it.
impl PartialEq<str> for CoreText {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for CoreText {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

/// Text from elsewhere, shown as it is.
impl From<&str> for CoreText {
    fn from(text: &str) -> Self {
        Self::raw(text)
    }
}

impl From<String> for CoreText {
    fn from(text: String) -> Self {
        Self::raw(text)
    }
}

impl From<&CoreText> for CoreText {
    fn from(text: &CoreText) -> Self {
        text.clone()
    }
}

impl fmt::Display for CoreText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The single error type of the core. It serialises to the `{ code, message,
/// reason?, params?, detail? }` shape every rejected command returns:
/// `reason` names the message (finer than `code`), `params` fill it, and
/// `detail` is text from elsewhere (a host's or git's own words) the message
/// shows as it is. `message` is the same message in English.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: Params,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// An error worded by `reason`; `message` is its English. Most call
    /// sites use `app_error!`, which also fills the parameters.
    pub fn new(code: ErrorCode, reason: Reason, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason: Some(reason),
            params: Params::new(),
            detail: None,
        }
    }

    pub fn internal(reason: Reason, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, reason, message)
    }

    /// An error known only by its English text: one stored before errors
    /// had reasons.
    pub fn untranslated(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            reason: None,
            params: Params::new(),
            detail: None,
        }
    }

    #[must_use]
    pub fn param(mut self, name: &str, value: impl Into<Param>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Used by `app_error!`: the argument named `detail` is the detail, the
    /// others are parameters. A detail the core worded itself (such as "git
    /// reported no details.") stays a parameter, so it is translated too.
    #[must_use]
    pub fn arg(self, name: &str, value: impl Into<Param>) -> Self {
        if name != "detail" {
            return self.param(name, value);
        }
        match value.into() {
            Param::Text(text) => self.detail(text),
            Param::Number(number) => self.detail(number.to_string()),
            Param::Message(text) if text.reason.is_none() => self.detail(text.message),
            worded => self.param(name, worded),
        }
    }

    /// This error's message as a sentence that can fill another message
    /// (its detail, if any, goes along as a parameter).
    pub fn text(&self) -> CoreText {
        let mut params = self.params.clone();
        if let Some(detail) = &self.detail {
            params.insert("detail".to_string(), Param::Text(detail.clone()));
        }
        CoreText {
            message: self.message.clone(),
            reason: self.reason,
            params,
        }
    }

    /// Checked against the English catalog in tests; nothing otherwise.
    #[must_use]
    pub fn verified(self) -> Self {
        #[cfg(test)]
        catalog::check(
            self.reason,
            &self.params,
            self.detail.as_deref(),
            &self.message,
        );
        self
    }

    /// The parameters as the JSON object text they are stored as; None when
    /// there are none.
    pub fn params_json(&self) -> Option<String> {
        if self.params.is_empty() {
            return None;
        }
        serde_json::to_string(&self.params).ok()
    }

    /// An error as it was stored: its reason and parameters only when both
    /// read back (a row written before errors had reasons has neither, and
    /// is shown by its English message).
    pub fn restore(
        code: ErrorCode,
        message: String,
        reason: Option<&str>,
        params: Option<&str>,
        detail: Option<String>,
    ) -> Self {
        let reason = reason.and_then(Reason::from_key);
        let params = match params {
            None => Some(Params::new()),
            Some(json) => serde_json::from_str::<Params>(json).ok(),
        };
        match (reason, params) {
            (Some(reason), Some(params)) => Self {
                code,
                message,
                reason: Some(reason),
                params,
                detail,
            },
            _ => Self::untranslated(code, message),
        }
    }
}

impl ErrorCode {
    /// The contract's spelling, such as "rate_limited", as errors are stored.
    pub fn key(self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(key)) => key,
            _ => String::new(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(key.to_string())).ok()
    }
}

/// An error worded by a reason: `app_error!(Code, Reason, "English with
/// {name}", name = value, ...)`. The English is formatted from the named
/// arguments, which become the parameters (`detail` becomes the detail), so
/// the frontend's catalog entry for the reason is the same text with
/// `{{name}}`. In tests, the English is checked against that catalog entry.
macro_rules! app_error {
    ($code:ident, $reason:ident, $english:literal $(,)?) => {
        $crate::error::AppError::new(
            $crate::error::ErrorCode::$code,
            $crate::error::Reason::$reason,
            $english,
        )
        .verified()
    };
    ($code:ident, $reason:ident, $english:literal $(, $name:ident = $value:expr)+ $(,)?) => {{
        // Each value is computed once, then both formatted and kept.
        let ($($name,)+) = ($($value,)+);
        let error = $crate::error::AppError::new(
            $crate::error::ErrorCode::$code,
            $crate::error::Reason::$reason,
            format!($english $(, $name = $name)*),
        );
        $(let error = error.arg(stringify!($name), $name);)*
        error.verified()
    }};
}

/// A sentence worded by a reason, as `app_error!` without a code.
macro_rules! core_text {
    ($reason:ident, $english:literal $(,)?) => {
        $crate::error::CoreText::new($crate::error::Reason::$reason, $english).verified()
    };
    ($reason:ident, $english:literal $(, $name:ident = $value:expr)+ $(,)?) => {{
        let ($($name,)+) = ($($value,)+);
        let text = $crate::error::CoreText::new(
            $crate::error::Reason::$reason,
            format!($english $(, $name = $name)*),
        );
        $(let text = text.arg(stringify!($name), $name);)*
        text.verified()
    }};
}

pub(crate) use app_error;
pub(crate) use core_text;

/// `text` as `{:?}` writes it, without the quotes around it: a value a
/// message quotes (`"\"{path}\" is not..."`), with what cannot be shown
/// escaped (`\0`, `\n`).
pub fn escaped(text: &str) -> String {
    let debug = format!("{text:?}");
    debug[1..debug.len() - 1].to_string()
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        app_error!(
            Internal,
            StorageFailed,
            "Project storage failed: {detail}",
            detail = error
        )
    }
}

/// The English catalog of the frontend (`src/i18n/locales/en/errors.ts`),
/// read by tests: every reason must have an entry, and every message the
/// core words must read exactly as the entry fills in.
#[cfg(test)]
pub mod catalog {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    use super::{Param, Params, Reason};

    pub const PATH: &str = "../src/i18n/locales/en/errors.ts";

    /// Every `key: "text"` entry of the reasons, by key (plural forms keep
    /// their `_one` / `_other` suffix).
    pub fn entries() -> &'static HashMap<String, String> {
        static ENTRIES: OnceLock<HashMap<String, String>> = OnceLock::new();
        ENTRIES.get_or_init(|| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PATH);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let start = source
                .find("reasons: {")
                .expect("the English errors catalog has a reasons object");
            parse_entries(&source[start + "reasons: {".len()..])
        })
    }

    /// Reads `key: "text",` pairs (single or double quotes; the text may
    /// start on the next line) until the object closes.
    fn parse_entries(source: &str) -> HashMap<String, String> {
        let mut entries = HashMap::new();
        let mut rest = source.trim_start();
        while !rest.starts_with('}') && !rest.is_empty() {
            if let Some(comment) = rest.strip_prefix("//") {
                rest = comment.split_once('\n').map_or("", |(_, after)| after).trim_start();
                continue;
            }
            let (key, after) = rest
                .split_once(':')
                .unwrap_or_else(|| panic!("expected a key at {:?}", &rest[..rest.len().min(60)]));
            let mut text = String::new();
            let mut chars = after.trim_start().char_indices();
            let quote = match chars.next() {
                Some((_, quote @ ('"' | '\''))) => quote,
                _ => panic!("the entry {key:?} is not a quoted string"),
            };
            let mut end = 0;
            while let Some((index, char)) = chars.next() {
                match char {
                    '\\' => match chars.next() {
                        Some((_, 'n')) => text.push('\n'),
                        Some((_, escaped)) => text.push(escaped),
                        None => break,
                    },
                    closing if closing == quote => {
                        end = index + 1;
                        break;
                    }
                    other => text.push(other),
                }
            }
            entries.insert(key.trim().to_string(), text);
            rest = after.trim_start()[end..]
                .trim_start()
                .trim_start_matches(',')
                .trim_start();
        }
        entries
    }

    /// The entry `reason` fills in with `params` and `detail`, as the
    /// frontend does: plural forms by `count`, nested sentences worded the
    /// same way.
    pub fn render(reason: Reason, params: &Params, detail: Option<&str>) -> String {
        let entries = entries();
        let key = reason.key();
        let plural = match params.get("count") {
            Some(Param::Number(1)) => format!("{key}_one"),
            Some(Param::Number(_)) => format!("{key}_other"),
            _ => key.to_string(),
        };
        let template = entries
            .get(&plural)
            .or_else(|| entries.get(key))
            .unwrap_or_else(|| panic!("the English catalog has no entry for {key:?}"));
        let mut text = String::new();
        let mut rest = template.as_str();
        while let Some(start) = rest.find("{{") {
            text.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let end = after.find("}}").expect("a placeholder closes");
            let name = after[..end].trim();
            let value = match (name, params.get(name)) {
                (_, Some(param)) => param_text(param),
                ("detail", None) => detail
                    .unwrap_or_else(|| panic!("{key:?} shows a detail it was not given"))
                    .to_string(),
                (_, None) => panic!("{key:?} uses {name:?}, which it was not given"),
            };
            text.push_str(&value);
            rest = &after[end + 2..];
        }
        text.push_str(rest);
        text
    }

    fn param_text(param: &Param) -> String {
        match param {
            Param::Number(number) => number.to_string(),
            Param::Text(text) => text.clone(),
            Param::Message(text) => match text.reason {
                Some(reason) => render(reason, &text.params, None),
                None => text.message.clone(),
            },
        }
    }

    pub fn check(reason: Option<Reason>, params: &Params, detail: Option<&str>, message: &str) {
        if let Some(reason) = reason {
            let rendered = render(reason, params, detail);
            assert_eq!(
                rendered,
                message,
                "the English catalog entry for {:?} does not read as the core's message",
                reason.key()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_to_the_contract_shape() {
        let error = AppError::untranslated(ErrorCode::NotAGitRepository, "nope");
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "code": "not_a_git_repository", "message": "nope" })
        );
    }

    #[test]
    fn serialises_its_reason_params_and_detail() {
        let cause = core_text!(TransportTimeout, "it did not answer in time");
        let error = app_error!(
            CloneFailed,
            CloneFailed,
            "Could not clone {url}: {detail}",
            url = "git@example.com:acme/api.git",
            detail = "fatal: repository not found"
        )
        .param("cause", cause)
        .param("count", 3_u32);
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "clone_failed",
                "message": "Could not clone git@example.com:acme/api.git: fatal: repository not found",
                "reason": "clone_failed",
                "params": {
                    "url": "git@example.com:acme/api.git",
                    "cause": { "message": "it did not answer in time", "reason": "transport_timeout" },
                    "count": 3
                },
                "detail": "fatal: repository not found"
            })
        );
    }

    #[test]
    fn params_read_back_as_they_were_written() {
        let params: Params = [
            ("path".to_string(), Param::from("/tmp/a")),
            ("count".to_string(), Param::from(2_u64)),
            (
                "cause".to_string(),
                Param::from(core_text!(TransportTimeout, "it did not answer in time").sentence()),
            ),
            ("said".to_string(), Param::from(CoreText::raw("nope"))),
        ]
        .into();
        let json = serde_json::to_string(&params).unwrap();
        assert_eq!(serde_json::from_str::<Params>(&json).unwrap(), params);
    }

    #[test]
    fn text_from_elsewhere_is_a_plain_string_and_reads_back_from_one() {
        let raw = CoreText::raw("exit status 2");
        assert_eq!(serde_json::to_value(&raw).unwrap(), "exit status 2");
        // What a test run record kept before messages had reasons.
        let stored: CoreText = serde_json::from_value(serde_json::json!("boom")).unwrap();
        assert_eq!(stored, CoreText::raw("boom"));

        let worded = core_text!(
            AnalyzerNotInstalled,
            "{analyzer} is not installed; run pnpm analyzers:build.",
            analyzer = crate::analyzers::REGISTRY[0].title()
        );
        let json = serde_json::to_value(&worded).unwrap();
        assert_eq!(json["reason"], "analyzer_not_installed");
        assert_eq!(json["params"]["analyzer"]["reason"], "analyzer_title");
        assert_eq!(json["params"]["analyzer"]["params"]["language"], "Go");
        assert_eq!(serde_json::from_value::<CoreText>(json).unwrap(), worded);
    }

    #[test]
    fn every_code_matches_the_contract_spelling() {
        let codes = [
            (ErrorCode::PathNotFound, "path_not_found"),
            (ErrorCode::NotAGitRepository, "not_a_git_repository"),
            (ErrorCode::AlreadyAdded, "already_added"),
            (ErrorCode::DestinationExists, "destination_exists"),
            (ErrorCode::CloneFailed, "clone_failed"),
            (ErrorCode::FetchFailed, "fetch_failed"),
            (ErrorCode::NoRemote, "no_remote"),
            (ErrorCode::GitNotFound, "git_not_found"),
            (ErrorCode::UnknownProject, "unknown_project"),
            (ErrorCode::UnknownBranch, "unknown_branch"),
            (ErrorCode::NoCommonHistory, "no_common_history"),
            (ErrorCode::UnknownFile, "unknown_file"),
            (ErrorCode::UnknownComment, "unknown_comment"),
            (ErrorCode::NotConnected, "not_connected"),
            (ErrorCode::AuthFailed, "auth_failed"),
            (ErrorCode::MissingScope, "missing_scope"),
            (ErrorCode::RemoteNotFound, "remote_not_found"),
            (ErrorCode::RateLimited, "rate_limited"),
            (ErrorCode::NetworkFailed, "network_failed"),
            (ErrorCode::RemoteFailed, "remote_failed"),
            (ErrorCode::PublishBlocked, "publish_blocked"),
            (ErrorCode::ModelNotConfigured, "model_not_configured"),
            (ErrorCode::ModelRefused, "model_refused"),
            (ErrorCode::ModelTruncated, "model_truncated"),
            (ErrorCode::ModelInvalidOutput, "model_invalid_output"),
            (ErrorCode::ClaudeCliNotFound, "claude_cli_not_found"),
            (ErrorCode::ClaudeCliFailed, "claude_cli_failed"),
            (ErrorCode::Cancelled, "cancelled"),
            (ErrorCode::NoFolder, "no_folder"),
            (ErrorCode::UnknownWorktree, "unknown_worktree"),
            (ErrorCode::WorktreeDirty, "worktree_dirty"),
            (ErrorCode::TerminalsOpen, "terminals_open"),
            (ErrorCode::UnknownTerminal, "unknown_terminal"),
            (ErrorCode::Internal, "internal"),
        ];
        for (code, expected) in codes {
            assert_eq!(serde_json::to_value(code).unwrap(), expected);
        }
    }

    #[test]
    fn every_reason_has_an_english_catalog_entry_and_no_entry_is_left_over() {
        let entries = catalog::entries();
        let missing: Vec<&str> = Reason::ALL
            .iter()
            .map(|reason| reason.key())
            .filter(|key| {
                !entries.contains_key(*key)
                    && !entries.contains_key(&format!("{key}_one"))
                    && !entries.contains_key(&format!("{key}_other"))
            })
            .collect();
        assert_eq!(missing, Vec::<&str>::new(), "reasons without an entry");
        let unknown: Vec<&String> = entries
            .keys()
            .filter(|key| {
                let plural = key
                    .strip_suffix("_one")
                    .or_else(|| key.strip_suffix("_other"));
                Reason::from_key(key).is_none() && plural.and_then(Reason::from_key).is_none()
            })
            .collect();
        assert_eq!(unknown, Vec::<&String>::new(), "entries of no reason");
    }

    #[test]
    fn reason_keys_are_unique_snake_case_spellings_of_their_names() {
        let mut keys = std::collections::HashSet::new();
        for reason in Reason::ALL {
            let key = reason.key();
            assert!(keys.insert(key), "{key} twice");
            let snake: String = format!("{reason:?}")
                .chars()
                .enumerate()
                .flat_map(|(index, c)| {
                    let lower = c.to_ascii_lowercase();
                    if c.is_ascii_uppercase() && index > 0 {
                        vec!['_', lower]
                    } else {
                        vec![lower]
                    }
                })
                .collect();
            assert_eq!(key, snake);
            assert_eq!(Reason::from_key(key), Some(*reason));
            assert_eq!(serde_json::to_value(reason).unwrap(), key);
        }
    }

    #[test]
    fn the_catalog_fills_in_params_plurals_and_nested_sentences() {
        let error = AppError::from(rusqlite::Error::InvalidQuery);
        assert_eq!(error.reason, Some(Reason::StorageFailed));
        assert_eq!(error.detail.as_deref(), Some("Query is not read-only"));
        assert_eq!(
            catalog::render(Reason::StorageFailed, &error.params, error.detail.as_deref()),
            error.message
        );
    }
}
