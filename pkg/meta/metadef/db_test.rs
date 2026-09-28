// Copyright 2025 PingCAP, Inc.
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

// `db` 模块库名分类函数的单元测试。
//
// 对齐 Go `db_test.go`：覆盖内存 schema、系统相关库与严格 SystemDB 三类判定。

// 本文件由 pkg/meta/metadef/db_test.go 迁移而来，保留 Go 测试断言顺序。
use super::{IsMemDB, IsSystemDB, IsSystemRelatedDB};

/// 三个内存 schema 应为真，普通 `mysql` 库名应为假。
#[test]
fn test_is_mem_db() {
    // IsMemDB 约定输入为小写库名，Go 测试覆盖三个内存 schema 与普通 mysql 库的反例。
    assert!(IsMemDB("information_schema"));
    assert!(IsMemDB("performance_schema"));
    assert!(IsMemDB("metrics_schema"));
    assert!(!IsMemDB("mysql"));
}

/// SystemRelated 应包含 mysql / sys / workload_schema，且对大小写敏感的反例成立。
#[test]
fn test_is_system_related_db() {
    // SystemRelated 比 SystemDB 范围更大：mysql、sys 与 workload_schema 都要判定为相关系统库。
    assert!(IsSystemRelatedDB("mysql"));
    assert!(IsSystemRelatedDB("sys"));
    assert!(IsSystemRelatedDB("workload_schema"));
    // Go 这里刻意使用大写 INFORMATION_SCHEMA，保留大小写敏感的反例语义。
    assert!(!IsSystemRelatedDB("INFORMATION_SCHEMA"));
}

/// IsSystemDB 只接受 `mysql`，`sys` 虽为 system-related 但不能混入。
#[test]
fn test_is_system_db() {
    // IsSystemDB 只接受 mysql.SystemDB；sys 虽然是 system-related，但不能被纳入 system DB。
    assert!(IsSystemDB("mysql"));
    assert!(!IsSystemDB("sys"));
}
