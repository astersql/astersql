// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{PhysicalLimit, PhysicalTableScan};

struct SupportedPushDownClient;

impl kv::Client for SupportedPushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("limit protobuf test must not send a KV request")
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
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: base::BuildPBContext {
                ExprCtx: expression_for_build,
                Client: Some(Arc::new(SupportedPushDownClient)),
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
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
        panic!("limit protobuf test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

#[test]
fn tiflash_protobuf_carries_the_limit_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut limit = PhysicalLimit::New(context.clone(), 2, 5);
    let expected_id = limit
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .explain_id(&[])
        .to_string();
    let mut scan = PhysicalTableScan::New(context.clone());
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    limit
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(scan)]);
    let mut build_pb = base::BuildPBContext {
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        Client: Some(Arc::new(SupportedPushDownClient)),
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = limit
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash limit");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn tikv_protobuf_without_expressions_does_not_require_a_client() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let limit = PhysicalLimit::New(context, 0, 1);
    let mut build_pb = base::BuildPBContext {
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        Client: None,
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = limit
        .ToPB(&mut build_pb, kv::StoreType::TiKV)
        .expect("an expression-free TiKV limit does not consult the client");

    assert_eq!(executor.get_tp(), tipb::ExecType::TypeLimit);
    assert_eq!(executor.get_limit().get_limit(), 1);
}

#[test]
fn resolve_indices_keeps_duplicate_inline_projection_columns_distinct() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut duplicate = expression::Column::default();
    duplicate.UniqueID = 7;
    let mut trailing = expression::Column::default();
    trailing.UniqueID = 9;
    let mut child = PhysicalTableScan::New(context.clone());
    child
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            duplicate.Clone(),
            duplicate.Clone(),
            trailing.Clone(),
        ]));
    let mut limit = PhysicalLimit::New(context, 0, 1);
    limit
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            duplicate.Clone(),
            duplicate,
            trailing,
        ]));
    limit
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(child)]);

    limit.ResolveIndices().expect("resolve inline projection");

    assert_eq!(
        limit
            .PhysicalSchemaProducer
            .SchemaRef()
            .expect("limit schema")
            .Columns
            .iter()
            .map(|column| column.Index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn memory_usage_matches_go_scalar_and_prefix_pointer_accounting() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut limit = PhysicalLimit::New(context, 0, 1);
    limit.PartitionBy.push(property::SortItem {
        Col: expression::Column::default(),
        Desc: false,
    });
    let expected = limit.PhysicalSchemaProducer.MemoryUsage()
        + std::mem::size_of::<u64>() as i64 * 2
        + std::mem::size_of::<*const expression::Column>() as i64
        + std::mem::size_of::<usize>() as i64;

    assert_eq!(limit.MemoryUsage(), expected);
}
