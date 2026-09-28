// Copyright 2026 AsterSQL.

// `testutil` mock 表构造辅助函数的单元测试。

use super::*;

/// 验证 NewMockStatisticsTable 能生成指定列/索引数，且追加列/索引后计数与内存占用正确。
#[test]
fn mock_table_builds_and_appends_columns_and_indices() {
    let mut table = NewMockStatisticsTable(2, 3, true, true, true);
    assert_eq!(table.ColNum(), 2);
    assert_eq!(table.IdxNum(), 3);
    assert!(table.MemoryUsage().TotalMemUsage > 0);
    MockTableAppendColumn(&mut table);
    MockTableAppendIndex(&mut table);
    assert_eq!(table.ColNum(), 3);
    assert_eq!(table.IdxNum(), 4);

    // Go helper 给 Histogram 固定传 ID=0；ColumnInfo 只填写 ID，
    // 所以其 FieldType 仍是零值。
    let column = table.HistColl.GetCol(1).unwrap();
    let index = table.HistColl.GetIdx(1).unwrap();
    assert_eq!(column.Histogram.ID, 0);
    assert_eq!(index.Histogram.ID, 0);
    assert_eq!(column.Histogram.Tp.GetType(), mysql::r#type::TypeBlob);
    assert_eq!(column.Info.as_ref().unwrap().FieldType.GetType(), 0);
}

/// 关闭可选载荷时，Histogram 与 ColumnInfo 应保持 Go 零值语义。
#[test]
fn mock_table_without_payloads_uses_zero_value_histograms() {
    let table = NewMockStatisticsTable(1, 1, false, false, false);
    let column = table.HistColl.GetCol(1).unwrap();
    let index = table.HistColl.GetIdx(1).unwrap();

    assert!(column.CMSketch.is_none());
    assert!(column.TopN.is_none());
    assert!(index.CMSketch.is_none());
    assert!(index.TopN.is_none());
    assert_eq!(column.Histogram.ID, 0);
    assert_eq!(index.Histogram.ID, 0);
    assert_eq!(column.Histogram.Tp.GetType(), 0);
    assert_eq!(index.Histogram.Tp.GetType(), 0);
}
