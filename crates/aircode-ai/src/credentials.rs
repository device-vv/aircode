use std::fmt;

use keyring::Entry;
use secrecy::{ExposeSecret, SecretString};

use crate::AiError;

const SERVICE_NAME: &str = "dev.aircode.desktop";

pub struct ApiKey(SecretString);

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKey([REDACTED])")
    }
}

impl ApiKey {
    pub fn new(value: String) -> Result<Self, AiError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(AiError::Credentials("API key cannot be empty".into()));
        }
        Ok(Self(SecretString::new(value.into())))
    }

    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

pub struct CredentialStore;

impl CredentialStore {
    pub fn save(account: &str, key: &ApiKey) -> Result<(), AiError> {
        Entry::new(SERVICE_NAME, account)
            .map_err(credential_error)?
            .set_password(key.expose())
            .map_err(credential_error)
    }

    pub fn load(account: &str) -> Result<Option<ApiKey>, AiError> {
        match Entry::new(SERVICE_NAME, account)
            .map_err(credential_error)?
            .get_password()
        {
            Ok(value) => ApiKey::new(value).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(credential_error(error)),
        }
    }

    pub fn delete(account: &str) -> Result<(), AiError> {
        match Entry::new(SERVICE_NAME, account)
            .map_err(credential_error)?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(credential_error(error)),
        }
    }
}

fn credential_error(error: keyring::Error) -> AiError {
    AiError::Credentials(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_debug_does_not_expose_value() {
        let key = ApiKey::new("sk-super-secret".into()).unwrap();
        let output = format!("{key:?}");

        assert!(!output.contains("sk-super-secret"));
    }

    #[test]
    fn empty_api_key_is_rejected() {
        assert!(ApiKey::new("  ".into()).is_err());
    }
}
