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

// Join 输出行数（cardinality）估算。
//
// `EstimateFullJoinRowCount` 按左右剖面行数与连接键 NDV 估算 full join
// 结果规模；笛卡尔积直接相乘。启用 DP join reorder 时，对未被 GroupNDV
// 覆盖的剩余键乘以 0.9 相关性因子（与 Go/Presto 方案一致）。

use crate::*;

// expression、planctx、property 及 EstimateColsNDVWithMatchedLen 由相邻迁移文件后续接线。

// EstimateFullJoinRowCount 估算 full join 的输出行数。
/// 估算 full join 输出行数：笛卡尔积相乘，否则按连接键 NDV 缩放并可选相关性因子。
pub fn EstimateFullJoinRowCount(
    sctx: &dyn planctx::PlanContext,
    isCartesian: bool,
    leftProfile: &property::StatsInfo,
    rightProfile: &property::StatsInfo,
    leftJoinKeys: &[expression::Column],
    rightJoinKeys: &[expression::Column],
    leftSchema: &expression::Schema,
    rightSchema: &expression::Schema,
    leftNAJoinKeys: Option<&[expression::Column]>,
    rightNAJoinKeys: Option<&[expression::Column]>,
) -> f64 {
    if isCartesian {
        // 笛卡尔积没有连接键选择率，输出直接等于左右行数乘积。
        return leftProfile.RowCount * rightProfile.RowCount;
    }

    // 普通连接键存在时优先使用；只有两侧普通键均为空才采用 Null-Aware join keys。
    let (leftKeyNDV, leftColCnt, rightKeyNDV, rightColCnt) =
        if !leftJoinKeys.is_empty() || !rightJoinKeys.is_empty() {
            let (left_ndv, left_count) =
                EstimateColsNDVWithMatchedLen(Some(sctx), leftJoinKeys, leftSchema, leftProfile);
            let (right_ndv, right_count) =
                EstimateColsNDVWithMatchedLen(Some(sctx), rightJoinKeys, rightSchema, rightProfile);
            (left_ndv, left_count, right_ndv, right_count)
        } else {
            let (left_ndv, left_count) = EstimateColsNDVWithMatchedLen(
                Some(sctx),
                leftNAJoinKeys.unwrap_or_default(),
                leftSchema,
                leftProfile,
            );
            let (right_ndv, right_count) = EstimateColsNDVWithMatchedLen(
                Some(sctx),
                rightNAJoinKeys.unwrap_or_default(),
                rightSchema,
                rightProfile,
            );
            (left_ndv, left_count, right_ndv, right_count)
        };

    let count = leftProfile.RowCount * rightProfile.RowCount / leftKeyNDV.max(rightKeyNDV);
    if sctx.GetSessionVars().TiDBOptJoinReorderThreshold <= 0 {
        // 未启用 DP join reorder 时保持基础 NDV 估算，不附加多键相关性假设。
        return count;
    }

    // 启用 DP 选择后，沿用 Go/Presto 方案：每个未被 group NDV 覆盖的剩余连接键乘 0.9。
    let remained = leftJoinKeys.len() as i32 - leftColCnt.max(rightColCnt) as i32;
    count * 0.9_f64.powf(remained as f64)
}
