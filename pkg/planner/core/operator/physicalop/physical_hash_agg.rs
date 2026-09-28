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

// 物理算子：哈希聚合（PhysicalHashAgg）。
//
// 哈希聚合按分组键（Group By）把输入行散列到桶中再计算聚合函数；
// 与流式聚合不同，不要求输入有序。支持下推到 TiFlash（列存分析引擎）的预聚合模式。

// 物理算子：哈希聚合（PhysicalHashAgg）。
//
// 哈希聚合按分组键（Group By）把输入行散列到桶中再计算聚合函数；
// 与流式聚合不同，不要求输入有序。支持下推到 TiFlash（列存分析引擎）的预聚合模式。

use base::{ContextRef, Plan as _, Task};
use costusage::{CostVer2, PlanCostOption};

use crate::BasePhysicalAgg;

/// 物理哈希聚合算子：以哈希表按分组键聚合，可选 TiFlash 预聚合模式。
pub struct PhysicalHashAgg {
    /// 聚合公共基座：分组项、聚合函数与 Schema 生产。
    pub BasePhysicalAgg: BasePhysicalAgg,
    /// TiFlash 预聚合模式字符串（空表示未启用或不适用）。
    pub TiflashPreAggMode: String,
}

impl PhysicalHashAgg {
    /// 返回可变的聚合基座指针，供优化改写共享状态。
    pub fn GetPointer(&mut self) -> &mut BasePhysicalAgg {
        &mut self.BasePhysicalAgg
    }
    /// 深拷贝算子并切换到新的计划上下文（PlanContext）。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            BasePhysicalAgg: self.BasePhysicalAgg.CloneWithSelf(new_ctx)?,
            TiflashPreAggMode: self.TiflashPreAggMode.clone(),
        })
    }
    /// 估算本算子占用的内存字节数。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalAgg.MemoryUsage()
    }
    /// 返回 CPU 代价除数：(并行度相关因子, 其它因子)；含 DISTINCT 时禁用除数优化。
    pub fn CPUCostDivisor(&self, has_distinct: bool) -> (f64, f64) {
        let session_vars = self
            .BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetSessionVars();
        let executor_concurrency = session_vars
            .GetSystemVar(vardef::TiDBExecutorConcurrency)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(vardef::DefExecutorConcurrency);
        let concurrency = |name, default| {
            let configured = session_vars
                .GetSystemVar(name)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(default);
            if configured <= 0 {
                executor_concurrency
            } else {
                configured
            }
        };
        hash_agg_cpu_cost_divisor(
            has_distinct,
            concurrency(
                vardef::TiDBHashAggFinalConcurrency,
                vardef::DefTiDBHashAggFinalConcurrency,
            ),
            concurrency(
                vardef::TiDBHashAggPartialConcurrency,
                vardef::DefTiDBHashAggPartialConcurrency,
            ),
        )
    }
    /// 旧版代价：输入行数 × 聚合函数代价系数（MPP 时系数可能不同）。
    pub fn GetCost(&self, input_rows: f64, _is_root: bool, is_mpp: bool, _flag: u64) -> f64 {
        input_rows.max(0.0) * self.BasePhysicalAgg.GetAggFuncCostFactor(is_mpp)
    }
    /// 计划代价 V1：委托基座物理计划计算。
    pub fn GetPlanCostVer1(
        &mut self,
        task_type: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task_type, option)
    }
    /// 计划代价 V2：委托基座物理计划计算。
    pub fn GetPlanCostVer2(
        &mut self,
        task_type: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task_type, option, inl)
    }
    /// 将本算子挂接到子 Task（执行任务）上。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }
    /// 编码为 tipb Executor（TypeAggregation），供存储层/下推执行。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let mut executor = aggregate_to_pb(
            &self.BasePhysicalAgg,
            ctx,
            store,
            tipb::ExecType::TypeAggregation,
        )?;
        if store == kv::StoreType::TiFlash && !self.TiflashPreAggMode.is_empty() {
            executor
                .mut_aggregation()
                .set_pre_agg_mode(tiflash_pre_agg_mode(&self.TiflashPreAggMode)?);
        }
        Ok(executor)
    }
}

/// 由逻辑聚合（LogicalAggregation）构造物理哈希聚合，完整克隆聚合描述符以免改写污染逻辑计划。
pub fn NewPhysicalHashAgg(
    logical: &logicalop::LogicalAggregation,
    producer: crate::PhysicalSchemaProducer,
) -> Result<PhysicalHashAgg, expression::Error> {
    let mut base = BasePhysicalAgg::New(producer);
    base.GroupByItems = logical
        .GroupByItems
        .iter()
        .map(|item| item.CloneExpr())
        .collect();
    // 物理优化可能原地改写聚合描述符；先完整克隆，保留 mode/排序/分组与返回类型。
    // Some physical optimization paths rewrite aggregate descriptors in place.
    // Clone the complete descriptors so those rewrites cannot mutate the
    // logical plan, while preserving mode, ordering, grouping and return type.
    base.AggFuncs = clone_agg_funcs(&logical.AggFuncs);
    Ok(PhysicalHashAgg {
        BasePhysicalAgg: base,
        TiflashPreAggMode: String::new(),
    })
}

/// 克隆聚合函数描述符列表（crate 内复用，供单元测试校验完整性）。
pub(crate) fn clone_agg_funcs(
    functions: &[aggregation::AggFuncDesc],
) -> Vec<aggregation::AggFuncDesc> {
    functions.to_vec()
}

pub(crate) fn hash_agg_cpu_cost_divisor(
    has_distinct: bool,
    final_concurrency: i64,
    partial_concurrency: i64,
) -> (f64, f64) {
    if has_distinct || (final_concurrency == 1 && partial_concurrency == 1) {
        return (0.0, 0.0);
    }
    (
        final_concurrency.min(partial_concurrency) as f64,
        (final_concurrency + partial_concurrency) as f64,
    )
}

pub(crate) fn tiflash_pre_agg_mode(
    mode: &str,
) -> Result<tipb::TiFlashPreAggMode, expression::Error> {
    match mode {
        vardef::ForcePreAggStr => Ok(tipb::TiFlashPreAggMode::ForcePreAgg),
        vardef::AutoStr => Ok(tipb::TiFlashPreAggMode::Auto),
        vardef::ForceStreamingStr => Ok(tipb::TiFlashPreAggMode::ForceStreaming),
        _ => Err(expression::errors::New(format!(
            "unexpected tiflash pre agg mode: {mode}"
        ))),
    }
}

/// 将聚合基座编码为 tipb::Aggregation，并包装为 Executor；TiFlash 路径会附带子 Executor。
pub(crate) fn aggregate_to_pb(
    base: &BasePhysicalAgg,
    ctx: &mut base::BuildPBContext,
    store: kv::StoreType,
    exec_type: tipb::ExecType,
) -> Result<Box<tipb::Executor>, expression::Error> {
    // 取 PB 客户端与表达式构建上下文，把分组项与聚合函数转为 protobuf。
    let client = ctx
        .GetClient()
        .ok_or_else(|| expression::errors::New("PB client is required"))?;
    let build_ctx = ctx.GetExprCtx();
    let group_by = expression::ExpressionsToPBList(
        build_ctx.GetEvalCtx(),
        &base.GroupByItems,
        client.as_ref(),
    )?;
    let pushdown = expression::NewPushDownContext(
        build_ctx,
        Some(client),
        ctx.InExplainStmt,
        ctx.WarnHandler.clone(),
        ctx.ExtraWarnghandler.clone(),
        ctx.GroupConcatMaxLen,
    );
    let mut functions = Vec::with_capacity(base.AggFuncs.len());
    for function in &base.AggFuncs {
        functions.push(aggregation::AggFuncToPBExpr(&pushdown, function, store)?);
    }
    let mut aggregation = tipb::Aggregation::new();
    aggregation.set_group_by(group_by.into());
    aggregation.set_agg_func(functions.into());
    // TiFlash 下推时把第一个孩子一并编码进 Aggregation。
    let mut executor_id = String::new();
    if store == kv::StoreType::TiFlash {
        let child = base
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .into_iter()
            .next()
            .ok_or_else(|| expression::errors::New("TiFlash hash aggregation requires a child"))?;
        aggregation.set_child(*child.to_pb(ctx, store)?);
        executor_id = base
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .explain_id(&[])
            .to_string();
    }
    let mut executor = tipb::Executor::new();
    executor.set_tp(exec_type);
    executor.set_aggregation(aggregation);
    executor.set_executor_id(executor_id);
    executor.set_fine_grained_shuffle_stream_count(
        base.PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount,
    );
    executor.set_fine_grained_shuffle_batch_size(ctx.TiFlashFineGrainedShuffleBatchSize);
    Ok(Box::new(executor))
}
