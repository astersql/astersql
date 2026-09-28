// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// AST 语句只读判定工具，对照 Go 的 `util.go` / `IsReadOnly`。
//
// 「只读」指执行该语句不会修改数据库数据或会话/全局系统变量状态。
// 计划缓存等场景依赖此判定，避免把写副作用语句误判为可安全复用。

#[path = "../util/escape.rs"]
mod parser_escape;
pub use self::parser_escape::UnescapeChar;

use crate::{
    AdminStmt, AdminStmtType, DoStmt, ExplainStmt, Node, SelectLockType, SelectStmt,
    SetOprSelectList, SetOprStmt, ShowStmt, TraceStmt, VariableExpr, Visitor,
};

/// The value used when a size was not specified.
/// 未指定长度/大小时使用的哨兵值（u64 最大值）。
pub const UNSPECIFIED_SIZE: u64 = u64::MAX;

/// Go-compatible spelling retained for callers migrated mechanically.
/// 保留 Go 风格拼写的别名常量，供机械迁移调用方继续使用。
#[allow(non_upper_case_globals)]
pub const UnspecifiedSize: u64 = UNSPECIFIED_SIZE;

/// Returns whether `node` can be evaluated without changing database or session state.
///
/// When `check_global_vars` is false, assignments to global system variables inside a
/// select are deliberately ignored, matching Go's `IsReadOnly` behavior.
///
/// 判断节点是否只读。`check_global_vars` 为 false 时，故意忽略 SELECT 内对全局
/// 系统变量的赋值，与 Go `IsReadOnly` 行为一致。
pub fn is_read_only(node: &dyn Node, check_global_vars: bool) -> bool {
    // SELECT：带行锁（FOR UPDATE / FOR SHARE 等）视为写意图，非只读。
    if let Some(statement) = node.as_any().downcast_ref::<SelectStmt>() {
        if statement.lock_info.as_ref().is_some_and(|lock| {
            matches!(
                lock.lock_type,
                SelectLockType::ForUpdate
                    | SelectLockType::ForUpdateNoWait
                    | SelectLockType::ForUpdateWaitN
                    | SelectLockType::ForShare
                    | SelectLockType::ForShareNoWait
            )
        }) {
            return false;
        }

        // 不检查全局变量时，无锁 SELECT 直接视为只读。
        if !check_global_vars {
            return true;
        }

        // 遍历子树，检测形如对全局系统变量赋值的 SET_VAR 写法。
        let mut checker = ReadOnlyChecker { read_only: true };
        statement.accept(&mut checker);
        return checker.read_only;
    }

    // EXPLAIN：非 ANALYZE 始终只读；ANALYZE 则递归检查被解释语句。
    if let Some(statement) = node.as_any().downcast_ref::<ExplainStmt>() {
        return !statement.analyze
            || statement
                .stmt
                .as_deref()
                .is_some_and(|stmt| is_read_only(stmt, check_global_vars));
    }
    // DO / SHOW 本身不改库，视为只读。
    if node.as_any().is::<DoStmt>() || node.as_any().is::<ShowStmt>() {
        return true;
    }

    // UNION/INTERSECT/EXCEPT 等集合运算：所有分支都只读才算只读。
    if let Some(statement) = node.as_any().downcast_ref::<SetOprStmt>() {
        return statement
            .select_list
            .selects
            .iter()
            .all(|select| is_read_only(select.as_ref(), check_global_vars));
    }
    if let Some(list) = node.as_any().downcast_ref::<SetOprSelectList>() {
        return list
            .selects
            .iter()
            .all(|select| is_read_only(select.as_ref(), check_global_vars));
    }

    // ADMIN：仅白名单内的查询类子命令视为只读。
    if let Some(statement) = node.as_any().downcast_ref::<AdminStmt>() {
        return matches!(
            statement.statement_type,
            AdminStmtType::ShowDdl
                | AdminStmtType::ShowDdlJobs
                | AdminStmtType::ShowSlow
                | AdminStmtType::CaptureBindings
                | AdminStmtType::ShowNextRowId
                | AdminStmtType::ShowDdlJobQueries
                | AdminStmtType::ShowDdlJobQueriesWithRange
        );
    }
    // TRACE：只读性取决于被追踪的内部语句。
    if let Some(statement) = node.as_any().downcast_ref::<TraceStmt>() {
        return is_read_only(statement.Stmt.as_ref(), check_global_vars);
    }
    false
}

/// Go-compatible spelling retained for callers migrated mechanically.
/// Go 风格 `IsReadOnly` 入口，委托给 `is_read_only`。
#[allow(non_snake_case)]
pub fn IsReadOnly(node: &dyn Node, check_global_vars: bool) -> bool {
    is_read_only(node, check_global_vars)
}

/// Visitor that detects the `SET_VAR` shape used for global system-variable writes.
/// 访问者：检测全局系统变量赋值形态；一旦发现则将 `read_only` 置为 false。
pub struct ReadOnlyChecker {
    /// 当前子树是否仍判定为只读。
    pub read_only: bool,
}

impl Visitor for ReadOnlyChecker {
    fn enter(&mut self, input: &dyn Node) -> bool {
        // 系统变量且带赋值表达式，即对全局/会话变量的写操作。
        if let Some(variable) = input.as_any().downcast_ref::<VariableExpr>() {
            if variable.is_system && variable.value.is_some() {
                self.read_only = false;
                return true;
            }
        }
        false
    }

    fn leave(&mut self, _input: &dyn Node) -> bool {
        self.read_only
    }
}
