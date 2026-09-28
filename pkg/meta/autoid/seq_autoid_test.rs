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

// Port of `pkg/meta/autoid/seq_autoid_test.go`.
//
// SEQUENCE 对象的 AutoID 分配测试：内存 IdStore 模拟持久化水位，验证缓存批次、
// cycle（循环到 min/max）与并发分配不重复。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::*;

/// 内存键值存储：用互斥锁保护的 AutoIdKey → i64 映射，模拟 meta 中的序列水位。
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
}

/// 单次事务视图：读优先看 scratch，写进 scratch，提交时合并回底层 map。
struct MemoryTransaction<'a> {
    values: &'a mut HashMap<AutoIdKey, i64>,
    scratch: HashMap<AutoIdKey, i64>,
}

impl IdTransaction for MemoryTransaction<'_> {
    fn get(&self, key: AutoIdKey) -> Result<i64> {
        Ok(*self
            .scratch
            .get(&key)
            .or_else(|| self.values.get(&key))
            .unwrap_or(&0))
    }

    fn put(&mut self, key: AutoIdKey, value: i64) -> Result<()> {
        self.scratch.insert(key, value);
        Ok(())
    }

    fn inc(&mut self, key: AutoIdKey, step: i64) -> Result<i64> {
        let value = self.get(key)?.wrapping_add(step);
        self.scratch.insert(key, value);
        Ok(value)
    }

    fn copy_to(&mut self, from: AutoIdKey, to: AutoIdKey) -> Result<()> {
        let value = self.get(from)?;
        self.scratch.insert(to, value);
        Ok(())
    }
}

impl IdStore for MemoryStore {
    fn run_in_transaction(
        &self,
        operation: &mut dyn FnMut(&mut dyn IdTransaction) -> Result<()>,
    ) -> Result<()> {
        // 加锁后在 scratch 中执行操作，成功再写回，保证事务原子性语义。
        let mut values = self.values.lock().unwrap();
        let mut txn = MemoryTransaction {
            values: &mut values,
            scratch: HashMap::new(),
        };
        operation(&mut txn)?;
        for (key, value) in txn.scratch {
            values.insert(key, value);
        }
        Ok(())
    }
}

/// 构造空的共享内存 IdStore。
fn store() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::default())
}

/// Corresponds to Go `TestSequenceAutoid`.
/// 正向递增 SEQUENCE：验证缓存窗口、步长游标与 cycle 后负向区间。
#[test]
fn test_sequence_autoid() {
    let store = store();
    let seq = SequenceInfo {
        start: 1,
        cycle: true,
        cache: true,
        min_value: -10,
        max_value: 10,
        increment: 2,
        cache_value: 3,
    };
    let sequence_base = seq.start - 1;
    // Seed initial sequence value like CreateSequenceAndSetSeqValue.
    // 写入初始序列水位（start-1），与建序列后落盘语义一致。
    {
        let key = AutoIdKey {
            database_id: 1,
            table_id: 1,
            kind: AutoIdKeyKind::SequenceValue,
        };
        store
            .run_in_transaction(&mut |txn| txn.put(key, sequence_base))
            .unwrap();
    }

    let alloc = DefaultAllocator::new_sequence(store, 1, 1, seq.clone());

    // 第一批缓存：base/end/round 与 calc_sequence_batch_size 应对齐。
    let (base, end, round) = alloc.alloc_seq_cache().unwrap();
    assert_eq!(base, 0);
    assert_eq!(end, 5);
    assert_eq!(round, 0);

    let offset = seq.start;
    let size = calc_sequence_batch_size(
        sequence_base,
        seq.cache_value,
        seq.increment,
        offset,
        seq.min_value,
        seq.max_value,
    )
    .unwrap();
    assert_eq!(end - base, size);

    // 在缓存区间内按 increment 步进：1 → 3 → 5。
    let (next_val, ok) = seek_to_first_sequence_value(base, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, 1);
    let mut cursor = next_val;

    let (next_val, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, 3);
    cursor = next_val;

    let (next_val, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, 5);

    // 第二批仍在正数区间，继续取 7、9；区间末 seek 失败。
    let (base, end, round) = alloc.alloc_seq_cache().unwrap();
    assert_eq!(base, 5);
    assert_eq!(end, 10);
    assert_eq!(round, 0);
    let size = calc_sequence_batch_size(
        sequence_base,
        seq.cache_value,
        seq.increment,
        offset,
        seq.min_value,
        seq.max_value,
    )
    .unwrap();
    assert_eq!(end - base, size);

    let (next_val, ok) = seek_to_first_sequence_value(base, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, 7);
    cursor = next_val;
    let (next_val, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, 9);
    cursor = next_val;
    let (_, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(!ok);

    // cycle 后 round=1，落到负数区间；offset 切到 min_value。
    let (base, end, round) = alloc.alloc_seq_cache().unwrap();
    assert_eq!(base, -11);
    assert_eq!(end, -6);
    assert_eq!(round, 1);
    let size = calc_sequence_batch_size(
        sequence_base,
        seq.cache_value,
        seq.increment,
        offset,
        seq.min_value,
        seq.max_value,
    )
    .unwrap();
    assert_eq!(end - base, size);

    let offset = seq.min_value;
    let (next_val, ok) = seek_to_first_sequence_value(base, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, -10);
    cursor = next_val;
    let (next_val, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, -8);
    cursor = next_val;
    let (next_val, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(ok);
    assert_eq!(next_val, -6);
    cursor = next_val;
    let (_, ok) = seek_to_first_sequence_value(cursor, seq.increment, offset, base, end);
    assert!(!ok);
}

/// Corresponds to Go `TestConcurrentAllocSequence`.
/// 多线程并发 alloc_seq_cache：递减序列，断言全局不出现重复 ID。
#[test]
fn test_concurrent_alloc_sequence() {
    let store = store();
    let seq = SequenceInfo {
        start: 100,
        cycle: false,
        cache: true,
        min_value: -100,
        max_value: 100,
        increment: -2,
        cache_value: 3,
    };
    let sequence_base = seq.start + 1;
    {
        let key = AutoIdKey {
            database_id: 2,
            table_id: 2,
            kind: AutoIdKeyKind::SequenceValue,
        };
        store
            .run_in_transaction(&mut |txn| txn.put(key, sequence_base))
            .unwrap();
    }

    // 10 个线程各取若干缓存批，将 (end, base] 内 ID 写入共享集合检测重复。
    let seen = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let mut joins = Vec::new();
    for i in 0..10 {
        let store = store.clone();
        let seq = seq.clone();
        let seen = seen.clone();
        joins.push(thread::spawn(move || {
            thread::sleep(Duration::from_micros((i % 10) as u64));
            let alloc = DefaultAllocator::new_sequence(store, 2, 2, seq);
            for _ in 0..3 {
                // Go sends allocation failures through errCh and asserts the channel result.
                // Panicking here is the Rust equivalent because every worker is joined below.
                let (base, end, _) = alloc
                    .alloc_seq_cache()
                    .expect("concurrent sequence cache allocation must succeed");
                let mut guard = seen.lock().unwrap();
                // 递减序列：区间内 ID 从 end 到 base（不含 base）倒序枚举。
                for id in ((end)..base).rev() {
                    assert!(guard.insert(id), "duplicate id {id}");
                }
            }
        }));
    }
    for join in joins {
        join.join().unwrap();
    }
}
