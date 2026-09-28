// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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
// 任务类型与整型编码双向转换的单元测试。

// limitations under the License.

use super::*;

/// 已知类型往返一致，空类型编码为 0。
#[test]
fn test_task_type() {
    let cases = [
        (TaskTypeExample, 1),
        (ImportInto, 2),
        (Backfill, 3),
        ("", 0),
    ];
    for (task_type, value) in cases {
        assert_eq!(Type2Int(task_type), value);
    }
    for (task_type, value) in cases {
        assert_eq!(Int2Type(value), task_type);
    }
}
