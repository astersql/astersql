// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

use base_dependency as base;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::{ImportInto, LoadData};

struct TestPlanContext {
    next_id: AtomicI32,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        panic!("initialize tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        panic!("initialize tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        panic!("initialize tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        panic!("initialize tests do not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("initialize tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        next_id: AtomicI32::new(40),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[test]
fn load_and_import_init_allocate_go_plan_identity() {
    let ctx = context();

    let load = LoadData::default().Init(Arc::clone(&ctx));
    assert_eq!(load.ID(), 41);
    assert_eq!(load.TP(), "LoadData");
    assert_eq!(load.QueryBlockOffset(), 0);
    assert!(Arc::ptr_eq(load.SCtx(), &ctx));

    let import = ImportInto::default().Init(Arc::clone(&ctx));
    assert_eq!(import.ID(), 42);
    assert_eq!(import.TP(), "ImportInto");
    assert_eq!(import.QueryBlockOffset(), 0);
    assert!(Arc::ptr_eq(import.SCtx(), &ctx));
}
