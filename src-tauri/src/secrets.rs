//! Secret storage: macOS Keychain through `keyring`, or a plaintext table in the app DB when
//! `TABLER_SECRETS_BACKEND=file` (development only — unsigned rebuilds trigger Keychain prompts).

use uuid::Uuid;

use crate::appdb::AppDb;
use crate::error::AppError;
use crate::types::profile::SecretStatus;

const SERVICE: &str = "tabler";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    DbPassword,
    SshPassword,
    SshPassphrase,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::DbPassword => "db",
            SecretKind::SshPassword => "ssh",
            SecretKind::SshPassphrase => "ssh-passphrase",
        }
    }

    pub fn all() -> [SecretKind; 3] {
        [
            SecretKind::DbPassword,
            SecretKind::SshPassword,
            SecretKind::SshPassphrase,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Keychain,
    File,
}

#[derive(Clone)]
pub struct Secrets {
    backend: Backend,
    appdb: AppDb,
}

fn account(profile_id: Uuid, kind: SecretKind) -> String {
    format!("{profile_id}/{}", kind.suffix())
}

impl Secrets {
    pub fn new(backend: Backend, appdb: AppDb) -> Secrets {
        Secrets { backend, appdb }
    }

    /// Picks the backend: `TABLER_SECRETS_BACKEND` env var wins, then the `secretsBackend` setting,
    /// then Keychain in release builds / file in debug builds.
    pub fn from_env(appdb: AppDb) -> Secrets {
        let from_env = std::env::var("TABLER_SECRETS_BACKEND").ok();
        let from_settings = appdb
            .get_setting("secretsBackend")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(str::to_string));
        let chosen = from_env.or(from_settings).unwrap_or_else(|| {
            if cfg!(debug_assertions) {
                "file".into()
            } else {
                "keychain".into()
            }
        });
        let backend = match chosen.as_str() {
            "file" => Backend::File,
            _ => Backend::Keychain,
        };
        if backend == Backend::File {
            log::warn!(
                "secrets backend = file (plaintext in the app database); use only for development"
            );
        } else {
            log::info!("secrets backend = keychain");
        }
        Secrets::new(backend, appdb)
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub async fn get(
        &self,
        profile_id: Uuid,
        kind: SecretKind,
    ) -> Result<Option<String>, AppError> {
        let key = account(profile_id, kind);
        match self.backend {
            Backend::File => self.appdb.get_secret(&key),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
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
    }

    pub async fn set(
        &self,
        profile_id: Uuid,
        kind: SecretKind,
        value: &str,
    ) -> Result<(), AppError> {
        let key = account(profile_id, kind);
        let value = value.to_string();
        match self.backend {
            Backend::File => self.appdb.set_secret(&key, &value),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                entry
                    .set_password(&value)
                    .map_err(|e| AppError::Secrets(e.to_string()))
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }

    pub async fn delete(&self, profile_id: Uuid, kind: SecretKind) -> Result<(), AppError> {
        let key = account(profile_id, kind);
        match self.backend {
            Backend::File => self.appdb.delete_secret(&key),
            Backend::Keychain => tokio::task::spawn_blocking(move || {
                let entry = keyring::v1::Entry::new(SERVICE, &key)
                    .map_err(|e| AppError::Secrets(e.to_string()))?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::v1::Error::NoEntry) => Ok(()),
                    Err(e) => Err(AppError::Secrets(e.to_string())),
                }
            })
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?,
        }
    }

    pub async fn delete_all(&self, profile_id: Uuid) -> Result<(), AppError> {
        for kind in SecretKind::all() {
            self.delete(profile_id, kind).await?;
        }
        Ok(())
    }

    pub async fn copy_all(&self, from: Uuid, to: Uuid) -> Result<(), AppError> {
        for kind in SecretKind::all() {
            if let Some(v) = self.get(from, kind).await? {
                self.set(to, kind, &v).await?;
            }
        }
        Ok(())
    }

    pub async fn status(&self, profile_id: Uuid) -> Result<SecretStatus, AppError> {
        Ok(SecretStatus {
            has_password: self
                .get(profile_id, SecretKind::DbPassword)
                .await?
                .is_some(),
            has_ssh_password: self
                .get(profile_id, SecretKind::SshPassword)
                .await?
                .is_some(),
            has_ssh_passphrase: self
                .get(profile_id, SecretKind::SshPassphrase)
                .await?
                .is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_backend_round_trip() {
        let secrets = Secrets::new(Backend::File, AppDb::open_in_memory().unwrap());
        let id = Uuid::new_v4();
        assert_eq!(secrets.get(id, SecretKind::DbPassword).await.unwrap(), None);
        secrets.set(id, SecretKind::DbPassword, "pw").await.unwrap();
        assert_eq!(
            secrets
                .get(id, SecretKind::DbPassword)
                .await
                .unwrap()
                .as_deref(),
            Some("pw")
        );
        let status = secrets.status(id).await.unwrap();
        assert!(status.has_password && !status.has_ssh_password);
        let other = Uuid::new_v4();
        secrets.copy_all(id, other).await.unwrap();
        assert!(secrets.status(other).await.unwrap().has_password);
        secrets.delete_all(id).await.unwrap();
        assert!(!secrets.status(id).await.unwrap().has_password);
    }
}
