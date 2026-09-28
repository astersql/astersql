// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use super::{AccessPath, FilterPathByIsolationRead};

struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: plan_base::BuildPBContext,
    builtin_function_usage: plan_base::BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: plan_base::BuildPBContext {
                ExprCtx: expression_for_build,
                Client: None,
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_function_usage: plan_base::BuiltinFunctionUsageCounter::default(),
        }
    }
}

impl plan_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::Relaxed) + 1
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
        panic!("misc test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &plan_base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn path(store_type: kv::StoreType) -> AccessPath {
    AccessPath {
        StoreType: store_type,
        ..Default::default()
    }
}

#[test]
fn filter_path_error_lists_available_engines_in_go_order() {
    let mut context = TestPlanContext::new();
    context
        .session
        .SetSystemVar(vardef::TiDBIsolationReadEngines, "tidb")
        .unwrap();

    let error = FilterPathByIsolationRead(
        &context,
        vec![path(kv::StoreType::TiKV), path(kv::StoreType::TiFlash)],
        parser_ast::NewCIStr("t"),
        parser_ast::NewCIStr("test"),
    )
    .err()
    .expect("disallowed engines must leave no access path");

    assert!(
        error
            .to_string()
            .contains("valid values can be 'tiflash, tikv'")
    );
}

#[test]
fn filter_path_still_raises_mpp_warning_when_no_path_remains() {
    let mut context = TestPlanContext::new();
    context.session.AllowMPPExecution = true;
    context.session.EnforceMPPExecution = true;
    context
        .session
        .SetSystemVar(vardef::TiDBIsolationReadEngines, "tidb")
        .unwrap();

    assert!(
        FilterPathByIsolationRead(
            &context,
            vec![path(kv::StoreType::TiKV)],
            parser_ast::NewCIStr("t"),
            parser_ast::NewCIStr("test"),
        )
        .is_err()
    );
    assert_eq!(context.session.StmtCtx.GetExtraWarnings().len(), 1);
}
