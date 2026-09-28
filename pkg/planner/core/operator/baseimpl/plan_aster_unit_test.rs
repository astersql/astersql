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

// `Plan` 基类单元测试：校验计划节点构造、ID 分配、克隆与 Plan Cache 相关行为。
//
// 覆盖与 Go 侧一致的身份规则（ExplainID、QueryBlockOffset、统计信息浅共享等），
// 以及 `CloneForPlanCache` 默认不可缓存时的返回约定。
use crate::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// 测试用 PlanContext：提供可控的 plan_id 与 ignore_explain_id_suffix，其余接口按需 panic。
struct TestPlanContext {
    plan_id: AtomicI32,
    ignore_explain_id_suffix: AtomicBool,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

/// 构造测试上下文，`plan_id` 为下一次 `alloc_plan_id` 的起点（取后自增）。
impl TestPlanContext {
    /// 以给定起始 plan_id 与 ExplainID 后缀开关创建测试上下文。
    fn new(plan_id: i32, ignore_explain_id_suffix: bool) -> Self {
        Self {
            plan_id: AtomicI32::new(plan_id),
            ignore_explain_id_suffix: AtomicBool::new(ignore_explain_id_suffix),
            builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
        }
    }
}

/// 实现 PlanContext：仅支持分配计划 ID、读取 Explain 后缀开关与内建函数计数。
impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn plan_id_checkpoint(&self) -> Option<i32> {
        Some(self.plan_id.load(Ordering::SeqCst))
    }

    fn restore_plan_id_checkpoint(&self, checkpoint: i32) {
        self.plan_id.store(checkpoint, Ordering::SeqCst);
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        self.ignore_explain_id_suffix.load(Ordering::SeqCst)
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("base plan tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("base plan tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("base plan tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("base plan tests do not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("base plan tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

#[test]
fn plan_id_checkpoint_discards_transient_candidate_allocations() {
    let ctx = context(29, false);
    let checkpoint = ctx
        .plan_id_checkpoint()
        .expect("test allocator supports snapshots");
    assert_eq!(ctx.alloc_plan_id(), 30);
    assert_eq!(ctx.alloc_plan_id(), 31);
    ctx.restore_plan_id_checkpoint(checkpoint);
    assert_eq!(ctx.alloc_plan_id(), 30);
}

/// 包装为 `PlanContextRef`（Arc），供 `NewBasePlan` 使用。
fn context(plan_id: i32, ignore_explain_id_suffix: bool) -> PlanContextRef {
    Arc::new(TestPlanContext::new(plan_id, ignore_explain_id_suffix))
}

#[test]
/// 校验 NewBasePlan 初始字段，以及 ReAlloc4Cascades 重置 ID/类型并清空统计信息。
fn construction_and_reallocation_match_go_identity_rules() {
    let ctx = context(0, false);
    let mut plan = NewBasePlan(Arc::clone(&ctx), "TableScan", 7);

    assert_eq!(plan.ID(), 1);
    assert_eq!(plan.TP(&[]), "TableScan");
    assert_eq!(plan.QueryBlockOffset(), 7);
    assert_eq!(plan.OutputNames().0.len(), 0);
    assert_eq!(plan.ExplainInfo(), "N/A");
    assert_eq!(plan.ExplainID(&[]).to_string(), "TableScan_1");
    assert_eq!(plan.MemoryUsage(), PlanSize + "TableScan".len() as i64);

    plan.SetStats(Some(Arc::new(property::StatsInfo::default())));
    plan.ReAlloc4Cascades("Projection");
    assert_eq!(plan.ID(), 2);
    assert_eq!(plan.TP(&[]), "Projection");
    assert_eq!(plan.QueryBlockOffset(), 7);
    assert!(plan.StatsInfo().is_none());
}

#[test]
/// 校验 CloneWithNewCtx 仅替换上下文，统计信息指针浅共享；IgnoreExplainIDSuffix 影响 ExplainID。
fn clone_switches_only_context_and_shallow_shares_statistics() {
    let original_ctx = context(10, false);
    let replacement_ctx = context(20, true);
    let mut plan = NewBasePlan(Arc::clone(&original_ctx), "HashJoin", 3);
    plan.SetStats(Some(Arc::new(property::StatsInfo::default())));
    let original_stats = plan.StatsInfo().unwrap() as *const property::StatsInfo;

    let cloned = plan.CloneWithNewCtx(Arc::clone(&replacement_ctx));
    assert!(Arc::ptr_eq(plan.SCtx(), &original_ctx));
    assert!(Arc::ptr_eq(cloned.SCtx(), &replacement_ctx));
    assert_eq!(cloned.ID(), plan.ID());
    assert_eq!(cloned.QueryBlockOffset(), plan.QueryBlockOffset());
    assert_eq!(
        cloned.StatsInfo().unwrap() as *const property::StatsInfo,
        original_stats
    );
    assert_eq!(cloned.ExplainID(&[]).to_string(), "HashJoin");
}

#[test]
/// 校验 SetID/SetTP、NoncacheableReason 首次写入保留，以及默认 CloneForPlanCache 失败。
fn setters_and_default_plan_cache_behavior_match_go() {
    let ctx = context(0, false);
    let mut plan = NewBasePlan(Arc::clone(&ctx), "Selection", 1);
    let replacement_ctx = context(100, true);

    plan.SetID(42);
    plan.SetTP("Limit");
    plan.SetQueryBlockOffset(9);
    assert_eq!(plan.ID(), 42);
    assert_eq!(plan.TP(&[true]), "Limit");
    assert_eq!(plan.ExplainID(&[true]).to_string(), "Limit_42");
    assert_eq!(plan.QueryBlockOffset(), 9);

    plan.SetSCtx(Arc::clone(&replacement_ctx));
    assert!(Arc::ptr_eq(plan.SCtx(), &replacement_ctx));
    assert_eq!(plan.ExplainID(&[]).to_string(), "Limit");

    plan.SetOutputNames(types::metadata::NameSlice(vec![None]));
    plan.ReplaceExprColumns(&HashMap::new());
    assert!(plan.OutputNames().0.is_empty());

    plan.SetNoncacheableReason("first");
    plan.SetNoncacheableReason("second");
    assert_eq!(plan.GetNoncacheableReason(), "first");

    let (cloned, ok) = plan.CloneForPlanCache(ctx);
    assert!(!ok);
    assert!(cloned.is_none());
}

#[test]
/// Go 的 `len(string)` 与 Rust 的 `String::len` 都按 UTF-8 字节计费。
fn memory_usage_counts_type_name_bytes_like_go() {
    let plan = NewBasePlan(context(0, false), "扫描", 0);

    assert_eq!(plan.MemoryUsage(), PlanSize + "扫描".len() as i64);
}
