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

use crate::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

struct MutableExplainContext {
    plan_id: AtomicI32,
    ignore_suffix: AtomicBool,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for MutableExplainContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        self.ignore_suffix.load(Ordering::SeqCst)
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("explain ID test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("explain ID test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("explain ID test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("explain ID test does not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("explain ID test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

#[test]
fn explain_id_reads_context_when_formatted_like_go_stringer_func() {
    let ctx = Arc::new(MutableExplainContext {
        plan_id: AtomicI32::new(0),
        ignore_suffix: AtomicBool::new(false),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    });
    let plan = NewBasePlan(ctx.clone(), "TableScan", 0);
    let explain_id = plan.ExplainID(&[]);

    ctx.ignore_suffix.store(true, Ordering::SeqCst);

    assert_eq!(explain_id.to_string(), "TableScan");
}
