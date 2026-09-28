// Copyright 2026 AsterSQL.
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

// 标量子查询注册表与相关会话状态的单元测试。
//
// 覆盖：EXPLAIN 可恢复的线程亲和（thread-affine）计划上下文、
// 快照/还原不移动值、相关优化器变量与 fix 的排序重置，
// 以及用户变量类型字段与 LIKE 反斜杠转义默认值。

use std::rc::Rc;

use super::SessionVars;

/// 模拟 EXPLAIN 需要 downcast 回具体类型的标量子查询计划探针。
#[derive(Debug)]
struct ExplainPlanProbe {
    /// 探针标识，用于断言注册与还原后的同一性。
    id: i32,
    /// 线程亲和上下文句柄；用 `Rc` 验证指针相等而非仅值相等。
    thread_affine_context: Rc<String>,
}

/// 注册真实线程亲和上下文后，EXPLAIN 路径应能 downcast 并比对指针。
#[test]
fn registry_retains_real_thread_affine_explain_context() {
    let vars = SessionVars::new();
    let context = Rc::new("actual planner context".to_owned());

    vars.RegisterScalarSubQ(ExplainPlanProbe {
        id: 17,
        thread_affine_context: Rc::clone(&context),
    });

    assert_eq!(vars.ScalarSubqueryCount(), 1);
    // 在借用作用域内访问注册表，确认具体类型与 Rc 指针均未变。
    vars.WithScalarSubQueries(|registered| {
        let subquery = registered[0]
            .as_ref()
            .downcast_ref::<ExplainPlanProbe>()
            .expect("EXPLAIN must recover the registered concrete context");
        assert_eq!(subquery.id, 17);
        assert!(Rc::ptr_eq(&subquery.thread_affine_context, &context));
    });
}

/// 同一快照可反复 Restore：清空后再恢复不应移动或丢失注册值。
#[test]
fn scalar_subquery_snapshot_restores_repeatedly_without_moving_values() {
    let vars = SessionVars::new();
    vars.RegisterScalarSubQ(ExplainPlanProbe {
        id: 23,
        thread_affine_context: Rc::new("round state".to_owned()),
    });
    let snapshot = vars.SnapshotScalarSubQueries();

    // 先清空再还原，重复两轮以模拟多轮 EXPLAIN/优化器往返。
    vars.RestoreScalarSubQueries(Vec::new());
    vars.RestoreScalarSubQueries(snapshot.clone());
    assert_eq!(vars.ScalarSubqueryCount(), 1);
    vars.RestoreScalarSubQueries(Vec::new());
    vars.RestoreScalarSubQueries(snapshot);
    vars.WithScalarSubQueries(|registered| {
        assert_eq!(
            registered[0]
                .as_ref()
                .downcast_ref::<ExplainPlanProbe>()
                .unwrap()
                .id,
            23
        );
    });
}

/// 相关优化器变量名与 fix id 应按排序返回，并可由 Reset 一并清空。
#[test]
fn relevant_optimizer_inputs_are_sorted_and_reset_together() {
    let vars = SessionVars::new();

    // Go defaults recording to disabled, so calls outside the optimizer's
    // statement-scoped recording window must be ignored.
    vars.RecordRelevantOptVar("ignored_var");
    vars.RecordRelevantOptFix(99);
    assert_eq!(vars.RelevantOptVarsAndFixes(), (Vec::new(), Vec::new()));

    vars.ResetRelevantOptVarsAndFixes(true);
    vars.RecordRelevantOptVar("z_var");
    vars.RecordRelevantOptVar("a_var");
    vars.RecordRelevantOptVar("a_var");
    vars.RecordRelevantOptFix(9);
    vars.RecordRelevantOptFix(2);
    vars.RecordRelevantOptFix(2);
    assert_eq!(
        vars.RelevantOptVarsAndFixes(),
        (vec!["a_var".to_owned(), "z_var".to_owned()], vec![2, 9])
    );
    vars.ResetRelevantOptVarsAndFixes(false);
    assert_eq!(vars.RelevantOptVarsAndFixes(), (Vec::new(), Vec::new()));

    vars.RecordRelevantOptVar("ignored_after_reset");
    vars.RecordRelevantOptFix(100);
    assert_eq!(vars.RelevantOptVarsAndFixes(), (Vec::new(), Vec::new()));
}

/// 用户变量类型应存真实 `FieldType`，外部修改不影响会话内已存副本。
#[test]
fn session_user_var_types_are_real_locked_field_types() {
    let vars = SessionVars::new();
    let mut field_type = parser_ast::ast::FieldType::default();
    field_type.SetFlen(64);

    vars.SetUserVarType("answer", field_type.clone());
    // 本地再改 flen，不应影响已写入会话的类型快照。
    field_type.SetFlen(128);

    let mut loaded = vars
        .GetUserVarType("answer")
        .expect("stored user-variable type");
    assert_eq!(loaded.GetFlen(), 64);
    loaded.SetFlen(256);
    assert_eq!(vars.GetUserVarType("answer").unwrap().GetFlen(), 64);
}

/// `EnableNoBackslashEscapesInLike` 应与 Go 侧默认常量一致。
#[test]
fn no_backslash_escapes_like_uses_the_go_default() {
    let vars = SessionVars::new();
    assert_eq!(
        vars.EnableNoBackslashEscapesInLike,
        vardef::DefTiDBEnableNoBackslashEscapesInLike
    );
}
