// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{PhysicalSelection, PhysicalTableScan};

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
        panic!("selection protobuf test must not send a KV request")
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
        panic!("selection protobuf test does not build ranges")
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
fn normalized_explain_honors_ignore_inlist_plan_digest_like_go() {
    let _guard = IGNORE_INLIST_LOCK.lock().expect("lock ignore-in-list flag");
    let context = Arc::new(TestPlanContext::new());
    let result_type = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let condition = expression::NewFunctionBase(
        context.expression.as_ref(),
        expression::ast::In,
        result_type.clone(),
        vec![
            Box::new(expression::Column::new(result_type, 1, 1, 0)),
            Box::new(expression::NewInt64Const(1)),
            Box::new(expression::NewInt64Const(2)),
        ],
    )
    .expect("build IN condition");
    let mut selection = PhysicalSelection::New(context);
    selection.Conditions.push(condition);
    let previous = vardef::IgnoreInlistPlanDigest.Load();

    vardef::IgnoreInlistPlanDigest.Store(true);
    let ignored = selection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(false);
    let normalized = selection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(previous);

    assert_ne!(ignored, normalized);
    assert_eq!(
        ignored,
        String::from_utf8_lossy(&expression::SortedExplainExpressionListIgnoreInlist(
            &selection.Conditions,
        ))
    );
}

#[test]
fn tiflash_protobuf_carries_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut selection = PhysicalSelection::New(context.clone());
    let expected_id = selection.explain_id(&[]).to_string();
    let mut scan = PhysicalTableScan::New(context);
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    selection.set_children(vec![Box::new(scan)]);
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

    let executor = selection
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash selection");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn empty_tikv_selection_does_not_require_a_client() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let selection = PhysicalSelection::New(context);
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

    let executor = selection
        .ToPB(&mut build_pb, kv::StoreType::TiKV)
        .expect("an expression-free TiKV selection does not consult the client");

    assert_eq!(executor.get_tp(), tipb::ExecType::TypeSelection);
    assert!(executor.get_selection().get_conditions().is_empty());
    assert!(executor.get_executor_id().is_empty());
}
