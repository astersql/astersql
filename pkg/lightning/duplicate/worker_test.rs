// Copyright 2023 PingCAP, Inc.
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

// `gen_split_key` 单元测试。
//
// 覆盖相等键、前缀关系、中点拆分以及相邻字节时补 `0xff` 的边界情形。

use super::gen_split_key;

// Go's byte arithmetic wraps even for descending, non-prefix inputs.
#[test]
fn split_key_preserves_go_byte_overflow() {
    assert_eq!(gen_split_key(&[255], &[2]), vec![0]);
    assert_eq!(gen_split_key(&[255], &[0]), vec![255, 255]);
}

/// 一组起止键与期望拆分键的用例。
struct SplitKeyCase {
    /// 区间起始用户键。
    start_key: Vec<u8>,
    /// 区间结束用户键。
    end_key: Vec<u8>,
    /// 期望生成的拆分键。
    split_key: Vec<u8>,
}

/// 验证 `gen_split_key` 在多种字节序列下的输出与 Go 行为一致。
#[test]
fn test_gen_split_key() {
    let test_cases = vec![
        // 起止相同：原样返回。
        SplitKeyCase {
            start_key: vec![1, 2],
            end_key: vec![1, 2],
            split_key: vec![1, 2],
        },
        // start 为 end 前缀：在分歧处取 end 字节一半。
        SplitKeyCase {
            start_key: vec![1, 2],
            end_key: vec![1, 2, 3, 4, 5],
            split_key: vec![1, 2, 1],
        },
        // 分歧字节有空隙：取中点。
        SplitKeyCase {
            start_key: vec![1, 2, 3, 4, 5, 6],
            end_key: vec![1, 2, 5, 6, 7, 8],
            split_key: vec![1, 2, 4],
        },
        // 分歧字节相邻：沿 start 补 0xff。
        SplitKeyCase {
            start_key: vec![1, 2, 3, 4],
            end_key: vec![1, 2, 4, 5],
            split_key: vec![1, 2, 3, 0xff],
        },
        SplitKeyCase {
            start_key: vec![1, 2, 3, 0xff, 4],
            end_key: vec![1, 2, 4, 5],
            split_key: vec![1, 2, 3, 0xff, 0xff],
        },
        // start 尾部全为 0xff：再追加一个 0xff。
        SplitKeyCase {
            start_key: vec![1, 2, 3, 0xff, 0xff],
            end_key: vec![1, 2, 4, 5],
            split_key: vec![1, 2, 3, 0xff, 0xff, 0xff],
        },
    ];

    for test_case in test_cases {
        assert_eq!(
            gen_split_key(&test_case.start_key, &test_case.end_key),
            test_case.split_key
        );
    }
}
