// Copyright 2026 AsterSQL.

use crate::analyze_col::{columnInfo, tableInfo};

fn column(name: &str, changing: bool, removing: bool) -> columnInfo {
    columnInfo {
        name: name.to_owned(),
        changing,
        removing,
        ..columnInfo::default()
    }
}

#[test]
fn non_temporary_column_count_matches_go_modify_column_semantics() {
    let table = tableInfo {
        columns: vec![
            column("a", false, false),
            column("_Col$_a_0", true, false),
            column("_Del$_b", false, true),
            column("c", false, false),
        ],
        ..tableInfo::default()
    };

    assert_eq!(table.nonTemporaryColumnCount(), 2);
}
