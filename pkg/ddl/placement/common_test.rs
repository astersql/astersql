// Copyright 2021 PingCAP, Inc.
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

// Placement 公共常量与 [`GroupID`] 辅助函数的单元测试。
//
// 校验 Bundle（规则组）ID 的字符串格式与 Go 版本一致：
// 前缀为 `TiDB_DDL_`，后接对象的整型 ID（含负数）。

// Copyright 2026 AsterSQL.
use super::*;

/// 验证 [`GroupID`] 对正数与负数均按 Go 侧格式拼接。
#[test]
fn group_id_matches_go_format() {
    assert_eq!(GroupID(1), "TiDB_DDL_1");
    assert_eq!(GroupID(90), "TiDB_DDL_90");
    assert_eq!(GroupID(-1), "TiDB_DDL_-1");
}
