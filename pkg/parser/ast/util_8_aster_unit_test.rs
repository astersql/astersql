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

// `is_read_only` 行为对齐测试，对照 Go `IsReadOnly` 用例。
//
// 覆盖 DML/DO/SHOW、EXPLAIN/TRACE 包装、SELECT 行锁、集合运算、
// ADMIN 白名单，以及全局系统变量赋值的可选检查。

use super::*;

/// 基础语句与 EXPLAIN/TRACE 包装后的只读性应与 Go 一致。
#[test]
fn classifies_basic_and_wrapped_statements_like_go() {
    assert!(!is_read_only(&DeleteStmt::default(), true));
    assert!(!is_read_only(&InsertStmt::default(), true));
    assert!(!is_read_only(&UpdateStmt::default(), true));
    assert!(is_read_only(&DoStmt::default(), true));
    assert!(is_read_only(&ShowStmt::default(), true));

    // 普通 EXPLAIN 不执行被解释语句，即使内层是 INSERT 也只读；
    // EXPLAIN ANALYZE 会真实执行，故继承内层写语义。
    let explain_insert = ExplainStmt::new(false, Box::new(InsertStmt::default()));
    assert!(is_read_only(&explain_insert, true));
    let analyze_insert = ExplainStmt::new(true, Box::new(InsertStmt::default()));
    assert!(!is_read_only(&analyze_insert, true));
    let analyze_select = ExplainStmt::new(true, Box::new(SelectStmt::default()));
    assert!(is_read_only(&analyze_select, true));

    let trace_select = TraceStmt::new(Box::new(SelectStmt::default()));
    assert!(is_read_only(&trace_select, true));
    let trace_delete = TraceStmt::new(Box::new(DeleteStmt::default()));
    assert!(!is_read_only(&trace_delete, true));
}

/// 所有写意图行锁（FOR UPDATE / FOR SHARE 族）都应判定为非只读。
#[test]
fn rejects_all_mutating_select_locks() {
    for lock_type in [
        SelectLockType::ForUpdate,
        SelectLockType::ForUpdateNoWait,
        SelectLockType::ForUpdateWaitN,
        SelectLockType::ForShare,
        SelectLockType::ForShareNoWait,
    ] {
        let statement = SelectStmt::with_lock(lock_type);
        assert!(!is_read_only(&statement, true), "{lock_type:?}");
    }

    assert!(is_read_only(
        &SelectStmt::with_lock(SelectLockType::None),
        true
    ));
}

/// 集合运算要求每一个 SELECT 分支都只读，整体才只读。
#[test]
fn set_operations_are_read_only_only_when_every_select_is_read_only() {
    let read_only = || Box::new(SelectStmt::default()) as Box<dyn Node>;
    let locked = || Box::new(SelectStmt::with_lock(SelectLockType::ForUpdate)) as Box<dyn Node>;

    let list = SetOprSelectList::new(vec![read_only(), read_only()]);
    assert!(is_read_only(&list, true));
    let list = SetOprSelectList::new(vec![read_only(), locked()]);
    assert!(!is_read_only(&list, true));

    let statement = SetOprStmt::new(SetOprSelectList::new(vec![read_only(), read_only()]));
    assert!(is_read_only(&statement, true));
    let statement = SetOprStmt::new(SetOprSelectList::new(vec![locked(), read_only()]));
    assert!(!is_read_only(&statement, true));
}

/// ADMIN 查询类白名单与 Go 一致；CheckTable 等维护类命令非只读。
#[test]
fn admin_read_only_whitelist_matches_go() {
    for statement_type in [
        AdminStmtType::ShowDdl,
        AdminStmtType::ShowDdlJobs,
        AdminStmtType::ShowSlow,
        AdminStmtType::CaptureBindings,
        AdminStmtType::ShowNextRowId,
        AdminStmtType::ShowDdlJobQueries,
        AdminStmtType::ShowDdlJobQueriesWithRange,
    ] {
        assert!(is_read_only(&AdminStmt::new(statement_type), true));
    }
    assert!(!is_read_only(
        &AdminStmt::new(AdminStmtType::CheckTable),
        true
    ));
}

/// 全局系统变量赋值：`check_global_vars=true` 时视为写；关闭检查则仍只读。
#[test]
fn global_system_variable_assignment_is_optional_write_check() {
    // VariableExpr::new(is_system, has_value)
    let read = SelectStmt::with_child(Box::new(VariableExpr::new(true, false)));
    assert!(is_read_only(&read, true));

    let user_assignment = SelectStmt::with_child(Box::new(VariableExpr::new(false, true)));
    assert!(is_read_only(&user_assignment, true));

    let system_assignment = SelectStmt::with_child(Box::new(VariableExpr::new(true, true)));
    assert!(!is_read_only(&system_assignment, true));
    assert!(is_read_only(&system_assignment, false));
}

/// `UNSPECIFIED_SIZE` 哨兵值必须等于 u64::MAX。
#[test]
fn unspecified_size_is_max_u64() {
    assert_eq!(UNSPECIFIED_SIZE, u64::MAX);
}
