// Copyright 2026 AsterSQL.

// 物理计划简化实现的 Go 语义回归测试。
//
// 覆盖 CTE 扫描的 EXPLAIN 文本，以及更新计划中表列区间定位和
// `DEFAULT(列名)` 判定这两处容易因 Rust 端直觉实现而偏离 Go 的边界行为。

use crate::physical_common_plans::{
    PhysicalExpr, Stats, TableColumnPosition, find_table_index, is_default_expr_same_column,
};
use crate::physical_cte_table::PhysicalCteTable;

/// CTE 扫描说明必须保留 Go 端约定的存储编号格式。
#[test]
fn cte_table_explain_matches_go_format() {
    let table = PhysicalCteTable {
        id_for_storage: 42,
        seed_statistics: Stats::default(),
        schema: vec![1, 2],
    };

    assert_eq!(table.explain_info(), "Scan on CTE_42");
}

/// 表索引定位只比较各区间起点，并取最后一个不大于列序号的区间。
/// 因此即使列序号越过最后一个 `end`，也仍归到最后一张表。
#[test]
fn table_column_index_uses_last_start_like_go_search() {
    let positions = vec![
        TableColumnPosition {
            table_id: 1,
            start: 0,
            end: 3,
        },
        TableColumnPosition {
            table_id: 2,
            start: 3,
            end: 8,
        },
    ];

    assert_eq!(find_table_index(&positions, 7), Some(1));
    assert_eq!(find_table_index(&positions, 99), Some(1));
}

/// `DEFAULT(列名)` 复刻 Go 的字段查找约束：只有首个候选列匹配才算当前列。
#[test]
fn default_column_reference_must_be_the_first_name() {
    let expression = PhysicalExpr::Default {
        column_name: Some("second".to_owned()),
    };
    assert!(!is_default_expr_same_column(
        &["first".to_owned(), "second".to_owned()],
        &expression,
    ));
}
