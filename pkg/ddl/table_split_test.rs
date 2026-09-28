// Copyright 2025 PingCAP, Inc.
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

// 表 Region 分裂（table split / scatter region）相关测试。
//
// Region 是 TiKV 中的数据分片单元；表级/索引级 split policy 指定按主键或索引键
// 预分裂 Region，以改善热点与并行度。`tidb_scatter_region` 控制建表时是否自动打散。
//

// Copyright 2026 AsterSQL.

use std::collections::{BTreeMap, BTreeSet};

use crate::executor::{
    ColumnInfo, IndexInfo, TableInfo as ExecutorTableInfo, warn_missing_region_split_policy,
};
use crate::table::{TableInfo, TableState, alter_region_split_policy, table_physical_ids};

/// 构造带两个分区 ID 的测试表。
fn split_table() -> TableInfo {
    TableInfo {
        id: 10,
        schema_id: 1,
        name: "orders".into(),
        state: TableState::Public,
        partition_ids: vec![11, 12],
        auto_increment_id: 0,
        auto_random_id: 0,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".into(),
        collation: "utf8mb4_bin".into(),
        version: 1,
        foreign_keys: vec![],
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    }
}

/// Go TestTableSplit 的可执行元数据契约：分区表按各分区物理 ID 分裂，
/// 非分区表则使用表 ID。
#[test]
fn table_split_uses_every_physical_partition_id() {
    let mut table = split_table();
    assert_eq!(vec![11, 12], table_physical_ids(&table));

    table.partition_ids.clear();
    assert_eq!(vec![10], table_physical_ids(&table));
}

/// Go TestTableSplitPolicy 的可执行元数据契约：完整策略文本会被持久化，
/// 重复写入幂等，替换与清除均被视为真实变更。
#[test]
fn table_split_policy_persists_replaces_and_clears() {
    let mut table = split_table();
    let first = "BETWEEN (0) AND (1000000) REGIONS 4";
    assert!(alter_region_split_policy(&mut table, Some(first.into())));
    assert_eq!(Some(first), table.split_policy.as_deref());
    assert!(!alter_region_split_policy(&mut table, Some(first.into())));

    let replacement = "BETWEEN (100) AND (100000) REGIONS 3";
    assert!(alter_region_split_policy(
        &mut table,
        Some(replacement.into())
    ));
    assert_eq!(Some(replacement), table.split_policy.as_deref());
    assert!(alter_region_split_policy(&mut table, None));
    assert_eq!(None, table.split_policy);
    assert!(!alter_region_split_policy(&mut table, None));
}

/// Go TestTableSplitPolicyMultipleIndexes 的可执行元数据契约：多个索引的策略
/// 彼此独立，未配置策略的索引保持为空。
#[test]
fn multiple_index_split_policies_remain_independent() {
    let mut idx_user = IndexInfo::new("idx_user", vec!["user_id".into()]);
    idx_user.split_policy = Some("BETWEEN (100) AND (100000) REGIONS 3".into());
    let mut idx_status = IndexInfo::new("idx_status", vec!["status".into()]);
    idx_status.split_policy = Some("BETWEEN ('a') AND ('z') REGIONS 2".into());
    let idx_created = IndexInfo::new("idx_created", vec!["created_at".into()]);

    let policies = [idx_user, idx_status, idx_created]
        .into_iter()
        .map(|index| (index.name, index.split_policy))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        Some("BETWEEN (100) AND (100000) REGIONS 3"),
        policies["idx_user"].as_deref()
    );
    assert_eq!(
        Some("BETWEEN ('a') AND ('z') REGIONS 2"),
        policies["idx_status"].as_deref()
    );
    assert_eq!(None, policies["idx_created"]);
}

/// Go TestTableSplitPolicyWarning 的可执行契约：表上任一既有索引带策略时，
/// 新增无策略索引会追加包含新索引名与热点原因的 warning。
#[test]
fn adding_index_without_split_policy_emits_go_equivalent_warning() {
    let mut existing = IndexInfo::new("idx_a", vec!["a".into()]);
    existing.split_policy = Some("BETWEEN (0) AND (100) REGIONS 4".into());
    let table = ExecutorTableInfo {
        indexes: vec![existing],
        ..ExecutorTableInfo::new("orders", vec![ColumnInfo::integer("a")])
    };
    let mut warnings = vec!["existing warning".to_owned()];
    warn_missing_region_split_policy(&table, "idx_b", &mut warnings);
    assert_eq!(2, warnings.len());
    assert_eq!("existing warning", warnings[0]);
    assert!(warnings[1].contains("region split strategy"));
    assert!(warnings[1].contains("idx_b"));
    assert!(warnings[1].contains("write hotspots"));

    // 已有索引均无 policy 时，新增索引不产生 warning。
    let no_policy = ExecutorTableInfo {
        indexes: vec![IndexInfo::new("idx_a", vec!["a".into()])],
        ..ExecutorTableInfo::new("orders", vec![ColumnInfo::integer("a")])
    };
    warnings.clear();
    warn_missing_region_split_policy(&no_policy, "idx_b", &mut warnings);
    assert!(warnings.is_empty());
}
