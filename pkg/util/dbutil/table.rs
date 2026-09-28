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

// 表级辅助：按名查列、表模式（Normal/Import/Restore）保护校验。
//
// 对应 Go `pkg/util/dbutil` 中表相关工具。`TableMode` 区分普通、导入、恢复等状态；
// 非 Normal 时拒绝可能破坏受保护表的操作。

use astersql_infoschema::{CiString, ColumnInfo};

/// 表运行模式：普通、导入中、恢复中。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableMode {
    /// 普通可读写模式。
    #[default]
    Normal,
    /// 导入流程中的受保护模式。
    Import,
    /// 恢复流程中的受保护模式。
    Restore,
}

/// 在列元数据切片中按列名（大小写不敏感）查找列。
pub fn FindColumnByName<'a>(columns: &'a [ColumnInfo], name: &str) -> Option<&'a ColumnInfo> {
    // 列名比较统一转小写，对齐 MySQL 对标识符折叠的常见行为。
    let name = name.to_lowercase();
    columns.iter().find(|column| column.name.lower == name)
}

/// 断言表处于 Normal 模式；否则返回含表名与模式的保护错误。
pub fn CheckTableModeIsNormal(table_name: &CiString, table_mode: TableMode) -> Result<(), String> {
    if table_mode != TableMode::Normal {
        let mode = match table_mode {
            TableMode::Normal => "Normal",
            TableMode::Import => "Import",
            TableMode::Restore => "Restore",
        };
        Err(format!(
            "ErrProtectedTableMode: Table {} is in mode {}",
            table_name.original, mode
        ))
    } else {
        Ok(())
    }
}
