// Copyright 2026 PingCAP, Inc.
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

// JobMDL 迁移单元测试：校验默认零值与表 ID 去重语义。
//
// 与 Go 零值一致：`ver == 0` 且 `table_ids` 为空；`HashSet` 保证同一表 ID 只保留一次。

use super::JobMDL;

/// 默认构造应与 Go 结构体零值一致。
#[test]
fn job_mdl_default_matches_go_zero_value() {
    let job = JobMDL::default();

    assert_eq!(job.ver, 0);
    assert!(job.table_ids.is_empty());
}

/// 重复插入同一表 ID 时集合长度只计一次。
#[test]
fn job_mdl_tracks_each_table_once() {
    let mut job = JobMDL::default();
    job.ver = -7;
    // 故意重复 42，验证 HashSet 去重。
    job.table_ids.extend([42, 42, 81]);

    assert_eq!(job.ver, -7);
    assert_eq!(job.table_ids.len(), 2);
    assert!(job.table_ids.contains(&42));
    assert!(job.table_ids.contains(&81));
}
