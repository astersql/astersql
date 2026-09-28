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

// `system` 模块保留全局 ID 区间判定的单元测试。
//
// 对齐 Go：验证 `(lower, upper]` 开闭区间语义，下界本身仍属用户可用 ID。

// 本文件由 pkg/meta/metadef/system_test.go 迁移而来，保留 Reserved ID 边界断言。
use super::{IsReservedID, ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound};

/// 验证保留 ID 区间：上界与下界+1 为真，下界与普通 ID 为假。
#[test]
fn test_is_reserved_id() {
    // 上边界本身属于保留 ID 区间，Go 测试用它确认闭区间的右端点。
    assert!(IsReservedID(ReservedGlobalIDUpperBound));
    // 下边界加一属于保留区间，说明 lower bound 本身不是第一个保留值。
    assert!(IsReservedID(ReservedGlobalIDLowerBound + 1));
    // 下边界本身与普通业务 ID 都不是保留 ID，保留 Go 的两个反例。
    assert!(!IsReservedID(ReservedGlobalIDLowerBound));
    assert!(!IsReservedID(123));
}
