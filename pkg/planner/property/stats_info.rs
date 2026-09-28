// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 计划输出统计摘要（StatsInfo）：行数、列 NDV、直方图集合引用。
//
// NDV（Number of Distinct Values）是代价估算的关键输入；Scale 按选择率缩放
// 行数与 NDV，回调 `ScaleNDVFunc` 打破 cardinality ↔ property 循环依赖。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use crate::{expression, variable};

/// 按选择率缩放 NDV 的回调签名：(会话变量, 原 NDV, 原行数, 选中行数) → 新 NDV。
pub type ScaleNDVCallback = fn(&variable::SessionVars, f64, f64, f64) -> f64;

/// Injectable callback breaks the planner/cardinality -> property dependency cycle.
/// 可注入回调，打破 planner/cardinality → property 的循环依赖。
pub static ScaleNDVFunc: RwLock<Option<ScaleNDVCallback>> = RwLock::new(None);

/// 安装或清空 NDV 缩放回调。
pub fn SetScaleNDVFunc(callback: Option<ScaleNDVCallback>) {
    *ScaleNDVFunc
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = callback;
}

/// 一组列的联合 NDV（列 UniqueID 列表 + 估计不同值个数）。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupNDV {
    /// 列 UniqueID 列表（通常已排序以便查找匹配）。
    pub Cols: Vec<i64>,
    /// 该列组的不同值个数估计。
    pub NDV: f64,
}

/// 将 GroupNDV 切片格式化为可读字符串（主要用于测试）。
pub fn ToString(ndvs: &[GroupNDV]) -> String {
    let groups = ndvs
        .iter()
        .map(|group| {
            let cols = group
                .Cols
                .iter()
                .map(|column| column.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            format!("{{[{cols}] {}}}", group.NDV)
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{groups}]")
}

/// Until `pkg/statistics` becomes a crate, keep the real histogram object opaque and shared.
/// No histogram behavior is implemented here; callers retain and downcast their original object.
/// 在 statistics 独立成 crate 前，直方图集合以不透明共享对象形式持有；
/// 本模块不实现直方图逻辑，调用方自行 downcast。
pub type HistCollRef = Arc<dyn Any + Send + Sync>;

/// 计划节点输出的基础统计：行数、列 NDV、直方图引用与版本。
#[derive(Clone, Default)]
pub struct StatsInfo {
    /// 估计输出行数。
    pub RowCount: f64,
    /// 列 UniqueID → NDV。
    pub ColNDVs: HashMap<i64, f64>,
    /// 直方图集合的不透明引用（可选）。
    pub HistColl: Option<HistCollRef>,
    /// 统计版本；伪统计时常为 PseudoVersion。
    pub StatsVersion: u64,
    /// 列组 NDV 列表。
    pub GroupNDVs: Vec<GroupNDV>,
}

impl fmt::Debug for StatsInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StatsInfo")
            .field("RowCount", &self.RowCount)
            .field("ColNDVs", &self.ColNDVs)
            .field("HistColl", &self.HistColl.as_ref().map(|_| "HistColl"))
            .field("StatsVersion", &self.StatsVersion)
            .field("GroupNDVs", &self.GroupNDVs)
            .finish()
    }
}

impl fmt::Display for StatsInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 按列 ID 排序后输出，保证调试字符串稳定。
        let mut ndvs = self.ColNDVs.iter().collect::<Vec<_>>();
        ndvs.sort_unstable_by_key(|(id, _)| **id);
        let values = ndvs
            .into_iter()
            .map(|(id, ndv)| format!("{id}:{ndv}"))
            .collect::<Vec<_>>()
            .join(" ");
        write!(formatter, "count {}, ColNDVs map[{values}]", self.RowCount)
    }
}

impl StatsInfo {
    /// 返回 Display 字符串。
    pub fn String(&self) -> String {
        self.to_string()
    }

    /// 将行数截断为 i64（与 Go Count 对齐）。
    pub fn Count(&self) -> i64 {
        self.RowCount as i64
    }

    /// 按选择率 factor 缩放行数与各 NDV（经 ScaleNDVFunc）。
    pub fn Scale(&self, vars: &variable::SessionVars, factor: f64) -> StatsInfo {
        let callback = ScaleNDVFunc
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expect("property::ScaleNDVFunc must be installed before StatsInfo::Scale");
        let original_row_count = self.RowCount;
        let row_count = self.RowCount * factor;
        let ColNDVs = self
            .ColNDVs
            .iter()
            .map(|(&id, &ndv)| (id, callback(vars, ndv, original_row_count, row_count)))
            .collect();
        let GroupNDVs = self
            .GroupNDVs
            .iter()
            .map(|group| GroupNDV {
                Cols: group.Cols.clone(),
                NDV: callback(vars, group.NDV, original_row_count, row_count),
            })
            .collect();
        StatsInfo {
            RowCount: row_count,
            ColNDVs,
            HistColl: self.HistColl.clone(),
            StatsVersion: self.StatsVersion,
            GroupNDVs,
        }
    }

    /// 按期望行数缩放；若期望不小于当前行数或行数 ≤ 1 则原样克隆。
    pub fn ScaleByExpectCnt(&self, vars: &variable::SessionVars, expect_count: f64) -> StatsInfo {
        if expect_count >= self.RowCount || self.RowCount <= 1.0 {
            return self.clone();
        }
        self.Scale(vars, expect_count / self.RowCount)
    }

    /// 按列 UniqueID（排序后）查找匹配的 GroupNDV。
    pub fn GetGroupNDV4Cols(&self, columns: &[expression::Column]) -> Option<&GroupNDV> {
        if columns.is_empty() || self.GroupNDVs.is_empty() {
            return None;
        }
        let mut ids = columns
            .iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        self.GroupNDVs
            .iter()
            .find(|group| group.Cols.as_slice() == ids.as_slice())
    }
}

/// 由 Limit 推导子节点统计：行数取 min，列 NDV 截断到行数，清空 GroupNDVs。
pub fn DeriveLimitStats(child: &StatsInfo, limit_count: f64) -> StatsInfo {
    let row_count = limit_count.min(child.RowCount);
    StatsInfo {
        RowCount: row_count,
        ColNDVs: child
            .ColNDVs
            .iter()
            .map(|(&id, &ndv)| (id, ndv.min(row_count)))
            .collect(),
        HistColl: child.HistColl.clone(),
        StatsVersion: 0,
        GroupNDVs: Vec::new(),
    }
}
