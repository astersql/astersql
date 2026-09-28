// CoreTestSDK 的 Rust 对齐回归测试。
//
// 重点核对模拟表元数据、分区结构、字段提取边界和规划器测试套件的初始化状态，
// 防止 Rust 测试夹具与 Go 版本约定发生偏移。

use super::mock::{
    FieldType, PartitionDefinition, PartitionType, SchemaState, mock_context,
    mock_global_index_hash_partition_table, mock_hash_partition_table, mock_list_partition_table,
    mock_no_pk_table, mock_partition_info_schema, mock_range_partition_table, mock_signed_table,
    mock_state_none_column_table, mock_unsigned_table, mock_view,
};
use super::testkit::{create_planner_suite, create_planner_suite_elements, get_field_value};

// 校验基础有符号表的列属性、索引状态及前缀索引长度。
#[test]
fn mock_signed_table_matches_go_metadata() {
    let table = mock_signed_table();
    assert_eq!(table.id, 1);
    assert_eq!(table.name, "t");
    assert_eq!(table.columns.len(), 12);
    assert_eq!(table.columns[0].field_type, FieldType::Long);
    assert!(table.columns[0].primary_key && table.columns[0].not_null);
    for index in [1, 2, 3, 8, 9] {
        assert!(table.columns[index].not_null, "column index {index}");
    }
    assert!(table.columns[10].no_default);
    assert_eq!(table.indexes.len(), 7);
    assert_eq!(table.indexes[1].name, "x");
    assert_eq!(table.indexes[1].state, SchemaState::WriteOnly);
    assert!(table.indexes[1].unique);
    assert!(table.indexes[4].unique);
    assert_eq!(table.indexes[0].column_offsets, [2, 3, 4]);
    assert_eq!(
        table.indexes[6].columns,
        [
            ("e_str".to_owned(), None),
            ("d_str".to_owned(), None),
            ("c_str".to_owned(), Some(10)),
        ]
    );
}

// 校验各类派生模拟表保留 Go 夹具中的主键标记和分区形状。
#[test]
fn mock_table_variants_preserve_go_flags_and_partition_shapes() {
    let unsigned = mock_unsigned_table();
    assert!(unsigned.columns[0].primary_key);
    assert!(unsigned.columns[0].unsigned && unsigned.columns[0].not_null);
    assert!(unsigned.columns[1].not_null);
    assert!(unsigned.columns[2].unsigned);
    assert!(unsigned.indexes.iter().all(|index| index.id == 0));

    let no_pk = mock_no_pk_table();
    assert_eq!(
        no_pk
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(no_pk.columns[0].id, 2);
    assert_eq!(no_pk.columns[1].id, 3);
    assert!(no_pk.primary_key_is_handle);
    assert!(no_pk.columns[1].unsigned);
    assert!(no_pk.indexes.is_empty());

    let view = mock_view();
    assert_eq!(
        view.view.as_ref().unwrap().select_statement,
        "select b,c,d from t"
    );
    assert_eq!(
        view.view.as_ref().unwrap().security,
        super::mock::ViewSecurity::Definer
    );
    assert_eq!(view.view.as_ref().unwrap().definer, "root@");

    let range = mock_range_partition_table();
    assert_eq!(
        range.partition.as_ref().unwrap().partition_type,
        PartitionType::Range
    );
    assert_eq!(
        range.partition.as_ref().unwrap().definitions[0].less_than,
        ["16"]
    );
    assert_eq!(range.columns.last().unwrap().name, "ptn");
    assert!(range.partition.as_ref().unwrap().enabled);
    assert_eq!(range.partition.as_ref().unwrap().num, 0);

    let hash = mock_hash_partition_table();
    assert_eq!(
        hash.partition.as_ref().unwrap().partition_type,
        PartitionType::Hash
    );
    assert!(
        hash.partition
            .as_ref()
            .unwrap()
            .definitions
            .iter()
            .all(|definition| definition.less_than.is_empty())
    );
    assert_eq!(hash.partition.as_ref().unwrap().num, 2);

    let list = mock_list_partition_table();
    assert_eq!(
        list.partition.as_ref().unwrap().partition_type,
        PartitionType::List
    );
    assert_eq!(
        list.partition.as_ref().unwrap().definitions[1].in_values,
        [["2".to_owned()]]
    );

    let global = mock_global_index_hash_partition_table();
    assert_eq!(global.id, 1);
    assert_eq!(global.name, "pt2_global_index");
    assert_eq!(
        global.indexes.iter().filter(|index| index.global).count(),
        2
    );
    assert!(
        global
            .indexes
            .iter()
            .any(|index| index.name == "b_global" && index.unique)
    );
}

// SchemaState::None 的列仍保留在夹具中，以覆盖不可见列的元数据语义。
#[test]
fn state_none_table_matches_go_visibility_fixture() {
    let table = mock_state_none_column_table();
    assert_eq!(table.columns.len(), 3);
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.indexes[0].name, "b");
    assert!(table.columns[0].primary_key);
    assert!(table.columns[0].unsigned && table.columns[0].not_null);
    assert!(table.columns[1].not_null);
    assert!(table.columns[2].unsigned);
    assert_eq!(table.columns[2].state, SchemaState::None);
}

// 字段值只在前缀前有内容、值后存在空格分隔符时有效。
#[test]
fn get_field_value_matches_go_space_and_prefix_boundaries() {
    assert_eq!(
        get_field_value("partition:", "  partition:p0, table:t"),
        "p0"
    );
    assert_eq!(
        get_field_value("partition:", "x partition:p0, table:t"),
        "p0"
    );
    assert_eq!(get_field_value("partition:", "partition:p0, table:t"), "");
    assert_eq!(get_field_value("partition:", "x partition: table:t"), "");
    assert_eq!(get_field_value("partition:", "x partition:p0,"), "");
    assert_eq!(get_field_value("missing:", "x partition:p0, table:t"), "");
}

// 默认套件需按表及分区顺序分配全局 ID，并正确维护解析器与关闭状态。
#[test]
fn planner_suite_initializes_tables_ids_parser_and_close_state() {
    let mut suite = create_planner_suite_elements();
    assert_eq!(suite.info_schema().len(), 9);
    assert_eq!(suite.info_schema()[0].id, 1);
    assert_eq!(suite.info_schema()[4].id, 5);
    assert_eq!(
        suite.info_schema()[4]
            .partition
            .as_ref()
            .unwrap()
            .definitions[0]
            .id,
        6
    );
    assert_eq!(suite.info_schema()[8].id, 15);
    assert_eq!(
        suite.info_schema()[8]
            .partition
            .as_ref()
            .unwrap()
            .definitions[1]
            .id,
        17
    );
    assert!(suite.parser().config.window_functions);
    assert!(suite.parser().config.strict_double_type_check);
    assert_eq!(suite.session_context().current_database, "test");
    assert!(!suite.is_closed());
    suite.close();
    assert!(suite.is_closed());
    assert!(!suite.session_context().stats_handle_created);
}

// 自定义上下文和分区定义必须原样传入套件，不得隐式启用默认解析器选项。
#[test]
fn mock_context_and_partition_info_schema_keep_go_setup_state() {
    let context = mock_context();
    assert_eq!(context.current_database, "test");
    assert_eq!(context.division_precision_increment, 4);
    assert!(context.store_initialized);
    assert!(context.domain_bound);
    assert!(context.stats_handle_created);
    assert!(!context.window_functions_enabled);

    let schema = mock_partition_info_schema(vec![PartitionDefinition {
        id: 41,
        name: "p1".to_owned(),
        less_than: vec!["16".to_owned()],
        in_values: Vec::new(),
    }]);
    assert_eq!(schema.tables.len(), 1);
    let table = &schema.tables[0];
    assert_eq!(table.columns.last().unwrap().name, "ptn");
    assert_eq!(table.partition.as_ref().unwrap().expression, "ptn");
    assert_eq!(table.partition.as_ref().unwrap().definitions[0].id, 41);

    let suite = create_planner_suite(context, schema.clone());
    assert_eq!(suite.get_is(), &schema);
    assert!(!suite.get_parser().config.window_functions);
    assert!(!suite.get_parser().config.strict_double_type_check);
    assert_eq!(suite.get_sctx().current_database, "test");
    assert_eq!(suite.get_ctx().current_database, "test");
}
