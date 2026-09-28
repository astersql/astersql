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

//! 备份场景的 checkpoint 适配层，对齐 Go `br/pkg/checkpoint/backup.go`。
//!
//! 将通用 `CheckpointRunner` 特化为备份键值类型，固定目录布局
//!（`checkpoints/backup/{data,checksum,checkpoint.meta,lock}`），
//! 并提供 Append/Walk/元数据读写/清理入口。
//! 真正的刷盘循环与加锁在 `checkpoint`/`external_storage`；本文件只做路径与类型绑定。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::checkpoint::{
    CheckpointDir, CheckpointMessage, CheckpointRunner, ChecksumItem, RangeGroup, RangeType,
    loadCheckpointChecksum, loadCheckpointMeta, newCheckpointRunner, removeCheckpointData,
    saveCheckpointMetadata, walkCheckpointFile,
};
use crate::external_storage::newExternalCheckpointStorage;
use crate::stubs::{CipherInfo, Context, File, GlobalTimer, Result, Storage};

/// 备份 checkpoint 的分组键；当前 Append 使用空串，与 Go 一致。
pub type BackupKeyType = String;
/// 备份 value 即一个已完成的 key range + 产出文件列表。
pub type BackupValueType = RangeType;

/// 备份 checkpoint 根目录。
pub const CheckpointBackupDir: &str = "checkpoints/backup";
/// 分片 data 文件目录（`.cpt`）。
pub const CheckpointDataDirForBackup: &str = "checkpoints/backup/data";
/// checksum 分片目录。
pub const CheckpointChecksumDirForBackup: &str = "checkpoints/backup/checksum";
/// 备份 checkpoint 元数据文件路径。
pub const CheckpointMetaPathForBackup: &str = "checkpoints/backup/checkpoint.meta";
/// 互斥锁文件路径，防止多 BR 并发写同一备份集。
pub const CheckpointLockPathForBackup: &str = "checkpoints/backup/checkpoint.lock";

/// 组装备份场景的 flushPath，供 externalCheckpointStorage 使用。
fn flushPathForBackup() -> crate::checkpoint::flushPath {
    crate::checkpoint::flushPath {
        CheckpointDataDir: CheckpointDataDirForBackup.to_string(),
        CheckpointChecksumDir: CheckpointChecksumDirForBackup.to_string(),
        CheckpointLockPath: CheckpointLockPathForBackup.to_string(),
    }
}

/// 将 RangeGroup JSON 序列化为 data 分片载荷（对齐 Go valueMarshaler）。
fn valueMarshalerForBackup(group: &RangeGroup<BackupKeyType, BackupValueType>) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(group)?)
}

/// 测试入口：用自定义 tick/timer 启动备份 Runner，便于加速刷盘与锁续期。
pub fn StartCheckpointBackupRunnerForTest(
    ctx: &Context,
    storage: Arc<dyn Storage>,
    cipher: Option<CipherInfo>,
    tick: Duration,
    timer: Arc<dyn GlobalTimer>,
) -> Result<CheckpointRunner<BackupKeyType, BackupValueType>> {
    let checkpointStorage =
        newExternalCheckpointStorage(ctx, storage, Some(timer), flushPathForBackup())?;
    let runner = newCheckpointRunner(checkpointStorage, cipher, valueMarshalerForBackup);
    // 四个 tick 使用同一间隔，简化测试时序。
    runner.startCheckpointMainLoop(ctx.clone(), tick, tick, tick, tick);
    Ok(runner)
}

/// 生产入口：以默认 flush/checksum/lock/retry 周期启动备份 Runner。
pub fn StartCheckpointRunnerForBackup(
    ctx: &Context,
    storage: Arc<dyn Storage>,
    cipher: Option<CipherInfo>,
    timer: Arc<dyn GlobalTimer>,
) -> Result<CheckpointRunner<BackupKeyType, BackupValueType>> {
    let checkpointStorage =
        newExternalCheckpointStorage(ctx, storage, Some(timer), flushPathForBackup())?;
    let runner = newCheckpointRunner(checkpointStorage, cipher, valueMarshalerForBackup);
    runner.startCheckpointMainLoop(
        ctx.clone(),
        crate::checkpoint::defaultTickDurationForFlush,
        crate::checkpoint::defaultTickDurationForChecksum,
        crate::checkpoint::defaultTickDurationForLock,
        crate::checkpoint::defaultRetryDuration,
    );
    Ok(runner)
}

/// 追加一个已完成备份 range：包装为单元素 Group，GroupKey 置空。
pub fn AppendForBackup(
    ctx: &Context,
    r: &CheckpointRunner<BackupKeyType, BackupValueType>,
    startKey: &[u8],
    endKey: &[u8],
    files: Vec<File>,
) -> Result<()> {
    r.Append(
        ctx,
        CheckpointMessage {
            GroupKey: String::new(),
            Group: vec![BackupValueType {
                StartKey: startKey.to_vec(),
                EndKey: endKey.to_vec(),
                Files: files,
            }],
        },
    )
}

/// 遍历备份 data 目录中的 checkpoint 分片，回调每个 (key, value)。
/// 返回值 Duration 对齐 Go：表示加载/遍历耗时（由底层 walk 计算）。
pub fn WalkCheckpointFileForBackup<F>(
    ctx: &Context,
    s: &dyn Storage,
    cipher: Option<&CipherInfo>,
    fn_: F,
) -> Result<Duration>
where
    F: FnMut(BackupKeyType, BackupValueType) -> Result<()>,
{
    walkCheckpointFile(ctx, s, cipher, CheckpointDataDirForBackup, fn_)
}

/// 备份 checkpoint 元数据：GC 服务 ID、配置哈希、BackupTS；
/// Checksum/DataMap 运行时填充，不序列化进 meta 文件。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointMetadataForBackup {
    #[serde(rename = "gc-service-id", default)]
    pub GCServiceId: String,
    #[serde(rename = "config-hash", default)]
    pub ConfigHash: Vec<u8>,
    #[serde(rename = "backup-ts", default)]
    pub BackupTS: u64,
    /// 从 checksum 目录加载的表级校验汇总（跳过 serde）。
    #[serde(skip)]
    pub CheckpointChecksum: HashMap<i64, ChecksumItem>,
    /// 是否需要加载 data map 的提示标志（运行时，不落盘）。
    #[serde(skip)]
    pub LoadCheckpointDataMap: bool,
}

/// 读取 meta 文件并合并 checksum 目录内容。
pub fn LoadCheckpointMetadata(
    ctx: &Context,
    s: &dyn Storage,
) -> Result<CheckpointMetadataForBackup> {
    let mut m = CheckpointMetadataForBackup::default();
    loadCheckpointMeta(ctx, s, CheckpointMetaPathForBackup, &mut m)?;
    let (checksum, _) = loadCheckpointChecksum(ctx, s, CheckpointChecksumDirForBackup)?;
    m.CheckpointChecksum = checksum;
    Ok(m)
}

/// 将元数据写入固定 meta 路径；不含 runtime-only 字段。
pub fn SaveCheckpointMetadata(
    ctx: &Context,
    s: &dyn Storage,
    meta: &CheckpointMetadataForBackup,
) -> Result<()> {
    saveCheckpointMetadata(ctx, s, meta, CheckpointMetaPathForBackup)
}

/// 删除整个备份 checkpoint 目录树（含 data/checksum/meta/lock）。
pub fn RemoveCheckpointDataForBackup(ctx: &Context, s: &dyn Storage) -> Result<()> {
    let _ = CheckpointDir;
    removeCheckpointData(ctx, s, CheckpointBackupDir)
}
