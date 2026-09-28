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

// DDL 重组（reorg / backfill）相关元数据模型。
//
// 重组指在线 DDL（如加索引、改列）期间对存量数据的回填与增量合并。
// 本模块保存回填状态机、阶段、并发/批大小等可在线调整参数，以及单个 backfill job 进度。

// 时区、session 变量与同包模型等待后续模块接线。
//

use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

mod go_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut encoded = String::with_capacity(value.len().div_ceil(3) * 4);
        for chunk in value.chunks(3) {
            let bits = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            encoded.push(ALPHABET[((bits >> 18) & 0x3f) as usize] as char);
            encoded.push(ALPHABET[((bits >> 12) & 0x3f) as usize] as char);
            encoded.push(if chunk.len() > 1 {
                ALPHABET[((bits >> 6) & 0x3f) as usize] as char
            } else {
                '='
            });
            encoded.push(if chunk.len() > 2 {
                ALPHABET[(bits & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(encoded) = Option::<String>::deserialize(deserializer)? else {
            return Ok(Vec::new());
        };
        if encoded.len() % 4 != 0 {
            return Err(D::Error::custom("invalid base64 length"));
        }
        let mut decoded = Vec::with_capacity(encoded.len() / 4 * 3);
        for chunk in encoded.as_bytes().chunks_exact(4) {
            let mut values = [0_u8; 4];
            for (index, byte) in chunk.iter().copied().enumerate() {
                values[index] = match byte {
                    b'A'..=b'Z' => byte - b'A',
                    b'a'..=b'z' => byte - b'a' + 26,
                    b'0'..=b'9' => byte - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    b'=' if index >= 2 => 0,
                    _ => return Err(D::Error::custom("invalid base64 byte")),
                };
            }
            let bits = (u32::from(values[0]) << 18)
                | (u32::from(values[1]) << 12)
                | (u32::from(values[2]) << 6)
                | u32::from(values[3]);
            decoded.push((bits >> 16) as u8);
            if chunk[2] != b'=' {
                decoded.push((bits >> 8) as u8);
            }
            if chunk[3] != b'=' {
                decoded.push(bits as u8);
            }
        }
        Ok(decoded)
    }
}

// BackfillState 对应 backfill-merge 状态机；判别值必须与已持久化元数据一致。
/// backfill-merge 状态机；判别值必须与已持久化元数据一致。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize_repr, Deserialize_repr)]
pub enum BackfillState {
    #[default]
    /// 不适用回填合并流程。
    BackfillStateInapplicable = 0,
    /// 回填正在运行。
    BackfillStateRunning = 1,
    /// 回填完成，等待合并增量。
    BackfillStateReadyToMerge = 2,
    /// 正在合并增量索引/数据。
    BackfillStateMerging = 3,
}

impl BackfillState {
    // String 对应 Go fmt.Stringer，为日志保留完整的人类可读状态。
    /// 返回日志用人类可读状态。
    pub fn String(self) -> &'static str {
        match self {
            Self::BackfillStateRunning => "backfill state running",
            Self::BackfillStateReadyToMerge => "backfill state ready to merge",
            Self::BackfillStateMerging => "backfill state merging",
            Self::BackfillStateInapplicable => "backfill state inapplicable",
        }
    }
}

// ReorgStage 是可持久化阶段，防止修改列流程重复执行已经完成的工作。
/// 可持久化的改列重组阶段，防止重复执行已完成步骤。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize_repr, Deserialize_repr)]
pub enum ReorgStage {
    #[default]
    /// 尚未进入改列重组。
    ReorgStageNone = 0,
    /// 正在更新列数据。
    ReorgStageModifyColumnUpdateColumn = 1,
    /// 正在重建相关索引。
    ReorgStageModifyColumnRecreateIndex = 2,
    /// 改列重组已完成。
    ReorgStageModifyColumnCompleted = 3,
}

// DDLReorgMeta 保存重组快照环境、执行方式、告警和可在线调整的资源参数。
/// DDL 重组快照环境：SQLMode、告警、执行方式与可在线调整的资源参数。
#[derive(Debug, Default)]
pub struct DDLReorgMeta {
    /// 重组时的 SQL Mode 快照。
    pub SQLMode: mysql::SQLMode,
    /// 警告错误映射。
    pub Warnings: HashMap<errors::ErrorID, terror::Error>,
    /// 各警告出现次数。
    pub WarningsCount: HashMap<errors::ErrorID, i64>,
    /// 时区位置（可选）。
    pub Location: Option<Box<TimeZoneLocation>>,
    /// 回填写入策略类型。
    pub ReorgTp: ReorgType,
    /// 是否启用快速重组。
    pub IsFastReorg: bool,
    /// 是否分布式重组。
    pub IsDistReorg: bool,
    /// 是否使用云存储落盘中间结果。
    pub UseCloudStorage: bool,
    /// 所属资源组名称。
    pub ResourceGroupName: String,
    /// 元数据版本。
    pub Version: i64,
    /// 目标执行范围（scope）。
    pub TargetScope: String,
    /// 参与节点数上限。
    pub MaxNodeCount: i32,
    /// ANALYZE 子状态。
    pub AnalyzeState: i8,
    /// 改列重组阶段。
    pub Stage: ReorgStage,
    // None 表示字段尚未加入时生成的旧元数据，读取时由调用方提供默认值。
    pub UseNewCollate: Option<bool>,
    // 三个原子量会被 admin alter ddl jobs 在线更新，调用方不得绕过 getter/setter。
    pub Concurrency: AtomicI64,
    pub BatchSize: AtomicI64,
    pub MaxWriteSpeed: AtomicI64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
/// 序列化用中间结构：原子字段以普通 i64 落地 JSON。
struct DDLReorgMetaWire {
    #[serde(rename = "sql_mode")]
    SQLMode: mysql::SQLMode,
    #[serde(rename = "warnings", deserialize_with = "deserialize_null_default")]
    Warnings: HashMap<errors::ErrorID, terror::Error>,
    #[serde(
        rename = "warnings_count",
        deserialize_with = "deserialize_null_default"
    )]
    WarningsCount: HashMap<errors::ErrorID, i64>,
    #[serde(rename = "location")]
    Location: Option<Box<TimeZoneLocation>>,
    #[serde(rename = "reorg_tp")]
    ReorgTp: ReorgType,
    #[serde(rename = "is_fast_reorg")]
    IsFastReorg: bool,
    #[serde(rename = "is_dist_reorg")]
    IsDistReorg: bool,
    #[serde(rename = "use_cloud_storage")]
    UseCloudStorage: bool,
    #[serde(rename = "resource_group_name")]
    ResourceGroupName: String,
    #[serde(rename = "version")]
    Version: i64,
    #[serde(rename = "target_scope")]
    TargetScope: String,
    #[serde(rename = "max_node_count")]
    MaxNodeCount: i32,
    #[serde(rename = "analyze_state")]
    AnalyzeState: i8,
    #[serde(rename = "stage")]
    Stage: ReorgStage,
    #[serde(rename = "use_new_collate", skip_serializing_if = "Option::is_none")]
    UseNewCollate: Option<bool>,
    #[serde(rename = "concurrency")]
    Concurrency: i64,
    #[serde(rename = "batch_size")]
    BatchSize: i64,
    #[serde(rename = "max_write_speed")]
    MaxWriteSpeed: i64,
}

impl Serialize for DDLReorgMeta {
    /// 将原子字段 load 后写入 wire 结构再序列化。
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // 原子量以 SeqCst 读取，保证与 admin 在线更新观测一致。
        DDLReorgMetaWire {
            SQLMode: self.SQLMode,
            Warnings: self.Warnings.clone(),
            WarningsCount: self.WarningsCount.clone(),
            Location: self.Location.clone(),
            ReorgTp: self.ReorgTp,
            IsFastReorg: self.IsFastReorg,
            IsDistReorg: self.IsDistReorg,
            UseCloudStorage: self.UseCloudStorage,
            ResourceGroupName: self.ResourceGroupName.clone(),
            Version: self.Version,
            TargetScope: self.TargetScope.clone(),
            MaxNodeCount: self.MaxNodeCount,
            AnalyzeState: self.AnalyzeState,
            Stage: self.Stage,
            UseNewCollate: self.UseNewCollate,
            Concurrency: self.Concurrency.load(Ordering::SeqCst),
            BatchSize: self.BatchSize.load(Ordering::SeqCst),
            MaxWriteSpeed: self.MaxWriteSpeed.load(Ordering::SeqCst),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DDLReorgMeta {
    /// 从 wire 结构还原，原子字段重新包装为 AtomicI64。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = DDLReorgMetaWire::deserialize(deserializer)?;
        Ok(Self {
            SQLMode: wire.SQLMode,
            Warnings: wire.Warnings,
            WarningsCount: wire.WarningsCount,
            Location: wire.Location,
            ReorgTp: wire.ReorgTp,
            IsFastReorg: wire.IsFastReorg,
            IsDistReorg: wire.IsDistReorg,
            UseCloudStorage: wire.UseCloudStorage,
            ResourceGroupName: wire.ResourceGroupName,
            Version: wire.Version,
            TargetScope: wire.TargetScope,
            MaxNodeCount: wire.MaxNodeCount,
            AnalyzeState: wire.AnalyzeState,
            Stage: wire.Stage,
            UseNewCollate: wire.UseNewCollate,
            Concurrency: AtomicI64::new(wire.Concurrency),
            BatchSize: AtomicI64::new(wire.BatchSize),
            MaxWriteSpeed: AtomicI64::new(wire.MaxWriteSpeed),
        })
    }
}

impl DDLReorgMeta {
    // ShallowCopy 对应 Go 的结构体值拷贝；原子值在同一时刻以顺序一致语义装载到新实例。
    /// 结构体浅拷贝；原子值在同一时刻顺序一致装载到新实例。
    pub fn ShallowCopy(&self) -> Self {
        Self {
            SQLMode: self.SQLMode,
            Warnings: self.Warnings.clone(),
            WarningsCount: self.WarningsCount.clone(),
            Location: self.Location.clone(),
            ReorgTp: self.ReorgTp,
            IsFastReorg: self.IsFastReorg,
            IsDistReorg: self.IsDistReorg,
            UseCloudStorage: self.UseCloudStorage,
            ResourceGroupName: self.ResourceGroupName.clone(),
            Version: self.Version,
            TargetScope: self.TargetScope.clone(),
            MaxNodeCount: self.MaxNodeCount,
            AnalyzeState: self.AnalyzeState,
            Stage: self.Stage,
            UseNewCollate: self.UseNewCollate,
            Concurrency: AtomicI64::new(self.Concurrency.load(Ordering::SeqCst)),
            BatchSize: AtomicI64::new(self.BatchSize.load(Ordering::SeqCst)),
            MaxWriteSpeed: AtomicI64::new(self.MaxWriteSpeed.load(Ordering::SeqCst)),
        }
    }

    // GetConcurrency 对旧集群产生的零值元数据回退到当前全局 DDL worker 配置。
    /// 获取并发度；零值回退到当前全局 DDL worker 配置。
    pub fn GetConcurrency(&self) -> i32 {
        let concurrency = self.Concurrency.load(Ordering::SeqCst);
        if concurrency == 0 {
            vardef::GetDDLReorgWorkerCounter() as i32
        } else {
            concurrency as i32
        }
    }

    /// 设置重组并发度（可被 admin alter ddl jobs 在线更新）。
    pub fn SetConcurrency(&self, concurrency: i32) {
        self.Concurrency.store(concurrency as i64, Ordering::SeqCst);
    }

    // GetBatchSize 与并发度相同：零值表示旧元数据，回退到当前全局批大小。
    /// 获取批大小；零值回退到当前全局批大小。
    pub fn GetBatchSize(&self) -> i32 {
        let batch_size = self.BatchSize.load(Ordering::SeqCst);
        if batch_size == 0 {
            vardef::GetDDLReorgBatchSize() as i32
        } else {
            batch_size as i32
        }
    }

    /// 设置重组批大小。
    pub fn SetBatchSize(&self, batch_size: i32) {
        self.BatchSize.store(batch_size as i64, Ordering::SeqCst);
    }

    // MaxWriteSpeed 的零值本身表示不限速，因此不做旧版本回退。
    /// 获取最大写入速度；零值表示不限速。
    pub fn GetMaxWriteSpeed(&self) -> i32 {
        self.MaxWriteSpeed.load(Ordering::SeqCst) as i32
    }

    /// 设置最大写入速度。
    pub fn SetMaxWriteSpeed(&self, max_write_speed: i32) {
        self.MaxWriteSpeed
            .store(max_write_speed as i64, Ordering::SeqCst);
    }

    // GetUseNewCollateOrDefault 让字段加入前生成的元数据沿用调用环境默认值。
    /// 读取新排序规则开关；缺失时使用调用方默认值。
    pub fn GetUseNewCollateOrDefault(&self, default_val: bool) -> bool {
        self.UseNewCollate.unwrap_or(default_val)
    }

    // setUseNewCollate 记录持久化表快照当时的新排序规则开关，而非执行进程的当前值。
    /// 记录持久化表快照当时的新排序规则开关。
    pub fn setUseNewCollate(&mut self, use_new_collate: bool) {
        self.UseNewCollate = Some(use_new_collate);
    }
}

/// 重组元数据版本 0（历史默认）。
pub const ReorgMetaVersion0: i64 = 0;
// 版本 1 用于修正 #46306 中表范围终止 key 是否包含的解释。
/// 当前重组元数据版本（修正表范围终止 key 包含语义）。
pub const CurrentReorgMetaVersion: i64 = 1;

/// ANALYZE：未开始。
pub const AnalyzeStateNone: i8 = 0;
/// ANALYZE：运行中。
pub const AnalyzeStateRunning: i8 = 1;
/// ANALYZE：已跳过。
pub const AnalyzeStateSkipped: i8 = 2;
/// ANALYZE：已完成。
pub const AnalyzeStateDone: i8 = 3;
/// ANALYZE：超时。
pub const AnalyzeStateTimeout: i8 = 4;
/// ANALYZE：失败。
pub const AnalyzeStateFailed: i8 = 5;

// ReorgType 标识 backfill 数据写入与增量合并策略。
/// 标识 backfill 数据写入与增量合并策略。
#[repr(i8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize_repr, Deserialize_repr)]
pub enum ReorgType {
    #[default]
    /// 未指定回填类型。
    ReorgTypeNone = 0,
    /// 全部索引 KV 经事务接口写入（原始实现）。
    // 原始实现：全部索引 KV 经事务接口写入。
    ReorgTypeTxn = 1,
    /// Lightning 编码/导入 SST；DML 增量先写临时索引，最后回并。
    // Lightning 编码/导入 SST，DML 增量先写临时索引，最后回并。
    ReorgTypeIngest = 2,
    /// 主 backfill 走事务，仍把 DML 增量重定向并在结束后合并。
    // 主 backfill 走事务，但仍把 DML 增量重定向并在结束后合并。
    ReorgTypeTxnMerge = 3,
}

impl ReorgType {
    /// Ingest / TxnMerge 需要额外的增量合并阶段。
    pub fn NeedMergeProcess(self) -> bool {
        matches!(self, Self::ReorgTypeIngest | Self::ReorgTypeTxnMerge)
    }

    /// 返回短类型名（txn / ingest / txn-merge）；None 为空串。
    pub fn String(self) -> &'static str {
        match self {
            Self::ReorgTypeTxn => "txn",
            Self::ReorgTypeIngest => "ingest",
            Self::ReorgTypeTxnMerge => "txn-merge",
            Self::ReorgTypeNone => "",
        }
    }
}

// BackfillMeta 是单个 backfill job 的可序列化进度，包括范围、当前位置、告警及父 JobMeta。
/// 单个 backfill job 的可序列化进度：范围、当前位置、告警及父 JobMeta。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BackfillMeta {
    /// 是否唯一索引回填。
    #[serde(rename = "is_unique")]
    pub IsUnique: bool,
    /// 终止 key 是否包含 EndKey。
    #[serde(rename = "end_include")]
    pub EndInclude: bool,
    /// 回填错误（若有）。
    #[serde(rename = "err")]
    pub Error: Option<Box<terror::Error>>,
    /// SQL Mode 快照。
    #[serde(rename = "sql_mode")]
    pub SQLMode: mysql::SQLMode,
    /// 警告错误映射。
    #[serde(rename = "warnings", deserialize_with = "deserialize_null_default")]
    pub Warnings: HashMap<errors::ErrorID, terror::Error>,
    /// 各警告出现次数。
    #[serde(
        rename = "warnings_count",
        deserialize_with = "deserialize_null_default"
    )]
    pub WarningsCount: HashMap<errors::ErrorID, i64>,
    /// 时区位置。
    #[serde(rename = "location")]
    pub Location: Option<Box<TimeZoneLocation>>,
    /// 回填类型。
    #[serde(rename = "reorg_tp")]
    pub ReorgTp: ReorgType,
    /// 已处理行数。
    #[serde(rename = "row_count")]
    pub RowCount: i64,
    /// 扫描起始 key。
    #[serde(rename = "start_key", with = "go_bytes")]
    pub StartKey: Vec<u8>,
    /// 扫描终止 key。
    #[serde(rename = "end_key", with = "go_bytes")]
    pub EndKey: Vec<u8>,
    /// 当前进度 key。
    #[serde(rename = "curr_key", with = "go_bytes")]
    pub CurrKey: Vec<u8>,
    /// 父 DDL job 元数据。
    #[serde(rename = "job_meta")]
    pub JobMeta: Option<Box<JobMeta>>,
}

impl BackfillMeta {
    // Encode 对应 json.Marshal；序列化错误继续经 errors.Trace 保留调用栈语义。
    /// JSON 序列化进度；错误经 Trace 保留调用栈语义。
    pub fn Encode(&self) -> Result<Vec<u8>, errors::Error> {
        serde_json::to_vec(self).map_err(errors::Trace)
    }

    // Decode 对应 json.Unmarshal，并原地替换接收者，避免部分字段解码成功后泄露半成品状态。
    /// JSON 反序列化并原地替换，避免半成品状态泄漏。
    pub fn Decode(&mut self, bytes: &[u8]) -> Result<(), errors::Error> {
        let decoded = serde_json::from_slice(bytes).map_err(errors::Trace)?;
        *self = decoded;
        Ok(())
    }
}
