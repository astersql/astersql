// Copyright 2022 PingCAP, Inc.
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

// Copyright 2026 AsterSQL.
// Repair Table（修复表）相关单元测试。
//
// Repair Mode（修复模式）用于在元信息损坏或与期望结构不一致时，
// 通过 `ADMIN REPAIR TABLE` 用新的 CREATE TABLE 定义覆盖表元数据，
// 同时必须保留原有物理 ID（表/列/索引/分区 ID），以避免 KV 层数据失联。

use crate::executor::{
    ColumnInfo, ColumnKind, ExecutorError, Ident, IndexInfo, PartitionDefinition,
    RepairTableRegistry, TableInfo, repair_table_definition,
};

/// 构造一张“损坏待修”的样例表：含主键、多列与 RANGE 分区定义及固定物理 ID。
fn damaged_table() -> TableInfo {
    let mut a = ColumnInfo::integer("a");
    a.id = 11;
    a.nullable = false;
    let mut b = ColumnInfo::integer("b");
    b.id = 12;
    b.kind = ColumnKind::String;
    let mut c = ColumnInfo::integer("c");
    c.id = 13;

    let mut primary = IndexInfo::new("PRIMARY", vec!["a".into()]);
    primary.id = 21;
    primary.primary = true;
    primary.unique = true;

    let mut table = TableInfo::new("origin", vec![a, b, c]);
    table.id = 101;
    table.schema_id = 9;
    table.auto_increment = 43;
    table.indexes.push(primary);
    // 五个 RANGE 分区边界 10/30/50/70/90，对应物理分区 ID 301..305。
    table.partitions = ["10", "30", "50", "70", "90"]
        .into_iter()
        .enumerate()
        .map(|(index, bound)| {
            let mut partition = PartitionDefinition::new(format!("p{bound}"), vec![bound.into()]);
            partition.id = 301 + index as i64;
            partition
        })
        .collect();
    table
}

/// 验证 repair 会继承旧表的物理 ID / schema_id / auto_inc，并允许删分区与改名。
#[test]
fn repair_preserves_physical_ids_and_allows_metadata_changes() {
    let old = damaged_table();
    let mut replacement = old.clone();
    // 故意清零新定义中的物理 ID，确认 repair 会从旧表回填。
    replacement.id = 0;
    replacement.name = "origin_rename".into();
    replacement.auto_increment = 0;
    replacement.columns[0].id = 0;
    replacement.columns[1].id = 0;
    replacement.columns[2].id = 0;
    replacement.indexes[0].id = 0;
    // 移除 p70（原 index 3），剩余分区仍按名称匹配继承旧 ID。
    replacement.partitions.remove(3);
    for partition in &mut replacement.partitions {
        partition.id = 0;
    }

    let repaired = repair_table_definition(&old, replacement, false).unwrap();
    assert_eq!(101, repaired.id);
    assert_eq!(9, repaired.schema_id);
    assert_eq!(43, repaired.auto_increment);
    assert_eq!(
        vec![11, 12, 13],
        repaired.columns.iter().map(|c| c.id).collect::<Vec<_>>()
    );
    assert_eq!(21, repaired.indexes[0].id);
    assert_eq!(
        vec![301, 302, 303, 305],
        repaired.partitions.iter().map(|p| p.id).collect::<Vec<_>>()
    );
}

/// 验证列丢失/类型变更、索引类型变更、分区丢失、Hash 分区数不一致时均被拒绝。
#[test]
fn repair_rejects_lost_or_incompatible_objects() {
    let old = damaged_table();

    let mut unknown_column = old.clone();
    unknown_column.columns[1].name = "lost".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_column, false),
        Err(ExecutorError::InvalidTableDefinition(message)) if message == "Column lost has lost"
    ));

    let mut changed_type = old.clone();
    changed_type.columns[0].kind = ColumnKind::String;
    assert!(matches!(
        repair_table_definition(&old, changed_type, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Column a type should be the same"
    ));

    let mut changed_index = old.clone();
    changed_index.indexes[0].unique = false;
    assert!(matches!(
        repair_table_definition(&old, changed_index, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Index PRIMARY type should be the same"
    ));

    let mut unknown_index = old.clone();
    unknown_index.indexes[0].name = "lost".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_index, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Index lost has lost"
    ));

    let mut unknown_partition = old.clone();
    unknown_partition.partitions[2].name = "pnew".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_partition, false),
        Err(ExecutorError::InvalidPartition(message)) if message == "Partition pnew has lost"
    ));

    let mut changed_partition_bound = old.clone();
    changed_partition_bound.partitions[1].less_than = vec!["25".into()];
    assert!(matches!(
        repair_table_definition(&old, changed_partition_bound, false),
        Err(ExecutorError::InvalidPartition(message)) if message == "Partition p30 has lost"
    ));

    // Hash 分区语义依赖分区数量固定；减少分区数必须失败。
    let mut fewer_hash_partitions = old.clone();
    fewer_hash_partitions.partitions.pop();
    assert!(matches!(
        repair_table_definition(&old, fewer_hash_partitions, true),
        Err(ExecutorError::InvalidPartition(message))
            if message == "Hash partition num should be the same"
    ));
}

/// 验证修复注册表：需开启 Repair Mode、列表非空，修复成功后表才对外可见。
#[test]
fn repair_registry_enforces_mode_fetch_visibility_and_successful_removal() {
    let ident = Ident::new("test", "origin");
    let old = damaged_table();
    let mut registry = RepairTableRegistry::default();

    // 未开启 Repair Mode 时拒绝 repair。
    assert!(matches!(
        registry.repair(&ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message)) if message == "TiDB is not in REPAIR MODE"
    ));
    registry.set_mode(true);
    assert!(matches!(
        registry.repair(&ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message)) if message == "repair list is empty"
    ));

    // fetch 后表对 infoschema 不可见，直到 repair 成功移除。
    registry.fetch_tables([(ident.clone(), old.clone())]);
    assert!(!registry.is_visible(&ident));
    let repaired = registry.repair(&ident, old, false).unwrap();
    assert_eq!(101, repaired.id);
    assert!(registry.is_visible(&ident));
}

/// 对应 Go 中系统库拒绝、标识符大小写不敏感，以及失败 repair 不移除待修表。
#[test]
fn repair_registry_matches_go_error_and_retry_contracts() {
    let old = damaged_table();
    let mut registry = RepairTableRegistry::default();
    registry.set_mode(true);
    registry.fetch_tables([(Ident::new("test", "origin"), old.clone())]);

    let system_ident = Ident::new("performance_schema", "origin");
    assert!(matches!(
        registry.repair(&system_ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message))
            if message == "memory or system database is not for repair"
    ));

    let mixed_case_ident = Ident::new("TeSt", "OrIgIn");
    assert!(!registry.is_visible(&mixed_case_ident));

    let mut incompatible = old.clone();
    incompatible.columns[0].kind = ColumnKind::String;
    assert!(matches!(
        registry.repair(&mixed_case_ident, incompatible, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Column a type should be the same"
    ));
    assert!(!registry.is_visible(&mixed_case_ident));

    let repaired = registry.repair(&mixed_case_ident, old, false).unwrap();
    assert_eq!(101, repaired.id);
    assert!(registry.is_visible(&mixed_case_ident));
}
