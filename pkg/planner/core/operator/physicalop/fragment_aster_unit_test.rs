// Copyright 2026 AsterSQL.

// `SessionRootMppTaskGenerator` 端到端单元测试。
//
// 覆盖：根片段任务与 KV ranges 构造、Receiver 边界拆分子片段并链接
// TargetTasks、共享 CTE 读者复用同一生产者片段并去重扫描。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::time::Duration;

use base::{PhysicalPlan, Plan};

use crate::{
    EncodedMppScan, MppScanRangeEncoder, PhysicalExchangeReceiver, PhysicalExchangeSender,
    PhysicalTableScan, RootMppTaskGenerator, SessionRootMppTaskGenerator,
};

/// 仅分配计划 ID 的测试 PlanContext。
struct TestContext(AtomicI32, base::BuiltinFunctionUsageCounter);
impl base::PlanContext for TestContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("unused")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("fragment test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 固定地址的假 MPPTaskMeta。
#[derive(Clone)]
struct Meta(&'static str);
impl kv::MPPTaskMeta for Meta {
    fn GetAddress(&self) -> String {
        self.0.to_owned()
    }
    fn CloneBox(&self) -> Box<dyn kv::MPPTaskMeta> {
        Box::new(self.clone())
    }
}

/// 计数 `ConstructMPPTasks` 调用次数的假 MPPClient。
struct TestClient(AtomicUsize);
impl kv::MPPClient for TestClient {
    fn ConstructMPPTasks(
        &self,
        _: &kv::Context,
        _: &kv::MPPBuildTasksRequest,
        _: Duration,
        _: kv::tiflashcompute::DispatchPolicy,
        _: kv::tiflash::ReplicaRead,
        _: &mut dyn FnMut(kv::Error),
    ) -> Result<Vec<Box<dyn kv::MPPTaskMeta>>, kv::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Box::new(Meta("tiflash-1:3930"))])
    }
    fn DispatchMPPTask(
        &self,
        _: kv::DispatchMPPTaskParam<'_>,
    ) -> Result<(kv::DispatchTaskResponse, bool), kv::Error> {
        unreachable!()
    }
    fn EstablishMPPConns(
        &self,
        _: kv::EstablishMPPConnsParam<'_>,
    ) -> Result<(kv::MPPStreamResponse, bool), kv::Error> {
        unreachable!()
    }
    fn CancelMPPTasks(&self, _: kv::CancelMPPTasksParam) {
        unreachable!()
    }
    fn CheckVisibility(&self, _: u64) -> Result<(), kv::Error> {
        unreachable!()
    }
    fn GetMPPStoreCount(&self) -> Result<i32, kv::Error> {
        Ok(1)
    }
}

/// 把扫描编码为单条 KeyRange 的假 range encoder。
struct RangeEncoder;
impl MppScanRangeEncoder for RangeEncoder {
    fn Encode(&self, scan: &PhysicalTableScan) -> Result<EncodedMppScan, expression::Error> {
        let range = kv::KeyRange::default();
        Ok(EncodedMppScan {
            request: kv::MPPBuildTasksRequest {
                KeyRanges: Some(vec![range.clone()]),
                StartTS: 7,
                PartitionIDAndRanges: Vec::new(),
            },
            kv_ranges: vec![range],
            table_id: scan.PhysicalTableID,
            tiflash_static_prune: false,
        })
    }
}

/// 用给定 client 组装默认会话参数的生成器。
fn generator(client: Arc<TestClient>) -> SessionRootMppTaskGenerator {
    SessionRootMppTaskGenerator::New(
        client,
        kv::Context::todo(),
        Arc::new(RangeEncoder),
        Duration::ZERO,
        kv::tiflashcompute::DispatchPolicy::default(),
        kv::tiflash::ReplicaRead::default(),
        kv::MppVersionV2,
        9,
        "session".to_owned(),
    )
}

/// 创建指向 TiFlash 的表扫描，并设置物理表 ID。
fn scan(ctx: base::ContextRef, table_id: i64) -> PhysicalTableScan {
    let mut scan = PhysicalTableScan::New(ctx);
    scan.PhysicalTableID = table_id;
    scan.StoreType = kv::StoreType::TiFlash;
    scan
}

/// 根 Sender+Scan：应构造 1 个片段、非空 ranges，且根 TargetTasks ID 为 -1。
#[test]
fn generator_constructs_tasks_ranges_and_root_target() {
    let client = Arc::new(TestClient(AtomicUsize::new(0)));
    let ctx = context();
    let mut sender = PhysicalExchangeSender::New(ctx.clone());
    PhysicalPlan::set_children(&mut sender, vec![Box::new(scan(ctx, 42))]);
    let generated = generator(client.clone())
        .GenerateRootMPPTasks(&sender, 7, 8, kv::MPPQueryID::default())
        .expect("root tasks");

    assert_eq!(client.0.load(Ordering::SeqCst), 1);
    assert_eq!(generated.fragments.len(), 1);
    assert_eq!(generated.kv_ranges.len(), 1);
    assert!(generated.node_addresses.contains("tiflash-1:3930"));
    let sink_guard = generated.fragments[0].Sink.lock().unwrap();
    let sink = sink_guard
        .as_any()
        .downcast_ref::<PhysicalExchangeSender>()
        .unwrap();
    assert_eq!(sink.Tasks.len(), 1);
    assert_eq!(sink.TargetTasks[0].ID, -1);
}

/// Receiver 边界应拆出子片段，并把父层任务地址写回子 Sink 的 TargetTasks。
#[test]
fn receiver_boundary_creates_child_fragment_and_links_tasks() {
    let client = Arc::new(TestClient(AtomicUsize::new(0)));
    let ctx = context();
    let mut child_sender = PhysicalExchangeSender::New(ctx.clone());
    PhysicalPlan::set_children(&mut child_sender, vec![Box::new(scan(ctx.clone(), 42))]);
    let mut receiver = PhysicalExchangeReceiver::New(ctx.clone());
    PhysicalPlan::set_children(&mut receiver, vec![Box::new(child_sender)]);
    let receiver_tasks = receiver.Tasks();
    assert!(receiver_tasks.is_empty());
    let mut root_sender = PhysicalExchangeSender::New(ctx);
    PhysicalPlan::set_children(&mut root_sender, vec![Box::new(receiver)]);

    let generated = generator(client)
        .GenerateRootMPPTasks(&root_sender, 7, 8, kv::MPPQueryID::default())
        .expect("two fragments");
    assert_eq!(generated.fragments.len(), 2);
    let child_guard = generated.fragments[1].Sink.lock().unwrap();
    let child = child_guard
        .as_any()
        .downcast_ref::<PhysicalExchangeSender>()
        .unwrap();
    assert_eq!(child.TargetTasks.len(), 1);
    assert_eq!(
        child.TargetTasks[0].Meta.as_ref().unwrap().GetAddress(),
        "tiflash-1:3930"
    );
}

/// 两个共享同一 producer plan ID 的 CTE 读者只调度一次扫描，
/// 但 AppendTargetTasks 与 Go 一致保留每个读者的目标任务。
#[test]
fn shared_cte_readers_reuse_one_producer_fragment_and_preserve_targets() {
    let client = Arc::new(TestClient(AtomicUsize::new(0)));
    let ctx = context();
    let receiver = |ctx: base::ContextRef| {
        let mut child_sender = PhysicalExchangeSender::New(ctx.clone());
        child_sender.set_id(77);
        PhysicalPlan::set_children(&mut child_sender, vec![Box::new(scan(ctx.clone(), 42))]);
        let mut receiver = PhysicalExchangeReceiver::New(ctx);
        PhysicalPlan::set_children(&mut receiver, vec![Box::new(child_sender)]);
        receiver
    };
    let mut root_sender = PhysicalExchangeSender::New(ctx.clone());
    PhysicalPlan::set_children(
        &mut root_sender,
        vec![Box::new(receiver(ctx.clone())), Box::new(receiver(ctx))],
    );

    let generated = generator(client.clone())
        .GenerateRootMPPTasks(&root_sender, 7, 8, kv::MPPQueryID::default())
        .expect("shared CTE producer");
    assert_eq!(client.0.load(Ordering::SeqCst), 1);
    assert_eq!(generated.fragments.len(), 2);
    let child_guard = generated.fragments[1].Sink.lock().unwrap();
    let child = child_guard
        .as_any()
        .downcast_ref::<PhysicalExchangeSender>()
        .unwrap();
    assert_eq!(child.TargetTasks.len(), 2);
}
