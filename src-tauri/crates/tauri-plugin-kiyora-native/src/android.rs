use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{path::PathBuf, time::Duration};
use tauri::{Runtime, plugin::PluginHandle};
use vault_security::{SecretStore, SecurityError};
use zeroize::{Zeroize, Zeroizing};

pub struct AndroidSecrets<R: Runtime>(PluginHandle<R>);

#[derive(Serialize)]
struct GetSecret<'a> {
    key: &'a str,
}
#[derive(Serialize)]
struct SetSecret<'a> {
    key: &'a str,
    value: &'a [u8],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretResponse {
    value: Option<Vec<u8>>,
}
impl Drop for SecretResponse {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoragePaths {
    pub profiles: PathBuf,
    pub tdjson: PathBuf,
}

impl<R: Runtime> Clone for AndroidSecrets<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<R: Runtime> AndroidSecrets<R> {
    pub(crate) fn new(handle: PluginHandle<R>) -> Self {
        Self(handle)
    }

    /// Tauri setup runs on the Rust event thread. Other callers must use a
    /// blocking worker; never call this bridge on Android's main looper.
    fn call<T: DeserializeOwned>(
        &self,
        command: &str,
        payload: impl Serialize,
    ) -> Result<T, SecurityError> {
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(
                Duration::from_secs(15),
                self.0.run_mobile_plugin_async(command, payload),
            )
            .await
            .map_err(|_| SecurityError::SecureStorageUnavailable)?
            .map_err(|_| SecurityError::SecureStorageUnavailable)
        })
    }

    pub fn storage_paths(&self) -> Result<StoragePaths, SecurityError> {
        self.call("storagePaths", ())
    }
}

impl<R: Runtime> SecretStore for AndroidSecrets<R> {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
        let mut response: SecretResponse = self.call("getSecret", GetSecret { key })?;
        let value = response.value.take().map(Zeroizing::new);
        if value.as_ref().is_some_and(|bytes| bytes.len() != 32) {
            return Err(SecurityError::SecureStorageUnavailable);
        }
        Ok(value)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
        if value.len() != 32 {
            return Err(SecurityError::SecureStorageUnavailable);
        }
        self.call("setSecret", SetSecret { key, value })
    }
}
