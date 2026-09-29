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

// DDL 层 terror 错误变量与 reorg 可重试边界注册表。
//
// 对应 Go `pkg/util/dbterror/ddl_terror.go`。以 `LazyLock` 延迟构造各 DDL
// 错误实例（对应 Go 包初始化），并维护 reorganization（表结构重组，如加索引回填）
// 可重试错误码与错误消息清单。

// 本文件由 pkg/util/dbterror/ddl_terror.go 迁移而来，保留 Go 错误清单和重试边界。
// 本文件描述 DDL 层 terror 错误变量、reorg 可重试错误码和可重试错误消息的注册清单。
// 错误变量使用 LazyLock 对应 Go 包初始化期构造。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::HashSet;
use std::sync::LazyLock;

use super::ClassDDL;
use astersql_parser_terror::{self as terror, parser::mysql as parser_mysql};

/// 本文件使用的 MySQL 错误码与错误名表局部别名。
mod mysql {
    pub use astersql_errno::errcode::*;
    pub use astersql_errno::errname::MySQLErrName;
}

/// 按 Go `fmt.Sprintf` 路径顺序将模板中的 `%s` 替换为实参，供 NewStdErr 组装消息。
fn format_mysql_message(template: &str, arguments: &[&str]) -> String {
    let mut formatted = template.to_owned();
    for argument in arguments {
        formatted = formatted.replacen("%s", argument, 1);
    }
    formatted
}

// 下面每个 pub static 对应 Go var 块中的一个 DDL 错误变量；顺序与源文件保持一致。
// NewStdErr 的消息先按 Go fmt.Sprintf 路径展开，再保留为 parser mysql 模板。
// ErrInvalidWorker means the worker is invalid.
// ErrInvalidWorker：DDL worker（执行 DDL 任务的后台工作者）无效。
pub static ErrInvalidWorker: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidDDLWorker));
// ErrNotOwner means we are not owner and can't handle DDL jobs.
// ErrNotOwner：当前实例不是 DDL owner（集群中唯一负责调度 DDL 的角色），不能处理 DDL 任务。
pub static ErrNotOwner: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNotOwner));
// ErrCantDecodeRecord means we can't decode the record.
pub static ErrCantDecodeRecord: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCantDecodeRecord));
// ErrInvalidDDLJob means the DDL job is invalid.
pub static ErrInvalidDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidDDLJob));
// ErrCancelledDDLJob means the DDL job is cancelled.
pub static ErrCancelledDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCancelledDDLJob));
// ErrPausedDDLJob returns when the DDL job cannot be paused.
pub static ErrPausedDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPausedDDLJob));
// ErrBDRRestrictedDDL means the DDL is restricted in BDR mode.
pub static ErrBDRRestrictedDDL: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBDRRestrictedDDL));
// ErrDDLAutoPausedByKVDiskFull means TiDB paused the DDL job because a storage node disk is full.
pub static ErrDDLAutoPausedByKVDiskFull: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDDLAutoPausedByKVDiskFull));
// ErrRunMultiSchemaChanges means we run multi schema changes.
// ErrRunMultiSchemaChanges：不支持在一次 DDL 中做多 schema 变更。
pub static ErrRunMultiSchemaChanges: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["multi schema change for %s"],
            ),
            &[],
        ),
    )
});
// ErrOperateSameColumn means we change the same columns multiple times in a DDL.
pub static ErrOperateSameColumn: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["operate same column '%s'"],
            ),
            &[],
        ),
    )
});
// ErrOperateSameIndex means we change the same indexes multiple times in a DDL.
pub static ErrOperateSameIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["operate same index '%s'"],
            ),
            &[],
        ),
    )
});
// ErrWaitReorgTimeout means we wait for reorganization timeout.
// ErrWaitReorgTimeout：等待 reorganization 超时（借用锁等待超时错误码承载消息）。
pub static ErrWaitReorgTimeout: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrLockWaitTimeout,
        &mysql::MySQLErrName[&mysql::ErrWaitReorgTimeout],
    )
});
// ErrInvalidStoreVer means invalid store version.
pub static ErrInvalidStoreVer: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidStoreVersion));
// ErrRepairTableFail is used to repair tableInfo in repair mode.
pub static ErrRepairTableFail: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrRepairTable));

// ErrUnsupportedAddColumnarIndex means add columnar index is unsupported
pub static ErrUnsupportedAddColumnarIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["add columnar index: %s"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedAddVectorIndex means add vector index is unsupported
pub static ErrUnsupportedAddVectorIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["add vector index: %s"],
            ),
            &[],
        ),
    )
});
// ErrCantDropColWithIndex means can't drop the column with index. We don't support dropping column with index covered now.
pub static ErrCantDropColWithIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["drop column with index"],
            ),
            &[],
        ),
    )
});
// ErrCantDropColWithAutoInc means can't drop column with auto_increment
pub static ErrCantDropColWithAutoInc: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(mysql::ErrUnsupportedDDLOperation, &parser_mysql::errname::Message(&format_mysql_message(&mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw, &["can't remove column with auto_increment when @@tidb_allow_remove_auto_inc disabled"]), &[]))
});
// ErrCantDropColWithCheckConstraint means can't drop column with check constraint
pub static ErrCantDropColWithCheckConstraint: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDependentByCheckConstraint));
// ErrUnsupportedEngineAttribute means engine attribute option is unsupported
pub static ErrUnsupportedEngineAttribute: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrEngineAttributeNotSupported));
// ErrUnsupportedAddColumn means add columns is unsupported
pub static ErrUnsupportedAddColumn: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["add column"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedModifyColumn means modify columns is unsupoorted
pub static ErrUnsupportedModifyColumn: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["modify column: %s"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedModifyCharset means modify charset is unsupoorted
pub static ErrUnsupportedModifyCharset: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["modify %s"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedModifyCollation means modify collation is unsupoorted
pub static ErrUnsupportedModifyCollation: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["modifying collation from %s to %s"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedPKHandle is used to indicate that we can't support this PK handle.
pub static ErrUnsupportedPKHandle: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["drop integer primary key"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedCharset means we don't support the charset.
pub static ErrUnsupportedCharset: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["charset %s and collate %s"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedShardRowIDBits means we don't support the shard_row_id_bits.
pub static ErrUnsupportedShardRowIDBits: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["shard_row_id_bits for table with primary key as row id"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedAlterTableWithValidation means we don't support the alter table with validation.
pub static ErrUnsupportedAlterTableWithValidation: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                "ALTER TABLE WITH VALIDATION is currently unsupported",
                &[],
            ),
        )
    });
// ErrUnsupportedAlterTableWithoutValidation means we don't support the alter table without validation.
pub static ErrUnsupportedAlterTableWithoutValidation: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                "ALTER TABLE WITHOUT VALIDATION is currently unsupported",
                &[],
            ),
        )
    });
// ErrUnsupportedAlterTableOption means we don't support the alter table option.
pub static ErrUnsupportedAlterTableOption: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message("This type of ALTER TABLE is currently unsupported", &[]),
    )
});
// ErrUnsupportedAlterCacheForSysTable means we don't support the alter cache for system table.
pub static ErrUnsupportedAlterCacheForSysTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                "ALTER table cache for tables in system database is currently unsupported",
                &[],
            ),
        )
    });
// ErrUnsupportedAddPartialIndex the partial index condition is not supported
pub static ErrUnsupportedAddPartialIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["add partial index: %s"],
            ),
            &[],
        ),
    )
});
// ErrModifyColumnReferencedByPartialCondition is used when a column is referenced by a partial index condition.
pub static ErrModifyColumnReferencedByPartialCondition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrModifyColumnReferencedByPartialCondition));
// ErrBlobKeyWithoutLength is used when BLOB is used as key but without a length.
pub static ErrBlobKeyWithoutLength: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBlobKeyWithoutLength));
// ErrKeyPart0 is used when key parts length is 0.
pub static ErrKeyPart0: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrKeyPart0));
// ErrIncorrectPrefixKey is used when the prefix length is incorrect for a string key.
pub static ErrIncorrectPrefixKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongSubKey));
// ErrTooLongKey is used when the column key is too long.
pub static ErrTooLongKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongKey));
// ErrKeyColumnDoesNotExits is used when the key column doesn't exist.
pub static ErrKeyColumnDoesNotExits: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrKeyColumnDoesNotExits));
// ErrInvalidDDLJobVersion is used when the DDL job version is invalid.
pub static ErrInvalidDDLJobVersion: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidDDLJobVersion));
// ErrInvalidUseOfNull is used when the column is not null.
pub static ErrInvalidUseOfNull: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidUseOfNull));
// ErrTooManyFields is used when too many columns are used in a select statement.
pub static ErrTooManyFields: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooManyFields));
// ErrTooManyKeys is used when too many keys used.
pub static ErrTooManyKeys: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooManyKeys));
// ErrInvalidSplitRegionRanges is used when split region ranges is invalid.
pub static ErrInvalidSplitRegionRanges: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidSplitRegionRanges));
// ErrReorgPanic is used when reorg process is panic.
pub static ErrReorgPanic: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrReorgPanic));
// ErrFkColumnCannotDrop is used when foreign key column can't be dropped.
pub static ErrFkColumnCannotDrop: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFkColumnCannotDrop));
// ErrFkColumnCannotDropChild is used when foreign key column can't be dropped.
pub static ErrFkColumnCannotDropChild: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFkColumnCannotDropChild));
// ErrFKIncompatibleColumns is used when foreign key column type is incompatible.
pub static ErrFKIncompatibleColumns: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFKIncompatibleColumns));
// ErrOnlyOnRangeListPartition is used when the partition type is range list.
pub static ErrOnlyOnRangeListPartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrOnlyOnRangeListPartition));
// ErrWrongKeyColumn is for table column cannot be indexed.
pub static ErrWrongKeyColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongKeyColumn));
// ErrWrongKeyColumnFunctionalIndex is for expression cannot be indexed.
pub static ErrWrongKeyColumnFunctionalIndex: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongKeyColumnFunctionalIndex));
// ErrWrongFKOptionForGeneratedColumn is for wrong foreign key reference option on generated columns.
pub static ErrWrongFKOptionForGeneratedColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongFKOptionForGeneratedColumn));
// ErrUnsupportedOnGeneratedColumn is for unsupported actions on generated columns.
pub static ErrUnsupportedOnGeneratedColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnsupportedOnGeneratedColumn));
// ErrGeneratedColumnNonPrior forbids to refer generated column non prior to it.
pub static ErrGeneratedColumnNonPrior: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrGeneratedColumnNonPrior));
// ErrDependentByGeneratedColumn forbids to delete columns which are dependent by generated columns.
pub static ErrDependentByGeneratedColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDependentByGeneratedColumn));
// ErrJSONUsedAsKey forbids to use JSON as key or index.
pub static ErrJSONUsedAsKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrJSONUsedAsKey));
// ErrBlobCantHaveDefault forbids to give not null default value to TEXT/BLOB/JSON.
pub static ErrBlobCantHaveDefault: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBlobCantHaveDefault));
// ErrTooLongIndexComment means the comment for index is too long.
pub static ErrTooLongIndexComment: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongIndexComment));
// ErrTooLongTableComment means the comment for table is too long.
pub static ErrTooLongTableComment: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongTableComment));
// ErrTooLongFieldComment means the comment for field/column is too long.
pub static ErrTooLongFieldComment: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongFieldComment));
// ErrTooLongTablePartitionComment means the comment for table partition is too long.
pub static ErrTooLongTablePartitionComment: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongTablePartitionComment));
// ErrInvalidDefaultValue returns for invalid default value for columns.
pub static ErrInvalidDefaultValue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidDefault));
// ErrDefValGeneratedNamedFunctionIsNotAllowed returns for disallowed function as default value expression of column.
pub static ErrDefValGeneratedNamedFunctionIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDefValGeneratedNamedFunctionIsNotAllowed));
// ErrGeneratedColumnRefAutoInc forbids to refer generated columns to auto-increment columns .
pub static ErrGeneratedColumnRefAutoInc: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrGeneratedColumnRefAutoInc));
// ErrExpressionIndexCanNotRefer forbids to refer expression index to auto-increment column.
pub static ErrExpressionIndexCanNotRefer: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexRefAutoIncrement));
// ErrUnsupportedAddPartition returns for does not support add partitions.
pub static ErrUnsupportedAddPartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["add partitions"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedCoalescePartition returns for does not support coalesce partitions.
pub static ErrUnsupportedCoalescePartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["coalesce partitions"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedReorganizePartition returns for does not support reorganize partitions.
pub static ErrUnsupportedReorganizePartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["reorganize partition"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedCheckPartition returns for does not support check partitions.
pub static ErrUnsupportedCheckPartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["check partition"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedOptimizePartition returns for does not support optimize partitions.
pub static ErrUnsupportedOptimizePartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["optimize partition"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedRebuildPartition returns for does not support rebuild partitions.
pub static ErrUnsupportedRebuildPartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["rebuild partition"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedRemovePartition returns for does not support remove partitions.
pub static ErrUnsupportedRemovePartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["remove partitioning"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedRepairPartition returns for does not support repair partitions.
pub static ErrUnsupportedRepairPartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["repair partition"],
            ),
            &[],
        ),
    )
});
// ErrGeneratedColumnFunctionIsNotAllowed returns for unsupported functions for generated columns.
pub static ErrGeneratedColumnFunctionIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrGeneratedColumnFunctionIsNotAllowed));
// ErrGeneratedColumnRowValueIsNotAllowed returns for generated columns referring to row values.
pub static ErrGeneratedColumnRowValueIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrGeneratedColumnRowValueIsNotAllowed));
// ErrUnsupportedPartitionByRangeColumns returns for does unsupported partition by range columns.
pub static ErrUnsupportedPartitionByRangeColumns: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                    &["partition by range columns"],
                ),
                &[],
            ),
        )
    });
// ErrFunctionalIndexFunctionIsNotAllowed returns for unsupported functions for functional index.
pub static ErrFunctionalIndexFunctionIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexFunctionIsNotAllowed));
// ErrFunctionalIndexRowValueIsNotAllowed returns for functional index referring to row values.
pub static ErrFunctionalIndexRowValueIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexRowValueIsNotAllowed));
// ErrUnsupportedCreatePartition returns for does not support create partitions.
pub static ErrUnsupportedCreatePartition: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["partition type, treat as normal table"],
            ),
            &[],
        ),
    )
});
// ErrUnsupportedIndexType returns for unsupported index type.
pub static ErrUnsupportedIndexType: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["index type"],
            ),
            &[],
        ),
    )
});
// ErrWindowInvalidWindowFuncUse returns for invalid window function use.
pub static ErrWindowInvalidWindowFuncUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWindowInvalidWindowFuncUse));

// ErrDupKeyName returns for duplicated key name.
pub static ErrDupKeyName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDupKeyName));
// ErrFkDupName returns for duplicated FK name.
pub static ErrFkDupName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFkDupName));
// ErrInvalidDDLState returns for invalid ddl model object state.
pub static ErrInvalidDDLState: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrInvalidDDLState,
        &parser_mysql::errname::Message(&mysql::MySQLErrName[&mysql::ErrInvalidDDLState].Raw, &[]),
    )
});
// ErrUnsupportedModifyPrimaryKey returns an error when add or drop the primary key.
// It's exported for testing.
pub static ErrUnsupportedModifyPrimaryKey: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["%s primary key"],
            ),
            &[],
        ),
    )
});
// ErrPKIndexCantBeInvisible return an error when primary key is invisible index
pub static ErrPKIndexCantBeInvisible: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPKIndexCantBeInvisible));

// ErrColumnBadNull returns for a bad null value.
pub static ErrColumnBadNull: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBadNull));
// ErrBadField forbids to refer to unknown column.
pub static ErrBadField: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBadField));
// ErrCantRemoveAllFields returns for deleting all columns.
pub static ErrCantRemoveAllFields: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCantRemoveAllFields));
// ErrCantDropFieldOrKey returns for dropping a non-existent field or key.
pub static ErrCantDropFieldOrKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCantDropFieldOrKey));
// ErrInvalidOnUpdate returns for invalid ON UPDATE clause.
pub static ErrInvalidOnUpdate: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidOnUpdate));
// ErrTooLongIdent returns for too long name of database/table/column/index.
pub static ErrTooLongIdent: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongIdent));
// ErrWrongDBName returns for wrong database name.
pub static ErrWrongDBName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongDBName));
// ErrWrongTableName returns for wrong table name.
pub static ErrWrongTableName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongTableName));
// ErrWrongColumnName returns for wrong column name.
pub static ErrWrongColumnName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongColumnName));
// ErrWrongPartitionName returns for wrong partition name.
pub static ErrWrongPartitionName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongPartitionName));
// ErrWrongUsage returns for wrong ddl syntax usage.
pub static ErrWrongUsage: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongUsage));
// ErrInvalidGroupFuncUse returns for using invalid group functions.
pub static ErrInvalidGroupFuncUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidGroupFuncUse));
// ErrTableMustHaveColumns returns for missing column when creating a table.
pub static ErrTableMustHaveColumns: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTableMustHaveColumns));
// ErrWrongNameForIndex returns for wrong index name.
pub static ErrWrongNameForIndex: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongNameForIndex));
// ErrUnknownCharacterSet returns unknown character set.
pub static ErrUnknownCharacterSet: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnknownCharacterSet));
// ErrUnknownCollation returns unknown collation.
pub static ErrUnknownCollation: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnknownCollation));
// ErrCollationCharsetMismatch returns when collation not match the charset.
pub static ErrCollationCharsetMismatch: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCollationCharsetMismatch));
// ErrConflictingDeclarations return conflict declarations.
pub static ErrConflictingDeclarations: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrConflictingDeclarations,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrConflictingDeclarations].Raw,
                &["CHARACTER SET ", "%s", "CHARACTER SET ", "%s"],
            ),
            &[],
        ),
    )
});
// ErrPrimaryCantHaveNull returns All parts of a PRIMARY KEY must be NOT NULL; if you need NULL in a key, use UNIQUE instead
pub static ErrPrimaryCantHaveNull: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPrimaryCantHaveNull));
// ErrErrorOnRename returns error for wrong database name in alter table rename
pub static ErrErrorOnRename: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrErrorOnRename));
// ErrViewSelectClause returns error for create view with select into clause
pub static ErrViewSelectClause: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrViewSelectClause));
// ErrViewSelectVariable returns error for create view with select into clause
pub static ErrViewSelectVariable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrViewSelectVariable));

// ErrNotAllowedTypeInPartition returns not allowed type error when creating table partition with unsupported expression type.
pub static ErrNotAllowedTypeInPartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFieldTypeNotAllowedAsPartitionField));
// ErrPartitionMgmtOnNonpartitioned returns it's not a partition table.
pub static ErrPartitionMgmtOnNonpartitioned: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionMgmtOnNonpartitioned));
// ErrDropPartitionNonExistent returns error in list of partition.
pub static ErrDropPartitionNonExistent: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDropPartitionNonExistent));
// ErrSameNamePartition returns duplicate partition name.
pub static ErrSameNamePartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSameNamePartition));
// ErrSameNamePartitionField returns duplicate partition field.
pub static ErrSameNamePartitionField: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSameNamePartitionField));
// ErrRangeNotIncreasing returns values less than value must be strictly increasing for each partition.
pub static ErrRangeNotIncreasing: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrRangeNotIncreasing));
// ErrPartitionMaxvalue returns maxvalue can only be used in last partition definition.
pub static ErrPartitionMaxvalue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionMaxvalue));
// ErrMaxvalueInValuesIn returns maxvalue cannot be used in values in.
pub static ErrMaxvalueInValuesIn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrMaxvalueInValuesIn));
// ErrDropLastPartition returns cannot remove all partitions, use drop table instead.
pub static ErrDropLastPartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDropLastPartition));
// ErrTooManyPartitions returns too many partitions were defined.
pub static ErrTooManyPartitions: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooManyPartitions));
// ErrPartitionConstDomain returns partition constant is out of partition function domain.
pub static ErrPartitionConstDomain: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionConstDomain));
// ErrPartitionFunctionIsNotAllowed returns this partition function is not allowed.
pub static ErrPartitionFunctionIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionFunctionIsNotAllowed));
// ErrPartitionFuncNotAllowed returns partition function returns the wrong type.
pub static ErrPartitionFuncNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionFuncNotAllowed));
// ErrUniqueKeyNeedAllFieldsInPf returns must include all columns in the table's partitioning function.
pub static ErrUniqueKeyNeedAllFieldsInPf: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUniqueKeyNeedAllFieldsInPf));
// ErrWrongExprInPartitionFunc Constant, random or timezone-dependent expressions in (sub)partitioning function are not allowed.
pub static ErrWrongExprInPartitionFunc: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongExprInPartitionFunc));
// ErrWarnDataTruncated returns data truncated error.
pub static ErrWarnDataTruncated: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::WarnDataTruncated));
// ErrCoalesceOnlyOnHashPartition returns coalesce partition can only be used on hash/key partitions.
pub static ErrCoalesceOnlyOnHashPartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCoalesceOnlyOnHashPartition));
// ErrViewWrongList returns create view must include all columns in the select clause
pub static ErrViewWrongList: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrViewWrongList));
// ErrAlterOperationNotSupported returns when alter operations is not supported.
pub static ErrAlterOperationNotSupported: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrAlterOperationNotSupportedReason));
// ErrWrongObject returns for wrong object.
pub static ErrWrongObject: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongObject));
// ErrTableCantHandleFt returns FULLTEXT keys are not supported by table type
pub static ErrTableCantHandleFt: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTableCantHandleFt));
// ErrFieldNotFoundPart returns an error when 'partition by columns' are not found in table columns.
pub static ErrFieldNotFoundPart: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFieldNotFoundPart));
// ErrWrongTypeColumnValue returns 'Partition column values of incorrect type'
pub static ErrWrongTypeColumnValue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongTypeColumnValue));
// ErrValuesIsNotIntType returns 'VALUES value for partition '%-.64s' must have type INT'
pub static ErrValuesIsNotIntType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrValuesIsNotIntType));
// ErrFunctionalIndexPrimaryKey returns 'The primary key cannot be a functional index'
pub static ErrFunctionalIndexPrimaryKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexPrimaryKey));
// ErrFunctionalIndexOnField returns 'Functional index on a column is not supported. Consider using a regular index instead'
pub static ErrFunctionalIndexOnField: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexOnField));
// ErrInvalidAutoRandom returns when auto_random is used incorrectly.
pub static ErrInvalidAutoRandom: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidAutoRandom));
// ErrUnsupportedConstraintCheck returns when use ADD CONSTRAINT CHECK
pub static ErrUnsupportedConstraintCheck: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnsupportedConstraintCheck));
// ErrDerivedMustHaveAlias returns when a sub select statement does not have a table alias.
pub static ErrDerivedMustHaveAlias: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDerivedMustHaveAlias));
// ErrNullInValuesLessThan returns when a range partition LESS THAN expression includes a NULL
pub static ErrNullInValuesLessThan: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNullInValuesLessThan));

// ErrSequenceRunOut returns when the sequence has been run out.
pub static ErrSequenceRunOut: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSequenceRunOut));
// ErrSequenceInvalidData returns when sequence values are conflicting.
pub static ErrSequenceInvalidData: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSequenceInvalidData));
// ErrSequenceAccessFail returns when sequences are not able to access.
pub static ErrSequenceAccessFail: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSequenceAccessFail));
// ErrNotSequence returns when object is not a sequence.
pub static ErrNotSequence: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNotSequence));
// ErrUnknownSequence returns when drop / alter unknown sequence.
pub static ErrUnknownSequence: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnknownSequence));
// ErrSequenceUnsupportedTableOption returns when unsupported table option exists in sequence.
pub static ErrSequenceUnsupportedTableOption: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSequenceUnsupportedTableOption));
// ErrColumnTypeUnsupportedNextValue is returned when sequence next value is assigned to unsupported column type.
pub static ErrColumnTypeUnsupportedNextValue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrColumnTypeUnsupportedNextValue));
// ErrAddColumnWithSequenceAsDefault is returned when the new added column with sequence's nextval as it's default value.
pub static ErrAddColumnWithSequenceAsDefault: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrAddColumnWithSequenceAsDefault));
// ErrUnsupportedExpressionIndex is returned when create an expression index without allow-expression-index.
pub static ErrUnsupportedExpressionIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(mysql::ErrUnsupportedDDLOperation, &parser_mysql::errname::Message(&format_mysql_message(&mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw, &["creating expression index containing unsafe functions without allow-expression-index in config"]), &[]))
});
// ErrPartitionExchangePartTable is returned when exchange table partition with another table is partitioned.
pub static ErrPartitionExchangePartTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionExchangePartTable));
// ErrPartitionExchangeTempTable is returned when exchange table partition with a temporary table
pub static ErrPartitionExchangeTempTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionExchangeTempTable));
// ErrTablesDifferentMetadata is returned when exchanges tables is not compatible.
pub static ErrTablesDifferentMetadata: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTablesDifferentMetadata));
// ErrRowDoesNotMatchPartition is returned when the row record of exchange table does not match the partition rule.
pub static ErrRowDoesNotMatchPartition: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrRowDoesNotMatchPartition));
// ErrPartitionExchangeForeignKey is returned when exchanged normal table has foreign keys.
pub static ErrPartitionExchangeForeignKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionExchangeForeignKey));
// ErrCheckNoSuchTable is returned when exchanged normal table is view or sequence.
pub static ErrCheckNoSuchTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckNoSuchTable));
// ErrUnsupportedPartitionType is returned when exchange table partition type is not supported.
pub static ErrUnsupportedPartitionType: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["partition type of table %s when exchanging partition"],
            ),
            &[],
        ),
    )
});
// ErrPartitionExchangeDifferentOption is returned when attribute does not match between partition table and normal table.
pub static ErrPartitionExchangeDifferentOption: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionExchangeDifferentOption));
// ErrTableOptionUnionUnsupported is returned when create/alter table with union option.
pub static ErrTableOptionUnionUnsupported: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTableOptionUnionUnsupported));
// ErrTableOptionInsertMethodUnsupported is returned when create/alter table with insert method option.
pub static ErrTableOptionInsertMethodUnsupported: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTableOptionInsertMethodUnsupported));

// ErrInvalidPlacementPolicyCheck is returned when txn_scope and commit data changing do not meet the placement policy
pub static ErrInvalidPlacementPolicyCheck: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPlacementPolicyCheck));

// ErrPlacementPolicyWithDirectOption is returned when create/alter table with both placement policy and placement options existed.
pub static ErrPlacementPolicyWithDirectOption: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPlacementPolicyWithDirectOption));

// ErrPlacementPolicyInUse is returned when placement policy is in use in drop/alter.
pub static ErrPlacementPolicyInUse: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPlacementPolicyInUse));

// ErrMultipleDefConstInListPart returns multiple definition of same constant in list partitioning.
pub static ErrMultipleDefConstInListPart: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrMultipleDefConstInListPart));

// ErrTruncatedWrongValue is returned when data has been truncated during conversion.
pub static ErrTruncatedWrongValue: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTruncatedWrongValue));

// ErrWarnDataOutOfRange is returned when the value in a numeric column that is outside the permissible range of the column data type.
// See https://dev.mysql.com/doc/refman/5.5/en/out-of-range-and-overflow.html for details
pub static ErrWarnDataOutOfRange: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWarnDataOutOfRange));

// ErrTooLongValueForType is returned when the individual enum element length is too long.
pub static ErrTooLongValueForType: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTooLongValueForType));

// ErrUnknownEngine is returned when the table engine is unknown.
pub static ErrUnknownEngine: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnknownStorageEngine));

// ErrExchangePartitionDisabled is returned when exchange partition is disabled.
pub static ErrExchangePartitionDisabled: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(mysql::ErrUnsupportedDDLOperation, &parser_mysql::errname::Message("Exchange Partition is disabled, please set 'tidb_enable_exchange_partition' if you need to need to enable it", &[]))
});

// ErrPartitionNoTemporary returns when partition at temporary mode
pub static ErrPartitionNoTemporary: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrPartitionNoTemporary));

// ErrOptOnTemporaryTable returns when exec unsupported opt at temporary mode
pub static ErrOptOnTemporaryTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrOptOnTemporaryTable));
// ErrOptOnCacheTable returns when exec unsupported opt at cache mode
pub static ErrOptOnCacheTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrOptOnCacheTable));
// ErrUnsupportedOnCommitPreserve returns when exec unsupported opt on commit preserve
pub static ErrUnsupportedOnCommitPreserve: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            "TiDB doesn't support ON COMMIT PRESERVE ROWS for now",
            &[],
        ),
    )
});
// ErrUnsupportedClusteredSecondaryKey returns when exec unsupported clustered secondary key
pub static ErrUnsupportedClusteredSecondaryKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                "CLUSTERED/NONCLUSTERED keyword is only supported for primary key",
                &[],
            ),
        )
    });

// ErrUnsupportedLocalTempTableDDL returns when ddl operation unsupported for local temporary table
pub static ErrUnsupportedLocalTempTableDDL: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message("TiDB doesn't support %s for local temporary table", &[]),
    )
});
// ErrInvalidAttributesSpec is returned when meeting invalid attributes.
pub static ErrInvalidAttributesSpec: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidAttributesSpec));
// ErrFunctionalIndexOnJSONOrGeometryFunction returns when creating expression index and the type of the expression is JSON.
pub static ErrFunctionalIndexOnJSONOrGeometryFunction: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexOnJSONOrGeometryFunction));
// ErrDependentByFunctionalIndex returns when the dropped column depends by expression index.
pub static ErrDependentByFunctionalIndex: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDependentByFunctionalIndex));
// ErrFunctionalIndexOnBlob when the expression of expression index returns blob or text.
pub static ErrFunctionalIndexOnBlob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrFunctionalIndexOnBlob));
// ErrDependentByPartitionFunctional returns when the dropped column depends by expression partition.
pub static ErrDependentByPartitionFunctional: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDependentByPartitionFunctional));

// ErrUnsupportedAlterTableSpec means we don't support this alter table specification (i.e. unknown)
pub static ErrUnsupportedAlterTableSpec: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["Unsupported/unknown ALTER TABLE specification"],
            ),
            &[],
        ),
    )
});
// ErrGeneralUnsupportedDDL as a generic error to customise by argument
pub static ErrGeneralUnsupportedDDL: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrUnsupportedDDLOperation,
        &parser_mysql::errname::Message(
            &format_mysql_message(
                &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                &["%s"],
            ),
            &[],
        ),
    )
});

// ErrAutoConvert when auto convert happens
pub static ErrAutoConvert: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrAutoConvert));
// ErrWrongStringLength when UserName or HostName is too long
pub static ErrWrongStringLength: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWrongStringLength));

// ErrBinlogUnsafeSystemFunction when use a system function that may return a different value on the slave.
pub static ErrBinlogUnsafeSystemFunction: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrBinlogUnsafeSystemFunction));

// ErrDDLJobNotFound indicates the job id was not found.
pub static ErrDDLJobNotFound: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDDLJobNotFound));
// ErrCancelFinishedDDLJob returns when cancel a finished ddl job.
pub static ErrCancelFinishedDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCancelFinishedDDLJob));
// ErrCannotCancelDDLJob returns when cancel a almost finished ddl job, because cancel in now may cause data inconsistency.
pub static ErrCannotCancelDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCannotCancelDDLJob));
// ErrCannotPauseDDLJob returns when the State is not qualified to be paused.
pub static ErrCannotPauseDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCannotPauseDDLJob));
// ErrCannotResumeDDLJob returns when the State is not qualified to be resumed.
pub static ErrCannotResumeDDLJob: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCannotResumeDDLJob));
// ErrDDLSetting returns when failing to enable/disable DDL.
pub static ErrDDLSetting: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDDLSetting));
// ErrIngestFailed returns when the DDL ingest job is failed.
pub static ErrIngestFailed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrIngestFailed));
// ErrIngestCheckEnvFailed returns when the DDL ingest env is failed to init.
pub static ErrIngestCheckEnvFailed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrIngestCheckEnvFailed));

// ErrColumnInChange indicates there is modification on the column in parallel.
pub static ErrColumnInChange: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrColumnInChange));

// ErrAlterTiFlashModeForTableWithoutTiFlashReplica returns when set tiflash mode on table whose tiflash_replica is null or tiflash_replica_count = 0
pub static ErrAlterTiFlashModeForTableWithoutTiFlashReplica: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(0u16, &parser_mysql::errname::Message("TiFlash mode will take effect after at least one TiFlash replica is set for the table", &[]))
    });
// ErrUnsupportedTiFlashOperationForSysOrMemTable means we don't support the alter tiflash related action(e.g. set tiflash mode, set tiflash replica) for system table.
pub static ErrUnsupportedTiFlashOperationForSysOrMemTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                    &["`set TiFlash replica` settings for system table and memory table"],
                ),
                &[],
            ),
        )
    });
// ErrUnsupportedTiFlashOperationForUnsupportedCharsetTable is used when alter alter tiflash related action(e.g. set tiflash mode, set tiflash replica) with unsupported charset.
pub static ErrUnsupportedTiFlashOperationForUnsupportedCharsetTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                    &["`set TiFlash replica` settings for table contains %s charset"],
                ),
                &[],
            ),
        )
    });
/// Columnar Storage 状态不可验证时拒绝设置 TiFlash 副本。
pub static ErrTiFlashColumnarStorageCheckFailed: LazyLock<Box<terror::Error>> = LazyLock::new(
    || {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                    &["`set TiFlash replica` because the Columnar Storage status of cluster %s cannot be verified, please retry later"],
                ),
                &[],
            ),
        )
    },
);
/// Columnar Storage 关闭时拒绝设置 TiFlash 副本。
pub static ErrTiFlashColumnarStorageNotEnabled: LazyLock<Box<terror::Error>> = LazyLock::new(
    || {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw,
                    &["`set TiFlash replica` because Columnar Storage is not enabled for cluster %s (tidb_columnar_storage_enabled=%q)"],
                ),
                &[],
            ),
        )
    },
);
// ErrTiFlashBackfillIndex is the error that tiflash backfill the index failed.
pub static ErrTiFlashBackfillIndex: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrTiFlashBackfillIndex,
        &parser_mysql::errname::Message(
            &mysql::MySQLErrName[&mysql::ErrTiFlashBackfillIndex].Raw,
            &[],
        ),
    )
});

// ErrDropIndexNeededInForeignKey returns when drop index which is needed in foreign key.
pub static ErrDropIndexNeededInForeignKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrDropIndexNeededInForeignKey));
// ErrForeignKeyCannotDropParent returns when drop table which has foreign key referred.
pub static ErrForeignKeyCannotDropParent: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrForeignKeyCannotDropParent));
// ErrTruncateIllegalForeignKey returns when truncate table which has foreign key referred.
pub static ErrTruncateIllegalForeignKey: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTruncateIllegalForeignKey));
// ErrForeignKeyColumnCannotChange returns when change column which used by foreign key.
pub static ErrForeignKeyColumnCannotChange: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrForeignKeyColumnCannotChange));
// ErrForeignKeyColumnCannotChangeChild returns when change child table's column which used by foreign key.
pub static ErrForeignKeyColumnCannotChangeChild: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrForeignKeyColumnCannotChangeChild));
// ErrNoReferencedRow2 returns when there are rows in child table don't have related foreign key value in refer table.
pub static ErrNoReferencedRow2: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNoReferencedRow2));

// ErrUnsupportedColumnInTTLConfig returns when a column type is not expected in TTL config
pub static ErrUnsupportedColumnInTTLConfig: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnsupportedColumnInTTLConfig));
// ErrTTLColumnCannotDrop returns when a column is dropped while referenced by TTL config
pub static ErrTTLColumnCannotDrop: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTTLColumnCannotDrop));
// ErrSetTTLOptionForNonTTLTable returns when the `TTL_ENABLE` or `TTL_JOB_INTERVAL` option is set on a non-TTL table
pub static ErrSetTTLOptionForNonTTLTable: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrSetTTLOptionForNonTTLTable));
// ErrTempTableNotAllowedWithTTL returns when setting TTL config for a temp table
pub static ErrTempTableNotAllowedWithTTL: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTempTableNotAllowedWithTTL));
// ErrUnsupportedTTLReferencedByFK returns when the TTL config is set for a table referenced by foreign key
pub static ErrUnsupportedTTLReferencedByFK: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnsupportedTTLReferencedByFK));
// ErrUnsupportedPrimaryKeyTypeWithTTL returns when create or alter a table with TTL options but the primary key is not supported
pub static ErrUnsupportedPrimaryKeyTypeWithTTL: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrUnsupportedPrimaryKeyTypeWithTTL));
/// Starter 部署模式只接受受支持的 TTL job interval。
pub static ErrUnsupportedTTLJobIntervalInStarter: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrUnsupportedDDLOperation,
            &parser_mysql::errname::Message(
                "TTL_JOB_INTERVAL other than '%s' is not supported in starter deployment mode",
                &[],
            ),
        )
    });

// ErrNotSupportedYet returns when tidb does not support this feature.
pub static ErrNotSupportedYet: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNotSupportedYet));

// ErrColumnCheckConstraintReferOther is returned when create column check constraint referring other column.
pub static ErrColumnCheckConstraintReferOther: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrColumnCheckConstraintReferencesOtherColumn));
// ErrTableCheckConstraintReferUnknown is returned when create table check constraint referring non-existing column.
pub static ErrTableCheckConstraintReferUnknown: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrTableCheckConstraintReferUnknown));
// ErrConstraintNotFound is returned for dropping a non-existent constraint.
pub static ErrConstraintNotFound: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrConstraintNotFound));
// ErrCheckConstraintIsViolated is returned for violating an existent check constraint.
pub static ErrCheckConstraintIsViolated: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintViolated));
// ErrCheckConstraintNamedFuncIsNotAllowed is returned for not allowed function with name.
pub static ErrCheckConstraintNamedFuncIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintNamedFunctionIsNotAllowed));
// ErrCheckConstraintFuncIsNotAllowed is returned for not allowed function.
pub static ErrCheckConstraintFuncIsNotAllowed: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintFunctionIsNotAllowed));
// ErrCheckConstraintVariables is returned for referring user or system variables.
pub static ErrCheckConstraintVariables: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintVariables));
// ErrCheckConstraintRefersAutoIncrementColumn is returned for referring auto-increment columns.
pub static ErrCheckConstraintRefersAutoIncrementColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintRefersAutoIncrementColumn));
// ErrCheckConstraintUsingFKReferActionColumn is returned for referring foreign key columns.
pub static ErrCheckConstraintUsingFKReferActionColumn: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintClauseUsingFKReferActionColumn));
// ErrNonBooleanExprForCheckConstraint is returned for non bool expression.
pub static ErrNonBooleanExprForCheckConstraint: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrNonBooleanExprForCheckConstraint));
// ErrWarnDeprecatedIntegerDisplayWidth share the same code 1681, and it will be returned when length is specified in integer.
pub static ErrWarnDeprecatedIntegerDisplayWidth: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| {
        ClassDDL.NewStdErr(
            mysql::ErrWarnDeprecatedSyntaxNoReplacement,
            &parser_mysql::errname::Message(
                &format_mysql_message(
                    &mysql::MySQLErrName[&mysql::ErrWarnDeprecatedSyntaxNoReplacement].Raw,
                    &["Integer display width", ""],
                ),
                &[],
            ),
        )
    });
// ErrWarnDeprecatedZerofill is for when the deprectated zerofill attribute is used
pub static ErrWarnDeprecatedZerofill: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr( mysql::ErrWarnDeprecatedSyntaxNoReplacement, &parser_mysql::errname::Message(&format_mysql_message(&mysql::MySQLErrName[&mysql::ErrWarnDeprecatedSyntaxNoReplacement].Raw, &["The ZEROFILL attribute", " Use the LPAD function to zero-pad numbers, or store the formatted numbers in a CHAR column.", ]), &[]), )
});
// ErrCheckConstraintDupName is for duplicate check constraint names
pub static ErrCheckConstraintDupName: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrCheckConstraintDupName));
// ErrUnsupportedDistTask is for `tidb_enable_dist_task enabled` but `tidb_ddl_enable_fast_reorg` disabled.
pub static ErrUnsupportedDistTask: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(mysql::ErrUnsupportedDDLOperation, &parser_mysql::errname::Message(&format_mysql_message(&mysql::MySQLErrName[&mysql::ErrUnsupportedDDLOperation].Raw, &["tidb_enable_dist_task setting. To utilize distributed task execution, please enable tidb_ddl_enable_fast_reorg first."]), &[]))
});
// ErrGlobalIndexNotExplicitlySet is for Global index when not explicitly said GLOBAL, including UPDATE INDEXES
pub static ErrGlobalIndexNotExplicitlySet: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrGlobalIndexNotExplicitlySet));
// ErrWarnGlobalIndexNeedManuallyAnalyze is used for global indexes,
// which cannot trigger automatic analysis when it contains prefix columns or virtual generated columns.
pub static ErrWarnGlobalIndexNeedManuallyAnalyze: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrWarnGlobalIndexNeedManuallyAnalyze));
// ErrEngineAttributeInvalidFormat is returned when meeting invalid format of engine attribute.
pub static ErrEngineAttributeInvalidFormat: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrEngineAttributeInvalidFormat));
// ErrStorageClassInvalidSpec is reserved for future use.
pub static ErrStorageClassInvalidSpec: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrStorageClassInvalidSpec));
// ErrCannotSetAffinityOnTable is returned when set an invalid affinity on a table
pub static ErrCannotSetAffinityOnTable: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    ClassDDL.NewStdErr(
        mysql::ErrInvalidAffinityOption,
        &parser_mysql::errname::Message("Can not set %s on a %s.", &[]),
    )
});
// ErrInvalidTableAffinity is returned when set an invalid affinity value on a table
pub static ErrInvalidTableAffinity: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrInvalidAffinityOption));
// ErrForbiddenDDL is returned when a DDL operation is forbidden.
pub static ErrForbiddenDDL: LazyLock<Box<terror::Error>> =
    LazyLock::new(|| ClassDDL.NewStd(mysql::ErrForbiddenDDL));

// Go 会在包初始化阶段按 var 块顺序构造全部错误。集中保留同一顺序，供 crate
// 初始化入口在 `terror::RegisterFinish` 前一次性 force，避免冻结后首次访问才注册。
pub(crate) static DDL_ERRORS: [&LazyLock<Box<terror::Error>>; 231] = [
    &ErrInvalidWorker,
    &ErrNotOwner,
    &ErrCantDecodeRecord,
    &ErrInvalidDDLJob,
    &ErrCancelledDDLJob,
    &ErrPausedDDLJob,
    &ErrBDRRestrictedDDL,
    &ErrDDLAutoPausedByKVDiskFull,
    &ErrRunMultiSchemaChanges,
    &ErrOperateSameColumn,
    &ErrOperateSameIndex,
    &ErrWaitReorgTimeout,
    &ErrInvalidStoreVer,
    &ErrRepairTableFail,
    &ErrUnsupportedAddColumnarIndex,
    &ErrUnsupportedAddVectorIndex,
    &ErrCantDropColWithIndex,
    &ErrCantDropColWithAutoInc,
    &ErrCantDropColWithCheckConstraint,
    &ErrUnsupportedEngineAttribute,
    &ErrUnsupportedAddColumn,
    &ErrUnsupportedModifyColumn,
    &ErrUnsupportedModifyCharset,
    &ErrUnsupportedModifyCollation,
    &ErrUnsupportedPKHandle,
    &ErrUnsupportedCharset,
    &ErrUnsupportedShardRowIDBits,
    &ErrUnsupportedAlterTableWithValidation,
    &ErrUnsupportedAlterTableWithoutValidation,
    &ErrUnsupportedAlterTableOption,
    &ErrUnsupportedAlterCacheForSysTable,
    &ErrUnsupportedAddPartialIndex,
    &ErrModifyColumnReferencedByPartialCondition,
    &ErrBlobKeyWithoutLength,
    &ErrKeyPart0,
    &ErrIncorrectPrefixKey,
    &ErrTooLongKey,
    &ErrKeyColumnDoesNotExits,
    &ErrInvalidDDLJobVersion,
    &ErrInvalidUseOfNull,
    &ErrTooManyFields,
    &ErrTooManyKeys,
    &ErrInvalidSplitRegionRanges,
    &ErrReorgPanic,
    &ErrFkColumnCannotDrop,
    &ErrFkColumnCannotDropChild,
    &ErrFKIncompatibleColumns,
    &ErrOnlyOnRangeListPartition,
    &ErrWrongKeyColumn,
    &ErrWrongKeyColumnFunctionalIndex,
    &ErrWrongFKOptionForGeneratedColumn,
    &ErrUnsupportedOnGeneratedColumn,
    &ErrGeneratedColumnNonPrior,
    &ErrDependentByGeneratedColumn,
    &ErrJSONUsedAsKey,
    &ErrBlobCantHaveDefault,
    &ErrTooLongIndexComment,
    &ErrTooLongTableComment,
    &ErrTooLongFieldComment,
    &ErrTooLongTablePartitionComment,
    &ErrInvalidDefaultValue,
    &ErrDefValGeneratedNamedFunctionIsNotAllowed,
    &ErrGeneratedColumnRefAutoInc,
    &ErrExpressionIndexCanNotRefer,
    &ErrUnsupportedAddPartition,
    &ErrUnsupportedCoalescePartition,
    &ErrUnsupportedReorganizePartition,
    &ErrUnsupportedCheckPartition,
    &ErrUnsupportedOptimizePartition,
    &ErrUnsupportedRebuildPartition,
    &ErrUnsupportedRemovePartition,
    &ErrUnsupportedRepairPartition,
    &ErrGeneratedColumnFunctionIsNotAllowed,
    &ErrGeneratedColumnRowValueIsNotAllowed,
    &ErrUnsupportedPartitionByRangeColumns,
    &ErrFunctionalIndexFunctionIsNotAllowed,
    &ErrFunctionalIndexRowValueIsNotAllowed,
    &ErrUnsupportedCreatePartition,
    &ErrUnsupportedIndexType,
    &ErrWindowInvalidWindowFuncUse,
    &ErrDupKeyName,
    &ErrFkDupName,
    &ErrInvalidDDLState,
    &ErrUnsupportedModifyPrimaryKey,
    &ErrPKIndexCantBeInvisible,
    &ErrColumnBadNull,
    &ErrBadField,
    &ErrCantRemoveAllFields,
    &ErrCantDropFieldOrKey,
    &ErrInvalidOnUpdate,
    &ErrTooLongIdent,
    &ErrWrongDBName,
    &ErrWrongTableName,
    &ErrWrongColumnName,
    &ErrWrongPartitionName,
    &ErrWrongUsage,
    &ErrInvalidGroupFuncUse,
    &ErrTableMustHaveColumns,
    &ErrWrongNameForIndex,
    &ErrUnknownCharacterSet,
    &ErrUnknownCollation,
    &ErrCollationCharsetMismatch,
    &ErrConflictingDeclarations,
    &ErrPrimaryCantHaveNull,
    &ErrErrorOnRename,
    &ErrViewSelectClause,
    &ErrViewSelectVariable,
    &ErrNotAllowedTypeInPartition,
    &ErrPartitionMgmtOnNonpartitioned,
    &ErrDropPartitionNonExistent,
    &ErrSameNamePartition,
    &ErrSameNamePartitionField,
    &ErrRangeNotIncreasing,
    &ErrPartitionMaxvalue,
    &ErrMaxvalueInValuesIn,
    &ErrDropLastPartition,
    &ErrTooManyPartitions,
    &ErrPartitionConstDomain,
    &ErrPartitionFunctionIsNotAllowed,
    &ErrPartitionFuncNotAllowed,
    &ErrUniqueKeyNeedAllFieldsInPf,
    &ErrWrongExprInPartitionFunc,
    &ErrWarnDataTruncated,
    &ErrCoalesceOnlyOnHashPartition,
    &ErrViewWrongList,
    &ErrAlterOperationNotSupported,
    &ErrWrongObject,
    &ErrTableCantHandleFt,
    &ErrFieldNotFoundPart,
    &ErrWrongTypeColumnValue,
    &ErrValuesIsNotIntType,
    &ErrFunctionalIndexPrimaryKey,
    &ErrFunctionalIndexOnField,
    &ErrInvalidAutoRandom,
    &ErrUnsupportedConstraintCheck,
    &ErrDerivedMustHaveAlias,
    &ErrNullInValuesLessThan,
    &ErrSequenceRunOut,
    &ErrSequenceInvalidData,
    &ErrSequenceAccessFail,
    &ErrNotSequence,
    &ErrUnknownSequence,
    &ErrSequenceUnsupportedTableOption,
    &ErrColumnTypeUnsupportedNextValue,
    &ErrAddColumnWithSequenceAsDefault,
    &ErrUnsupportedExpressionIndex,
    &ErrPartitionExchangePartTable,
    &ErrPartitionExchangeTempTable,
    &ErrTablesDifferentMetadata,
    &ErrRowDoesNotMatchPartition,
    &ErrPartitionExchangeForeignKey,
    &ErrCheckNoSuchTable,
    &ErrUnsupportedPartitionType,
    &ErrPartitionExchangeDifferentOption,
    &ErrTableOptionUnionUnsupported,
    &ErrTableOptionInsertMethodUnsupported,
    &ErrInvalidPlacementPolicyCheck,
    &ErrPlacementPolicyWithDirectOption,
    &ErrPlacementPolicyInUse,
    &ErrMultipleDefConstInListPart,
    &ErrTruncatedWrongValue,
    &ErrWarnDataOutOfRange,
    &ErrTooLongValueForType,
    &ErrUnknownEngine,
    &ErrExchangePartitionDisabled,
    &ErrPartitionNoTemporary,
    &ErrOptOnTemporaryTable,
    &ErrOptOnCacheTable,
    &ErrUnsupportedOnCommitPreserve,
    &ErrUnsupportedClusteredSecondaryKey,
    &ErrUnsupportedLocalTempTableDDL,
    &ErrInvalidAttributesSpec,
    &ErrFunctionalIndexOnJSONOrGeometryFunction,
    &ErrDependentByFunctionalIndex,
    &ErrFunctionalIndexOnBlob,
    &ErrDependentByPartitionFunctional,
    &ErrUnsupportedAlterTableSpec,
    &ErrGeneralUnsupportedDDL,
    &ErrAutoConvert,
    &ErrWrongStringLength,
    &ErrBinlogUnsafeSystemFunction,
    &ErrDDLJobNotFound,
    &ErrCancelFinishedDDLJob,
    &ErrCannotCancelDDLJob,
    &ErrCannotPauseDDLJob,
    &ErrCannotResumeDDLJob,
    &ErrDDLSetting,
    &ErrIngestFailed,
    &ErrIngestCheckEnvFailed,
    &ErrColumnInChange,
    &ErrAlterTiFlashModeForTableWithoutTiFlashReplica,
    &ErrUnsupportedTiFlashOperationForSysOrMemTable,
    &ErrUnsupportedTiFlashOperationForUnsupportedCharsetTable,
    &ErrTiFlashColumnarStorageCheckFailed,
    &ErrTiFlashColumnarStorageNotEnabled,
    &ErrTiFlashBackfillIndex,
    &ErrDropIndexNeededInForeignKey,
    &ErrForeignKeyCannotDropParent,
    &ErrTruncateIllegalForeignKey,
    &ErrForeignKeyColumnCannotChange,
    &ErrForeignKeyColumnCannotChangeChild,
    &ErrNoReferencedRow2,
    &ErrUnsupportedColumnInTTLConfig,
    &ErrTTLColumnCannotDrop,
    &ErrSetTTLOptionForNonTTLTable,
    &ErrTempTableNotAllowedWithTTL,
    &ErrUnsupportedTTLReferencedByFK,
    &ErrUnsupportedPrimaryKeyTypeWithTTL,
    &ErrUnsupportedTTLJobIntervalInStarter,
    &ErrNotSupportedYet,
    &ErrColumnCheckConstraintReferOther,
    &ErrTableCheckConstraintReferUnknown,
    &ErrConstraintNotFound,
    &ErrCheckConstraintIsViolated,
    &ErrCheckConstraintNamedFuncIsNotAllowed,
    &ErrCheckConstraintFuncIsNotAllowed,
    &ErrCheckConstraintVariables,
    &ErrCheckConstraintRefersAutoIncrementColumn,
    &ErrCheckConstraintUsingFKReferActionColumn,
    &ErrNonBooleanExprForCheckConstraint,
    &ErrWarnDeprecatedIntegerDisplayWidth,
    &ErrWarnDeprecatedZerofill,
    &ErrCheckConstraintDupName,
    &ErrUnsupportedDistTask,
    &ErrGlobalIndexNotExplicitlySet,
    &ErrWarnGlobalIndexNeedManuallyAnalyze,
    &ErrEngineAttributeInvalidFormat,
    &ErrStorageClassInvalidSpec,
    &ErrCannotSetAffinityOnTable,
    &ErrInvalidTableAffinity,
    &ErrForbiddenDDL,
];

pub(crate) fn initialize_ddl_errors() {
    for error in DDL_ERRORS {
        LazyLock::force(error);
    }
}

// ReorgRetryableErrCodes are the error codes that are retryable for reorganization.
// ReorgRetryableErrCodes 对应 Go 的 map[uint16]struct{}，用 HashSet<u16> 表达只关心 key 的集合语义。
/// reorganization 可重试错误码集合；命中则 DDL reorg 可安全重试。
pub static ReorgRetryableErrCodes: LazyLock<HashSet<u16>> = LazyLock::new(|| {
    HashSet::from([
        mysql::ErrPDServerTimeout as u16,
        mysql::ErrTiKVServerTimeout as u16,
        mysql::ErrTiKVServerBusy as u16,
        mysql::ErrResolveLockTimeout as u16,
        mysql::ErrRegionUnavailable as u16,
        mysql::ErrTxnAbortedByGC as u16,
        mysql::ErrWriteConflict as u16,
        mysql::ErrTiKVStoreLimit as u16,
        mysql::ErrTiKVStaleCommand as u16,
        mysql::ErrTiKVMaxTimestampNotSynced as u16,
        mysql::ErrTiFlashServerTimeout as u16,
        mysql::ErrTiFlashServerBusy as u16,
        mysql::ErrInfoSchemaExpired as u16,
        mysql::ErrInfoSchemaChanged as u16,
        mysql::ErrWriteConflictInTiDB as u16,
        mysql::ErrTxnRetryable as u16,
        mysql::ErrNotOwner as u16,
        // PD client returns regions with no leader.
        // Go map value 是空 struct；这里只保留可重试错误码本身。
        mysql::ErrInvalidSplitRegionRanges as u16,
        // Temporary network partitioning may cause pk commit failure.
        terror::CodeResultUndetermined.0 as u16,
    ])
});

// ReorgRetryableErrMsgs are the error messages that are retryable for reorganization.
// ReorgRetryableErrMsgs 保留 Go 字符串切片顺序，用于按错误消息判断 reorg 是否可重试。
/// reorganization 可重试错误消息原文列表（按子串/全文匹配）。
/// 含 MVCC（多版本并发控制）revision 被压缩等瞬时故障文案。
pub static ReorgRetryableErrMsgs: &[&str] = &[
    "context deadline exceeded",
    "requested lease not found",
    "mvcc: required revision has been compacted",
    "All returned regions have no leaders",
];
