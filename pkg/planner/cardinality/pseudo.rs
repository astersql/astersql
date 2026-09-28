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

// 伪统计（pseudo statistics）基数估算。
//
// 当列/索引直方图、TopN 等真实统计缺失或无效时，优化器（选择执行计划的组件）
// 用固定比率估算选择率与行数：等值约 1/1000、比较约 1/3、区间约 1/40，
// 并在唯一键或主键 handle（行标识）场景进一步收紧结果。

use crate::*;

// 统计信息缺失时的伪基数估算规则。
// expression、statistics、types 与 ranger 等名称保留为后续跨文件接线的外部依赖。

/// 等值/IN 谓词的伪分母：选择率约 1/1000。
pub(crate) const pseudoEqualRate: f64 = 1000.0;
/// 开区间比较（<、>、<=、>=）的伪分母：选择率约 1/3。
const pseudoLessRate: f64 = 3.0;
/// BETWEEN 式闭区间的伪分母：选择率约 1/40。
const pseudoBetweenRate: f64 = 40.0;

/// PseudoAvgCountPerValue 对应 Go 的同名导出函数：直方图缺失时，以实时行数除以默认 NDV 比率。
pub fn PseudoAvgCountPerValue(t: &statistics::Table) -> f64 {
    t.RealtimeCount as f64 / pseudoEqualRate
}

/// pseudoColumnHasUniqueKey 保留 Go `HasUniKeyFlag` 与主键标志的组合语义。
pub(crate) fn pseudoColumnHasUniqueKey(info: &statistics::ColumnInfo) -> bool {
    info.IsPrimaryKey || mysql::HasUniKeyFlag(info.FieldType.GetFlag())
}

/// pseudoSelectivity 遍历谓词，选择最严格的伪选择率，并利用单列或复合唯一键进一步收紧结果。
pub(crate) fn pseudoSelectivity(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    exprs: &[expression::ExprBox],
) -> f64 {
    let mut minFactor = sctx.GetSessionVars().SelectivityFactor;
    let mut colExists = std::collections::HashSet::<String>::new();
    for expr in exprs {
        // Go 只处理 ScalarFunction；其他表达式没有可识别的列约束，直接略过。
        let Some(fun) = expr.as_scalar_function() else {
            continue;
        };
        let colID = getConstantColumnID(fun.GetArgs());
        if colID == unknownColumnID {
            continue;
        }
        // 即使这里只做伪估算，Go 仍会触发一次统计有效性检查/按需加载。
        statistics::ColumnStatsIsInvalid(None, coll.Pseudo);
        match fun.FuncName.L.as_str() {
            ast::EQ | ast::NullEQ | ast::In => {
                minFactor = minFactor.min(1.0 / pseudoEqualRate);
                let Some(col) = coll.GetCol(colID) else {
                    continue;
                };
                if let Some(info) = col.Info.as_ref() {
                    colExists.insert(info.Name.clone());
                    if pseudoColumnHasUniqueKey(info) {
                        return 1.0 / coll.RealtimeCount as f64;
                    }
                }
            }
            ast::GE | ast::GT | ast::LE | ast::LT => {
                minFactor = minFactor.min(1.0 / pseudoLessRate);
                // FIXME(Go): 尚未把成对上下界识别为 BETWEEN。
            }
            _ => {}
        }
    }
    if colExists.is_empty() {
        return minFactor;
    }

    // 对复合索引逐列确认谓词覆盖情况；首列命中时保留 Go 的统计加载副作用。
    let mut hasUniqueKey = false;
    coll.ForEachIndexImmutable(|_id, idx| {
        let mut unique = true;
        let mut firstMatch = false;
        let Some(info) = idx.Info.as_ref() else {
            return false;
        };
        for col in &info.Columns {
            if !colExists.contains(&col.Name) {
                unique = false;
                break;
            }
            firstMatch = true;
        }
        if firstMatch {
            statistics::IndexStatsIsInvalid(None, coll.Pseudo);
        }
        if info.Unique && unique {
            hasUniqueKey = true;
            return true;
        }
        false
    });
    if hasUniqueKey {
        1.0 / coll.RealtimeCount as f64
    } else {
        minFactor
    }
}

/// getPseudoRowCountBySignedIntRanges 估算有符号整型主键范围；点范围至多返回一行。
pub(crate) fn getPseudoRowCountBySignedIntRanges(
    intRanges: &[&ranger::Range],
    tableRowCount: f64,
) -> f64 {
    let mut rowCount = 0.0;
    for rg in intRanges {
        let mut low = rg.LowVal[0].GetInt64();
        if matches!(rg.LowVal[0].Kind(), types::KindNull | types::KindMinNotNull) {
            low = i64::MIN;
        }
        let mut high = rg.HighVal[0].GetInt64();
        if rg.HighVal[0].Kind() == types::KindMaxValue {
            high = i64::MAX;
        }
        let mut cnt = if low == i64::MIN && high == i64::MAX {
            tableRowCount
        } else if low == i64::MIN || high == i64::MAX {
            tableRowCount / pseudoLessRate
        } else if low == high {
            1.0 // 主键作为 handle 时，等值范围至多命中一行。
        } else {
            tableRowCount / pseudoBetweenRate
        };
        // 用整数域宽度限制伪估算；wrapping_sub 保留 Go 溢出后不进入分支的效果。
        let width = high.wrapping_sub(low);
        if width > 0 && cnt > width as f64 {
            cnt = width as f64;
        }
        rowCount += cnt;
    }
    rowCount.min(tableRowCount)
}

/// getPseudoRowCountByUnsignedIntRanges 与有符号版本相同，但最小边界映射为 0。
pub(crate) fn getPseudoRowCountByUnsignedIntRanges(
    intRanges: &[&ranger::Range],
    tableRowCount: f64,
) -> f64 {
    let mut rowCount = 0.0;
    for rg in intRanges {
        let mut low = rg.LowVal[0].GetUint64();
        if matches!(rg.LowVal[0].Kind(), types::KindNull | types::KindMinNotNull) {
            low = 0;
        }
        let mut high = rg.HighVal[0].GetUint64();
        if rg.HighVal[0].Kind() == types::KindMaxValue {
            high = u64::MAX;
        }
        let mut cnt = if low == 0 && high == u64::MAX {
            tableRowCount
        } else if low == 0 || high == u64::MAX {
            tableRowCount / pseudoLessRate
        } else if low == high {
            1.0
        } else {
            tableRowCount / pseudoBetweenRate
        };
        if high > low && cnt > (high - low) as f64 {
            cnt = (high - low) as f64;
        }
        rowCount += cnt;
    }
    rowCount.min(tableRowCount)
}

/// getPseudoRowCountByIndexRanges 按等值前缀和首个非等值列组合索引选择率。
pub(crate) fn getPseudoRowCountByIndexRanges(
    tc: &types::Context,
    indexRanges: &[&ranger::Range],
    tableRowCount: f64,
    colsLen: usize,
) -> Result<f64, errors::Error> {
    if tableRowCount == 0.0 {
        return Ok(0.0);
    }
    let mut totalCount = 0.0;
    for indexRange in indexRanges {
        let mut count = tableRowCount;
        let (mut i, err) = indexRange.PrefixEqualLen(tc.clone());
        if let Some(err) = err {
            return Err(errors::Trace(err));
        }
        if i == colsLen && !indexRange.LowExclude && !indexRange.HighExclude {
            totalCount += 1.0;
            continue;
        }
        if i >= indexRange.LowVal.len() {
            i = indexRange.LowVal.len() - 1;
        }
        let rowCount = getPseudoRowCountByColumnRanges(tc, tableRowCount, &[*indexRange], i)?;
        count = count / tableRowCount * rowCount;
        // 每个完整等值前缀只过滤 1/100，避免多列索引估算过快坍缩。
        for _ in 0..i {
            count /= 100.0;
        }
        totalCount += count;
    }
    if totalCount > tableRowCount {
        totalCount = tableRowCount / 3.0;
    }
    Ok(totalCount)
}

/// getPseudoRowCountByColumnRanges 在列统计缺失时，根据空值、无穷边界、点和区间采用固定比率。
pub(crate) fn getPseudoRowCountByColumnRanges(
    tc: &types::Context,
    tableRowCount: f64,
    columnRanges: &[&ranger::Range],
    colIdx: usize,
) -> Result<f64, errors::Error> {
    let mut rowCount = 0.0;
    for ran in columnRanges {
        let lowKind = ran.LowVal[colIdx].Kind();
        let highKind = ran.HighVal[colIdx].Kind();
        if lowKind == types::KindNull && highKind == types::KindMaxValue {
            rowCount += tableRowCount;
        } else if lowKind == types::KindMinNotNull {
            let nullCount = tableRowCount / pseudoEqualRate;
            rowCount += if highKind == types::KindMaxValue {
                tableRowCount - nullCount
            } else {
                tableRowCount / pseudoLessRate - nullCount
            };
        } else if highKind == types::KindMaxValue {
            rowCount += tableRowCount / pseudoLessRate;
        } else {
            // Datum 比较可能因类型或排序规则失败，沿用 Go 的 errors.Trace 立即返回。
            let compare = ran.LowVal[colIdx]
                .Compare(
                    tc.clone(),
                    &ran.HighVal[colIdx],
                    ran.Collators[colIdx].as_ref(),
                )
                .map_err(errors::Trace)?;
            rowCount += if compare == 0 {
                tableRowCount / pseudoEqualRate
            } else {
                tableRowCount / pseudoBetweenRate
            };
        }
    }
    Ok(rowCount.min(tableRowCount))
}
