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

// 表锁与 RENAME TABLE / DROP SCHEMA 交互的单元测试。
//
// 覆盖：
// - RENAME 要求本会话持有 Public 的 WRITE 锁，同会话 READ 锁不足以放行；
// - 会话仍持有任意表锁时禁止 DROP SCHEMA，解锁后方可继续。

use crate::table_lock::{
    LockTableInfo, LockTablesArgs, SessionInfo, TableLockError, TableLockState, TableLockTarget,
    TableLockType, check_drop_schema_lock, check_rename_table_lock, lock_table, unlock_table,
};

/// 构造固定的测试会话（server=`tidb-1`，session_id=7）。
fn session() -> SessionInfo {
    SessionInfo {
        server_id: "tidb-1".into(),
        session_id: 7,
    }
}

/// 构造未加锁的测试表元信息。
fn table() -> LockTableInfo {
    LockTableInfo {
        schema_id: 1,
        table_id: 10,
        table_name: "t1".into(),
        lock: None,
    }
}

/// 验证 RENAME 需要本会话的 Public WRITE 锁；READ 锁应返回 TableNotLockedForWrite。
#[test]
fn rename_requires_a_public_write_lock_owned_by_the_session() {
    let owner = session();
    let mut table = table();
    lock_table(&mut table, TableLockType::Write, &owner).unwrap();
    // 将锁推进到 Public，模拟加锁 DDL 已完成。
    table.lock.as_mut().unwrap().state = TableLockState::Public;
    assert_eq!(Ok(()), check_rename_table_lock(&table, &owner));

    // 同会话持有 READ 锁时，RENAME 仍应被拒绝。
    table.lock.as_mut().unwrap().lock_type = TableLockType::Read;
    assert_eq!(
        Err(TableLockError::TableNotLockedForWrite("t1".into())),
        check_rename_table_lock(&table, &owner)
    );
}

/// 验证持锁会话在解锁前无法 DROP SCHEMA，解锁后校验通过。
#[test]
fn active_session_lock_blocks_schema_drop_until_unlock() {
    let owner = session();
    let mut table = table();
    lock_table(&mut table, TableLockType::Write, &owner).unwrap();
    table.lock.as_mut().unwrap().state = TableLockState::Public;
    assert_eq!(
        Err(TableLockError::LockOrActiveTransaction),
        check_drop_schema_lock(std::slice::from_ref(&table), &owner)
    );

    // 通过 unlock_table 移除本会话持锁后，DROP SCHEMA 校验应放行。
    let args = LockTablesArgs {
        lock_tables: Vec::new(),
        unlock_tables: vec![TableLockTarget {
            schema_id: 1,
            table_id: 10,
            lock_type: TableLockType::Write,
        }],
        session: owner.clone(),
        index_of_lock: 0,
        index_of_unlock: 0,
        cleanup: false,
    };
    assert!(unlock_table(&mut table, &args));
    assert_eq!(Ok(()), check_drop_schema_lock(&[table], &owner));
}
