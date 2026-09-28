// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 表属性（attributes）相关 DDL 的 SQL 层测试。
//
// 表属性是形如 `key=value` 的键值对（如 `merge_option=deny`，用于控制底层
// 存储 Region——即数据分片——是否允许合并），可以设置在整表（键为 `table`）
// 或某个分区（键为 `partition:<分区名>`）上。本文件验证在各类 DDL 操作
// （修改属性、TRUNCATE、RENAME、RECOVER/FLASHBACK、DROP、重建同名表、
// 分区调整）之后，属性能否被正确保留、清除或迁移。

use std::collections::BTreeMap;

use crate::table::{
    GcController, TableCatalog, TableError, TableInfo, TableState, alter_attributes,
    table_physical_ids,
};

/// 测试辅助函数：把 `(键, 值)` 切片转换为属性映射（BTreeMap 保证键有序，便于断言比较）。
fn attributes(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// 测试辅助函数：构造一个处于 Public（对外可见）状态的表元信息。
///
/// `partition_ids` 为分区表的各物理分区 ID；传空 Vec 表示普通（非分区）表。
/// 其余字段（自增 ID、字符集、外键等）均取默认值，与本组测试无关。
fn table(id: i64, schema_id: i64, name: &str, partition_ids: Vec<i64>) -> TableInfo {
    TableInfo {
        id,
        schema_id,
        name: name.to_owned(),
        state: TableState::Public,
        partition_ids,
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 0,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// 测试辅助函数：按 DDL 状态机逐步删除表，并断言每一步的状态迁移。
///
/// 删除表遵循多阶段在线 schema 变更（参考 F1/TiDB 的 online DDL）：
/// Public -> WriteOnly（仅可写）-> DeleteOnly（仅可删）-> None（彻底移除），
/// `ts` 为删除发生时的时间戳，用于后续 GC/恢复判断。
fn drop_table(catalog: &mut TableCatalog, schema_id: i64, name: &str, ts: u64) {
    assert_eq!(
        TableState::WriteOnly,
        catalog.drop_table_step(schema_id, name, ts).unwrap()
    );
    assert_eq!(
        TableState::DeleteOnly,
        catalog.drop_table_step(schema_id, name, ts).unwrap()
    );
    assert_eq!(
        TableState::None,
        catalog.drop_table_step(schema_id, name, ts).unwrap()
    );
}

/// 测试 ALTER TABLE ... ATTRIBUTES：表级与分区级属性的设置、去重与重置。
#[test]
fn test_alter_table_partition_attributes() {
    let mut table = table(1, 1, "alter_p", vec![10, 20, 30, 40]);
    // 首次设置属性返回 true（发生变化）；重复设置相同属性返回 false（无变化）。
    assert!(alter_attributes(
        &mut table,
        attributes(&[("table", "merge_option=deny")])
    ));
    assert!(!alter_attributes(
        &mut table,
        attributes(&[("table", "merge_option=deny")])
    ));

    // 同时设置表级属性与 p0、p1 两个分区级属性。
    let with_partitions = attributes(&[
        ("table", "merge_option=deny"),
        ("partition:p0", "merge_option=allow"),
        ("partition:p1", "merge_option=allow,key=value"),
    ]);
    assert!(alter_attributes(&mut table, with_partitions.clone()));
    assert_eq!(
        Some(&"merge_option=allow".to_owned()),
        table.attributes.get("partition:p0")
    );

    // 只保留表级属性等价于把分区属性重置为 DEFAULT：分区级条目应被清除。
    let table_only = attributes(&[("table", "merge_option=deny")]);
    assert!(alter_attributes(&mut table, table_only.clone()));
    assert_eq!(
        table_only, table.attributes,
        "DEFAULT removes partition attributes"
    );
    assert_eq!(vec![10, 20, 30, 40], table_physical_ids(&table));

    // Go checks that table-level attribute ranges are recomputed when partitions
    // are added, dropped, and truncated, while the attribute value itself stays.
    let original_ids = table_physical_ids(&table);
    table.partition_ids.push(50);
    let added_ids = table_physical_ids(&table);
    assert_ne!(original_ids, added_ids);
    assert_eq!(
        Some(&"merge_option=deny".to_owned()),
        table.attributes.get("table")
    );

    table.partition_ids.pop();
    assert_eq!(original_ids, table_physical_ids(&table));

    table.partition_ids.push(60);
    let before_truncate = table_physical_ids(&table);
    *table.partition_ids.last_mut().unwrap() = 61;
    assert_ne!(before_truncate, table_physical_ids(&table));
    assert_ne!(original_ids, table_physical_ids(&table));
    assert_eq!(
        Some(&"merge_option=deny".to_owned()),
        table.attributes.get("table")
    );
}

/// 测试 TRUNCATE TABLE：清空表会分配新的表/分区物理 ID，但属性应原样保留。
#[test]
fn test_truncate_table() {
    let mut catalog = TableCatalog::default();
    // 分区表场景：TRUNCATE 后旧物理 ID（表 1 + 分区 11、12）被替换为新 ID。
    let mut partitioned = table(1, 1, "truncate_t", vec![11, 12]);
    alter_attributes(
        &mut partitioned,
        attributes(&[("table", "key=value"), ("partition:p0", "key1=value1")]),
    );
    catalog.insert(partitioned).unwrap();
    let old_ids = catalog
        .truncate_table(1, "truncate_t", 2, vec![21, 22])
        .unwrap();
    assert_eq!(vec![1, 11, 12], old_ids);
    let current = catalog.get(1, "truncate_t").unwrap();
    assert_eq!(vec![21, 22], table_physical_ids(current));
    assert_eq!("key=value", current.attributes["table"]);
    assert_eq!("key1=value1", current.attributes["partition:p0"]);

    // 普通（非分区）表场景：TRUNCATE 同样保留表级属性。
    let mut plain = table(3, 1, "truncate_ot", Vec::new());
    alter_attributes(&mut plain, attributes(&[("table", "key=value")]));
    catalog.insert(plain).unwrap();
    assert_eq!(
        vec![3],
        catalog
            .truncate_table(1, "truncate_ot", 4, Vec::new())
            .unwrap()
    );
    assert_eq!(
        "key=value",
        catalog.get(1, "truncate_ot").unwrap().attributes["table"]
    );
}

/// 测试 RENAME TABLE：重命名（含批量重命名）不应影响属性与物理 ID。
#[test]
fn test_rename_table() {
    let mut catalog = TableCatalog::default();
    // 单表重命名：属性与分区物理 ID 均随新名字保留。
    let mut original = table(1, 1, "rename_t", vec![11]);
    alter_attributes(
        &mut original,
        attributes(&[("table", "key=value"), ("partition:p0", "key1=value1")]),
    );
    catalog.insert(original).unwrap();
    catalog.rename_table(1, "rename_t", 1, "rename_t1").unwrap();
    let renamed = catalog.get(1, "rename_t1").unwrap();
    assert_eq!("key=value", renamed.attributes["table"]);
    assert_eq!("key1=value1", renamed.attributes["partition:p0"]);
    assert_eq!(vec![11], table_physical_ids(renamed));

    // The Go test separately covers a non-partitioned table rename.
    let mut plain = table(4, 1, "rename_ot", Vec::new());
    alter_attributes(&mut plain, attributes(&[("table", "key=value")]));
    catalog.insert(plain).unwrap();
    catalog
        .rename_table(1, "rename_ot", 1, "rename_ot1")
        .unwrap();
    let renamed_plain = catalog.get(1, "rename_ot1").unwrap();
    assert_eq!("key=value", renamed_plain.attributes["table"]);
    assert_eq!(vec![4], table_physical_ids(renamed_plain));

    // 批量重命名：两张表同时改到不同的 schema，各自的属性互不串扰。
    let mut first = table(2, 1, "multi1", Vec::new());
    let mut second = table(3, 1, "multi2", Vec::new());
    alter_attributes(&mut first, attributes(&[("table", "key=multi1")]));
    alter_attributes(&mut second, attributes(&[("table", "key=multi2")]));
    catalog.insert(first).unwrap();
    catalog.insert(second).unwrap();
    catalog
        .rename_tables(&[
            (1, "multi1".into(), 2, "multi1".into()),
            (1, "multi2".into(), 3, "multi2".into()),
        ])
        .unwrap();
    assert_eq!(
        "key=multi1",
        catalog.get(2, "multi1").unwrap().attributes["table"]
    );
    assert_eq!(
        "key=multi2",
        catalog.get(3, "multi2").unwrap().attributes["table"]
    );
}

/// 测试 RECOVER TABLE：在 GC（垃圾回收，按安全点时间戳清理旧数据）尚未清除
/// 数据的前提下恢复已删除的表，属性应完整还原。
#[test]
fn test_recover_table() {
    let mut catalog = TableCatalog::default();
    let mut original = table(10, 1, "recover_t", vec![11]);
    alter_attributes(
        &mut original,
        attributes(&[("table", "key=value"), ("partition:p0", "key1=value1")]),
    );
    catalog.insert(original).unwrap();
    drop_table(&mut catalog, 1, "recover_t", 100);
    // GC 安全点 90 < 删除时间戳 100，数据尚未被回收，可以恢复。
    let mut gc = GcController {
        enabled: true,
        safe_point: 90,
    };
    assert!(catalog.recover_table(10, &mut gc).unwrap());
    assert!(gc.enabled);
    let recovered = catalog.get(1, "recover_t").unwrap();
    assert_eq!("key=value", recovered.attributes["table"]);
    assert_eq!("key1=value1", recovered.attributes["partition:p0"]);
}

/// 测试 FLASHBACK TABLE：恢复（recover）后再重命名的组合流程，
/// 并验证 TRUNCATE 后的新表 ID 也能被闪回，属性始终保留。
#[test]
fn test_flashback_table() {
    let mut catalog = TableCatalog::default();
    let mut original = table(20, 1, "flash_t", vec![21]);
    alter_attributes(
        &mut original,
        attributes(&[("table", "key=value"), ("partition:p0", "key1=value1")]),
    );
    catalog.insert(original).unwrap();
    drop_table(&mut catalog, 1, "flash_t", 100);
    let mut gc = GcController {
        enabled: false,
        safe_point: 50,
    };
    // 闪回 = 恢复旧表 + 重命名为新表名。
    catalog.recover_table(20, &mut gc).unwrap();
    catalog.rename_table(1, "flash_t", 1, "flash_t1").unwrap();
    assert_eq!(
        "key=value",
        catalog.get(1, "flash_t1").unwrap().attributes["table"]
    );
    assert_eq!(
        "key1=value1",
        catalog.get(1, "flash_t1").unwrap().attributes["partition:p0"]
    );
    assert_eq!(
        vec![21],
        table_physical_ids(catalog.get(1, "flash_t1").unwrap())
    );

    // TRUNCATE 产生新表 ID 30，删除后按新 ID 闪回同样有效。
    catalog.truncate_table(1, "flash_t1", 30, vec![31]).unwrap();
    drop_table(&mut catalog, 1, "flash_t1", 150);
    catalog.recover_table(30, &mut gc).unwrap();
    catalog.rename_table(1, "flash_t1", 1, "flash_t2").unwrap();
    assert_eq!(
        "key=value",
        catalog.get(1, "flash_t2").unwrap().attributes["table"]
    );
    assert_eq!(
        "key1=value1",
        catalog.get(1, "flash_t2").unwrap().attributes["partition:p0"]
    );
    assert_eq!(
        vec![31],
        table_physical_ids(catalog.get(1, "flash_t2").unwrap())
    );
}

/// 测试 DROP TABLE：删除后表不可见；新建同名表不应继承旧表的属性。
#[test]
fn test_drop_table() {
    let mut catalog = TableCatalog::default();
    let mut original = table(40, 1, "drop_t", vec![41, 42]);
    alter_attributes(&mut original, attributes(&[("table", "key=value")]));
    catalog.insert(original).unwrap();
    drop_table(&mut catalog, 1, "drop_t", 100);
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "drop_t"));

    // 用相同表名重新建表：属性应为空，不受已删除旧表影响。
    let replacement = table(50, 1, "drop_t", vec![51, 52]);
    catalog.insert(replacement).unwrap();
    assert!(catalog.get(1, "drop_t").unwrap().attributes.is_empty());
}

/// 测试删表后重建同名表：此时恢复旧表应因名字冲突失败，且新表属性不受影响。
#[test]
fn test_create_with_same_name() {
    let mut catalog = TableCatalog::default();
    let mut old = table(60, 1, "recreate_t", vec![61]);
    alter_attributes(
        &mut old,
        attributes(&[("table", "key=value"), ("partition:p0", "key1=value1")]),
    );
    catalog.insert(old).unwrap();
    drop_table(&mut catalog, 1, "recreate_t", 100);
    assert_eq!(Err(TableError::NotFound), catalog.get(1, "recreate_t"));

    let mut new = table(70, 1, "recreate_t", vec![71]);
    alter_attributes(
        &mut new,
        attributes(&[("table", "key=value"), ("partition:p1", "key1=value1")]),
    );
    catalog.insert(new).unwrap();
    let mut gc = GcController {
        enabled: true,
        safe_point: 0,
    };
    // 同名新表已存在，恢复旧表（ID 60）必须返回冲突错误。
    assert_eq!(
        Err(TableError::RecoveryConflict),
        catalog.recover_table(60, &mut gc)
    );
    assert_eq!(
        "key=value",
        catalog.get(1, "recreate_t").unwrap().attributes["table"]
    );
    assert_eq!(
        "key1=value1",
        catalog.get(1, "recreate_t").unwrap().attributes["partition:p1"]
    );
    assert!(
        !catalog
            .get(1, "recreate_t")
            .unwrap()
            .attributes
            .contains_key("partition:p0")
    );
}

/// 测试分区级操作：删除分区、TRUNCATE 分区（换新分区 ID）以及
/// EXCHANGE PARTITION（分区与普通表互换数据）对属性的影响。
#[test]
fn test_partition() {
    let mut catalog = TableCatalog::default();
    let mut partitioned = table(80, 1, "part", vec![81, 82, 83]);
    alter_attributes(
        &mut partitioned,
        attributes(&[
            ("table", "key=value"),
            ("partition:p0", "key1=value1"),
            ("partition:p1", "key2=value2"),
        ]),
    );
    catalog.insert(partitioned).unwrap();

    // 模拟 DROP PARTITION p0：移除其物理 ID 与对应属性，p1 的属性不受影响。
    let part_table = catalog.get_mut(1, "part").unwrap();
    part_table.partition_ids.remove(0);
    part_table.attributes.remove("partition:p0");
    assert_eq!(vec![82, 83], table_physical_ids(part_table));
    assert_eq!("key2=value2", part_table.attributes["partition:p1"]);

    // 模拟 TRUNCATE PARTITION p1：物理 ID 更换为新值，但分区属性保留。
    let old_p1 = part_table.partition_ids[0];
    part_table.partition_ids[0] = 92;
    assert_ne!(old_p1, part_table.partition_ids[0]);
    assert_eq!("key2=value2", part_table.attributes["partition:p1"]);

    // 模拟 EXCHANGE PARTITION：分区 p1 与普通表 part1 互换，属性也随之对调。
    let mut standalone = table(90, 1, "part1", Vec::new());
    alter_attributes(&mut standalone, attributes(&[("table", "role=table")]));
    let partition_attributes = part_table.attributes.remove("partition:p1").unwrap();
    let standalone_attributes = standalone.attributes.remove("table").unwrap();
    part_table
        .attributes
        .insert("partition:p1".into(), standalone_attributes);
    standalone
        .attributes
        .insert("table".into(), partition_attributes);
    assert_eq!("key2=value2", standalone.attributes["table"]);
    assert_eq!("role=table", part_table.attributes["partition:p1"]);
}
