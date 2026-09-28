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

// 已移除系统变量检测的单元测试，对齐 Go `TestRemovedOpt`。

use task_variable::removed::{CheckSysVarIsRemoved, IsRemovedSysVar};
use task_variable::vardef;

/// 仍支持的变量名常量，用于与已移除项对照。
const TIDB_ENABLE_ALTER_PLACEMENT: &str = "tidb_enable_alter_placement";

// Go: TestRemovedOpt.
/// 未移除变量应通过检查；已移除的 `tidb_enable_alter_placement` 应报错。
#[test]
fn test_removed_opt() {
    assert!(CheckSysVarIsRemoved(vardef::TiDBEnable1PC).is_ok());
    assert!(!IsRemovedSysVar(vardef::TiDBEnable1PC));
    assert!(CheckSysVarIsRemoved(TIDB_ENABLE_ALTER_PLACEMENT).is_err());
    assert!(IsRemovedSysVar(TIDB_ENABLE_ALTER_PLACEMENT));
}
