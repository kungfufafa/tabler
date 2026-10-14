//! Provider API keys: in the macOS Keychain (service `tabler`, account `assistant:<provider>`),
//! or in the app DB's dev-only plaintext secrets table when the app runs with the file secrets
//! backend (`TABLER_SECRETS_BACKEND=file`), like the connection passwords. Keys never go back
//! to the renderer.

use crate::appdb::AppDb;
use crate::error::AppError;
use crate::secrets::Backend;
use crate::types::assistant::AssistantProvider;

const SERVICE: &str = "tabler";

/// Where keys are kept.
#[derive(Clone)]
pub struct KeyStore {
    backend: Backend,
    appdb: AppDb,
}

impl KeyStore {
    pub fn new(backend: Backend, appdb: AppDb) -> KeyStore {
        KeyStore { backend, appdb }
    }

    pub async fn get(&self, provider: AssistantProvider) -> Result<Option<String>, AppError> {
        let account = provider.key_account();
        match self.backend {
            Backend::File => self.appdb.get_secret(account),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, account)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                match entry.get_password() {
                    Ok(p) => Ok(Some(p)),
                    Err(keyring::v1::Error::NoEntry) => Ok(None),
                    Err(e) => Err(AppError::Secrets(e.to_string())),
                }
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
        .map(|key| key.filter(|k| !k.trim().is_empty()))
    }

    /// Stores `key` (trimmed); `None` or blank deletes it.
    pub async fn set(
        &self,
        provider: AssistantProvider,
        key: Option<&str>,
    ) -> Result<(), AppError> {
        let account = provider.key_account();
        let key = key.map(str::trim).filter(|k| !k.is_empty());
        if key.is_some_and(|k| k.len() > 4096 || k.chars().any(char::is_control)) {
            return Err(AppError::InvalidInput("That API key is not valid".into()));
        }
        let key = key.map(str::to_string);
        match self.backend {
            Backend::File => match key {
                Some(k) => self.appdb.set_secret(account, &k),
                None => self.appdb.delete_secret(account),
            },
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, account)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                match key {
                    Some(k) => entry
                        .set_password(&k)
                        .map_err(|e| AppError::Secrets(e.to_string())),
                    None => match entry.delete_credential() {
                        Ok(()) | Err(keyring::v1::Error::NoEntry) => Ok(()),
                        Err(e) => Err(AppError::Secrets(e.to_string())),
                    },
                }
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_backend_round_trip() {
        let store = KeyStore::new(Backend::File, AppDb::open_in_memory().unwrap());
        assert_eq!(store.get(AssistantProvider::Anthropic).await.unwrap(), None);
        store
            .set(AssistantProvider::Anthropic, Some("  sk-ant-1  "))
            .await
            .unwrap();
        assert_eq!(
            store
                .get(AssistantProvider::Anthropic)
                .await
                .unwrap()
                .as_deref(),
            Some("sk-ant-1")
        );
        assert_eq!(store.get(AssistantProvider::Openai).await.unwrap(), None);
        assert!(
            store
                .set(AssistantProvider::Openai, Some("bad\nkey"))
                .await
                .is_err()
        );
        store
            .set(AssistantProvider::Anthropic, Some(" "))
            .await
            .unwrap();
        assert_eq!(store.get(AssistantProvider::Anthropic).await.unwrap(), None);
    }
}
