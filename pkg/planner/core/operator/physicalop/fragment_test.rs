// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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
// Fragment 公开 API 的单元测试（对照 Go `fragment_test.go`）。
//
// Go 侧 `singleton` / `fillLocalCTECounts` 在本仓库 Rust 实现中已重新设计，
// 详见下方迁移说明；本文件只覆盖 `Fragment::New`、`MemoryUsage` 与 clone 共享 Sink。

// 本文件对应 pkg/planner/core/operator/physicalop/fragment_test.go。Go 版本的两个
// 测试直接操作 Go 里的具体结构体字段：`Fragment{}` 自带 `singleton bool` 字段和
// `init(p PhysicalPlan) error` 方法（依据根节点下两个 ExchangeReceiver 子节点的
// ExchangeType 是否都是 Broadcast 判断 singleton），以及包级私有类型
// `mppTaskGenerator` 上的 `fillLocalCTECounts([]*Fragment) error`（按 fragment
// self tasks 的地址统计 CTE sink/source 数量并写回 `PhysicalCTESink`/
// `PhysicalCTESource` 的 `CteSinkNum`/`CteSourceNum`）。
//
// 本仓库的 Rust `Fragment`（见 fragment.rs）是围绕 `RootMppTaskGenerator` trait 和
// `MPPSink` trait object 的完全重新设计：`Fragment` 只保存 `Sink: Arc<Mutex<Box<dyn
// MPPSink>>>` 和 `IsRoot: bool`，没有 `singleton` 字段、没有 `init` 方法；
// `SessionRootMppTaskGenerator`（`mppTaskGenerator` 的对应实现）也没有暴露
// `fillLocalCTECounts`，而是把“receiver 是否 PassThrough 时把自身任务收敛到 1
// 个”这条语义直接内联进 `build_sender`（见 fragment.rs 的
// `tasks.truncate(1)` 分支），CTE sink/source 计数也不是这条生产路径的一部分。
// 这两个 Go 私有 API 之间的差异是本任务 writes 清单之外的生产能力缺口，因此这里
// 不新增生产逻辑去凑一个字段对字段的假迁移，而是：
//   1. 以原始字符串保留 Go 源码供对照；
//   2. 针对 Rust `Fragment` 的真实公开 API（`Fragment::New`、`MemoryUsage`）写
//      直接的单元测试；PassThrough 截断到单任务、CTE 生产者去重复用等
//      `mppTaskGenerator` 语义的等价覆盖，已经在同目录 `fragment_aster_unit_test.rs`
//      的 `receiver_boundary_creates_child_fragment_and_links_tasks` 与
//      `shared_cte_readers_reuse_one_producer_fragment_and_deduplicate_targets`
//      两个测试里通过端到端 `GenerateRootMPPTasks` 验证，不在本文件重复。
#![allow(dead_code)]

/// 保留 Go `TestFragmentInitSingleton` / `TestFillLocalCTECountsUsesLocalTaskCounts` 源码对照。
const _GO_FRAGMENT_TEST_REFERENCE: &str = r########"
func TestFragmentInitSingleton(t *testing.T) {
	r1, r2 := &PhysicalExchangeReceiver{}, &PhysicalExchangeReceiver{}
	r1.SetChildren(&PhysicalExchangeSender{ExchangeType: tipb.ExchangeType_PassThrough})
	r2.SetChildren(&PhysicalExchangeSender{ExchangeType: tipb.ExchangeType_Broadcast})
	p := &PhysicalHashJoin{}

	f := &Fragment{}
	p.SetChildren(r1, r1)
	err := f.init(p)
	require.NoError(t, err)
	require.Equal(t, f.singleton, true)

	f = &Fragment{}
	p.SetChildren(r1, r2)
	err = f.init(p)
	require.NoError(t, err)
	require.Equal(t, f.singleton, true)

	f = &Fragment{}
	p.SetChildren(r2, r1)
	err = f.init(p)
	require.NoError(t, err)
	require.Equal(t, f.singleton, true)

	f = &Fragment{}
	p.SetChildren(r2, r2)
	err = f.init(p)
	require.NoError(t, err)
	require.Equal(t, f.singleton, false)
}

func TestFillLocalCTECountsUsesLocalTaskCounts(t *testing.T) {
	task := func(addr string) *kv.MPPTask {
		return &kv.MPPTask{Meta: &mppAddr{addr: addr}}
	}

	// This models a CTE producer with UNION ALL split into two CTESink fragments:
	// one sink runs on tiflash0, the other runs on tiflash1, and both CTE consumers
	// run on both TiFlash nodes. Each TiFlash address should see one local sink
	// and two local sources for the CTE.
	sink0 := &PhysicalCTESink{IDForStorage: 1}
	sink0.SetSelfTasks([]*kv.MPPTask{task("tiflash0")})
	sink1 := &PhysicalCTESink{IDForStorage: 1}
	sink1.SetSelfTasks([]*kv.MPPTask{task("tiflash1")})

	source0 := &PhysicalCTESource{IDForStorage: 1}
	sourceSink0 := &PhysicalExchangeSender{}
	sourceSink0.SetChildren(source0)
	sourceSink0.SetSelfTasks([]*kv.MPPTask{task("tiflash0"), task("tiflash1")})
	source1 := &PhysicalCTESource{IDForStorage: 1}
	sourceSink1 := &PhysicalExchangeSender{}
	sourceSink1.SetChildren(source1)
	sourceSink1.SetSelfTasks([]*kv.MPPTask{task("tiflash0"), task("tiflash1")})

	frags := []*Fragment{
		{Sink: sink0},
		{Sink: sink1},
		{Sink: sourceSink0},
		{Sink: sourceSink1},
	}
	err := (&mppTaskGenerator{}).fillLocalCTECounts(frags)
	require.NoError(t, err)

	for _, sink := range []*PhysicalCTESink{sink0, sink1} {
		require.Equal(t, uint32(1), sink.CteSinkNum)
		require.Equal(t, uint32(2), sink.CteSourceNum)
	}
	for _, source := range []*PhysicalCTESource{source0, source1} {
		require.Equal(t, uint32(1), source.CteSinkNum)
		require.Equal(t, uint32(2), source.CteSourceNum)
	}
}
"########;

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::Plan;
use tipb;

use crate::{AllocMPPQueryID, Fragment, PhysicalExchangeSender};

/// Go `AllocMPPQueryID` 从初值 1 开始原子递增，因此首次返回 2，且后续严格递增。
#[test]
fn alloc_mpp_query_id_matches_go_monotonic_sequence() {
    let first = AllocMPPQueryID();
    let second = AllocMPPQueryID();

    assert!(first >= 2);
    assert_eq!(second, first + 1);
}

/// Go `TestFragmentInitSingleton` 的 Rust 等价测试：PassThrough 接收端使
/// fragment 进入 singleton，而两个 Broadcast 接收端不会使其 singleton。
#[test]
fn fragment_init_tracks_pass_through_singleton_semantics() {
    let ctx = context();
    let mut pass_through = PhysicalExchangeSender::New(ctx.clone());
    pass_through.ExchangeType = tipb::ExchangeType::PassThrough;
    let pass_through = Box::new(pass_through);
    let mut broadcast = PhysicalExchangeSender::New(ctx.clone());
    broadcast.ExchangeType = tipb::ExchangeType::Broadcast;
    let broadcast = Box::new(broadcast);

    let mut receiver1 = crate::PhysicalExchangeReceiver::New(ctx.clone());
    receiver1
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![pass_through]);
    let mut pass_through_receiver = crate::PhysicalExchangeReceiver::New(ctx.clone());
    pass_through_receiver
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(PhysicalExchangeSender::New(ctx.clone()))]);
    let mut broadcast_receiver1 = crate::PhysicalExchangeReceiver::New(ctx.clone());
    broadcast_receiver1
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![broadcast]);
    let mut broadcast_receiver2 = crate::PhysicalExchangeReceiver::New(ctx.clone());
    let mut broadcast2 = PhysicalExchangeSender::New(ctx.clone());
    broadcast2.ExchangeType = tipb::ExchangeType::Broadcast;
    broadcast_receiver2
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(broadcast2)]);

    let mut plan = crate::PhysicalLimit::New(ctx.clone(), 0, 0);
    plan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(receiver1), Box::new(pass_through_receiver)]);
    let mut fragment = Fragment::New(Box::new(PhysicalExchangeSender::New(context())), false);
    fragment.init(&plan).expect("fragment init");
    assert!(fragment.singleton);

    let mut plan = crate::PhysicalLimit::New(ctx, 0, 0);
    plan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![
            Box::new(broadcast_receiver1),
            Box::new(broadcast_receiver2),
        ]);
    let mut fragment = Fragment::New(Box::new(PhysicalExchangeSender::New(context())), false);
    fragment.init(&plan).expect("fragment init");
    assert!(!fragment.singleton);
}

/// 仅分配计划 ID 的测试 PlanContext；访问会话等上下文会 abort/panic。
struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("fragment identity test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 验证 New 正确包装 Sink，并区分根/非根片段的 IsRoot 标志。
#[test]
fn fragment_new_wraps_sink_and_reports_root_flag() {
    let ctx = context();
    let sender = PhysicalExchangeSender::New(ctx);

    let root = Fragment::New(Box::new(sender), true);
    assert!(root.IsRoot);
    {
        let sink = root.Sink.lock().expect("MPP sink lock");
        assert!(
            sink.as_any()
                .downcast_ref::<PhysicalExchangeSender>()
                .is_some()
        );
    }

    let ctx = context();
    let child_sender = PhysicalExchangeSender::New(ctx);
    let child = Fragment::New(Box::new(child_sender), false);
    assert!(!child.IsRoot);
}

/// 验证 MemoryUsage 会委托给内部 Sink，装载任务后严格大于空 Sink。
#[test]
fn fragment_memory_usage_tracks_sink_state_and_struct_size() {
    let ctx = context();
    let empty_sender = PhysicalExchangeSender::New(ctx.clone());
    let empty = Fragment::New(Box::new(empty_sender), false);
    let baseline = empty.MemoryUsage();
    assert!(baseline >= std::mem::size_of::<Fragment>() as i64);

    let mut loaded_sender = PhysicalExchangeSender::New(ctx);
    loaded_sender.SetSelfTasks(vec![kv::MPPTask::default(); 4]);
    let loaded = Fragment::New(Box::new(loaded_sender), false);
    // 装载了 self tasks 的 sink 内存占用必须严格大于空 sink，
    // 说明 `Fragment::MemoryUsage` 确实委托给了内部 `MPPSink::memory_usage`
    // 而不是只统计固定的结构体大小。
    assert!(loaded.MemoryUsage() > baseline);
}

/// 验证 clone 共享同一 Sink Arc，对克隆体的写操作对原体可见。
#[test]
fn fragment_clone_shares_the_same_underlying_sink() {
    let ctx = context();
    let sender = PhysicalExchangeSender::New(ctx);
    let original = Fragment::New(Box::new(sender), true);
    let cloned = original.clone();

    assert!(Arc::ptr_eq(&original.Sink, &cloned.Sink));
    cloned
        .Sink
        .lock()
        .expect("MPP sink lock")
        .set_self_tasks(vec![kv::MPPTask::default()]);
    assert_eq!(
        original
            .Sink
            .lock()
            .expect("MPP sink lock")
            .get_self_tasks()
            .len(),
        1
    );
}
