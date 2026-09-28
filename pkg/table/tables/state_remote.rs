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

// 缓存表（cached table）远程锁状态实现。
//
// 对应 Go 侧通过系统表 `mysql.table_cache_meta` 协调多节点读写租约（lease）：
// 读锁可并发续约，写锁需等待旧读租约过期；本模块提供可注入的 `RemoteStore`
// 抽象与内存实现，供嵌入式场景与确定性测试使用。

use crate::cache::{StateRemote, lease_from_ts};
use std::collections::HashMap;
use std::fmt;
use std::thread;
use std::time::Duration;

/// TSO / 混合时间戳中逻辑部分占用的低位比特数（物理毫秒在高位）。
const LOGICAL_BITS: u32 = 18;

/// 缓存表在远程元数据中的锁类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CachedTableLockType {
    #[default]
    /// 无锁。
    None,
    /// 读锁：允许多读者共享租约。
    Read,
    /// 意向写：已声明写意图，等待旧读租约过期。
    Intend,
    /// 写锁：独占写租约。
    Write,
}

impl CachedTableLockType {
    /// 返回与 Go / SQL 展示一致的大写锁类型字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::Read => "READ",
            Self::Intend => "INTEND",
            Self::Write => "WRITE",
        }
    }
}

/// 远程存储中一行锁元数据：锁类型、当前租约与旧读租约。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LockRow {
    pub lock_type: CachedTableLockType,
    pub lease: u64,
    pub old_read_lease: u64,
}

/// Storage boundary for `mysql.table_cache_meta`. Implementations must make a
/// load/update pair exclusive when `for_update` is true, matching the Go
/// pessimistic transaction. The in-memory implementation is intended for
/// embedded users and deterministic tests.
///
/// `mysql.table_cache_meta` 的存储边界：`for_update` 为真时 load/update 须互斥，
/// 对齐 Go 悲观事务（pessimistic transaction）语义。
pub trait RemoteStore: Send {
    type Error;

    /// 返回当前时间戳（通常为 TSO 风格的混合时间戳）。
    fn current_ts(&mut self) -> Result<u64, Self::Error>;
    /// 加载指定表的锁行；`for_update` 表示后续将更新，需独占可见性。
    fn load(&mut self, table_id: i64, for_update: bool) -> Result<LockRow, Self::Error>;
    /// 写回指定表的锁行。
    fn update(&mut self, table_id: i64, row: LockRow) -> Result<(), Self::Error>;
}

/// 进程内 `RemoteStore`：用 HashMap 模拟远程锁表，时钟可手动推进。
#[derive(Clone, Debug, Default)]
pub struct MemoryRemoteStore {
    now: u64,
    rows: HashMap<i64, LockRow>,
}

/// Error returned when the emulated remote metadata row does not exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRemoteStoreError {
    table_id: i64,
}

impl fmt::Display for MemoryRemoteStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "table_cache_meta tid not exist {}",
            self.table_id
        )
    }
}

impl std::error::Error for MemoryRemoteStoreError {}

impl MemoryRemoteStore {
    /// 以给定当前时间戳构造空锁表。
    pub fn new(now: u64) -> Self {
        Self {
            now,
            rows: HashMap::new(),
        }
    }

    /// 测试用：直接设置当前时间戳。
    pub fn set_current_ts(&mut self, now: u64) {
        self.now = now;
    }

    /// 测试用：预置某表的锁行。
    pub fn insert(&mut self, table_id: i64, row: LockRow) {
        self.rows.insert(table_id, row);
    }

    /// 查询某表当前锁行（不存在则返回 None）。
    pub fn row(&self, table_id: i64) -> Option<LockRow> {
        self.rows.get(&table_id).copied()
    }
}

impl RemoteStore for MemoryRemoteStore {
    type Error = MemoryRemoteStoreError;

    fn current_ts(&mut self) -> Result<u64, Self::Error> {
        Ok(self.now)
    }

    fn load(&mut self, table_id: i64, _for_update: bool) -> Result<LockRow, Self::Error> {
        self.rows
            .get(&table_id)
            .copied()
            .ok_or(MemoryRemoteStoreError { table_id })
    }

    fn update(&mut self, table_id: i64, row: LockRow) -> Result<(), Self::Error> {
        self.rows.insert(table_id, row);
        Ok(())
    }
}

/// 持有本地锁缓存并通过对 `RemoteStore` 的读写实现 `StateRemote` 协议。
pub struct StateRemoteHandle<S> {
    store: S,
    lock_type: CachedTableLockType,
    lease: u64,
    old_read_lease: u64,
}

impl<S> StateRemoteHandle<S> {
    /// 用给定存储后端构造句柄，本地锁初始为 None。
    pub fn new(store: S) -> Self {
        Self {
            store,
            lock_type: CachedTableLockType::None,
            lease: 0,
            old_read_lease: 0,
        }
    }

    /// 只读访问底层存储。
    pub fn store(&self) -> &S {
        &self.store
    }

    /// 可变访问底层存储（测试推进时钟等）。
    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// 返回当前本地缓存的锁行快照。
    pub fn local_state(&self) -> LockRow {
        LockRow {
            lock_type: self.lock_type,
            lease: self.lease,
            old_read_lease: self.old_read_lease,
        }
    }
}

impl<S: RemoteStore> StateRemoteHandle<S> {
    /// 从远程加载锁行并同步到本地缓存。
    fn load_row(&mut self, table_id: i64, for_update: bool) -> Result<LockRow, S::Error> {
        let row = self.store.load(table_id, for_update)?;
        self.lock_type = row.lock_type;
        self.lease = row.lease;
        self.old_read_lease = row.old_read_lease;
        Ok(row)
    }

    /// 非独占加载远程锁行。
    pub fn load(&mut self, table_id: i64) -> Result<LockRow, S::Error> {
        self.load_row(table_id, false)
    }

    /// 尝试获取/续期读锁：若远程已是写/意向写且租约未过期则失败。
    pub fn lock_for_read_inner(&mut self, table_id: i64, new_lease: u64) -> Result<bool, S::Error> {
        // 本地已持有未过期的写类锁时，拒绝再以更小或相等的读租约覆盖。
        if self.lease >= new_lease
            && matches!(
                self.lock_type,
                CachedTableLockType::Intend | CachedTableLockType::Write
            )
        {
            return Ok(false);
        }

        let now = self.store.current_ts()?;
        let mut row = self.load_row(table_id, false)?;
        if now > row.lease {
            // 远程租约已过期：仅当 new_lease 仍大于 now 时写入读锁。
            if new_lease > now {
                row.lock_type = CachedTableLockType::Read;
                row.lease = new_lease;
                self.store.update(table_id, row)?;
                return Ok(true);
            }
            return Ok(false);
        }
        if matches!(
            row.lock_type,
            CachedTableLockType::Write | CachedTableLockType::Intend
        ) {
            return Ok(false);
        }
        // 远程已是读锁：必要时抬高租约。
        if new_lease > row.lease {
            row.lock_type = CachedTableLockType::Read;
            row.lease = new_lease;
            self.store.update(table_id, row)?;
        }
        Ok(true)
    }

    /// 单次尝试获取写锁；若需等待旧读租约，返回非零 wait 时长而不 sleep。
    pub fn lock_for_write_once(
        &mut self,
        table_id: i64,
        lease_duration: Duration,
    ) -> Result<(Duration, u64), S::Error> {
        let now = self.store.current_ts()?;
        let mut row = self.load_row(table_id, true)?;
        let target = lease_from_ts(now, lease_duration);
        let mut wait = Duration::ZERO;

        if now > row.lease {
            // 租约已过期：直接写入写锁。
            row.lock_type = CachedTableLockType::Write;
            row.lease = target;
            self.store.update(table_id, row)?;
            return Ok((wait, target));
        }

        match row.lock_type {
            CachedTableLockType::None => {
                row.lock_type = CachedTableLockType::Write;
                row.lease = target;
                self.store.update(table_id, row)?;
                self.set_local(LockRow {
                    old_read_lease: 0,
                    ..row
                });
            }
            CachedTableLockType::Read => {
                // 读转意向写：保留 old_read_lease，调用方需等待其过期。
                let old_read_lease = row.lease;
                row.lock_type = CachedTableLockType::Intend;
                row.old_read_lease = old_read_lease;
                row.lease = target.max(old_read_lease);
                self.store.update(table_id, row)?;
                self.set_local(LockRow {
                    lease: target,
                    ..row
                });
                wait = wait_for_lease_expire(old_read_lease, now);
            }
            CachedTableLockType::Intend => {
                if now > row.old_read_lease {
                    row.lock_type = CachedTableLockType::Write;
                    row.lease = target;
                    self.store.update(table_id, row)?;
                    self.set_local(LockRow {
                        old_read_lease: 0,
                        ..row
                    });
                } else {
                    wait = wait_for_lease_expire(row.old_read_lease, now);
                }
            }
            CachedTableLockType::Write => {
                if target > row.lease {
                    row.lease = target;
                    self.store.update(table_id, row)?;
                    self.set_local(LockRow {
                        old_read_lease: 0,
                        ..row
                    });
                }
            }
        }
        Ok((wait, target))
    }

    /// 续期读租约：仅当本地旧租约与远程一致且仍为读锁时抬高 lease。
    pub fn renew_read_lease_inner(
        &mut self,
        table_id: i64,
        old_local_lease: u64,
        new_value: u64,
    ) -> Result<u64, S::Error> {
        let now = self.store.current_ts()?;
        let mut row = self.load_row(table_id, false)?;
        if now >= row.lease || row.lock_type != CachedTableLockType::Read {
            return Ok(0);
        }
        // 本地视角与远程不一致：若本地租约仍未过期则返回远程值，否则视为失败。
        if old_local_lease != row.lease {
            return Ok(if now < old_local_lease { row.lease } else { 0 });
        }
        if new_value > row.lease {
            row.lease = new_value;
            self.store.update(table_id, row)?;
        }
        Ok(row.lease)
    }

    /// 续期写租约；成功后按 Go 语义把调用方传入的 new_lease 写入本地缓存。
    pub fn renew_write_lease_inner(
        &mut self,
        table_id: i64,
        new_lease: u64,
    ) -> Result<bool, S::Error> {
        let now = self.store.current_ts()?;
        let mut row = self.load_row(table_id, true)?;
        if now >= row.lease || row.lock_type != CachedTableLockType::Write {
            return Ok(false);
        }
        if new_lease > row.lease {
            row.lease = new_lease;
            self.store.update(table_id, row)?;
        }
        // Keep the Go local-cache behaviour: the caller's lease becomes local.
        self.set_local(LockRow {
            lease: new_lease,
            ..row
        });
        Ok(true)
    }

    /// 用远程行覆盖本地锁缓存字段。
    fn set_local(&mut self, row: LockRow) {
        self.lock_type = row.lock_type;
        self.lease = row.lease;
        self.old_read_lease = row.old_read_lease;
    }
}

impl<S: RemoteStore> StateRemote for StateRemoteHandle<S> {
    type Error = S::Error;

    fn lock_for_read(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error> {
        self.lock_for_read_inner(table_id, lease)
    }

    fn renew_read_lease(
        &mut self,
        table_id: i64,
        old_lease: u64,
        new_lease: u64,
    ) -> Result<u64, Self::Error> {
        self.renew_read_lease_inner(table_id, old_lease, new_lease)
    }

    fn lock_for_write(
        &mut self,
        table_id: i64,
        lease_duration: Duration,
    ) -> Result<u64, Self::Error> {
        // 已持写锁且剩余租约仍大于半个周期时直接复用，避免频繁争用远程行。
        if self.lock_type == CachedTableLockType::Write {
            let now = self.store.current_ts()?;
            let safe = lease_from_ts(now, lease_duration / 2);
            if self.lease > safe {
                return Ok(self.lease);
            }
        }
        // 循环调用 lock_for_write_once，直到无需再等待旧读租约。
        loop {
            let (wait, lease) = self.lock_for_write_once(table_id, lease_duration)?;
            if wait.is_zero() {
                return Ok(lease);
            }
            thread::sleep(wait);
        }
    }

    fn renew_write_lease(&mut self, table_id: i64, lease: u64) -> Result<bool, Self::Error> {
        self.renew_write_lease_inner(table_id, lease)
    }
}

/// 计算距离 `old_read_lease` 过期还需等待的时长（从混合时间戳提取物理毫秒）。
pub fn wait_for_lease_expire(old_read_lease: u64, now: u64) -> Duration {
    if old_read_lease < now {
        return Duration::ZERO;
    }
    let old_ms = old_read_lease >> LOGICAL_BITS;
    let now_ms = now >> LOGICAL_BITS;
    if old_ms > now_ms {
        Duration::from_millis(old_ms - now_ms)
    } else {
        // 物理毫秒相同但逻辑部分仍未过：至少再等 1 微秒。
        Duration::from_micros(1)
    }
}
