// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use base::{PhysicalPlan as _, Plan as _};

use crate::{PhysicalTableScan, PhysicalWindow};

struct PushDownClient;

impl kv::Client for PushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("window protobuf test must not send a KV request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: base::BuildPBContext,
    builtin_usage: base::BuiltinFunctionUsageCounter,
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
                Client: Some(Arc::new(PushDownClient)),
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

#[test]
fn window_and_shuffle_initialization_preserve_allocated_ids() {
    let context = Arc::new(TestPlanContext::new());
    let window = PhysicalWindow::New(context.clone());
    let window_id = window.id();
    let window = window.Init(
        context.clone(),
        property::StatsInfo::default(),
        3,
        property::PhysicalProperty::default(),
    );
    assert_eq!(window.id(), window_id);
    assert_eq!(window.query_block_offset(), 3);
    assert_eq!(context.plan_id.load(Ordering::SeqCst), window_id);

    let shuffle = crate::PhysicalShuffle::New(context.clone(), 4, vec![]);
    let shuffle_id = shuffle.id();
    let shuffle = shuffle.Init(
        context.clone(),
        property::StatsInfo::default(),
        3,
        Box::new(window),
    );
    assert_eq!(shuffle.id(), shuffle_id);
    assert_eq!(shuffle.children()[0].id(), window_id);
    assert_eq!(context.plan_id.load(Ordering::SeqCst), shuffle_id);
}

#[test]
fn tiflash_protobuf_carries_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut window = PhysicalWindow::New(context.clone());
    let expected_id = window.explain_id(&[]).to_string();
    let mut scan = PhysicalTableScan::New(context);
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    window.set_children(vec![Box::new(scan)]);
    let mut build_pb = base::BuildPBContext {
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        Client: Some(Arc::new(PushDownClient)),
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = window
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash window");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn resolve_indices_updates_passthrough_schema_columns_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let field_type = (*expression::types::NewFieldType(mysql::r#type::TypeLonglong)).clone();
    let mut scan = PhysicalTableScan::New(context.clone());
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![expression::Column::new(
            field_type.clone(),
            7,
            70,
            0,
        )]));

    let mut window = PhysicalWindow::New(context);
    window.WindowFuncDescs.push(logicalop::WindowFuncDesc {
        Name: "row_number".to_owned(),
        Args: Vec::new(),
    });
    window
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            expression::Column::new(field_type.clone(), 7, 70, 99),
            expression::Column::new(field_type, 8, 80, 100),
        ]));
    window.set_children(vec![Box::new(scan)]);

    window.ResolveIndices().expect("resolve window indices");

    let columns = &window
        .PhysicalSchemaProducer
        .SchemaRef()
        .expect("window schema")
        .Columns;
    assert_eq!(columns[0].Index, 0);
    assert_eq!(columns[1].Index, 100);
}

pub(super) fn ru_orchestration_context() -> base::ContextRef {
    Arc::new(TestPlanContext::new())
}

#[test]
fn ru_shuffle_clone_preserves_sources_keys_and_splitter() {
    let ctx = ru_orchestration_context();
    let mut source = PhysicalTableScan::New(ctx.clone());
    let source_id = source.id();
    source
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![expression::Column::new(
            (*expression::types::NewFieldType(mysql::r#type::TypeLonglong)).clone(),
            7,
            7,
            0,
        )]));
    let mut shuffle = crate::PhysicalShuffle::New(ctx.clone(), 4, vec![]);
    shuffle.DataSources = vec![Box::new(source)];
    shuffle.ByItemArrays = vec![vec![Box::new(expression::Column::new(
        (*expression::types::NewFieldType(mysql::r#type::TypeLonglong)).clone(),
        7,
        7,
        99,
    ))]];
    shuffle.SplitterType = crate::physical_shuffle::PartitionSplitterType::Range;
    shuffle.ResolveIndices().unwrap();
    assert_eq!(
        shuffle.ByItemArrays[0][0]
            .as_any()
            .downcast_ref::<expression::Column>()
            .unwrap()
            .Index,
        0
    );
    let cloned = shuffle.Clone(ctx).unwrap();
    assert_eq!(cloned.DataSources[0].id(), source_id);
    assert_eq!(
        cloned.SplitterType,
        crate::physical_shuffle::PartitionSplitterType::Range
    );
    assert_eq!(cloned.ByItemArrays[0].len(), 1);
    assert!(!std::ptr::eq(
        cloned.DataSources[0].as_any(),
        shuffle.DataSources[0].as_any()
    ));
    shuffle.ByItemArrays.clear();
    assert!(shuffle.ResolveIndices().is_err());
}
