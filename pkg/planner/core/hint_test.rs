// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 优化器 Hint 解析与安全策略相关的单元测试。
//
// 覆盖 `set_var` 语句级变量 Hint、慢查询日志 Hint（`write_slow_log`）、
// 以及受限 Hint（Restricted Hint，按安全策略忽略并告警）的解析与过滤行为。

use hint_dependency::{
    ExtractTableHintsFromStmtNode, HintWriteSlowLog, NewQBHintHandler, ParsePlanHints,
    ParseStmtHints, RegisterRestrictedHintChecker, hintWarnHandler,
};
use parser_ast_dependency::{HintData, HintSetVar, NewCIStr, TableOptimizerHint};
use std::sync::{Mutex, MutexGuard};
use variable_dependency::{
    Context as VariableContext, GetSysVar, GlobalVarAccessor, SessionVars, VariableError,
    register_builtin_sysvars,
};

/// 串行化受限 Hint 检查器注册，避免测试间互相污染。
static RESTRICTED_HINT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取受限 Hint 测试互斥锁；中毒时仍取内部守卫继续跑测。
fn lock_restricted_hint_checker() -> MutexGuard<'static, ()> {
    RESTRICTED_HINT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 构造带名称与数据载荷的表级优化器 Hint。
fn hint(name: &str, data: HintData) -> TableOptimizerHint {
    TableOptimizerHint {
        HintName: NewCIStr(name),
        HintData: data,
        ..TableOptimizerHint::default()
    }
}

/// 测试用：始终允许 `set_var` Hint。
fn allow_set_var(_name: String, _hint: String) -> (bool, Option<hint_dependency::errors::Error>) {
    (true, None)
}

/// 测试用：不提供假想索引（hypothetical index）。
fn no_hypothetical_index(
    _database: parser_ast_dependency::CIStr,
    _table: parser_ast_dependency::CIStr,
    _column: parser_ast_dependency::CIStr,
) -> (i32, Option<hint_dependency::errors::Error>) {
    (0, None)
}

/// 解析 SQL 并提取表级 Hint；`EXPLAIN` 则从其包装语句提取。
fn parsed_hints(sql: &str) -> Vec<TableOptimizerHint> {
    let mut parser = parser_dependency::New();
    let statement = parser
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse hint endpoint {sql:?}: {error}"));
    if let Some(explain) = statement
        .as_any()
        .downcast_ref::<parser_ast_dependency::ExplainStmt>()
    {
        return ExtractTableHintsFromStmtNode(
            explain
                .stmt
                .as_deref()
                .expect("EXPLAIN must wrap a statement"),
            None,
        );
    }
    ExtractTableHintsFromStmtNode(statement.as_ref(), None)
}

/// 将 SQL 中的 Hint 解析为语句级 `StmtHints`，要求无告警。
fn parsed_statement_hints(sql: &str) -> hint_dependency::StmtHints {
    let _guard = lock_restricted_hint_checker();
    let (statement, _, warnings) = ParseStmtHints(
        parsed_hints(sql),
        allow_set_var,
        no_hypothetical_index,
        "test".to_owned(),
        0,
    );
    assert!(warnings.is_empty(), "{sql}: {warnings:?}");
    statement
}

/// 收集 Hint 告警文本的测试桩。
#[derive(Default)]
struct Warnings(Vec<String>);

impl hintWarnHandler for Warnings {
    fn SetHintWarning(&mut self, warning: String) {
        self.0.push(warning);
    }

    fn SetHintWarningFromError(&mut self, error: &dyn std::error::Error) {
        self.0.push(error.to_string());
    }
}

/// 全局系统变量访问器测试桩：读未知变量报错，写操作空成功。
#[derive(Default)]
struct TestGlobalVariables;

impl GlobalVarAccessor for TestGlobalVariables {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _context: &VariableContext,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

#[test]
/// 同名 `set_var` 保留首次赋值，且不泄漏到下一条语句。
fn set_var_hints_keep_first_value_and_do_not_leak_into_the_next_statement() {
    let _guard = lock_restricted_hint_checker();
    RegisterRestrictedHintChecker(|_| None);
    // 同一变量两次 set_var：应保留首次值，并对第二次发出告警。
    let hints = vec![
        hint(
            "set_var",
            HintData::SetVar(HintSetVar {
                VarName: "timestamp".to_owned(),
                Value: "1".to_owned(),
            }),
        ),
        hint(
            "set_var",
            HintData::SetVar(HintSetVar {
                VarName: "timestamp".to_owned(),
                Value: "2".to_owned(),
            }),
        ),
        hint(
            "set_var",
            HintData::SetVar(HintSetVar {
                VarName: "tidb_default_string_match_selectivity".to_owned(),
                Value: "0.3".to_owned(),
            }),
        ),
    ];
    let (statement, offsets, warnings) = ParseStmtHints(
        hints,
        allow_set_var,
        no_hypothetical_index,
        "test".to_owned(),
        0,
    );
    assert_eq!(
        statement.SetVars.get("timestamp").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        statement
            .SetVars
            .get("tidb_default_string_match_selectivity")
            .map(String::as_str),
        Some("0.3")
    );
    assert_eq!(offsets, vec![0, 2]);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].to_string().contains("set_var(timestamp=2)"));

    let (next, _, warnings) = ParseStmtHints(
        Vec::new(),
        allow_set_var,
        no_hypothetical_index,
        "test".to_owned(),
        0,
    );
    assert!(next.SetVars.is_empty());
    assert!(warnings.is_empty());
}

#[test]
/// 覆盖 Go 侧各 `set_var` 入口：查询/绑定/EXPLAIN/偏序索引开关等。
fn all_go_set_var_endpoints_parse_their_statement_scoped_values() {
    let cases = [
        (
            "timestamp query",
            "select /*+ set_var(timestamp=1) */ @@timestamp + 41",
            "timestamp",
            "1",
        ),
        (
            "timestamp binding SQL",
            "select /*+ set_var(timestamp=1) */ @@timestamp + 41",
            "timestamp",
            "1",
        ),
        (
            "binding max execution time",
            "select /*+ set_var(max_execution_time=1234) */ * from foo where a = 1",
            "max_execution_time",
            "1234",
        ),
        (
            "query max execution time",
            "select /*+ set_var(max_execution_time=2222) */ * from foo where a = 1",
            "max_execution_time",
            "2222",
        ),
        (
            "decimal session variable",
            "select /*+ set_var(tidb_default_string_match_selectivity=0.3) */ @@tidb_default_string_match_selectivity",
            "tidb_default_string_match_selectivity",
            "0.3",
        ),
        (
            "EXPLAIN statement",
            "explain select /*+ set_var(max_execution_time=100) */ @@max_execution_time",
            "max_execution_time",
            "100",
        ),
        (
            "partial ordered index COST",
            "select /*+ set_var(tidb_opt_partial_ordered_index_for_topn=COST) */ * from t order by b limit 10",
            "tidb_opt_partial_ordered_index_for_topn",
            "COST",
        ),
        (
            "partial ordered index DISABLE",
            "select /*+ set_var(tidb_opt_partial_ordered_index_for_topn=DISABLE) */ @@tidb_opt_partial_ordered_index_for_topn",
            "tidb_opt_partial_ordered_index_for_topn",
            "DISABLE",
        ),
    ];

    // 逐条校验语句级 SetVars 映射与无 Hint 的干净语句。
    for (name, sql, variable, expected) in cases {
        let statement = parsed_statement_hints(sql);
        assert_eq!(
            statement.SetVars.get(variable).map(String::as_str),
            Some(expected),
            "{name}"
        );
    }

    let clean_statement = parsed_statement_hints("select @@max_execution_time");
    assert!(clean_statement.SetVars.is_empty());
}

#[test]
/// 小数选择率与偏序 TopN 变量经系统变量校验/会话 Hook 读写。
fn decimal_and_partial_ordered_values_run_through_system_variable_hooks() {
    register_builtin_sysvars();
    let cases = [
        (
            vardef_dependency::TiDBDefaultStrMatchSelectivity,
            "0.3",
            "0.3",
        ),
        (
            vardef_dependency::TiDBOptPartialOrderedIndexForTopN,
            "COST",
            "COST",
        ),
        (
            vardef_dependency::TiDBOptPartialOrderedIndexForTopN,
            "disable",
            "DISABLE",
        ),
    ];
    let mut variables = SessionVars::new(Box::<TestGlobalVariables>::default());
    for (name, input, expected) in cases {
        let system_variable = GetSysVar(name).unwrap_or_else(|| panic!("registered sysvar {name}"));
        let normalized = system_variable
            .Validate(&mut variables, input, vardef_dependency::ScopeSession)
            .unwrap_or_else(|error| panic!("validate {name}={input}: {error}"));
        assert_eq!(normalized, expected);
        system_variable
            .SetSessionFromHook(&mut variables, &normalized)
            .unwrap_or_else(|error| panic!("apply {name}={normalized}: {error}"));
        assert_eq!(
            system_variable
                .GetSessionFromHook(&mut variables)
                .unwrap_or_else(|error| panic!("read {name}: {error}")),
            expected
        );
    }

    let partial_ordered = GetSysVar(vardef_dependency::TiDBOptPartialOrderedIndexForTopN)
        .expect("partial ordered TopN sysvar");
    // 偏序 TopN 仅接受 COST/DISABLE 等枚举，布尔写法必须拒绝。
    for invalid in ["ON", "OFF", "0", "1", "true", "false", "yes", "no"] {
        assert!(
            partial_ordered
                .Validate(&mut variables, invalid, vardef_dependency::ScopeSession)
                .is_err(),
            "{invalid} must not enable the COST-only optimization"
        );
    }
}

#[test]
/// 从真实 SQL 文本提取 `write_slow_log` Hint。
fn write_slow_log_is_extracted_from_the_real_go_sql() {
    let statement = parsed_statement_hints("select /*+ write_slow_log */ * from t where a = 1");
    assert!(statement.WriteSlowLog);
    assert!(!parsed_statement_hints("select * from t where a = 1").WriteSlowLog);
}

#[test]
/// 受限计划 Hint（如 `IGNORE_INDEX`）被解析器过滤并产生告警。
fn restricted_binding_plan_hint_is_filtered_by_plan_hint_parser() {
    let _guard = lock_restricted_hint_checker();
    RegisterRestrictedHintChecker(|name| {
        (name == "ignore_index").then(|| {
            hint_dependency::errors::NewNoStackError(
                "the IGNORE_INDEX() optimizer hint is restricted under the current security policy and is ignored",
            )
        })
    });
    let hints = parsed_hints(
        "select /*+ ignore_index(sem_binding_hint_t, idx_a) */ a from sem_binding_hint_t where a = 1",
    );
    let mut processor = NewQBHintHandler(None);
    let mut warnings = Warnings::default();
    let (plan, _) = ParsePlanHints(
        hints,
        0,
        "test".to_owned(),
        &mut processor,
        false,
        false,
        false,
        true,
        &mut warnings,
    )
    .expect("restricted plan hint is ignored with a warning");
    assert!(plan.IndexHintList.is_empty());
    assert_eq!(warnings.0.len(), 1);
    assert!(warnings.0[0].contains("IGNORE_INDEX() optimizer hint is restricted"));
    RegisterRestrictedHintChecker(|_| None);
}

#[test]
/// `write_slow_log` 保留，受限 `set_var` 被规范过滤器剔除。
fn write_slow_log_and_restricted_hints_use_the_canonical_filter() {
    let _guard = lock_restricted_hint_checker();
    RegisterRestrictedHintChecker(|name| {
        (name == "set_var").then(|| {
            hint_dependency::errors::NewNoStackError(
                "the SET_VAR() optimizer hint is restricted under the current security policy and is ignored",
            )
        })
    });
    let hints = vec![
        hint(HintWriteSlowLog, HintData::None),
        hint(
            "set_var",
            HintData::SetVar(HintSetVar {
                VarName: "timestamp".to_owned(),
                Value: "1".to_owned(),
            }),
        ),
    ];
    let (statement, offsets, warnings) = ParseStmtHints(
        hints,
        allow_set_var,
        no_hypothetical_index,
        "test".to_owned(),
        0,
    );
    assert!(statement.WriteSlowLog);
    assert!(
        statement
            .OriginalTableHints
            .iter()
            .all(|hint| hint.HintName.L != "set_var")
    );
    assert_eq!(offsets, Vec::<i32>::new());
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .to_string()
            .contains("restricted under the current security policy")
    );

    RegisterRestrictedHintChecker(|_| None);
}
