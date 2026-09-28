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

// 绑定（Binding）的执行计划演进（Plan Evolution）模块。
//
// 背景术语说明：
// - 执行计划（Plan）：优化器为一条 SQL 生成的具体执行方式（访问路径、连接顺序等）。
// - 绑定 / 基线（Binding / Baseline）：将某条 SQL 固定到某个执行计划上的机制，
//   用于避免优化器因统计信息波动而选出性能较差的计划。
// - 计划演进（Plan Evolution）：在同一条 SQL 的多个历史候选计划中，
//   依据运行期统计（延迟、扫描行数等）挑选表现更优的计划作为新的基线。
//
// 本模块定义了计划性能预测器 [`PlanPerfPredictor`] 接口，并提供两种实现：
// - `ruleBasedPlanPerfPredictor`：基于启发式规则打分；
// - `llmBasedPlanPerfPredictor`：预留的基于大语言模型的实现（当前为空实现）。

use crate::{BindingPlanInfo, IsSimplePointPlan, Result};

/// 计划性能预测器接口：对一组候选执行计划进行打分，
/// 用于在计划演进过程中挑选“最佳计划”。
pub trait PlanPerfPredictor: Send + Sync {
    /// 对候选计划逐一打分并给出解释。
    ///
    /// 返回值为两个等长向量：
    /// - 每个计划的得分（0.0 ~ 1.0，1.0 表示被判定为最佳计划）；
    /// - 对应的判定理由文本（未被选中的计划为空字符串）。
    fn PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)>;
}

/// 基于启发式规则的计划性能预测器。
///
/// 依据计划的运行期统计指标（平均延迟、平均扫描行数、
/// 每返回一行所需扫描行数等）用一组固定规则判定最佳计划。
pub struct ruleBasedPlanPerfPredictor;

impl PlanPerfPredictor for ruleBasedPlanPerfPredictor {
    fn PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)> {
        let mut scores = vec![0.0; plans.len()];
        let mut explanations = vec![String::new(); plans.len()];
        // 没有候选计划时直接返回全零得分。
        if plans.is_empty() {
            return Ok((scores, explanations));
        }
        // 只有一个候选计划时，它就是最佳计划。
        if plans.len() == 1 {
            scores[0] = 1.0;
            return Ok((scores, explanations));
        }
        // 规则一：若存在简单点查计划（PointGet/BatchPointGet，
        // 即通过主键或唯一索引直接定位行的计划），它总是最优。
        if let Some(index) = plans.iter().position(|plan| IsSimplePointPlan(&plan.Plan)) {
            scores[index] = 1.0;
            explanations[index] = "Simple PointGet or BatchPointGet is the best plan".to_owned();
            return Ok((scores, explanations));
        }
        // 任一计划缺少执行统计（从未被执行过）时，无法可靠比较，放弃判定。
        if plans.iter().any(|plan| plan.ExecTimes == 0) {
            return Ok((scores, explanations));
        }
        // 按多个指标依次排序：每返回一行的扫描行数 -> 平均延迟
        // -> 平均扫描行数 -> 每返回一行的延迟，越小越好。
        plans.sort_by(|a, b| {
            a.ScanRowsPerReturnRow
                .total_cmp(&b.ScanRowsPerReturnRow)
                .then_with(|| a.AvgLatency.total_cmp(&b.AvgLatency))
                .then_with(|| a.AvgScanRows.total_cmp(&b.AvgScanRows))
                .then_with(|| a.LatencyPerReturnRow.total_cmp(&b.LatencyPerReturnRow))
        });
        // 规则二：排序后第一名的“每返回一行扫描行数”若比第二名好至少 50%，
        // 说明其扫描效率显著占优，判定为最佳计划。
        if plans[0].ScanRowsPerReturnRow < plans[1].ScanRowsPerReturnRow / 2.0 {
            scores[0] = 1.0;
            explanations[0] =
                "Plan's scan_rows_per_returned_row is 50% better than others'".to_owned();
            return Ok((scores, explanations));
        }
        // 规则三：第一名在延迟、扫描行数、每返回一行延迟三项指标上
        // 均比其余所有计划好至少 50% 时，判定为最佳计划。
        if plans[1..].iter().all(|plan| {
            plans[0].AvgLatency <= plan.AvgLatency / 2.0
                && plans[0].AvgScanRows <= plan.AvgScanRows / 2.0
                && plans[0].LatencyPerReturnRow <= plan.LatencyPerReturnRow / 2.0
        }) {
            scores[0] = 1.0;
            explanations[0] = "Plan's latency, scan_rows and latency_per_returned_row are 50% better than others'".to_owned();
        }
        Ok((scores, explanations))
    }
}

/// 基于大语言模型（LLM）的计划性能预测器占位实现。
///
/// 目前仅返回全零得分与空解释，不做任何实际判定，留待后续接入模型能力。
pub struct llmBasedPlanPerfPredictor;

impl PlanPerfPredictor for llmBasedPlanPerfPredictor {
    fn PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)> {
        Ok((vec![0.0; plans.len()], vec![String::new(); plans.len()]))
    }
}
