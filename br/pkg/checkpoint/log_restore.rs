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

//! 日志恢复（log restore）检查点的类型、序列化与任务进度查询。
//!
//! 对应 Go `br/pkg/checkpoint/log_restore.go`：定义已恢复文件范围的 KV 形态、
//! 压缩后的落盘格式、检查点 Runner 启动入口，以及 snapshot+log 联合恢复时的
//! 进度元数据。本文件不实现存储后端，只组装 `LogMetaManager` / `SnapshotMetaManager`
//! 与 `CheckpointRunner` 之间的数据契约。

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::checkpoint::{CheckpointMessage, CheckpointRunner, RangeGroup};
use crate::manager::{DefaultTickDurationConfig, LogMetaManager, SnapshotMetaManager};
use crate::storage::tableCheckpointStorage;
use crate::stubs::{CIStr, Context, Result, Session, TiFlashReplicaInfo};

/// 检查点分组键：与 Go `LogRestoreKeyType = string` 对齐，通常标识一批日志文件。
pub type LogRestoreKeyType = String;

/// 单条已完成恢复的文件定位：下游表 ID + 元数据内 group/file 下标。
/// 对应 Go `LogRestoreValueType`；Append 时以稀疏条目写入 Runner。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRestoreValueType {
    /// 下游表 ID（restore 后的 table id）
    pub TableID: i64,
    /// 元数据中 group 下标
    pub Goff: i64,
    /// group 内 file 下标
    pub Foff: i64,
}

/// 落盘用的压缩形态：同一 Goff 下按 TableID 聚合多个 Foff。
/// 对应 Go `LogRestoreValueMarshaled`，JSON 字段名保持 `goff`/`foffs`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRestoreValueMarshaled {
    #[serde(rename = "goff")]
    pub Goff: i64,
    #[serde(rename = "foffs")]
    pub Foffs: HashMap<i64, Vec<i64>>,
}

/// 将稀疏 `LogRestoreValueType` 列表折叠为按 Goff→TableID→Foffs 的紧凑 JSON。
///
/// 与 Go `valueMarshalerForLogRestore` 同语义：减少检查点文件体积。
/// 输入是 `RangeGroup`；输出仍包一层 `RangeGroup`，但 `Group` 元素变为
/// `LogRestoreValueMarshaled`。序列化失败时返回 `Result` 错误（对齐 Go 的 json.Marshal）。
pub fn valueMarshalerForLogRestore(
    group: &RangeGroup<LogRestoreKeyType, LogRestoreValueType>,
) -> Result<Vec<u8>> {
    // goff -> table-id -> []foff；先聚合再序列化，避免逐条落盘膨胀
    let mut gMap: HashMap<i64, HashMap<i64, Vec<i64>>> = HashMap::new();
    for g in &group.Group {
        let fMap = gMap.entry(g.Goff).or_default();
        fMap.entry(g.TableID).or_default().push(g.Foff);
    }
    let mut logValues = Vec::with_capacity(gMap.len());
    for (goff, foffs) in gMap {
        logValues.push(LogRestoreValueMarshaled {
            Goff: goff,
            Foffs: foffs,
        });
    }
    Ok(serde_json::to_vec(&RangeGroup {
        GroupKey: group.GroupKey.clone(),
        Group: logValues,
    })?)
}

/// 构造表级检查点存储；session 所有权与关闭时机由后续 Runner 约定（见 Go 注释）。
pub fn newTableCheckpointStorage(
    se: Box<dyn Session>,
    checkpointDBName: String,
) -> tableCheckpointStorage {
    tableCheckpointStorage::new(se, checkpointDBName)
}

/// 测试入口：用注入的 tick 覆盖 checksum/flush 周期，便于缩短等待。
/// 对应 Go `StartCheckpointLogRestoreRunnerForTest`；生产路径请用下方正式 API。
pub fn StartCheckpointLogRestoreRunnerForTest(
    ctx: &Context,
    tick: Duration,
    manager: &dyn LogMetaManager,
) -> Result<CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>> {
    let mut cfg = DefaultTickDurationConfig();
    cfg.tickDurationForChecksum = tick;
    cfg.tickDurationForFlush = tick;
    manager.StartCheckpointRunner(ctx, cfg, valueMarshalerForLogRestore)
}

/// 正式启动日志恢复检查点 Runner，使用默认 tick，并绑定 `valueMarshalerForLogRestore`。
/// 对应 Go `StartCheckpointRunnerForLogRestore`。
pub fn StartCheckpointRunnerForLogRestore(
    ctx: &Context,
    manager: &dyn LogMetaManager,
) -> Result<CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>> {
    manager.StartCheckpointRunner(
        ctx,
        DefaultTickDurationConfig(),
        valueMarshalerForLogRestore,
    )
}

/// 向 Runner 追加一条已完成的文件范围；内部包装为单元素 `CheckpointMessage`。
/// 对应 Go `AppendRangeForLogRestore`。
pub fn AppendRangeForLogRestore(
    ctx: &Context,
    r: &CheckpointRunner<LogRestoreKeyType, LogRestoreValueType>,
    groupKey: LogRestoreKeyType,
    tableID: i64,
    goff: i64,
    foff: i64,
) -> Result<()> {
    r.Append(
        ctx,
        CheckpointMessage {
            GroupKey: groupKey,
            Group: vec![LogRestoreValueType {
                TableID: tableID,
                Goff: goff,
                Foff: foff,
            }],
        },
    )
}

/// 日志恢复任务级元数据：上下游集群、TS 边界、GC 比例与 TiFlash 记录。
/// JSON 字段名与 Go `CheckpointMetadataForLogRestore` 保持一致，便于跨语言读写同一对象存储。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointMetadataForLogRestore {
    #[serde(rename = "upstream-cluster-id", default)]
    pub UpstreamClusterID: u64,
    #[serde(rename = "restore-start-ts", default)]
    pub RestoreStartTS: u64,
    #[serde(rename = "restored-ts", default)]
    pub RestoredTS: u64,
    #[serde(rename = "start-ts", default)]
    pub StartTS: u64,
    #[serde(rename = "rewrite-ts", default)]
    pub RewriteTS: u64,
    #[serde(rename = "gc-ratio", default)]
    pub GcRatio: String,
    /// 快照恢复阶段记录的 TiFlash 副本信息；空 map 时省略序列化（对齐 omitempty）
    #[serde(
        rename = "tiflash-recorder",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub TiFlashItems: HashMap<i64, TiFlashReplicaInfo>,
}

/// snapshot + log 联合恢复的进度相位。
///
/// 约束（与 Go `RestoreProgress` 注释一致）：
/// - `InSnapshotRestore`：id-map 尚未持久化，可从快照阶段重试；
/// - `InLogRestoreAndIdMapPersisted`：id-map 已写入外部存储，且可能已有 rename 等
///   meta-kv 落库；此时若再跑快照恢复会导致重复表，必须跳过快照阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum RestoreProgress {
    #[default]
    InSnapshotRestore = 0,
    /// 仅当 id-map 已持久化后才进入此状态
    InLogRestoreAndIdMapPersisted = 1,
}

impl Serialize for RestoreProgress {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i32(*self as i32)
    }
}

impl<'de> Deserialize<'de> for RestoreProgress {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match i32::deserialize(deserializer)? {
            0 => Ok(Self::InSnapshotRestore),
            1 => Ok(Self::InLogRestoreAndIdMapPersisted),
            value => Err(de::Error::custom(format_args!(
                "invalid restore progress {value}"
            ))),
        }
    }
}

/// 进度文件载荷，仅承载 `RestoreProgress`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointProgress {
    #[serde(rename = "progress")]
    pub Progress: RestoreProgress,
}

/// 绑定到特定集群的最近一次日志恢复任务摘要。
/// 对应 Go `TaskInfoForLogRestore`：合并 log/snapshot 两侧元数据存在性与进度。
#[derive(Clone, Debug, Default)]
pub struct TaskInfoForLogRestore {
    pub Metadata: Option<CheckpointMetadataForLogRestore>,
    pub HasSnapshotMetadata: bool,
    pub Progress: RestoreProgress,
}

impl TaskInfoForLogRestore {
    /// id-map 是否已保存：进度进入 `InLogRestoreAndIdMapPersisted` 即为 true。
    pub fn IdMapSaved(&self) -> bool {
        self.Progress == RestoreProgress::InLogRestoreAndIdMapPersisted
    }
}

/// 汇总 log/snapshot manager 上的进度与元数据，供恢复入口决定从哪一阶段续跑。
///
/// 数据流：先读 log 进度 → 再读 log 元数据 →（可选）探测 snapshot 元数据是否存在。
/// `snapshotManager` 为 `None` 时跳过快照侧探测，与 Go 中 `snapshotManager != nil` 分支对齐。
pub fn GetCheckpointTaskInfo(
    ctx: &Context,
    snapshotManager: Option<&dyn SnapshotMetaManager>,
    logManager: &dyn LogMetaManager,
) -> Result<TaskInfoForLogRestore> {
    let mut metadata = None;
    let mut progress = RestoreProgress::InSnapshotRestore;
    let mut hasSnapshotMetadata = false;

    if logManager.ExistsCheckpointProgress(ctx)? {
        let checkpointProgress = logManager.LoadCheckpointProgress(ctx)?;
        progress = checkpointProgress.Progress;
    }
    if logManager.ExistsCheckpointMetadata(ctx)? {
        metadata = Some(logManager.LoadCheckpointMetadata(ctx)?);
    }
    if let Some(sm) = snapshotManager {
        hasSnapshotMetadata = sm.ExistsCheckpointMetadata(ctx)?;
    }

    Ok(TaskInfoForLogRestore {
        Metadata: metadata,
        HasSnapshotMetadata: hasSnapshotMetadata,
        Progress: progress,
    })
}

/// 摄入索引修复用的 ADD INDEX SQL 记录；运行时字段不序列化。
/// 对应 Go `CheckpointIngestIndexRepairSQL`（`OldIndexIDFound`/`IndexRepaired` 带 `json:"-"`）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointIngestIndexRepairSQL {
    #[serde(rename = "index-id")]
    pub IndexID: i64,
    #[serde(rename = "schema-name")]
    pub SchemaName: CIStr,
    #[serde(rename = "table-name")]
    pub TableName: CIStr,
    #[serde(rename = "index-name")]
    pub IndexName: String,
    #[serde(rename = "add-sql")]
    pub AddSQL: String,
    #[serde(rename = "add-args")]
    pub AddArgs: Vec<serde_json::Value>,
    /// 仅进程内状态：是否已找到旧 index id
    #[serde(skip)]
    pub OldIndexIDFound: bool,
    /// 仅进程内状态：是否已完成修复
    #[serde(skip)]
    pub IndexRepaired: bool,
}

/// 外键更新 SQL 记录；与索引修复结构对称，运行时标志不落盘。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointForeignKeyUpdateSQL {
    #[serde(rename = "fk-id")]
    pub FKID: i64,
    #[serde(rename = "schema-name")]
    pub SchemaName: String,
    #[serde(rename = "table-name")]
    pub TableName: String,
    #[serde(rename = "fk-name")]
    pub FKName: String,
    #[serde(rename = "add-sql")]
    pub AddSQL: String,
    #[serde(rename = "add-args")]
    pub AddArgs: Vec<serde_json::Value>,
    #[serde(skip)]
    pub OldForeignKeyFound: bool,
    #[serde(skip)]
    pub ForeignKeyUpdated: bool,
}

/// 索引修复与外键更新 SQL 的打包容器，供检查点批量持久化。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointIngestIndexRepairSQLs {
    pub SQLs: Vec<CheckpointIngestIndexRepairSQL>,
    pub FKSQLs: Vec<CheckpointForeignKeyUpdateSQL>,
}
