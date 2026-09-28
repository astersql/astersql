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

// 表锁（Table Lock）元数据与状态推进逻辑。
//
// 对应 MySQL 风格的 `LOCK TABLES` / `UNLOCK TABLES`：会话可对表申请
// 读锁、只读锁、写锁或本地写锁。分布式环境下锁状态按
// `None -> PreLock -> Public` 两阶段推进（先预锁再公开），每步递增
// schema 版本，保证集群内各节点看到的锁信息至多相差一个版本。
//
// 主要内容：
// - [`SessionInfo`] / [`TableLockType`] / [`TableLockState`]：会话与锁类型、状态；
// - [`TableLockInfo`] / [`LockTableInfo`] / [`LockTablesArgs`]：锁元数据与批处理参数；
// - [`check_rename_table_lock`] / [`check_drop_schema_lock`]：RENAME / DROP SCHEMA 前的锁校验；
// - [`advance_lock_tables`] / [`advance_unlock_tables`]：按索引逐步推进加锁/解锁。

use std::collections::BTreeMap;

/// 持有表锁的会话标识：由 TiDB 实例 ID 与连接 ID 共同唯一确定。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionInfo {
    /// TiDB 实例（server）唯一标识。
    pub server_id: String,
    /// 会话连接 ID（ConnectionID）。
    pub session_id: u64,
}

/// 表锁类型，对应 `LOCK TABLES ... READ/WRITE/WRITE LOCAL` 与 `ALTER TABLE ... READ ONLY`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableLockType {
    /// 共享读锁：允许多会话同时持有。
    Read,
    /// 表只读模式（read only）：禁止写入与其它锁表操作。
    ReadOnly,
    /// 排他写锁：集群内仅一个会话可持有。
    Write,
    /// 本地写锁：语义近似写锁，主要用于同实例内互斥。
    WriteLocal,
}

/// 表锁在在线 schema 变更协议中的可见性阶段。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableLockState {
    /// 尚未进入加锁流程，或刚写入元数据。
    #[default]
    None,
    /// 预锁阶段：锁已记录但尚未对所有节点完全公开。
    PreLock,
    /// 公开阶段：锁对集群内所有会话生效。
    Public,
}

/// 单张表上的锁元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableLockInfo {
    /// 当前锁类型。
    pub lock_type: TableLockType,
    /// 持有该锁的会话列表（共享读锁可有多个）。
    pub sessions: Vec<SessionInfo>,
    /// 锁的 schema 可见性状态。
    pub state: TableLockState,
    /// 加锁时间戳，用于死锁/超时检测等。
    pub timestamp: u64,
}

/// 参与锁表操作的表描述（含可选的当前锁信息）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockTableInfo {
    /// 所属 schema（数据库）ID。
    pub schema_id: i64,
    /// 表 ID。
    pub table_id: i64,
    /// 表名（用于错误消息）。
    pub table_name: String,
    /// 当前锁信息；`None` 表示未加锁。
    pub lock: Option<TableLockInfo>,
}

/// 一次加锁/解锁请求中的单个表目标。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableLockTarget {
    /// 目标 schema ID。
    pub schema_id: i64,
    /// 目标表 ID。
    pub table_id: i64,
    /// 请求的锁类型。
    pub lock_type: TableLockType,
}

/// `LOCK TABLES` / `UNLOCK TABLES` DDL job 的参数。
///
/// `index_of_lock` / `index_of_unlock` 记录批处理进度，每次
/// [`advance_lock_tables`] / [`advance_unlock_tables`] 只处理一项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockTablesArgs {
    /// 待加锁的表列表。
    pub lock_tables: Vec<TableLockTarget>,
    /// 待解锁的表列表。
    pub unlock_tables: Vec<TableLockTarget>,
    /// 发起操作的会话。
    pub session: SessionInfo,
    /// 当前加锁进度下标。
    pub index_of_lock: usize,
    /// 当前解锁进度下标。
    pub index_of_unlock: usize,
    /// 为真时表示 `ADMIN CLEANUP TABLE LOCK`，直接清除整表锁元数据。
    pub cleanup: bool,
}

/// 表锁相关操作可能返回的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TableLockError {
    /// 目标表在元数据中不存在。
    TableNotFound(i64),
    /// 当前会话虽持有锁，但不是 WRITE 锁（如 RENAME 要求）。
    TableNotLockedForWrite(String),
    /// 会话仍持有任意表锁，禁止 DROP SCHEMA 等操作。
    LockOrActiveTransaction,
    /// 表已被其它会话/不兼容锁类型占用。
    TableLocked {
        table_name: String,
        lock_type: TableLockType,
        owner: SessionInfo,
    },
    /// 锁状态与期望不符。
    InvalidState(TableLockState),
}

/// Validate the lock permission used by RENAME TABLE.
///
/// An unlocked table can be renamed. Once this session has explicitly locked
/// it, Go requires a public WRITE lock; a READ lock owned by the same session
/// is intentionally not upgraded for the rename operation.
///
/// 校验 RENAME TABLE 所需的锁权限：未加锁可直接重命名；
/// 若本会话已持锁，则必须是 Public 的 WRITE 锁，READ 锁不会自动升级。
pub fn check_rename_table_lock(
    table: &LockTableInfo,
    session: &SessionInfo,
) -> Result<(), TableLockError> {
    // 未加锁的表允许重命名。
    let Some(lock) = &table.lock else {
        return Ok(());
    };
    // 锁必须已推进到 Public，否则其它节点可能尚未看到完整锁信息。
    if lock.state != TableLockState::Public {
        return Err(TableLockError::InvalidState(lock.state));
    }
    // 本会话必须是持锁者之一。
    if find_session_info_index(&lock.sessions, session).is_none() {
        return Err(TableLockError::TableLocked {
            table_name: table.table_name.clone(),
            lock_type: lock.lock_type,
            owner: lock
                .sessions
                .first()
                .cloned()
                .unwrap_or_else(|| session.clone()),
        });
    }
    // RENAME 要求 WRITE 锁；同会话的 READ 锁不足以放行。
    if lock.lock_type != TableLockType::Write {
        return Err(TableLockError::TableNotLockedForWrite(
            table.table_name.clone(),
        ));
    }
    Ok(())
}

/// DROP SCHEMA is rejected while the requesting session owns any table lock,
/// even when that lock belongs to another schema.
///
/// 请求会话若仍持有任意表锁（即使属于其它 schema），则拒绝 DROP SCHEMA。
pub fn check_drop_schema_lock(
    tables: &[LockTableInfo],
    session: &SessionInfo,
) -> Result<(), TableLockError> {
    if tables.iter().any(|table| {
        table
            .lock
            .as_ref()
            .is_some_and(|lock| find_session_info_index(&lock.sessions, session).is_some())
    }) {
        return Err(TableLockError::LockOrActiveTransaction);
    }
    Ok(())
}

/// 在会话列表中查找与给定会话匹配的下标（按 server_id + session_id）。
pub fn find_session_info_index(sessions: &[SessionInfo], session: &SessionInfo) -> Option<usize> {
    sessions.iter().position(|candidate| {
        candidate.server_id == session.server_id && candidate.session_id == session.session_id
    })
}

/// 判断已有锁类型与请求类型是否可共享（仅 Read↔Read、ReadOnly↔ReadOnly）。
fn lock_types_are_shareable(existing: TableLockType, requested: TableLockType) -> bool {
    matches!(
        (existing, requested),
        (TableLockType::Read, TableLockType::Read)
            | (TableLockType::ReadOnly, TableLockType::ReadOnly)
    )
}

/// 检查表当前锁是否与请求的锁类型冲突。
///
/// PreLock 阶段或可共享锁类型直接放行；持锁会话可重复申请同类型锁，
/// 或在唯一持锁者且非 ReadOnly 时允许切换类型。
pub fn check_table_locked(
    table: &LockTableInfo,
    requested: TableLockType,
    session: &SessionInfo,
) -> Result<(), TableLockError> {
    let Some(lock) = &table.lock else {
        return Ok(());
    };
    // PreLock 尚未对全局生效，或同类型共享读/只读锁，均允许继续。
    if lock.state == TableLockState::PreLock || lock_types_are_shareable(lock.lock_type, requested)
    {
        return Ok(());
    }
    // 本会话已在持锁列表中时的兼容规则。
    if find_session_info_index(&lock.sessions, session).is_some() {
        if lock.lock_type == requested
            || (lock.sessions.len() == 1 && lock.lock_type != TableLockType::ReadOnly)
        {
            return Ok(());
        }
    }
    Err(TableLockError::TableLocked {
        table_name: table.table_name.clone(),
        lock_type: lock.lock_type,
        owner: lock
            .sessions
            .first()
            .cloned()
            .unwrap_or_else(|| session.clone()),
    })
}

/// 为表写入或追加锁元数据（不含状态推进；状态由 [`advance_lock_tables`] 负责）。
pub fn lock_table(
    table: &mut LockTableInfo,
    requested: TableLockType,
    session: &SessionInfo,
) -> Result<(), TableLockError> {
    // 无锁时创建初始锁记录，状态设为 None，等待后续推进到 PreLock/Public。
    let Some(lock) = &mut table.lock else {
        table.lock = Some(TableLockInfo {
            lock_type: requested,
            sessions: vec![session.clone()],
            state: TableLockState::None,
            timestamp: 0,
        });
        return Ok(());
    };
    // PreLock 阶段幂等返回，避免重复追加会话。
    if lock.state == TableLockState::PreLock {
        return Ok(());
    }
    // 可共享锁：将本会话加入持锁列表（若尚未在内）。
    if lock_types_are_shareable(lock.lock_type, requested) {
        if find_session_info_index(&lock.sessions, session).is_none() {
            lock.sessions.push(session.clone());
        }
        return Ok(());
    }
    Err(TableLockError::TableLocked {
        table_name: table.table_name.clone(),
        lock_type: lock.lock_type,
        owner: lock
            .sessions
            .first()
            .cloned()
            .unwrap_or_else(|| session.clone()),
    })
}

/// 解除本会话在表上的锁；`cleanup` 为真时清除整表锁。
///
/// 返回是否实际修改了锁元数据。
pub fn unlock_table(table: &mut LockTableInfo, args: &LockTablesArgs) -> bool {
    let Some(lock) = &mut table.lock else {
        return false;
    };
    // ADMIN CLEANUP：无视会话列表，直接清空锁。
    if args.cleanup {
        table.lock = None;
        return true;
    }
    let Some(index) = find_session_info_index(&lock.sessions, &args.session) else {
        return false;
    };
    lock.sessions.remove(index);
    // 无剩余持锁会话时删除整段锁元数据。
    if lock.sessions.is_empty() {
        table.lock = None;
    }
    true
}

/// 单次锁表/解锁状态推进后的结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableLockOutcome {
    /// 推进后的 schema 版本号。
    pub schema_version: i64,
    /// 批处理是否已全部完成。
    pub finished: bool,
}

/// 推进一次 `LOCK TABLES` 批处理：先完成待解锁项，再逐表加锁并推进状态。
///
/// 首次进入加锁阶段（`index_of_lock == 0`）时会预检全部目标表的锁冲突；
/// 每张表经历 `None -> PreLock -> Public` 两步，每步递增 schema 版本。
pub fn advance_lock_tables(
    tables: &mut BTreeMap<i64, LockTableInfo>,
    args: &mut LockTablesArgs,
    start_timestamp: u64,
    schema_version: &mut i64,
) -> Result<TableLockOutcome, TableLockError> {
    // 优先处理批次中尚未完成的解锁项。
    if args.index_of_unlock < args.unlock_tables.len() {
        let target = &args.unlock_tables[args.index_of_unlock];
        if let Some(table) = tables.get_mut(&target.table_id)
            && unlock_table(table, args)
        {
            *schema_version += 1;
        }
        // Go ignores a missing schema/table during batch unlock.
        // Go 在批量解锁时忽略缺失的 schema/表。
        args.index_of_unlock += 1;
        return Ok(TableLockOutcome {
            schema_version: *schema_version,
            finished: false,
        });
    }

    // 首次加锁前对全部目标做冲突预检，避免部分加锁后才发现冲突。
    if args.index_of_lock == 0 {
        for target in &args.lock_tables {
            let table = tables
                .get(&target.table_id)
                .ok_or(TableLockError::TableNotFound(target.table_id))?;
            check_table_locked(table, target.lock_type, &args.session)?;
        }
    }
    if args.index_of_lock >= args.lock_tables.len() {
        return Ok(TableLockOutcome {
            schema_version: *schema_version,
            finished: true,
        });
    }
    let target = &args.lock_tables[args.index_of_lock];
    let table = tables
        .get_mut(&target.table_id)
        .ok_or(TableLockError::TableNotFound(target.table_id))?;
    lock_table(table, target.lock_type, &args.session)?;
    let lock = table
        .lock
        .as_mut()
        .ok_or(TableLockError::InvalidState(TableLockState::None))?;
    // 两阶段状态推进：None→PreLock，再 PreLock/Public→Public 并前进下标。
    match lock.state {
        TableLockState::None => lock.state = TableLockState::PreLock,
        TableLockState::PreLock | TableLockState::Public => {
            lock.state = TableLockState::Public;
            args.index_of_lock += 1;
        }
    }
    lock.timestamp = start_timestamp;
    *schema_version += 1;
    Ok(TableLockOutcome {
        schema_version: *schema_version,
        finished: args.index_of_lock == args.lock_tables.len(),
    })
}

/// 推进一次 `UNLOCK TABLES` 批处理，每次只处理 `index_of_unlock` 指向的一项。
pub fn advance_unlock_tables(
    tables: &mut BTreeMap<i64, LockTableInfo>,
    args: &mut LockTablesArgs,
    schema_version: &mut i64,
) -> TableLockOutcome {
    if args.index_of_unlock < args.unlock_tables.len() {
        let target = &args.unlock_tables[args.index_of_unlock];
        if let Some(table) = tables.get_mut(&target.table_id)
            && unlock_table(table, args)
        {
            *schema_version += 1;
        }
        args.index_of_unlock += 1;
    }
    TableLockOutcome {
        schema_version: *schema_version,
        finished: args.index_of_unlock == args.unlock_tables.len(),
    }
}
