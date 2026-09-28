// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 表级统计：`Table`/`HistColl`、列索引存在图、伪统计、拷贝意图与健康度。
//
// 优化器与自动 ANALYZE 依赖此结构持有列/索引直方图集合、实时行数与修改量；
// 伪表在无真实统计时提供默认行数估计。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::{Column, Index, IsAnalyzed, NewStatsFullLoadStatus, StatsLoadedStatus, Version2};

/// 伪统计版本号（无真实 ANALYZE 版本时使用）。
pub const PseudoVersion: u64 = 0;
/// 伪表默认估计行数。
pub const PseudoRowCount: i64 = 10_000;
/// Default Go `statistics.AutoAnalyzeMinCnt` value.
/// Go 默认自动 ANALYZE 最小行数阈值。
pub const AutoAnalyzeMinCnt: i64 = 1_000;

/// 修改行数占比超过该阈值时，统计信息视为过期（对应 Go 可变全局值）。
static RATIO_OF_PSEUDO_ESTIMATE: AtomicU64 = AtomicU64::new(0.7_f64.to_bits());

/// 返回当前过期统计阈值。
pub fn RatioOfPseudoEstimate() -> f64 {
    f64::from_bits(RATIO_OF_PSEUDO_ESTIMATE.load(Ordering::SeqCst))
}

/// 设置过期统计阈值，供测试与兼容配置使用。
pub fn SetRatioOfPseudoEstimate(ratio: f64) {
    RATIO_OF_PSEUDO_ESTIMATE.store(ratio.to_bits(), Ordering::SeqCst);
}

/// 测试可覆盖的 AutoAnalyzeMinCnt；负数表示使用默认常量。
static AUTO_ANALYZE_MIN_CNT_OVERRIDE: AtomicI64 = AtomicI64::new(-1);

/// Effective auto-analyze minimum row count (Go mutable `AutoAnalyzeMinCnt`).
/// 当前生效的自动 ANALYZE 最小行数（对应 Go 可变的 AutoAnalyzeMinCnt）。
pub fn EffectiveAutoAnalyzeMinCnt() -> i64 {
    let override_value = AUTO_ANALYZE_MIN_CNT_OVERRIDE.load(Ordering::SeqCst);
    if override_value < 0 {
        AutoAnalyzeMinCnt
    } else {
        override_value
    }
}

/// Test/helper setter matching Go `statistics.AutoAnalyzeMinCnt = value`.
/// 测试辅助：覆盖 AutoAnalyzeMinCnt。
pub fn SetAutoAnalyzeMinCnt(value: i64) {
    AUTO_ANALYZE_MIN_CNT_OVERRIDE.store(value, Ordering::SeqCst);
}

/// Restore the default Go `AutoAnalyzeMinCnt` of 1000.
/// 恢复默认 AutoAnalyzeMinCnt（清除覆盖）。
pub fn ResetAutoAnalyzeMinCnt() {
    AUTO_ANALYZE_MIN_CNT_OVERRIDE.store(-1, Ordering::SeqCst);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 拷贝表统计时的意图：仅元数据、可写列/索引图或全部数据可写。
pub enum CopyIntent {
    MetaOnly,
    ColumnMapWritable,
    IndexMapWritable,
    BothMapsWritable,
    AllDataWritable,
}

#[derive(Clone, Debug)]
/// 一张物理表的统计快照：直方图集合、存在图与 ANALYZE 版本时间戳。
pub struct Table {
    pub ColAndIdxExistenceMap: Box<ColAndIdxExistenceMap>,
    pub HistColl: HistColl,
    pub Version: u64,
    pub LastAnalyzeVersion: u64,
    pub LastStatsHistVersion: u64,
    pub TblInfoUpdateTS: u64,
    pub IsPkIsHandle: bool,
}

impl std::ops::Deref for Table {
    type Target = HistColl;

    fn deref(&self) -> &Self::Target {
        &self.HistColl
    }
}

impl std::ops::DerefMut for Table {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.HistColl
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 列/索引是否存在及是否已 ANALYZE 的布尔映射。
pub struct ColAndIdxExistenceMap {
    pub colAnalyzed: HashMap<i64, bool>,
    pub idxAnalyzed: HashMap<i64, bool>,
}

impl ColAndIdxExistenceMap {
    /// 深拷贝为堆上新盒子。
    pub fn Clone(&self) -> Box<ColAndIdxExistenceMap> {
        Box::new(std::clone::Clone::clone(self))
    }

    /// 删除指定列 ID 的存在记录。
    pub fn DeleteColNotFound(&mut self, id: i64) {
        self.colAnalyzed.remove(&id);
    }

    /// 删除指定索引 ID 的存在记录。
    pub fn DeleteIdxNotFound(&mut self, id: i64) {
        self.idxAnalyzed.remove(&id);
    }

    /// 查询该列/索引是否标记为已分析。
    pub fn HasAnalyzed(&self, id: i64, is_index: bool) -> bool {
        if is_index {
            self.idxAnalyzed.get(&id).copied().unwrap_or(false)
        } else {
            self.colAnalyzed.get(&id).copied().unwrap_or(false)
        }
    }

    /// 查询是否存在该列/索引条目（不论是否已分析）。
    pub fn Has(&self, id: i64, is_index: bool) -> bool {
        if is_index {
            self.idxAnalyzed.contains_key(&id)
        } else {
            self.colAnalyzed.contains_key(&id)
        }
    }

    /// 插入或更新列存在/分析状态。
    pub fn InsertCol(&mut self, id: i64, analyzed: bool) {
        self.colAnalyzed.insert(id, analyzed);
    }

    /// 插入或更新索引存在/分析状态。
    pub fn InsertIndex(&mut self, id: i64, analyzed: bool) {
        self.idxAnalyzed.insert(id, analyzed);
    }

    /// 列与索引映射是否皆为空。
    pub fn IsEmpty(&self) -> bool {
        self.colAnalyzed.is_empty() && self.idxAnalyzed.is_empty()
    }

    /// 列条目数量。
    pub fn ColNum(&self) -> usize {
        self.colAnalyzed.len()
    }

    /// 克隆整张存在图。
    pub fn CloneMap(&self) -> Box<ColAndIdxExistenceMap> {
        Box::new(self.clone())
    }
}

/// 默认列存在图容量。
pub const defaultColCap: usize = 16;
/// 默认索引存在图容量。
pub const defaultIdxCap: usize = 4;

/// 使用默认容量创建列/索引存在图。
pub fn NewColAndIndexExistenceMapWithoutSize() -> Box<ColAndIdxExistenceMap> {
    NewColAndIndexExistenceMap(defaultColCap, defaultIdxCap)
}

/// 按指定容量创建列/索引存在图。
pub fn NewColAndIndexExistenceMap(
    column_capacity: usize,
    index_capacity: usize,
) -> Box<ColAndIdxExistenceMap> {
    Box::new(ColAndIdxExistenceMap {
        colAnalyzed: HashMap::with_capacity(column_capacity),
        idxAnalyzed: HashMap::with_capacity(index_capacity),
    })
}

/// 比较两张存在图是否相等。
pub fn ColAndIdxExistenceMapIsEqual(
    left: &ColAndIdxExistenceMap,
    right: &ColAndIdxExistenceMap,
) -> bool {
    left == right
}

#[derive(Clone, Debug)]
/// 直方图集合：列/索引统计、物理表 ID、实时行数、修改量与 ID 映射。
pub struct HistColl {
    pub Columns: HashMap<i64, Box<Column>>,
    pub Indices: HashMap<i64, Box<Index>>,
    pub PhysicalID: i64,
    pub RealtimeCount: i64,
    pub ModifyCount: i64,
    pub StatsVer: i32,
    pub Pseudo: bool,
    pub CanNotTriggerLoad: bool,
    pub Idx2ColUniqueIDs: HashMap<i64, Vec<i64>>,
    pub ColUniqueID2IdxIDs: HashMap<i64, Vec<i64>>,
    pub UniqueID2colInfoID: HashMap<i64, i64>,
    pub MVIdx2Columns: HashMap<i64, Vec<i64>>,
}

/// 创建空的直方图集合，预分配列/索引哈希容量。
pub fn NewHistColl(
    id: i64,
    realtime_count: i64,
    modify_count: i64,
    column_count: usize,
    index_count: usize,
) -> Box<HistColl> {
    Box::new(HistColl {
        Columns: HashMap::with_capacity(column_count),
        Indices: HashMap::with_capacity(index_count),
        PhysicalID: id,
        RealtimeCount: realtime_count,
        ModifyCount: modify_count,
        StatsVer: 0,
        Pseudo: false,
        CanNotTriggerLoad: false,
        Idx2ColUniqueIDs: HashMap::new(),
        ColUniqueID2IdxIDs: HashMap::new(),
        UniqueID2colInfoID: HashMap::new(),
        MVIdx2Columns: HashMap::new(),
    })
}

/// 用已有列/索引映射创建直方图集合。
pub fn NewHistCollWithColsAndIdxs(
    id: i64,
    realtime_count: i64,
    modify_count: i64,
    columns: HashMap<i64, Box<Column>>,
    indices: HashMap<i64, Box<Index>>,
) -> Box<HistColl> {
    let mut result = NewHistColl(
        id,
        realtime_count,
        modify_count,
        columns.len(),
        indices.len(),
    );
    result.Columns = columns;
    result.Indices = indices;
    result
}

impl HistColl {
    /// 设置/覆盖指定列统计。
    pub fn SetCol(&mut self, id: i64, column: Box<Column>) {
        self.Columns.insert(id, column);
    }

    /// 设置/覆盖指定索引统计。
    pub fn SetIdx(&mut self, id: i64, index: Box<Index>) {
        self.Indices.insert(id, index);
    }

    /// 按 ID 只读获取列统计。
    pub fn GetCol(&self, id: i64) -> Option<&Column> {
        self.Columns.get(&id).map(Box::as_ref)
    }

    /// 按 ID 可变获取列统计。
    pub fn GetColMut(&mut self, id: i64) -> Option<&mut Column> {
        self.Columns.get_mut(&id).map(Box::as_mut)
    }

    /// 按 ID 只读获取索引统计。
    pub fn GetIdx(&self, id: i64) -> Option<&Index> {
        self.Indices.get(&id).map(Box::as_ref)
    }

    /// 按 ID 可变获取索引统计。
    pub fn GetIdxMut(&mut self, id: i64) -> Option<&mut Index> {
        self.Indices.get_mut(&id).map(Box::as_mut)
    }

    /// 移除并返回列统计。
    pub fn RemoveCol(&mut self, id: i64) -> Option<Box<Column>> {
        self.Columns.remove(&id)
    }

    /// 移除并返回索引统计。
    pub fn RemoveIdx(&mut self, id: i64) -> Option<Box<Index>> {
        self.Indices.remove(&id)
    }

    /// 对多值索引（MV Index）按索引行数相对分析行数缩放实时/修改计数。
    pub fn GetScaledRealtimeAndModifyCnt(&self, item: &Index) -> (i64, i64) {
        if !item.Info.as_ref().is_some_and(|info| info.MVIndex) || !item.IsFullLoad() {
            return (self.RealtimeCount, self.ModifyCount);
        }
        let analyze_count = self.GetAnalyzeRowCount();
        let index_count = item.TotalRowCount();
        if analyze_count <= 0.0 || index_count <= 0.0 {
            return (self.RealtimeCount, self.ModifyCount);
        }
        let scale = index_count / analyze_count;
        (
            (self.RealtimeCount as f64 * scale) as i64,
            (self.ModifyCount as f64 * scale) as i64,
        )
    }

    /// 修改量相对实时行数超过约 70% 时视为过期。
    pub fn IsOutdated(&self) -> bool {
        let analyze_count = self.GetAnalyzeRowCount();
        let row_count = if analyze_count < 0.0 {
            self.RealtimeCount as f64
        } else {
            analyze_count
        };
        row_count > 0.0 && self.ModifyCount as f64 / row_count > RatioOfPseudoEstimate()
    }

    /// 浅层克隆整份直方图集合。
    pub fn Copy(&self) -> HistColl {
        self.clone()
    }

    /// 汇总所有列与索引的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.Columns
            .values()
            .map(|column| column.MemoryUsage())
            .sum::<i64>()
            + self
                .Indices
                .values()
                .map(|index| index.MemoryUsage())
                .sum::<i64>()
    }

    /// 返回统计版本号。
    pub fn GetStatsVer(&self) -> i32 {
        self.StatsVer
    }

    /// 不可变遍历列；visitor 返回 true 时提前停止。
    pub fn ForEachColumnImmutable(&self, mut visitor: impl FnMut(i64, &Column) -> bool) {
        for (id, column) in &self.Columns {
            if visitor(*id, column) {
                break;
            }
        }
    }

    /// 不可变遍历索引；visitor 返回 true 时提前停止。
    pub fn ForEachIndexImmutable(&self, mut visitor: impl FnMut(i64, &Index) -> bool) {
        for (id, index) in &self.Indices {
            if visitor(*id, index) {
                break;
            }
        }
    }

    /// 列统计条目数。
    pub fn ColNum(&self) -> usize {
        self.Columns.len()
    }

    /// 索引统计条目数。
    pub fn IdxNum(&self) -> usize {
        self.Indices.len()
    }

    /// 按列 ID 稳定排序后返回列引用切片。
    pub fn StableOrderColSlice(&self) -> Vec<&Column> {
        let mut entries = self.Columns.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(id, _)| **id);
        entries
            .into_iter()
            .map(|(_, column)| column.as_ref())
            .collect()
    }

    /// 无序返回所有列引用。
    pub fn GetColSlice(&self) -> Vec<&Column> {
        self.Columns.values().map(Box::as_ref).collect()
    }

    /// 按索引 ID 稳定排序后返回索引引用切片。
    pub fn StableOrderIdxSlice(&self) -> Vec<&Index> {
        let mut entries = self.Indices.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(id, _)| **id);
        entries
            .into_iter()
            .map(|(_, index)| index.as_ref())
            .collect()
    }

    /// 无序返回所有索引引用。
    pub fn GetIdxSlice(&self) -> Vec<&Index> {
        self.Indices.values().map(Box::as_ref).collect()
    }

    /// Bootstrap 时将全部索引标记为全量已加载。
    pub fn SetAllIndexFullLoadForBootstrap(&mut self) {
        for index in self.Indices.values_mut() {
            index.StatsLoadedStatus = NewStatsFullLoadStatus();
        }
    }

    /// 为所有列/索引直方图预计算标量边界。
    pub fn CalcPreScalar(&mut self) {
        for index in self.Indices.values_mut() {
            for bucket_index in 1..index.Histogram.Buckets.len() {
                index.Histogram.Buckets[bucket_index].Count +=
                    index.Histogram.Buckets[bucket_index - 1].Count;
            }
            index.Histogram.PreCalculateScalar();
        }
        for column in self.Columns.values_mut() {
            for bucket_index in 1..column.Histogram.Buckets.len() {
                column.Histogram.Buckets[bucket_index].Count +=
                    column.Histogram.Buckets[bucket_index - 1].Count;
            }
            column.Histogram.PreCalculateScalar();
        }
    }

    /// 丢弃仍需加载（未完整加载）的列/索引上的非必要数据。
    pub fn DropEvicted(&mut self) {
        for column in self.Columns.values_mut() {
            if column.StatsLoadedStatus.IsLoadNeeded() {
                column.DropUnnecessaryData();
            }
        }
        for index in self.Indices.values_mut() {
            if index.StatsLoadedStatus.IsLoadNeeded() {
                index.DropUnnecessaryData();
            }
        }
    }

    /// 取首个全量加载列（或非 MV 索引）的总行数作为分析行数；找不到返回 -1。
    pub fn GetAnalyzeRowCount(&self) -> f64 {
        for column in self.StableOrderColSlice() {
            if column.IsFullLoad() {
                return column.TotalRowCount();
            }
        }
        for index in self.StableOrderIdxSlice() {
            if index.Info.as_ref().is_some_and(|info| info.MVIndex) {
                continue;
            }
            if index.IsFullLoad() {
                return index.TotalRowCount();
            }
        }
        -1.0
    }

    /// 按列 info ID → unique ID 重映射列键，生成新的 HistColl。
    pub fn ID2UniqueID(&self, id_to_unique_id: &HashMap<i64, i64>) -> HistColl {
        let mut result = PseudoHistColl(self.PhysicalID, !self.CanNotTriggerLoad);
        result.Pseudo = self.Pseudo;
        result.RealtimeCount = self.RealtimeCount;
        result.ModifyCount = self.ModifyCount;
        result.StatsVer = self.StatsVer;
        for (id, unique_id) in id_to_unique_id {
            if let Some(column) = self.Columns.get(id) {
                result.Columns.insert(*unique_id, column.clone());
            }
        }
        result
    }

    /// 在 ID2UniqueID 基础上补齐索引及其列 unique ID 映射。
    pub fn GenerateHistCollFromColumnInfo(
        &self,
        id_to_unique_id: &HashMap<i64, i64>,
        index_to_column_ids: &HashMap<i64, Vec<i64>>,
    ) -> HistColl {
        let mut result = self.ID2UniqueID(id_to_unique_id);
        result.UniqueID2colInfoID = id_to_unique_id
            .iter()
            .map(|(id, unique_id)| (*unique_id, *id))
            .collect();
        for (index_id, column_ids) in index_to_column_ids {
            let unique_ids = column_ids
                .iter()
                .map_while(|id| id_to_unique_id.get(id).copied())
                .collect::<Vec<_>>();
            if unique_ids.is_empty() {
                continue;
            }
            if let Some(index) = self.Indices.get(index_id) {
                result.Indices.insert(*index_id, index.clone());
                result
                    .Idx2ColUniqueIDs
                    .insert(*index_id, unique_ids.clone());
                result
                    .ColUniqueID2IdxIDs
                    .entry(unique_ids[0])
                    .or_default()
                    .push(*index_id);
            }
        }
        for ids in result.ColUniqueID2IdxIDs.values_mut() {
            ids.sort_unstable();
        }
        result
    }
}

impl Table {
    /// 创建带空存在图与空 HistColl 的表统计。
    pub fn New(physical_id: i64, realtime_count: i64, modify_count: i64) -> Table {
        Table {
            ColAndIdxExistenceMap: NewColAndIndexExistenceMapWithoutSize(),
            HistColl: *NewHistColl(physical_id, realtime_count, modify_count, 0, 0),
            Version: PseudoVersion,
            LastAnalyzeVersion: 0,
            LastStatsHistVersion: 0,
            TblInfoUpdateTS: 0,
            IsPkIsHandle: false,
        }
    }

    /// 按列 ID 获取列统计。
    pub fn GetCol(&self, id: i64) -> Option<&Column> {
        self.HistColl.GetCol(id)
    }

    /// 按索引 ID 获取索引统计。
    pub fn GetIdx(&self, id: i64) -> Option<&Index> {
        self.HistColl.GetIdx(id)
    }

    /// 按拷贝意图复制表统计（当前各意图均完整 clone）。
    pub fn Copy(&self, intent: CopyIntent) -> Table {
        match intent {
            CopyIntent::MetaOnly
            | CopyIntent::ColumnMapWritable
            | CopyIntent::IndexMapWritable
            | CopyIntent::BothMapsWritable
            | CopyIntent::AllDataWritable => self.clone(),
        }
    }

    /// 汇总列/索引内存占用明细。
    pub fn MemoryUsage(&self) -> TableMemoryUsage {
        let columns = self
            .HistColl
            .Columns
            .iter()
            .map(|(id, column)| (*id, column.MemoryUsage()))
            .collect::<HashMap<_, _>>();
        let indices = self
            .HistColl
            .Indices
            .iter()
            .map(|(id, index)| (*id, index.MemoryUsage()))
            .collect::<HashMap<_, _>>();
        TableMemoryUsage {
            TotalMemUsage: columns.values().chain(indices.values()).sum(),
            Columns: columns,
            Indices: indices,
        }
    }

    /// Go Table.IsAnalyzed uses the last analyze timestamp, including metadata-only ANALYZE.
    pub fn IsAnalyzed(&self) -> bool {
        self.LastAnalyzeVersion > 0
    }

    /// 删除列统计并同步存在图。
    pub fn DelCol(&mut self, id: i64) {
        self.HistColl.Columns.remove(&id);
        self.ColAndIdxExistenceMap.DeleteColNotFound(id);
    }

    /// 删除索引统计并同步存在图。
    pub fn DelIdx(&mut self, id: i64) {
        self.HistColl.Indices.remove(&id);
        self.ColAndIdxExistenceMap.DeleteIdxNotFound(id);
    }

    /// `Copy` 的别名，对齐 Go CopyAs。
    pub fn CopyAs(&self, intent: CopyIntent) -> Table {
        self.Copy(intent)
    }

    /// 调试用字符串：表头 + 各列/索引 String。
    pub fn String(&self) -> String {
        let mut lines = vec![format!(
            "Table:{} RealtimeCount:{}",
            self.HistColl.PhysicalID, self.HistColl.RealtimeCount
        )];
        for column in self.HistColl.StableOrderColSlice() {
            lines.push(column.String());
        }
        for index in self.HistColl.StableOrderIdxSlice() {
            lines.push(index.String());
        }
        lines.join("\n")
    }

    /// 查找首列名匹配（忽略大小写）的索引。
    pub fn IndexStartWithColumn(&self, column_name: &str) -> Option<&Index> {
        self.HistColl.Indices.values().find_map(|index| {
            index.Info.as_ref().and_then(|info| {
                info.Columns
                    .first()
                    .is_some_and(|column| column.Name.eq_ignore_ascii_case(column_name))
                    .then_some(index.as_ref())
            })
        })
    }

    /// 按列名（忽略大小写）查找列统计。
    pub fn ColumnByName(&self, column_name: &str) -> Option<&Column> {
        self.HistColl.Columns.values().find_map(|column| {
            column.Info.as_ref().and_then(|info| {
                info.Name
                    .eq_ignore_ascii_case(column_name)
                    .then_some(column.as_ref())
            })
        })
    }

    /// 按列或索引 ID 取出直方图、CMSketch、TopN、FMSketch。
    pub fn GetStatsInfo(
        &self,
        id: i64,
        is_index: bool,
    ) -> Option<(
        &crate::Histogram,
        Option<&crate::CMSketch>,
        Option<&crate::TopN>,
        Option<&crate::FMSketch>,
    )> {
        if is_index {
            self.GetIdx(id).map(|index| {
                (
                    &index.Histogram,
                    index.CMSketch.as_ref(),
                    index.TopN.as_ref(),
                    index.FMSketch.as_ref(),
                )
            })
        } else {
            self.GetCol(id).map(|column| {
                (
                    &column.Histogram,
                    column.CMSketch.as_ref(),
                    column.TopN.as_ref(),
                    column.FMSketch.as_ref(),
                )
            })
        }
    }

    /// 是否具备自动分析资格：达到自动分析行数阈值且不是伪统计。
    pub fn IsEligibleForAnalysis(&self) -> bool {
        self.MeetAutoAnalyzeMinCnt() && !self.HistColl.Pseudo
    }

    /// 实时行数是否达到自动 ANALYZE 最小阈值。
    pub fn MeetAutoAnalyzeMinCnt(&self) -> bool {
        self.HistColl.RealtimeCount >= EffectiveAutoAnalyzeMinCnt()
    }

    /// 计算统计健康度（0–100）及是否已分析；伪表返回 (0, false)。
    pub fn GetStatsHealthy(&self) -> (i64, bool) {
        if self.HistColl.Pseudo {
            return (0, false);
        }
        if !self.IsAnalyzed() {
            return (0, true);
        }
        let analyzed = self.HistColl.GetAnalyzeRowCount();
        let count = if analyzed > 0.0 {
            analyzed
        } else {
            self.HistColl.RealtimeCount as f64
        };
        let healthy = if (self.HistColl.ModifyCount as f64) < count {
            ((1.0 - self.HistColl.ModifyCount as f64 / count) * 100.0) as i64
        } else if self.HistColl.ModifyCount == 0 {
            100
        } else {
            0
        };
        (healthy, true)
    }

    /// 判断列是否需要加载（含全量加载需求）及是否已分析。
    pub fn ColumnIsLoadNeeded(&self, id: i64, full_load: bool) -> (Option<&Column>, bool, bool) {
        if self.HistColl.Pseudo {
            return (None, false, false);
        }
        let column = self.GetCol(id);
        let analyzed = self.ColAndIdxExistenceMap.HasAnalyzed(id, false);
        let Some(column) = column else {
            return (None, self.ColAndIdxExistenceMap.Has(id, false), analyzed);
        };
        if !analyzed {
            return (None, false, false);
        }
        let needed = if full_load {
            !column.IsFullLoad()
        } else {
            !column.IsStatsInitialized()
        };
        (Some(column), needed, true)
    }

    /// 判断索引是否需要加载。
    pub fn IndexIsLoadNeeded(&self, id: i64) -> (Option<&Index>, bool) {
        let index = self.GetIdx(id);
        let needed = match index {
            None => self.ColAndIdxExistenceMap.HasAnalyzed(id, true),
            Some(index) => index.IsAnalyzed() && !index.IsFullLoad(),
        };
        (index, needed)
    }

    /// 任一列或索引统计是否已初始化。
    pub fn IsInitialized(&self) -> bool {
        self.HistColl
            .Columns
            .values()
            .any(|column| column.IsStatsInitialized())
            || self
                .HistColl
                .Indices
                .values()
                .any(|index| index.IsStatsInitialized())
    }

    /// 委托 HistColl 判断是否过期。
    pub fn IsOutdated(&self) -> bool {
        self.HistColl.IsOutdated()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 表级内存占用汇总：总量及按列/索引 ID 分解。
pub struct TableMemoryUsage {
    pub TotalMemUsage: i64,
    pub Columns: HashMap<i64, i64>,
    pub Indices: HashMap<i64, i64>,
}

impl TableMemoryUsage {
    /// 索引侧跟踪内存合计。
    pub fn TotalIdxTrackingMemUsage(&self) -> i64 {
        self.Indices.values().sum()
    }

    /// 列侧跟踪内存合计。
    pub fn TotalColTrackingMemUsage(&self) -> i64 {
        self.Columns.values().sum()
    }

    /// 列+索引跟踪内存合计。
    pub fn TotalTrackingMemUsage(&self) -> i64 {
        self.TotalColTrackingMemUsage() + self.TotalIdxTrackingMemUsage()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 单列内存占用明细（直方图、CM/TopN/FM Sketch）。
pub struct ColumnMemUsage {
    pub ColumnID: i64,
    pub TotalMemUsage: i64,
    pub HistogramMemUsage: i64,
    pub CMSketchMemUsage: i64,
    pub TopNMemUsage: i64,
    pub FMSketchMemUsage: i64,
}

impl ColumnMemUsage {
    /// 返回总内存占用。
    pub fn TotalMemoryUsage(&self) -> i64 {
        self.TotalMemUsage
    }
    /// 返回列 ID。
    pub fn ItemID(&self) -> i64 {
        self.ColumnID
    }
    /// 返回跟踪用内存占用。
    pub fn TrackingMemUsage(&self) -> i64 {
        self.CMSketchMemUsage + self.TopNMemUsage + self.HistogramMemUsage
    }
    /// 直方图内存占用。
    pub fn HistMemUsage(&self) -> i64 {
        self.HistogramMemUsage
    }
    /// TopN 内存占用。
    pub fn TopnMemUsage(&self) -> i64 {
        self.TopNMemUsage
    }
    /// CMSketch 内存占用。
    pub fn CMSMemUsage(&self) -> i64 {
        self.CMSketchMemUsage
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 单索引内存占用明细。
pub struct IndexMemUsage {
    pub IndexID: i64,
    pub TotalMemUsage: i64,
    pub HistogramMemUsage: i64,
    pub CMSketchMemUsage: i64,
    pub TopNMemUsage: i64,
    pub FMSketchMemUsage: i64,
}

impl IndexMemUsage {
    /// 返回总内存占用。
    pub fn TotalMemoryUsage(&self) -> i64 {
        self.TotalMemUsage
    }
    /// 返回索引 ID。
    pub fn ItemID(&self) -> i64 {
        self.IndexID
    }
    /// 返回跟踪用内存占用。
    pub fn TrackingMemUsage(&self) -> i64 {
        self.CMSketchMemUsage + self.TopNMemUsage + self.HistogramMemUsage
    }
    /// 直方图内存占用。
    pub fn HistMemUsage(&self) -> i64 {
        self.HistogramMemUsage
    }
    /// TopN 内存占用。
    pub fn TopnMemUsage(&self) -> i64 {
        self.TopNMemUsage
    }
    /// CMSketch 内存占用。
    pub fn CMSMemUsage(&self) -> i64 {
        self.CMSketchMemUsage
    }
}

/// 构造伪直方图集合：固定 PseudoRowCount，可配置是否禁止触发加载。
pub fn PseudoHistColl(physical_id: i64, allow_trigger_loading: bool) -> HistColl {
    let mut result = *NewHistColl(physical_id, PseudoRowCount, 0, 0, 0);
    result.Pseudo = true;
    result.CanNotTriggerLoad = !allow_trigger_loading;
    result
}

/// 构造伪表统计（允许触发加载）。
pub fn PseudoTable(physical_id: i64) -> Table {
    Table {
        ColAndIdxExistenceMap: NewColAndIndexExistenceMapWithoutSize(),
        HistColl: PseudoHistColl(physical_id, true),
        Version: PseudoVersion,
        LastAnalyzeVersion: 0,
        LastStatsHistVersion: 0,
        TblInfoUpdateTS: 0,
        IsPkIsHandle: false,
    }
}

/// 检查现有已分析统计是否需要改写为请求版本；伪统计或未分析统计无需改写。
pub fn AnalyzeVersionMatchesForTableStats(table: Option<&Table>, requested_version: i32) -> bool {
    assert_eq!(
        requested_version, Version2,
        "requested analyze version should be 2"
    );
    let Some(table) = table else {
        return true;
    };
    if table.HistColl.Pseudo {
        return true;
    }
    !IsAnalyzed(table.HistColl.StatsVer) || table.HistColl.StatsVer == requested_version
}

/// 伪统计对应的全量加载状态。
pub fn FullLoadStatusForPseudo() -> StatsLoadedStatus {
    NewStatsFullLoadStatus()
}
