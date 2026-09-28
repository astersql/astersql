// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// GROUPING 标量内核回归测试。
//
// 对应 Go `builtin_grouping_test.go` 中三种模式（BitAnd / NumericCmp /
// NumericSet）的 fixture；用单参数 mark 集构造 `GroupingSig` 后断言 `eval`。

use std::collections::HashSet;

use crate::builtin_grouping_kernel::{GroupingMode, GroupingSig};

/// 单条 GROUPING 用例：输入 id、模式、marks 与期望整数结果。
#[derive(Debug)]
struct GroupingCase {
    grouping_id: u64,
    mode: GroupingMode,
    grouping_ids: &'static [u64],
    expected_result: i64,
}

/// 用给定模式与 mark 列表初始化可用的 `GroupingSig`。
fn create_grouping_func(mode: GroupingMode, grouping_ids: &[u64]) -> GroupingSig {
    let mut signature = GroupingSig::new();
    signature
        .set_metadata(
            mode,
            vec![grouping_ids.iter().copied().collect::<HashSet<_>>()],
        )
        .expect("the Go fixture contains valid GROUPING metadata");
    signature
}

/// 遍历 BitAnd / NumericCmp / NumericSet 全部 Go fixture 并断言结果。
#[test]
fn test_grouping() {
    let tests = [
        // GroupingMode_ModeBitAnd
        // 按位与语义下的典型 id/mask 组合。
        GroupingCase {
            grouping_id: 1,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[1],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 1,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[3],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 1,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[6],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[1],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[3],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[6],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 4,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[2],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 4,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[4],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 4,
            mode: GroupingMode::BitAnd,
            grouping_ids: &[6],
            expected_result: 0,
        },
        // GroupingMode_ModeNumericCmp
        // 数值比较语义。
        GroupingCase {
            grouping_id: 0,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[0],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 0,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[2],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[0],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[1],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[2],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericCmp,
            grouping_ids: &[3],
            expected_result: 1,
        },
        // GroupingMode_ModeNumericSet
        // 集合包含语义。
        GroupingCase {
            grouping_id: 1,
            mode: GroupingMode::NumericSet,
            grouping_ids: &[1, 2],
            expected_result: 0,
        },
        GroupingCase {
            grouping_id: 1,
            mode: GroupingMode::NumericSet,
            grouping_ids: &[2],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericSet,
            grouping_ids: &[1, 3],
            expected_result: 1,
        },
        GroupingCase {
            grouping_id: 2,
            mode: GroupingMode::NumericSet,
            grouping_ids: &[2, 3],
            expected_result: 0,
        },
    ];

    // 逐条构造签名、求值并与期望比对；失败信息带出 id/模式/marks。
    for test_case in tests {
        let grouping_func = create_grouping_func(test_case.mode, test_case.grouping_ids);
        let actual_result = grouping_func
            .eval(test_case.grouping_id)
            .expect("GROUPING evaluation should accept initialized metadata");

        assert_eq!(
            actual_result, test_case.expected_result,
            "grouping_id={}, mode={:?}, grouping_ids={:?}",
            test_case.grouping_id, test_case.mode, test_case.grouping_ids,
        );
    }
}
