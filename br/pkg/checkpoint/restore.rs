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

//! 快照恢复（snapshot restore）检查点的类型与 Runner 入口。
//!
//! 对应 Go `br/pkg/checkpoint/restore.go`：以 table ID 为分组键，记录已完成的
//! range-key 或文件名；并通过 `SnapshotMetaManager` 启动刷盘 Runner。
//! 本文件不实现存储，只定义数据契约与追加约束（rangeKey 与 name 互斥）。

// 依赖 SnapshotMetaManager 启动 Runner；Uuid 用于任务级幂等标识。
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::checkpoint::{CheckpointMessage, CheckpointRunner, RangeGroup};
use crate::manager::{DefaultTickDurationConfig, SnapshotMetaManager};
use crate::stubs::{ClusterConfig, Context, Error, Result};

/// 快照恢复检查点分组键：下游表 ID。对应 Go `RestoreKeyType = int64`。
pub type RestoreKeyType = i64;

/// 单条已完成单元：要么是 SST range-key，要么是文件名（二者互斥）。
/// JSON 空字段省略，对齐 Go omitempty 习惯。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreValueType {
    #[serde(
        rename = "range-key",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub RangeKey: String,
    #[serde(rename = "name", default, skip_serializing_if = "String::is_empty")]
    pub Name: String,
}

/// Append 入参的便捷载体：携带 tableID 与 rangeKey/name 之一。
pub struct CheckpointItem {
    /// 下游表 ID，作为 Runner 的 GroupKey
    pub tableID: RestoreKeyType,
    /// 非空时表示按 SST range 完成
    pub rangeKey: String,
    /// 非空时表示按文件名完成；与 rangeKey 互斥
    pub name: String,
}

/// 构造“按 range-key 完成”的检查点项。
pub fn NewCheckpointRangeKeyItem(tableID: RestoreKeyType, rangeKey: String) -> CheckpointItem {
    CheckpointItem {
        tableID,
        rangeKey,
        name: String::new(),
    }
}

/// 构造“按文件名完成”的检查点项。
pub fn NewCheckpointFileItem(tableID: RestoreKeyType, fileName: String) -> CheckpointItem {
    CheckpointItem {
        tableID,
        rangeKey: String::new(),
        name: fileName,
    }
}

/// 快照恢复无需额外压缩，直接 JSON 序列化整个 `RangeGroup`。
fn valueMarshalerForRestore(
    group: &RangeGroup<RestoreKeyType, RestoreValueType>,
) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(group)?)
}

/// 测试入口：注入 flush/checksum/retry 周期，缩短等待。
/// 对应 Go `StartCheckpointRestoreRunnerForTest`。
pub fn StartCheckpointRestoreRunnerForTest(
    ctx: &Context,
    tick: Duration,
    retryDuration: Duration,
    manager: &dyn SnapshotMetaManager,
) -> Result<CheckpointRunner<RestoreKeyType, RestoreValueType>> {
    let mut cfg = DefaultTickDurationConfig();
    cfg.tickDurationForChecksum = tick;
    cfg.tickDurationForFlush = tick;
    cfg.retryDuration = retryDuration;
    manager.StartCheckpointRunner(ctx, cfg, valueMarshalerForRestore)
}

/// 正式启动快照恢复检查点 Runner，使用默认 tick。
/// 对应 Go `StartCheckpointRunnerForRestore`。
pub fn StartCheckpointRunnerForRestore(
    ctx: &Context,
    manager: &dyn SnapshotMetaManager,
) -> Result<CheckpointRunner<RestoreKeyType, RestoreValueType>> {
    manager.StartCheckpointRunner(ctx, DefaultTickDurationConfig(), valueMarshalerForRestore)
}

/// 向 Runner 追加一条完成记录。
///
/// 约束：`rangeKey` 与 `name` 必须恰好一个非空，否则返回与 Go 相同文案的错误。
/// 数据流：`CheckpointItem` → 单元素 `CheckpointMessage` → `Runner.Append`。
pub fn AppendRangesForRestore(
    ctx: &Context,
    r: &CheckpointRunner<RestoreKeyType, RestoreValueType>,
    c: &CheckpointItem,
) -> Result<()> {
    let mut group = RestoreValueType::default();
    if !c.rangeKey.is_empty() {
        group.RangeKey = c.rangeKey.clone();
    } else if !c.name.is_empty() {
        group.Name = c.name.clone();
    } else {
        return Err(Error::new(
            "either rangekey or name should be used in checkpoint append",
        ));
    }
    r.Append(
        ctx,
        CheckpointMessage {
            GroupKey: c.tableID,
            Group: vec![group],
        },
    )
}

/// 预分配 ID 区间及校验哈希，防止跨任务错用。对应 Go `PreallocIDs`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PreallocIDs {
    /// 预分配起点
    pub Start: i64,
    /// 可复用边界（小于此可重用）
    pub ReusableBorder: i64,
    /// 预分配终点（不含或按 Go 语义）
    pub End: i64,
    /// 区间完整性校验
    pub Hash: [u8; 32],
}

/// 快照恢复任务级元数据：上下游 TS、调度器配置、预分配 ID 与任务 UUID。
/// JSON 字段名与 Go `CheckpointMetadataForSnapshotRestore` 对齐，保证跨语言可读。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckpointMetadataForSnapshotRestore {
    #[serde(rename = "upstream-cluster-id", default)]
    pub UpstreamClusterID: u64,
    #[serde(rename = "restore-start-ts", default)]
    pub RestoreStartTS: u64,
    #[serde(rename = "restored-ts", default)]
    pub RestoredTS: u64,
    #[serde(rename = "log-restored-ts", default)]
    pub LogRestoredTS: u64,
    #[serde(rename = "schedulers-config", default)]
    pub SchedulersConfig: Option<ClusterConfig>,
    #[serde(rename = "hash", default)]
    pub Hash: Vec<u8>,
    #[serde(rename = "prealloc-ids", default)]
    pub PreallocIDs: Option<PreallocIDs>,
    /// 按 Go `google/uuid.UUID.MarshalText` 的标准连字符字符串形式序列化。
    #[serde(rename = "restore-uuid", default, with = "uuid_serde")]
    pub RestoreUUID: Uuid,
}

/// Uuid ↔ 标准文本的 serde 适配，对齐 Go `encoding/json` 对 TextMarshaler 的处理。
mod uuid_serde {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};
    use uuid::Uuid;

    pub fn serialize<S>(u: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&u.hyphenated().to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Uuid::parse_str(&text).map_err(D::Error::custom)
    }
}
