// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// `BaseImpl` / `LimitImpl` 代价读写行为的单元测试。
//
// 对应 Go `base_test.go` 的 `TestBaseImplementation`：验证空子节点时
// CalcCost 返回 0、SetCost/GetCost 可读写，以及 LimitImpl 的 GetPlan 类型名。

// 本文件对应 pkg/planner/implementation/base_test.go。Go 用 MockContext +
// PhysicalLimit 构造 baseImpl，断言 GetPlan / CalcCost / SetCost / GetCost。
// Rust 的 BaseImpl 不再内嵌 plan（plan 在 LimitImpl 等具体实现里）；这里用真实
// PhysicalLimit + LimitImpl + BaseImpl 覆盖同一行为。

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use astersql_expression::Schema;
use astersql_planner_core_base::{
    BuildPBContext, BuiltinFunctionUsageCounter, ContextRef, PhysicalPlan, PlanContext,
};
use astersql_planner_core_operator_physicalop::PhysicalLimit;
use astersql_planner_memo::{Implementation, ImplementationRef};
use astersql_statistics::{HistColl, NewHistColl};

use super::{
    AttachChildren, BaseImpl, ChildCost, CloneChildren, NewIndexReaderImpl, NewLimitImpl,
    NewSortImpl, NewTableDualImpl, NewTableReaderImpl, PlanAccess, ReaderCostPlan, SortCostPlan,
};

/// 测试用最小 `PlanContext`：只实现分配 plan id 与内置函数计数，其余接口未实现。
struct TestPlanContext {
    /// 下一个可分配的物理计划 ID。
    next_id: AtomicI32,
    /// 内置函数使用计数器（本测试不实际触发）。
    usage: BuiltinFunctionUsageCounter,
}

impl Default for TestPlanContext {
    fn default() -> Self {
        Self {
            next_id: AtomicI32::new(1),
            usage: BuiltinFunctionUsageCounter::default(),
        }
    }
}

impl PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        unimplemented!("base_implementation test does not read session vars")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("base_implementation test does not build expressions")
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        unimplemented!("base_implementation test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("base_implementation test does not null-reject check")
    }
    fn GetBuildPBCtx(&self) -> &BuildPBContext {
        unimplemented!("base_implementation test does not build PB")
    }
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.usage.Inc(scalar_func_sig_name)
    }
}

/// 构造测试用会话上下文引用。
fn test_ctx() -> ContextRef {
    Arc::new(TestPlanContext::default())
}

/// 把 `PhysicalLimit` 包成 `PlanAccess`，供 `NewLimitImpl` 使用。
struct LimitAccess {
    plan: PhysicalLimit,
}

impl PlanAccess for LimitAccess {
    fn Plan(&self) -> &dyn PhysicalPlan {
        &self.plan
    }
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan {
        &mut self.plan
    }
}

/// Reader 代价测试桩：固定网络因子/行宽，仅让 worker 数可控。
struct ReaderAccess {
    plan: PhysicalLimit,
    workers: usize,
}

impl PlanAccess for ReaderAccess {
    fn Plan(&self) -> &dyn PhysicalPlan {
        &self.plan
    }
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan {
        &mut self.plan
    }
}

impl ReaderCostPlan for ReaderAccess {
    fn NetworkFactor(&self, _table: &astersql_meta_model::TableInfo) -> f64 {
        2.0
    }

    fn AverageRowSize(
        &self,
        _histograms: &HistColl,
        child: &dyn PhysicalPlan,
        _index: bool,
    ) -> f64 {
        if std::ptr::eq(child, &self.plan as &dyn PhysicalPlan) {
            3.0
        } else {
            7.0
        }
    }

    fn CopIteratorWorkers(&self) -> usize {
        self.workers
    }
}

struct SortAccess {
    plan: PhysicalLimit,
    expected_count: f64,
}

impl PlanAccess for SortAccess {
    fn Plan(&self) -> &dyn PhysicalPlan {
        &self.plan
    }
    fn PlanMut(&mut self) -> &mut dyn PhysicalPlan {
        &mut self.plan
    }
}

impl SortCostPlan for SortAccess {
    fn ExpectedCount(&self) -> f64 {
        self.expected_count
    }

    fn SelfCost(&self, input_rows: f64, _schema: &Schema) -> f64 {
        input_rows
    }

    fn InjectProjectionBelowSort(&mut self, child: Box<dyn PhysicalPlan>) -> Box<dyn PhysicalPlan> {
        child
    }
}

fn child_impl(sctx: &ContextRef, cost: f64) -> ImplementationRef {
    let mut child = NewLimitImpl(Box::new(LimitAccess {
        plan: PhysicalLimit::New(sctx.clone(), 0, 0),
    }));
    child.SetCost(cost);
    Rc::new(RefCell::new(child))
}

fn empty_histograms() -> HistColl {
    *NewHistColl(0, 0, 0, 0, 0)
}

// TestBaseImplementation 对应 Go 的 TestBaseImplementation。
/// 验证 LimitImpl 计划类型名与 BaseImpl/Implementation 代价读写语义。
#[test]
fn TestBaseImplementation() {
    // Go defer view.Stop() / StatsHandle().Close()：Rust 侧用最小 PlanContext
    // 直接构造 PhysicalLimit，不创建 opencensus/domain 后台句柄。
    let sctx = test_ctx();
    let p = PhysicalLimit::New(sctx, 0, 0);
    assert_eq!(p.Offset, 0);
    assert_eq!(p.Count, 0);

    let mut impl_ = NewLimitImpl(Box::new(LimitAccess { plan: p }));
    assert_eq!(impl_.GetPlan().tp(&[]), "Limit");

    // CalcCost 在 baseImpl / BaseImpl 中对空 children 应返回 0，同时写入 cost。
    let cost = Implementation::CalcCost(&impl_, 10.0, &[]);
    assert_eq!(0.0, cost);
    assert_eq!(0.0, impl_.GetCost());

    impl_.SetCost(6.0);
    assert_eq!(6.0, impl_.GetCost());

    // 同步覆盖 BaseImpl 自身的代价读写（Go baseImpl 字段语义）。
    let base = BaseImpl::default();
    assert_eq!(0.0, base.GetCost());
    assert_eq!(0.0, base.CalcCost(10.0, &[]));
    base.SetCost(6.0);
    assert_eq!(6.0, base.GetCost());
}

/// 覆盖 Go `baseImpl` 对非空 children 的累加、代价上限和挂接语义。
#[test]
fn base_implementation_children_follow_go() {
    let sctx = test_ctx();
    let children = [child_impl(&sctx, 2.5), child_impl(&sctx, 3.5)];
    let base = BaseImpl::default();

    assert_eq!(6.0, base.CalcCost(10.0, &children));
    assert_eq!(6.0, base.GetCost());
    assert_eq!(9.0, base.ScaleCostLimit(9.0));
    assert_eq!(3.0, base.GetCostLimit(9.0, &children));
    assert_eq!(2.5, ChildCost(&children, 0));

    let cloned = CloneChildren(&children);
    assert_eq!(2, cloned.len());
    assert!(cloned.iter().all(|plan| plan.tp(&[]) == "Limit"));

    let mut parent = PhysicalLimit::New(sctx, 0, 0);
    AttachChildren(&mut parent, &children);
    let attached = parent.children();
    assert_eq!(2, attached.len());
    assert!(attached.iter().all(|plan| plan.tp(&[]) == "Limit"));
}

/// Go 的 TableDualImpl/MemoryTableScanImpl CalcCost 返回 0，但不覆盖已缓存 cost。
#[test]
fn zero_cost_calc_does_not_reset_cached_cost() {
    let sctx = test_ctx();
    let mut impl_ = NewTableDualImpl(Box::new(LimitAccess {
        plan: PhysicalLimit::New(sctx, 0, 0),
    }));
    impl_.SetCost(6.0);

    assert_eq!(0.0, impl_.CalcCost(10.0, &[]));
    assert_eq!(6.0, impl_.GetCost());
}

/// Go 原样使用 DistSQLScanConcurrency；为 0 时除法得到正无穷且 cost limit 为 0。
#[test]
fn reader_zero_workers_follow_go_float_semantics() {
    let sctx = test_ctx();
    let child = child_impl(&sctx, 4.0);
    let children = [child];

    let table_reader = NewTableReaderImpl(
        Box::new(ReaderAccess {
            plan: PhysicalLimit::New(sctx.clone(), 0, 0),
            workers: 0,
        }),
        astersql_meta_model::TableInfo::default(),
        empty_histograms(),
    );
    assert!(table_reader.CalcCost(5.0, &children).is_infinite());
    assert_eq!(0.0, table_reader.GetCostLimit(10.0, &[]));

    let index_reader = NewIndexReaderImpl(
        Box::new(ReaderAccess {
            plan: PhysicalLimit::New(sctx, 0, 0),
            workers: 0,
        }),
        astersql_meta_model::TableInfo::default(),
        empty_histograms(),
    );
    assert!(index_reader.CalcCost(5.0, &children).is_infinite());
    assert_eq!(0.0, index_reader.GetCostLimit(10.0, &[]));
}

/// Go TableReader 用 reader.Schema 估算行宽，而 IndexReader 用子计划 Schema。
#[test]
fn readers_use_the_same_row_size_source_as_go() {
    let sctx = test_ctx();
    let child = child_impl(&sctx, 4.0);
    let children = [child];

    let table_reader = NewTableReaderImpl(
        Box::new(ReaderAccess {
            plan: PhysicalLimit::New(sctx.clone(), 0, 0),
            workers: 2,
        }),
        astersql_meta_model::TableInfo::default(),
        empty_histograms(),
    );
    assert_eq!(17.0, table_reader.CalcCost(5.0, &children));

    let index_reader = NewIndexReaderImpl(
        Box::new(ReaderAccess {
            plan: PhysicalLimit::New(sctx, 0, 0),
            workers: 2,
        }),
        astersql_meta_model::TableInfo::default(),
        empty_histograms(),
    );
    assert_eq!(37.0, index_reader.CalcCost(5.0, &children));
}

/// Go math.Min 会传播 NaN；Sort 的期望行数异常时不能悄悄退回子统计行数。
#[test]
fn sort_count_min_propagates_nan_like_go() {
    let sctx = test_ctx();
    let child = child_impl(&sctx, 0.0);
    let sort = NewSortImpl(Box::new(SortAccess {
        plan: PhysicalLimit::New(sctx, 0, 0),
        expected_count: f64::NAN,
    }));

    assert!(sort.CalcCost(0.0, &[child]).is_nan());
}
