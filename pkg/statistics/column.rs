// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 列级统计信息结构：直方图、CMSketch、TopN、FMSketch 及其加载状态。
//
// 优化器（根据统计估计代价、选择执行计划）用这些结构估计谓词选择率；
// `StatsLoadedStatus` 标记直方图是否已从存储完整加载或已被驱逐。

use crate::{
    AllEvicted, CMSketch, FMSketch, Histogram, IsAnalyzed, IsColumnAnalyzedOrSynthesized,
    NewHistogram, StatsLoadedStatus, TopN, Version2,
};

#[derive(Clone, Debug)]
/// 列的元信息：列 ID、名称、字段类型，以及是否主键。
pub struct ColumnInfo {
    pub ID: i64,
    pub Name: String,
    pub FieldType: types::FieldType,
    pub IsPrimaryKey: bool,
}

#[derive(Clone, Debug)]
/// 单列统计：直方图为主，可选附带 CMSketch/TopN/FMSketch，并记录加载状态与版本。
pub struct Column {
    pub CMSketch: Option<CMSketch>,
    pub TopN: Option<TopN>,
    pub FMSketch: Option<FMSketch>,
    pub Info: Option<ColumnInfo>,
    pub Histogram: Histogram,
    pub StatsLoadedStatus: StatsLoadedStatus,
    pub PhysicalID: i64,
    pub StatsVer: i64,
    pub IsHandle: bool,
}

/// 解引用到内嵌直方图，便于直接调用 `Histogram` 方法。
impl std::ops::Deref for Column {
    type Target = Histogram;

    fn deref(&self) -> &Self::Target {
        &self.Histogram
    }
}

/// 可变解引用到内嵌直方图。
impl std::ops::DerefMut for Column {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.Histogram
    }
}

impl Column {
    /// 深拷贝整列统计（对应 Go 的 `Copy`）。
    pub fn Copy(&self) -> Column {
        self.clone()
    }

    /// 以字符串形式展示直方图内容。
    pub fn String(&self) -> String {
        self.Histogram.ToString(0)
    }

    /// 返回列总行数估计。
    ///
    /// 统计版本 ≥ Version2 时，直方图行数需加上 TopN 中单独维护的高频值行数。
    pub fn TotalRowCount(&self) -> f64 {
        if self.StatsVer >= Version2 as i64 {
            self.Histogram.TotalRowCount()
                + self
                    .TopN
                    .as_ref()
                    .map_or(0.0, |value| value.TotalCount() as f64)
        } else {
            self.Histogram.TotalRowCount()
        }
    }

    pub fn NotNullCount(&self) -> f64 {
        // 返回非空行数估计；Version2 起同样叠加 TopN 计数。
        if self.StatsVer >= Version2 as i64 {
            self.Histogram.NotNullCount()
                + self
                    .TopN
                    .as_ref()
                    .map_or(0.0, |value| value.TotalCount() as f64)
        } else {
            self.Histogram.NotNullCount()
        }
    }

    pub fn GetIncreaseFactor(&self, realtime_row_count: i64) -> f64 {
        // 根据实时行数相对统计行数的比值，得到行数增长放大因子。
        let count = self.TotalRowCount();
        if count == 0.0 {
            1.0
        } else {
            realtime_row_count as f64 / count
        }
    }

    pub fn MemoryUsage(&self) -> i64 {
        // 估算本列统计结构占用的内存字节数。
        self.Histogram.MemoryUsage()
            + self.CMSketch.as_ref().map_or(0, CMSketch::MemoryUsage)
            + self.TopN.as_ref().map_or(0, TopN::MemoryUsage)
            + self.FMSketch.as_ref().map_or(0, FMSketch::MemoryUsage)
    }

    pub fn ItemID(&self) -> i64 {
        // 返回列项 ID：优先取 Info.ID，否则回退到直方图 ID。
        self.Info
            .as_ref()
            .map_or(self.Histogram.ID, |value| value.ID)
    }

    /// 驱逐非必要载荷：清空桶边界等，标记为 AllEvicted。
    ///
    /// Version1 还会丢弃 CMSketch；TopN 与直方图桶一律清空以释放内存。
    pub fn DropUnnecessaryData(&mut self) {
        if self.StatsVer < Version2 as i64 {
            self.CMSketch = None;
        }
        self.TopN = None;
        self.Histogram.Bounds.clear();
        self.Histogram.Buckets.clear();
        self.Histogram.Scalars.clear();
        self.StatsLoadedStatus.evictedStatus = AllEvicted;
    }

    /// 判断统计是否处于全部驱逐状态。
    pub fn IsAllEvicted(&self) -> bool {
        self.StatsLoadedStatus.IsAllEvicted()
    }

    /// 返回当前驱逐状态码。
    pub fn GetEvictedStatus(&self) -> i32 {
        self.StatsLoadedStatus.evictedStatus
    }

    /// 判断统计是否已完成初始化。
    pub fn IsStatsInitialized(&self) -> bool {
        self.StatsLoadedStatus.IsStatsInitialized()
    }

    /// 判断是否仍需从存储加载统计。
    pub fn IsLoadNeeded(&self) -> bool {
        self.StatsLoadedStatus.IsLoadNeeded()
    }

    /// 判断必要统计（如直方图核心部分）是否已加载。
    pub fn IsEssentialStatsLoaded(&self) -> bool {
        self.StatsLoadedStatus.IsEssentialStatsLoaded()
    }

    /// 判断是否已完整加载。
    pub fn IsFullLoad(&self) -> bool {
        self.StatsLoadedStatus.IsFullLoad()
    }

    /// 返回统计版本号。
    pub fn GetStatsVer(&self) -> i64 {
        self.StatsVer
    }

    /// 判断是否存在 CMSketch。
    pub fn IsCMSExist(&self) -> bool {
        self.CMSketch.is_some()
    }

    /// 判断该列是否经过 ANALYZE（基于 StatsVer）。
    pub fn IsAnalyzed(&self) -> bool {
        IsAnalyzed(self.StatsVer as i32)
    }

    /// 判断列统计是否可用（已分析或可合成）。
    pub fn StatsAvailable(&self) -> bool {
        IsColumnAnalyzedOrSynthesized(
            self.StatsVer as i32,
            self.Histogram.NDV,
            self.Histogram.NullCount,
        )
    }

    /// 取得直方图引用。
    pub fn GetHistogram(&self) -> &Histogram {
        &self.Histogram
    }

    /// 取得 TopN 引用（若存在）。
    pub fn GetTopN(&self) -> Option<&TopN> {
        self.TopN.as_ref()
    }
}

/// 判断列统计是否无效：缺失、伪统计、零行，或 NDV>0 但必要统计未加载。
pub fn ColumnStatsIsInvalid(column: Option<&Column>, pseudo: bool) -> bool {
    let Some(column) = column else {
        return true;
    };
    pseudo
        || column.TotalRowCount() == 0.0
        || (!column.IsEssentialStatsLoaded() && column.Histogram.NDV > 0)
}

/// 构造空的列统计骨架，供尚无 ANALYZE 结果时占位。
pub fn EmptyColumn(physical_id: i64, primary_key_is_handle: bool, info: ColumnInfo) -> Column {
    Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Histogram: NewHistogram(info.ID, 0, 0, 0, &info.FieldType, 0, 0),
        IsHandle: primary_key_is_handle && info.IsPrimaryKey,
        PhysicalID: physical_id,
        Info: Some(info),
        StatsLoadedStatus: StatsLoadedStatus::default(),
        StatsVer: 0,
    }
}
