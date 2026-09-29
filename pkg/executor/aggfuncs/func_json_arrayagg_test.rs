// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// JSON_ARRAYAGG 聚合的单元测试。
//
// 可执行用例验证元素顺序保持、merge 追加顺序以及 reset 后结果为空。

use crate::aggfuncs::{
    DEF_BOOL_SIZE, DEF_DURATION_SIZE, DEF_FLOAT64_SIZE, DEF_INT64_SIZE, DEF_INTERFACE_SIZE,
    DEF_TIME_SIZE, DEF_UINT64_SIZE, SpillValue,
};
use crate::func_json_arrayagg::JsonArrayAgg;
use astersql_util_serialization::types;

/// 校验有序收集、merge 保持左右顺序，以及 reset 后 result 为 None。
#[test]
fn json_arrayagg_preserves_order_memory_accounting_and_merge_order() {
    let empty = JsonArrayAgg::default();
    assert_eq!(empty.result(), None);

    // 左侧先写入 Int64/String，再 merge 右侧 Uint64，顺序应为 1, "a", 2。
    let mut left = JsonArrayAgg::default();
    assert_eq!(
        left.update([
            SpillValue::Bool(true),
            SpillValue::Int64(1),
            SpillValue::Uint64(2),
            SpillValue::Float64(3.0),
            SpillValue::String("a".into()),
        ]),
        DEF_INTERFACE_SIZE * 5
            + DEF_BOOL_SIZE
            + DEF_INT64_SIZE
            + DEF_UINT64_SIZE
            + DEF_FLOAT64_SIZE
            + 1
    );
    let mut right = JsonArrayAgg::default();
    right.update([SpillValue::String("right".into())]);
    let source_before_merge = right.clone();
    left.merge(&right);
    assert_eq!(right, source_before_merge);
    assert_eq!(
        left.result(),
        Some(
            [
                SpillValue::Bool(true),
                SpillValue::Int64(1),
                SpillValue::Uint64(2),
                SpillValue::Float64(3.0),
                SpillValue::String("a".into()),
                SpillValue::String("right".into()),
            ]
            .as_slice()
        )
    );
    left.reset();
    assert_eq!(left.result(), None);
}

#[test]
fn json_arrayagg_accounts_for_all_supported_variable_and_temporal_values() {
    let mut array = JsonArrayAgg::default();
    let binary_json = types::BinaryJSON {
        Value: vec![1, 2, 3],
        ..Default::default()
    };
    let opaque = types::Opaque {
        Buf: vec![4, 5],
        ..Default::default()
    };

    assert_eq!(
        array.update([
            SpillValue::BinaryJson(binary_json),
            SpillValue::Opaque(opaque),
            SpillValue::Time(types::Time::default()),
            SpillValue::Duration(types::Duration::default()),
        ]),
        DEF_INTERFACE_SIZE * 4 + (3 + 1) + (2 + 1) + DEF_TIME_SIZE + DEF_DURATION_SIZE
    );
}
