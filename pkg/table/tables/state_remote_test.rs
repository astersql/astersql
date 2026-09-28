// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 缓存表远程锁状态的确定性测试。
//
// 通过可控时钟的内存存储覆盖读写锁转换、租约续期及本地缓存行为，
// 并校验 Rust 实现与 Go 版本在边界条件下保持一致。

use crate::cache::StateRemote;
use crate::state_remote::{
    CachedTableLockType, LockRow, MemoryRemoteStore, StateRemoteHandle, wait_for_lease_expire,
};
use std::time::Duration;

const LOGICAL_BITS: u32 = 18;

/// 将物理毫秒编码为 TSO 混合时间戳，逻辑位统一置零以便精确控制测试时钟。
fn ts(milliseconds: u64) -> u64 {
    milliseconds << LOGICAL_BITS
}

/// 锁类型字符串属于远程元数据协议，必须与 Go 枚举值完全一致。
#[test]
fn lock_type_strings_match_remote_enum_values() {
    assert_eq!(CachedTableLockType::None.as_str(), "NONE");
    assert_eq!(CachedTableLockType::Read.as_str(), "READ");
    assert_eq!(CachedTableLockType::Intend.as_str(), "INTEND");
    assert_eq!(CachedTableLockType::Write.as_str(), "WRITE");
}

#[test]
fn load_rejects_missing_memory_row_like_go() {
    let mut handle = StateRemoteHandle::new(MemoryRemoteStore::new(ts(100)));
    assert_eq!(
        handle.load(5).unwrap_err().to_string(),
        "table_cache_meta tid not exist 5"
    );
    assert_eq!(handle.local_state(), LockRow::default());
}

#[test]
fn read_lock_acquires_expired_row_and_never_decreases_lease() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Intend,
            lease: ts(90),
            old_read_lease: ts(80),
        },
    );
    let mut handle = StateRemoteHandle::new(store);
    assert_eq!(handle.lock_for_read_inner(5, ts(120)), Ok(true));
    assert_eq!(
        handle.store().row(5),
        Some(LockRow {
            lock_type: CachedTableLockType::Read,
            lease: ts(120),
            old_read_lease: ts(80),
        })
    );
    // Go's updateRow changes only lock_type and lease. loadRow's pre-update
    // snapshot therefore remains local until a later remote read.
    assert_eq!(
        handle.local_state(),
        LockRow {
            lock_type: CachedTableLockType::Intend,
            lease: ts(90),
            old_read_lease: ts(80),
        }
    );
    assert_eq!(handle.lock_for_read_inner(5, ts(110)), Ok(true));
    assert_eq!(handle.store().row(5).unwrap().lease, ts(120));
    assert_eq!(handle.lock_for_read_inner(5, ts(140)), Ok(true));
    assert_eq!(handle.store().row(5).unwrap().lease, ts(140));
    assert_eq!(handle.local_state().lease, ts(120));
}

#[test]
fn active_intend_or_write_lock_rejects_reader() {
    for lock_type in [CachedTableLockType::Intend, CachedTableLockType::Write] {
        let mut store = MemoryRemoteStore::new(ts(100));
        store.insert(
            5,
            LockRow {
                lock_type,
                lease: ts(150),
                old_read_lease: ts(120),
            },
        );
        let mut handle = StateRemoteHandle::new(store);
        assert_eq!(handle.lock_for_read_inner(5, ts(130)), Ok(false));
    }
}

#[test]
fn read_to_write_transition_waits_then_becomes_write() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Read,
            lease: ts(120),
            old_read_lease: 0,
        },
    );
    let mut handle = StateRemoteHandle::new(store);

    // 首次写锁尝试先写入意向锁，并要求等待旧读租约到期。
    let (wait, lease) = handle
        .lock_for_write_once(5, Duration::from_millis(50))
        .unwrap();
    assert_eq!(wait, Duration::from_millis(20));
    assert_eq!(lease, ts(150));
    assert_eq!(
        handle.store().row(5),
        Some(LockRow {
            lock_type: CachedTableLockType::Intend,
            lease: ts(150),
            old_read_lease: ts(120),
        })
    );
    assert_eq!(
        handle.local_state(),
        LockRow {
            lock_type: CachedTableLockType::Intend,
            lease: ts(150),
            old_read_lease: ts(120),
        }
    );

    handle.store_mut().set_current_ts(ts(121));
    // 推进到旧读租约之后，第二次尝试才能完成意向锁到写锁的转换。
    let (wait, lease) = handle
        .lock_for_write_once(5, Duration::from_millis(50))
        .unwrap();
    assert_eq!(wait, Duration::ZERO);
    assert_eq!(lease, ts(171));
    assert_eq!(
        handle.store().row(5).unwrap().lock_type,
        CachedTableLockType::Write
    );
    assert_eq!(handle.store().row(5).unwrap().old_read_lease, ts(120));
    assert_eq!(handle.local_state().old_read_lease, 0);
}

#[test]
fn existing_write_lock_is_extended_but_not_decreased() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Write,
            lease: ts(130),
            old_read_lease: 0,
        },
    );
    let mut handle = StateRemoteHandle::new(store);
    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(10))
            .unwrap(),
        (Duration::ZERO, ts(110))
    );
    assert_eq!(handle.store().row(5).unwrap().lease, ts(130));
    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(50))
            .unwrap(),
        (Duration::ZERO, ts(150))
    );
    assert_eq!(handle.store().row(5).unwrap().lease, ts(150));
}

#[test]
fn write_lock_updates_preserve_remote_old_read_lease_and_go_local_snapshots() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Read,
            lease: ts(90),
            old_read_lease: ts(80),
        },
    );
    let mut handle = StateRemoteHandle::new(store);

    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(50))
            .unwrap(),
        (Duration::ZERO, ts(150))
    );
    assert_eq!(handle.store().row(5).unwrap().old_read_lease, ts(80));
    assert_eq!(
        handle.local_state(),
        LockRow {
            lock_type: CachedTableLockType::Read,
            lease: ts(90),
            old_read_lease: ts(80),
        }
    );

    handle.store_mut().insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Intend,
            lease: ts(150),
            old_read_lease: ts(90),
        },
    );
    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(60))
            .unwrap(),
        (Duration::ZERO, ts(160))
    );
    assert_eq!(handle.store().row(5).unwrap().old_read_lease, ts(90));
    assert_eq!(
        handle.local_state(),
        LockRow {
            lock_type: CachedTableLockType::Write,
            lease: ts(160),
            old_read_lease: 0,
        }
    );

    handle.store_mut().insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::None,
            lease: ts(150),
            old_read_lease: ts(70),
        },
    );
    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(70))
            .unwrap(),
        (Duration::ZERO, ts(170))
    );
    assert_eq!(handle.store().row(5).unwrap().old_read_lease, ts(70));
    assert_eq!(handle.local_state().old_read_lease, 0);

    handle.store_mut().insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Write,
            lease: ts(150),
            old_read_lease: ts(60),
        },
    );
    assert_eq!(
        handle
            .lock_for_write_once(5, Duration::from_millis(80))
            .unwrap(),
        (Duration::ZERO, ts(180))
    );
    assert_eq!(handle.store().row(5).unwrap().old_read_lease, ts(60));
    assert_eq!(handle.local_state().old_read_lease, 0);
}

#[test]
fn renew_read_lease_handles_aba_and_expiration() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Read,
            lease: ts(150),
            old_read_lease: 0,
        },
    );
    let mut handle = StateRemoteHandle::new(store);

    // 远端租约变化时返回其当前值；过期的本地租约不能误续期新一轮读锁（ABA 防护）。
    assert_eq!(
        handle.renew_read_lease_inner(5, ts(140), ts(160)),
        Ok(ts(150))
    );
    assert_eq!(handle.renew_read_lease_inner(5, ts(90), ts(160)), Ok(0));
    assert_eq!(
        handle.renew_read_lease_inner(5, ts(150), ts(160)),
        Ok(ts(160))
    );
    assert_eq!(handle.store().row(5).unwrap().lease, ts(160));
    assert_eq!(handle.local_state().lease, ts(150));

    handle.store_mut().set_current_ts(ts(160));
    assert_eq!(handle.renew_read_lease_inner(5, ts(160), ts(170)), Ok(0));
}

#[test]
fn renew_write_lease_preserves_go_local_cache_behavior() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Write,
            lease: ts(150),
            old_read_lease: 0,
        },
    );
    let mut handle = StateRemoteHandle::new(store);
    assert_eq!(handle.renew_write_lease_inner(5, ts(160)), Ok(true));
    assert_eq!(handle.store().row(5).unwrap().lease, ts(160));
    assert_eq!(handle.renew_write_lease_inner(5, ts(140)), Ok(true));
    assert_eq!(handle.store().row(5).unwrap().lease, ts(160));
    // Go 兼容语义允许本地缓存采用调用方传入值，即使远端租约不会回退。
    assert_eq!(handle.local_state().lease, ts(140));
}

/// 公共写锁入口应复用仍处于安全窗口内的本地租约，避免访问远程锁行。
#[test]
fn public_write_lock_reuses_safe_local_write_lease() {
    let mut store = MemoryRemoteStore::new(ts(100));
    store.insert(
        5,
        LockRow {
            lock_type: CachedTableLockType::Write,
            lease: ts(150),
            old_read_lease: 0,
        },
    );
    let mut handle = StateRemoteHandle::new(store);
    handle.load(5).unwrap();

    assert_eq!(
        StateRemote::lock_for_write(&mut handle, 5, Duration::from_millis(20)),
        Ok(ts(150))
    );
}

/// 等待时长只取 TSO 的物理毫秒；同毫秒内仍未过期时保留最小等待量。
#[test]
fn wait_duration_uses_physical_time_and_minimum_microsecond() {
    assert_eq!(
        wait_for_lease_expire(ts(120), ts(100)),
        Duration::from_millis(20)
    );
    assert_eq!(
        wait_for_lease_expire(ts(100) + 2, ts(100) + 1),
        Duration::from_micros(1)
    );
    assert_eq!(wait_for_lease_expire(ts(99), ts(100)), Duration::ZERO);
}
