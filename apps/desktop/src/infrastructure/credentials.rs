use anyhow::{Context as _, Result};
use uuid::Uuid;

const SERVICE: &str = "nexus-agent-provider-profile";

pub(crate) trait CredentialStore {
    fn set_api_key(&self, profile_id: Uuid, api_key: &str) -> Result<()>;
    fn api_key(&self, profile_id: Uuid) -> Result<Option<String>>;
    fn delete_api_key(&self, profile_id: Uuid) -> Result<()>;
}

pub(crate) struct SystemCredentialStore;

impl CredentialStore for SystemCredentialStore {
    fn set_api_key(&self, profile_id: Uuid, api_key: &str) -> Result<()> {
        entry(profile_id)?
            .set_password(api_key)
            .context("写入系统凭据库")
    }

    fn api_key(&self, profile_id: Uuid) -> Result<Option<String>> {
        match entry(profile_id)?.get_password() {
            Ok(api_key) => Ok(Some(api_key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("读取系统凭据库"),
        }
    }

    fn delete_api_key(&self, profile_id: Uuid) -> Result<()> {
        match entry(profile_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("删除系统凭据"),
        }
    }
}

fn entry(profile_id: Uuid) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &profile_id.to_string()).context("打开系统凭据库")
}
