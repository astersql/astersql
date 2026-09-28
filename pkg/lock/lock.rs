// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 表锁检查器：对照会话锁上下文与 InfoSchema 快照执行权限规则。
//
// 对应 Go `pkg/lock`：在 DDL/DML 前判断表是否被本会话或其它会话锁定，
// 以及当前权限（Privilege）是否满足锁类型要求。InfoSchema 为信息系统
// 元数据快照；表锁元数据挂在表定义的 `Lock` 字段上。

use std::sync::LazyLock;

use astersql_infoschema as infoschema;
use astersql_infoschema_context as infoschema_context;
use astersql_lock_context as lock_context;
use astersql_meta_metadef as metadef;
use astersql_parser_mysql::privs as mysql;
use astersql_table as table;
use astersql_util_dbterror as dbterror;

/// ClassSchema 标准错误的装箱别名，与 Go terror.Error 原型对应。
type StandardError = Box<dbterror::terror::Error>;

/// 将 AST 侧 `TableLockType` 转为模型层 `ModelTableLockType`，便于与 meta.Lock.Tp 比较。
fn modelLockType(
    lock_type: lock_context::ast::TableLockType,
) -> lock_context::model::ModelTableLockType {
    lock_context::model::ModelTableLockType(lock_type.0)
}

// InfoSchema's public Rust surface currently exposes its shared error type but not the
// package error prototypes. Build the same ClassSchema prototypes here so codes, RFC IDs,
// redaction positions, and formatted messages remain identical to the Go package values.
// InfoSchema 尚未导出包级错误原型，故在此按 Go 包值构造同等 ClassSchema 原型。
static ERR_TABLE_NOT_LOCKED_FOR_WRITE: LazyLock<StandardError> =
    LazyLock::new(|| dbterror::ClassSchema.NewStd(dbterror::errno::ErrTableNotLockedForWrite));
static ERR_TABLE_NOT_LOCKED: LazyLock<StandardError> =
    LazyLock::new(|| dbterror::ClassSchema.NewStd(dbterror::errno::ErrTableNotLocked));
static ERR_TABLE_NOT_EXISTS: LazyLock<StandardError> =
    LazyLock::new(|| dbterror::ClassSchema.NewStd(dbterror::errno::ErrNoSuchTable));
static ERR_TABLE_LOCKED: LazyLock<StandardError> =
    LazyLock::new(|| dbterror::ClassSchema.NewStd(dbterror::errno::ErrTableLocked));

/// Checker checks table locks against the current session lock context and InfoSchema snapshot.
/// 表锁检查器：持有只读会话锁上下文与 InfoSchema 引用。
pub struct Checker<'a> {
    /// 当前会话的表锁只读视图。
    ctx: &'a dyn lock_context::TableLockReadContext,
    /// 当前事务可见的信息系统快照。
    is: &'a dyn infoschema::InfoSchema,
}

/// Error returned when dropping a table write-locked by the current session.
/// 本会话写锁住的表被 DROP 时返回的特殊错误。
pub static ErrLockedTableDropped: LazyLock<infoschema::Error> = LazyLock::new(|| {
    dbterror::errors::New("other table can be accessed after locked table dropped")
});

/// Creates a table-lock checker over the supplied read-only session and schema state.
/// 基于只读会话锁上下文与 InfoSchema 构造检查器。
pub fn NewChecker<'a>(
    ctx: &'a dyn lock_context::TableLockReadContext,
    is: &'a dyn infoschema::InfoSchema,
) -> Checker<'a> {
    Checker { ctx, is }
}

impl Checker<'_> {
    /// Checks one database/table operation against TiDB's table-lock rules.
    /// 按 TiDB 表锁规则检查单次库/表操作是否允许。
    ///
    /// `alter_writeable` 为真时表示 ALTER 等可写场景下对 ReadOnly 锁的特殊放行。
    pub fn CheckTableLock(
        &self,
        db: &str,
        table_name: &str,
        privilege: mysql::PrivilegeType,
        alter_writeable: bool,
    ) -> Result<(), infoschema::Error> {
        // 空目标或 LOCK TABLES 权限本身不做检查。
        if (db.is_empty() && table_name.is_empty()) || privilege == mysql::LockTablesPriv {
            return Ok(());
        }
        // System and memory databases do not support table locks.
        // 系统库与内存库不支持表锁，直接放行。
        if metadef::IsMemOrSysDB(db) {
            return Ok(());
        }
        // Check an operation on the whole database.
        // 表名为空表示针对整个库的操作。
        if !alter_writeable && table_name.is_empty() {
            return self.CheckLockInDB(db, privilege);
        }

        if privilege == mysql::ShowDBPriv || privilege == mysql::AllPrivMask {
            // AllPrivMask is currently used only by SHOW CREATE TABLE.
            // AllPrivMask 目前仅用于 SHOW CREATE TABLE。
            return Ok(());
        }
        if privilege == mysql::CreatePriv || privilege == mysql::CreateViewPriv {
            if self.ctx.HasLockedTables() {
                // MySQL checks an existing target first, while TiDB reports the lock error first.
                // TiDB 优先报「表未加锁」错误，与 MySQL 先查目标是否存在不同。
                return Err(ERR_TABLE_NOT_LOCKED.GenWithStackByArgs(&[table_name.into()]));
            }
            return Ok(());
        }

        // Keep the Go lookup here even though it is a relatively expensive metadata operation.
        // 按库名+表名查元数据；代价较高但需与 Go 路径一致。
        let database_name = infoschema::CiString::new(db);
        let table_name_ci = infoschema::CiString::new(table_name);
        let meta = match self.is.ModelTableInfoByName(&database_name, &table_name_ci) {
            Ok(meta) => meta,
            // Ignore a missing target for DROP TABLE IF EXISTS.
            // DROP TABLE IF EXISTS 时目标不存在则忽略。
            Err(err) if err.code == "ErrNoSuchTable" || err.code == "ErrTableNotExists" => {
                return Ok(());
            }
            Err(err) => return Err(err.into_shared()),
        };

        // 表上未挂锁元数据则无需继续检查。
        let Some(lock) = meta.Lock.as_ref() else {
            return Ok(());
        };

        // DROP 且本会话已持锁：写锁禁止删；读/本地写/只读则要求写锁权限。
        if privilege == mysql::DropPriv && meta.Name.O == table_name && self.ctx.HasLockedTables() {
            for locked_table in self.ctx.GetAllTableLocks() {
                if locked_table.TableID != meta.ID {
                    continue;
                }
                if lock.Tp == modelLockType(lock_context::ast::TableLockWrite) {
                    return Err((*ErrLockedTableDropped).clone());
                }
                if lock.Tp == modelLockType(lock_context::ast::TableLockRead)
                    || lock.Tp == modelLockType(lock_context::ast::TableLockWriteLocal)
                    || lock.Tp == modelLockType(lock_context::ast::TableLockReadOnly)
                {
                    return Err(ERR_TABLE_NOT_LOCKED_FOR_WRITE.GenWithStackByArgs(&[meta
                        .Name
                        .O
                        .clone()
                        .into()]));
                }
            }
        }

        // 本会话已持有任意表锁：只能操作自己已锁且权限匹配的表。
        if !alter_writeable && self.ctx.HasLockedTables() {
            let (locked, lock_type) = self.ctx.CheckTableLocked(meta.ID);
            if locked {
                if checkLockTpMeetPrivilege(lock_type, privilege) {
                    return Ok(());
                }
                return Err(ERR_TABLE_NOT_LOCKED_FOR_WRITE.GenWithStackByArgs(&[meta
                    .Name
                    .O
                    .clone()
                    .into()]));
            }
            return Err(ERR_TABLE_NOT_LOCKED.GenWithStackByArgs(&[meta.Name.O.clone().into()]));
        }

        // 他会话持锁时：部分只读场景仍允许 SELECT；否则报「表已被锁定」。
        if privilege == mysql::SelectPriv
            && (lock.Tp == modelLockType(lock_context::ast::TableLockRead)
                || lock.Tp == modelLockType(lock_context::ast::TableLockWriteLocal)
                || lock.Tp == modelLockType(lock_context::ast::TableLockReadOnly))
        {
            return Ok(());
        }
        if alter_writeable && lock.Tp == modelLockType(lock_context::ast::TableLockReadOnly) {
            return Ok(());
        }

        let session = lock
            .Sessions
            .first()
            .expect("a locked table must record at least one session");
        Err(ERR_TABLE_LOCKED.GenWithStackByArgs(&[
            meta.Name.L.clone().into(),
            lock_context::ast::TableLockType(lock.Tp.0)
                .to_string()
                .into(),
            session.String().into(),
        ]))
    }

    /// Checks an operation that targets a database rather than a named table.
    /// 检查针对整个数据库（无具体表名）的操作。
    pub fn CheckLockInDB(
        &self,
        db: &str,
        privilege: mysql::PrivilegeType,
    ) -> Result<(), infoschema::Error> {
        // 会话已持锁时禁止库级 CREATE/DROP/ALTER。
        if self.ctx.HasLockedTables()
            && (privilege == mysql::CreatePriv
                || privilege == mysql::DropPriv
                || privilege == mysql::AlterPriv)
        {
            return Err(table::ErrLockOrActiveTransaction.GenWithStackByArgs(&[]));
        }
        if privilege == mysql::CreatePriv {
            return Ok(());
        }

        // 遍历带表锁属性的表，逐表复用 CheckTableLock。
        for schema in self
            .is
            .ListTablesWithSpecialAttribute(infoschema_context::TableLockAttribute)
        {
            for table_info in schema.TableInfos {
                self.CheckTableLock(db, &table_info.Name.L, privilege, false)?;
            }
        }
        Ok(())
    }
}

/// Reports whether a session-owned lock permits the requested privilege.
/// 判断会话已持有的锁类型是否允许给定权限。
pub(crate) fn checkLockTpMeetPrivilege(
    lock_type: lock_context::ast::TableLockType,
    privilege: mysql::PrivilegeType,
) -> bool {
    // TableLockReadOnly is session-independent and is handled by CheckTableLock.
    // TableLockReadOnly 与会话无关，由 CheckTableLock 另行处理。
    if lock_type == lock_context::ast::TableLockWrite
        || lock_type == lock_context::ast::TableLockWriteLocal
    {
        return true;
    }
    if lock_type == lock_context::ast::TableLockRead {
        // SHOW/ALL/CREATE/CREATE VIEW have already returned before this check.
        // SHOW/ALL/CREATE 等已在更早分支返回，此处读锁仅允许 SELECT。
        return privilege == mysql::SelectPriv;
    }
    false
}
