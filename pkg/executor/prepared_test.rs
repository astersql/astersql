// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// EXECUTE（执行已准备语句）构建路径的单元测试。
//
// 验证 `ExecuteExec::Build`：成功时挂接执行器并按计划调低优先级；
// 失败时透传 builder 错误且不留下半成品执行器。

use crate::prepared::{
    DeallocateBackend, DeallocateExec, ExecuteBuilder, ExecuteExec, GeneratedPreparedStatement,
    NewPrepareExec, PrepareBackend,
};

/// 可切换成功/失败的测试用 `ExecuteBuilder`。
struct Builder {
    fail: bool,
}

impl ExecuteBuilder<i32> for Builder {
    type Executor = String;
    type Error = &'static str;

    fn build(&mut self, plan: &i32) -> Result<String, Self::Error> {
        if self.fail {
            Err("build failed")
        } else {
            Ok(format!("executor-{plan}"))
        }
    }

    fn no_priority(&self) -> bool {
        true
    }

    /// 计划 ID 大于 10 时建议降低优先级。
    fn need_lower_priority(&self, plan: &i32) -> bool {
        *plan > 10
    }
}

/// 成功构建应写入执行器并设置 lower_priority；失败应透传错误。
#[test]
fn prepared_execute_builds_canonical_executor_and_propagates_builder_error() {
    let mut execute = ExecuteExec {
        name: "stmt".to_owned(),
        using_vars: Vec::<()>::new(),
        statement_executor: None,
        statement: (),
        plan: 11,
        lower_priority: false,
    };
    // 成功路径：生成 executor-11，且因 plan>10 将 lower_priority 置真。
    execute.Build(&mut Builder { fail: false }).unwrap();
    assert_eq!(execute.statement_executor.as_deref(), Some("executor-11"));
    assert!(execute.lower_priority);

    let mut failed = ExecuteExec {
        name: "bad".to_owned(),
        using_vars: Vec::<()>::new(),
        statement_executor: None,
        statement: (),
        plan: 1,
        lower_priority: false,
    };
    // 失败路径：错误原样返回，statement_executor 保持为空。
    assert_eq!(
        failed.Build(&mut Builder { fail: true }),
        Err("build failed")
    );
    assert!(failed.statement_executor.is_none());
}

struct DeallocateState {
    cache_enabled: bool,
    statement_is_valid: bool,
    cache_key_error: bool,
    removed_name: bool,
    removed_statement: bool,
}

impl DeallocateBackend for DeallocateState {
    type PreparedStatement = ();
    type CacheKey = ();
    type Error = &'static str;

    fn prepared_statement_id(&self, name: &str) -> Option<u32> {
        (name == "stmt").then_some(1)
    }

    fn prepared_statement(&self, _id: u32) -> Option<Self::PreparedStatement> {
        self.statement_is_valid.then_some(())
    }

    fn statement_not_found(&self) -> Self::Error {
        "statement not found"
    }

    fn invalid_plan_cache_statement(&self) -> Self::Error {
        "invalid PlanCacheStmt type"
    }

    fn remove_prepared_statement_name(&mut self, _name: &str) {
        self.removed_name = true;
    }

    fn prepared_plan_cache_enabled(&self) -> bool {
        self.cache_enabled
    }

    fn plan_cache_key(
        &self,
        _statement: &Self::PreparedStatement,
    ) -> Result<Self::CacheKey, Self::Error> {
        if self.cache_key_error {
            Err("cache key failed")
        } else {
            Ok(())
        }
    }

    fn keep_plan_cache_on_close(&self) -> bool {
        false
    }

    fn delete_plan_cache_entry(&mut self, _key: Self::CacheKey) {}

    fn remove_prepared_statement(&mut self, _id: u32) {
        self.removed_statement = true;
    }
}

#[test]
fn deallocate_rejects_invalid_statement_even_when_plan_cache_is_disabled() {
    let mut execute = DeallocateExec {
        backend: DeallocateState {
            cache_enabled: false,
            statement_is_valid: false,
            cache_key_error: false,
            removed_name: false,
            removed_statement: false,
        },
        name: "stmt".to_owned(),
    };

    assert_eq!(execute.Next((), ()), Err("invalid PlanCacheStmt type"));
    assert!(!execute.backend.removed_name);
    assert!(!execute.backend.removed_statement);
}

#[test]
fn deallocate_removes_name_before_plan_cache_key_error() {
    let mut execute = DeallocateExec {
        backend: DeallocateState {
            cache_enabled: true,
            statement_is_valid: true,
            cache_key_error: true,
            removed_name: false,
            removed_statement: false,
        },
        name: "stmt".to_owned(),
    };

    assert_eq!(execute.Next((), ()), Err("cache key failed"));
    assert!(execute.backend.removed_name);
    assert!(!execute.backend.removed_statement);
}

struct PrepareState;

impl PrepareBackend for PrepareState {
    type Context = ();
    type Statement = i32;
    type PreparedStatement = String;
    type Plan = bool;
    type ResultField = ();
    type Error = &'static str;

    fn prepared_statement_exists(&self, _id: u32) -> bool {
        false
    }

    fn warning_count(&self) -> usize {
        0
    }

    fn parse_sql<C>(
        &mut self,
        _ctx: C,
        _sql: &str,
        _reset_protocol_context: bool,
    ) -> Result<Vec<Self::Statement>, Self::Error> {
        Ok(vec![1])
    }

    fn in_restricted_sql(&self) -> bool {
        false
    }

    fn append_statement_error(&mut self, _error: &Self::Error) {}

    fn retain_warnings_from(&mut self, _index: usize) {}

    fn syntax_error(&self, error: Self::Error) -> Self::Error {
        error
    }

    fn prepare_multiple_statements_error(&self) -> Self::Error {
        "multiple statements"
    }

    fn reset_context_of_statement(
        &mut self,
        _statement: &Self::Statement,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn generate_plan_cache_statement<C>(
        &mut self,
        _ctx: C,
        _statement: Self::Statement,
    ) -> Result<GeneratedPreparedStatement<Self::PreparedStatement, Self::Plan>, Self::Error> {
        Ok(GeneratedPreparedStatement {
            statement: "prepared".to_owned(),
            plan: true,
            parameter_count: 2,
        })
    }

    fn top_profiling_enabled(&self) -> bool {
        false
    }

    fn register_top_sql(&mut self, _statement: &Self::PreparedStatement) {}

    fn reset_plan_identifiers(&mut self) {}

    fn is_no_result_plan(&self, _plan: &Self::Plan) -> bool {
        true
    }

    fn result_fields(&self, _plan: &Self::Plan) -> Vec<Self::ResultField> {
        vec![]
    }

    fn next_prepared_statement_id(&mut self) -> u32 {
        7
    }

    fn bind_prepared_statement_name(&mut self, _name: String, _id: u32) {}

    fn add_prepared_statement(
        &mut self,
        _id: u32,
        _statement: &Self::PreparedStatement,
    ) -> Result<(), Self::Error> {
        Err("registration failed")
    }
}

#[test]
fn prepare_retains_generated_statement_when_registration_fails() {
    let mut execute = NewPrepareExec(PrepareState, "select ?".to_owned());

    assert_eq!(execute.Next(()), Err("registration failed"));
    assert_eq!(execute.id, 7);
    assert_eq!(execute.parameter_count, 2);
    assert_eq!(execute.statement.as_deref(), Some("prepared"));
}
