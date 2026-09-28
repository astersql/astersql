// Copyright 2024 PingCAP, Inc.
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

// 统计 util JSON 对象的迁移期单元测试。
//
// 覆盖 `JSONTable::Sort` 对谓词列（PredicateColumns）按列 ID 排序，
// 以及 `JSONColumn::TotalMemoryUsage` 对直方图/CMSketch/FMSketch 等 protobuf 消息体积的合计。

#![cfg(test)]
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

#[path = "json_objects.rs"]
mod json_objects;

use json_objects::{JSONColumn, JSONPredicateColumn, JSONTable};
use protobuf::Message as _;
use std::collections::HashMap;

/// 构造仅含列 ID 的谓词列，用于排序断言。
fn predicate_column(id: i64) -> Box<JSONPredicateColumn> {
    Box::new(JSONPredicateColumn {
        LastUsedAt: None,
        LastAnalyzedAt: None,
        ID: id,
    })
}

/// 构造无统计消息载荷的空列，内存占用应为 0。
fn empty_column() -> JSONColumn {
    JSONColumn {
        Histogram: None,
        CMSketch: None,
        FMSketch: None,
        StatsVer: None,
        NullCount: 0,
        TotColSize: 0,
        LastUpdateVersion: 0,
        Correlation: 0.0,
    }
}

/// 验证 `Sort` 将 PredicateColumns 按 ID 升序排列（含负数与重复 ID）。
#[test]
fn sort_orders_predicate_columns_by_id() {
    let mut table = JSONTable {
        Columns: HashMap::new(),
        Indices: HashMap::new(),
        Partitions: HashMap::new(),
        DatabaseName: "test".to_owned(),
        TableName: "t".to_owned(),
        PredicateColumns: vec![
            predicate_column(9),
            predicate_column(-3),
            predicate_column(4),
            predicate_column(4),
        ],
        Count: 0,
        ModifyCount: 0,
        Version: 0,
        IsHistoricalStats: false,
    };

    table.Sort();

    let ids: Vec<i64> = table
        .PredicateColumns
        .iter()
        .map(|column| column.ID)
        .collect();
    assert_eq!(ids, vec![-3, 4, 4, 9]);
}

/// 无 Histogram/CMSketch/FMSketch 时 TotalMemoryUsage 为 0。
#[test]
fn total_memory_usage_is_zero_when_all_messages_are_absent() {
    assert_eq!(empty_column().TotalMemoryUsage(), 0);
}

/// 有统计 protobuf 时，内存占用等于各消息 `compute_size` 之和。
#[test]
fn total_memory_usage_sums_present_protobuf_messages() {
    let mut histogram = tipb::Histogram::new();
    histogram.set_ndv(17);
    let mut cm_sketch = tipb::CmSketch::new();
    cm_sketch.set_default_value(23);
    let mut fm_sketch = tipb::FmSketch::new();
    fm_sketch.set_mask(31);
    let expected = histogram.compute_size() as i64
        + cm_sketch.compute_size() as i64
        + fm_sketch.compute_size() as i64;

    let mut column = empty_column();
    column.Histogram = Some(Box::new(histogram));
    column.CMSketch = Some(Box::new(cm_sketch));
    column.FMSketch = Some(Box::new(fm_sketch));

    assert_eq!(column.TotalMemoryUsage(), expected);
}
