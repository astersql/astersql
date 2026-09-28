// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 外部存储上的 checkpoint 落盘与互斥锁，对齐 Go `external_storage.go`。
//!
//! `externalCheckpointStorage` 实现 `checkpointStorage`：将 data/checksum
//! 写成 UUID 命名的 `.cpt` 文件，并用 `checkpoint.lock` + TSO 做多 BR 互斥。
//! 提供 restore 场景的目录格式串与按 taskName 解析路径的辅助函数。
//! 仅在构造时传入 timer 才会 `initialLock`；无 timer 时跳过锁（测试常用）。
//! 锁文案与 Go 保持一致，便于运维按错误提示清理残留 lock 文件。
//! flush 使用 UUID 文件名，避免并发 Runner 互相覆盖同一分片。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::checkpoint::{checkpointStorage, flushPath, lockTimeToLive};
use crate::stubs::{ComposeTS, Context, Error, GlobalTimer, Result, Storage, WithRetry};

// Mirrors Go's `failed-after-checkpoint-updates-lock` injection point. Keeping
// the hook independent from flush failpoints preserves the Go failure surface.
static FAILED_AFTER_CHECKPOINT_UPDATES_LOCK: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn set_failed_after_checkpoint_updates_lock_for_test(enabled: bool) {
    FAILED_AFTER_CHECKPOINT_UPDATES_LOCK.store(enabled, Ordering::SeqCst);
}

/// restore checkpoint 根目录格式（`%s` = taskName）。
pub const CheckpointRestoreDirFormat: &str = "checkpoints/restore-%s";
/// restore data 目录格式。
pub const CheckpointDataDirForRestoreFormat: &str = "checkpoints/restore-%s/data";
/// restore checksum 目录格式。
pub const CheckpointChecksumDirForRestoreFormat: &str = "checkpoints/restore-%s/checksum";
/// restore meta 路径格式。
pub const CheckpointMetaPathForRestoreFormat: &str = "checkpoints/restore-%s/checkpoint.meta";
/// restore 进度 meta 路径格式。
pub const CheckpointProgressPathForRestoreFormat: &str = "checkpoints/restore-%s/progress.meta";
/// restore ingest index meta 路径格式。
pub const CheckpointIngestIndexPathForRestoreFormat: &str =
    "checkpoints/restore-%s/ingest_index.meta";

/// 按任务名构造 restore 的 flushPath；LockPath 置空（restore 锁策略与 backup 不同）。
pub fn flushPathForRestore(taskName: &str) -> flushPath {
    flushPath {
        CheckpointDataDir: getCheckpointDataDirByName(taskName),
        CheckpointChecksumDir: getCheckpointChecksumDirByName(taskName),
        CheckpointLockPath: String::new(),
    }
}

/// `checkpoints/restore-{task}/checkpoint.meta`
pub fn getCheckpointMetaPathByName(taskName: &str) -> String {
    format!("checkpoints/restore-{taskName}/checkpoint.meta")
}

/// `checkpoints/restore-{task}/data`
pub fn getCheckpointDataDirByName(taskName: &str) -> String {
    format!("checkpoints/restore-{taskName}/data")
}

/// `checkpoints/restore-{task}/checksum`
pub fn getCheckpointChecksumDirByName(taskName: &str) -> String {
    format!("checkpoints/restore-{taskName}/checksum")
}

/// `checkpoints/restore-{task}/progress.meta`
pub fn getCheckpointProgressPathByName(taskName: &str) -> String {
    format!("checkpoints/restore-{taskName}/progress.meta")
}

/// `checkpoints/restore-{task}/ingest_index.meta`
pub fn getCheckpointIngestIndexPathByName(taskName: &str) -> String {
    format!("checkpoints/restore-{taskName}/ingest_index.meta")
}

/// 基于 `Storage` 的 checkpoint 存储：持有路径、底层存储、本进程 lockId 与可选 timer。
pub struct externalCheckpointStorage {
    /// data/checksum/lock 三类路径集合。
    pub flushPath: flushPath,
    /// 底层对象存储（本地/S3 等均经 Storage trait）。
    pub storage: Arc<dyn Storage>,
    /// 本实例的锁 ID（由 TSO Compose 得到）。
    lockId: Mutex<u64>,
    /// 用于取 TS；为 None 时不做 initialLock。
    timer: Option<Arc<dyn GlobalTimer>>,
}

/// 构造存储；若提供 timer 则立即抢占/校验锁文件。
pub fn newExternalCheckpointStorage(
    ctx: &Context,
    s: Arc<dyn Storage>,
    timer: Option<Arc<dyn GlobalTimer>>,
    flushPath: flushPath,
) -> Result<Arc<externalCheckpointStorage>> {
    let checkpointStorage = Arc::new(externalCheckpointStorage {
        flushPath,
        storage: s,
        lockId: Mutex::new(0),
        timer: timer.clone(),
    });
    if timer.is_some() {
        checkpointStorage.initialLock(ctx)?;
    }
    Ok(checkpointStorage)
}

impl externalCheckpointStorage {
    /// 经 WithRetry 从 timer 取物理/逻辑 TS；对齐 Go aggressive PD 的 32 次尝试。
    fn getTS(&self, ctx: &Context) -> Result<(i64, i64)> {
        let timer = self
            .timer
            .as_ref()
            .ok_or_else(|| Error::new("timer is nil"))?;
        let mut p = 0i64;
        let mut l = 0i64;
        let mut retry = 0;
        WithRetry(
            ctx,
            || match timer.GetTS(ctx) {
                Ok((pp, ll)) => {
                    p = pp;
                    l = ll;
                    Ok(())
                }
                Err(err) => {
                    retry += 1;
                    let _ = retry;
                    Err(err)
                }
            },
            32,
        )?;
        Ok((p, l))
    }

    /// 将当前 lockId 与过期时间写入锁文件（ExpireAt = 物理时间 + TTL）。
    fn flushLock(&self, ctx: &Context, p: i64) -> Result<()> {
        let lock = CheckpointLock {
            LockId: *self.lockId.lock().unwrap(),
            ExpireAt: p + lockTimeToLive.as_millis() as i64,
        };
        let data = serde_json::to_vec(&lock)?;
        self.storage
            .WriteFile(ctx, &self.flushPath.CheckpointLockPath, &data)
    }

    /// 校验已有锁：过期且对方 LockId 更大 → 冲突失败；未过期且 ID 不同 → 提示等待/手动删。
    fn checkLockFile(&self, ctx: &Context, now: i64) -> Result<()> {
        let data = self
            .storage
            .ReadFile(ctx, &self.flushPath.CheckpointLockPath)?;
        let lock: CheckpointLock = serde_json::from_slice(&data)?;
        let my_id = *self.lockId.lock().unwrap();
        if lock.ExpireAt <= now {
            // 锁已过期但仍发现“更晚启动却更早写锁”的异常序，拒绝覆盖。
            if lock.LockId > my_id {
                return Err(Error::new(format!(
                    "There are another BR({}) running after but setting lock before this one({}). \
                     Please check whether the BR is running. If not, you can retry.",
                    lock.LockId, my_id
                )));
            }
        } else if lock.LockId != my_id {
            // 有效锁属于其他实例：返回剩余秒数与可删路径提示（文案对齐 Go）。
            let uri = self.storage.URI().trim_end_matches('/').to_string();
            return Err(Error::new(format!(
                "The existing lock will expire in {} seconds. \
                 There may be another BR({}) running. If not, you can wait for the lock to expire, \
                 or delete the file `{}{}` manually.",
                (lock.ExpireAt - now) / 1000,
                lock.LockId,
                uri,
                self.flushPath.CheckpointLockPath
            )));
        }
        Ok(())
    }
}

impl checkpointStorage for externalCheckpointStorage {
    /// 将 data 载荷写入 `{dataDir}/{uuid}.cpt`。
    fn flushCheckpointData(&self, ctx: &Context, data: &[u8]) -> Result<()> {
        let fname = format!(
            "{}/{}.cpt",
            self.flushPath.CheckpointDataDir,
            Uuid::new_v4().simple()
        );
        self.storage.WriteFile(ctx, &fname, data)
    }

    /// 将 checksum 载荷写入 `{checksumDir}/{uuid}.cpt`。
    fn flushCheckpointChecksum(&self, ctx: &Context, data: &[u8]) -> Result<()> {
        let fname = format!(
            "{}/{}.cpt",
            self.flushPath.CheckpointChecksumDir,
            Uuid::new_v4().simple()
        );
        self.storage.WriteFile(ctx, &fname, data)
    }

    /// 初始化锁：用 TSO 生成 lockId，校验已有锁后写入，再 sleep 3s 复检防竞态覆盖。
    fn initialLock(&self, ctx: &Context) -> Result<()> {
        let (p, l) = self.getTS(ctx)?;
        *self.lockId.lock().unwrap() = ComposeTS(p, l);
        let exist = self
            .storage
            .FileExists(ctx, &self.flushPath.CheckpointLockPath)?;
        if exist {
            self.checkLockFile(ctx, p)?;
        }
        self.flushLock(ctx, p)?;
        // wait for 3 seconds to check whether the lock file is overwritten by another BR
        // 与 Go 相同的宽限窗口，降低双写竞态漏检概率。
        thread::sleep(Duration::from_secs(3));
        self.checkLockFile(ctx, p)
    }

    /// 续期：先校验仍持有锁，再刷新 ExpireAt。
    fn updateLock(&self, ctx: &Context) -> Result<()> {
        let (p, _) = self.getTS(ctx)?;
        self.checkLockFile(ctx, p)?;
        self.flushLock(ctx, p)?;
        if FAILED_AFTER_CHECKPOINT_UPDATES_LOCK.load(Ordering::SeqCst) {
            return Err(Error::new(
                "failpoint: failed after checkpoint updates lock",
            ));
        }
        Ok(())
    }

    /// 关闭钩子：当前无额外资源释放，保留以对齐 Go Close。
    fn close(&self) {}
}

/// 锁文件 JSON：LockId 标识持有者，ExpireAt 为物理毫秒截止时间。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointLock {
    /// 持有者身份，通常为 ComposeTS(physical, logical)。
    #[serde(rename = "lock-id")]
    pub LockId: u64,
    /// 过期物理时间（毫秒）；与 getTS 的 physical 同量纲比较。
    #[serde(rename = "expire-at")]
    pub ExpireAt: i64,
}
