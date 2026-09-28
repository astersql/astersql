// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 计划缓存命中后的计划重建与范围校验。
//
// 对缓存取出的物理计划做递归范围重建，并复刻 Go 侧的范围安全门禁。

use crate::{PlanKind, PlanNode};
/// 对缓存计划执行 `rebuildRange`；成功返回 true。
pub fn RebuildPlan4CachedPlan(plan: &mut PlanNode) -> bool {
    rebuildRange(plan).is_ok()
}
/// 深度优先重建普通子树及 IndexMerge partial plans，并拒绝不安全扫描范围。
pub fn rebuildRange(plan: &mut PlanNode) -> Result<(), String> {
    for child in &mut plan.children {
        rebuildRange(child)?;
    }
    match &mut plan.kind {
        PlanKind::IndexScan { ranges, .. } => {
            let rebuilt = RangeRebuildResult {
                ranges: ranges.clone(),
                access_conditions: vec![],
                remained_conditions: vec![],
            };
            if !isSafeRange(&[], &rebuilt, false, Some(ranges)) {
                return Err("rebuild to get an unsafe range".into());
            }
        }
        PlanKind::IndexMergeReader { partial_plans, .. } => {
            for partial in partial_plans {
                rebuildRange(partial)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Rust 轻量计划模型中的 ranger 拆分结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeRebuildResult {
    pub ranges: Vec<String>,
    pub access_conditions: Vec<String>,
    pub remained_conditions: Vec<String>,
}

fn has_full_range(ranges: &[String], unsigned_int_handle: bool) -> bool {
    let expected = if unsigned_int_handle {
        "full unsigned"
    } else {
        "full"
    };
    ranges.len() == 1 && ranges[0].eq_ignore_ascii_case(expected)
}

/// 对应 Go `isSafeRange`：拒绝残留条件、条件丢失、空范围及非 full 到 full 的退化。
pub fn isSafeRange(
    access_conditions: &[String],
    rebuilt: &RangeRebuildResult,
    unsigned_int_handle: bool,
    original_ranges: Option<&[String]>,
) -> bool {
    if !rebuilt.remained_conditions.is_empty()
        || rebuilt.access_conditions.len() != access_conditions.len()
        || rebuilt.ranges.is_empty()
    {
        return false;
    }

    !(!access_conditions.is_empty()
        && has_full_range(&rebuilt.ranges, unsigned_int_handle)
        && original_ranges.is_some_and(|ranges| !has_full_range(ranges, unsigned_int_handle)))
}
