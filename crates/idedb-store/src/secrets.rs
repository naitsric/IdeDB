use std::collections::HashMap;
use std::sync::Mutex;

use crate::Result;

#[cfg(target_os = "macos")]
pub use keychain::Keychain;

/// Where data source passwords live, keyed by data source id.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>>;
    fn set(&self, id: &str, secret: &str) -> Result<()>;
    /// Removing a missing secret is not an error.
    fn delete(&self, id: &str) -> Result<()>;
}

/// The login keychain. IdeDB ships for macOS only; other platforms compile
/// the crate (for CI) without it.
#[cfg(target_os = "macos")]
mod keychain {
    use security_framework::passwords;

    use super::SecretStore;
    use crate::{Error, Result};

    /// `errSecItemNotFound` from Security.framework.
    const ITEM_NOT_FOUND: i32 = -25300;

    /// One generic password item per data source.
    pub struct Keychain {
        service: String,
    }

    impl Keychain {
        /// `service` namespaces the items, e.g. the app identifier.
        pub fn new(service: impl Into<String>) -> Self {
            Self { service: service.into() }
        }
    }

    impl SecretStore for Keychain {
        fn get(&self, id: &str) -> Result<Option<String>> {
            match passwords::get_generic_password(&self.service, id) {
                Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|e| Error::Secret(e.to_string())),
                Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(Error::Secret(e.to_string())),
            }
        }

        fn set(&self, id: &str, secret: &str) -> Result<()> {
            passwords::set_generic_password(&self.service, id, secret.as_bytes())
                .map_err(|e| Error::Secret(e.to_string()))
        }

        fn delete(&self, id: &str) -> Result<()> {
            match passwords::delete_generic_password(&self.service, id) {
                Err(e) if e.code() != ITEM_NOT_FOUND => Err(Error::Secret(e.to_string())),
                _ => Ok(()),
            }
        }
    }
}

/// In-process secrets, lost on quit: for tests, and for builds on platforms
/// without a keychain integration.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }

    fn set(&self, id: &str, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(id.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}
