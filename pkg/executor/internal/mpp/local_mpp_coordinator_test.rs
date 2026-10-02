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

// 对应 `pkg/executor/internal/mpp/local_mpp_coordinator_test.go`：验证 MPP execution
// summary 上报判断 `need_report_execution_summary` 与 zone quick-fill 分支
// `TaskZoneInfoHelper::try_quick_fill_with_uncertain_zones`，均直接调用 crate 内部真实实现。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use astersql_planner_core_base::{ContextRef, PhysicalPlan, Plan};
use astersql_planner_core_operator_physicalop::{
    PhysicalExchangeSender, PhysicalHashJoin, PhysicalLimit, PhysicalProjection,
    PhysicalTableReader, PhysicalTableScan,
};

use crate::local_mpp_coordinator::{TaskZoneInfoHelper, need_report_execution_summary};

/// 最小 PlanContext：只分配 plan id 与内置函数计数，其余接口 panic。
struct TestPlanContext(
    AtomicI32,
    astersql_planner_core_base::BuiltinFunctionUsageCounter,
);

impl astersql_planner_core_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &astersql_planner_planctx::variable::SessionVars {
        panic!("local mpp coordinator plan-shape test does not read session variables")
    }

    fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("local mpp coordinator plan-shape test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &astersql_planner_planctx::rangerctx::RangerContext<'_> {
        panic!("local mpp coordinator plan-shape test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("local mpp coordinator plan-shape test does not evaluate null rejection")
    }

    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        panic!("local mpp coordinator plan-shape test does not build protobuf")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

/// 构造测试用 `ContextRef`，plan id 自 0 递增。
fn plan_context() -> ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 对应 `TestNeedReportExecutionSummary`，验证 exchange sender ID 与计划形状的过滤规则。
/// 全部使用生产 `PhysicalPlan` 节点（`PhysicalTableScan`/`PhysicalLimit`/
/// `PhysicalExchangeSender`/`PhysicalTableReader`/`PhysicalProjection`/`PhysicalHashJoin`），
/// 与 Go 源文件用真实 `physicalop` 节点构造计划树保持一致。
#[test]
fn test_need_report_execution_summary() {
    let context = plan_context();

    let table_scan = PhysicalTableScan::New(context.clone());
    let mut limit = PhysicalLimit::New(context.clone(), 0, 0);
    PhysicalPlan::set_children(&mut limit, vec![Box::new(table_scan)]);

    let mut pass_sender = PhysicalExchangeSender::New(context.clone());
    pass_sender.set_id(10);
    PhysicalPlan::set_children(&mut pass_sender, vec![Box::new(limit)]);

    let mut table_reader = PhysicalTableReader::New(context.clone());
    table_reader.SetTablePlanForTest(Box::new(pass_sender));

    let mut limit_tidb = PhysicalLimit::New(context.clone(), 0, 0);
    PhysicalPlan::set_children(&mut limit_tidb, vec![Box::new(table_reader)]);

    assert!(need_report_execution_summary(&limit_tidb, 10, false));
    assert!(!need_report_execution_summary(&limit_tidb, 11, false));

    // 重新构造同一棵计划树：Go 测试复用 tableReader，这里改为重新搭建等价节点，
    // 因为上面的 limit_tidb 已经拿走了 table_reader 的所有权。
    let table_scan_for_projection = PhysicalTableScan::New(context.clone());
    let mut limit_for_projection = PhysicalLimit::New(context.clone(), 0, 0);
    PhysicalPlan::set_children(
        &mut limit_for_projection,
        vec![Box::new(table_scan_for_projection)],
    );
    let mut pass_sender_for_projection = PhysicalExchangeSender::New(context.clone());
    pass_sender_for_projection.set_id(10);
    PhysicalPlan::set_children(
        &mut pass_sender_for_projection,
        vec![Box::new(limit_for_projection)],
    );
    let mut table_reader_for_projection = PhysicalTableReader::New(context.clone());
    table_reader_for_projection.SetTablePlanForTest(Box::new(pass_sender_for_projection));

    // Projection 包住 table reader 时，Go 期望不直接上报该 exchange sender 的 summary。
    let mut projection = PhysicalProjection::New(context.clone());
    PhysicalPlan::set_children(&mut projection, vec![Box::new(table_reader_for_projection)]);
    assert!(!need_report_execution_summary(&projection, 10, false));

    let mut table_scan2 = PhysicalTableScan::New(context.clone());
    table_scan2.set_id(20);
    let mut table_reader2 = PhysicalTableReader::New(context.clone());
    table_reader2.SetTablePlanForTest(Box::new(table_scan2));

    let join = hash_join(
        context.clone(),
        vec![Box::new(table_reader2), Box::new(projection)],
    );
    let mut limit_tidb2 = PhysicalLimit::New(context, 0, 0);
    PhysicalPlan::set_children(&mut limit_tidb2, vec![Box::new(join)]);
    assert!(need_report_execution_summary(&limit_tidb2, 10, false));
}

/// 构造一个只填充了 join type 与子节点的 `PhysicalHashJoin`，与 Go 测试里
/// `join := &physicalop.PhysicalHashJoin{}` 后仅 `SetChildren` 的用法对齐。
fn hash_join(context: ContextRef, children: Vec<Box<dyn PhysicalPlan>>) -> PhysicalHashJoin {
    let producer = astersql_planner_core_operator_physicalop::PhysicalSchemaProducer::New(
        astersql_planner_core_operator_physicalop::NewBasePhysicalPlan(context, "HashJoin", 0),
    );
    let base = astersql_planner_core_operator_physicalop::BasePhysicalJoin::New(
        producer,
        astersql_planner_core_base::JoinType::InnerJoin,
    );
    let mut join = astersql_planner_core_operator_physicalop::NewPhysicalHashJoin(base, 1, false);
    PhysicalPlan::set_children(&mut join, children);
    join
}

/// 对应 Go 的 `mockTaskZoneInfoHelper`，把 store 地址到 zone 的 map 转成真实
/// `TaskZoneInfoHelper`；本 crate 的生产实现已将逐 executor 展开为
/// `(executor_id, is_exchange_sender, slots)`，因此不再需要构造 `tipb.Executor`。
fn mock_task_zone_info_helper(
    is_root: bool,
    task_zone: &str,
    tidb_zone: &str,
) -> TaskZoneInfoHelper {
    let mut helper = TaskZoneInfoHelper::new(HashMap::new(), tidb_zone.to_owned());
    helper.set_fragment(is_root, task_zone.to_owned());
    helper
}

/// 对应 `TestZoneHelperTryQuickFill`，覆盖 task zone 为空、root sender、非 sender 三类分支。
#[test]
fn test_zone_helper_try_quick_fill() {
    let mut slots = 3_usize;
    let helper = mock_task_zone_info_helper(false, "", "east");

    // task zone 为空时，Go 逻辑快速返回 true，并为每个 slot 填 true。
    let flags = helper
        .try_quick_fill_with_uncertain_zones("ExchangeSender_1", true, slots)
        .expect("quick fill applies when task zone is empty");
    assert_eq!(slots, flags.len());
    assert!(flags.iter().all(|flag| *flag));

    // root task 且 executor 是 exchange sender 时，比较 tidbZone 与 currentTaskZone。
    let mut helper = mock_task_zone_info_helper(true, "west", "east");
    slots = 1;
    let flags = helper
        .try_quick_fill_with_uncertain_zones("ExchangeSender_1", true, slots)
        .expect("root exchange sender always quick fills");
    assert_eq!(slots, flags.len());
    assert!(flags.iter().all(|flag| !*flag));

    helper.set_fragment(true, "east".to_owned());
    let flags = helper
        .try_quick_fill_with_uncertain_zones("ExchangeSender_1", true, slots)
        .expect("root exchange sender in the same zone quick fills true");
    assert_eq!(slots, flags.len());
    assert!(flags.iter().all(|flag| *flag));

    // 非 root exchange sender 且 current task zone 非空时，Go 返回 false，不填 flags。
    let mut helper = mock_task_zone_info_helper(false, "west", "east");
    slots = 3;
    assert!(
        helper
            .try_quick_fill_with_uncertain_zones("ExchangeSender_1", true, slots)
            .is_none()
    );

    // root task 但 executor 是 exchange receiver，也不走 quick fill。
    helper.set_fragment(true, "west".to_owned());
    assert!(
        helper
            .try_quick_fill_with_uncertain_zones("ExchangeReceiver_2", false, slots)
            .is_none()
    );
}

#[derive(Default)]
struct DispatchRaceTransport {
    dispatched: std::sync::Mutex<Vec<i64>>,
    cancelled_stores: std::sync::Mutex<Vec<HashMap<String, bool>>>,
}

impl crate::local_mpp_coordinator::CoordinatorTransport for DispatchRaceTransport {
    fn Dispatch(
        &self,
        _: &astersql_kv::Context,
        request: &astersql_kv::MPPDispatchRequest,
    ) -> Result<
        Option<Box<dyn crate::local_mpp_coordinator::CoordinatorResponseStream>>,
        astersql_errors::SharedError,
    > {
        self.dispatched.lock().unwrap().push(request.ID);
        Ok(None)
    }

    fn Cancel(
        &self,
        _: &astersql_kv::Context,
        stores: HashMap<String, bool>,
        _: &[astersql_kv::MPPDispatchRequest],
    ) -> Result<(), astersql_errors::SharedError> {
        self.cancelled_stores.lock().unwrap().push(stores);
        Ok(())
    }

    fn CheckVisibility(&self, _: u64) -> Result<(), astersql_errors::SharedError> {
        Ok(())
    }
}

#[derive(Clone)]
struct DispatchRaceMeta;

impl astersql_kv::MPPTaskMeta for DispatchRaceMeta {
    fn GetAddress(&self) -> String {
        "tiflash:3930".to_owned()
    }
    fn CloneBox(&self) -> Box<dyn astersql_kv::MPPTaskMeta> {
        Box::new(self.clone())
    }
}

fn dispatch_race_coordinator(
    transport: Arc<DispatchRaceTransport>,
    state: astersql_kv::MppTaskStates,
) -> crate::local_mpp_coordinator::LocalMppCoordinator {
    let mut coordinator = crate::local_mpp_coordinator::new_local_mpp_coordinator(
        transport,
        Box::new(PhysicalExchangeSender::New(plan_context())),
        None,
        vec![10],
        11,
        astersql_kv::MPPQueryID::default(),
        12,
        "tidb:4000".to_owned(),
        astersql_kv::MppVersionV2,
        vec![astersql_kv::KeyRange::default()],
        1,
        Arc::new(crate::NoopMppReportSink),
        std::time::Duration::from_millis(1),
    );
    coordinator.install_request(astersql_kv::MPPDispatchRequest {
        ID: 1,
        Meta: Some(Box::new(DispatchRaceMeta)),
        State: state,
        ..Default::default()
    });
    coordinator
}

#[test]
fn test_dispatch_cancel_race_skips_non_ready_tasks() {
    use astersql_kv::{MppCoordinator, MppTaskStates, Response};
    for state in [
        MppTaskStates::MppTaskCancelled,
        MppTaskStates::MppTaskDone,
        MppTaskStates::MppTaskRunning,
    ] {
        let transport = Arc::new(DispatchRaceTransport::default());
        let mut coordinator = dispatch_race_coordinator(transport.clone(), state);
        let context = astersql_kv::Context::todo();
        coordinator.Execute(&context).unwrap();
        assert!(coordinator.Next(&context).unwrap().is_none());
        assert!(
            transport.dispatched.lock().unwrap().is_empty(),
            "state {state:?} was dispatched"
        );
        coordinator.Close().unwrap();
    }
}

#[test]
fn test_dispatch_cancel_race_dispatch_wins_includes_store() {
    use astersql_kv::{MppCoordinator, MppTaskStates, Response};
    let transport = Arc::new(DispatchRaceTransport::default());
    let coordinator = std::sync::Mutex::new(dispatch_race_coordinator(
        transport.clone(),
        MppTaskStates::MppTaskReady,
    ));
    let mut guard = coordinator.lock().unwrap();
    guard.Execute(&astersql_kv::Context::todo()).unwrap();
    guard.Close().unwrap();
    assert_eq!(*transport.dispatched.lock().unwrap(), vec![1]);
    assert_eq!(
        transport.cancelled_stores.lock().unwrap().as_slice(),
        &[HashMap::from([("tiflash:3930".to_owned(), true)])]
    );
}

#[test]
fn test_dispatch_cancel_race_cancel_wins_under_coordinator_lock() {
    use astersql_kv::{MppCoordinator, MppTaskStates, Response};
    let transport = Arc::new(DispatchRaceTransport::default());
    let coordinator = Arc::new(std::sync::Mutex::new(dispatch_race_coordinator(
        transport.clone(),
        MppTaskStates::MppTaskReady,
    )));
    let mut guard = coordinator.lock().unwrap();
    let (started, waiting) = std::sync::mpsc::channel();
    let dispatch_coordinator = coordinator.clone();
    let dispatch = std::thread::spawn(move || {
        started.send(()).unwrap();
        dispatch_coordinator
            .lock()
            .unwrap()
            .Execute(&astersql_kv::Context::todo())
    });
    waiting.recv().unwrap();
    guard.Close().unwrap();
    drop(guard);
    assert!(dispatch.join().unwrap().is_err());
    assert!(transport.dispatched.lock().unwrap().is_empty());
    assert!(transport.cancelled_stores.lock().unwrap()[0].is_empty());
}
