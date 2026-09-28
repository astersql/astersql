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

// cardinality 包测试入口与公共夹具。
//
// `StatsNode` 的辅助函数，供同包其他 `*_test.rs` 复用。

use std::sync::Once;

use crate::{OptionalTopNExt, StatsNode};

/// 保证公共测试环境只初始化一次。
static COMMON_TEST_SETUP: Once = Once::new();

/// 调用 `testsetup::SetupForCommonTest`，供各 cardinality 单测在开头调用。
pub(crate) fn setup_for_cardinality_test() {
    COMMON_TEST_SETUP.call_once(testsetup::SetupForCommonTest);
}

/// Rust 测试只读 Go `BookKeeper` 加载的 cardinality golden 输入与输出。
///
/// Rust 测试框架没有 Go `TestMain` 的进程级写回钩子；现有测试也不在录制
/// 模式下更新 golden 文件，因此用编译期嵌入保留同一套件的读取生命周期。
pub(crate) fn GetCardinalitySuiteData() -> (&'static str, &'static str) {
    (
        include_str!("testdata/cardinality_suite_in.json"),
        include_str!("testdata/cardinality_suite_out.json"),
    )
}

/// 构造带指定 ID、列掩码与列数的模拟 `StatsNode`，对应 Go 测试辅助。
pub fn MockStatsNode(id: i64, mask: i64, num_columns: usize) -> Box<StatsNode> {
    Box::new(StatsNode {
        ID: id,
        mask,
        numCols: num_columns,
        ..Default::default()
    })
}

#[test]
fn test_main_lifecycle_and_stats_node_helper() {
    setup_for_cardinality_test();

    let (input, output) = GetCardinalitySuiteData();
    assert!(input.trim_start().starts_with('['));
    assert!(output.trim_start().starts_with('['));
    assert!(input.contains("TestCollationColumnEstimate"));
    assert!(output.contains("TestCollationColumnEstimate"));

    let node = MockStatsNode(7, 0b101, 3);
    assert_eq!(node.ID, 7);
    assert_eq!(node.mask, 0b101);
    assert_eq!(node.numCols, 3);
}

/// Go 的 `(*TopN)(nil)` 查询均返回零，Rust 的两个可空边界必须一致。
#[test]
fn optional_top_n_none_is_zero() {
    let owned: Option<crate::statistics::TopN> = None;
    assert_eq!(owned.Num(), 0);
    assert_eq!(owned.MinCount(), 0);
    assert_eq!(owned.TotalCount(), 0);

    let borrowed: Option<&crate::statistics::TopN> = None;
    assert_eq!(borrowed.Num(), 0);
    assert_eq!(borrowed.MinCount(), 0);
    assert_eq!(borrowed.TotalCount(), 0);
}

/// `NewNoStackError` 保留消息文本，不添加上下文或栈包装。
#[test]
fn no_stack_error_preserves_message() {
    let error = crate::errors::NewNoStackError("cardinality test error");
    assert_eq!(error.to_string(), "cardinality test error");
}
