// Copyright 2026 AsterSQL.

// MPP 协调器派发准备逻辑的单元测试。
//
// 覆盖：TiFlash store 信息收集、Exchange same-zone 标志填充、
// `need_report_execution_summary` 计划形状判断，以及
// `prepare_dispatch_requests_from_root` 对 DAG/会话字段的序列化。

use std::collections::HashMap;

use kvproto::mpp::TaskMeta;
use protobuf::Message;

use astersql_planner_core_base::{PhysicalPlan, Plan};
use astersql_planner_core_operator_physicalop::{
    PhysicalExchangeSender, PhysicalLimit, PhysicalTableReader,
};

use crate::local_mpp_coordinator::{
    DispatchSessionInfo, TaskZoneInfoHelper, TiFlashStore, TiFlashStoreInfo,
    add_tiflash_store_info, need_report_execution_summary, prepare_dispatch_requests_from_root,
};

/// 计划遍历测试用的最小 PlanContext 夹具。
mod plan_fixture {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// 只分配 plan id 与内置函数计数；读会话/表达式会 panic。
    pub(super) struct Context(
        AtomicI32,
        astersql_planner_core_base::BuiltinFunctionUsageCounter,
    );

    impl astersql_planner_core_base::PlanContext for Context {
        fn alloc_plan_id(&self) -> i32 {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }

        fn ignore_explain_id_suffix(&self) -> bool {
            false
        }

        fn GetSessionVars(&self) -> &astersql_planner_planctx::variable::SessionVars {
            panic!("plan traversal test does not read session variables")
        }

        fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
            panic!("plan traversal test does not evaluate expressions")
        }

        fn GetRangerCtx(&self) -> &astersql_planner_planctx::rangerctx::RangerContext<'_> {
            panic!("plan traversal test does not build ranges")
        }

        fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
            panic!("plan traversal test does not evaluate null rejection")
        }

        fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
            panic!("plan traversal test does not build protobuf")
        }

        fn BuiltinFunctionUsageInc(&self, name: &str) {
            self.1.Inc(name)
        }
    }

    /// 构造自增 plan id 的 ContextRef。
    pub(super) fn context() -> astersql_planner_core_base::ContextRef {
        Arc::new(Context(
            AtomicI32::new(0),
            astersql_planner_core_base::BuiltinFunctionUsageCounter::default(),
        ))
    }
}

/// 测试用 TiFlashStore：固定 StoreID/Address/zone 标签。
struct TestStore {
    id: u64,
    address: String,
    zone: Option<String>,
}

impl TiFlashStore for TestStore {
    fn StoreID(&self) -> u64 {
        self.id
    }

    fn Address(&self) -> &str {
        &self.address
    }

    fn LabelValue(&self, key: &str) -> Option<&str> {
        (key == astersql_ddl_placement::DCLabelKey)
            .then(|| self.zone.as_deref())
            .flatten()
    }
}

/// 仅携带 store 地址的 MPPTaskMeta。
#[derive(Clone)]
struct TaskAddress(String);

impl astersql_kv::MPPTaskMeta for TaskAddress {
    fn GetAddress(&self) -> String {
        self.0.clone()
    }

    fn CloneBox(&self) -> Box<dyn astersql_kv::MPPTaskMeta> {
        Box::new(self.clone())
    }
}

/// 将地址编码为 TaskMeta protobuf 字节。
fn encoded_task(address: &str) -> Vec<u8> {
    let mut task = TaskMeta::new();
    task.set_address(address.to_owned());
    task.write_to_bytes().expect("task meta encodes")
}

/// `add_tiflash_store_info` 按地址写入 store_id 与 DC zone。
#[test]
fn store_collection_preserves_address_id_and_zone() {
    let mut stores = HashMap::new();
    add_tiflash_store_info(
        &mut stores,
        &TestStore {
            id: 19,
            address: "tiflash-1:3930".to_owned(),
            zone: Some("zone-a".to_owned()),
        },
    );

    assert_eq!(
        stores["tiflash-1:3930"],
        TiFlashStoreInfo {
            zone: "zone-a".to_owned(),
            store_id: 19
        }
    );
}

/// ExchangeSender 目标：同 zone 为 true，异 zone 为 false，未知地址视为同 zone。
#[test]
fn exchange_flags_follow_go_same_zone_and_unknown_zone_rules() {
    let stores = HashMap::from([
        (
            "tiflash-a:3930".to_owned(),
            TiFlashStoreInfo {
                zone: "zone-a".to_owned(),
                store_id: 1,
            },
        ),
        (
            "tiflash-b:3930".to_owned(),
            TiFlashStoreInfo {
                zone: "zone-b".to_owned(),
                store_id: 2,
            },
        ),
    ]);
    let mut helper = TaskZoneInfoHelper::new(stores, "tidb-zone".to_owned());
    helper.set_fragment(false, "zone-a".to_owned());

    let mut sender = tipb::ExchangeSender::new();
    sender.set_encoded_task_meta(
        vec![
            encoded_task("tiflash-a:3930"),
            encoded_task("tiflash-b:3930"),
            encoded_task("missing:3930"),
        ]
        .into(),
    );
    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeExchangeSender);
    executor.set_executor_id("sender-1".to_owned());
    executor.set_exchange_sender(sender);

    helper
        .fill_same_zone_flag_for_exchange(&mut executor)
        .expect("known exchange protocol");
    assert_eq!(
        executor.get_exchange_sender().get_same_zone_flag(),
        &[true, false, true]
    );
}

/// ExchangeReceiver 与 Go 一样，也要按来源 task 的 zone 填充标志。
#[test]
fn exchange_receiver_flags_follow_go_source_zone_rules() {
    let stores = HashMap::from([
        (
            "tiflash-a:3930".to_owned(),
            TiFlashStoreInfo {
                zone: "zone-a".to_owned(),
                store_id: 1,
            },
        ),
        (
            "tiflash-b:3930".to_owned(),
            TiFlashStoreInfo {
                zone: "zone-b".to_owned(),
                store_id: 2,
            },
        ),
    ]);
    let mut helper = TaskZoneInfoHelper::new(stores, "tidb-zone".to_owned());
    helper.set_fragment(false, "zone-a".to_owned());

    let mut receiver = tipb::ExchangeReceiver::new();
    receiver.set_encoded_task_meta(
        vec![
            encoded_task("tiflash-a:3930"),
            encoded_task("tiflash-b:3930"),
            encoded_task("missing:3930"),
        ]
        .into(),
    );
    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeExchangeReceiver);
    executor.set_executor_id("receiver-1".to_owned());
    executor.set_exchange_receiver(receiver);

    helper
        .fill_same_zone_flag_for_exchange(&mut executor)
        .expect("known exchange protocol");
    assert_eq!(
        executor.get_exchange_receiver().get_same_zone_flag(),
        &[true, false, true]
    );
}

/// Root ExchangeSender 比较当前 TiFlash zone 与 TiDB zone。
#[test]
fn root_sender_compares_the_current_tiflash_zone_with_tidb() {
    let mut helper = TaskZoneInfoHelper::new(HashMap::new(), "zone-a".to_owned());
    helper.set_fragment(true, "zone-b".to_owned());
    let mut sender = tipb::ExchangeSender::new();
    sender.set_encoded_task_meta(vec![encoded_task("tidb"), encoded_task("tidb-2")].into());
    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeExchangeSender);
    executor.set_executor_id("root-sender".to_owned());
    executor.set_exchange_sender(sender);

    helper
        .fill_same_zone_flag_for_exchange(&mut executor)
        .expect("root sender fills one TiDB target flag");
    assert_eq!(
        executor.get_exchange_sender().get_same_zone_flag(),
        &[false]
    );
}

/// 未知执行器类型应保持 Go 的告警后继续行为，不阻断 DAG 编码。
#[test]
fn unknown_executor_type_is_ignored_by_zone_flag_fill() {
    let mut helper = TaskZoneInfoHelper::new(HashMap::new(), String::new());
    let mut executor = tipb::Executor::new();
    executor.set_tp(tipb::ExecType::TypeKill);
    helper
        .fill_same_zone_flag_for_exchange(&mut executor)
        .expect("unknown executor is non-fatal in Go");
}

/// 仅当 Limit 下方 TableReader 的 table plan id 匹配时才上报 summary。
#[test]
fn report_summary_requires_limit_and_matching_table_plan_id() {
    let context = plan_fixture::context();
    let mut sender = PhysicalExchangeSender::New(context.clone());
    sender.set_id(10);
    let mut reader = PhysicalTableReader::New(context.clone());
    reader.SetTablePlanForTest(Box::new(sender));
    let mut limit = PhysicalLimit::New(context, 0, 1);
    PhysicalPlan::set_children(&mut limit, vec![Box::new(reader)]);

    assert!(need_report_execution_summary(&limit, 10, false));
    assert!(!need_report_execution_summary(&limit, 11, false));
    let reader = limit
        .children()
        .into_iter()
        .next()
        .expect("limit child exists");
    assert!(!need_report_execution_summary(reader, 10, false));
}

/// prepare_dispatch_requests_from_root：写分区 table_id、会话字段并序列化 DAG。
#[test]
fn dispatch_request_preparation_updates_partition_and_serializes_real_dag() {
    let mut table_scan = tipb::TableScan::new();
    table_scan.set_table_id(1);
    let mut root = tipb::Executor::new();
    root.set_tp(tipb::ExecType::TypeTableScan);
    root.set_tbl_scan(table_scan);

    let tasks = vec![astersql_kv::MPPTask {
        Meta: Some(Box::new(TaskAddress("tiflash-a:3930".to_owned()))),
        ID: 41,
        StartTs: 43,
        GatherID: 47,
        TableID: 53,
        MppVersion: astersql_kv::MppVersionV3,
        SessionID: 59,
        SessionAlias: "task-session".to_owned(),
        ..astersql_kv::MPPTask::default()
    }];
    let stores = HashMap::from([(
        "tiflash-a:3930".to_owned(),
        TiFlashStoreInfo {
            zone: "zone-a".to_owned(),
            store_id: 61,
        },
    )]);
    let session = DispatchSessionInfo {
        time_zone_name: "UTC".to_owned(),
        schema_version: 67,
        resource_group_name: "rg".to_owned(),
        connection_id: 71,
        connection_alias: "coordinator-session".to_owned(),
        sql_digest: "sql-digest".to_owned(),
        plan_digest: "plan-digest".to_owned(),
        tidb_zone: "zone-a".to_owned(),
        ..DispatchSessionInfo::default()
    };

    let prepared = prepare_dispatch_requests_from_root(
        &root,
        3,
        true,
        &tasks,
        &stores,
        &session,
        "tidb:4000",
        true,
    )
    .expect("dispatch request prepares");
    assert_eq!(prepared.task_ids, vec![41]);
    assert_eq!(prepared.store_ids, vec![61]);
    let request = &prepared.requests[0];
    assert_eq!(request.ID, 41);
    assert_eq!(request.SchemaVar, 67);
    assert_eq!(request.CoordinatorAddress, "tidb:4000");
    assert!(request.ReportExecutionSummary);
    assert_eq!(request.ResourceGroupName, "rg");
    assert_eq!(request.ConnectionID, 71);

    let dag: tipb::DagRequest =
        protobuf::parse_from_bytes(&request.Data).expect("DAG request decodes");
    assert_eq!(dag.get_time_zone_name(), "UTC");
    assert_eq!(dag.get_output_offsets(), &[0, 1, 2]);
    assert_eq!(dag.get_encode_type(), tipb::EncodeType::TypeChunk);
    assert_eq!(dag.get_root_executor().get_tbl_scan().get_table_id(), 53);
}
