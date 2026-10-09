// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan as _, Plan as _};

use crate::physical_index_hash_join::LegacyPhysicalIndexHashJoin;
use crate::*;

#[test]
fn cached_select_lock_round_trip_retains_lock_mode_and_child() {
    let old_context = context();
    let mut lock = LegacyPhysicalLock::New(old_context.clone(), "for update".into(), 0);
    lock.PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(PhysicalTableDual::New(old_context, 1))]);
    let restored = CachedPlan::try_capture(&lock)
        .expect("capture canonical SelectLock")
        .restore(context())
        .expect("restore SelectLock with new context");
    let lock = restored
        .as_any()
        .downcast_ref::<LegacyPhysicalLock>()
        .expect("restored physical lock type");
    assert_eq!(lock.LockType, "for update");
    assert_eq!(lock.WaitSeconds, 0);
    assert!(lock.children()[0].as_any().is::<PhysicalTableDual>());
}

struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: base::BuildPBContext,
    builtin_usage: base::BuiltinFunctionUsageCounter,
}

#[test]
fn dml_round_trip_preserves_assignments_layouts_and_generated_columns() {
    assert_send_sync::<CachedPlan>();
    let assignment = |column_id, expression_id| expression::Assignment {
        Col: column(column_id),
        ColName: parser_ast::NewCIStr("assigned"),
        Expr: Box::new(column(expression_id)),
        LazyErr: None,
    };
    let mut update = Update::New(context(), Box::new(PhysicalTableDual::New(context(), 2)));
    update.OrderedList = vec![assignment(401, 402)];
    update.AllAssignmentsAreConstant = false;
    update.VirtualAssignmentsOffset = 1;
    update.IgnoreError = true;
    let restored = CachedPlan::try_capture_plan(&update)
        .expect("capture")
        .restore_plan(context())
        .expect("restore");
    let restored = restored.as_any().downcast_ref::<Update>().unwrap();
    assert_eq!(restored.OrderedList[0].Col.UniqueID, 401);
    assert_eq!(restored.OrderedList[0].ColName.O, "assigned");
    assert_eq!(
        restored.OrderedList[0]
            .Expr
            .as_any()
            .downcast_ref::<expression::Column>()
            .unwrap()
            .UniqueID,
        402
    );
    assert!(!restored.AllAssignmentsAreConstant);
    assert_eq!(restored.VirtualAssignmentsOffset, 1);
    assert!(restored.IgnoreError);

    let mut delete = Delete::New(context(), Box::new(PhysicalTableDual::New(context(), 3)));
    delete.IsMultiTable = true;
    delete.IgnoreErr = true;
    delete.TblColPosInfos = vec![TblColPosInfo {
        TblID: 77,
        Start: 2,
        End: 5,
        HandleCols: vec![column(403)],
        IndexesRowLayout: Some(DeleteIndexRowLayout::New(vec![DeleteIndexLayout {
            ID: 88,
            Name: "idx_order".into(),
            Columns: vec!["id".into()],
            Offsets: vec![4],
        }])),
    }];
    let restored = CachedPlan::try_capture_plan(&delete)
        .expect("capture")
        .restore_plan(context())
        .expect("restore");
    let restored = restored.as_any().downcast_ref::<Delete>().unwrap();
    assert!(restored.IsMultiTable && restored.IgnoreErr);
    let layout = &restored.TblColPosInfos[0];
    assert_eq!((layout.TblID, layout.Start, layout.End), (77, 2, 5));
    assert_eq!(layout.HandleCols[0].UniqueID, 403);
    assert_eq!(
        layout
            .IndexesRowLayout
            .as_ref()
            .unwrap()
            .Get(88)
            .unwrap()
            .Offsets,
        vec![4]
    );

    let mut insert = Insert::New(context());
    insert.TableSchema = Some(expression::NewSchema(vec![column(404)]));
    insert.Columns = vec![Box::new(parser_ast::ColumnName {
        Name: parser_ast::NewCIStr("id"),
        ..Default::default()
    })];
    insert.Lists = vec![vec![Box::new(column(405))]];
    insert.OnDuplicate = vec![Box::new(assignment(406, 407))];
    insert.Schema4OnDuplicate = Some(expression::NewSchema(vec![column(408)]));
    insert.GenCols.Exprs = vec![Box::new(column(409))];
    insert.GenCols.OnDuplicates = vec![Box::new(assignment(410, 411))];
    insert.SelectPlan = Some(Box::new(PhysicalTableDual::New(context(), 4)));
    insert.IsReplace = true;
    insert.IgnoreErr = true;
    insert.NeedFillDefaultValue = true;
    insert.AllAssignmentsAreConstant = true;
    insert.RowLen = 6;
    let restored = CachedPlan::try_capture_plan(&insert)
        .expect("capture")
        .restore_plan(context())
        .expect("restore");
    let restored = restored.as_any().downcast_ref::<Insert>().unwrap();
    assert_eq!(
        restored.TableSchema.as_ref().unwrap().Columns[0].UniqueID,
        404
    );
    assert_eq!(restored.Columns[0].Name.O, "id");
    assert_eq!(restored.Lists.len(), 1);
    assert_eq!(restored.OnDuplicate[0].Col.UniqueID, 406);
    assert_eq!(restored.GenCols.Exprs.len(), 1);
    assert_eq!(restored.GenCols.OnDuplicates[0].Col.UniqueID, 410);
    assert_eq!(
        restored
            .SelectPlan
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        4
    );
    assert!(restored.IsReplace && restored.IgnoreErr && restored.NeedFillDefaultValue);
    assert!(restored.AllAssignmentsAreConstant);
    assert_eq!(restored.RowLen, 6);
}

#[test]
fn dml_foreign_keys_are_rejected_precisely() {
    let mut insert = Insert::New(context());
    insert.FKChecks.push(Box::new(FKCheck::New(context())));
    let error = match CachedPlan::try_capture_plan(&insert) {
        Err(error) => error,
        Ok(_) => panic!("expected rejection"),
    };
    assert_eq!(
        error.to_string(),
        "Insert with foreign-key checks or cascades is not cacheable"
    );
    let mut insert = Insert::New(context());
    insert
        .FKCascades
        .push(Box::new(FKCascade::New(context(), FKCascadeType::OnDelete)));
    let error = match CachedPlan::try_capture_plan(&insert) {
        Err(error) => error,
        Ok(_) => panic!("expected rejection"),
    };
    assert_eq!(
        error.to_string(),
        "Insert with foreign-key checks or cascades is not cacheable"
    );

    let mut update = Update::New(context(), Box::new(PhysicalTableDual::New(context(), 1)));
    update.FKChecks.push(Box::new(FKCheck::New(context())));
    let error = match CachedPlan::try_capture_plan(&update) {
        Err(error) => error,
        Ok(_) => panic!("expected rejection"),
    };
    assert_eq!(
        error.to_string(),
        "Update with foreign-key checks or cascades is not cacheable"
    );

    let mut delete = Delete::New(context(), Box::new(PhysicalTableDual::New(context(), 1)));
    delete
        .FKCascades
        .push(Box::new(FKCascade::New(context(), FKCascadeType::OnUpdate)));
    let error = match CachedPlan::try_capture_plan(&delete) {
        Err(error) => error,
        Ok(_) => panic!("expected rejection"),
    };
    assert_eq!(
        error.to_string(),
        "Delete with foreign-key checks or cascades is not cacheable"
    );
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let build_expression: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: base::BuildPBContext {
                ExprCtx: build_expression,
                Client: None,
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_usage: base::BuiltinFunctionUsageCounter::default(),
        }
    }
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unused")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        &self.build_pb
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_usage.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext::new())
}
fn assert_send_sync<T: Send + Sync>() {}

fn join_base(ctx: base::ContextRef, plan_type: &str) -> BasePhysicalJoin {
    let mut base = BasePhysicalJoin::New(
        producer(ctx.clone(), plan_type, 201),
        base::JoinType::LeftOuterJoin,
    );
    base.LeftConditions = vec![Box::new(column(202))];
    base.RightConditions = vec![Box::new(column(203))];
    base.OtherConditions = vec![Box::new(column(204))];
    base.InnerChildIdx = 0;
    base.OuterJoinKeys = vec![column(205)];
    base.InnerJoinKeys = vec![column(206)];
    base.LeftJoinKeys = vec![column(207)];
    base.RightJoinKeys = vec![column(208)];
    base.IsNullEQ = vec![true];
    base.DefaultValues = vec![ranger::types::NewIntDatum(9)];
    base.LeftNAJoinKeys = vec![column(209)];
    base.RightNAJoinKeys = vec![column(210)];
    base.PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![
            Box::new(PhysicalTableDual::New(ctx.clone(), 11)),
            Box::new(PhysicalTableDual::New(ctx, 22)),
        ]);
    base
}

fn table_fixture() -> model::TableInfo {
    model::TableInfo {
        ID: 7,
        Name: parser_ast::NewCIStr("orders"),
        Columns: vec![model::ColumnInfo {
            ID: 3,
            Name: parser_ast::NewCIStr("id"),
            ..Default::default()
        }],
        Partition: Some(model::PartitionInfo {
            Definitions: vec![model::PartitionDefinition {
                ID: 71,
                Name: parser_ast::NewCIStr("p0"),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn index_fixture() -> model::IndexInfo {
    model::IndexInfo {
        ID: 9,
        Name: parser_ast::NewCIStr("idx_id"),
        Columns: vec![model::IndexColumn {
            Name: parser_ast::NewCIStr("id"),
            Offset: 0,
            ..Default::default()
        }],
        Unique: true,
        ..Default::default()
    }
}

fn column(unique_id: i64) -> expression::Column {
    expression::Column::new(expression::types::FieldType::default(), 3, unique_id, 0)
}

fn range(low: i64, high: i64) -> ranger::Range {
    ranger::Range {
        LowVal: vec![ranger::types::NewIntDatum(low)],
        HighVal: vec![ranger::types::NewIntDatum(high)],
        Collators: ranger::collate::GetBinaryCollatorSlice(1),
        LowExclude: true,
        HighExclude: false,
    }
}

fn round_trip(plan: &dyn base::PhysicalPlan) -> Box<dyn base::PhysicalPlan> {
    CachedPlan::try_capture(plan)
        .expect("capture")
        .restore(context())
        .expect("restore")
}

fn partition_fixture() -> PhysPlanPartInfo {
    PhysPlanPartInfo {
        PruningConds: vec![Box::new(column(301))],
        PartitionNames: vec![parser_ast::NewCIStr("p0")],
        Columns: vec![column(302)],
        ColumnNames: types::metadata::NameSlice(Vec::new()),
    }
}

fn reader_round_trip_assertions() {
    assert_send_sync::<CachedPlan>();
    assert_send_sync::<CachedLocalIndexLookup>();

    let mut index_scan = PhysicalIndexScan::New(context());
    index_scan.Table = Some(table_fixture());
    index_scan.Index = Some(index_fixture());
    index_scan.Ranges = ranger::Ranges(vec![range(10, 20)]);
    let mut index_reader = PhysicalIndexReader::New(context());
    index_reader.IndexPlan = Some(Box::new(index_scan));
    index_reader.OutputColumns = vec![column(303)];
    index_reader.PlanPartInfo = Some(partition_fixture());
    let restored = round_trip(&index_reader);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalIndexReader>()
        .unwrap();
    assert_eq!(restored.OutputColumns[0].UniqueID, 303);
    assert_eq!(
        restored.PlanPartInfo.as_ref().unwrap().PartitionNames[0].O,
        "p0"
    );
    assert_eq!(
        restored.PlanPartInfo.as_ref().unwrap().Columns[0].UniqueID,
        302
    );
    assert_eq!(
        restored
            .IndexPlan
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<PhysicalIndexScan>()
            .unwrap()
            .Ranges[0]
            .LowVal[0]
            .GetInt64(),
        10
    );

    let mut table_scan = PhysicalTableScan::New(context());
    table_scan.Table = Some(table_fixture());
    table_scan.Ranges = ranger::Ranges(vec![range(30, 40)]);
    let mut table_reader = PhysicalTableReader::New(context());
    table_reader.TablePlan = Some(Box::new(table_scan));
    table_reader.StoreType = kv::StoreType::TiFlash;
    table_reader.ReadReqType = ReadReqType::BatchCop;
    table_reader.IsCommonHandle = true;
    table_reader.PlanPartInfo = Some(partition_fixture());
    let restored = round_trip(&table_reader);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalTableReader>()
        .unwrap();
    assert_eq!(restored.StoreType, kv::StoreType::TiFlash);
    assert_eq!(restored.ReadReqType, ReadReqType::BatchCop);
    assert!(restored.IsCommonHandle);
    assert_eq!(
        restored
            .TablePlan
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<PhysicalTableScan>()
            .unwrap()
            .Ranges[0]
            .HighVal[0]
            .GetInt64(),
        40
    );

    let mut lookup = PhysicalIndexLookUpReader::New(context());
    lookup.IndexPlan = Some(Box::new(PhysicalIndexScan::New(context())));
    lookup.TablePlan = Some(Box::new(PhysicalTableScan::New(context())));
    lookup.IndexLookUpPushDown = true;
    lookup.Paging = true;
    lookup.ExtraHandleCol = Some(column(304));
    lookup.PushedLimit = Some(crate::physical_plan_misc::PushedDownLimit {
        Offset: 2,
        Count: 7,
    });
    lookup.CommonHandleCols = vec![column(305)];
    lookup.PlanPartInfo = Some(partition_fixture());
    lookup.ExpectedCnt = 11;
    lookup.KeepOrder = true;
    let restored = round_trip(&lookup);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalIndexLookUpReader>()
        .unwrap();
    assert!(restored.IndexLookUpPushDown && restored.Paging && restored.KeepOrder);
    assert_eq!(
        (restored.PushedLimit.unwrap().Offset, restored.ExpectedCnt),
        (2, 11)
    );
    assert_eq!(restored.ExtraHandleCol.as_ref().unwrap().UniqueID, 304);
    assert_eq!(restored.CommonHandleCols[0].UniqueID, 305);

    let mut merge = PhysicalIndexMergeReader::New(context());
    merge.IsIntersectionType = true;
    merge.AccessMVIndex = true;
    merge.PushedLimit = Some(crate::physical_plan_misc::PushedDownLimit {
        Offset: 3,
        Count: 9,
    });
    merge.ByItems = vec![planner_util::ByItems {
        Expr: Box::new(column(306)),
        Desc: true,
    }];
    merge.PartialPlansRaw = vec![
        Box::new(PhysicalIndexScan::New(context())),
        Box::new(PhysicalTableScan::New(context())),
    ];
    merge.TablePlan = Some(Box::new(PhysicalTableScan::New(context())));
    merge.PlanPartInfo = Some(partition_fixture());
    merge.KeepOrder = true;
    let restored = round_trip(&merge);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalIndexMergeReader>()
        .unwrap();
    assert!(restored.IsIntersectionType && restored.AccessMVIndex && restored.KeepOrder);
    assert_eq!(restored.PartialPlansRaw.len(), 2);
    assert!(restored.ByItems[0].Desc);
    assert_eq!(restored.PushedLimit.unwrap().Count, 9);

    use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode, Stats};
    use crate::physical_indexlookup::PhysicalLocalIndexLookup;
    let node = |id| PhysicalPlanNode {
        id,
        kind: PhysicalKind::Scan { table_id: id },
        schema: vec![id],
        children: vec![],
        stats: Stats::default(),
        required_properties: vec![],
    };
    let local = PhysicalLocalIndexLookup {
        index_plan: node(1),
        table_plan: node(2),
        keep_order: true,
        schema: vec![8, 9],
        index_handle_offsets: vec![1],
    };
    let restored = CachedLocalIndexLookup::capture(&local).restore();
    assert_eq!(restored.index_plan.id, 1);
    assert_eq!(restored.table_plan.id, 2);
    assert!(restored.keep_order);
    assert_eq!(restored.index_handle_offsets, vec![1]);
}

#[test]
fn cached_reader_round_trip() {
    reader_round_trip_assertions();
}

#[test]
fn reader_cached_round_trip() {
    reader_round_trip_assertions();
}

#[test]
fn leaf_table_dual_round_trip_preserves_fields() {
    let mut plan = PhysicalTableDual::New(context(), 4);
    plan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(31)]));
    plan.SetOutputNames(types::metadata::NameSlice(vec![Some(Arc::new(
        types::metadata::FieldName::default(),
    ))]));
    let explain = plan.ExplainInfo();
    let restored = round_trip(&plan);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalTableDual>()
        .unwrap();
    assert_eq!(restored.RowCount, 4);
    assert_eq!(restored.schema().Columns[0].UniqueID, 31);
    assert_eq!(restored.OutputNames().0.len(), 1);
    assert_eq!(restored.ExplainInfo(), explain);
}

#[test]
fn leaf_table_scan_round_trip_preserves_fields() {
    let mut plan = PhysicalTableScan::New(context());
    plan.Table = Some(table_fixture());
    plan.Columns = plan.Table.as_ref().unwrap().Columns.clone();
    plan.DBName = "shop".into();
    plan.TableAsName = "o".into();
    plan.PhysicalTableID = 71;
    plan.IsPartition = true;
    plan.Ranges = ranger::Ranges(vec![range(1, 8)]);
    plan.RangeInfo = "outer.id".into();
    plan.AccessCondition = vec![Box::new(column(41))];
    plan.FilterCondition = vec![Box::new(column(42))];
    plan.StoreType = kv::StoreType::TiFlash;
    plan.IsMPPOrBatchCop = true;
    plan.Desc = true;
    plan.KeepOrder = true;
    plan.IsCommonHandle = true;
    let explain = plan.ExplainInfo();
    let restored = round_trip(&plan);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalTableScan>()
        .unwrap();
    assert_eq!(restored.Table.as_ref().unwrap().ID, 7);
    assert_eq!(restored.Columns[0].ID, 3);
    assert_eq!(restored.Ranges[0].LowVal[0].GetInt64(), 1);
    assert!(restored.Ranges[0].LowExclude);
    assert_eq!(restored.AccessCondition.len(), 1);
    assert_eq!(restored.FilterCondition.len(), 1);
    assert_eq!((restored.PhysicalTableID, restored.IsPartition), (71, true));
    assert!(
        restored.Desc && restored.KeepOrder && restored.IsCommonHandle && restored.IsMPPOrBatchCop
    );
    assert_eq!(restored.ExplainInfo(), explain);
}

#[test]
fn leaf_index_scan_round_trip_preserves_fields() {
    let mut plan = PhysicalIndexScan::New(context());
    plan.Table = Some(table_fixture());
    plan.Index = Some(index_fixture());
    plan.IdxCols = vec![column(51)];
    plan.IdxColLens = vec![12];
    plan.Ranges = ranger::Ranges(vec![range(2, 9)]);
    plan.Columns = plan.Table.as_ref().unwrap().Columns.clone();
    plan.DBName = "shop".into();
    plan.TableAsName = "o".into();
    plan.PhysicalTableID = 71;
    plan.IsPartition = true;
    plan.Desc = true;
    plan.KeepOrder = true;
    plan.DoubleRead = true;
    plan.NeedCommonHandle = true;
    plan.PKIsHandleCol = Some(column(52));
    plan.ConstColsByCond = vec![true];
    plan.AccessCondition = vec![Box::new(column(53))];
    plan.FilterCondition = vec![Box::new(column(54))];
    plan.DataSourceSchema = Some(expression::NewSchema(vec![column(55)]));
    let explain = plan.ExplainInfo();
    let restored = round_trip(&plan);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalIndexScan>()
        .unwrap();
    assert_eq!(restored.Table.as_ref().unwrap().ID, 7);
    assert_eq!(restored.Index.as_ref().unwrap().ID, 9);
    assert_eq!(restored.IdxCols[0].UniqueID, 51);
    assert_eq!(restored.IdxColLens, vec![12]);
    assert_eq!(restored.Ranges[0].HighVal[0].GetInt64(), 9);
    assert_eq!(
        restored.DataSourceSchema.as_ref().unwrap().Columns[0].UniqueID,
        55
    );
    assert_eq!(restored.PKIsHandleCol.as_ref().unwrap().UniqueID, 52);
    assert_eq!(restored.ConstColsByCond, vec![true]);
    assert_eq!(restored.ExplainInfo(), explain);
}

#[test]
fn leaf_point_get_round_trip_matches_fast_clone() {
    let mut plan = PointGetPlan::New(context());
    plan.DBName = "shop".into();
    plan.TblInfo = Some(table_fixture());
    plan.IndexInfo = Some(index_fixture());
    plan.PartitionIdx = Some(0);
    plan.Handle = Some(88);
    plan.IndexValues = vec![ranger::types::NewIntDatum(5)];
    plan.IdxCols = vec![column(61)];
    plan.IdxColLens = vec![10];
    plan.AccessConditions = vec![Box::new(column(62))];
    plan.UnsignedHandle = true;
    plan.Lock = true;
    plan.LockWaitTime = 77;
    plan.Columns = plan.TblInfo.as_ref().unwrap().Columns.clone();
    plan.AccessColumns = vec![column(63)];
    plan.CostValue = 3.5;
    plan.SetOutputNames(types::metadata::NameSlice(vec![Some(Arc::new(
        types::metadata::FieldName::default(),
    ))]));
    let fast = plan.Clone(context()).unwrap();
    let restored = round_trip(&plan);
    let restored = restored.as_any().downcast_ref::<PointGetPlan>().unwrap();
    assert_eq!(plan.OutputNames().0.len(), 1);
    assert_eq!(fast.OutputNames().0.len(), 1);
    assert_eq!(restored.OutputNames().0.len(), 1);
    assert_eq!(restored.ExplainInfo(), fast.ExplainInfo());
    assert_eq!(restored.PartitionIdx, fast.PartitionIdx);
    assert_eq!(restored.Handle, fast.Handle);
    assert_eq!(
        restored.IndexValues[0].GetInt64(),
        fast.IndexValues[0].GetInt64()
    );
    assert_eq!(restored.IdxCols[0].UniqueID, 61);
    assert_eq!(restored.AccessColumns[0].UniqueID, 63);
    assert_eq!(
        (restored.Lock, restored.LockWaitTime, restored.CostValue),
        (true, 77, 3.5)
    );
}

#[test]
fn leaf_batch_point_get_round_trip_preserves_fields() {
    let mut plan = BatchPointGetPlan::New(context());
    plan.PointGetPlan.TblInfo = Some(table_fixture());
    plan.PointGetPlan.IndexInfo = Some(index_fixture());
    plan.Handles = vec![8, 9];
    plan.IndexValueRows = vec![
        vec![ranger::types::NewIntDatum(8)],
        vec![ranger::types::NewIntDatum(9)],
    ];
    plan.PartitionIdxs = vec![0, 0];
    plan.KeepOrder = true;
    plan.Desc = true;
    plan.Lock = true;
    plan.set_output_names(types::metadata::NameSlice(vec![Some(Arc::new(
        types::metadata::FieldName::default(),
    ))]));
    let explain = plan.ExplainInfo();
    let restored = round_trip(&plan);
    let restored = restored
        .as_any()
        .downcast_ref::<BatchPointGetPlan>()
        .unwrap();
    assert_eq!(plan.output_names().0.len(), 1);
    assert_eq!(restored.output_names().0.len(), 1);
    assert_eq!(restored.Handles, vec![8, 9]);
    assert_eq!(restored.IndexValueRows[0][0].GetInt64(), 8);
    assert_eq!(restored.IndexValueRows[1][0].GetInt64(), 9);
    assert_eq!(restored.PartitionIdxs, vec![0, 0]);
    assert!(restored.KeepOrder && restored.Desc && restored.Lock);
    assert_eq!(restored.ExplainInfo(), explain);
}

#[test]
fn cached_base_plan_round_trip_rebinds_context() {
    let old_context = context();
    let new_context = context();
    let mut base = BasePhysicalPlan::New(old_context.clone(), "parent", 7);
    base.Plan.SetID(41);
    base.Plan.SetNoncacheableReason("first reason");
    base.TiFlashFineGrainedShuffleStreamCount = 8;
    base.SetStatsTableName(Some("orders".to_owned()));
    base.SetStoreType(Some(kv::StoreType::TiFlash));
    base.set_stats(property::StatsInfo {
        RowCount: 19.0,
        StatsVersion: 23,
        ..Default::default()
    });
    base.SetChildrenReqProps(vec![property::NewPhysicalProperty(
        property::RootTaskType,
        &[],
        false,
        99.0,
        true,
    )]);
    base.SetChildren(vec![Box::new(BasePhysicalPlan::New(
        old_context.clone(),
        "child",
        8,
    ))]);
    base.PlanCostInit = true;
    base.PlanCost = 12.5;

    let snapshot = CachedPlanBase::try_from_base(&base).expect("cacheable base");
    let restored = snapshot
        .restore(new_context.clone())
        .expect("restored base");

    assert!(Arc::ptr_eq(base.Plan.SCtx(), &old_context));
    assert!(Arc::ptr_eq(restored.Plan.SCtx(), &new_context));
    assert!(!Arc::ptr_eq(restored.Plan.SCtx(), base.Plan.SCtx()));
    assert_eq!(restored.Plan.ID(), 41);
    assert_eq!(restored.Plan.TP(&[]), "parent");
    assert_eq!(restored.Plan.QueryBlockOffset(), 7);
    assert_eq!(restored.Plan.GetNoncacheableReason(), "first reason");
    assert!(restored.PlanCostInit);
    assert_eq!(restored.PlanCost, 12.5);
    assert_eq!(restored.TiFlashFineGrainedShuffleStreamCount, 8);
    assert_eq!(restored.StatsCount(), 19.0);
    assert_eq!(restored.GetChildReqProps(0).ExpectedCnt, 99.0);
    assert_eq!(restored.Children().len(), 1);
    assert_eq!(restored.Children()[0].tp(&[]), "child");
    assert!(Arc::ptr_eq(restored.Children()[0].s_ctx(), &new_context));
}

#[test]
fn cached_schema_producer_round_trip_preserves_schema() {
    let old_context = context();
    let new_context = context();
    let mut producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(old_context, "p", 3));
    producer.SetSchema(expression::NewSchema(vec![expression::Column::new(
        expression::types::FieldType::default(),
        2,
        22,
        0,
    )]));

    let snapshot = CachedSchemaProducer::try_from_producer(&producer).unwrap();
    let restored = snapshot.restore(new_context.clone()).unwrap();
    assert!(Arc::ptr_eq(
        restored.BasePhysicalPlan.Plan.SCtx(),
        &new_context
    ));
    assert_eq!(restored.SchemaRef().unwrap().Columns[0].UniqueID, 22);
}

#[test]
fn cache_snapshot_base_types_are_send_and_sync() {
    assert_send_sync::<CachedContext>();
    assert_send_sync::<CachedPhysicalProperty>();
    assert_send_sync::<CachedStats>();
    assert_send_sync::<CachedPlanBase>();
    assert_send_sync::<CachedSchemaProducer>();
    assert_send_sync::<CachedPlan>();
    assert_send_sync::<CachedTableDual>();
    assert_send_sync::<CachedTableScan>();
    assert_send_sync::<CachedIndexScan>();
    assert_send_sync::<CachedPointGet>();
    assert_send_sync::<CachedBatchPointGet>();
    assert_send_sync::<CachedSelection>();
    assert_send_sync::<CachedProjection>();
    assert_send_sync::<CachedTopN>();
    assert_send_sync::<CachedLimit>();
    assert_send_sync::<CachedStreamAgg>();
    assert_send_sync::<CachedHashAgg>();
    assert_send_sync::<CachedUnionAll>();
    assert_send_sync::<CachedUnionScan>();
    assert_send_sync::<CachedHashJoin>();
    assert_send_sync::<CachedMergeJoin>();
    assert_send_sync::<CachedIndexJoin>();
    assert_send_sync::<CachedIndexHashJoin>();
}

#[test]
fn join_cached_round_trip_preserves_hash_and_merge_fields() {
    let mut hash = NewPhysicalHashJoin(join_base(context(), "HashJoin"), 13, true);
    hash.StoreTp = kv::StoreType::TiFlash;
    hash.MppShuffleJoin = true;
    hash.FromHashJoinHint = true;
    hash.HasTableAlias = true;
    hash.RuntimeFilterTypes = vec![RuntimeFilterType::MinMax];
    let restored = round_trip(&hash);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalHashJoin>()
        .unwrap();
    assert_eq!(restored.Concurrency, 13);
    assert!(restored.UseOuterToBuild && restored.MppShuffleJoin);
    assert!(restored.FromHashJoinHint && restored.HasTableAlias);
    assert_eq!(restored.StoreTp, kv::StoreType::TiFlash);
    assert_eq!(restored.RuntimeFilterTypes, vec![RuntimeFilterType::MinMax]);
    assert_eq!(restored.BasePhysicalJoin.LeftNAJoinKeys[0].UniqueID, 209);
    assert_eq!(restored.BasePhysicalJoin.RightNAJoinKeys[0].UniqueID, 210);
    assert_eq!(
        restored.children()[0]
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        11
    );
    assert_eq!(
        restored.children()[1]
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        22
    );

    let merge = PhysicalMergeJoin {
        BasePhysicalJoin: join_base(context(), "MergeJoin"),
        Desc: true,
        CompareFuncs: vec![std::sync::Arc::new(|_, _, _, _| 1); 2],
    };
    let restored = round_trip(&merge);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalMergeJoin>()
        .unwrap();
    assert!(restored.Desc);
    assert_eq!(restored.CompareFuncs.len(), 2);
    assert_eq!(restored.BasePhysicalJoin.InnerChildIdx, 0);
    assert_eq!(restored.BasePhysicalJoin.DefaultValues[0].GetInt64(), 9);
    assert_eq!(restored.BasePhysicalJoin.LeftConditions.len(), 1);
    assert_eq!(restored.BasePhysicalJoin.RightConditions.len(), 1);
    assert_eq!(restored.BasePhysicalJoin.OtherConditions.len(), 1);
}

#[test]
fn join_cached_round_trip_preserves_index_fields_and_inner_plan() {
    let mut join = PhysicalIndexJoin::New(join_base(context(), "IndexJoin"));
    join.InnerPlan = Some(Box::new(PhysicalTableDual::New(context(), 33)));
    join.Ranges = ranger::Ranges(vec![range(4, 14)]);
    join.KeyOff2IdxOff = vec![2, 0];
    join.IdxColLens = vec![8, -1];
    let mut filters = physical_index_join::ColWithCmpFuncManager::New(Some(column(213)), 6);
    filters.AppendNewExpr("gt".into(), Box::new(column(214)), &[column(215)]);
    join.CompareFilters = Some(filters);
    join.OuterHashKeys = vec![column(211)];
    join.InnerHashKeys = vec![column(212)];
    join.FromDecorrelatedApply = true;
    let restored = round_trip(&join);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalIndexJoin>()
        .unwrap();
    assert_eq!(
        restored
            .InnerPlan
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        33
    );
    assert_eq!(restored.Ranges[0].LowVal[0].GetInt64(), 4);
    assert_eq!(restored.Ranges[0].HighVal[0].GetInt64(), 14);
    assert_eq!(restored.KeyOff2IdxOff, vec![2, 0]);
    assert_eq!(restored.IdxColLens, vec![8, -1]);
    assert_eq!(restored.OuterHashKeys[0].UniqueID, 211);
    assert_eq!(restored.InnerHashKeys[0].UniqueID, 212);
    let filters = restored.CompareFilters.as_ref().unwrap();
    assert_eq!(filters.TargetCol.as_ref().unwrap().UniqueID, 213);
    assert_eq!(filters.ColLength, 6);
    assert_eq!(filters.OpType, vec!["gt"]);
    assert_eq!(filters.OpArg.len(), 1);
    assert_eq!(filters.TmpConstant.len(), 1);
    assert_eq!(filters.AffectedColSchema.Columns[0].UniqueID, 215);
    assert!(restored.FromDecorrelatedApply);
}

#[test]
fn join_cached_round_trip_preserves_index_hash_children_and_properties() {
    let node = |id, rows| physical_common_plans::PhysicalPlanNode {
        id,
        kind: physical_common_plans::PhysicalKind::Other(format!("child-{id}")),
        schema: vec![id],
        children: Vec::new(),
        stats: physical_common_plans::Stats {
            row_count: rows,
            version: id as u64,
        },
        required_properties: Vec::new(),
    };
    let join = LegacyPhysicalIndexHashJoin {
        outer: node(1, 10.0),
        inner: node(2, 20.0),
        keep_outer_order: true,
        concurrency: 7,
        cached_cost: Some(42.0),
    };
    let restored = CachedIndexHashJoin::capture(&join).restore();
    assert_eq!(restored.outer.id, 1);
    assert_eq!(restored.inner.id, 2);
    assert!(restored.keep_outer_order);
    assert_eq!(restored.concurrency, 7);
    assert_eq!(restored.cached_cost, Some(42.0));
}

#[test]
fn unary_selection_round_trip_preserves_nested_child_and_conditions() {
    let old_context = context();
    let mut child = PhysicalTableDual::New(old_context.clone(), 2);
    child
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(80)]));

    let mut selection = PhysicalSelection::New(old_context);
    selection.Conditions = vec![Box::new(column(80))];
    selection.FromDataSource = true;
    selection
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(child)]);

    let restored = round_trip(&selection);
    let restored = restored
        .as_any()
        .downcast_ref::<PhysicalSelection>()
        .unwrap();
    assert_eq!(restored.Conditions.len(), 1);
    assert!(restored.FromDataSource);
    assert_eq!(restored.children().len(), 1);
    assert!(restored.children()[0].as_any().is::<PhysicalTableDual>());
}

fn producer(ctx: base::ContextRef, plan_type: &str, unique_id: i64) -> PhysicalSchemaProducer {
    let mut producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx, plan_type, 0));
    producer.SetSchema(expression::NewSchema(vec![column(unique_id)]));
    producer
}

fn aggregate(ctx: base::ContextRef, plan_type: &str, unique_id: i64) -> BasePhysicalAgg {
    let mut aggregate = BasePhysicalAgg::New(producer(ctx, plan_type, unique_id));
    aggregate.GroupByItems = vec![Box::new(column(unique_id))];
    aggregate.AggFuncs = vec![aggregation::AggFuncDesc {
        baseFuncDesc: aggregation::baseFuncDesc {
            Name: parser_ast::AggFuncCount.into(),
            Args: vec![Box::new(column(unique_id))],
            RetTp: Some(expression::types::FieldType::default()),
        },
        Mode: aggregation::CompleteMode,
        HasDistinct: true,
        OrderByItems: vec![planner_util::ByItems {
            Expr: Box::new(column(unique_id)),
            Desc: true,
        }],
        GroupingID: 9,
    }];
    aggregate.MppRunMode = AggMppRunMode::Mpp2Phase;
    aggregate.MppPartitionCols = vec![property::MPPPartitionColumn {
        Col: column(unique_id),
        CollateID: -46,
    }];
    aggregate
}

#[test]
fn cached_unary_agg_union_round_trip() {
    assert!(std::mem::size_of::<CachedPlan>() <= 2 * std::mem::size_of::<usize>());
    let old_context = context();

    let mut selection = PhysicalSelection::New(old_context.clone());
    selection.Conditions = vec![Box::new(column(101))];
    selection.FromDataSource = true;
    selection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(101)]));
    selection
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(PhysicalTableDual::New(
            old_context.clone(),
            3,
        ))]);
    let mut projection = PhysicalProjection::New(old_context.clone());
    projection.Exprs = vec![Box::new(column(101))];
    projection.CalculateNoDelay = true;
    projection.AvoidColumnEvaluator = true;
    projection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![column(101)]));
    projection
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(selection)]);

    let mut limit = PhysicalLimit::New(old_context.clone(), 2, 7);
    limit.PartitionBy = vec![property::SortItem {
        Col: column(102),
        Desc: true,
    }];
    limit.PrefixCol = Some(column(103));
    limit.PrefixLen = 4;
    limit
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(projection)]);
    let mut top_n = PhysicalTopN::New(old_context.clone(), 1, 5);
    top_n.ByItems = vec![planner_util::ByItems {
        Expr: Box::new(column(102)),
        Desc: true,
    }];
    top_n.PartitionBy = vec![property::SortItem {
        Col: column(103),
        Desc: false,
    }];
    top_n.PrefixCol = Some(column(104));
    top_n.PrefixLen = 8;
    top_n
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(limit)]);

    let mut stream = PhysicalStreamAgg {
        BasePhysicalAgg: aggregate(old_context.clone(), "StreamAgg", 105),
    };
    stream
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(top_n)]);
    let mut hash = PhysicalHashAgg {
        BasePhysicalAgg: aggregate(old_context.clone(), "HashAgg", 106),
        TiflashPreAggMode: "force_preagg".into(),
    };
    hash.BasePhysicalAgg
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(stream)]);

    let mut union_scan = PhysicalUnionScan::New(old_context.clone());
    union_scan.Conditions = vec![Box::new(column(107))];
    union_scan.HandleCols = planner_util::NewIntHandleCols(column(108));
    union_scan
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(hash)]);
    let mut union_all = PhysicalUnionAll::New(old_context);
    union_all.Mpp = true;
    union_all
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![
            Box::new(union_scan),
            Box::new(PhysicalTableDual::New(context(), 4)),
        ]);

    let explain = union_all.explain_info();
    let restored = round_trip(&union_all);
    let union_all = restored
        .as_any()
        .downcast_ref::<PhysicalUnionAll>()
        .unwrap();
    assert_eq!(union_all.explain_info(), explain);
    assert!(union_all.Mpp);
    assert_eq!(union_all.children().len(), 2);
    assert_eq!(
        union_all.children()[1]
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        4
    );
    let union_scan = union_all.children()[0]
        .as_any()
        .downcast_ref::<PhysicalUnionScan>()
        .unwrap();
    assert_eq!(union_scan.Conditions.len(), 1);
    assert_eq!(union_scan.HandleCols.GetCol(0).unwrap().UniqueID, 108);
    let hash = union_scan.children()[0]
        .as_any()
        .downcast_ref::<PhysicalHashAgg>()
        .unwrap();
    assert_eq!(hash.TiflashPreAggMode, "force_preagg");
    assert_eq!(hash.BasePhysicalAgg.AggFuncs[0].GroupingID, 9);
    assert!(hash.BasePhysicalAgg.AggFuncs[0].HasDistinct);
    assert_eq!(hash.BasePhysicalAgg.MppPartitionCols[0].CollateID, -46);
    let stream = hash.children()[0]
        .as_any()
        .downcast_ref::<PhysicalStreamAgg>()
        .unwrap();
    assert_eq!(stream.BasePhysicalAgg.GroupByItems.len(), 1);
    let top_n = stream.children()[0]
        .as_any()
        .downcast_ref::<PhysicalTopN>()
        .unwrap();
    assert_eq!((top_n.Offset, top_n.Count, top_n.PrefixLen), (1, 5, 8));
    assert!(top_n.ByItems[0].Desc);
    let limit = top_n.children()[0]
        .as_any()
        .downcast_ref::<PhysicalLimit>()
        .unwrap();
    assert_eq!((limit.Offset, limit.Count, limit.PrefixLen), (2, 7, 4));
    let projection = limit.children()[0]
        .as_any()
        .downcast_ref::<PhysicalProjection>()
        .unwrap();
    assert!(projection.CalculateNoDelay && projection.AvoidColumnEvaluator);
    assert_eq!(projection.schema().Columns[0].UniqueID, 101);
    let selection = projection.children()[0]
        .as_any()
        .downcast_ref::<PhysicalSelection>()
        .unwrap();
    assert!(selection.FromDataSource);
    assert_eq!(
        selection.children()[0]
            .as_any()
            .downcast_ref::<PhysicalTableDual>()
            .unwrap()
            .RowCount,
        3
    );
}

#[test]
fn go_merge_187_join_aggregation_typed_index_cache() {
    let mut index = PhysicalIndexJoin::New(join_base(context(), "IndexJoin"));
    index.OuterHashKeys = vec![column(501), column(502)];
    index.InnerHashKeys = vec![column(503), column(504)];
    let mut filter = crate::ColWithCmpFuncManager::New(Some(column(501)), -1);
    filter.OpType = vec!["gt".into()];
    filter.OpArg = vec![Box::new(column(502))];
    index.CompareFilters = Some(filter);
    index.set_children(vec![
        Box::new(PhysicalTableDual::New(context(), 2)),
        Box::new(PhysicalTableDual::New(context(), 3)),
    ]);
    let mut hash = crate::PhysicalIndexHashJoin::New(index);
    hash.KeepOuterOrder = true;
    let restored = round_trip(&hash);
    let restored = restored
        .as_any()
        .downcast_ref::<crate::PhysicalIndexHashJoin>()
        .unwrap();
    assert!(restored.KeepOuterOrder);
    assert_eq!(restored.OuterHashKeys.len(), 2);
    assert_eq!(restored.CompareFilters.as_ref().unwrap().OpType, ["gt"]);
    assert_eq!(restored.children().len(), 2);
    let cloned = hash.clone_physical(context()).unwrap();
    assert!(cloned.as_any().is::<crate::PhysicalIndexHashJoin>());
    let (cached, ok) = hash.clone_for_plan_cache(context());
    assert!(
        ok && cached
            .unwrap()
            .as_any()
            .is::<crate::PhysicalIndexHashJoin>()
    );
    let mut merge = crate::PhysicalIndexMergeJoin::New(hash.PhysicalIndexJoin);
    merge.CompareFuncs = vec![std::sync::Arc::new(|_, _, _, _| 1)];
    merge.OuterCompareFuncs = vec![std::sync::Arc::new(|_, _, _, _| -1)];
    merge.KeyOff2KeyOffOrderByIdx = vec![1, 0];
    merge.NeedOuterSort = true;
    merge.Desc = true;
    let restored = round_trip(&merge);
    let restored = restored
        .as_any()
        .downcast_ref::<crate::PhysicalIndexMergeJoin>()
        .unwrap();
    assert!(restored.NeedOuterSort && restored.Desc);
    assert_eq!(restored.CompareFuncs.len(), 1);
    assert_eq!(restored.OuterCompareFuncs.len(), 1);
    assert_eq!(restored.KeyOff2KeyOffOrderByIdx, [1, 0]);
    assert_eq!(restored.OuterHashKeys.len(), 2);
    assert_eq!(restored.CompareFilters.as_ref().unwrap().OpType, ["gt"]);
    assert!(
        merge
            .clone_physical(context())
            .unwrap()
            .as_any()
            .is::<crate::PhysicalIndexMergeJoin>()
    );
}

#[test]
fn go_merge_187_join_aggregation_attach_keeps_concrete_variant() {
    let mut merge = crate::PhysicalIndexMergeJoin::New(PhysicalIndexJoin::New(join_base(
        context(),
        "IndexJoin",
    )));
    merge.NeedOuterSort = true;
    merge.CompareFuncs = vec![std::sync::Arc::new(|_, _, _, _| 1)];
    merge.OuterCompareFuncs = vec![std::sync::Arc::new(|_, _, _, _| -1)];
    let rewritten = merge.PhysicalIndexJoin.Clone(context()).unwrap();
    let attached = crate::preserve_index_join_variant(merge.as_any(), rewritten);
    let restored = attached
        .as_any()
        .downcast_ref::<crate::PhysicalIndexMergeJoin>()
        .unwrap();
    assert!(restored.NeedOuterSort);
    assert_eq!(restored.CompareFuncs.len(), 1);
    assert_eq!(restored.OuterCompareFuncs.len(), 1);
    let mut hash = crate::PhysicalIndexHashJoin::New(PhysicalIndexJoin::New(join_base(
        context(),
        "IndexJoin",
    )));
    hash.KeepOuterOrder = true;
    let attached = crate::preserve_index_join_variant(
        hash.as_any(),
        hash.PhysicalIndexJoin.Clone(context()).unwrap(),
    );
    assert!(
        attached
            .as_any()
            .downcast_ref::<crate::PhysicalIndexHashJoin>()
            .unwrap()
            .KeepOuterOrder
    );
}
