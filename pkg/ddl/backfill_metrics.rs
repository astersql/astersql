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

// DDL 回填（backfill）指标模块。
//
// “回填”指在执行 DDL（数据定义语言，如加索引、改列类型、分区重组）时，
// 后台任务需要按行扫描已有数据并补写新的索引或列数据的过程。
// 本模块负责：
// - 定义各类回填操作在监控系统中使用的标签（label）常量；
// - 根据回填动作与标签计算指标应挂载到的表 ID（逻辑表或物理分区）；
// - 提供 `BackfillMetrics` 结构，按指标键累计总量并记录进度百分比。

use std::collections::BTreeMap;

/// 添加索引回填的指标标签。
pub const LABEL_ADD_INDEX: &str = "add_index";
/// 添加索引过程中合并临时索引（temp index merge）阶段的指标标签。
pub const LABEL_ADD_INDEX_MERGE: &str = "add_index_merge_tmp";
/// 修改列类型（需要重写数据）回填的指标标签。
pub const LABEL_MODIFY_COLUMN: &str = "modify_column";
/// 分区重组（reorganize partition）回填的指标标签。
pub const LABEL_REORG_PARTITION: &str = "reorg_partition";
/// 分区重组速率类指标的标签前缀。
pub const LABEL_REORG_PARTITION_RATE: &str = "reorg_partition_rate";
/// 清理索引速率指标的标签（用于删除/截断分区后的索引清理）。
pub const LABEL_CLEANUP_INDEX_RATE: &str = "cleanup_idx_rate";

/// 需要回填数据的 DDL 动作类型。
///
/// 每个变体对应一种会触发后台数据回填的 DDL 操作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackfillAction {
    /// 添加二级索引。
    AddIndex,
    /// 添加主键（本质上也是构建一个索引）。
    AddPrimaryKey,
    /// 修改列类型，需重写整表数据。
    ModifyColumn,
    /// 重组分区：把若干分区的数据重新划分到新分区。
    ReorganizePartition,
    /// 修改表的分区方式（如从非分区表改为分区表）。
    AlterTablePartitioning,
    /// 移除分区（把分区表变回普通表）。
    RemovePartitioning,
    /// 删除表分区。
    DropTablePartition,
    /// 截断（清空）表分区。
    TruncateTablePartition,
    /// 其他不属于上述分类的动作。
    Other,
}

/// 根据回填动作返回其进度指标使用的标签。
///
/// `merging_temporary_index` 表示添加索引是否处于“合并临时索引”阶段：
/// 在线加索引时，DML 产生的增量数据会先写入临时索引，最后再合并回目标索引。
/// 不产生进度指标的动作返回空字符串。
pub fn backfill_progress_label(
    action: BackfillAction,
    merging_temporary_index: bool,
) -> &'static str {
    match action {
        // 加索引与加主键共用一套标签，按是否处于临时索引合并阶段区分。
        BackfillAction::AddIndex | BackfillAction::AddPrimaryKey => {
            if merging_temporary_index {
                LABEL_ADD_INDEX_MERGE
            } else {
                LABEL_ADD_INDEX
            }
        }
        BackfillAction::ModifyColumn => LABEL_MODIFY_COLUMN,
        // 三种分区变更操作均归入“分区重组”标签。
        BackfillAction::ReorganizePartition
        | BackfillAction::AlterTablePartitioning
        | BackfillAction::RemovePartitioning => LABEL_REORG_PARTITION,
        _ => "",
    }
}

/// 判断动作是否属于分区重组类操作（重组分区、修改分区方式、移除分区）。
pub fn is_partition_reorganization(action: BackfillAction) -> bool {
    matches!(
        action,
        BackfillAction::ReorganizePartition
            | BackfillAction::AlterTablePartitioning
            | BackfillAction::RemovePartitioning
    )
}

/// 判断动作是否为删除分区或截断分区。
pub fn is_partition_drop_or_truncate(action: BackfillAction) -> bool {
    matches!(
        action,
        BackfillAction::DropTablePartition | BackfillAction::TruncateTablePartition
    )
}

/// 判断指标标签是否属于分区重组类（等于重组标签，或以重组速率标签为前缀）。
pub fn is_partition_reorganization_label(label: &str) -> bool {
    label == LABEL_REORG_PARTITION || label.starts_with(LABEL_REORG_PARTITION_RATE)
}

/// 回填指标上报所需的表信息。
///
/// 分区表中，“逻辑表”指整张分区表本身，“物理表”指其中某个分区；
/// 指标可能需要挂在逻辑表或具体分区上，由动作与标签共同决定。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReorganizationMetricInfo {
    /// 当前回填对应的 DDL 动作。
    pub action: BackfillAction,
    /// 逻辑表 ID；非分区场景可能为空。
    pub logical_table_id: Option<i64>,
    /// 物理表（分区）ID。
    pub physical_table_id: i64,
}

/// 计算指标应关联的表 ID。
///
/// 规则：
/// - 无信息时返回 0；
/// - 非分区重组动作默认使用物理表 ID，但删除/截断分区后的索引清理
///   速率指标应挂在逻辑表上；
/// - 分区重组动作中，重组类标签挂逻辑表，其余挂物理表。
pub fn backfill_metrics_table_id(info: Option<&ReorganizationMetricInfo>, label: &str) -> i64 {
    let Some(info) = info else {
        return 0;
    };
    if !is_partition_reorganization(info.action) {
        // 删除/截断分区后的索引清理速率：分区已不存在，指标归属逻辑表。
        if label == LABEL_CLEANUP_INDEX_RATE && is_partition_drop_or_truncate(info.action) {
            return info.logical_table_id.unwrap_or(info.physical_table_id);
        }
        return info.physical_table_id;
    }
    // 分区重组：重组类标签统计整表进度，用逻辑表 ID；否则用物理分区 ID。
    if is_partition_reorganization_label(label) {
        info.logical_table_id.unwrap_or(info.physical_table_id)
    } else {
        info.physical_table_id
    }
}

/// 指标键：唯一标识一条回填指标序列。
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MetricKey {
    /// 指标关联的表 ID（逻辑表或物理分区，见 `backfill_metrics_table_id`）。
    pub table_id: i64,
    /// 指标标签，如 `add-index`。
    pub label: String,
    /// 库（schema）名。
    pub schema_name: String,
    /// 表名。
    pub table_name: String,
    /// 对象名（如索引名、列名）。
    pub object_name: String,
}

/// 回填指标存储：按键累计总量并记录进度百分比。
///
/// 使用 `BTreeMap` 使遍历顺序稳定，便于确定性输出。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackfillMetrics {
    /// 各指标键的累计总量（如已处理行数）。
    totals: BTreeMap<MetricKey, f64>,
    /// 各指标键的进度百分比（0~100）。
    progress: BTreeMap<MetricKey, f64>,
}

impl BackfillMetrics {
    /// 将 `value` 累加到指定键的总量上，键不存在时从 0 开始。
    pub fn add_total(&mut self, key: MetricKey, value: f64) {
        *self.totals.entry(key).or_default() += value;
    }
    /// 设置进度值，自动截断到 [0, 100] 区间。
    pub fn set_progress(&mut self, key: MetricKey, value: f64) {
        self.progress.insert(key, value.clamp(0.0, 100.0));
    }
    /// 读取累计总量，不存在时返回 0。
    pub fn total(&self, key: &MetricKey) -> f64 {
        self.totals.get(key).copied().unwrap_or_default()
    }
    /// 读取进度值，不存在时返回 0。
    pub fn progress(&self, key: &MetricKey) -> f64 {
        self.progress.get(key).copied().unwrap_or_default()
    }
    /// 清除指定表的所有指标（例如 DDL 结束或回滚后）。
    pub fn clear_table(&mut self, table_id: i64) {
        self.totals.retain(|key, _| key.table_id != table_id);
        self.progress.retain(|key, _| key.table_id != table_id);
    }
}
