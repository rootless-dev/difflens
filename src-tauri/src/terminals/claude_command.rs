//! The command a Claude Code session runs, as Preferences › Terminal keeps
//! it: `claude`, or a command of the user's own (`claude --model opus`, a
//! wrapper script). It is read here, in the core, as each session starts,
//! never taken from the frontend, so nothing else can make a session run
//! something the user did not save; sessions already open keep the command
//! they started with.
//!
//! The choice and the last custom command are two settings, so choosing the
//! default keeps the custom command for when the user comes back to it. A
//! custom command runs through the login shell as `exec <command>`, like
//! `claude`; it has to be one line, since a line break would make the shell
//! run what follows as a command of its own. A stored value this version
//! cannot use (one written by hand, or by a later version) is ignored and the
//! session runs the default.

use serde::{Deserialize, Serialize};

use crate::error::{app_error, AppResult};
use crate::store::Store;

/// The most characters a custom command may have.
pub const MAX_CLAUDE_COMMAND_CHARS: usize = 1000;
/// Whether sessions run the default or the custom command.
const MODE_SETTING: &str = "terminal.claude_command.mode";
/// The last custom command saved, kept while the default is chosen.
pub(super) const CUSTOM_SETTING: &str = "terminal.claude_command.custom";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeCommandMode {
    /// `claude`.
    #[default]
    Default,
    /// The custom command, once one is saved; the default until then.
    Custom,
}

impl ClaudeCommandMode {
    fn as_str(self) -> &'static str {
        match self {
            ClaudeCommandMode::Default => "default",
            ClaudeCommandMode::Custom => "custom",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [ClaudeCommandMode::Default, ClaudeCommandMode::Custom]
            .into_iter()
            .find(|mode| mode.as_str() == value)
    }
}

/// What Preferences shows: the choice, and the custom command saved, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCommandSetting {
    pub mode: ClaudeCommandMode,
    pub custom: Option<String>,
}

/// The choice and the custom command kept; the default, and no command,
/// before the user saves any or when what is kept cannot be used.
pub fn claude_command_setting(store: &Store) -> AppResult<ClaudeCommandSetting> {
    let mode = store
        .setting(MODE_SETTING)?
        .as_deref()
        .and_then(ClaudeCommandMode::parse)
        .unwrap_or_default();
    let custom = store
        .setting(CUSTOM_SETTING)?
        .and_then(|command| valid_command(&command).ok());
    Ok(ClaudeCommandSetting { mode, custom })
}

/// Keeps the choice; the custom command stays saved either way.
pub fn set_claude_command_mode(
    store: &Store,
    mode: ClaudeCommandMode,
) -> AppResult<ClaudeCommandSetting> {
    store.set_setting(MODE_SETTING, mode.as_str())?;
    claude_command_setting(store)
}

/// Keeps `command`, trimmed, as the custom command and chooses it. Fails,
/// keeping what was saved before, when it is empty, longer than
/// `MAX_CLAUDE_COMMAND_CHARS` or not a single line.
pub fn save_custom_claude_command(store: &Store, command: &str) -> AppResult<ClaudeCommandSetting> {
    let command = valid_command(command)?;
    store.set_setting(CUSTOM_SETTING, &command)?;
    set_claude_command_mode(store, ClaudeCommandMode::Custom)
}

/// The custom command a new session runs instead of the default one: `None`
/// while the default is chosen, or the custom one is but none is saved.
pub fn session_claude_command(store: &Store) -> AppResult<Option<String>> {
    let setting = claude_command_setting(store)?;
    Ok(match setting.mode {
        ClaudeCommandMode::Default => None,
        ClaudeCommandMode::Custom => setting.custom,
    })
}

/// `command` trimmed, when a session can run it.
fn valid_command(command: &str) -> AppResult<String> {
    let command = command.trim();
    if command.is_empty() {
        return Err(app_error!(
            Internal,
            ClaudeCommandEmpty,
            "Enter the command that starts Claude Code."
        ));
    }
    if command.chars().any(char::is_control) {
        return Err(app_error!(
            Internal,
            ClaudeCommandNotOneLine,
            "The Claude Code command must be a single line, without control characters."
        ));
    }
    if command.chars().count() > MAX_CLAUDE_COMMAND_CHARS {
        return Err(app_error!(
            Internal,
            ClaudeCommandTooLong,
            "The Claude Code command can have at most 1000 characters."
        ));
    }
    Ok(command.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Reason;

    fn setting(mode: ClaudeCommandMode, custom: Option<&str>) -> ClaudeCommandSetting {
        ClaudeCommandSetting {
            mode,
            custom: custom.map(str::to_string),
        }
    }

    #[test]
    fn the_default_runs_until_a_custom_command_is_saved() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(
            claude_command_setting(&store).unwrap(),
            setting(ClaudeCommandMode::Default, None)
        );
        assert_eq!(session_claude_command(&store).unwrap(), None);

        // Custom chosen, nothing saved yet: sessions keep the default.
        let chosen = set_claude_command_mode(&store, ClaudeCommandMode::Custom).unwrap();
        assert_eq!(chosen, setting(ClaudeCommandMode::Custom, None));
        assert_eq!(session_claude_command(&store).unwrap(), None);

        let saved = save_custom_claude_command(&store, "  claude --model opus \t").unwrap();
        assert_eq!(saved, setting(ClaudeCommandMode::Custom, Some("claude --model opus")));
        assert_eq!(
            session_claude_command(&store).unwrap().as_deref(),
            Some("claude --model opus")
        );
    }

    #[test]
    fn choosing_the_default_keeps_the_custom_command_for_later() {
        let store = Store::open_in_memory().unwrap();
        save_custom_claude_command(&store, "/opt/bin/claude-wrapper").unwrap();

        let default = set_claude_command_mode(&store, ClaudeCommandMode::Default).unwrap();
        assert_eq!(
            default,
            setting(ClaudeCommandMode::Default, Some("/opt/bin/claude-wrapper"))
        );
        assert_eq!(session_claude_command(&store).unwrap(), None);

        let custom = set_claude_command_mode(&store, ClaudeCommandMode::Custom).unwrap();
        assert_eq!(
            custom,
            setting(ClaudeCommandMode::Custom, Some("/opt/bin/claude-wrapper"))
        );
        assert_eq!(
            session_claude_command(&store).unwrap().as_deref(),
            Some("/opt/bin/claude-wrapper")
        );
    }

    #[test]
    fn a_command_a_session_cannot_run_is_refused_and_the_saved_one_kept() {
        let store = Store::open_in_memory().unwrap();
        save_custom_claude_command(&store, "claude").unwrap();
        set_claude_command_mode(&store, ClaudeCommandMode::Default).unwrap();
        let too_long = "x".repeat(MAX_CLAUDE_COMMAND_CHARS + 1);
        let refused = [
            ("", Reason::ClaudeCommandEmpty),
            ("   ", Reason::ClaudeCommandEmpty),
            ("claude\nrm -rf ~", Reason::ClaudeCommandNotOneLine),
            ("claude\r--resume", Reason::ClaudeCommandNotOneLine),
            ("claude\0", Reason::ClaudeCommandNotOneLine),
            (too_long.as_str(), Reason::ClaudeCommandTooLong),
        ];
        for (command, reason) in refused {
            let error = save_custom_claude_command(&store, command).unwrap_err();
            assert_eq!(error.reason, Some(reason), "{command:?}");
        }
        assert_eq!(
            claude_command_setting(&store).unwrap(),
            setting(ClaudeCommandMode::Default, Some("claude")),
            "neither the command nor the choice changed"
        );
        let longest = "x".repeat(MAX_CLAUDE_COMMAND_CHARS);
        assert!(save_custom_claude_command(&store, &longest).is_ok());
    }

    #[test]
    fn stored_values_this_version_cannot_use_fall_back_to_the_default() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting(MODE_SETTING, "custom").unwrap();
        for stored in ["claude\necho injected", "", "   ", &"x".repeat(MAX_CLAUDE_COMMAND_CHARS + 1)] {
            store.set_setting(CUSTOM_SETTING, stored).unwrap();
            assert_eq!(
                claude_command_setting(&store).unwrap(),
                setting(ClaudeCommandMode::Custom, None),
                "{stored:?}"
            );
            assert_eq!(session_claude_command(&store).unwrap(), None, "{stored:?}");
        }

        store.set_setting(CUSTOM_SETTING, "claude --model opus").unwrap();
        store.set_setting(MODE_SETTING, "sometimes").unwrap();
        assert_eq!(
            claude_command_setting(&store).unwrap(),
            setting(ClaudeCommandMode::Default, Some("claude --model opus"))
        );
        assert_eq!(session_claude_command(&store).unwrap(), None);
    }

    #[test]
    fn the_setting_reaches_the_frontend_in_its_shape() {
        let value = serde_json::to_value(setting(ClaudeCommandMode::Custom, Some("claude"))).unwrap();
        assert_eq!(value, serde_json::json!({ "mode": "custom", "custom": "claude" }));
        let mode: ClaudeCommandMode = serde_json::from_value(serde_json::json!("default")).unwrap();
        assert_eq!(mode, ClaudeCommandMode::Default);
    }
}
