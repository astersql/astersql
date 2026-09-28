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

// 导入引擎公共接口：本地/外部引擎统一操作面、键范围与重复键策略。
//
// 对应 Go `ingestor/engineapi` 的 Engine / Range / ConflictInfo / OnDuplicateKey。
// Lightning 与 IMPORT INTO 通过本接口加载可导入数据、统计冲突并生成 Region 切分键。
//
// Region：TiKV 数据分片；KV：键值对；重复键策略决定全局排序发现同键后的处理方式。

use std::sync::mpsc::SyncSender;

use crate::ingest_data::{Context, EngineError, IngestData};

/// Range 对应 Go 的同名结构，保存未经重复检测编码的起止键。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Range {
    /// 范围起始键（通常闭区间左端）。
    pub Start: Vec<u8>,
    /// End 通常是开区间边界；只有 import_sstpb.SSTMeta 使用例外语义。
    pub End: Vec<u8>,
}

/// DataAndRanges 把一份可导入数据与其有序范围绑定。
/// 每个范围会生成一个 regionJob，并由该任务读取 Data；这里只保留所有权形状，不启动任务。
#[allow(non_snake_case)]
pub struct DataAndRanges {
    /// Go 的接口值迁移为 trait object，以保留本地和外部数据实现的动态分发。
    pub Data: Box<dyn IngestData>,
    pub SortedRanges: Vec<Range>,
}

/// Engine 对应 Go 的公共引擎接口，供 local backend 统一操作本地与外部引擎。
#[allow(non_snake_case)]
pub trait Engine: Send + Sync {
    /// ID 返回引擎标识符。
    fn ID(&self) -> String;

    /// LoadIngestData 对应 Go 的 `chan<- DataAndRanges`：实现只能向 outCh 发送批次。
    /// Context 的取消传播与 channel 阻塞/关闭语义需在后续接线时保持。
    fn LoadIngestData(
        &self,
        ctx: &Context,
        outCh: &SyncSender<DataAndRanges>,
    ) -> Result<(), EngineError>;

    /// KVStatistics 返回引擎内 KV 的总字节数与总条数，顺序与 Go 命名返回值一致。
    fn KVStatistics(&self) -> (i64, i64);

    /// ImportedStatistics 返回已经导入的 KV 字节数与条数。
    fn ImportedStatistics(&self) -> (i64, i64);

    /// ConflictInfo 返回冲突 KV 信息。
    /// Go 暂时把外部引擎特有能力放进公共接口，以兼容仍需考虑 Lightning TiDB backend 的代码。
    fn ConflictInfo(&self) -> ConflictInfo;

    /// GetKeyRange 返回半开区间 `[startKey, endKey)`。
    /// 即使引擎内部启用了重复检测编码，返回键也必须先解码。
    fn GetKeyRange(&self) -> Result<(Vec<u8>, Vec<u8>), EngineError>;

    /// GetRegionSplitKeys 根据引擎内 KV 分布生成 Region 切分键。
    /// 返回键不得携带重复检测编码，且必须包含本次导入的起止键。
    fn GetRegionSplitKeys(&self) -> Result<Vec<Vec<u8>>, EngineError>;

    /// Close 对应 Go 的资源收尾；实现负责释放文件、reader 等外部资源并返回关闭错误。
    fn Close(&mut self) -> Result<(), EngineError>;
}

/// ConflictInfo 记录会造成表中行冲突的 PK/UK KV，而不是所有同键的 duplicate KV。
/// 非唯一索引键可能因主键重复而成为 duplicate KV，但冲突消解不需要处理这部分键。
#[derive(Clone, Debug, Default)]
#[allow(non_snake_case)]
pub struct ConflictInfo {
    /// 已记录的冲突 KV 对数量，可来自主键或唯一键。
    pub Count: u64,
    /// 含冲突 KV 的文件列表，文件格式与普通 KV 文件相同。
    pub Files: Vec<String>,
}

#[allow(non_snake_case)]
impl ConflictInfo {
    /// Merge 对应 Go 的指针接收者方法：累加计数并按原顺序接管另一份文件列表。
    pub fn Merge(&mut self, other: &ConflictInfo) {
        // Go unsigned integer arithmetic wraps; use an explicit operation so the
        // behavior is identical in Rust debug and release builds.
        // Go 无符号整数溢出回绕；显式 wrapping_add 保证 debug/release 行为一致。
        self.Count = self.Count.wrapping_add(other.Count);
        self.Files.extend(other.Files.iter().cloned());
    }
}

/// OnDuplicateKey 表示全局排序发现重复键后的动作。
/// Lightning 的 OnDup 名称相近但语义不同；该枚举放在 engineapi 中是为了避免 Go import cycle。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OnDuplicateKey(pub i32);

/// 保持当前行为并忽略重复键，直到完全切换到其余三种策略。
#[allow(non_upper_case_globals)]
pub const OnDuplicateKeyIgnore: OnDuplicateKey = OnDuplicateKey(0);
/// 把重复键记录到外部存储；编码或归并排序阶段只看到局部有序数据，
/// 因此某些步骤可能仅在重复次数大于 2 时记录。IMPORT INTO 的 PK/UK 使用该策略。
#[allow(non_upper_case_globals)]
pub const OnDuplicateKeyRecord: OnDuplicateKey = OnDuplicateKey(1);
/// 静默移除重复键，IMPORT INTO 的非唯一二级索引使用该策略。
#[allow(non_upper_case_globals)]
pub const OnDuplicateKeyRemove: OnDuplicateKey = OnDuplicateKey(2);
/// 发现重复键时返回错误，可用于增加唯一索引。
#[allow(non_upper_case_globals)]
pub const OnDuplicateKeyError: OnDuplicateKey = OnDuplicateKey(3);

impl std::fmt::Display for OnDuplicateKey {
    /// fmt 对应 Go 的 String 方法，并保留四个稳定的外部字符串。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 将策略枚举映射为稳定的对外字符串（与 Go fmt.Stringer 一致）。
        let value = match *self {
            OnDuplicateKeyIgnore => "ignore",
            OnDuplicateKeyRecord => "record",
            OnDuplicateKeyRemove => "remove",
            OnDuplicateKeyError => "error",
            // Go 的命名整数类型允许构造常量之外的值，必须保留 unknown 回退。
            OnDuplicateKey(_) => "unknown",
        };
        f.write_str(value)
    }
}

#[allow(non_snake_case)]
impl OnDuplicateKey {
    /// String 保留 Go `fmt.Stringer` 的显式方法形状。
    pub fn String(&self) -> String {
        self.to_string()
    }
}
