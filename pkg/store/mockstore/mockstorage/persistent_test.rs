// Copyright 2026 AsterSQL.

use crate::KVStore;

#[test]
fn persistent_transactions_reopen_read_only_without_changing_journal() {
    let dir = tempfile::tempdir().unwrap();
    let store = KVStore::OpenPersistent(dir.path(), false).unwrap();
    let mut txn = store.Begin(&[]).unwrap();
    txn.Set(b"system", b"metadata");
    txn.Set(b"deleted", b"old");
    txn.Commit().unwrap();
    let mut txn = store.Begin(&[]).unwrap();
    txn.Delete(b"deleted");
    txn.Commit().unwrap();
    assert!(KVStore::OpenPersistent(dir.path(), false).is_err());
    store.Close().unwrap();
    let before = std::fs::read(dir.path().join("canonical-kv.wal")).unwrap();
    let store = KVStore::OpenPersistent(dir.path(), true).unwrap();
    let mut read = store.Begin(&[]).unwrap();
    assert_eq!(read.Get(b"system"), Some(b"metadata".to_vec()));
    read.Commit()
        .expect("read-only transactions can commit without a write record");
    let mut txn = store.Begin(&[]).unwrap();
    assert_eq!(txn.Get(b"system"), Some(b"metadata".to_vec()));
    assert_eq!(txn.Get(b"deleted"), None);
    txn.Set(b"system", b"changed");
    assert!(txn.Commit().is_err());
    assert_eq!(
        store.Begin(&[]).unwrap().Get(b"system"),
        Some(b"metadata".to_vec())
    );
    store.Close().unwrap();
    assert_eq!(
        before,
        std::fs::read(dir.path().join("canonical-kv.wal")).unwrap()
    );
}

#[test]
fn persistent_replay_recovers_incomplete_tail_and_rejects_corruption() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let store = KVStore::OpenPersistent(dir.path(), false).unwrap();
    let mut txn = store.Begin(&[]).unwrap();
    txn.Set(b"k", b"v");
    txn.Commit().unwrap();
    store.Close().unwrap();
    let path = dir.path().join("canonical-kv.wal");
    let len = std::fs::metadata(&path).unwrap().len();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"[partial")
        .unwrap();
    let store = KVStore::OpenPersistent(dir.path(), false).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
    assert_eq!(store.Begin(&[]).unwrap().Get(b"k"), Some(b"v".to_vec()));
    store.Close().unwrap();
    std::fs::write(path, b"corrupt\n").unwrap();
    assert!(KVStore::OpenPersistent(dir.path(), false).is_err());
}

#[test]
fn persistent_open_does_not_replace_an_unrecognized_store() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("existing-data"), b"keep").unwrap();
    assert!(KVStore::OpenPersistent(dir.path(), false).is_err());
    assert!(!dir.path().join("canonical-kv.wal").exists());
    assert_eq!(
        std::fs::read(dir.path().join("existing-data")).unwrap(),
        b"keep"
    );
}
