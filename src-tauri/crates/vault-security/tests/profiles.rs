#![allow(clippy::unwrap_used)]
use std::{cell::RefCell, collections::BTreeMap};
use vault_domain::UserId;
use vault_security::*;
use zeroize::Zeroizing;

#[derive(Default)]
struct TestSecrets(RefCell<BTreeMap<String, Vec<u8>>>);
impl SecretStore for TestSecrets {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
        Ok(self.0.borrow().get(key).cloned().map(Zeroizing::new))
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
        self.0.borrow_mut().insert(key.into(), value.into());
        Ok(())
    }
}
#[test]
fn profile_lock_owner_and_secret_isolation_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::at(dir.path()).unwrap();
    let a = store.create("A".into()).unwrap();
    let b = store.create("B".into()).unwrap();
    let mut first = store.acquire(a.profile_id).unwrap();
    assert!(matches!(
        store.acquire(a.profile_id),
        Err(SecurityError::ProfileBusy)
    ));
    first.bind_owner(UserId::new(10).unwrap()).unwrap();
    assert!(matches!(
        first.bind_owner(UserId::new(11).unwrap()),
        Err(SecurityError::AccountMismatch)
    ));
    let secrets = TestSecrets::default();
    let key = first.database_key(&secrets).unwrap();
    assert_eq!(*key, *first.database_key(&secrets).unwrap());
    let second = store.acquire(b.profile_id).unwrap();
    assert_ne!(*key, *second.database_key(&secrets).unwrap());
    assert_ne!(first.database_path(), second.database_path());
    drop(first);
    let first = store.acquire(a.profile_id).unwrap();
    assert_eq!(first.profile().owner_id, Some(UserId::new(10).unwrap()));
    assert_eq!(store.list().unwrap().len(), 2);
    std::fs::write(first.tdlib_path().join("db.sqlite"), b"encrypted").unwrap();
    assert!(matches!(
        first.database_key(&TestSecrets::default()),
        Err(SecurityError::MissingKey)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_profile_and_lock_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::at(dir.path()).unwrap();
    let p = store.create("A".into()).unwrap();
    let profile_dir = dir.path().join("profiles").join(p.profile_id.to_string());
    std::os::unix::fs::symlink(
        dir.path().join("unrelated"),
        profile_dir.join("profile.lock"),
    )
    .unwrap();
    assert!(store.acquire(p.profile_id).is_err());
}
