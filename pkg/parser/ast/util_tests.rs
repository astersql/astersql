// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `util` 相关单元测试的聚合入口。
//
// 再导出测试所需的 AST 语句类型与只读判定 API，并挂载
// `util_8_aster_unit_test` 以覆盖 Go 对齐用例。

/// 再导出 `util` 模块公开项（含 `is_read_only` / `UNSPECIFIED_SIZE` 等）。
pub use crate::util::*;
/// 再导出本批测试用到的语句与表达式节点类型。
pub use crate::{
    AdminStmt, AdminStmtType, DeleteStmt, DoStmt, ExplainStmt, InsertStmt, Node, SelectLockType,
    SelectStmt, SetOprSelectList, SetOprStmt, ShowStmt, TraceStmt, UpdateStmt, VariableExpr,
};

/// 通过路径属性挂载 Aster 侧 util 只读判定单元测试。
#[path = "util_8_aster_unit_test.rs"]
mod util_8_aster_unit_test;
