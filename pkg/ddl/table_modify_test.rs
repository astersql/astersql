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

// 表只读（read only）与 `LOCK TABLES` 互斥行为的单元测试。
//
// 可执行部分用 [`TableAccessController`] 模拟访问控制：
// read only 禁止写入与各类锁表；已有锁禁止切 read only；写锁互斥规则对齐 Go。

// Copyright 2026 AsterSQL.

use crate::executor::{ExecutorError, TableAccessController, TableLockType};

/// 验证 read only 禁止写入与全部锁表模式，cleanup 后写权限恢复。
#[test]
fn read_only_blocks_writes_and_all_table_lock_modes() {
    let mut access = TableAccessController::default();
    // 首次设置 read only 返回 true（状态变更）；重复设置幂等返回 false。
    assert!(access.set_read_only(true).unwrap());
    assert!(!access.set_read_only(true).unwrap());
    assert_eq!(Err(ExecutorError::LockConflict), access.check_write());
    for lock_type in [
        TableLockType::Read,
        TableLockType::Write,
        TableLockType::WriteLocal,
    ] {
        assert_eq!(Err(ExecutorError::LockConflict), access.lock(1, lock_type));
    }
    // cleanup 清除只读/锁元数据后允许写入。
    access.cleanup();
    assert_eq!(Ok(()), access.check_write());
}

/// 验证已有锁禁止切 read only，以及写锁/本地写锁的互斥兼容性。
#[test]
fn existing_locks_block_read_only_and_lock_compatibility_matches_go() {
    let mut access = TableAccessController::default();
    // 多个共享读锁可并存，但整体禁止切 read only，也禁止再加写锁。
    access.lock(1, TableLockType::Read).unwrap();
    access.lock(2, TableLockType::Read).unwrap();
    assert_eq!(2, access.lock_count());
    assert_eq!(Err(ExecutorError::LockConflict), access.set_read_only(true));
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(3, TableLockType::Write)
    );
    assert!(access.unlock(1));
    assert!(access.unlock(2));

    // Write 与 WriteLocal 均排他：第二会话同类型加锁应冲突。
    access.lock(1, TableLockType::Write).unwrap();
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::Write)
    );
    assert!(access.unlock(1));
    access.lock(1, TableLockType::WriteLocal).unwrap();
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::WriteLocal)
    );
}

/// Go 测试会在不执行 UNLOCK 的情况下，让同一会话依次把 READ 锁替换为
/// WRITE 和 WRITE LOCAL；换锁应成功，同时仍拒绝另一会话取得排他锁。
#[test]
fn same_connection_can_replace_its_table_lock_like_go() {
    let mut access = TableAccessController::default();

    access.lock(1, TableLockType::Read).unwrap();
    access.lock(1, TableLockType::Write).unwrap();
    assert_eq!(1, access.lock_count());
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::Write)
    );

    access.lock(1, TableLockType::WriteLocal).unwrap();
    assert_eq!(1, access.lock_count());
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::WriteLocal)
    );
}
