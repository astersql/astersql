// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{PhysicalProjection, PhysicalTableScan};

static IGNORE_INLIST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct PushDownClient;

impl kv::Client for PushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("projection protobuf test must not send a KV request")
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
fn projection_protobuf_carries_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut projection = PhysicalProjection::New(context.clone());
    let expected_id = projection.explain_id(&[]).to_string();
    let mut scan = PhysicalTableScan::New(context);
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    projection.set_children(vec![Box::new(scan)]);
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

    let executor = projection
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash projection");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn cop_projection_attaches_inside_index_reader_but_root_projection_stays_outside() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let column = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let schema = expression::NewSchema(vec![
        column.clone(),
        expression::Column::new(
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            2,
            2,
            1,
        ),
    ]);
    let output_schema = expression::NewSchema(vec![column.clone()]);
    let mut scan = crate::PhysicalIndexScan::New(context.clone());
    scan.PhysicalSchemaProducer.SetSchema(schema.Clone());
    let mut reader = crate::PhysicalIndexReader::New(context.clone());
    reader.PhysicalSchemaProducer.SetSchema(schema.Clone());
    reader.SetChildren(vec![Box::new(scan)]);
    for task_type in [property::CopSingleReadTaskType, property::RootTaskType] {
        let mut projection = PhysicalProjection::New(context.clone());
        projection.Exprs = vec![Box::new(column.clone())];
        projection
            .PhysicalSchemaProducer
            .SetSchema(output_schema.Clone());
        let mut child = property::PhysicalProperty::default();
        child.TaskTp = task_type;
        let projection = projection.Init(
            context.clone(),
            property::StatsInfo::default(),
            0,
            vec![Box::new(child)],
        );
        let task = projection.Attach2Task(vec![Box::new(crate::RootTask::New(
            Box::new(reader.Clone(context.clone()).unwrap()),
            None,
        ))]);
        if task_type == property::CopSingleReadTaskType {
            assert!(task.plan().as_any().is::<crate::PhysicalIndexReader>());
            assert!(
                task.plan().children()[0]
                    .as_any()
                    .is::<PhysicalProjection>()
            );
        } else {
            assert!(task.plan().as_any().is::<PhysicalProjection>());
            assert!(
                task.plan().children()[0]
                    .as_any()
                    .is::<crate::PhysicalIndexReader>()
            );
        }
        assert!(
            reader.children()[0]
                .as_any()
                .is::<crate::PhysicalIndexScan>()
        );
    }
}

#[test]
fn ordinary_cast_projection_stays_above_tikv_reader_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let input_type = *expression::types::NewFieldType(expression::mysql::TypeVarString);
    let output_type = *expression::types::NewFieldType(expression::mysql::TypeDouble);
    let input = expression::Column::new(input_type, 1, 1, 0);
    let cast = expression::NewFunctionBase(
        context.GetExprCtx(),
        expression::ast::Cast,
        output_type.clone(),
        vec![Box::new(input.clone())],
    )
    .expect("build cast expression");
    let mut scan = crate::PhysicalIndexScan::New(context.clone());
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![input]));
    let mut reader = crate::PhysicalIndexReader::New(context.clone());
    reader
        .PhysicalSchemaProducer
        .SetSchema(scan.schema().Clone());
    reader.SetChildren(vec![Box::new(scan)]);
    let mut projection = PhysicalProjection::New(context.clone());
    projection.Exprs = vec![cast];
    projection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![expression::Column::new(
            output_type,
            2,
            2,
            0,
        )]));
    let mut child = property::PhysicalProperty::default();
    child.TaskTp = property::CopSingleReadTaskType;
    let projection = projection.Init(
        context,
        property::StatsInfo::default(),
        0,
        vec![Box::new(child)],
    );
    let task = projection.Attach2Task(vec![Box::new(crate::RootTask::New(Box::new(reader), None))]);
    assert!(task.plan().as_any().is::<PhysicalProjection>());
    assert!(
        task.plan().children()[0]
            .as_any()
            .is::<crate::PhysicalIndexReader>()
    );
}

#[test]
fn normalized_explain_honors_ignore_inlist_plan_digest_like_go() {
    let _guard = IGNORE_INLIST_LOCK.lock().expect("lock ignore-in-list flag");
    let context = Arc::new(TestPlanContext::new());
    let result_type = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let expression = expression::NewFunctionBase(
        context.expression.as_ref(),
        expression::ast::In,
        result_type.clone(),
        vec![
            Box::new(expression::Column::new(result_type, 1, 1, 0)),
            Box::new(expression::NewInt64Const(1)),
            Box::new(expression::NewInt64Const(2)),
        ],
    )
    .expect("build IN expression");
    let mut projection = PhysicalProjection::New(context);
    projection.Exprs.push(expression);
    let previous = vardef::IgnoreInlistPlanDigest.Load();

    vardef::IgnoreInlistPlanDigest.Store(true);
    let ignored = projection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(false);
    let normalized = projection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(previous);

    assert_ne!(ignored, normalized);
    assert_eq!(
        ignored,
        String::from_utf8_lossy(&expression::SortedExplainExpressionListIgnoreInlist(
            &projection.Exprs,
        ))
    );
}

#[test]
fn projection_explain_reads_shared_statement_format() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut projection = PhysicalProjection::New(context.clone());
    projection.Exprs = vec![Box::new(expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        1,
        0,
    ))];
    projection
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![expression::Column::new(
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            1,
            1,
            0,
        )]));
    let ordinary = projection.ExplainInfo();
    assert!(ordinary.contains('#'), "{ordinary}");
    context
        .GetSessionVars()
        .StmtCtx
        .SetExplainContext(true, true, "ru");
    assert_eq!(projection.ExplainInfo(), ordinary);
    context
        .GetSessionVars()
        .StmtCtx
        .SetExplainContext(true, false, "plan_tree");
    let tree = projection.ExplainInfo();
    assert!(!tree.contains('#'), "{tree}");
}
