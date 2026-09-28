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

// Aster 迁移补充单测：用内存 HashMap 模拟会话锁映射，验证读写契约与释放语义。
//
// 覆盖 `TableLockContext` 嵌入只读接口、按 table ID 释放（忽略其它字段）、
// 按 ID 列表释放以及清空全部锁。

use std::collections::HashMap;

use super::{TableLockContext, TableLockReadContext, ast, model};

/// 测试用会话锁映射：table_id → (schema_id, lock_type 字节)。
#[derive(Default)]
struct SessionTableLocks {
    locks: HashMap<i64, (i64, u8)>,
}

impl TableLockReadContext for SessionTableLocks {
    fn CheckTableLocked(&self, table_id: i64) -> (bool, ast::TableLockType) {
        self.locks
            .get(&table_id)
            .map(|(_, lock_type)| (true, ast::TableLockType(*lock_type)))
            .unwrap_or((false, ast::TableLockNone))
    }

    fn GetAllTableLocks(&self) -> Vec<model::TableLockTpInfo> {
        self.locks
            .iter()
            .map(
                |(&table_id, &(schema_id, lock_type))| model::TableLockTpInfo {
                    SchemaID: schema_id,
                    TableID: table_id,
                    Tp: model::ModelTableLockType(lock_type),
                },
            )
            .collect()
    }

    fn HasLockedTables(&self) -> bool {
        !self.locks.is_empty()
    }
}

impl TableLockContext for SessionTableLocks {
    fn AddTableLock(&mut self, locks: &[model::TableLockTpInfo]) {
        let read_only: u8 = ast::TableLockReadOnly.0.into();
        for lock in locks {
            // Go session locks do not retain read-only locks because those are
            // unrelated to the session-local lock map.
            if lock.Tp.0 != read_only {
                self.locks.insert(lock.TableID, (lock.SchemaID, lock.Tp.0));
            }
        }
    }

    fn ReleaseTableLocks(&mut self, locks: &[model::TableLockTpInfo]) {
        for lock in locks {
            self.locks.remove(&lock.TableID);
        }
    }

    fn ReleaseTableLockByTableIDs(&mut self, table_ids: &[i64]) {
        for table_id in table_ids {
            self.locks.remove(table_id);
        }
    }

    fn ReleaseAllTableLocks(&mut self) {
        self.locks.clear();
    }
}

/// 构造一条 `TableLockTpInfo` 测试数据。
fn lock(schema_id: i64, table_id: i64, lock_type: u8) -> model::TableLockTpInfo {
    model::TableLockTpInfo {
        SchemaID: schema_id,
        TableID: table_id,
        Tp: model::ModelTableLockType(lock_type),
    }
}

/// 断言锁数量与 `HasLockedTables` 一致。
fn assert_read_contract(context: &impl TableLockReadContext, expected_count: usize) {
    assert_eq!(context.GetAllTableLocks().len(), expected_count);
    assert_eq!(context.HasLockedTables(), expected_count != 0);
}

/// 写接口必须完整暴露 Go 只读契约：空映射、Add 后可查询类型与数量。
#[test]
fn write_context_embeds_the_complete_go_read_contract() {
    let mut context = SessionTableLocks::default();
    assert_read_contract(&context, 0);
    assert_eq!(context.CheckTableLocked(41), (false, ast::TableLockNone));

    context.AddTableLock(&[
        lock(7, 41, ast::TableLockRead.0.into()),
        lock(7, 42, ast::TableLockWrite.0.into()),
        lock(7, 43, ast::TableLockReadOnly.0.into()),
    ]);

    assert_read_contract(&context, 2);
    assert_eq!(context.CheckTableLocked(41), (true, ast::TableLockRead));
    assert_eq!(context.CheckTableLocked(42), (true, ast::TableLockWrite));
    assert_eq!(context.CheckTableLocked(43), (false, ast::TableLockNone));
}

/// 释放语义对齐 Go：按 table ID 删除，与 schema/类型字段无关；最后可清空。
#[test]
fn release_operations_match_the_go_session_lock_map_semantics() {
    let mut context = SessionTableLocks::default();
    context.AddTableLock(&[
        lock(7, 41, ast::TableLockRead.0.into()),
        lock(7, 42, ast::TableLockWrite.0.into()),
        lock(8, 43, ast::TableLockReadOnly.0.into()),
        lock(8, 44, ast::TableLockWriteLocal.0.into()),
    ]);

    // Go removes ReleaseTableLocks entries by table ID, independent of the other fields.
    // 即使 SchemaID/类型与写入时不同，只要 TableID 匹配就会删除。
    context.ReleaseTableLocks(&[lock(999, 42, ast::TableLockNone.0.into())]);
    assert_eq!(context.CheckTableLocked(42), (false, ast::TableLockNone));

    context.ReleaseTableLockByTableIDs(&[41, 43]);
    assert_read_contract(&context, 1);
    assert_eq!(
        context.CheckTableLocked(44),
        (true, ast::TableLockWriteLocal)
    );

    context.ReleaseAllTableLocks();
    assert_read_contract(&context, 0);
}
