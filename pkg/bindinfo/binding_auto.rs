// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 自动绑定演进（binding auto-evolution）模块。
//
// “绑定”（Binding）是把某条 SQL 固定到一个特定执行计划的机制，用于在优化器
// 选择不佳时人为锁定更优的计划。本模块实现对某条 SQL 的候选计划探索流程：
//
// 1. 从历史记录（如 Statement Summary）中收集该 SQL 已存在的绑定及其执行统计；
// 2. 通过 `PlanGenerator` 生成新的候选计划；
// 3. 可选地实际执行各候选绑定以补齐执行信息（analyze 模式）；
// 4. 依次用规则打分器与 LLM 打分器（`PlanPerfPredictor`）对候选计划进行性能
//    预测，并把得分最高的计划标记为推荐（Recommend = YES）。
//
// 术语说明：
// - 执行计划（Plan）：优化器为 SQL 生成的算子树，决定数据的访问与计算方式；
// - Plan Digest：执行计划的哈希摘要，用于唯一标识一个计划形态；
// - SQL Digest：SQL 归一化（去掉具体参数值）后的哈希摘要，用于聚合同类语句。

use crate::{
    BindError, Binding, PlanGenerator, PlanPerfPredictor, Result, StatusDeleted,
    llmBasedPlanPerfPredictor, planGenerator, ruleBasedPlanPerfPredictor,
};
use std::sync::Arc;

/// 某个执行计划的累计执行统计信息，通常来自 Statement Summary
/// （语句汇总表，记录历史 SQL 的执行指标）。
#[derive(Clone, Debug, Default)]
pub struct PlanExecInfo {
    /// 执行计划的文本表示（EXPLAIN 风格的算子树）。
    pub Plan: String,
    /// 累计返回给客户端的行数。
    pub ResultRows: i64,
    /// 累计执行次数。
    pub ExecCount: i64,
    /// 累计扫描（处理）的键数量，反映存储层的扫描开销。
    pub ProcessedKeys: i64,
    /// 累计执行耗时（与来源表一致的时间单位，如纳秒）。
    pub TotalTime: i64,
}

/// 计划探索所依赖的运行时能力抽象，由上层（会话/执行器）实现，
/// 使本模块无需直接依赖优化器与存储层的具体实现。
pub trait PlanRuntime: Send + Sync {
    /// 按 SQL 文本或 SQL Digest 查询历史上出现过的绑定列表。
    fn historical_bindings(
        &self,
        current_db: &str,
        sql_or_digest: &str,
        charset: &str,
        collation: &str,
    ) -> Result<Vec<Arc<Binding>>>;
    /// 按 Plan Digest 查询该计划的历史执行统计；不存在时返回 `None`。
    fn plan_exec_info(&self, plan_digest: &str) -> Result<Option<PlanExecInfo>>;
    /// 实际执行一次绑定对应的 SQL，返回本次执行的统计信息（analyze 模式使用）。
    fn execute_binding(&self, binding: &Binding) -> Result<PlanExecInfo>;
    /// 为给定 SQL 构造计划生成规格（描述可探索的优化器开关/提示等搜索空间）。
    fn generation_spec(
        &self,
        default_schema: &str,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> Result<crate::GenerationSpec>;
    /// 在指定的搜索状态（一组优化器开关/提示取值）下生成候选执行计划。
    fn plan_under_state(
        &self,
        spec: &crate::GenerationSpec,
        state: &crate::state,
    ) -> Result<crate::genedPlan>;
}

/// 计划探索的会话上下文，携带解析/规范化 SQL 所需的环境信息。
#[derive(Clone, Debug, Default)]
pub struct ExploreContext {
    /// 当前数据库（schema）名，用于补全未限定库名的表引用。
    pub CurrentDB: String,
    /// 字符集（如 utf8mb4），影响 SQL 文本的解析与摘要计算。
    pub Charset: String,
    /// 排序规则（collation），决定字符串比较与排序的规则。
    pub Collation: String,
}

/// 候选“绑定 + 计划”的汇总信息，聚合了执行统计与推荐结论，
/// 是 `ExplorePlansForSQL` 的返回单元。
#[derive(Clone, Debug, Default)]
pub struct BindingPlanInfo {
    /// 候选绑定本体（含绑定 SQL、状态、Plan Digest 等）。
    pub Binding: Arc<Binding>,
    /// 该绑定对应执行计划的文本表示。
    pub Plan: String,
    /// 平均单次执行延迟（TotalTime / ExecCount）。
    pub AvgLatency: f64,
    /// 历史执行次数；为 0 表示尚无执行统计。
    pub ExecTimes: i64,
    /// 平均单次扫描行（键）数。
    pub AvgScanRows: f64,
    /// 平均单次返回行数。
    pub AvgReturnedRows: f64,
    /// 每返回一行付出的平均延迟，衡量计划的“单位产出耗时”。
    pub LatencyPerReturnRow: f64,
    /// 每返回一行需要扫描的平均行数，衡量计划的扫描效率。
    pub ScanRowsPerReturnRow: f64,
    /// 推荐结论：`"YES (from ...)"` 或 `"NO"`。
    pub Recommend: String,
    /// 推荐理由（由性能预测器给出的解释文本）。
    pub Reason: String,
}

/// 绑定计划演进的对外接口：针对一条 SQL 探索候选计划并给出推荐。
pub trait BindingPlanEvolution: Send + Sync {
    /// 为指定 SQL（或 SQL Digest）探索候选计划：
    /// 收集历史绑定、生成新计划，`analyze` 为真时会实际执行候选以获取统计，
    /// 最后由性能预测器标记推荐项。
    fn ExplorePlansForSQL(
        &self,
        stmtSCtx: &ExploreContext,
        sqlOrDigest: &str,
        analyze: bool,
    ) -> Result<Vec<BindingPlanInfo>>;
}

/// `BindingPlanEvolution` 的默认实现，组合运行时、计划生成器与两种性能预测器。
pub(crate) struct bindingAuto {
    /// 运行时能力（查历史绑定、查执行统计、执行绑定等）。
    pub(crate) runtime: Arc<dyn PlanRuntime>,
    /// 候选计划生成器，负责在优化器搜索空间中产生新计划。
    pub(crate) planGenerator: Box<dyn PlanGenerator>,
    /// 基于启发式规则的性能预测器，优先使用。
    pub(crate) ruleBasedPredictor: Box<dyn PlanPerfPredictor>,
    /// 基于大语言模型（LLM）的性能预测器，规则打分失败时兜底。
    pub(crate) llmPredictor: Box<dyn PlanPerfPredictor>,
}

/// 构造默认的 `bindingAuto` 实例并以 trait 对象形式返回。
pub fn newBindingAuto(runtime: Arc<dyn PlanRuntime>) -> Arc<dyn BindingPlanEvolution> {
    Arc::new(bindingAuto {
        planGenerator: Box::new(planGenerator::new(Arc::clone(&runtime))),
        runtime,
        ruleBasedPredictor: Box::new(ruleBasedPlanPerfPredictor),
        llmPredictor: Box::new(llmBasedPlanPerfPredictor),
    })
}

impl BindingPlanEvolution for bindingAuto {
    fn ExplorePlansForSQL(
        &self,
        stmtSCtx: &ExploreContext,
        sqlOrDigest: &str,
        analyze: bool,
    ) -> Result<Vec<BindingPlanInfo>> {
        // 第一步：收集该 SQL 的历史绑定及其执行统计作为初始候选集。
        let mut historical_plans = self.getBindingPlanInfo(
            &stmtSCtx.CurrentDB,
            sqlOrDigest,
            &stmtSCtx.Charset,
            &stmtSCtx.Collation,
        )?;
        // 第二步：调用计划生成器，把新探索出的候选计划并入候选集。
        let mut generated_plans = self.planGenerator.Generate(
            &stmtSCtx.CurrentDB,
            sqlOrDigest,
            &stmtSCtx.Charset,
            &stmtSCtx.Collation,
        )?;
        // analyze 模式：对缺少执行统计的候选实际执行一次，补齐指标。
        if analyze {
            // 与 Go 一致：历史候选已有语句统计，只执行本轮新生成的候选。
            self.runToGetExecInfo(&mut generated_plans)?;
        }
        historical_plans.append(&mut generated_plans);
        // 先用规则打分器推荐；若其无法给出有效评分（全为 0），再回退到 LLM 打分器。
        if !Self::fillRecommendation(
            &mut historical_plans,
            self.ruleBasedPredictor.as_ref(),
            "rule-based",
        )? {
            Self::fillRecommendation(&mut historical_plans, self.llmPredictor.as_ref(), "LLM")?;
        }
        Ok(historical_plans)
    }
}

impl bindingAuto {
    /// 对尚无执行统计（ExecTimes == 0）的候选逐一实际执行，补齐执行指标。
    fn runToGetExecInfo(&self, plans: &mut [BindingPlanInfo]) -> Result<()> {
        for plan in plans.iter_mut().filter(|plan| plan.ExecTimes == 0) {
            let info = self.runtime.execute_binding(&plan.Binding)?;
            apply_exec_info(plan, &info);
        }
        Ok(())
    }

    /// 查询历史绑定并组装为候选列表：跳过已删除的绑定，
    /// 并尽量附带其历史执行统计。
    fn getBindingPlanInfo(
        &self,
        currentDB: &str,
        sqlOrDigest: &str,
        charset: &str,
        collation: &str,
    ) -> Result<Vec<BindingPlanInfo>> {
        if sqlOrDigest.trim().is_empty() {
            return Err(BindError("SQL or digest is empty".to_owned()));
        }
        let bindings =
            self.runtime
                .historical_bindings(currentDB, sqlOrDigest.trim(), charset, collation)?;
        let mut plans = Vec::new();
        for binding in bindings {
            // 已标记删除的绑定不再参与候选评估。
            if binding.Status == StatusDeleted {
                continue;
            }
            // 只有携带 Plan Digest 的绑定才可能查到历史执行统计。
            let info = if binding.PlanDigest.is_empty() {
                None
            } else {
                self.getPlanExecInfo(&binding.PlanDigest)?
            };
            let mut candidate = BindingPlanInfo {
                Binding: binding,
                ..BindingPlanInfo::default()
            };
            // Go 仅在聚合统计含正执行次数时填充计划和派生指标。
            if let Some(info) = info.filter(|info| info.ExecCount > 0) {
                apply_exec_info(&mut candidate, &info);
            }
            plans.push(candidate);
        }
        Ok(plans)
    }

    /// 用给定的性能预测器为候选计划打分，并把得分最高者标记为推荐。
    /// 返回是否成功产生推荐（全部得分为 0 视为预测器无法判断）。
    fn fillRecommendation(
        plans: &mut [BindingPlanInfo],
        predictor: &dyn PlanPerfPredictor,
        name: &str,
    ) -> Result<bool> {
        if plans.is_empty() {
            return Ok(false);
        }
        let (scores, explanations) = predictor.PerfPredicate(plans)?;
        let max_score = scores.iter().copied().fold(0.0_f64, f64::max);
        // 最高分为 0 说明预测器未能区分优劣，交由调用方回退到下一个预测器。
        if max_score == 0.0 {
            return Ok(false);
        }
        // 仅把第一个达到最高分的候选标为 YES，其余一律标为 NO。
        let mut recommended = false;
        for (index, plan) in plans.iter_mut().enumerate() {
            if !recommended && scores.get(index) == Some(&max_score) {
                plan.Recommend = format!("YES (from {name})");
                plan.Reason = explanations.get(index).cloned().unwrap_or_default();
                recommended = true;
            } else {
                plan.Recommend = "NO".to_owned();
                plan.Reason.clear();
            }
        }
        Ok(recommended)
    }

    /// 按 Plan Digest 查询历史执行统计；摘要为空时直接返回 `None`。
    fn getPlanExecInfo(&self, planDigest: &str) -> Result<Option<PlanExecInfo>> {
        if planDigest.is_empty() {
            return Ok(None);
        }
        self.runtime.plan_exec_info(planDigest)
    }
}

/// 把累计执行统计换算为均值指标写入候选：
/// 先计算单次平均值，再派生“每返回行”的效率指标。
fn apply_exec_info(plan: &mut BindingPlanInfo, info: &PlanExecInfo) {
    plan.Plan = info.Plan.clone();
    plan.ExecTimes = info.ExecCount;
    // 避免除零：只有存在执行记录时才计算平均值。
    if info.ExecCount > 0 {
        plan.AvgLatency = info.TotalTime as f64 / info.ExecCount as f64;
        plan.AvgScanRows = info.ProcessedKeys as f64 / info.ExecCount as f64;
        plan.AvgReturnedRows = info.ResultRows as f64 / info.ExecCount as f64;
    }
    if plan.AvgReturnedRows > 0.0 {
        plan.LatencyPerReturnRow = plan.AvgLatency / plan.AvgReturnedRows;
        plan.ScanRowsPerReturnRow = plan.AvgScanRows / plan.AvgReturnedRows;
    }
}

/// 保留 Go 源码中的小写类型名，作为 `PlanExecInfo` 的别名。
pub type planExecInfo = PlanExecInfo;

/// 判断计划文本是否为“简单点查”计划：即除表头外，所有算子都属于
/// Point_Get（按主键/唯一键单行读取）、Batch_Point_Get（批量点查）、
/// Selection（过滤）或 Projection（投影）。此类计划已足够简单高效，
/// 通常无须再做绑定演进。
pub fn IsSimplePointPlan(plan: &str) -> bool {
    let mut non_empty = false;
    // 逐行检查计划文本中每个算子名，遇到不在白名单内的算子立即判否。
    for line in plan.lines().map(str::trim).filter(|line| !line.is_empty()) {
        non_empty = true;
        let operator = line.split_whitespace().next().unwrap_or_default();
        // "id" 是 EXPLAIN 输出的表头列名，跳过表头行。
        if operator == "id"
            || operator.contains("Point_Get")
            || operator.contains("Batch_Point_Get")
            || operator.contains("Selection")
            || operator.contains("Projection")
        {
            continue;
        }
        return false;
    }
    non_empty
}
