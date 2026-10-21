// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;

use crate::repo::*;
use crate::stubs::{MemStorage, Storage};

#[test]
fn snapshot_repo_layout_and_backup_id() {
    assert_eq!(Layout::parse(" ").unwrap(), Layout::Legacy);
    assert_eq!(Layout::parse("repo").unwrap(), Layout::Repo);
    assert!(Layout::parse("other").is_err());
    assert!(BackupId::new(0).is_err());
    let id = BackupId::new(0xf00d).unwrap();
    assert_eq!(id.to_string(), "61453");
    assert_eq!(id.storage_name(), "000000000000F00D");
    assert_eq!(
        BackupId::parse_storage_name(&id.storage_name()).unwrap(),
        id
    );
    assert_eq!(snapshot_metadata_dir(id), "_meta/snapshot/000000000000F00D");
    assert_eq!(
        pending_file(&[0xab, 0xcd], id),
        "_meta/pending/ABCD/000000000000F00D.json"
    );
    assert_eq!(
        snapshot_store_data_prefix(9, id),
        "_data/snapshot/9/000000000000F00D"
    );
}

#[test]
fn snapshot_repo_prefixed_storage_maps_all_paths() {
    let root = Arc::new(MemStorage::new());
    let prefixed = PrefixedStorage::new(root.clone(), "_meta/snapshot/ID");
    prefixed.WriteFile("checkpoint.meta", b"ok").unwrap();
    assert_eq!(
        root.ReadFile("_meta/snapshot/ID/checkpoint.meta").unwrap(),
        b"ok"
    );
    assert_eq!(prefixed.ReadFile("checkpoint.meta").unwrap(), b"ok");
    let mut paths = vec![];
    prefixed
        .WalkDir("", &mut |path, _| {
            paths.push(path.to_string());
            Ok(())
        })
        .unwrap();
    assert_eq!(paths, vec!["checkpoint.meta"]);
    prefixed.DeleteFile("checkpoint.meta").unwrap();
    assert!(
        !root
            .FileExists("_meta/snapshot/ID/checkpoint.meta")
            .unwrap()
    );
}

#[test]
fn snapshot_repo_ensure_repo_rejects_legacy_and_is_idempotent() {
    let legacy = Arc::new(MemStorage::new());
    legacy.WriteFile("backupmeta", b"legacy").unwrap();
    assert!(
        ensure_repo(legacy, "br test")
            .unwrap_err()
            .to_string()
            .contains("legacy snapshot artifact")
    );

    let root = Arc::new(MemStorage::new());
    let first = ensure_repo(root.clone(), "br test").unwrap();
    let second = ensure_repo(root.clone(), "ignored").unwrap();
    assert_eq!(first.repo_id, second.repo_id);
    assert_eq!(first.created_by, "br test");
    assert!(root.FileExists(REPO_META_PATH).unwrap());
    assert!(root.FileExists(ROOT_LOCK_PATH).unwrap());
}

#[test]
fn snapshot_repo_list_pending_classifies_states() {
    let root = Arc::new(MemStorage::new());
    let hash = [0x12, 0x34];
    let finished = BackupId::new(1).unwrap();
    let resumable = BackupId::new(2).unwrap();
    let marker_only = BackupId::new(3).unwrap();
    for id in [finished, resumable, marker_only] {
        root.WriteFile(&pending_file(&hash, id), b"{}").unwrap();
    }
    root.WriteFile(&snapshot_metadata_file(finished), b"meta")
        .unwrap();
    root.WriteFile(
        &format!(
            "{}/{}",
            snapshot_metadata_dir(resumable),
            CHECKPOINT_META_PATH
        ),
        b"checkpoint",
    )
    .unwrap();
    let pending = list_pending_backups_for_config_hash(root, &hash).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|p| (p.backup_id, p.state))
            .collect::<Vec<_>>(),
        vec![
            (finished, PendingBackupState::Finished),
            (resumable, PendingBackupState::Resumable),
            (marker_only, PendingBackupState::NonResumable),
        ]
    );
}

#[test]
fn snapshot_repo_prepare_resume_and_ambiguity() {
    let root = Arc::new(MemStorage::new());
    let hash = [7, 8];
    let resumable = BackupId::new(42).unwrap();
    root.WriteFile(&pending_file(&hash, resumable), b"{}")
        .unwrap();
    root.WriteFile(
        &format!(
            "{}/{}",
            snapshot_metadata_dir(resumable),
            CHECKPOINT_META_PATH
        ),
        b"checkpoint",
    )
    .unwrap();
    let prepared =
        prepare_snapshot_backup(root.clone(), &hash, true, || BackupId::new(99)).unwrap();
    assert_eq!(prepared.backup_id, resumable);
    assert!(prepared.resume_from_checkpoint);
    let second = BackupId::new(43).unwrap();
    root.WriteFile(&pending_file(&hash, second), b"{}").unwrap();
    root.WriteFile(
        &format!("{}/{}", snapshot_metadata_dir(second), CHECKPOINT_META_PATH),
        b"checkpoint",
    )
    .unwrap();
    assert!(
        prepare_snapshot_backup(root, &hash, true, || BackupId::new(100))
            .unwrap_err()
            .to_string()
            .contains("multiple unfinished")
    );
}

#[test]
fn snapshot_repo_without_checkpoint_starts_fresh_despite_pending() {
    let root = Arc::new(MemStorage::new());
    let hash = [9];
    let old = BackupId::new(10).unwrap();
    root.WriteFile(&pending_file(&hash, old), b"{}").unwrap();
    root.WriteFile(
        &format!("{}/{}", snapshot_metadata_dir(old), CHECKPOINT_META_PATH),
        b"checkpoint",
    )
    .unwrap();
    let prepared = prepare_snapshot_backup(root, &hash, false, || BackupId::new(11)).unwrap();
    assert_eq!(prepared.backup_id, BackupId::new(11).unwrap());
    assert!(!prepared.resume_from_checkpoint);
}

#[test]
fn snapshot_repo_delete_removes_only_selected_snapshot() {
    let root = Arc::new(MemStorage::new());
    let hash = [1];
    let selected = BackupId::new(20).unwrap();
    let other = BackupId::new(21).unwrap();
    for id in [selected, other] {
        root.WriteFile(&pending_file(&hash, id), b"{}").unwrap();
        root.WriteFile(&snapshot_metadata_file(id), b"meta")
            .unwrap();
        root.WriteFile(
            &format!("{}/a.sst", snapshot_store_data_prefix(1, id)),
            b"sst",
        )
        .unwrap();
    }
    let result = delete_snapshot(root.clone(), selected).unwrap();
    assert_eq!(result.backup_id, selected);
    assert_eq!(result.deleted_files, 4);
    assert!(!root.FileExists(&snapshot_metadata_file(selected)).unwrap());
    assert!(root.FileExists(&snapshot_metadata_file(other)).unwrap());
}

#[test]
fn snapshot_repo_rewrites_store_paths_and_response_files() {
    let id = BackupId::new(0xf00d).unwrap();
    assert_eq!(
        rewrite_data_backend_prefix("bucket/base", 7, id),
        "bucket/base/_data/snapshot/7/000000000000F00D"
    );
    assert_eq!(
        rewrite_data_file_name("1_2_3.sst", 7, id),
        "_data/snapshot/7/000000000000F00D/1_2_3.sst"
    );
}

#[test]
fn snapshot_repo_resume_rejects_mismatched_checkpoint_backup_id() {
    let selected = BackupId::new(123).unwrap();
    validate_checkpoint_backup_id(0, selected).unwrap();
    validate_checkpoint_backup_id(123, selected).unwrap();
    assert!(
        validate_checkpoint_backup_id(124, selected)
            .unwrap_err()
            .to_string()
            .contains("doesn't match resumed backup")
    );
}
