// Copyright 2026 AsterSQL.

// 分区 benchmark 辅助逻辑的轻量单元测试。
//
// 这里不执行完整 benchmark，而是固定其关键迁移契约：动态分区定义保持 Go 版本的
// 字符串形状，预处理参数保留标量与批量边界，兼容结果集能够取尽全部行。

use super::bench_test::{
    PreparedQueryArgs, RecordSetDraft, RowDraft, drainRecordSet, getListPartitionDef,
    partitionByRange, partitionByRangeColumnsPrep,
};

// 同时覆盖表达式 LIST 与 LIST COLUMNS，防止首分区补入 0、分区数量等规则漂移。
#[test]
fn list_partition_definition_matches_go_shape() {
    let expression = getListPartitionDef("floor(id*0.5)*2", false);
    assert!(expression.contains(",255,256,0),partition p1"));
    let columns = getListPartitionDef("id", true);
    assert!(columns.starts_with("partition by list columns(id) (partition p0 values in (1,2,3"));
    assert!(columns.contains("partition p3 values in (99900,99901"));
    assert_eq!(columns.matches("partition p").count(), 4);
}

// 范围分区常量和参数转换共同约束 prepared benchmark 的边界输入形状。
#[test]
fn partition_constants_preserve_prepared_range_boundaries() {
    assert_eq!(
        partitionByRange,
        "partition by range(id) (partition p0 values less than (10), partition p1 values less than (1000), partition p3 values less than (100000), partition pMax values less than (maxvalue))"
    );
    assert_eq!(
        partitionByRangeColumnsPrep,
        "partition by range columns (id) (partition p0 values less than (10), partition p1 values less than (63), partition p3 values less than (100), partition pMax values less than (maxvalue))"
    );
    assert_eq!(1_i32.to_query_args(), vec![1]);
    assert_eq!((&[2, 10000, 1][..]).to_query_args(), vec![2, 10000, 1]);
}

// 使用兼容草稿类型验证排空路径会按原顺序返回全部剩余行。
#[test]
fn drain_record_set_reads_all_rows_in_order() {
    let rows = vec![
        RowDraft {
            values: vec!["1".to_owned()],
        },
        RowDraft {
            values: vec!["2".to_owned()],
        },
    ];
    let drained = drainRecordSet(
        super::bench_test::ContextDraft,
        RecordSetDraft::new(rows),
        super::bench_test::ChunkAllocatorDraft,
    )
    .unwrap();
    assert_eq!(
        drained,
        vec![
            RowDraft {
                values: vec!["1".to_owned()],
            },
            RowDraft {
                values: vec!["2".to_owned()],
            },
        ]
    );
}
