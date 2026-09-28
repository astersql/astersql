// Copyright 2026 AsterSQL.

// `AssertTableEqual` 的单元测试。
//
// 用 mock 统计表验证相等路径通过，以及实时行数不一致时触发 panic。

use super::*;
use cache_testutil::NewMockStatisticsTable;

/// 克隆同一 mock 表后断言应成功。
#[test]
fn assert_table_equal_accepts_equivalent_statistics() {
    let actual = NewMockStatisticsTable(2, 1, true, true, true);
    let expected = actual.clone();
    AssertTableEqual(&actual, &expected);
}

/// 仅改 `RealtimeCount` 时应因不相等而 panic。
#[test]
#[should_panic]
fn assert_table_equal_rejects_different_table_counts() {
    let actual = NewMockStatisticsTable(0, 0, false, false, false);
    let mut expected = actual.clone();
    expected.RealtimeCount = 1;
    AssertTableEqual(&actual, &expected);
}

/// Go 的 `TopN.Equal` 将 nil 与总计数为零的 TopN 视为相等，列和索引都应保留该语义。
#[test]
fn assert_table_equal_accepts_zero_count_topn_as_empty() {
    let actual = NewMockStatisticsTable(1, 1, false, false, false);
    let mut expected = actual.clone();

    let mut zero_count_top_n = statistics::NewTopN(1);
    zero_count_top_n.AppendTopN(b"zero".to_vec(), 0);
    expected
        .HistColl
        .GetColMut(1)
        .expect("mock column must exist")
        .TopN = Some(zero_count_top_n.clone());
    expected
        .HistColl
        .GetIdxMut(1)
        .expect("mock index must exist")
        .TopN = Some(zero_count_top_n);

    AssertTableEqual(&actual, &expected);
}
