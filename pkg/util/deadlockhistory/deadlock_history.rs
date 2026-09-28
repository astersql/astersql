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

// 死锁历史环形缓冲与 INFORMATION_SCHEMA.DEADLOCKS 列投影。
//
// 维护最近 N 条死锁事件（等待链、发生时间、是否可重试），供系统表查询。
// 死锁：事务互相等待对方持有的锁而无法继续。Datum 是 TiDB 内部通用值类型。

use std::sync::{Arc, LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

// ColDeadlockIDStr is the name of the DEADLOCK_ID column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// INFORMATION_SCHEMA.DEADLOCKS / CLUSTER_DEADLOCKS 的 DEADLOCK_ID 列名。
pub const ColDeadlockIDStr: &str = "DEADLOCK_ID";
// ColOccurTimeStr is the name of the OCCUR_TIME column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// OCCUR_TIME 列名：死锁发生时间。
pub const ColOccurTimeStr: &str = "OCCUR_TIME";
// ColRetryableStr is the name of the RETRYABLE column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// RETRYABLE 列名：该死锁是否可自动重试。
pub const ColRetryableStr: &str = "RETRYABLE";
// ColTryLockTrxIDStr is the name of the TRY_LOCK_TRX_ID column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// TRY_LOCK_TRX_ID 列名：尝试加锁的事务 ID。
pub const ColTryLockTrxIDStr: &str = "TRY_LOCK_TRX_ID";
// ColCurrentSQLDigestStr is the name of the CURRENT_SQL_DIGEST column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// CURRENT_SQL_DIGEST 列名：当前语句的 SQL digest（指纹）。
pub const ColCurrentSQLDigestStr: &str = "CURRENT_SQL_DIGEST";
// ColCurrentSQLDigestTextStr is the name of the CURRENT_SQL_DIGEST_TEXT column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// CURRENT_SQL_DIGEST_TEXT 列名：digest 对应的规范化 SQL 文本（本模块常返回 NULL）。
pub const ColCurrentSQLDigestTextStr: &str = "CURRENT_SQL_DIGEST_TEXT";
// ColKeyStr is the name of the KEY column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// KEY 列名：等待的键（十六进制编码）。
pub const ColKeyStr: &str = "KEY";
// ColKeyInfoStr is the name of the KEY_INFO column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// KEY_INFO 列名：键的可读解码信息（本模块常返回 NULL）。
pub const ColKeyInfoStr: &str = "KEY_INFO";
// ColTrxHoldingLockStr is the name of the TRX_HOLDING_LOCK column in INFORMATION_SCHEMA.DEADLOCKS and INFORMATION_SCHEMA.CLUSTER_DEADLOCKS table.
/// TRX_HOLDING_LOCK 列名：当前持有该锁的事务 ID。
pub const ColTrxHoldingLockStr: &str = "TRX_HOLDING_LOCK";

// WaitChainItem represents an entry in a deadlock's wait chain.
/// 死锁等待链中的一项：谁在等谁、在等哪把键、关联 SQL digest。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WaitChainItem {
    /// 尝试加锁事务当前语句的 SQL digest（十六进制字符串）。
    pub SQLDigest: String,
    /// 等待的键的原始字节。
    pub Key: Vec<u8>,
    /// 该事务近期 SQL digest 列表（可选扩展信息）。
    pub AllSQLDigests: Vec<String>,
    /// 尝试获取锁的事务 ID（start_ts）。
    pub TryLockTxn: u64,
    /// 持有锁、被等待的事务 ID。
    pub TxnHoldingLock: u64,
}

// DeadlockRecord represents a deadlock event and contains multiple transactions' information.
/// 一次死锁事件：发生时间、等待链、分配的 ID、是否可重试。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlockRecord {
    /// 死锁发生时间（带时区）。
    pub OccurTime: DateTime<Tz>,
    /// 等待链条目列表（环上每对等待关系一项）。
    pub WaitChain: Vec<WaitChainItem>,
    // The ID is allocated by DeadlockHistory::Push.
    /// 由 `DeadlockHistory::Push` 分配的单调递增 ID。
    pub ID: u64,
    /// 是否可重试（TiKV 侧标记）。
    pub IsRetryable: bool,
}

/// 构造 NULL Datum，用于空 digest/空键/未知列。
fn null_datum() -> types::Datum {
    types::NewDatum(&())
}

/// 将发生时间转为 MySQL TIMESTAMP Datum（微秒精度 MaxFsp）。
fn occur_time_datum(occur_time: DateTime<Tz>) -> types::Datum {
    let core_time = types::FromDate(
        occur_time.year(),
        occur_time.month() as i32,
        occur_time.day() as i32,
        occur_time.hour() as i32,
        occur_time.minute() as i32,
        occur_time.second() as i32,
        occur_time.nanosecond() as i32 / 1_000,
    );
    let value = types::NewTime(core_time, mysql::r#type::TypeTimestamp, types::MaxFsp);
    types::NewDatum(&value)
}

impl DeadlockRecord {
    // ToDatum creates the datum for one INFORMATION_SCHEMA.DEADLOCKS column.
    /// 按列名与等待链下标投影为 INFORMATION_SCHEMA.DEADLOCKS 的一列 Datum。
    pub fn ToDatum(&self, waitChainIdx: usize, columnName: &str) -> types::Datum {
        match columnName {
            ColDeadlockIDStr => types::NewDatum(&self.ID),
            ColOccurTimeStr => occur_time_datum(self.OccurTime),
            ColRetryableStr => types::NewDatum(&self.IsRetryable),
            ColTryLockTrxIDStr => types::NewDatum(&self.WaitChain[waitChainIdx].TryLockTxn),
            ColCurrentSQLDigestStr => {
                let digest = &self.WaitChain[waitChainIdx].SQLDigest;
                // 空 digest 投影为 NULL，与 Go 一致。
                if digest.is_empty() {
                    null_datum()
                } else {
                    types::NewDatum(digest)
                }
            }
            ColKeyStr => {
                let key = &self.WaitChain[waitChainIdx].Key;
                // 空键投影为 NULL；非空则大写十六进制编码。
                if key.is_empty() {
                    null_datum()
                } else {
                    let encoded = hex::encode_upper(key);
                    types::NewDatum(&encoded)
                }
            }
            ColTrxHoldingLockStr => types::NewDatum(&self.WaitChain[waitChainIdx].TxnHoldingLock),
            _ => null_datum(),
        }
    }
}

/// 环形缓冲内部状态：槽位、队头、有效长度、下一 ID。
#[derive(Debug)]
struct DeadlockHistoryState {
    /// 定长槽位；`None` 表示空槽。
    deadlocks: Vec<Option<Arc<DeadlockRecord>>>,
    /// 最旧记录所在下标（环形）。
    head: usize,
    /// 当前有效记录数。
    size: usize,
    /// 下一个将分配的死锁 ID（从 1 起）。
    current_id: u64,
}

// DeadlockHistory maintains the most recent deadlock events. All public APIs are thread safe.
/// 线程安全的最近死锁事件历史（读写锁保护的环形缓冲）。
#[derive(Debug)]
pub struct DeadlockHistory {
    /// 受 RwLock 保护的环形缓冲状态。
    state: RwLock<DeadlockHistoryState>,
}

// NewDeadlockHistory creates an instance of DeadlockHistory.
/// 创建容量为 `capacity` 的空历史；容量 0 时 Push 直接丢弃。
pub fn NewDeadlockHistory(capacity: usize) -> DeadlockHistory {
    DeadlockHistory {
        state: RwLock::new(DeadlockHistoryState {
            deadlocks: vec![None; capacity],
            head: 0,
            size: 0,
            current_id: 1,
        }),
    }
}

// GlobalDeadlockHistory is the process-wide deadlock history instance.
/// 进程级全局死锁历史（初始容量 0，通常由配置 Resize）。
pub static GlobalDeadlockHistory: LazyLock<DeadlockHistory> =
    LazyLock::new(|| NewDeadlockHistory(0));

impl DeadlockHistory {
    /// 获取读锁；poison 时吞掉并继续（与 Go 侧“尽力服务”一致）。
    fn read_state(&self) -> RwLockReadGuard<'_, DeadlockHistoryState> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 获取写锁；poison 时吞掉并继续。
    fn write_state(&self) -> RwLockWriteGuard<'_, DeadlockHistoryState> {
        self.state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 按从旧到新顺序展开环形缓冲中的全部记录。
    fn get_all(state: &DeadlockHistoryState) -> Vec<Arc<DeadlockRecord>> {
        let mut result = Vec::with_capacity(state.size);
        let capacity = state.deadlocks.len();
        // 未环绕：单段连续拷贝；已环绕：先尾段再头段。
        if state.head + state.size <= capacity {
            result.extend(
                state.deadlocks[state.head..state.head + state.size]
                    .iter()
                    .flatten()
                    .cloned(),
            );
        } else {
            result.extend(state.deadlocks[state.head..].iter().flatten().cloned());
            result.extend(
                state.deadlocks[..(state.head + state.size) % capacity]
                    .iter()
                    .flatten()
                    .cloned(),
            );
        }
        result
    }

    // Resize updates the maximum capacity and preserves the newest records.
    /// 调整容量并保留最新记录；缩容时丢弃最旧的多余项。
    pub fn Resize(&self, newCapacity: usize) {
        let mut state = self.write_state();
        let new_capacity = newCapacity;
        if new_capacity == state.deadlocks.len() {
            return;
        }

        let current = Self::get_all(&state);
        state.head = 0;
        if current.len() < new_capacity {
            // 扩容：重建空槽并把现有记录从 0 顺序放入。
            state.deadlocks = vec![None; new_capacity];
            state.size = current.len();
            for (index, record) in current.into_iter().enumerate() {
                state.deadlocks[index] = Some(record);
            }
        } else {
            // 缩容：只保留尾部 newest `new_capacity` 条。
            let start = current.len() - new_capacity;
            state.deadlocks = current.into_iter().skip(start).map(Some).collect();
            state.size = new_capacity;
        }
    }

    // Push allocates an ID and inserts the record, replacing the oldest record when full.
    /// 分配 ID 并插入；满时覆盖最旧槽并推进 head。容量 0 时直接返回。
    pub fn Push(&self, mut record: Box<DeadlockRecord>) {
        let mut state = self.write_state();
        let capacity = state.deadlocks.len();
        if capacity == 0 {
            return;
        }

        record.ID = state.current_id;
        state.current_id += 1;
        let record = Arc::from(record);

        if state.size == capacity {
            // 环形已满：覆盖 head 槽并前移。
            let head = state.head;
            state.deadlocks[head] = Some(record);
            state.head = (head + 1) % capacity;
        } else if state.size < capacity {
            // 未满：写到 (head+size) 模容量处并增大 size。
            let index = (state.head + state.size) % capacity;
            state.deadlocks[index] = Some(record);
            state.size += 1;
        } else {
            unreachable!();
        }
    }

    // GetAll gets all collected deadlock events in oldest-to-newest order.
    /// 返回从旧到新的全部死锁记录快照。
    pub fn GetAll(&self) -> Vec<Arc<DeadlockRecord>> {
        Self::get_all(&self.read_state())
    }

    // Clear removes all records but preserves the ID allocator, matching Go.
    /// 清空全部记录，但保留 ID 分配器（对齐 Go，Clear 后 ID 继续递增）。
    pub fn Clear(&self) {
        let mut state = self.write_state();
        state.deadlocks.fill(None);
        state.head = 0;
        state.size = 0;
    }

    /// 返回当前环形缓冲的 head 下标。
    pub fn Head(&self) -> usize {
        self.read_state().head
    }

    /// 返回当前有效记录数。
    pub fn Len(&self) -> usize {
        self.read_state().size
    }

    /// 返回环形缓冲容量。
    pub fn Capacity(&self) -> usize {
        self.read_state().deadlocks.len()
    }
}

// ErrDeadlock is the Rust counterpart of tikv client-go's ErrDeadlock payload.
/// TiKV 死锁错误载荷：protobuf Deadlock 消息加上是否可重试标志。
#[derive(Clone, Debug)]
pub struct ErrDeadlock {
    /// TiKV kvrpcpb.Deadlock（含 wait_chain 等）。
    pub Deadlock: resourcegrouptag::kvproto::kvrpcpb::Deadlock,
    /// 客户端是否应重试该事务。
    pub IsRetryable: bool,
}

// ErrDeadlockToDeadlockRecord generates a DeadlockRecord from a TiKV deadlock error.
/// 将 TiKV `ErrDeadlock` 转为 `DeadlockRecord`：解码 resource group tag 得 SQL digest。
pub fn ErrDeadlockToDeadlockRecord(dl: &ErrDeadlock) -> DeadlockRecord {
    let mut wait_chain = Vec::with_capacity(dl.Deadlock.get_wait_chain().len());
    for raw_item in dl.Deadlock.get_wait_chain() {
        // 解码失败或无 digest 时记空串，等待链项仍保留。
        let sql_digest = match resourcegrouptag::resource_group_tag::DecodeResourceGroupTag(
            raw_item.get_resource_group_tag(),
        ) {
            Ok(Some(digest)) => digest,
            Ok(None) => Vec::new(),
            Err(error) => {
                log::warn!("decoding resource group tag encounters error: {error}");
                Vec::new()
            }
        };
        wait_chain.push(WaitChainItem {
            SQLDigest: hex::encode(sql_digest),
            Key: raw_item.get_key().to_vec(),
            AllSQLDigests: Vec::new(),
            TryLockTxn: raw_item.get_txn(),
            TxnHoldingLock: raw_item.get_wait_for_txn(),
        });
    }
    DeadlockRecord {
        OccurTime: Utc::now().with_timezone(&chrono_tz::UTC),
        WaitChain: wait_chain,
        ID: 0,
        IsRetryable: dl.IsRetryable,
    }
}
