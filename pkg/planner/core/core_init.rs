// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 规划器回调注册表初始化。
//
// 对应 Go 包 `init` 中按固定顺序安装的回调槽位名称列表。Rust 侧用全局
// `OnceLock<RwLock<BTreeSet>>` 记录已注册名称，供后续物理计划代价、附着任务
// （Attach2Task）、统计推导等钩子按名查找。执行计划（execution plan）优化
// 依赖这些回调把逻辑算子展开为物理算子并估算代价。

use std::collections::BTreeSet;
use std::sync::{OnceLock, RwLock};

/// Names of callback slots installed by Go's package `init` in exact order.
/// Go 包 `init` 按精确顺序安装的回调槽位名称列表。
const CALLBACKS: &[&str] = &[
    "RegisterRestrictedHintChecker",
    "FindBestTask4BaseLogicalPlan",
    "FindBestTask4LogicalDataSource",
    "ExhaustPhysicalPlans4LogicalJoin",
    "ExhaustPhysicalPlans4LogicalApply",
    "GetActualProbeCntFromProbeParents",
    "GetEstimatedProbeCntFromProbeParents",
    "GetCost4PhysicalSort",
    "Attach2Task4PhysicalSort",
    "GetPlanCostVer14PhysicalSort",
    "GetPlanCostVer24PhysicalSort",
    "Attach2Task4NominalSort",
    "Attach2Task4PhysicalUnionAll",
    "GetPlanCostVer14PhysicalUnionAll",
    "GetPlanCostVer24PhysicalUnionAll",
    "GetPlanCostVer1PhysicalExchangeReceiver",
    "GetPlanCostVer2PhysicalExchangeReceiver",
    "ResolveIndices4PhysicalLimit",
    "Attach2Task4PhysicalLimit",
    "GetPlanCostVer14PhysicalTopN",
    "GetPlanCostVer24PhysicalTopN",
    "Attach2Task4PhysicalTopN",
    "ResolveIndices4PhysicalTopN",
    "Attach2Task4PhysicalSelection",
    "ResolveIndices4PhysicalSelection",
    "GetPlanCostVer24PhysicalSelection",
    "GetPlanCostVer14PhysicalSelection",
    "Attach2Task4PhysicalExpand",
    "Attach2Task4PhysicalUnionScan",
    "ResolveIndices4PhysicalUnionScan",
    "GetCost4PhysicalProjection",
    "Attach2Task4PhysicalProjection",
    "GetPlanCostVer14PhysicalProjection",
    "GetPlanCostVer24PhysicalProjection",
    "ResolveIndices4PhysicalProjection",
    "GetCost4PhysicalIndexJoin",
    "GetPlanCostVer14PhysicalIndexJoin",
    "GetIndexJoinCostVer24PhysicalIndexJoin",
    "Attach2Task4PhysicalIndexJoin",
    "GetCost4PhysicalMergeJoin",
    "Attach2Task4PhysicalMergeJoin",
    "GetPlanCostVer14PhysicalMergeJoin",
    "GetPlanCostVer24PhysicalMergeJoin",
    "GetCost4PhysicalHashJoin",
    "GetPlanCostVer14PhysicalHashJoin",
    "Attach2Task4PhysicalHashJoin",
    "GetPlanCostVer24PhysicalHashJoin",
    "GetCost4PhysicalIndexHashJoin",
    "GetPlanCostVer1PhysicalIndexHashJoin",
    "Attach2Task4PhysicalIndexHashJoin",
    "GetCost4PhysicalIndexMergeJoin",
    "GetPlanCostVer14PhysicalIndexMergeJoin",
    "Attach2Task4PhysicalIndexMergeJoin",
    "GetCost4PhysicalHashAgg",
    "Attach2Task4PhysicalHashAgg",
    "GetPlanCostVer14PhysicalHashAgg",
    "GetPlanCostVer24PhysicalHashAgg",
    "GetCost4PhysicalStreamAgg",
    "Attach2Task4PhysicalStreamAgg",
    "GetPlanCostVer14PhysicalStreamAgg",
    "GetPlanCostVer24PhysicalStreamAgg",
    "Attach2Task4PhysicalApply",
    "GetCost4PhysicalApply",
    "GetPlanCostVer14PhysicalApply",
    "GetPlanCostVer24PhysicalApply",
    "GetCost4PhysicalIndexLookUpReader",
    "GetPlanCostVer14PhysicalIndexLookUpReader",
    "GetPlanCostVer24PhysicalIndexLookUpReader",
    "ResolveIndices4PhysicalIndexLookUpReader",
    "Attach2Task4PhysicalWindow",
    "Attach2Task4PhysicalSequence",
    "GetPlanCostVer14PhysicalIndexScan",
    "GetPlanCostVer24PhysicalIndexScan",
    "GetPlanCostVer14PhysicalTableScan",
    "GetPlanCostVer24PhysicalTableScan",
    "GetPlanCostVer24PhysicalCTE",
    "Attach2Task4PhysicalCTEStorage",
    "GetPlanCostVer14PhysicalIndexReader",
    "GetPlanCostVer24PhysicalIndexReader",
    "GetPlanCostVer14PhysicalTableReader",
    "GetPlanCostVer24PhysicalTableReader",
    "GetCost4PointGetPlan",
    "GetPlanCostVer14PointGetPlan",
    "GetPlanCostVer24PointGetPlan",
    "GetCost4BatchPointGetPlan",
    "GetPlanCostVer14BatchPointGetPlan",
    "GetPlanCostVer24BatchPointGetPlan",
    "DoOptimize",
    "GetPlanCost",
    "AttachPlan2Task",
    "GetTaskPlanCost",
    "CompareTaskCost",
    "GetPossibleAccessPaths",
    "AddPrefix4ShardIndexes",
    "DeriveStats4DataSource",
    "DeriveStats4LogicalIndexScan",
    "DeriveStats4LogicalTableScan",
    "CollectFilters4MVIndex",
    "BuildPartialPaths4MVIndex",
    "PrepareCols4MVIndex",
    "InvalidTask",
    "EvalSimpleAst",
    "BuildSimpleExpr",
    "DecodeKeyFromString",
    "EncodeRecordKeyFromRow",
    "EncodeIndexKeyFromRow",
    "EvalAstExprWithPlanCtx",
    "RewriteAstExprWithPlanCtx",
    "DefaultDisabledLogicalRulesList",
    "GetPlanCostVer14PhysicalIndexMergeReader",
    "GetPlanCostVer24PhysicalIndexMergeReader",
];

/// 全局回调名称注册表；惰性初始化。
static REGISTRY: OnceLock<RwLock<BTreeSet<&'static str>>> = OnceLock::new();

/// 将 `CALLBACKS` 全部写入注册表（幂等；可重复调用）。
pub fn init() {
    let registry = REGISTRY.get_or_init(|| RwLock::new(BTreeSet::new()));
    registry
        .write()
        .expect("planner callback registry poisoned")
        .extend(CALLBACKS.iter().copied());
}

/// 返回当前已注册的规划器回调名称集合（必要时先 `init`）。
pub fn RegisteredPlannerCallbacks() -> BTreeSet<&'static str> {
    init();
    REGISTRY
        .get()
        .expect("registry initialized")
        .read()
        .expect("planner callback registry poisoned")
        .clone()
}
