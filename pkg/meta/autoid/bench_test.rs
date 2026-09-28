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

// Port of `pkg/meta/autoid/bench_test.go`.
// Go uses `testing.B`; here we exercise the same allocation / seek paths under
// `#[test]` with a bounded iteration count so the gate stays deterministic.
//
// 基准路径的确定性复现：用固定迭代次数的单元测试覆盖 Alloc、SequenceAlloc、
// Seek（批大小计算）三条热点路径，避免依赖不稳定的 `cargo bench`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::*;

/// 内存 KV：键为 [`AutoIdKey`]，值为当前全局 AutoID 水位。
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
}

/// 事务视图：先写 scratch，提交时合并回主表（模拟存储事务）。
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
        let mut values = self.values.lock().unwrap();
        let mut txn = MemoryTransaction {
            values: &mut values,
            scratch: HashMap::new(),
        };
        operation(&mut txn)?;
        // 提交：将事务 scratch 合并进持久化 map。
        for (key, value) in txn.scratch {
            values.insert(key, value);
        }
        Ok(())
    }
}

/// Corresponds to Go `BenchmarkAllocator_Alloc`.
/// 反复单步分配，覆盖本地缓存命中与向存储预留的路径。
#[test]
fn benchmark_allocator_alloc() {
    let store = Arc::new(MemoryStore::default());
    let alloc = DefaultAllocator::new(store, 1, 2, false, AllocatorType::RowId);
    let ctx = Context::background();
    for _ in 0..256 {
        alloc.alloc(&ctx, 1, 1, 1).unwrap();
    }
}

/// Corresponds to Go `BenchmarkAllocator_SequenceAlloc`.
/// 预置序列起点后反复 `alloc_seq_cache`，覆盖序列批分配。
#[test]
fn benchmark_allocator_sequence_alloc() {
    let store = Arc::new(MemoryStore::default());
    let seq = SequenceInfo {
        start: 1,
        cycle: true,
        cache: false,
        min_value: -10,
        max_value: i64::MAX,
        increment: 2,
        cache_value: 2_000_000,
    };
    {
        let key = AutoIdKey {
            database_id: 1,
            table_id: 1,
            kind: AutoIdKeyKind::SequenceValue,
        };
        store
            .run_in_transaction(&mut |txn| txn.put(key, seq.start - 1))
            .unwrap();
    }
    let alloc = DefaultAllocator::new_sequence(store, 1, 1, seq);
    for _ in 0..64 {
        alloc.alloc_seq_cache().unwrap();
    }
}

/// Corresponds to Go `BenchmarkAllocator_Seek`.
/// 纯计算：反复求序列批大小，不含存储 I/O。
#[test]
fn benchmark_allocator_seek() {
    let base = 21_421_948_021_i64;
    let offset = -351_354_365_326_i64;
    let increment = 3_i64;
    for _ in 0..256 {
        calc_sequence_batch_size(base, 3, increment, offset, i64::MIN, i64::MAX).unwrap();
    }
}
