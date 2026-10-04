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

// 列组 NDV（Number of Distinct Values，不同值个数）估算。
//
// 提供单列 NDV、多列保守最大值、指数退避组合，以及按会话风险参数在
// 均匀/偏斜模型间混合的缩放逻辑；结果供 Join 与选择率等下游除法使用，
// 故常将 NDV 钳制为至少 1。

use crate::*;

// expression、property、statistics、variable 由 crate 根重导出；指数退避实现来自相邻模块。

// init 对应 Go 包初始化钩子，把 property 层的 NDV 缩放回调指向本文件实现。
// Rust 没有同形状的包 init，这里保留显式函数供模块初始化阶段调用。
/// 注册 property 层 NDV 缩放回调（对应 Go 包 init）。
pub fn init() {
    property::SetScaleNDVFunc(Some(|vars, ndv, rows, selected| {
        ScaleNDV(Some(vars), ndv, rows, selected)
    }));
}

// distinctFactor 是缺少可用直方图时，用实时行数推导 NDV 的默认比例。
const distinctFactor: f64 = 0.8;

// EstimateColumnNDV 使用原始 DataSource 直方图估算指定列 NDV。
/// 使用 DataSource 直方图估算指定列的 NDV（不同值个数）。
pub fn EstimateColumnNDV(tbl: &statistics::Table, colID: i64) -> f64 {
    let hist = tbl.GetCol(colID);
    if let Some(hist) = hist {
        if hist.IsStatsInitialized() {
            let mut ndv = hist.Histogram.NDV as f64;
            // 用同一分析版本的总行数把分析时 NDV 缩放到当前实时行数。
            let analyzeCount = getTotalRowCount(tbl, hist);
            if analyzeCount > 0 {
                ndv *= tbl.RealtimeCount as f64 / analyzeCount as f64;
            }
            return ndv;
        }
    }
    // 无已初始化统计时采用伪估算，避免返回无法用于后续除法的空值。
    tbl.RealtimeCount as f64 * distinctFactor
}

// getTotalRowCount 返回收集目标列直方图时对应的全表行数。
fn getTotalRowCount(statsTbl: &statistics::Table, colHist: &statistics::Column) -> i64 {
    if colHist.IsFullLoad() {
        return colHist.TotalRowCount() as i64;
    }

    // 目标列未完整加载时，依次寻找同 LastUpdateVersion 的完整索引或列统计作为替代来源。
    let mut total = None;
    statsTbl.ForEachIndexImmutable(|_, index| {
        if index.IsFullLoad() && index.LastUpdateVersion == colHist.LastUpdateVersion {
            total = Some(index.TotalRowCount() as i64);
            return true;
        }
        false
    });
    if let Some(total) = total {
        return total;
    }

    statsTbl.ForEachColumnImmutable(|_, column| {
        if column.IsFullLoad() && column.LastUpdateVersion == colHist.LastUpdateVersion {
            total = Some(column.TotalRowCount() as i64);
            return true;
        }
        false
    });
    total.unwrap_or(0)
}

// EstimateColsNDVWithMatchedLen 返回列组 NDV，以及命中的 GroupNDV 所覆盖的列数。
// 该函数主要服务 join：精确组统计优先，其次是保守最大值，并可按会话风险参数混入指数退避结果。
/// 返回列组 NDV 与命中 GroupNDV 覆盖的列数；供 Join 等使用。
pub fn EstimateColsNDVWithMatchedLen(
    sctx: Option<&dyn planctx::PlanContext>,
    cols: &[expression::Column],
    schema: &expression::Schema,
    profile: &property::StatsInfo,
) -> (f64, usize) {
    EstimateColsNDVWithSessionVars(
        sctx.map(|context| context.GetSessionVars()),
        cols,
        schema,
        profile,
    )
}

/// Reuse the full NDV estimate with object-safe planner contexts exposing SessionVars.
pub fn EstimateColsNDVWithSessionVars(
    vars: Option<&variable::SessionVars>,
    cols: &[expression::Column],
    schema: &expression::Schema,
    profile: &property::StatsInfo,
) -> (f64, usize) {
    if cols.is_empty() {
        // 空连接键按一个 distinct group 处理，matched length 沿用 Go 返回 1。
        return (1.0, 1);
    }

    if let Some(groupNDV) = profile.GetGroupNDV4Cols(cols) {
        // 精确 GroupNDV 至少钳制为 1，避免下游用它作除数时出现零。
        return (groupNDV.NDV.max(1.0), groupNDV.Cols.len());
    }

    let conservativeNDV = estimateNaiveNDV(cols, schema, profile);
    if cols.len() == 1 {
        // 单列的保守估算与指数退避相同，无需重复收集和排序。
        return (conservativeNDV, 1);
    }

    let exponentialNDV = estimateNDVWithExponentialBackoff(cols, schema, profile);
    if let Some(vars) = vars {
        let skewRatio = vars.RiskGroupNDVSkewRatio;
        vars.RecordRelevantOptVar(vardef::TiDBOptRiskGroupNDVSkewRatio);
        return estimateColsNDVBySkewRatio(conservativeNDV, exponentialNDV, skewRatio);
    }
    (conservativeNDV, 1)
}

/// 按偏斜风险比混合保守 NDV 与指数退避 NDV 的算术核心。
/// 独立出来便于单测覆盖全部 risk ratio，而无需完整 PlanContext。
/// Shared arithmetic core for the session-variable and unit-test paths.
/// Keeping this separate lets tests exercise all risk ratios without needing a
/// storage-backed implementation of the large `PlanContext` interface.
pub(crate) fn estimateColsNDVBySkewRatio(
    conservativeNDV: f64,
    exponentialNDV: f64,
    skewRatio: f64,
) -> (f64, usize) {
    if skewRatio > 0.0 {
        return (
            calculateGroupNDVWithSkewRatio(conservativeNDV, exponentialNDV, skewRatio),
            1,
        );
    }
    (conservativeNDV, 1)
}

// estimateNaiveNDV 实现原始保守策略：取所有可匹配列 NDV 的最大值。
fn estimateNaiveNDV(
    cols: &[expression::Column],
    schema: &expression::Schema,
    profile: &property::StatsInfo,
) -> f64 {
    if cols.is_empty() {
        return 1.0;
    }
    let Some(indices) = schema.ColumnsIndices(cols) else {
        return 1.0;
    };
    let mut maxNDV = 1.0_f64;
    for index in indices {
        let column = &schema.Columns[index];
        if let Some(colNDV) = profile.ColNDVs.get(&column.UniqueID) {
            if *colNDV > 0.0 {
                maxNDV = maxNDV.max(*colNDV);
            }
        }
    }
    maxNDV
}

// estimateNDVWithExponentialBackoff 收集单列 NDV，降序排列后应用指数退避。
fn estimateNDVWithExponentialBackoff(
    cols: &[expression::Column],
    schema: &expression::Schema,
    profile: &property::StatsInfo,
) -> f64 {
    let defaultNDV = 1.0;
    if cols.is_empty() {
        return defaultNDV;
    }

    let Some(indices) = schema.ColumnsIndices(cols) else {
        // Go 会记录 schema/columns 诊断日志；不执行外部日志 IO，只保留安全回退值。
        return defaultNDV;
    };
    let mut singleColumnNDVs = Vec::with_capacity(cols.len());
    for index in indices {
        let column = &schema.Columns[index];
        if let Some(colNDV) = profile.ColNDVs.get(&column.UniqueID) {
            if *colNDV > 0.0 {
                singleColumnNDVs.push(*colNDV);
            }
        }
    }
    if singleColumnNDVs.is_empty() {
        return defaultNDV;
    }

    // 最大 NDV 放在首项以获得权重 1，其余列按 1/2、1/4、1/8 逐步衰减。
    singleColumnNDVs.sort_by(|left, right| right.total_cmp(left));
    let lowerBound = singleColumnNDVs[0].max(defaultNDV);
    let upperBound = profile.RowCount;
    if upperBound <= lowerBound {
        // 行数统计不准确或过小时，回退到至少不小于最大单列 NDV 的保守值。
        return lowerBound;
    }
    ApplyExponentialBackoff(&singleColumnNDVs, lowerBound, upperBound)
}

// calculateGroupNDVWithSkewRatio 在保守与指数估算之间做线性插值。
// 0 表示完全保守，1 表示完全信任指数退避，中间值表达相应风险偏好。
fn calculateGroupNDVWithSkewRatio(
    conservativeNDV: f64,
    exponentialNDV: f64,
    skewRatio: f64,
) -> f64 {
    conservativeNDV + (exponentialNDV - conservativeNDV) * skewRatio
}

// EstimateColsDNVWithMatchedLenFromUniqueIDs 与列对象入口相同，但先把 UniqueID 列表包装为临时 Column。
// 函数名中的 DNV 拼写保持 Go 公开 API，不在迁移中擅自更名。
/// 由 UniqueID 列表估算列组 NDV（函数名 DNV 拼写保持 Go API）。
pub fn EstimateColsDNVWithMatchedLenFromUniqueIDs(
    sctx: Option<&dyn planctx::PlanContext>,
    ids: &[i64],
    schema: &expression::Schema,
    profile: &property::StatsInfo,
) -> (f64, usize) {
    let mut cols = Vec::with_capacity(ids.len());
    for id in ids {
        cols.push(expression::Column::new(
            types::FieldType::default(),
            *id,
            *id,
            0,
        ));
    }
    EstimateColsNDVWithMatchedLen(sctx, &cols, schema, profile)
}

// ScaleNDV 根据行选择率缩放原始 NDV，并按风险参数混合均匀模型与偏斜模型。
/// 按行选择率缩放 NDV，并在均匀模型与偏斜模型间按风险参数混合。
pub fn ScaleNDV(
    vars: Option<&variable::SessionVars>,
    originalNDV: f64,
    originalRows: f64,
    selectedRows: f64,
) -> f64 {
    let skewRatio = vars
        .map(|vars| vars.RiskScaleNDVSkewRatio)
        .unwrap_or(vardef::DefOptRiskScaleNDVSkewRatio);
    let uniformNDV = estimateUniformNDV(originalNDV, originalRows, selectedRows);
    let skewedNDV = estimateSkewedNDV(originalNDV, originalRows, selectedRows);
    skewedNDV * skewRatio + uniformNDV * (1.0 - skewRatio)
}

// estimateUniformNDV 基于“每个值出现次数相同、每行被选概率相同”两个假设缩放 NDV。
fn estimateUniformNDV(originalNDV: f64, originalRows: f64, selectedRows: f64) -> f64 {
    if originalRows <= 0.0 || selectedRows <= 0.0 || originalNDV <= 0.0 {
        return 0.0;
    }
    if selectedRows >= originalRows {
        return originalNDV;
    }

    let selectivity = selectedRows / originalRows;
    // 均匀假设下每个 distinct value 平均出现 originalRows/originalNDV 次。
    let rowsPerValue = originalRows / originalNDV;
    let notSelectedPossPerRow = 1.0 - selectivity;
    let notSelectedPossPerValue = notSelectedPossPerRow.powf(rowsPerValue);
    let newNDV = originalNDV * (1.0 - notSelectedPossPerValue);

    // 只要选中行就至少有一个 distinct value，同时 NDV 不可能超过选中行数。
    newNDV.max(1.0).min(selectedRows)
}

// estimateSkewedNDV 采用最简单的线性选择率缩放，作为数据偏斜时的另一端估算。
fn estimateSkewedNDV(originalNDV: f64, originalRows: f64, selectedRows: f64) -> f64 {
    if originalRows <= 0.0 {
        return 0.0;
    }
    originalNDV * selectedRows / originalRows
}
