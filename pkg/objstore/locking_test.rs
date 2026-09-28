// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 远程锁单元测试：互斥/读写锁、元数据、冲突报告、重试与错误富化。

use std::any::Any;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{TimeZone, Utc};
use objstore::local::NewLocalStorage;
use objstore::locking::*;
use objstore::memstore::NewMemStorage;
use objstore::storage::{
    Context, ObjectReader, ObjectWriter, ReaderOption, Storage, StorageRef, WalkOption,
    WriterOption,
};

/// 创建临时目录上的 LocalStorage 作为锁后端。
fn create_mock_storage() -> (tempfile::TempDir, StorageRef) {
    let temp = tempfile::tempdir().unwrap();
    let storage: StorageRef = Arc::new(NewLocalStorage(temp.path()).unwrap());
    (temp, storage)
}

/// 仅填充 hint 的锁输入。
fn hint(text: &str) -> LockMetaInput {
    LockMetaInput {
        hint: text.into(),
        ..Default::default()
    }
}

/// 构造完整 LockMetaInput。
fn lock_input(owner_id: &str, lock_type: &str, hint: &str) -> LockMetaInput {
    LockMetaInput {
        owner_id: owner_id.into(),
        lock_type: lock_type.into(),
        hint: hint.into(),
    }
}

#[test]
/// 加锁后文件存在，Unlock 后删除。
fn test_try_lock_remote() {
    let ctx = Context::background();
    let (temp, storage) = create_mock_storage();
    let lock = TryLockRemote(&ctx, storage, "test.lock", hint("This file is mine!")).unwrap();
    assert!(temp.path().join("test.lock").exists());
    lock.Unlock(&ctx).unwrap();
    assert!(!temp.path().join("test.lock").exists());
}

#[test]
/// 二次互斥加锁失败并报告 conflict file。
fn test_conflict_lock() {
    let ctx = Context::background();
    let (temp, storage) = create_mock_storage();
    let lock = TryLockRemote(
        &ctx,
        storage.clone(),
        "test.lock",
        hint("This file is mine!"),
    )
    .unwrap();
    let error = TryLockRemote(&ctx, storage, "test.lock", hint("This file is mine!")).unwrap_err();
    assert!(error.to_string().contains("conflict file test.lock"));
    assert!(temp.path().join("test.lock").exists());
    lock.Unlock(&ctx).unwrap();
}

#[test]
/// 多读共存、读写互斥、写锁落盘为 `.WRIT`。
fn test_rw_lock() {
    let ctx = Context::background();
    let (temp, storage) = create_mock_storage();
    let read1 = TryLockRemoteRead(&ctx, storage.clone(), "test.lock", hint("reader 1")).unwrap();
    let read2 = TryLockRemoteRead(&ctx, storage.clone(), "test.lock", hint("reader 2")).unwrap();
    assert!(TryLockRemoteWrite(&ctx, storage.clone(), "test.lock", hint("writer")).is_err());
    read1.Unlock(&ctx).unwrap();
    read2.Unlock(&ctx).unwrap();
    let write = TryLockRemoteWrite(&ctx, storage.clone(), "test.lock", hint("writer")).unwrap();
    assert!(temp.path().join("test.lock.WRIT").exists());
    assert!(TryLockRemoteRead(&ctx, storage, "test.lock", hint("reader 3")).is_err());
    write.Unlock(&ctx).unwrap();
    assert!(!temp.path().join("test.lock.WRIT").exists());
}

#[test]
/// 两并发 TryLockRemote 仅一人成功。
fn test_concurrent_lock() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for owner in ["a", "b"] {
        let barrier = barrier.clone();
        let storage = storage.clone();
        let ctx = ctx.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            TryLockRemote(
                &ctx,
                storage,
                "test.lock",
                lock_input(owner, "exclusive", owner),
            )
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    results
        .into_iter()
        .flatten()
        .next()
        .unwrap()
        .Unlock(&ctx)
        .unwrap();
}

#[test]
/// Context 取消后 UnlockOnCleanUp 仍能删锁。
fn test_unlock_on_clean_up() {
    let ctx = Context::background();
    let (temp, storage) = create_mock_storage();
    let lock = TryLockRemote(&ctx, storage, "test.lock", hint("This file is mine!")).unwrap();
    ctx.cancel();
    lock.UnlockOnCleanUp(&ctx);
    assert!(!temp.path().join("test.lock").exists());
}

#[test]
/// MakeLockMeta 填充主机/PID，且不含过时 JSON 字段。
fn test_make_lock_meta() {
    for (input, owner, lock_type, hint_text) in [
        (
            lock_input("op-1", "migration-read", "holder"),
            "op-1",
            "migration-read",
            "holder",
        ),
        (hint("minimal"), "", "", "minimal"),
    ] {
        let meta = MakeLockMeta(input);
        assert_eq!(meta.owner_id, owner);
        assert_eq!(meta.lock_type, lock_type);
        assert_eq!(meta.hint, hint_text);
        assert!(!meta.locker_host.is_empty());
        assert_eq!(meta.locker_pid, std::process::id());
        let json = serde_json::to_string(&meta).unwrap();
        for obsolete in ["operation_started_at", "restore_id", "resource_type"] {
            assert!(!json.contains(obsolete));
        }
    }
}

#[test]
/// 旧版 JSON（无 owner/lock_type）仍可反序列化。
fn test_lock_meta_old_json_compatibility() {
    let meta: LockMeta = serde_json::from_str(r#"{"locked_at":"2026-06-15T01:02:03Z","locker_host":"host-a","locker_pid":123,"txn_id":"dHhu","hint":"old"}"#).unwrap();
    assert_eq!(meta.locker_host, "host-a");
    assert_eq!(meta.locker_pid, 123);
    assert_eq!(meta.txn_id, b"txn");
    assert_eq!(meta.hint, "old");
    assert!(meta.owner_id.is_empty());
    assert!(meta.lock_type.is_empty());
}

#[test]
/// Display 含时间/主机/hint/owner，不含 txn 原文。
fn test_lock_meta_string_includes_owner_fields() {
    let meta = LockMeta {
        locked_at: Utc.with_ymd_and_hms(2026, 6, 15, 4, 5, 6).unwrap(),
        locker_host: "host-a".into(),
        locker_pid: 123,
        txn_id: b"txn".to_vec(),
        hint: r#"restore_id=456 detail="hint-a""#.into(),
        owner_id: "op-1".into(),
        lock_type: "migration-write".into(),
    };
    let value = meta.to_string();
    for expected in [
        "2026-06-15 04:05:06",
        "host-a",
        "123",
        "hint-a",
        "op-1",
        "456",
        "migration-write",
    ] {
        assert!(value.contains(expected));
    }
    assert!(!value.to_lowercase().contains("txn_id"));
    assert!(!value.to_lowercase().contains("txn"));
}

#[test]
/// ErrLocked 展示 blocker 上限并标注省略数。
fn test_err_locked_error_limits_blocker_output() {
    let error = ErrLocked {
        path: "test.lock.WRIT".into(),
        blocker_count: 4,
        blockers: (0..4)
            .map(|index| LockBlocker {
                path: format!("test.lock.READ.{index}"),
                meta: LockMeta {
                    owner_id: format!("op-{index}"),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let value = error.to_string();
    for index in 0..3 {
        assert!(value.contains(&format!("test.lock.READ.{index}")));
    }
    assert!(!value.contains("test.lock.READ.3"));
    assert!(value.contains("omitted_conflict_files = 1"));
}

#[test]
/// 冲突错误携带远程 path 与 meta。
fn test_conflict_lock_reports_remote_path_and_meta() {
    let ctx = Context::background();
    let (_temp, storage) = create_mock_storage();
    let lock = TryLockRemote(
        &ctx,
        storage.clone(),
        "test.lock",
        lock_input("remote-op", "migration-read", "remote"),
    )
    .unwrap();
    let local = lock_input("local-op", "migration-write", "local");
    let error = TryLockRemote(&ctx, storage, "test.lock", local.clone()).unwrap_err();
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.path, "test.lock");
    assert_eq!(locked.local, local);
    assert_eq!(locked.meta.owner_id, "remote-op");
    assert_eq!(locked.meta.lock_type, "migration-read");
    lock.Unlock(&ctx).unwrap();
}

/// 构造“已有读锁 → 写锁失败”场景。
fn create_read_write_conflict(
    remote: LockMetaInput,
    local: LockMetaInput,
) -> (StorageRef, RemoteLock, anyhow::Error) {
    let storage: StorageRef = Arc::new(NewMemStorage());
    let ctx = Context::background();
    let read = TryLockRemoteRead(&ctx, storage.clone(), "test.lock", remote).unwrap();
    let error = TryLockRemoteWrite(&ctx, storage.clone(), "test.lock", local).unwrap_err();
    (storage, read, error)
}

#[test]
/// 写锁冲突应报告单个读锁 blocker。
fn test_write_lock_conflict_reports_read_lock_blocker() {
    let local = lock_input("write-op", "migration-write", "writer");
    let (_storage, read, error) = create_read_write_conflict(
        lock_input("read-op", "migration-read", "reader"),
        local.clone(),
    );
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.path, "test.lock.WRIT");
    assert_eq!(locked.local, local);
    assert_eq!(locked.blockers.len(), 1);
    assert!(locked.blockers[0].path.contains("test.lock.READ."));
    assert_eq!(locked.blockers[0].meta.owner_id, "read-op");
    assert_eq!(locked.blockers[0].meta.lock_type, "migration-read");
    assert!(locked.blockers[0].error.is_none());
    read.Unlock(&Context::background()).unwrap();
}

#[test]
/// 多个读锁均出现在 blockers 中。
fn test_write_lock_conflict_reports_multiple_read_lock_blockers() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    let read1 = TryLockRemoteRead(
        &ctx,
        storage.clone(),
        "test.lock",
        lock_input("read-op", "migration-read", "reader"),
    )
    .unwrap();
    let read2 = TryLockRemoteRead(
        &ctx,
        storage.clone(),
        "test.lock",
        lock_input("read-op", "migration-read", "reader"),
    )
    .unwrap();
    let error = TryLockRemoteWrite(
        &ctx,
        storage,
        "test.lock",
        lock_input("write-op", "migration-write", "writer"),
    )
    .unwrap_err();
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.blocker_count, 2);
    assert_eq!(locked.blockers.len(), 2);
    assert!(
        locked
            .blockers
            .iter()
            .all(|blocker| blocker.meta.owner_id == "read-op")
    );
    read1.Unlock(&ctx).unwrap();
    read2.Unlock(&ctx).unwrap();
}

#[test]
/// 超过 META 上限时只采样部分 blocker，日志字段同步截断。
fn test_write_lock_conflict_samples_read_lock_blockers() {
    let ctx = Context::background();
    let storage: StorageRef = Arc::new(NewMemStorage());
    let mut reads = Vec::new();
    for _ in 0..5 {
        reads.push(
            TryLockRemoteRead(
                &ctx,
                storage.clone(),
                "test.lock",
                lock_input("read-op", "migration-read", "reader"),
            )
            .unwrap(),
        );
    }
    let local = lock_input("write-op", "migration-write", "writer");
    let error = TryLockRemoteWrite(&ctx, storage, "test.lock", local.clone()).unwrap_err();
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.blocker_count, 5);
    assert_eq!(locked.blockers.len(), 3);
    assert!(error.to_string().contains("omitted_conflict_files = 2"));
    let fields = LockConflictLogFields("test.lock", &local, &error);
    require_field(&fields, "remote_blocker_count", "5");
    for index in 0..3 {
        require_field(
            &fields,
            &format!("remote_blocker_{index}_owner_id"),
            "read-op",
        );
    }
    for read in reads {
        read.Unlock(&ctx).unwrap();
    }
}

#[test]
/// LockWithRetry 取消时仍保留 ErrLocked 本地/远程元数据。
fn test_lock_with_retry_carries_local_and_remote_metadata() {
    let local = lock_input("local-op", "migration-write", "writer");
    let (_storage, read, initial) = create_read_write_conflict(
        lock_input("remote-op", "migration-read", "reader"),
        local.clone(),
    );
    assert!(initial.downcast_ref::<ErrLocked>().is_some());
    let storage: StorageRef = Arc::new(NewMemStorage());
    let read2 = TryLockRemoteRead(
        &Context::background(),
        storage.clone(),
        "test.lock",
        lock_input("remote-op", "migration-read", "reader"),
    )
    .unwrap();
    let ctx = Context::background();
    let cancel = ctx.clone();
    let error = LockWithRetry(
        &ctx,
        move |_ctx, storage, path, input| {
            let result = TryLockRemoteWrite(&Context::background(), storage, path, input);
            cancel.cancel();
            result
        },
        storage,
        "test.lock",
        local.clone(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("context canceled"));
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.local, local);
    assert_eq!(locked.blockers[0].meta.owner_id, "remote-op");
    read.Unlock(&Context::background()).unwrap();
    read2.Unlock(&Context::background()).unwrap();
}

#[test]
/// LockConflictLogFields 导出本地与远程字段。
fn test_lock_conflict_log_fields_carries_local_and_remote_metadata() {
    let local = lock_input("local-op", "migration-write", "writer");
    let (_storage, read, error) = create_read_write_conflict(
        lock_input("remote-op", "migration-read", "reader"),
        local.clone(),
    );
    let fields = LockConflictLogFields("test.lock", &local, &error);
    for (key, value) in [
        ("path", "test.lock"),
        ("local_owner_id", "local-op"),
        ("local_lock_type", "migration-write"),
        ("remote_blocker_0_owner_id", "remote-op"),
        ("remote_blocker_0_lock_type", "migration-read"),
        ("remote_blocker_0_hint", "reader"),
    ] {
        require_field(&fields, key, value);
    }
    read.Unlock(&Context::background()).unwrap();
}

/// 代理存储：WalkDir 注入错误以测错误富化路径。
struct WalkErrorStorage {
    inner: StorageRef,
    error: std::sync::Mutex<Option<anyhow::Error>>,
}

impl Storage for WalkErrorStorage {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()> {
        self.inner.DeleteFile(ctx, name)
    }
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        self.inner.WriteFile(ctx, name, data)
    }
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>> {
        self.inner.ReadFile(ctx, name)
    }
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool> {
        self.inner.FileExists(ctx, name)
    }
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn ObjectReader>> {
        self.inner.Open(ctx, name, option)
    }
    fn WalkDir(
        &self,
        _ctx: &Context,
        _option: Option<&WalkOption>,
        _callback: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        Err(self
            .error
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| anyhow!("intent walk failed")))
    }
    fn URI(&self) -> String {
        self.inner.URI()
    }
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn ObjectWriter>> {
        self.inner.Create(ctx, name, option)
    }
    fn Rename(&self, ctx: &Context, old: &str, new: &str) -> Result<()> {
        self.inner.Rename(ctx, old, new)
    }
    fn PresignFile(&self, ctx: &Context, name: &str, duration: Duration) -> Result<String> {
        self.inner.PresignFile(ctx, name, duration)
    }
    fn Close(&self) {
        self.inner.Close()
    }
    fn is_strong_consistent(&self) -> bool {
        self.inner.is_strong_consistent()
    }
}

#[test]
/// 富化 ErrLocked 时保留原始 context（如 intent walk failed）。
fn test_try_lock_remote_write_preserves_original_error_when_enriching_err_locked() {
    let blocker = LockBlocker {
        path: "test.lock.READ.remote".into(),
        meta: LockMeta {
            owner_id: "remote-op".into(),
            lock_type: "migration-read".into(),
            hint: "reader".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let walk_error = anyhow!(ErrLocked {
        blockers: vec![blocker],
        blocker_count: 1,
        ..Default::default()
    })
    .context("intent walk failed");
    let storage: StorageRef = Arc::new(WalkErrorStorage {
        inner: Arc::new(NewMemStorage()),
        error: std::sync::Mutex::new(Some(walk_error)),
    });
    let error = TryLockRemoteWrite(
        &Context::background(),
        storage,
        "test.lock",
        lock_input("local-op", "migration-write", "writer"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("intent walk failed"));
    let locked = error.downcast_ref::<ErrLocked>().unwrap();
    assert_eq!(locked.path, "test.lock.WRIT");
    assert_eq!(locked.blockers[0].path, "test.lock.READ.remote");
    assert_eq!(locked.blockers[0].meta.owner_id, "remote-op");
}

/// 断言日志字段键值存在且匹配。
fn require_field(fields: &[(String, String)], key: &str, value: &str) {
    assert_eq!(
        fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str()),
        Some(value),
        "missing or mismatched {key}"
    );
}
