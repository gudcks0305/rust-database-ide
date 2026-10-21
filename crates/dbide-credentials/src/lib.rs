//! Exact profile-scoped credentials. Native access is never implicit or a file fallback.
use std::{collections::HashMap, fmt, sync::Mutex};
use zeroize::Zeroizing;

pub const KEYCHAIN_SERVICE: &str = "com.local.rust-database-ide.credentials";
pub const MAX_PROFILE_ID_BYTES: usize = 128;
pub const MAX_SECRET_BYTES: usize = 64 * 1024;

/// Owns bytes that are zeroized on drop. Deliberately neither Clone nor Serialize.
/// Callers must also clear any separate UI/String copies they create.
pub struct Secret(Zeroizing<Vec<u8>>);
impl Secret {
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(Zeroizing::new(value.into()))
    }
    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
    pub fn expose_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.0)
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialOperation {
    Load,
    Save,
    Remove,
}

/// Contains neither profile IDs nor credential contents, including in Debug.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    InvalidProfileId,
    SecretTooLarge,
    UnsupportedPlatform,
    MemoryStoreUnavailable,
    Native {
        operation: CredentialOperation,
        code: i32,
    },
}
impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProfileId => write!(
                f,
                "Credential profile ID must be 1–128 UTF-8 bytes without surrounding whitespace or control characters"
            ),
            Self::SecretTooLarge => write!(f, "Credential exceeds the 64 KiB limit"),
            Self::UnsupportedPlatform => {
                write!(f, "Native credential storage is only supported on macOS")
            }
            Self::MemoryStoreUnavailable => write!(f, "Session credential storage is unavailable"),
            Self::Native { operation, code } => {
                write!(f, "Keychain {operation:?} failed (OSStatus {code})")
            }
        }
    }
}
impl std::error::Error for CredentialError {}

pub fn validate_profile_id(id: &str) -> Result<(), CredentialError> {
    if id.is_empty()
        || id.len() > MAX_PROFILE_ID_BYTES
        || id != id.trim()
        || id.chars().any(char::is_control)
    {
        return Err(CredentialError::InvalidProfileId);
    }
    Ok(())
}
fn validate_secret(secret: &Secret) -> Result<(), CredentialError> {
    if secret.expose_secret().len() > MAX_SECRET_BYTES {
        return Err(CredentialError::SecretTooLarge);
    }
    Ok(())
}

pub trait CredentialStore {
    fn load(&self, profile_id: &str) -> Result<Option<Secret>, CredentialError>;
    /// Only invoke after the user's explicit remember-credential choice.
    fn save(&self, profile_id: &str, secret: &Secret) -> Result<(), CredentialError>;
    /// Idempotent: an already missing entry succeeds.
    fn remove(&self, profile_id: &str) -> Result<(), CredentialError>;
}

/// Exact generic-password lookup. Constructing this type performs no native I/O.
/// This service is app scoping, not an authorization boundary against other local apps.
#[derive(Clone, Copy, Debug, Default)]
pub struct MacKeychainStore;

#[cfg(target_os = "macos")]
impl CredentialStore for MacKeychainStore {
    fn load(&self, profile_id: &str) -> Result<Option<Secret>, CredentialError> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        use security_framework_sys::base::errSecItemNotFound;
        validate_profile_id(profile_id)?;
        match generic_password(PasswordOptions::new_generic_password(
            KEYCHAIN_SERVICE,
            profile_id,
        )) {
            Ok(bytes) => {
                let secret = Secret::new(bytes);
                validate_secret(&secret)?;
                Ok(Some(secret))
            }
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(error) => Err(CredentialError::Native {
                operation: CredentialOperation::Load,
                code: error.code(),
            }),
        }
    }
    fn save(&self, profile_id: &str, secret: &Secret) -> Result<(), CredentialError> {
        validate_profile_id(profile_id)?;
        validate_secret(secret)?;
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            profile_id,
            secret.expose_secret(),
        )
        .map_err(|error| CredentialError::Native {
            operation: CredentialOperation::Save,
            code: error.code(),
        })
    }
    fn remove(&self, profile_id: &str) -> Result<(), CredentialError> {
        use security_framework_sys::base::errSecItemNotFound;
        validate_profile_id(profile_id)?;
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, profile_id) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == errSecItemNotFound => Ok(()),
            Err(error) => Err(CredentialError::Native {
                operation: CredentialOperation::Remove,
                code: error.code(),
            }),
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialStore for MacKeychainStore {
    fn load(&self, profile_id: &str) -> Result<Option<Secret>, CredentialError> {
        validate_profile_id(profile_id)?;
        Err(CredentialError::UnsupportedPlatform)
    }
    fn save(&self, profile_id: &str, secret: &Secret) -> Result<(), CredentialError> {
        validate_profile_id(profile_id)?;
        validate_secret(secret)?;
        Err(CredentialError::UnsupportedPlatform)
    }
    fn remove(&self, profile_id: &str) -> Result<(), CredentialError> {
        validate_profile_id(profile_id)?;
        Err(CredentialError::UnsupportedPlatform)
    }
}

/// Explicit test/session store. Never selected as an automatic native-store fallback.
/// Kept values are zeroized on replacement, removal, or drop.
#[derive(Default)]
pub struct InMemoryCredentialStore {
    values: Mutex<HashMap<String, Secret>>,
}
impl fmt::Debug for InMemoryCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InMemoryCredentialStore([REDACTED])")
    }
}
impl CredentialStore for InMemoryCredentialStore {
    fn load(&self, profile_id: &str) -> Result<Option<Secret>, CredentialError> {
        validate_profile_id(profile_id)?;
        let values = self
            .values
            .lock()
            .map_err(|_| CredentialError::MemoryStoreUnavailable)?;
        // Returning an owned zeroizing value makes this single explicit copy necessary.
        Ok(values
            .get(profile_id)
            .map(|secret| Secret::new(secret.expose_secret().to_vec())))
    }
    fn save(&self, profile_id: &str, secret: &Secret) -> Result<(), CredentialError> {
        validate_profile_id(profile_id)?;
        validate_secret(secret)?;
        self.values
            .lock()
            .map_err(|_| CredentialError::MemoryStoreUnavailable)?
            .insert(
                profile_id.to_owned(),
                Secret::new(secret.expose_secret().to_vec()),
            );
        Ok(())
    }
    fn remove(&self, profile_id: &str) -> Result<(), CredentialError> {
        validate_profile_id(profile_id)?;
        self.values
            .lock()
            .map_err(|_| CredentialError::MemoryStoreUnavailable)?
            .remove(profile_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_id_validation_is_bounded_and_does_not_echo_input() {
        for id in ["", " ", " leading", "trailing ", "bad\nvalue", "bad\0value"] {
            assert_eq!(
                validate_profile_id(id),
                Err(CredentialError::InvalidProfileId)
            );
        }
        validate_profile_id(&"a".repeat(64)).unwrap();
        validate_profile_id(&"a".repeat(128)).unwrap();
        assert_eq!(
            validate_profile_id(&"a".repeat(129)),
            Err(CredentialError::InvalidProfileId)
        );
        validate_profile_id(&"한".repeat(42)).unwrap();
        assert!(validate_profile_id(&"한".repeat(43)).is_err());
        let error = validate_profile_id("private-id\n").unwrap_err();
        assert!(!format!("{error:?} {error}").contains("private-id"));
    }
    #[test]
    fn secrets_and_memory_store_debug_are_redacted() {
        let secret = Secret::new("sensitive-test-value".to_owned());
        assert_eq!(secret.expose_str().unwrap(), "sensitive-test-value");
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        let store = InMemoryCredentialStore::default();
        store.save("profile-one", &secret).unwrap();
        assert_eq!(format!("{store:?}"), "InMemoryCredentialStore([REDACTED])");
    }
    #[test]
    fn absent_overwrite_removal_and_profile_isolation() {
        let store = InMemoryCredentialStore::default();
        assert!(store.load("one").unwrap().is_none());
        store.remove("one").unwrap();
        store.save("one", &Secret::new("first")).unwrap();
        store.save("two", &Secret::new("separate")).unwrap();
        store.save("one", &Secret::new("updated")).unwrap();
        assert_eq!(
            store.load("one").unwrap().unwrap().expose_secret(),
            b"updated"
        );
        assert_eq!(
            store.load("two").unwrap().unwrap().expose_secret(),
            b"separate"
        );
        store.remove("one").unwrap();
        assert!(store.load("one").unwrap().is_none());
        assert!(store.load("two").unwrap().is_some());
    }
    #[test]
    fn invalid_ids_and_large_secrets_never_modify_store() {
        let store = InMemoryCredentialStore::default();
        let secret = Secret::new("fixture");
        assert!(store.save("", &secret).is_err());
        assert!(store.load("").is_err());
        assert!(store.remove("").is_err());
        store.save("one", &secret).unwrap();
        assert_eq!(
            store.save("one", &Secret::new(vec![b'x'; MAX_SECRET_BYTES + 1])),
            Err(CredentialError::SecretTooLarge)
        );
        assert_eq!(
            store.load("one").unwrap().unwrap().expose_secret(),
            b"fixture"
        );
        // An empty password is a legitimate database credential.
        store.save("empty", &Secret::new(Vec::new())).unwrap();
        assert!(
            store
                .load("empty")
                .unwrap()
                .unwrap()
                .expose_secret()
                .is_empty()
        );
    }
    #[test]
    fn arbitrary_secret_bytes_are_preserved_without_lossy_conversion() {
        let store = InMemoryCredentialStore::default();
        store.save("bytes", &Secret::new(vec![0, 255])).unwrap();
        let secret = store.load("bytes").unwrap().unwrap();
        assert_eq!(secret.expose_secret(), &[0, 255]);
        assert!(secret.expose_str().is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "explicit native Keychain fixture only; requires DBIDE_TEST_KEYCHAIN=1"]
    fn native_keychain_fixture_round_trip() {
        assert_eq!(
            std::env::var("DBIDE_TEST_KEYCHAIN").as_deref(),
            Ok("1"),
            "Set DBIDE_TEST_KEYCHAIN=1 explicitly to permit a fixture Keychain entry"
        );
        let id = format!("dbide-keychain-test-{}", uuid::Uuid::new_v4());
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = MacKeychainStore.remove(&self.0);
            }
        }
        let _cleanup = Cleanup(id.clone());
        let store = MacKeychainStore;
        assert!(store.load(&id).unwrap().is_none());
        store
            .save(&id, &Secret::new("dbide-fake-fixture-password"))
            .unwrap();
        assert!(
            store.load(&id).unwrap().unwrap().expose_secret() == b"dbide-fake-fixture-password"
        );
        store
            .save(&id, &Secret::new("dbide-fake-fixture-updated"))
            .unwrap();
        assert!(store.load(&id).unwrap().unwrap().expose_secret() == b"dbide-fake-fixture-updated");
        store.remove(&id).unwrap();
        assert!(store.load(&id).unwrap().is_none());
        store.remove(&id).unwrap();
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_has_no_fallback() {
        let store = MacKeychainStore;
        assert!(matches!(
            store.load("one"),
            Err(CredentialError::UnsupportedPlatform)
        ));
        assert_eq!(
            store.save("one", &Secret::new("fixture")),
            Err(CredentialError::UnsupportedPlatform)
        );
        assert_eq!(
            store.remove("one"),
            Err(CredentialError::UnsupportedPlatform)
        );
    }
}
