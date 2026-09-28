// Copyright 2026 AsterSQL.

use crate::*;
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn possible_properties_copy_orders_and_cache_tiflash_like_go() {
    let order_column = Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        7,
        70,
        0,
    );
    let child = PossiblePropertiesInfo {
        Orders: vec![vec![order_column.clone()]],
        HasTiFlash: true,
    };
    let mut gather = TiKVSingleGather::default();

    let properties = gather.PreparePossibleProperties(&[child]);

    assert_eq!(properties.Orders.len(), 1);
    assert_eq!(properties.Orders[0][0].UniqueID, order_column.UniqueID);
    assert!(properties.HasTiFlash);
    assert!(gather.base().PreparePossiblePropertiesValue());

    let empty = gather.PreparePossibleProperties(&[]);
    assert!(empty.Orders.is_empty());
    assert!(!empty.HasTiFlash);
    assert!(!gather.base().PreparePossiblePropertiesValue());
}

#[test]
fn derive_stats_inherits_the_scan_child_like_go() {
    let mut source = DataSource::default();
    source.TableStats.RowCount = 4.0;

    let mut child = LogicalSchemaProducer::default();
    child.SetStats(StatsInfo {
        RowCount: 10.0,
        ..StatsInfo::default()
    });

    let mut gather = TiKVSingleGather {
        Source: Some(Rc::new(RefCell::new(source))),
        ..TiKVSingleGather::default()
    };
    gather.SetChildren(vec![Box::new(child)]);

    let (stats, changed) = gather.DeriveStats(false).expect("derive gather stats");

    assert!(changed);
    assert_eq!(stats.RowCount, 10.0);
}

#[test]
fn explain_appends_the_index_name_for_index_gathers() {
    let mut index = model::IndexInfo::default();
    index.Name = parser_ast::NewCIStr("idx_customer");
    let gather = TiKVSingleGather {
        Source: Some(Rc::new(RefCell::new(DataSource::default()))),
        IsIndexGather: true,
        Index: Some(index),
        ..TiKVSingleGather::default()
    };

    assert!(gather.ExplainInfo().ends_with(", index:idx_customer"));
}

#[test]
fn build_key_info_inherits_the_child_keys_like_go() {
    let child_key = Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        1,
        101,
        0,
    );
    let mut child = LogicalSchemaProducer::default();
    child.SetSchema(expression::NewSchema(vec![child_key.clone()]));
    child.Schema_mut().PKOrUK = vec![vec![child_key]];

    let output_key = Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        2,
        101,
        0,
    );
    let mut gather = TiKVSingleGather::default();
    gather.SetSchema(expression::NewSchema(vec![output_key]));
    gather.SetChildren(vec![Box::new(child)]);

    gather.BuildKeyInfo();

    assert_eq!(gather.Schema().PKOrUK.len(), 1);
    assert_eq!(gather.Schema().PKOrUK[0][0].ID, 2);
}
