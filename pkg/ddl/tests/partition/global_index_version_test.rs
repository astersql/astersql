// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 全局索引（Global Index）版本与能力开关相关单元测试。
//
// 全局索引跨分区表所有物理分区维护一份唯一/二级索引，区别于每个分区各自一份的本地索引。
// `GlobalIndexVersion`（Legacy / V1 / V2）标记元数据格式代际；
// `Get/SetGlobalIndexV1Supported` 控制集群是否接受 V1 能力边界。

use astersql_ddl::index::set_global_index_version;
use astersql_meta_model::{
    ColumnInfo, GetGlobalIndexV1Supported, GlobalIndexVersionLegacy, GlobalIndexVersionV1,
    GlobalIndexVersionV2, IndexColumn, IndexInfo, SetGlobalIndexV1Supported, TableInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::{NotNullFlag, PreventNullInsertFlag};
use std::sync::Mutex;

static GLOBAL_INDEX_SUPPORT_LOCK: Mutex<()> = Mutex::new(());

struct GlobalIndexSupportGuard(bool);

impl Drop for GlobalIndexSupportGuard {
    fn drop(&mut self) {
        SetGlobalIndexV1Supported(self.0);
    }
}

fn column(name: &str, not_null: bool) -> ColumnInfo {
    let mut column = ColumnInfo {
        Name: NewCIStr(name),
        ..Default::default()
    };
    if not_null {
        column.SetFlag(NotNullFlag);
    }
    column
}

fn index(global: bool, unique: bool, column_name: &str) -> IndexInfo {
    IndexInfo {
        Global: global,
        Unique: unique,
        Columns: vec![IndexColumn {
            Name: NewCIStr(column_name),
            Offset: 0,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// 验证 V1 能力开关可读写，且三个版本常量边界值稳定（0/1/2）。
#[test]
fn global_index_capability_switch_preserves_all_version_boundaries() {
    let _lock = GLOBAL_INDEX_SUPPORT_LOCK.lock().unwrap();
    // 先保存原值，测完再还原，避免污染其它测试。
    let original = GetGlobalIndexV1Supported();
    SetGlobalIndexV1Supported(false);
    assert!(!GetGlobalIndexV1Supported());
    SetGlobalIndexV1Supported(true);
    assert!(GetGlobalIndexV1Supported());
    SetGlobalIndexV1Supported(original);

    assert_eq!(GlobalIndexVersionLegacy, 0);
    assert_eq!(GlobalIndexVersionV1, 1);
    assert_eq!(GlobalIndexVersionV2, 2);
}

/// Go `TestGlobalIndexVersion0` / `TestGlobalIndexVersion1`: capability,
/// uniqueness, nullability and clustered handles jointly choose V0 or V1.
#[test]
fn global_index_version_selection_matches_go_compatibility_rules() {
    let _lock = GLOBAL_INDEX_SUPPORT_LOCK.lock().unwrap();
    let original = GetGlobalIndexV1Supported();
    let _guard = GlobalIndexSupportGuard(original);
    let nullable_table = TableInfo {
        Columns: vec![column("b", false)],
        ..Default::default()
    };

    SetGlobalIndexV1Supported(false);
    let mut non_unique = index(true, false, "b");
    set_global_index_version(&nullable_table, &mut non_unique);
    assert_eq!(non_unique.GlobalIndexVersion, GlobalIndexVersionLegacy);

    SetGlobalIndexV1Supported(true);
    set_global_index_version(&nullable_table, &mut non_unique);
    assert_eq!(non_unique.GlobalIndexVersion, GlobalIndexVersionV1);

    let mut nullable_unique = index(true, true, "b");
    set_global_index_version(&nullable_table, &mut nullable_unique);
    assert_eq!(nullable_unique.GlobalIndexVersion, GlobalIndexVersionV1);

    let required_table = TableInfo {
        Columns: vec![column("b", true)],
        ..Default::default()
    };
    let mut required_unique = index(true, true, "b");
    set_global_index_version(&required_table, &mut required_unique);
    assert_eq!(required_unique.GlobalIndexVersion, GlobalIndexVersionLegacy);

    let mut transitioning_table = required_table.clone();
    transitioning_table.Columns[0].SetFlag(NotNullFlag | PreventNullInsertFlag);
    let mut transitioning_unique = index(true, true, "b");
    set_global_index_version(&transitioning_table, &mut transitioning_unique);
    assert_eq!(
        transitioning_unique.GlobalIndexVersion,
        GlobalIndexVersionV1
    );

    let clustered_table = TableInfo {
        PKIsHandle: true,
        Columns: vec![column("b", false)],
        ..Default::default()
    };
    let mut clustered = index(true, false, "b");
    set_global_index_version(&clustered_table, &mut clustered);
    assert_eq!(clustered.GlobalIndexVersion, GlobalIndexVersionLegacy);
}

/// Go `TestUpdateIndexesResetsGlobalIndexVersion`: changing GLOBAL to LOCAL
/// always clears the stale version before subsequent DML uses the metadata.
#[test]
fn update_indexes_global_to_local_resets_the_version() {
    let _lock = GLOBAL_INDEX_SUPPORT_LOCK.lock().unwrap();
    let original = GetGlobalIndexV1Supported();
    let _guard = GlobalIndexSupportGuard(original);
    SetGlobalIndexV1Supported(true);
    let table = TableInfo {
        Columns: vec![column("b", false)],
        ..Default::default()
    };
    let mut changed = index(true, false, "b");
    set_global_index_version(&table, &mut changed);
    assert_eq!(changed.GlobalIndexVersion, GlobalIndexVersionV1);

    changed.Global = false;
    set_global_index_version(&table, &mut changed);
    assert_eq!(changed.GlobalIndexVersion, GlobalIndexVersionLegacy);
}

/// 验证各代 `GlobalIndexVersion` 经 `IndexInfo::Clone` 后仍保留。
#[test]
fn global_index_version_survives_the_real_metadata_clone_path() {
    for version in [
        GlobalIndexVersionLegacy,
        GlobalIndexVersionV1,
        GlobalIndexVersionV2,
    ] {
        let index = IndexInfo {
            Global: true,
            GlobalIndexVersion: version,
            ..Default::default()
        };
        let clone = index.Clone();
        assert!(clone.Global);
        assert_eq!(clone.GlobalIndexVersion, version);
    }
}
