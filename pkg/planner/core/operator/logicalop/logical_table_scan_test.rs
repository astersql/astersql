// Copyright 2026 AsterSQL.

use crate::*;
use std::cell::RefCell;
use std::rc::Rc;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.ID = id;
    column.UniqueID = id;
    column
}

#[test]
fn explain_uses_source_handle_and_formats_access_conditions_like_go() {
    let mut source = DataSource::default();
    source.HandleCols = Some(planner_util::NewIntHandleCols(column(7)));
    let scan = LogicalTableScan {
        Source: Some(Rc::new(RefCell::new(source))),
        HandleCols: Some(planner_util::NewIntHandleCols(column(8))),
        AccessConds: vec![Box::new(column(9))],
        ..Default::default()
    };

    let explain = scan.ExplainInfo();

    assert!(explain.contains("pk col:Column#7"), "{explain}");
    assert!(!explain.contains("Column#8"), "{explain}");
    assert!(explain.contains("cond:Column#9"), "{explain}");
}

#[test]
fn tiflash_property_requires_an_mpp_enabled_context_like_go() {
    let mut source = DataSource::default();
    source.TableInfo.TiFlashReplica = Some(model::TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..Default::default()
    });
    let scan = LogicalTableScan {
        Source: Some(Rc::new(RefCell::new(source))),
        ..Default::default()
    };

    assert!(!scan.PreparePossibleProperties().HasTiFlash);
}

#[test]
fn derive_stats_builds_a_full_integer_range_without_handle_columns() {
    let mut primary = model::ColumnInfo::New(1, parser_ast::NewCIStr("id"));
    primary.AddFlag(mysql::r#type::PriKeyFlag | mysql::r#type::UnsignedFlag);
    let mut source = DataSource::default();
    source.TableInfo.PKIsHandle = true;
    source.TableInfo.Columns = vec![primary];
    source.TableStats.RowCount = 12.0;
    let mut scan = LogicalTableScan {
        Source: Some(Rc::new(RefCell::new(source))),
        ..Default::default()
    };

    let (stats, changed) = scan.DeriveStats(false).expect("derive table scan stats");

    assert!(changed);
    assert_eq!(stats.RowCount, 12.0);
    assert_eq!(scan.Ranges.len(), 1);
}
