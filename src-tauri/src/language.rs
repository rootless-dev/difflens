//! The languages DiffLens is written in, and the one the user chose in
//! Preferences. The screen translates itself; the core only keeps the choice
//! and tells the model which language to write its prose in.

use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::store::Store;

/// A language of the app, as the BCP 47 tag the frontend uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt-BR")]
    BrazilianPortuguese,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::BrazilianPortuguese];

    pub fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::BrazilianPortuguese => "pt-BR",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.tag() == tag)
    }

    /// How a prompt names it to the model.
    pub fn english_name(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::BrazilianPortuguese => "Brazilian Portuguese",
        }
    }

    /// The line that opens the question of an analysis or a summary, naming
    /// the language its prose is written in; the system prompts say what
    /// stays as it is.
    pub fn prose_instruction(self) -> String {
        format!(
            "Write every prose field of your answer in {}.\n\n",
            self.english_name()
        )
    }
}

/// What the user chose: a language, or the operating system's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "pt-BR")]
    BrazilianPortuguese,
}

impl LanguagePreference {
    fn as_str(self) -> &'static str {
        match self {
            LanguagePreference::System => "system",
            LanguagePreference::English => "en",
            LanguagePreference::BrazilianPortuguese => "pt-BR",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [
            LanguagePreference::System,
            LanguagePreference::English,
            LanguagePreference::BrazilianPortuguese,
        ]
        .into_iter()
        .find(|preference| preference.as_str() == value)
    }
}

/// Key of the choice in the app-wide `settings` table.
const LANGUAGE_SETTING: &str = "language";

/// The choice kept; System before the first one, or when what is kept is not
/// a choice this version knows.
pub fn language_preference(store: &Store) -> AppResult<LanguagePreference> {
    Ok(store
        .setting(LANGUAGE_SETTING)?
        .as_deref()
        .and_then(LanguagePreference::parse)
        .unwrap_or_default())
}

pub fn set_language_preference(store: &Store, preference: LanguagePreference) -> AppResult<()> {
    store.set_setting(LANGUAGE_SETTING, preference.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preference_is_system_until_one_is_chosen_and_then_kept() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(
            language_preference(&store).unwrap(),
            LanguagePreference::System
        );
        set_language_preference(&store, LanguagePreference::BrazilianPortuguese).unwrap();
        assert_eq!(
            language_preference(&store).unwrap(),
            LanguagePreference::BrazilianPortuguese
        );
        set_language_preference(&store, LanguagePreference::English).unwrap();
        assert_eq!(
            language_preference(&store).unwrap(),
            LanguagePreference::English
        );
    }

    #[test]
    fn an_unknown_stored_choice_reads_as_system() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting(LANGUAGE_SETTING, "fr").unwrap();
        assert_eq!(
            language_preference(&store).unwrap(),
            LanguagePreference::System
        );
    }

    #[test]
    fn languages_and_preferences_use_the_frontend_tags() {
        assert_eq!(
            serde_json::to_value(Language::BrazilianPortuguese).unwrap(),
            "pt-BR"
        );
        assert_eq!(serde_json::to_value(Language::English).unwrap(), "en");
        assert_eq!(
            serde_json::from_value::<LanguagePreference>(serde_json::json!("system")).unwrap(),
            LanguagePreference::System
        );
        assert_eq!(
            Language::from_tag("pt-BR"),
            Some(Language::BrazilianPortuguese)
        );
        assert_eq!(Language::from_tag("de"), None);
    }
}
