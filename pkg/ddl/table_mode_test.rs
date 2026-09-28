// Copyright 2026 AsterSQL.
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

// 表模式（Table Mode）相关测试。
//
// 可执行部分覆盖模式切换、版本递增与 Import/Restore 下的操作权限矩阵。

/*

// get_cloned_table_info_from_domain 对应 Go helper：从 InfoSchema 读取表并 Clone 元信息。
fn get_cloned_table_info_from_domain(
    t: testing::T,
    db_name: &str,
    table_name: &str,
    dom: &domain::Domain,
) -> *mut model::TableInfo {
    let tbl = dom
        .InfoSchema()
        .TableByName(context::Background(), ast::NewCIStr(db_name), ast::NewCIStr(table_name))
        .expect("Go require.NoError");
    tbl.Meta().Clone()
}
*/

use crate::table_mode::{
    TableInfo, TableMode, TableOperation, alter_table_mode, on_alter_table_mode, table_mode_allows,
};

/// 校验模式切换时 ID 匹配、幂等不升版本，以及 Import↔Restore 非法互转。
#[test]
fn table_mode_transitions_validate_ids_and_advance_version_once() {
    let mut table = TableInfo {
        schema_id: 7,
        table_id: 11,
        mode: TableMode::Normal,
        version: 5,
    };
    // Normal -> Import：版本 5→6。
    assert_eq!(
        6,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    // 幂等：Go 的 onAlterTableMode 不更新 schema，返回零值版本，表版本保持 6。
    assert_eq!(
        0,
        on_alter_table_mode(&mut table, 7, 11, TableMode::Import).unwrap()
    );
    assert_eq!(6, table.version);
    // table_id 不匹配应失败。
    assert!(on_alter_table_mode(&mut table, 7, 12, TableMode::Normal).is_err());
    // Import 不能直接切到 Restore。
    assert!(alter_table_mode(&mut table, TableMode::Restore).is_err());
    // 先回 Normal，再进 Restore；随后 Restore→Import 仍非法。
    assert!(alter_table_mode(&mut table, TableMode::Normal).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Restore).unwrap());
    assert!(alter_table_mode(&mut table, TableMode::Import).is_err());
}

/// 校验 Import/Restore 仅允许 Metadata/Checksum，Normal 允许全部操作。
#[test]
fn protected_table_modes_allow_metadata_and_checksum_only() {
    for mode in [TableMode::Import, TableMode::Restore] {
        assert!(table_mode_allows(mode, TableOperation::Metadata));
        assert!(table_mode_allows(mode, TableOperation::Checksum));
        for operation in [
            TableOperation::Read,
            TableOperation::Write,
            TableOperation::Alter,
            TableOperation::Drop,
        ] {
            assert!(!table_mode_allows(mode, operation));
        }
    }
    for operation in [
        TableOperation::Metadata,
        TableOperation::Checksum,
        TableOperation::Read,
        TableOperation::Write,
        TableOperation::Alter,
        TableOperation::Drop,
    ] {
        assert!(table_mode_allows(TableMode::Normal, operation));
    }
}
