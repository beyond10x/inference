//! Where the API key comes from: an injected [`CredentialSource`].
//!
//! The client never looks a key up itself. The caller passes a source and the name of the secret
//! to resolve; the source returns its bytes as a [`Credential`], zeroized on drop and redacted in
//! `Debug`. Two sources ship here: [`EnvironmentSource`], over environment variables the caller
//! binds, and, with feature `keychain`, `KeychainSource`, over the OS keychain through the
//! `secrets` library.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use zeroize::Zeroizing;

/// The secret name the Phonic key is stored under: `secretsctl put phonic-api-key`.
pub const PHONIC_KEY_NAME: &str = "phonic-api-key";

/// The environment variable [`EnvironmentSource::phonic`] binds [`PHONIC_KEY_NAME`] to.
pub const PHONIC_KEY_VARIABLE: &str = "PHONIC_API_KEY";

/// The future a [`CredentialSource`] returns.
pub type CredentialFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Credential, CredentialError>> + Send + 'a>>;

/// Resolves one named secret to its bytes.
pub trait CredentialSource: Send + Sync {
    /// The secret named `name`.
    ///
    /// # Errors
    /// A [`CredentialError`], which never carries the name's location or the value.
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a>;
}

/// Secret bytes: zeroized on drop, never printed.
pub struct Credential(Zeroizing<Vec<u8>>);

impl Credential {
    /// Takes ownership of `bytes`.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The secret bytes.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// `prefix` followed by the key, as a header value marked sensitive; the credential is
    /// consumed. Without a prefix the value's bytes are the credential's own buffer; with one they
    /// are a single new buffer holding the prefix and the key, and the credential is zeroized at
    /// once. Either way the value owns its buffer rather than a copy of it, and the buffer is
    /// zeroized when the value and every clone of it drop.
    ///
    /// # Errors
    /// [`CredentialError::Invalid`] when the result cannot be a header value.
    pub(crate) fn into_header(self, prefix: &[u8]) -> Result<HeaderValue, CredentialError> {
        if prefix.is_empty() {
            return sensitive_header(self.0);
        }
        let mut value = Zeroizing::new(Vec::with_capacity(prefix.len() + self.0.len()));
        value.extend_from_slice(prefix);
        value.extend_from_slice(&self.0);
        drop(self);
        sensitive_header(value)
    }
}

/// `value` as a header value marked sensitive, over `value`'s own buffer: the header copies
/// nothing, and the buffer is zeroized when its last holder drops.
fn sensitive_header(value: Zeroizing<Vec<u8>>) -> Result<HeaderValue, CredentialError> {
    let mut header = HeaderValue::from_maybe_shared(Bytes::from_owner(value))
        .map_err(|_| CredentialError::Invalid)?;
    header.set_sensitive(true);
    Ok(header)
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential([REDACTED])")
    }
}

/// Why a credential could not be had. No variant carries a name, a location or a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialError {
    /// The source holds nothing under the name, or the name is not bound.
    Missing,
    /// The name is not one the source can address, or the value cannot be sent as a header value.
    Invalid,
    /// The source could not be read: a locked or absent keychain, a refused access.
    Unavailable,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "credential is missing",
            Self::Invalid => "credential name or value is invalid",
            Self::Unavailable => "credential source is unavailable",
        })
    }
}

impl std::error::Error for CredentialError {}

/// Secrets from environment variables, each name bound by the caller to one variable. Nothing
/// else in the environment is read; the variable is read on every resolve and never cached.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvironmentSource {
    bindings: BTreeMap<String, String>,
}

impl EnvironmentSource {
    /// A source binding each secret name to an environment variable.
    pub fn new(bindings: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
        }
    }

    /// [`PHONIC_KEY_NAME`] bound to [`PHONIC_KEY_VARIABLE`].
    pub fn phonic() -> Self {
        Self::new([(PHONIC_KEY_NAME.to_owned(), PHONIC_KEY_VARIABLE.to_owned())])
    }

    fn lookup(&self, name: &str) -> Result<Credential, CredentialError> {
        let variable = self.bindings.get(name).ok_or(CredentialError::Missing)?;
        let value = std::env::var_os(variable).ok_or(CredentialError::Missing)?;
        let value = value.into_string().map_err(|_| CredentialError::Invalid)?;
        Ok(Credential::new(value.into_bytes()))
    }
}

impl fmt::Debug for EnvironmentSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentSource")
            .field("bindings", &self.bindings.len())
            .finish()
    }
}

impl CredentialSource for EnvironmentSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(self.lookup(name)))
    }
}

#[cfg(feature = "keychain")]
pub use keychain::KeychainSource;

#[cfg(feature = "keychain")]
mod keychain {
    use std::fmt;
    use std::sync::Arc;

    use keyring_core::CredentialStore;
    use secrets_core::storage::{Address, ScopeName, SecretStorage, StorageError, Target};
    use secrets_keychain::KeychainBackend;

    use super::{Credential, CredentialError, CredentialFuture, CredentialSource};

    /// Secrets from the OS keychain through the `secrets` library, read where
    /// `secretsctl put <name>` stores them: the keychain backend's default service, tenant,
    /// namespace and user `default`. Read-only.
    pub struct KeychainSource {
        backend: KeychainBackend,
    }

    impl KeychainSource {
        /// A source over `store`, a credential store the embedding chose.
        pub fn new(store: Arc<CredentialStore>) -> Self {
            Self {
                backend: KeychainBackend::new(store),
            }
        }

        /// A source over the platform's own keychain. Opening it reads nothing; using it may ask
        /// the user to unlock the keychain.
        ///
        /// # Errors
        /// [`CredentialError::Unavailable`] when the platform keychain cannot be opened.
        #[cfg(feature = "native-keychain")]
        pub fn native() -> Result<Self, CredentialError> {
            secrets_keychain::native_store()
                .map(Self::new)
                .map_err(|_| CredentialError::Unavailable)
        }

        async fn read(&self, name: &str) -> Result<Credential, CredentialError> {
            let target = Target::unbound(address(name)?);
            let revealed = self.backend.read(&target).await.map_err(failure)?;
            Ok(Credential::new(revealed.value.expose().to_vec()))
        }
    }

    /// `default/default/default/<name>`, the local scope `secretsctl` stores into.
    pub(super) fn address(name: &str) -> Result<Address, CredentialError> {
        Address::parse(
            ScopeName::DEFAULT,
            ScopeName::DEFAULT,
            ScopeName::DEFAULT,
            name,
        )
        .map_err(|_| CredentialError::Invalid)
    }

    fn failure(error: StorageError) -> CredentialError {
        match error {
            StorageError::NotFound => CredentialError::Missing,
            StorageError::InvalidName => CredentialError::Invalid,
            _ => CredentialError::Unavailable,
        }
    }

    impl fmt::Debug for KeychainSource {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("KeychainSource")
                .field("service", &self.backend.service())
                .finish()
        }
    }

    impl CredentialSource for KeychainSource {
        fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
            Box::pin(self.read(name))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_phonic_key_is_addressed_where_secretsctl_puts_it() {
            let address = address(crate::PHONIC_KEY_NAME).unwrap();
            assert_eq!(
                secrets_keychain::entry_user(&address),
                "default/default/default/phonic-api-key"
            );
        }

        #[test]
        fn a_name_the_secrets_grammar_refuses_is_invalid() {
            assert_eq!(address("Phonic").err(), Some(CredentialError::Invalid));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unprefixed_header_value_is_the_credential_buffer_itself() {
        let credential = Credential::new(b"key-0123".to_vec());
        let buffer = credential.expose().as_ptr();
        let header = credential.into_header(b"").unwrap();
        assert_eq!(
            header.as_bytes().as_ptr(),
            buffer,
            "the header value holds a copy of the key"
        );
        assert_eq!(header.as_bytes(), b"key-0123");
        assert!(header.is_sensitive());
    }

    #[test]
    fn a_header_value_holds_its_zeroizing_buffer_and_no_copy() {
        let value = Zeroizing::new(b"Bearer key-0123".to_vec());
        let buffer = value.as_ptr();
        let header = sensitive_header(value).unwrap();
        assert_eq!(
            header.as_bytes().as_ptr(),
            buffer,
            "the header value holds a copy of the key"
        );
        assert!(header.is_sensitive());
    }

    #[test]
    fn a_prefixed_header_value_is_the_prefix_then_the_key() {
        let header = Credential::new(b"key-0123".to_vec())
            .into_header(b"Bearer ")
            .unwrap();
        assert_eq!(header.as_bytes(), b"Bearer key-0123");
        assert!(header.is_sensitive());
    }

    #[test]
    fn a_key_that_cannot_be_a_header_value_is_invalid() {
        assert_eq!(
            Credential::new(b"key\n".to_vec()).into_header(b"").err(),
            Some(CredentialError::Invalid)
        );
    }
}
