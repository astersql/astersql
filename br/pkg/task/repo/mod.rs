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

//! Snapshot repository layout and lifecycle primitives.
//!
//! A repository separates stable metadata (`_meta`) from per-store SST data
//! (`_data`).  Pending markers are indexed by immutable backup config hash so a
//! checkpoint-enabled invocation can find exactly one compatible attempt.

use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::stubs::{Error, Result, Storage};

pub const REPO_META_PATH: &str = "_meta/repo.json";
pub const ROOT_LOCK_PATH: &str = "backup.lock";
pub const CHECKPOINT_META_PATH: &str = "checkpoints/backup/checkpoint.meta";
const REPO_VERSION: u32 = 1;
const SNAPSHOT_METADATA_ROOT: &str = "_meta/snapshot";
const PENDING_ROOT: &str = "_meta/pending";
const SNAPSHOT_DATA_ROOT: &str = "_data/snapshot";
const DELETING_FILE: &str = "DELETING";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Legacy,
    Repo,
}

impl Layout {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "" | "legacy" => Ok(Self::Legacy),
            "repo" => Ok(Self::Repo),
            other => Err(Error::new(format!("unknown storage layout {other:?}"))),
        }
    }

    pub fn is_repo(self) -> bool {
        self == Self::Repo
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct BackupId(u64);

impl BackupId {
    pub fn new(ts: u64) -> Result<Self> {
        if ts == 0 {
            return Err(Error::new("backup id must not be zero"));
        }
        Ok(Self(ts))
    }

    pub fn parse(raw: &str) -> Result<Self> {
        raw.trim()
            .parse::<u64>()
            .map(Self)
            .map_err(|err| Error::new(format!("invalid backup id {raw:?}: {err}")))
    }

    pub fn parse_storage_name(raw: &str) -> Result<Self> {
        if raw.len() != 16 {
            return Err(Error::new(format!(
                "invalid backup id storage name {raw:?}: expect 16 hex characters"
            )));
        }
        u64::from_str_radix(raw, 16)
            .map(Self)
            .map_err(|err| Error::new(format!("invalid backup id storage name {raw:?}: {err}")))
    }

    pub fn storage_name(self) -> String {
        format!("{:016X}", self.0)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl Display for BackupId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub fn snapshot_metadata_dir(id: BackupId) -> String {
    format!("{SNAPSHOT_METADATA_ROOT}/{}", id.storage_name())
}

pub fn snapshot_metadata_file(id: BackupId) -> String {
    format!("{}/backupmeta", snapshot_metadata_dir(id))
}

pub fn snapshot_deleting_marker_file(id: BackupId) -> String {
    format!("{}/{DELETING_FILE}", snapshot_metadata_dir(id))
}

pub fn pending_config_hash_storage_name(hash: &[u8]) -> String {
    hex::encode_upper(hash)
}

pub fn pending_dir(hash: &[u8]) -> String {
    format!("{PENDING_ROOT}/{}", pending_config_hash_storage_name(hash))
}

pub fn pending_file(hash: &[u8], id: BackupId) -> String {
    format!("{}/{}.json", pending_dir(hash), id.storage_name())
}

pub fn snapshot_store_data_prefix(store_id: u64, id: BackupId) -> String {
    format!("{SNAPSHOT_DATA_ROOT}/{store_id}/{}", id.storage_name())
}

pub struct PrefixedStorage {
    base: Arc<dyn Storage>,
    prefix: String,
}

impl PrefixedStorage {
    pub fn new(base: Arc<dyn Storage>, prefix: impl Into<String>) -> Self {
        Self {
            base,
            prefix: prefix.into().trim_matches('/').to_string(),
        }
    }

    fn full_path(&self, name: &str) -> String {
        join_object_storage_key(&self.prefix, name)
    }

    fn trim_path<'a>(&self, name: &'a str) -> &'a str {
        name.strip_prefix(&self.prefix)
            .unwrap_or(name)
            .trim_start_matches('/')
    }
}

impl Storage for PrefixedStorage {
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>> {
        self.base.ReadFile(&self.full_path(name))
    }

    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()> {
        self.base.WriteFile(&self.full_path(name), data)
    }

    fn FileExists(&self, name: &str) -> Result<bool> {
        self.base.FileExists(&self.full_path(name))
    }

    fn DeleteFile(&self, name: &str) -> Result<()> {
        self.base.DeleteFile(&self.full_path(name))
    }

    fn WalkDir(&self, sub_dir: &str, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()> {
        let full = self.full_path(sub_dir);
        self.base
            .WalkDir(&full, &mut |path, size| f(self.trim_path(path), size))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepoMeta {
    pub repo_version: u32,
    pub repo_id: String,
    pub created_at: String,
    pub created_by: String,
}

pub fn load_repo_meta(storage: Arc<dyn Storage>) -> Result<RepoMeta> {
    let data = storage.ReadFile(REPO_META_PATH)?;
    let meta: RepoMeta = serde_json::from_slice(&data)
        .map_err(|err| Error::new(format!("decode repo metadata: {err}")))?;
    if meta.repo_version != REPO_VERSION {
        return Err(Error::new(format!(
            "unsupported repo version {}",
            meta.repo_version
        )));
    }
    Ok(meta)
}

pub fn ensure_repo(storage: Arc<dyn Storage>, created_by: &str) -> Result<RepoMeta> {
    if storage.FileExists(REPO_META_PATH)? {
        let meta = load_repo_meta(storage.clone())?;
        ensure_repo_guard(storage)?;
        return Ok(meta);
    }
    for legacy in ["backupmeta", ROOT_LOCK_PATH, CHECKPOINT_META_PATH] {
        if storage.FileExists(legacy)? {
            return Err(Error::new(format!(
                "storage already contains legacy snapshot artifact {legacy:?}"
            )));
        }
    }
    for prefix in [SNAPSHOT_METADATA_ROOT, PENDING_ROOT, SNAPSHOT_DATA_ROOT] {
        let mut found = false;
        storage.WalkDir(prefix, &mut |_, _| {
            found = true;
            Ok(())
        })?;
        if found {
            return Err(Error::new(format!(
                "storage already contains repo snapshot artifact under {prefix:?}"
            )));
        }
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::new(format!("create repo timestamp: {err}")))?;
    let meta = RepoMeta {
        repo_version: REPO_VERSION,
        repo_id: Uuid::new_v4().to_string(),
        created_at: now.as_secs().to_string(),
        created_by: created_by.to_string(),
    };
    let encoded = serde_json::to_vec(&meta)
        .map_err(|err| Error::new(format!("encode repo metadata: {err}")))?;
    storage.WriteFile(REPO_META_PATH, &encoded)?;
    ensure_repo_guard(storage)?;
    Ok(meta)
}

fn ensure_repo_guard(storage: Arc<dyn Storage>) -> Result<()> {
    if !storage.FileExists(ROOT_LOCK_PATH)? {
        storage.WriteFile(
            ROOT_LOCK_PATH,
            b"DO NOT DELETE\nThis path is managed as a BR snapshot repository.",
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingBackupState {
    Finished,
    Resumable,
    NonResumable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingBackup {
    pub backup_id: BackupId,
    pub marker_paths: Vec<String>,
    pub state: PendingBackupState,
}

pub fn list_pending_backups_for_config_hash(
    storage: Arc<dyn Storage>,
    hash: &[u8],
) -> Result<Vec<PendingBackup>> {
    let dir = pending_dir(hash);
    let mut markers = Vec::new();
    storage.WalkDir(&dir, &mut |path, _| {
        if let Some(name) = path
            .rsplit('/')
            .next()
            .and_then(|s| s.strip_suffix(".json"))
        {
            if let Ok(id) = BackupId::parse_storage_name(name) {
                markers.push((id, path.to_string()));
            }
        }
        Ok(())
    })?;
    markers.sort_by_key(|(id, _)| *id);
    let mut result = Vec::with_capacity(markers.len());
    for (id, marker) in markers {
        let state = if storage.FileExists(&snapshot_metadata_file(id))? {
            PendingBackupState::Finished
        } else if storage.FileExists(&format!(
            "{}/{}",
            snapshot_metadata_dir(id),
            CHECKPOINT_META_PATH
        ))? {
            PendingBackupState::Resumable
        } else {
            PendingBackupState::NonResumable
        };
        result.push(PendingBackup {
            backup_id: id,
            marker_paths: vec![marker],
            state,
        });
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSnapshotBackup {
    pub backup_id: BackupId,
    pub pending_marker_path: String,
    pub resume_from_checkpoint: bool,
}

pub fn prepare_snapshot_backup<F>(
    storage: Arc<dyn Storage>,
    config_hash: &[u8],
    use_checkpoint: bool,
    allocate: F,
) -> Result<PreparedSnapshotBackup>
where
    F: FnOnce() -> Result<BackupId>,
{
    let resumable: Vec<_> = list_pending_backups_for_config_hash(storage.clone(), config_hash)?
        .into_iter()
        .filter(|pending| pending.state == PendingBackupState::Resumable)
        .map(|pending| pending.backup_id)
        .collect();
    let (backup_id, resume_from_checkpoint) = if !use_checkpoint || resumable.is_empty() {
        (allocate()?, false)
    } else if resumable.len() == 1 {
        (resumable[0], true)
    } else {
        return Err(Error::new(format!(
            "found multiple unfinished repo backups for config hash {}: cannot resume an ambiguous backup",
            pending_config_hash_storage_name(config_hash)
        )));
    };
    let marker = pending_file(config_hash, backup_id);
    storage.WriteFile(&marker, b"{}")?;
    Ok(PreparedSnapshotBackup {
        backup_id,
        pending_marker_path: marker,
        resume_from_checkpoint,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotDeleteResult {
    pub backup_id: BackupId,
    pub deleted_files: usize,
}

pub fn delete_snapshot(
    storage: Arc<dyn Storage>,
    backup_id: BackupId,
) -> Result<SnapshotDeleteResult> {
    let deleting = snapshot_deleting_marker_file(backup_id);
    storage.WriteFile(&deleting, b"{}")?;
    let storage_name = backup_id.storage_name();
    let metadata_prefix = snapshot_metadata_dir(backup_id);
    let mut paths = Vec::new();
    for prefix in [PENDING_ROOT, &metadata_prefix, SNAPSHOT_DATA_ROOT] {
        storage.WalkDir(prefix, &mut |path, _| {
            let selected = if prefix == PENDING_ROOT {
                path.ends_with(&format!("/{storage_name}.json"))
            } else if prefix == SNAPSHOT_DATA_ROOT {
                path.split('/').any(|part| part == storage_name)
            } else {
                true
            };
            if selected {
                paths.push(path.to_string());
            }
            Ok(())
        })?;
    }
    paths.sort();
    paths.dedup();
    for path in &paths {
        storage.DeleteFile(path)?;
    }
    Ok(SnapshotDeleteResult {
        backup_id,
        deleted_files: paths.len(),
    })
}

pub fn rewrite_data_backend_prefix(base: &str, store_id: u64, id: BackupId) -> String {
    join_object_storage_key(base, &snapshot_store_data_prefix(store_id, id))
}

pub fn rewrite_data_file_name(name: &str, store_id: u64, id: BackupId) -> String {
    join_object_storage_key(&snapshot_store_data_prefix(store_id, id), name)
}

/// Legacy checkpoint metadata uses zero; repository-aware metadata must match
/// the selected pending backup before resume is activated.
pub fn validate_checkpoint_backup_id(checkpoint_backup_id: u64, selected: BackupId) -> Result<()> {
    if checkpoint_backup_id != 0 && checkpoint_backup_id != selected.value() {
        return Err(Error::new(format!(
            "repo checkpoint metadata backup id {checkpoint_backup_id} doesn't match resumed backup {selected}"
        )));
    }
    Ok(())
}

fn join_object_storage_key(base: &str, suffix: &str) -> String {
    match (base.trim_matches('/'), suffix.trim_matches('/')) {
        ("", suffix) => suffix.to_string(),
        (base, "") => base.to_string(),
        (base, suffix) => format!("{base}/{suffix}"),
    }
}
