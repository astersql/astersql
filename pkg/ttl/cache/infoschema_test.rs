// Copyright 2026 AsterSQL.
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

// `InfoSchemaCache` 行为测试：刷新判定、非分区/分区表同步与过滤条件。
//
// 通过 `MockInfoSchema` 注入可控的 schema 版本与 TTL 表列表，覆盖 Go
// `TestInfoSchemaCache` 的核心断言。

use std::time::Duration;

use crate::infoschema::{InfoSchemaProvider, NewInfoSchemaCache, TTLTableEntry};
use crate::table::{Column, KeyKind, PartitionDefinition, TTLInfo, TableInfo, TimeUnit};

/// 构造测试用 TTL 时间列（有符号整型句柄语义）。
fn ttl_column() -> Column {
    Column {
        id: 1,
        name: "created_at".into(),
        public: true,
        key_kind: KeyKind::SignedInt,
        nullable: false,
        hidden: false,
    }
}

/// 构造带 TTL 配置的表元数据；`partitions` 为空表示非分区表。
fn ttl_table(id: i64, partitions: Vec<PartitionDefinition>) -> TableInfo {
    TableInfo {
        id,
        name: "t".into(),
        public: true,
        pk_is_handle: false,
        common_handle: false,
        columns: vec![ttl_column()],
        primary_index_offsets: Vec::new(),
        indexes: Vec::new(),
        partitions,
        ttl: Some(TTLInfo {
            column_name: "created_at".into(),
            interval: "5".into(),
            unit: TimeUnit::Year,
        }),
    }
}

/// 可注入版本与 TTL 表列表的 InfoSchema mock。
struct MockInfoSchema {
    version: i64,
    tables: Vec<TTLTableEntry>,
}
impl InfoSchemaProvider for MockInfoSchema {
    fn schema_meta_version(&self) -> i64 {
        self.version
    }
    fn ttl_tables(&self) -> Vec<TTLTableEntry> {
        self.tables.clone()
    }
}

// 对应 Go TestInfoSchemaCache：新缓存立即需要更新；Update 后 schema 版本未变时不重复扫描。
#[test]
fn test_info_schema_cache_should_update() {
    let mut cache = NewInfoSchemaCache(Duration::from_secs(3600));
    assert!(cache.ShouldUpdate());
    let schema = MockInfoSchema {
        version: 1,
        tables: Vec::new(),
    };
    cache.Update(&schema);
    assert!(!cache.ShouldUpdate());
    assert_eq!(cache.Tables.len(), 0);
}

// 对应 Go：非分区 TTL 表在 Update 后按 table id 建立唯一条目。
#[test]
fn test_info_schema_cache_syncs_new_table() {
    let mut cache = NewInfoSchemaCache(Duration::from_secs(3600));
    let schema = MockInfoSchema {
        version: 2,
        tables: vec![TTLTableEntry {
            schema: "test".into(),
            table: ttl_table(1, Vec::new()),
            enabled: true,
        }],
    };
    cache.Update(&schema);
    assert_eq!(cache.Tables.len(), 1);
    for table in cache.Tables.values() {
        assert_eq!(table.TableInfo.name, "t");
    }
}

// 对应 Go：分区 TTL 表按分区 ID 展开为多条 PhysicalTable，并保留分区名。
#[test]
fn test_info_schema_cache_syncs_partitioned_table() {
    let mut cache = NewInfoSchemaCache(Duration::from_secs(3600));
    cache.Update(&MockInfoSchema {
        version: 2,
        tables: vec![TTLTableEntry {
            schema: "test".into(),
            table: ttl_table(1, Vec::new()),
            enabled: true,
        }],
    });
    assert!(cache.Tables.contains_key(&1));

    let partitions = vec![
        PartitionDefinition {
            id: 10,
            name: "p0".into(),
        },
        PartitionDefinition {
            id: 11,
            name: "p1".into(),
        },
    ];
    let schema = MockInfoSchema {
        version: 3,
        tables: vec![TTLTableEntry {
            schema: "test".into(),
            table: ttl_table(1, partitions),
            enabled: true,
        }],
    };
    cache.Update(&schema);
    assert_eq!(cache.Tables.len(), 2);
    assert!(!cache.Tables.contains_key(&1));
    let mut names: Vec<String> = Vec::new();
    for (id, table) in &cache.Tables {
        assert_eq!(table.TableInfo.name, "t");
        let def = table.PartitionDef.as_ref().expect("partition def expected");
        assert_eq!(def.id, *id);
        names.push(def.name.clone());
    }
    names.sort();
    assert_eq!(names, vec!["p0".to_owned(), "p1".to_owned()]);
}

// 对应 Go 中过滤条件：未开启 TTL 或表非 public 的表不会出现在缓存里。
#[test]
fn test_info_schema_cache_skips_disabled_and_non_public_tables() {
    let mut cache = NewInfoSchemaCache(Duration::from_secs(3600));
    let mut non_public = ttl_table(2, Vec::new());
    non_public.public = false;
    let schema = MockInfoSchema {
        version: 4,
        tables: vec![
            TTLTableEntry {
                schema: "test".into(),
                table: ttl_table(1, Vec::new()),
                enabled: false,
            },
            TTLTableEntry {
                schema: "test".into(),
                table: non_public,
                enabled: true,
            },
        ],
    };
    cache.Update(&schema);
    assert_eq!(cache.Tables.len(), 0);
}
