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

// bindinfo 自动绑定（auto binding）测试模块。
//
// 本文件是从 Go(TiDB) 的 `binding_auto_test.go` 机械迁移而来的测试基线，
// 覆盖 SQL 绑定（SQL Binding，即把某条 SQL 固定到指定执行计划上的机制）
// 的自动探索能力，主要包括：
// - `GenBriefPlanWithSCtx`：在会话上下文中为语句生成简要执行计划
//   （执行计划指优化器为 SQL 选择的具体执行方式，如索引扫描、聚合算子等）；
// - `EXPLAIN EXPLORE` 语句：枚举同一条 SQL 的多个候选计划（含不同索引提示、
//   `no_decorrelate` 去关联提示等），供用户挑选并生成绑定；
// - `IsSimplePointPlan`：判断计划文本是否为简单点查
//   （Point Get，按主键或唯一索引直接定位单行的最快访问路径）；
// - `RecordRelevantOptVarsAndFixes`：记录影响计划选择的优化器变量与 fix 项。
//
/// 冒烟测试：验证 `IsSimplePointPlan` 对典型计划文本的正反例分类。
///
/// - 正例：`Point_Get`（点查，按主键/唯一索引直接读取单行）与
///   `Batch_Point_Get`（批量点查，一次读取多个主键对应的行）应被判定为简单点查计划；
/// - 反例：`IndexRangeScan`（索引范围扫描，需要遍历一段索引区间）不属于点查，
///   应返回 false。
///
/// 该判断用于自动绑定流程：简单点查计划通常已是最优，无需再为其探索候选计划。
#[test]
fn canonical_simple_point_plan_classification_covers_positive_and_negative_plans() {
    assert!(crate::IsSimplePointPlan("Point_Get table:t"));
    assert!(crate::IsSimplePointPlan("Batch_Point_Get table:t"));
    assert!(!crate::IsSimplePointPlan("IndexRangeScan table:t"));
}

use crate::{
    BindError, Binding, BindingPlanEvolution, BindingPlanInfo, ExploreContext, GenerationSpec,
    PlanExecInfo, PlanGenerator, PlanPerfPredictor, PlanRuntime, Result, binding_auto::bindingAuto,
    genedPlan, state,
};
use std::sync::{Arc, Mutex};

struct FixedGenerator(Vec<BindingPlanInfo>);

impl PlanGenerator for FixedGenerator {
    fn Generate(&self, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<BindingPlanInfo>> {
        Ok(self.0.clone())
    }
}

struct ZeroPredictor;

impl PlanPerfPredictor for ZeroPredictor {
    fn PerfPredicate(&self, plans: &mut [BindingPlanInfo]) -> Result<(Vec<f64>, Vec<String>)> {
        Ok((vec![0.0; plans.len()], vec![String::new(); plans.len()]))
    }
}

struct RecordingRuntime {
    historical: Vec<Arc<Binding>>,
    executions: Mutex<Vec<String>>,
    historical_info: Option<PlanExecInfo>,
}

impl PlanRuntime for RecordingRuntime {
    fn historical_bindings(&self, _: &str, _: &str, _: &str, _: &str) -> Result<Vec<Arc<Binding>>> {
        Ok(self.historical.clone())
    }

    fn plan_exec_info(&self, _: &str) -> Result<Option<PlanExecInfo>> {
        Ok(self.historical_info.clone())
    }

    fn execute_binding(&self, binding: &Binding) -> Result<PlanExecInfo> {
        self.executions
            .lock()
            .unwrap()
            .push(binding.BindSQL.clone());
        Ok(PlanExecInfo {
            ExecCount: 1,
            ResultRows: 1,
            ..PlanExecInfo::default()
        })
    }

    fn generation_spec(&self, _: &str, _: &str, _: &str, _: &str) -> Result<GenerationSpec> {
        Err(BindError("unused generation_spec".to_owned()))
    }

    fn plan_under_state(&self, _: &GenerationSpec, _: &state) -> Result<genedPlan> {
        Err(BindError("unused plan_under_state".to_owned()))
    }
}

fn candidate(sql: &str, digest: &str) -> BindingPlanInfo {
    BindingPlanInfo {
        Binding: Arc::new(Binding {
            BindSQL: sql.to_owned(),
            PlanDigest: digest.to_owned(),
            ..Binding::default()
        }),
        ..BindingPlanInfo::default()
    }
}

#[test]
fn analyze_executes_only_generated_candidates_like_go() {
    let runtime = Arc::new(RecordingRuntime {
        historical: vec![candidate("historical", "history-digest").Binding],
        executions: Mutex::new(Vec::new()),
        historical_info: None,
    });
    let evolution = bindingAuto {
        runtime: runtime.clone(),
        planGenerator: Box::new(FixedGenerator(vec![candidate(
            "generated",
            "generated-digest",
        )])),
        ruleBasedPredictor: Box::new(ZeroPredictor),
        llmPredictor: Box::new(ZeroPredictor),
    };

    evolution
        .ExplorePlansForSQL(&ExploreContext::default(), "select 1", true)
        .unwrap();

    assert_eq!(*runtime.executions.lock().unwrap(), vec!["generated"]);
}

#[test]
fn non_positive_historical_exec_count_leaves_candidate_stats_empty_like_go() {
    let runtime = Arc::new(RecordingRuntime {
        historical: vec![candidate("historical", "history-digest").Binding],
        executions: Mutex::new(Vec::new()),
        historical_info: Some(PlanExecInfo {
            Plan: "stale plan".to_owned(),
            ExecCount: 0,
            ResultRows: 99,
            ProcessedKeys: 88,
            TotalTime: 77,
        }),
    });
    let evolution = bindingAuto {
        runtime,
        planGenerator: Box::new(FixedGenerator(Vec::new())),
        ruleBasedPredictor: Box::new(ZeroPredictor),
        llmPredictor: Box::new(ZeroPredictor),
    };

    let plans = evolution
        .ExplorePlansForSQL(&ExploreContext::default(), "select 1", false)
        .unwrap();

    assert_eq!(plans.len(), 1);
    assert!(plans[0].Plan.is_empty());
    assert_eq!(plans[0].ExecTimes, 0);
    assert_eq!(plans[0].AvgReturnedRows, 0.0);
}
