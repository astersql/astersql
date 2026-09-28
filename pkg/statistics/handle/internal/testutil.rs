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

// 统计句柄内部测试工具。
//
// 提供与 Go 测试对齐的表级统计深度相等断言，覆盖行数、直方图、
// CMSketch（Count-Min Sketch，近似频次结构）、TopN 与列/索引存在性映射。

use statistics::Table;

/// Mirrors Go's nil-safe `(*TopN).Equal`: every zero-total TopN is equivalent.
fn top_n_equal(left: Option<&statistics::TopN>, right: Option<&statistics::TopN>) -> bool {
    let left_total = left.map_or(0, statistics::TopN::TotalCount);
    let right_total = right.map_or(0, statistics::TopN::TotalCount);
    if left_total == 0 && right_total == 0 {
        return true;
    }
    matches!((left, right), (Some(left), Some(right)) if left.Equal(right))
}

/// Asserts all table, column, index, and existence-map statistics used by Go tests.
/// 断言表级统计与期望完全一致：实时行数、修改计数、各列/索引直方图与草图，以及存在性映射。
pub fn AssertTableEqual(actual: &Table, expected: &Table) {
    assert_eq!(actual.RealtimeCount, expected.RealtimeCount);
    assert_eq!(actual.ModifyCount, expected.ModifyCount);
    assert_eq!(actual.ColNum(), expected.ColNum());
    // 逐列比对直方图、CMSketch 与 TopN（高频值列表）。
    for (id, column) in &actual.HistColl.Columns {
        let expected_column = expected.GetCol(*id).expect("expected column must exist");
        assert!(statistics::HistogramEqual(
            &column.Histogram,
            &expected_column.Histogram,
            false
        ));
        assert_eq!(column.CMSketch, expected_column.CMSketch);
        assert!(top_n_equal(
            column.TopN.as_ref(),
            expected_column.TopN.as_ref()
        ));
    }

    assert_eq!(actual.IdxNum(), expected.IdxNum());
    // 逐索引比对同样的三类统计结构。
    for (id, index) in &actual.HistColl.Indices {
        let expected_index = expected.GetIdx(*id).expect("expected index must exist");
        assert!(statistics::HistogramEqual(
            &index.Histogram,
            &expected_index.Histogram,
            false
        ));
        assert_eq!(index.CMSketch, expected_index.CMSketch);
        assert!(top_n_equal(
            index.TopN.as_ref(),
            expected_index.TopN.as_ref()
        ));
    }
    assert!(statistics::ColAndIdxExistenceMapIsEqual(
        &actual.ColAndIdxExistenceMap,
        &expected.ColAndIdxExistenceMap,
    ));
}
