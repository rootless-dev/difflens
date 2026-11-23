//! Where secrets (pull request host tokens, model provider API keys) are
//! kept: the macOS Keychain, under DiffLens's own entries. They are read only
//! to make a request and never written anywhere else, logged or sent to the
//! frontend.

use crate::error::{AppError, AppResult, Reason};

/// Keychain service of every DiffLens entry.
const SERVICE: &str = "DiffLens";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> AppResult<Option<String>>;
    fn set(&self, key: &str, secret: &str) -> AppResult<()>;
    /// Succeeds when there is nothing to delete.
    fn delete(&self, key: &str) -> AppResult<()>;
}

/// The entry of a host's credential, such as "bitbucket:dana@example.com".
pub fn secret_key(host: &str, username: &str) -> String {
    format!("{host}:{username}")
}

pub struct KeychainSecrets;

impl KeychainSecrets {
    fn entry(key: &str) -> AppResult<keyring::Entry> {
        keyring::Entry::new(SERVICE, key).map_err(|error| keychain_failed("open", &error))
    }
}

impl SecretStore for KeychainSecrets {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_failed("read", &error)),
        }
    }

    fn set(&self, key: &str, secret: &str) -> AppResult<()> {
        Self::entry(key)?
            .set_password(secret)
            .map_err(|error| keychain_failed("save", &error))
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_failed("delete", &error)),
        }
    }
}

/// keyring's errors describe the failure, never the secret.
fn keychain_failed(action: &str, error: &keyring::Error) -> AppError {
    let reason = match action {
        "open" => Reason::KeychainOpenFailed,
        "read" => Reason::KeychainReadFailed,
        "save" => Reason::KeychainSaveFailed,
        _ => Reason::KeychainDeleteFailed,
    };
    AppError::internal(
        reason,
        format!("The Keychain could not {action} the DiffLens secret: {error}"),
    )
    .detail(error.to_string())
    .verified()
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Secrets kept in memory, so tests never touch the real Keychain.
    #[derive(Default)]
    pub struct MemorySecrets {
        pub secrets: Mutex<HashMap<String, String>>,
    }

    impl SecretStore for MemorySecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            Ok(self.secrets.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, secret: &str) -> AppResult<()> {
            self.secrets
                .lock()
                .unwrap()
                .insert(key.to_string(), secret.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> AppResult<()> {
            self.secrets.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn a_keychain_failure_names_what_it_was_doing_and_keeps_its_words() {
        for (action, reason) in [
            ("open", Reason::KeychainOpenFailed),
            ("read", Reason::KeychainReadFailed),
            ("save", Reason::KeychainSaveFailed),
            ("delete", Reason::KeychainDeleteFailed),
        ] {
            // Checked against the English catalog, as every worded error is.
            let error = keychain_failed(action, &keyring::Error::NoEntry);
            assert_eq!(error.reason, Some(reason));
            assert_eq!(error.detail, Some(keyring::Error::NoEntry.to_string()));
            assert!(error.message.starts_with(&format!("The Keychain could not {action} ")));
        }
    }
}
