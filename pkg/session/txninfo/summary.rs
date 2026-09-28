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

// 已结束事务的历史摘要记录（TrxHistoryRecorder）。
//
// 按 SQL digest 序列做 FNV 哈希，用 LRU 缓存近期长事务模式；
// 供 `INFORMATION_SCHEMA` 等展示事务摘要。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::txn_info::{Datum, TxnInfo};
use types::datum::NewStringDatum;

/// 对 digests 逐字节做 FNV-1a，得到事务模式指纹。
fn digest(digests: &[String]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for sql_digest in digests {
        for byte in sql_digest.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

/// LRU 中的一条事务摘要：指纹与原始 digest 列表。
struct TrxSummaryEntry {
    trxDigest: u64,
    digests: Vec<String>,
}

/// 固定容量的事务摘要 LRU：`elements` 判重，`cache` 维护次序。
struct TrxSummaries {
    capacity: usize,
    elements: HashMap<u64, ()>,
    cache: VecDeque<TrxSummaryEntry>,
}

/// 创建指定容量的空摘要表。
fn newTrxSummaries(capacity: usize) -> TrxSummaries {
    TrxSummaries {
        capacity,
        elements: HashMap::new(),
        cache: VecDeque::new(),
    }
}

impl TrxSummaries {
    /// 事务结束时更新 LRU：已存在则移到队头，否则插入并可能淘汰队尾。
    fn onTrxEnd(&mut self, digests: Vec<String>) {
        // 命中：从原位置摘下再 push_front，保持最近使用在前。
        let key = digest(&digests);
        if self.elements.contains_key(&key) {
            if let Some(position) = self.cache.iter().position(|entry| entry.trxDigest == key)
                && let Some(entry) = self.cache.remove(position)
            {
                self.cache.push_front(entry);
            }
            return;
        }

        // 未命中：写入新条目；超容量则弹出最旧项。
        self.elements.insert(key, ());
        self.cache.push_front(TrxSummaryEntry {
            trxDigest: key,
            digests,
        });
        if self.cache.len() > self.capacity {
            if let Some(last) = self.cache.pop_back() {
                self.elements.remove(&last.trxDigest);
            }
        }
    }

    /// 导出摘要为 Datum 行：十六进制指纹 + digests 的 JSON 字符串。
    fn dumpTrxSummary(&self) -> Vec<Vec<Datum>> {
        self.cache
            .iter()
            .map(|entry| {
                let digest_text = format!("{:x}", entry.trxDigest);
                let sqls = serde_json::to_string(&entry.digests)
                    .expect("a string slice is always JSON serializable");
                vec![NewStringDatum(digest_text), NewStringDatum(sqls)]
            })
            .collect()
    }

    /// 调整容量并立刻淘汰超出部分。
    fn resize(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.cache.len() > self.capacity {
            if let Some(last) = self.cache.pop_back() {
                self.elements.remove(&last.trxDigest);
            }
        }
    }

    /// 清空 LRU；与 Go 一样保留判重索引。
    fn clean(&mut self) {
        self.cache.clear();
    }
}

/// 记录器可变状态：最短计入时长与摘要表。
struct TrxHistoryRecorderState {
    minDuration: Duration,
    summaries: TrxSummaries,
}

/// 线程安全的事务历史摘要记录器。
pub struct TrxHistoryRecorder {
    state: Mutex<TrxHistoryRecorderState>,
}

impl TrxHistoryRecorder {
    /// 导出当前摘要行。
    pub fn DumpTrxSummary(&self) -> Vec<Vec<Datum>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summaries
            .dumpTrxSummary()
    }

    /// 事务结束回调：用 StartTS 推算物理开始时间，过短则忽略。
    ///
    /// StartTS 右移 18 位得到毫秒级物理时间戳（与 TiDB TSO 布局一致）。
    pub fn OnTrxEnd(&self, info: &TxnInfo) {
        let start_time = UNIX_EPOCH
            .checked_add(Duration::from_millis(info.StartTS >> 18))
            .unwrap_or(UNIX_EPOCH);
        let elapsed = SystemTime::now().duration_since(start_time);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 未达最小持续时长的事务不记入摘要。
        if elapsed.is_err() || elapsed.is_ok_and(|duration| duration < state.minDuration) {
            return;
        }
        state.summaries.onTrxEnd(info.AllSQLDigests.clone());
    }

    /// 清空摘要。
    pub fn Clean(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summaries
            .clean();
    }

    /// 设置计入摘要的最小事务时长。
    pub fn SetMinDuration(&self, duration: Duration) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .minDuration = duration;
    }

    /// 调整摘要 LRU 容量。
    pub fn ResizeSummaries(&self, capacity: usize) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summaries
            .resize(capacity);
    }
}

/// 创建记录器；默认最小时长 1 秒。
pub fn newTrxHistoryRecorder(capacity: usize) -> TrxHistoryRecorder {
    TrxHistoryRecorder {
        state: Mutex::new(TrxHistoryRecorderState {
            minDuration: Duration::from_secs(1),
            summaries: newTrxSummaries(capacity),
        }),
    }
}

/// 进程级全局记录器（容量 0，使用前需 Resize）。
pub static Recorder: LazyLock<TrxHistoryRecorder> = LazyLock::new(|| newTrxHistoryRecorder(0));
