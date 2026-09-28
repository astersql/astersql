// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use base::{PhysicalPlan as _, Plan as _};
use logicalop::LogicalPlan as _;

use crate::{ExhaustPhysicalPlans4LogicalTopN, PhysicalTableScan, PhysicalTopN};

struct PushDownClient;

impl kv::Client for PushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("TopN protobuf test must not send a KV request")
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
        let mut session = planctx::variable::SessionVars::default();
        session.AllowMPPExecution = true;
        Self {
            plan_id: AtomicI32::new(0),
            session,
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
fn tiflash_protobuf_carries_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut topn = PhysicalTopN::New(context.clone(), 0, 10);
    let expected_id = topn.explain_id(&[]).to_string();
    let mut scan = PhysicalTableScan::New(context);
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    topn.set_children(vec![Box::new(scan)]);
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

    let executor = topn
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash TopN");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn mpp_allowed_enumerates_all_go_task_types_for_a_root_request() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut logical = logicalop::LogicalTopN {
        Count: 10,
        ..Default::default()
    }
    .Init(context, 0);
    logical.SetSchema(expression::NewSchema(Vec::new()));
    logical.SetStats(property::StatsInfo {
        RowCount: 7.0,
        ..Default::default()
    });
    let required = property::PhysicalProperty::default();

    let groups = ExhaustPhysicalPlans4LogicalTopN(&logical, &required);
    let topn_plans = &groups[0];

    assert_eq!(topn_plans.len(), 4);
    let task_types = topn_plans
        .iter()
        .map(|plan| {
            plan.as_any()
                .downcast_ref::<PhysicalTopN>()
                .expect("TopN candidate")
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .GetChildReqProps(0)
                .TaskTp
        })
        .collect::<Vec<_>>();
    assert_eq!(
        task_types,
        vec![
            property::CopSingleReadTaskType,
            property::CopMultiReadTaskType,
            property::RootTaskType,
            property::MppTaskType,
        ]
    );
    assert!(
        topn_plans
            .iter()
            .all(|plan| plan.stats_info().RowCount == 7.0)
    );
}
