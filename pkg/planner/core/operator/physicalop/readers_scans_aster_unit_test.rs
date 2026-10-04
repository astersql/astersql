// Copyright 2026 AsterSQL.

// Reader/Scan 物理算子的类型与行为冒烟测试。
//
// 确认各类扫描/读请求实现 `PhysicalPlan`，空 Ranges 视为全扫，
// 以及 BatchPointGet 分区剪枝与 ReadReqType 命名与 Go 一致。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::PhysicalPlan;

use crate::{
    BatchPointGetPlan, PhysicalIndexLookUpReader, PhysicalIndexMergeReader, PhysicalIndexReader,
    PhysicalIndexScan, PhysicalTableReader, PhysicalTableScan, PointGetPlan, ReadReqType,
};

/// 最小 PlanContext：分配计划 ID；会话相关方法在本文件测试中不应触发。
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
        panic!("reader and scan test does not build ranges")
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

/// 构造测试 Context。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
/// 编译期断言：主要 Scan/Reader/PointGet 均实现 PhysicalPlan。
fn scans_and_readers_implement_physical_plan() {
    fn assert_plan<T: PhysicalPlan>() {}
    assert_plan::<PhysicalIndexScan>();
    assert_plan::<PhysicalTableScan>();
    assert_plan::<PhysicalIndexReader>();
    assert_plan::<PhysicalTableReader>();
    assert_plan::<PhysicalIndexLookUpReader>();
    assert_plan::<PhysicalIndexMergeReader>();
    assert_plan::<PointGetPlan>();
    assert_plan::<BatchPointGetPlan>();
}

#[test]
/// 新建扫描默认 Ranges 为空，应判定为全表/全索引扫描。
fn empty_scan_ranges_are_full_scans() {
    assert!(PhysicalIndexScan::New(context()).IsFullScan());
    let mut table_scan = PhysicalTableScan::New(context());
    // Go's IsFullScan requires the TableInfo installed during plan construction.
    table_scan.Table = Some(model::TableInfo::default());
    assert!(table_scan.IsFullScan());
}

#[test]
/// 分区剪枝后 PartitionIdxs 与 Handles 保持一一对应。
fn batch_partition_pruning_keeps_values_aligned() {
    let mut plan = BatchPointGetPlan::New(context());
    plan.PartitionIdxs = vec![0, 2, 1];
    plan.Handles = vec![10, 20, 30];
    plan.IndexValueRows = vec![
        vec![types::datum::NewIntDatum(10)],
        vec![types::datum::NewIntDatum(20)],
        vec![types::datum::NewIntDatum(30)],
    ];
    plan.PrunePrecomputedPartitionsAndValues(&[0, 1]);
    assert_eq!(plan.PartitionIdxs, vec![0, 1]);
    assert_eq!(plan.Handles, vec![10, 30]);
    assert_eq!(plan.IndexValueRows.len(), 2);
    assert_eq!(plan.IndexValueRows[0][0].GetInt64(), 10);
    assert_eq!(plan.IndexValueRows[1][0].GetInt64(), 30);
}

#[test]
/// ReadReqType 显示名与 Go（cop/batchCop/mpp）对齐。
fn read_request_names_match_go() {
    assert_eq!(ReadReqType::Cop.Name(), "cop");
    assert_eq!(ReadReqType::BatchCop.Name(), "batchCop");
    assert_eq!(ReadReqType::MPP.Name(), "mpp");
}

#[test]
fn table_scan_pb_contains_go_compatible_column_metadata() {
    let mut primary_type = expression::types::NewFieldType(mysql::r#type::TypeLonglong);
    primary_type.AddFlag(mysql::r#type::PriKeyFlag | mysql::r#type::UnsignedFlag);
    let primary = model::ColumnInfo {
        ID: 7,
        Name: parser_ast::NewCIStr("id"),
        FieldType: *primary_type,
        ..Default::default()
    };
    let table = model::TableInfo {
        ID: 42,
        PKIsHandle: true,
        Columns: vec![primary.clone()],
        ..Default::default()
    };
    let mut scan = PhysicalTableScan::New(context());
    scan.Table = Some(table);
    scan.Columns = vec![primary];
    let expression: Arc<dyn planctx::exprctx::BuildContext> =
        Arc::new(exprstatic::NewExprContext(Vec::new()));
    let mut build_context = base::BuildPBContext {
        ExprCtx: expression,
        Client: None,
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = scan
        .to_pb(&mut build_context, kv::StoreType::TiKV)
        .expect("encode table scan");
    let column = &executor.get_tbl_scan().get_columns()[0];
    assert_eq!(column.get_column_id(), 7);
    assert!(column.get_pk_handle());
    assert_ne!(column.get_flag() & mysql::r#type::UnsignedFlag as i32, 0);
}
