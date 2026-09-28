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

// 索引级统计：直方图 + 可选 CMSketch/TopN/FMSketch 及加载/驱逐状态。
//
// 优化器用其估计索引谓词命中行数；Version2 起总行数需叠加 TopN。

// 索引级统计：直方图 + 可选 CMSketch/TopN/FMSketch 及加载/驱逐状态。
//
// 优化器用其估计索引谓词命中行数；Version2 起总行数需叠加 TopN。

use crate::{
    AllEvicted, CMSketch, FMSketch, Histogram, IsAnalyzed, StatsLoadedStatus, TopN, Version2,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 索引列元信息：名称与前缀长度。
pub struct IndexColumnInfo {
    pub Name: String,
    pub Length: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 索引定义：ID、列、是否多值/唯一及条件表达式。
pub struct IndexInfo {
    pub ID: i64,
    pub Name: String,
    pub Columns: Vec<IndexColumnInfo>,
    pub MVIndex: bool,
    pub Unique: bool,
    pub ConditionExprString: String,
}

#[derive(Clone, Debug)]
/// 单索引统计集合；`Deref` 到内嵌直方图。
pub struct Index {
    pub CMSketch: Option<CMSketch>,
    pub TopN: Option<TopN>,
    pub FMSketch: Option<FMSketch>,
    pub Info: Option<IndexInfo>,
    pub Histogram: Histogram,
    pub StatsLoadedStatus: StatsLoadedStatus,
    pub PhysicalID: i64,
    pub StatsVer: i64,
}

/// 解引用到直方图。
impl std::ops::Deref for Index {
    type Target = Histogram;

    fn deref(&self) -> &Self::Target {
        &self.Histogram
    }
}

/// 可变解引用到直方图。
impl std::ops::DerefMut for Index {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.Histogram
    }
}

impl Index {
    /// 返回已初始化的 IndexInfo。
    pub fn InfoRef(&self) -> &IndexInfo {
        self.Info
            .as_ref()
            .expect("initialized index statistics require IndexInfo")
    }

    /// 深拷贝。
    pub fn Copy(&self) -> Index {
        self.clone()
    }

    /// 统计项 ID（优先 Info.ID）。
    pub fn ItemID(&self) -> i64 {
        self.Info.as_ref().map_or(self.Histogram.ID, |info| info.ID)
    }

    /// 直方图等详细数据是否已全部驱逐。
    pub fn IsAllEvicted(&self) -> bool {
        self.StatsLoadedStatus.IsAllEvicted()
    }

    /// 驱逐状态码。
    pub fn GetEvictedStatus(&self) -> i32 {
        self.StatsLoadedStatus.evictedStatus
    }

    /// 丢弃可重建的详细数据（TopN/边界等），标记 AllEvicted。
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

    /// 统计版本。
    pub fn GetStatsVer(&self) -> i64 {
        self.StatsVer
    }

    /// 是否持有 CMSketch。
    pub fn IsCMSExist(&self) -> bool {
        self.CMSketch.is_some()
    }

    /// 是否需要重新加载（已驱逐或部分缺失）。
    pub fn IsEvicted(&self) -> bool {
        self.StatsLoadedStatus.IsLoadNeeded()
    }

    /// 以字符串展示直方图（按索引列格式）。
    pub fn String(&self) -> String {
        self.Histogram.ToString(self.InfoRef().Columns.len())
    }

    /// 总行数估计；Version2 起叠加 TopN。
    pub fn TotalRowCount(&self) -> f64 {
        if self.StatsVer >= Version2 as i64 {
            self.Histogram.TotalRowCount()
                + self
                    .TopN
                    .as_ref()
                    .map_or(0.0, |top_n| top_n.TotalCount() as f64)
        } else {
            self.Histogram.TotalRowCount()
        }
    }

    /// 清空草图与直方图内容并标记全部驱逐。
    pub fn EvictAllStats(&mut self) {
        self.CMSketch = None;
        self.TopN = None;
        self.Histogram.Buckets.clear();
        self.StatsLoadedStatus.evictedStatus = AllEvicted;
    }

    /// 直方图与各草图内存占用之和。
    pub fn MemoryUsage(&self) -> i64 {
        self.Histogram.MemoryUsage()
            + self.CMSketch.as_ref().map_or(0, CMSketch::MemoryUsage)
            + self.TopN.as_ref().map_or(0, TopN::MemoryUsage)
    }

    /// 查询编码键频次：TopN → CMSketch → 直方图等值估计。
    pub fn QueryBytes(&self, data: &[u8]) -> u64 {
        if let Some(top_n) = self.TopN.as_ref() {
            let (count, found) = top_n.QueryTopN(data);
            if found {
                return count;
            }
        }
        if let Some(cmsketch) = self.CMSketch.as_ref() {
            return cmsketch.QueryBytes(data);
        }
        self.Histogram
            .EqualRowCount(
                &types::NewBytesDatum(data.to_vec()),
                self.StatsVer >= Version2 as i64,
            )
            .0 as u64
    }

    /// 实时行数相对统计总行数的放大系数。
    pub fn GetIncreaseFactor(&self, realtime_row_count: i64) -> f64 {
        let count = self.TotalRowCount();
        if count == 0.0 {
            1.0
        } else {
            realtime_row_count as f64 / count
        }
    }

    /// 返回内嵌直方图引用。
    pub fn GetHistogram(&self) -> &Histogram {
        &self.Histogram
    }

    /// 返回 TopN（若有）。
    pub fn GetTopN(&self) -> Option<&TopN> {
        self.TopN.as_ref()
    }

    /// 是否已经过 ANALYZE（版本非 Version0）。
    pub fn IsAnalyzed(&self) -> bool {
        IsAnalyzed(self.StatsVer as i32)
    }

    /// 统计是否已初始化。
    pub fn IsStatsInitialized(&self) -> bool {
        self.StatsLoadedStatus.IsStatsInitialized()
    }

    /// 必要统计是否已加载。
    pub fn IsEssentialStatsLoaded(&self) -> bool {
        self.StatsLoadedStatus.IsEssentialStatsLoaded()
    }

    /// 是否全量加载。
    pub fn IsFullLoad(&self) -> bool {
        self.StatsLoadedStatus.IsFullLoad()
    }
}

/// 索引统计是否不可用：缺失、伪统计或行数为 0。
pub fn IndexStatsIsInvalid(index: Option<&Index>, pseudo: bool) -> bool {
    let Some(index) = index else {
        return true;
    };
    pseudo || index.TotalRowCount() == 0.0
}
