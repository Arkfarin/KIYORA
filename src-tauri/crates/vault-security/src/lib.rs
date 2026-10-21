//! Per-profile isolation, OS locking and fail-closed secret storage.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use vault_domain::UserId;
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("profile filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("profile is already in use by another process")]
    ProfileBusy,
    #[error("invalid profile metadata or path")]
    InvalidProfile,
    #[error("Telegram account does not match this profile")]
    AccountMismatch,
    #[error("OS secure storage is unavailable; no plaintext fallback is allowed")]
    SecureStorageUnavailable,
    #[error("TDLib database exists but its secure key is missing")]
    MissingKey,
    #[error("OS random generator failed")]
    Random,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub profile_id: Uuid,
    pub label: String,
    pub owner_id: Option<UserId>,
}

pub struct ProfileStore {
    root: PathBuf,
}
impl ProfileStore {
    pub fn default_location() -> Result<Self, SecurityError> {
        let dirs = directories::ProjectDirs::from("io.github", "arkfarin", "kiyora")
            .ok_or(SecurityError::InvalidProfile)?;
        Self::at(dirs.data_local_dir())
    }
    pub fn at(root: impl AsRef<Path>) -> Result<Self, SecurityError> {
        secure_dir(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        secure_dir(&root.join("profiles"))?;
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn create(&self, label: String) -> Result<Profile, SecurityError> {
        if label.trim().is_empty() || label.len() > 128 || label.contains('\0') {
            return Err(SecurityError::InvalidProfile);
        }
        let profile = Profile {
            profile_id: Uuid::new_v4(),
            label,
            owner_id: None,
        };
        let dir = self
            .root
            .join("profiles")
            .join(profile.profile_id.to_string());
        secure_dir(&dir)?;
        save_profile(&dir, &profile)?;
        Ok(profile)
    }
    pub fn list(&self) -> Result<Vec<Profile>, SecurityError> {
        let mut result = Vec::new();
        for entry in fs::read_dir(self.root.join("profiles"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(id) = name.parse::<Uuid>() else {
                continue;
            };
            if name != id.to_string() || entry.file_type()?.is_symlink() {
                return Err(SecurityError::InvalidProfile);
            }
            let profile = read_profile(&entry.path())?;
            if profile.profile_id != id {
                return Err(SecurityError::InvalidProfile);
            }
            result.push(profile);
        }
        result.sort_by_key(|p| p.profile_id);
        Ok(result)
    }
    pub fn acquire(&self, id: Uuid) -> Result<ProfileLease, SecurityError> {
        let dir = self.root.join("profiles").join(id.to_string());
        if id.is_nil() || fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            return Err(SecurityError::InvalidProfile);
        }
        let profile = read_profile(&dir)?;
        if profile.profile_id != id {
            return Err(SecurityError::InvalidProfile);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(dir.join("profile.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| SecurityError::ProfileBusy)?;
        for name in ["tdlib", "files", "cache"] {
            secure_dir(&dir.join(name))?;
        }
        Ok(ProfileLease {
            profile,
            dir,
            _lock: lock,
        })
    }
}

pub struct ProfileLease {
    profile: Profile,
    dir: PathBuf,
    _lock: File,
}
impl ProfileLease {
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn directory(&self) -> &Path {
        &self.dir
    }
    pub fn database_path(&self) -> PathBuf {
        self.dir.join("vault.sqlite")
    }
    pub fn tdlib_path(&self) -> PathBuf {
        self.dir.join("tdlib")
    }
    pub fn files_path(&self) -> PathBuf {
        self.dir.join("files")
    }
    pub fn cache_path(&self) -> PathBuf {
        self.dir.join("cache")
    }
    pub fn bind_owner(&mut self, owner: UserId) -> Result<(), SecurityError> {
        if self
            .profile
            .owner_id
            .is_some_and(|existing| existing != owner)
        {
            return Err(SecurityError::AccountMismatch);
        }
        let mut profile = self.profile.clone();
        profile.owner_id = Some(owner);
        save_profile(&self.dir, &profile)?;
        self.profile = profile;
        Ok(())
    }
    pub fn database_key(
        &self,
        store: &impl SecretStore,
    ) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
        let key = format!("{}:tdlib-key", self.profile.profile_id);
        if let Some(secret) = store.get(&key)? {
            if secret.len() != 32 {
                return Err(SecurityError::MissingKey);
            }
            return Ok(secret);
        }
        if fs::read_dir(self.tdlib_path())?
            .next()
            .transpose()?
            .is_some()
        {
            return Err(SecurityError::MissingKey);
        }
        let mut secret = Zeroizing::new(vec![0u8; 32]);
        getrandom::fill(&mut secret).map_err(|_| SecurityError::Random)?;
        store.set(&key, &secret)?;
        // Verify persistence before allowing TDLib to create an encrypted database.
        let verified = store
            .get(&key)?
            .ok_or(SecurityError::SecureStorageUnavailable)?;
        if *verified != *secret {
            return Err(SecurityError::SecureStorageUnavailable);
        }
        Ok(secret)
    }
}

fn secure_dir(path: &Path) -> Result<(), SecurityError> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(SecurityError::InvalidProfile);
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn read_profile(dir: &Path) -> Result<Profile, SecurityError> {
    let path = dir.join("profile.json");
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(SecurityError::InvalidProfile);
    }
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(SecurityError::InvalidProfile);
    }
    serde_json::from_slice(&bytes).map_err(|_| SecurityError::InvalidProfile)
}
fn save_profile(dir: &Path, profile: &Profile) -> Result<(), SecurityError> {
    let bytes = serde_json::to_vec(profile).map_err(|_| SecurityError::InvalidProfile)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join("profile.json"))
        .map_err(|e| SecurityError::Io(e.error))?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub trait SecretStore {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError>;
}
pub struct OsSecrets;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl SecretStore for OsSecrets {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
        let entry = keyring::Entry::new("io.github.arkfarin.kiyora", key)
            .map_err(|_| SecurityError::SecureStorageUnavailable)?;
        match entry.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecurityError::SecureStorageUnavailable),
        }
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
        keyring::Entry::new("io.github.arkfarin.kiyora", key)
            .map_err(|_| SecurityError::SecureStorageUnavailable)?
            .set_secret(value)
            .map_err(|_| SecurityError::SecureStorageUnavailable)
    }
}
// Android must use the native Keystore provider. Never inherit keyring's mock default.
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
impl SecretStore for OsSecrets {
    fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
        Err(SecurityError::SecureStorageUnavailable)
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), SecurityError> {
        Err(SecurityError::SecureStorageUnavailable)
    }
}
