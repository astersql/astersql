// Copyright 2026 AsterSQL.

// Access Object 迁移期单元测试。
//
// 覆盖 `ScanAccessObject` / `OtherAccessObject` / `DynamicPartitionAccessObject`
// 的字符串展示、归一化文本与 `SetIntoPB`（写入 tipb ExplainOperator）行为，
// 确保与 Go 侧 EXPLAIN access object 列及二进制计划语义一致。

use super::access_obj::{
    DynamicPartitionAccessObject, DynamicPartitionAccessObjects, IndexAccess, OtherAccessObject,
    ScanAccessObject,
};

/// 构造带名称、列列表与是否聚簇索引标记的 `IndexAccess`。
fn index(name: &str, cols: &[&str], clustered: bool) -> IndexAccess {
    IndexAccess {
        Name: name.to_owned(),
        Cols: cols.iter().map(|col| (*col).to_owned()).collect(),
        IsClusteredIndex: clustered,
    }
}

/// 构造动态分区访问对象；`err` 非空时表示分区裁剪失败路径。
fn dynamic(
    database: &str,
    table: &str,
    all_partitions: bool,
    partitions: &[&str],
    err: &str,
) -> DynamicPartitionAccessObject {
    DynamicPartitionAccessObject {
        Database: database.to_owned(),
        Table: table.to_owned(),
        AllPartitions: all_partitions,
        Partitions: partitions.iter().map(|part| (*part).to_owned()).collect(),
        Err: err.to_owned(),
    }
}

/// 校验普通 String 保留真实分区名，NormalizedString 将分区折叠为 `?`。
#[test]
fn scan_access_object_formats_normal_and_normalized_output() {
    let scan = ScanAccessObject {
        Database: "app".to_owned(),
        Table: "orders".to_owned(),
        Indexes: vec![
            index("idx_customer", &["customer_id", "created_at"], false),
            index("PRIMARY", &["id"], true),
        ],
        Partitions: vec!["p0".to_owned(), "p1".to_owned()],
    };

    assert_eq!(
        scan.String(),
        "table:orders, partition:p0,p1, index:idx_customer(customer_id, created_at), clustered index:PRIMARY(id)"
    );
    assert_eq!(
        scan.NormalizedString(),
        "table:orders, partition:?, index:idx_customer(customer_id, created_at), clustered index:PRIMARY(id)"
    );

    // Go 直接追加分区/索引片段；表名为空时仍保留前导分隔符。
    let without_table = ScanAccessObject {
        Database: String::new(),
        Table: String::new(),
        Indexes: vec![index("idx_only", &[], false)],
        Partitions: vec!["p0".to_owned()],
    };
    assert_eq!(without_table.String(), ", partition:p0, index:idx_only()");
    assert_eq!(
        without_table.NormalizedString(),
        ", partition:?, index:idx_only()"
    );
}

/// 校验 SetIntoPB 覆盖已有 AccessObjects，并正确填充 tipb 扫描字段。
#[test]
fn scan_and_index_fill_tipb_and_replace_existing_access_objects() {
    let scan = ScanAccessObject {
        Database: "app".to_owned(),
        Table: "orders".to_owned(),
        Indexes: vec![index("idx_customer", &["customer_id"], false)],
        Partitions: vec!["p0".to_owned()],
    };
    let mut pb = tipb::ExplainOperator::new();
    // 预置一个空对象，确认后续 SetIntoPB 会整体替换而非追加。
    pb.mut_access_objects().push(tipb::AccessObject::new());

    scan.SetIntoPB(Some(&mut pb));

    assert_eq!(pb.get_access_objects().len(), 1);
    assert!(pb.get_access_objects()[0].has_scan_object());
    let scan_pb = pb.get_access_objects()[0].get_scan_object();
    assert_eq!(scan_pb.get_database(), "app");
    assert_eq!(scan_pb.get_table(), "orders");
    assert_eq!(scan_pb.get_partitions(), &["p0".to_owned()]);
    assert_eq!(scan_pb.get_indexes().len(), 1);
    assert_eq!(scan_pb.get_indexes()[0].get_name(), "idx_customer");
    assert_eq!(
        scan_pb.get_indexes()[0].get_cols(),
        &["customer_id".to_owned()]
    );
    assert!(!scan_pb.get_indexes()[0].get_is_clustered_index());

    let clustered = index("PRIMARY", &["id"], true).ToPB();
    assert_eq!(clustered.get_name(), "PRIMARY");
    assert_eq!(clustered.get_cols(), &["id".to_owned()]);
    assert!(clustered.get_is_clustered_index());
}

/// 空 OtherAccessObject 不改写 PB；非空时覆盖为 other_object。
#[test]
fn other_access_object_preserves_pb_when_empty_and_overwrites_when_present() {
    let mut pb = tipb::ExplainOperator::new();
    pb.mut_access_objects().push(tipb::AccessObject::new());

    OtherAccessObject(String::new()).SetIntoPB(Some(&mut pb));
    assert_eq!(pb.get_access_objects().len(), 1);

    let other = OtherAccessObject("range:cached".to_owned());
    assert_eq!(other.String(), "range:cached");
    assert_eq!(other.NormalizedString(), "range:cached");
    other.SetIntoPB(Some(&mut pb));
    assert_eq!(pb.get_access_objects().len(), 1);
    assert!(pb.get_access_objects()[0].has_other_object());
    assert_eq!(
        pb.get_access_objects()[0].get_other_object(),
        "range:cached"
    );
}

/// 覆盖分区裁剪错误、全分区、dual（无命中分区）与多对象拼接展示。
#[test]
fn dynamic_partition_strings_cover_error_all_dual_and_multiple_objects() {
    assert_eq!(
        dynamic("db", "t", false, &[], "pruning failed").String(),
        "pruning failed"
    );
    assert_eq!(dynamic("db", "t", true, &[], "").String(), "partition:all");
    assert_eq!(
        dynamic("db", "t", false, &[], "").String(),
        "partition:dual"
    );
    assert_eq!(
        dynamic("db", "t", false, &["p0", "p1"], "").String(),
        "partition:p0,p1"
    );

    let singleton =
        DynamicPartitionAccessObjects(vec![Box::new(dynamic("db", "orders", false, &["p0"], ""))]);
    assert_eq!(singleton.String(), "partition:p0");
    assert_eq!(singleton.NormalizedString(), "partition:p0");

    let objects = DynamicPartitionAccessObjects(vec![
        Box::new(dynamic("db", "orders", false, &["p0"], "")),
        Box::new(dynamic("db", "archive", true, &[], "")),
    ]);
    assert_eq!(
        objects.String(),
        "partition:p0 of orders, partition:all of archive"
    );
    assert_eq!(objects.NormalizedString(), objects.String());
}

/// 对齐 Go：错误项在 PB 中仍占零值槽位，不省略长度。
#[test]
fn dynamic_partition_pb_keeps_zero_value_slot_for_errors_like_go() {
    let objects = DynamicPartitionAccessObjects(vec![
        Box::new(dynamic("app", "orders", false, &["p0"], "")),
        Box::new(dynamic("app", "broken", false, &["p1"], "pruning failed")),
    ]);
    let mut pb = tipb::ExplainOperator::new();
    pb.mut_access_objects().push(tipb::AccessObject::new());

    objects.SetIntoPB(Some(&mut pb));

    assert!(pb.get_access_objects()[0].has_dynamic_partition_objects());
    let dynamic_pb = pb.get_access_objects()[0].get_dynamic_partition_objects();
    assert_eq!(dynamic_pb.get_objects().len(), 2);
    assert_eq!(dynamic_pb.get_objects()[0].get_database(), "app");
    assert_eq!(dynamic_pb.get_objects()[0].get_table(), "orders");
    assert!(!dynamic_pb.get_objects()[0].get_all_partitions());
    assert_eq!(
        dynamic_pb.get_objects()[0].get_partitions(),
        &["p0".to_owned()]
    );
    // 错误对象对应 Go 零值：库表名为空、分区列表为空。
    assert_eq!(dynamic_pb.get_objects()[1].get_database(), "");
    assert_eq!(dynamic_pb.get_objects()[1].get_table(), "");
    assert!(dynamic_pb.get_objects()[1].get_partitions().is_empty());

    let all =
        DynamicPartitionAccessObjects(vec![Box::new(dynamic("app", "archive", true, &[], ""))]);
    all.SetIntoPB(Some(&mut pb));
    assert_eq!(pb.get_access_objects().len(), 1);
    let all_pb = pb.get_access_objects()[0]
        .get_dynamic_partition_objects()
        .get_objects();
    assert_eq!(all_pb.len(), 1);
    assert!(all_pb[0].get_all_partitions());
}

/// 空列表与 `None` 目标均应成为 no-op，不 panic、不改写既有 PB。
#[test]
fn empty_dynamic_partition_list_and_none_targets_are_no_ops() {
    let mut pb = tipb::ExplainOperator::new();
    pb.mut_access_objects().push(tipb::AccessObject::new());
    DynamicPartitionAccessObjects(Vec::new()).SetIntoPB(Some(&mut pb));
    assert_eq!(pb.get_access_objects().len(), 1);

    let scan = ScanAccessObject {
        Database: String::new(),
        Table: String::new(),
        Indexes: Vec::new(),
        Partitions: Vec::new(),
    };
    scan.SetIntoPB(None);
    OtherAccessObject("x".to_owned()).SetIntoPB(None);
    DynamicPartitionAccessObjects(vec![Box::new(dynamic("db", "t", false, &[], ""))])
        .SetIntoPB(None);
}
