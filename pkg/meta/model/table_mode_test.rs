// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// TableMode（表模式）转换矩阵单元测试。
//
// TableMode 区分 Normal / Import / Restore：IMPORT INTO 与 BR restore（备份恢复）期间
// 会限制普通 DML/DDL；本测试核对 `CanTransitionTo` 是否禁止 Import↔Restore 互转。

/// 保留 Go 侧 `table_mode_test.go` 参考片段，便于对照迁移前后断言矩阵。
use crate::group_3::*;

#[test]
fn test_table_mode_can_transition_to() {
    use TableMode::{TableModeImport, TableModeNormal, TableModeRestore};
    let tests = [
        ("normal to normal", TableModeNormal, TableModeNormal, true),
        ("normal to import", TableModeNormal, TableModeImport, true),
        ("normal to restore", TableModeNormal, TableModeRestore, true),
        ("import to normal", TableModeImport, TableModeNormal, true),
        ("import to import", TableModeImport, TableModeImport, true),
        (
            "import to restore",
            TableModeImport,
            TableModeRestore,
            false,
        ),
        ("restore to normal", TableModeRestore, TableModeNormal, true),
        (
            "restore to import",
            TableModeRestore,
            TableModeImport,
            false,
        ),
        (
            "restore to restore",
            TableModeRestore,
            TableModeRestore,
            true,
        ),
    ];

    for (name, from, to, expect) in tests {
        // Go 的 t.Run(name) 子测试语义在这里用断言消息保留，方便定位转换矩阵失败项。
        assert_eq!(expect, from.CanTransitionTo(to), "{}", name);
    }
}
use crate::group_3::TableMode;

/// 精简版转换矩阵：Normal 可进特殊模式，特殊模式可回 Normal，但 Import 与 Restore 不可互转。
#[test]
fn table_mode_transition_matrix_blocks_cross_special_modes() {
    use TableMode::{TableModeImport, TableModeNormal, TableModeRestore};
    // Normal 可切换到 Import / Restore。
    assert!(TableModeNormal.CanTransitionTo(TableModeImport));
    assert!(TableModeNormal.CanTransitionTo(TableModeRestore));
    // Import / Restore 均可回到 Normal。
    assert!(TableModeImport.CanTransitionTo(TableModeNormal));
    assert!(TableModeRestore.CanTransitionTo(TableModeNormal));
    // 禁止两个特殊模式互相切换，避免导入与恢复状态冲突。
    assert!(!TableModeImport.CanTransitionTo(TableModeRestore));
    assert!(!TableModeRestore.CanTransitionTo(TableModeImport));
}
