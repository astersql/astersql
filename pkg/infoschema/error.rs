// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// InfoSchema 相关 schema 错误常量与分类。
//
// 对应 Go `dbterror.ClassSchema`（及个别 `ClassExecutor`）下的标准错误：
// 库/表/列/索引/外键/放置策略/资源组/脱敏策略等对象的存在性、冲突与权限错误。
// 每个常量携带 MySQL 风格错误名，供上层格式化消息与错误码映射。

// Lazy error metadata values.

/* Mechanical error table retained for migration history.
use std::sync::LazyLock;

// schema_error! 保留绝大多数 Go `dbterror.ClassSchema.NewStd` 初始化形状。
// LazyLock 对应 Go 包初始化期创建一次的共享错误值，调用点不会重复分配标准错误。
macro_rules! schema_error {
    ($name:ident, $code:ident) => {
        pub static $name: LazyLock<dbterror::Error> =
            LazyLock::new(|| dbterror::ClassSchema.NewStd(errno::$code));
    };
}

// 数据库创建、删除、访问和不存在错误。
schema_error!(ErrDatabaseExists, ErrDBCreateExists);
schema_error!(ErrDatabaseDropExists, ErrDBDropExists);
schema_error!(ErrAccessDenied, ErrAccessDenied);
schema_error!(ErrDatabaseNotExists, ErrBadDB);

// placement policy 与 resource group 的存在性、角色和功能状态错误。
schema_error!(ErrPlacementPolicyExists, ErrPlacementPolicyExists);
schema_error!(ErrPlacementPolicyNotExists, ErrPlacementPolicyNotExists);
schema_error!(ErrResourceGroupExists, ErrResourceGroupExists);
schema_error!(ErrResourceGroupNotExists, ErrResourceGroupNotExists);

// 该错误在 Go 中唯一使用 ClassExecutor，而不是 ClassSchema；保留其错误分类差异。
pub static ErrResourceGroupInvalidBackgroundTaskName: LazyLock<dbterror::Error> = LazyLock::new(|| {
    dbterror::ClassExecutor.NewStd(errno::ErrResourceGroupInvalidBackgroundTaskName)
});

schema_error!(ErrResourceGroupInvalidForRole, ErrResourceGroupInvalidForRole);
schema_error!(ErrReservedSyntax, ErrReservedSyntax);

// 表、序列、列和索引的存在性及重复声明错误。
schema_error!(ErrTableExists, ErrTableExists);
schema_error!(ErrTableDropExists, ErrBadTable);
schema_error!(ErrSequenceDropExists, ErrUnknownSequence);
schema_error!(ErrColumnNotExists, ErrBadField);
schema_error!(ErrColumnExists, ErrDupFieldName);
schema_error!(ErrKeyNameDuplicate, ErrDupKeyName);
schema_error!(ErrNonuniqTable, ErrNonuniqTable);
schema_error!(ErrMultiplePriKey, ErrMultiplePriKey);
schema_error!(ErrTooManyKeyParts, ErrTooManyKeyParts);

// 外键缺失以及显式表锁访问错误。
schema_error!(ErrForeignKeyNotExists, ErrCantDropFieldOrKey);
schema_error!(ErrTableNotLockedForWrite, ErrTableNotLockedForWrite);
schema_error!(ErrTableNotLocked, ErrTableNotLocked);
schema_error!(ErrTableNotExists, ErrNoSuchTable);
schema_error!(ErrKeyNotExists, ErrKeyDoesNotExist);

// 外键新增、分区限制、定义不匹配和索引重复错误。
schema_error!(ErrCannotAddForeign, ErrCannotAddForeign);
schema_error!(ErrForeignKeyOnPartitioned, ErrForeignKeyOnPartitioned);
schema_error!(ErrForeignKeyNotMatch, ErrWrongFkDef);
schema_error!(ErrIndexExists, ErrDupIndex);

// 用户对象、锁、对象类型和管理检查错误。
schema_error!(ErrUserDropExists, ErrBadUser);
schema_error!(ErrUserAlreadyExists, ErrUserAlreadyExists);
schema_error!(ErrTableLocked, ErrTableLocked);
schema_error!(ErrWrongObject, ErrWrongObject);
schema_error!(ErrAdminCheckTable, ErrAdminCheckTable);

// ErrEmptyDatabase 与 ErrDatabaseNotExists 都映射 ErrBadDB，保留 Go 中不同语义名称共享错误码的关系。
schema_error!(ErrEmptyDatabase, ErrBadDB);
schema_error!(ErrForbidSchemaChange, ErrForbidSchemaChange);
schema_error!(ErrTableWithoutPrimaryKey, ErrTableWithoutPrimaryKey);

// 外键引用虚拟列、父表、父列、父索引及 SET NULL/NOT NULL 冲突错误。
schema_error!(ErrForeignKeyCannotUseVirtualColumn, ErrForeignKeyCannotUseVirtualColumn);
schema_error!(ErrForeignKeyCannotOpenParent, ErrForeignKeyCannotOpenParent);
schema_error!(ErrForeignKeyNoColumnInParent, ErrForeignKeyNoColumnInParent);
schema_error!(ErrForeignKeyNoIndexInParent, ErrForeignKeyNoIndexInParent);
schema_error!(ErrForeignKeyColumnNotNull, ErrForeignKeyColumnNotNull);

// 功能开关、检查约束重名和表导入/恢复模式转换错误。
schema_error!(ErrResourceGroupSupportDisabled, ErrResourceGroupSupportDisabled);
schema_error!(ErrCheckConstraintDupName, ErrCheckConstraintDupName);
schema_error!(ErrProtectedTableMode, ErrProtectedTableMode);
schema_error!(ErrInvalidTableModeSet, ErrInvalidTableModeSet);
*/

#![allow(non_upper_case_globals)]

use std::fmt;

/// 错误类别：与 Go 声明中使用的 Schema / Executor 两类一致。
/// Error classes match the two classes used by the Go declarations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    /// schema 类错误（库表列索引等元数据）。
    Schema,
    /// executor 类错误（个别资源组后台任务名校验）。
    Executor,
}

/// 一条 schema 错误的元数据：错误类别 + MySQL 风格错误名。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaError {
    pub class: ErrorClass,
    pub mysql_name: &'static str,
}

impl SchemaError {
    /// 构造 Schema 类错误常量。
    pub const fn schema(mysql_name: &'static str) -> Self {
        Self {
            class: ErrorClass::Schema,
            mysql_name,
        }
    }

    /// 构造 Executor 类错误常量。
    pub const fn executor(mysql_name: &'static str) -> Self {
        Self {
            class: ErrorClass::Executor,
            mysql_name,
        }
    }

    /// 拼出带细节的错误消息：`{mysql_name}: {detail}`。
    pub fn message(&self, detail: impl fmt::Display) -> String {
        format!("{}: {}", self.mysql_name, detail)
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.mysql_name)
    }
}

/// 批量声明 Schema 类错误常量的宏。
macro_rules! schema_errors {
    ($(($rust:ident, $mysql:literal)),+ $(,)?) => {
        $(pub const $rust: SchemaError = SchemaError::schema($mysql);)+
    };
}

// 以下常量一一对应 Go ClassSchema 标准错误；名称保持与 errno / 调用点一致。
schema_errors!(
    (ErrDatabaseExists, "ErrDBCreateExists"),
    (ErrDatabaseDropExists, "ErrDBDropExists"),
    (ErrAccessDenied, "ErrAccessDenied"),
    (ErrDatabaseNotExists, "ErrBadDB"),
    (ErrPlacementPolicyExists, "ErrPlacementPolicyExists"),
    (ErrPlacementPolicyNotExists, "ErrPlacementPolicyNotExists"),
    (ErrResourceGroupExists, "ErrResourceGroupExists"),
    (ErrResourceGroupNotExists, "ErrResourceGroupNotExists"),
    (
        ErrResourceGroupInvalidForRole,
        "ErrResourceGroupInvalidForRole"
    ),
    (ErrReservedSyntax, "ErrReservedSyntax"),
    (ErrTableExists, "ErrTableExists"),
    (ErrTableDropExists, "ErrBadTable"),
    (ErrSequenceDropExists, "ErrUnknownSequence"),
    (ErrColumnNotExists, "ErrBadField"),
    (ErrColumnExists, "ErrDupFieldName"),
    (ErrKeyNameDuplicate, "ErrDupKeyName"),
    (ErrNonuniqTable, "ErrNonuniqTable"),
    (ErrMultiplePriKey, "ErrMultiplePriKey"),
    (ErrTooManyKeyParts, "ErrTooManyKeyParts"),
    (ErrForeignKeyNotExists, "ErrCantDropFieldOrKey"),
    (ErrTableNotLockedForWrite, "ErrTableNotLockedForWrite"),
    (ErrTableNotLocked, "ErrTableNotLocked"),
    (ErrTableNotExists, "ErrNoSuchTable"),
    (ErrKeyNotExists, "ErrKeyDoesNotExist"),
    (ErrCannotAddForeign, "ErrCannotAddForeign"),
    (ErrForeignKeyOnPartitioned, "ErrForeignKeyOnPartitioned"),
    (ErrForeignKeyNotMatch, "ErrWrongFkDef"),
    (ErrIndexExists, "ErrDupIndex"),
    (ErrUserDropExists, "ErrBadUser"),
    (ErrUserAlreadyExists, "ErrUserAlreadyExists"),
    (ErrTableLocked, "ErrTableLocked"),
    (ErrWrongObject, "ErrWrongObject"),
    (ErrAdminCheckTable, "ErrAdminCheckTable"),
    (ErrEmptyDatabase, "ErrBadDB"),
    (ErrForbidSchemaChange, "ErrForbidSchemaChange"),
    (ErrTableWithoutPrimaryKey, "ErrTableWithoutPrimaryKey"),
    (
        ErrForeignKeyCannotUseVirtualColumn,
        "ErrForeignKeyCannotUseVirtualColumn"
    ),
    (
        ErrForeignKeyCannotOpenParent,
        "ErrForeignKeyCannotOpenParent"
    ),
    (
        ErrForeignKeyNoColumnInParent,
        "ErrForeignKeyNoColumnInParent"
    ),
    (ErrForeignKeyNoIndexInParent, "ErrForeignKeyNoIndexInParent"),
    (ErrForeignKeyColumnNotNull, "ErrForeignKeyColumnNotNull"),
    (
        ErrResourceGroupSupportDisabled,
        "ErrResourceGroupSupportDisabled"
    ),
    (ErrCheckConstraintDupName, "ErrCheckConstraintDupName"),
    (ErrProtectedTableMode, "ErrProtectedTableMode"),
    (ErrInvalidTableModeSet, "ErrInvalidTableModeSet"),
);

/// 资源组无效后台任务名：Go 中唯一使用 ClassExecutor 的条目。
pub const ErrResourceGroupInvalidBackgroundTaskName: SchemaError =
    SchemaError::executor("ErrResourceGroupInvalidBackgroundTaskName");
