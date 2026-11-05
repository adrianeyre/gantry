//! The embedded server's secret key (ADR-0005, ADR-0008). Generated on first
//! run and kept in the OS keychain, never beside the database. The
//! `GANTRY_SECRET_KEY` environment variable, when set, is used instead, for a
//! machine with no keychain.
use base64::Engine;
use rand::RngCore;

/// The keychain entry's service and account names.
pub const SERVICE: &str = "io.github.adrianeyre.gantry";
pub const ACCOUNT: &str = "embedded-server-secret-key";

/// Where the key is kept.
pub trait KeyStore {
    /// The stored key, or `None` if there is none yet.
    fn get(&self) -> Result<Option<String>, SecretKeyError>;
    fn set(&self, key: &str) -> Result<(), SecretKeyError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SecretKeyError {
    #[error(
        "Gantry needs a secret key to encrypt Printer credentials, and could not use the system keychain to keep one ({0}). \
         Set the GANTRY_SECRET_KEY environment variable (at least 16 characters, such as the output of `openssl rand -base64 32`) and start the app again."
    )]
    Keychain(String),
    #[error("the system keychain did not keep the secret key it was given")]
    NotKept,
}

/// The OS keychain: Credential Manager on Windows, the Keychain on macOS, the
/// Secret Service (GNOME Keyring, KWallet) on Linux.
pub struct OsKeychain;

impl OsKeychain {
    fn entry() -> Result<keyring::Entry, SecretKeyError> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| SecretKeyError::Keychain(e.to_string()))
    }
}

impl KeyStore for OsKeychain {
    fn get(&self) -> Result<Option<String>, SecretKeyError> {
        match Self::entry()?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretKeyError::Keychain(e.to_string())),
        }
    }

    fn set(&self, key: &str) -> Result<(), SecretKeyError> {
        Self::entry()?
            .set_password(key)
            .map_err(|e| SecretKeyError::Keychain(e.to_string()))
    }
}

/// A new key: 32 random bytes, base64.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The key to start the embedded server with: `from_env` when set, else the
/// stored one, else a new one, stored and read back so a keychain that only
/// pretends to keep it is caught now rather than on the next start.
pub fn load_or_create(
    store: &dyn KeyStore,
    from_env: Option<String>,
) -> Result<String, SecretKeyError> {
    if let Some(key) = from_env.filter(|k| !k.trim().is_empty()) {
        return Ok(key);
    }
    if let Some(key) = store.get()? {
        return Ok(key);
    }
    let key = generate();
    store.set(&key)?;
    match store.get()? {
        Some(kept) if kept == key => Ok(key),
        _ => Err(SecretKeyError::NotKept),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct MemoryStore {
        key: RefCell<Option<String>>,
        sets: RefCell<u32>,
        forgetful: bool,
        broken: bool,
    }

    impl KeyStore for MemoryStore {
        fn get(&self) -> Result<Option<String>, SecretKeyError> {
            if self.broken {
                return Err(SecretKeyError::Keychain("no keychain here".into()));
            }
            Ok(if self.forgetful {
                None
            } else {
                self.key.borrow().clone()
            })
        }
        fn set(&self, key: &str) -> Result<(), SecretKeyError> {
            *self.sets.borrow_mut() += 1;
            *self.key.borrow_mut() = Some(key.to_owned());
            Ok(())
        }
    }

    #[test]
    fn the_first_run_generates_and_stores_a_strong_key_and_later_runs_reuse_it() {
        let store = MemoryStore::default();
        let first = load_or_create(&store, None).unwrap();
        // 32 bytes of base64, which gantry-server accepts as a secret key.
        assert_eq!(first.len(), 44);
        assert!(gantry_server::crypto::SecretKey::from_config(&first).is_some());
        let second = load_or_create(&store, None).unwrap();
        assert_eq!(first, second);
        assert_eq!(*store.sets.borrow(), 1);
    }

    #[test]
    fn keys_are_random() {
        assert_ne!(generate(), generate());
    }

    #[test]
    fn the_environment_variable_wins_and_nothing_is_stored() {
        let store = MemoryStore {
            broken: true,
            ..MemoryStore::default()
        };
        assert_eq!(
            load_or_create(&store, Some("a key from the environment".into())).unwrap(),
            "a key from the environment"
        );
        assert_eq!(*store.sets.borrow(), 0);
    }

    #[test]
    fn no_keychain_and_no_variable_refuses_and_says_how_to_fix_it() {
        let store = MemoryStore {
            broken: true,
            ..MemoryStore::default()
        };
        let message = load_or_create(&store, Some("  ".into()))
            .unwrap_err()
            .to_string();
        assert!(message.contains("GANTRY_SECRET_KEY"), "{message}");
    }

    #[test]
    fn a_keychain_that_does_not_keep_the_key_is_caught() {
        let store = MemoryStore {
            forgetful: true,
            ..MemoryStore::default()
        };
        assert!(matches!(
            load_or_create(&store, None),
            Err(SecretKeyError::NotKept)
        ));
    }
}
