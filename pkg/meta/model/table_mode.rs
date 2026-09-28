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

// 表模式（TableMode）元数据：Normal / Import / Restore。
//
// IMPORT INTO 与 BR restore 期间会把目标表切到非 Normal 模式，以阻止普通 DML/DDL。
// 模式切换只能由内部 AlterTableMode DDL 发起；本模块只描述模式与 job 参数目标。

use serde_repr::{Deserialize_repr, Serialize_repr};

/// TableMode 在 IMPORT INTO 或 BR restore 期间阻止普通 DML/DDL 改动目标表。
/// 修改模式只能由内部 AlterTableMode DDL 发起，本类型本身不执行该操作。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum TableMode {
    #[default]
    /// 普通可读写模式。
    TableModeNormal = 0,
    /// IMPORT INTO 导入中，禁止普通写入与多数 DDL。
    TableModeImport = 1,
    /// BR restore 恢复中，禁止普通写入与多数 DDL。
    TableModeRestore = 2,
}

impl TableMode {
    /// String 对应 Go fmt.Stringer，未知值在 Go 中返回空串；Rust 枚举排除了未知判别值。
    pub fn String(self) -> &'static str {
        match self {
            Self::TableModeNormal => "Normal",
            Self::TableModeImport => "Import",
            Self::TableModeRestore => "Restore",
        }
    }

    /// 目前只禁止 Import 与 Restore 互相转换；Normal 和同模式转换均被允许。
    pub fn CanTransitionTo(self, target: TableMode) -> bool {
        !matches!(
            (self, target),
            (Self::TableModeImport, Self::TableModeRestore)
                | (Self::TableModeRestore, Self::TableModeImport)
        )
    }
}

/// AlterTableModeTarget 同时承载调用方输入和 resolver 从元数据补全的字段。
/// 该结构只描述 job 参数，不负责解析元数据或构建、提交 job。
#[derive(Clone, Debug)]
pub struct AlterTableModeTarget {
    /// 必填 schema ID，标识目标表所属 schema。
    pub SchemaID: i64,
    /// 跨 keyspace 请求必填；本地 DDL 请求由运行时 resolver 补全并校验。
    pub SchemaName: ast::CIStr,
    /// 必填 table ID，标识需要切换模式的表。
    pub TableID: i64,
    /// 跨 keyspace 请求必填；本地请求在构建 job 前从元数据补全。
    pub TableName: ast::CIStr,
    /// resolver 从当前表元数据读取，供 BuildAlterTableModeJob 使用。
    pub CurrentMode: TableMode,
    /// 调用方请求的目标模式。
    pub TargetMode: TableMode,
}
