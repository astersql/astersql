// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 物理执行计划的文本编码与归一化摘要。
//
// 将扁平化物理计划（FlatPhysicalPlan）编码为可读字符串，或生成归一化文本与
// SHA-256 计划摘要（Plan Digest），供 EXPLAIN、计划比对与缓存键使用。
// Region / 存储引擎侧通过 `StoreType`（Root / TiKV / TiFlash）标注任务落点。

use crate::{FlatOperator, FlatPhysicalPlan, FlatPlanTree, PlanKind, PlanNode, StoreType};
use sha2::{Digest as _, Sha256};

/// 32 字节 SHA-256 计划摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanDigest([u8; 32]);

impl PlanDigest {
    /// 返回原始摘要字节。
    pub fn Bytes(&self) -> &[u8; 32] {
        &self.0
    }
    /// 返回小写十六进制字符串形式。
    pub fn String(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// 对字符串做 SHA-256，得到 `PlanDigest`。
fn digest(value: &str) -> PlanDigest {
    let bytes: [u8; 32] = Sha256::digest(value.as_bytes()).into();
    PlanDigest(bytes)
}

/// 将存储类型映射为编码输出中的引擎名。
fn store_name(store: StoreType) -> &'static str {
    match store {
        StoreType::Root => "root",
        StoreType::TiKV => "tikv",
        StoreType::TiFlash => "tiflash",
    }
}

/// 编码单个扁平算子行；`normalized` 时用 `?` 隐藏 id/行数并用算子名代替详细 info。
fn encode_operator(
    operator: &FlatOperator,
    normalized: bool,
    level_offset: usize,
    output: &mut String,
) {
    let id = if normalized {
        "?".to_owned()
    } else {
        operator.Origin.id.to_string()
    };
    let rows = if normalized {
        "?".to_owned()
    } else {
        format!("{:.2}", operator.Origin.estimated_rows)
    };
    let label = operator.Label.to_string();
    let info = if normalized {
        operator.Origin.kind.name()
    } else {
        &operator.Origin.operator_info
    };
    output.push_str(&format!(
        "{}\t{}{}\t{}\t{}\t{}\t{}\n",
        operator.Level.saturating_sub(level_offset),
        id,
        label,
        operator.Origin.kind.name(),
        rows,
        store_name(operator.StoreType),
        info
    ));
}

/// 按序编码一棵扁平计划树中的全部算子。
fn encodeFlatPlanTree(tree: &FlatPlanTree, normalized: bool, output: &mut String) {
    for operator in tree {
        encode_operator(operator, normalized, 0, output);
    }
}

/// 编码扁平物理计划的 Main / CTE / 标量子查询文本（非归一化）。
///
/// 空 Main 或处于 Execute 路径时返回空串。
pub fn EncodeFlatPlan(flat: &FlatPhysicalPlan) -> String {
    if flat.Main.is_empty() || flat.InExecute {
        return String::new();
    }
    let mut output =
        String::with_capacity(80 * (flat.Main.len() + flat.CTE.len() + flat.ScalarSubQ.len()));
    encodeFlatPlanTree(&flat.Main, false, &mut output);
    encodeFlatPlanTree(&flat.CTE, false, &mut output);
    encodeFlatPlanTree(&flat.ScalarSubQ, false, &mut output);
    output
}

/// 将 `PlanNode` 扁平化后编码为文本计划。
pub fn EncodePlan(plan: Option<&PlanNode>) -> String {
    let Some(plan) = plan else {
        return String::new();
    };
    let Some(flat) = crate::FlattenPhysicalPlan(Some(plan), false) else {
        return String::new();
    };
    EncodeFlatPlan(&flat)
}

/// 归一化扁平计划：隐藏易变字段，返回归一化文本及其 SHA-256 摘要。
pub fn NormalizeFlatPlan(flat: &FlatPhysicalPlan) -> (String, PlanDigest) {
    let (select_plan, select_plan_offset) = flat.GetSelectPlan();
    if select_plan
        .first()
        .is_none_or(|operator| !operator.Origin.IsPhysical())
    {
        return (String::new(), digest(""));
    }
    let mut output = String::new();
    for operator in select_plan {
        encode_operator(operator, true, select_plan_offset, &mut output);
    }
    let plan_digest = digest(&output);
    (output, plan_digest)
}

/// 将 `PlanNode` 扁平化后做归一化编码与摘要。
pub fn NormalizePlan(plan: Option<&PlanNode>) -> (String, PlanDigest) {
    let Some(plan) = plan else {
        return (String::new(), digest(""));
    };
    let Some(select_plan) = getSelectPlan(plan) else {
        return (String::new(), digest(""));
    };
    let Some(flat) = crate::FlattenPhysicalPlan(Some(select_plan), false) else {
        return (String::new(), digest(""));
    };
    NormalizeFlatPlan(&flat)
}

/// 从 DML（Update/Delete/Insert）取出内层 SELECT 计划，或直接返回物理计划。
pub fn getSelectPlan(plan: &PlanNode) -> Option<&PlanNode> {
    match plan.kind {
        PlanKind::Update | PlanKind::Delete | PlanKind::Insert => plan.children.first(),
        _ if plan.IsPhysical() => Some(plan),
        _ => None,
    }
}
