// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Port of `pkg/meta/autoid/autoid_test.go`.
// Uses an in-memory `IdStore` that preserves Go allocator / rebase / concurrency
// semantics without requiring mockstore/Unistore.
//
// 本地缓存分配器集成测试：有符号/无符号分配与 rebase、并发唯一性、
// 事务回滚、动态步长、批大小计算边界，以及 alloc/base 竞态窗口。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::*;

/// 可注入提交失败的内存 IdStore。
#[derive(Default)]
struct MemoryStore {
    values: Mutex<HashMap<AutoIdKey, i64>>,
    fail_commit: AtomicBool,
}

/// 事务 scratch：成功后才合并到主 map，模拟提交语义。
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
        // 注入失败：不进入事务，用于 TestRollbackAlloc。
        if self.fail_commit.load(Ordering::SeqCst) {
            return Err(AutoIdError::Storage("injected".into()));
        }
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

/// 默认空存储。
fn store() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::default())
}

/// 使用全局默认 step 的 RowId 分配器。
fn allocator(store: Arc<MemoryStore>, db: i64, table: i64, unsigned: bool) -> DefaultAllocator {
    allocator_with_step(store, db, table, unsigned, get_step())
}

/// 指定自定义 step 的 RowId 分配器。
fn allocator_with_step(
    store: Arc<MemoryStore>,
    db: i64,
    table: i64,
    unsigned: bool,
    step: i64,
) -> DefaultAllocator {
    DefaultAllocator::with_options(
        store,
        db,
        table,
        unsigned,
        AllocatorType::RowId,
        &[AllocatorOption::CustomStep(step)],
    )
}

/// Corresponds to Go `TestSignedAutoid`.
/// 覆盖有符号分配、rebase、跨分配器共享水位、increment/offset 批计算。
#[test]
fn test_signed_autoid() {
    let store = store();
    let ctx = Context::background();
    let step = 30_000_i64;
    let alloc = allocator_with_step(store.clone(), 1, 1, false, step);

    assert_eq!(alloc.next_global_auto_id().unwrap(), 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 2);
    assert_eq!(alloc.next_global_auto_id().unwrap(), step + 1);

    alloc.rebase(&ctx, 1, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 3);
    alloc.rebase(&ctx, 3, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 4);
    alloc.rebase(&ctx, 10, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 11);
    alloc.rebase(&ctx, 3010, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 3011);

    let alloc = allocator_with_step(store.clone(), 1, 1, false, step);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, step + 1);

    let alloc = allocator_with_step(store.clone(), 1, 2, false, step);
    alloc.rebase(&ctx, 1, false).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 2);

    let alloc = allocator_with_step(store.clone(), 1, 3, false, step);
    alloc.rebase(&ctx, 3210, false).unwrap();
    let alloc = allocator_with_step(store.clone(), 1, 3, false, step);
    alloc.rebase(&ctx, 3000, false).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 3211);
    alloc.rebase(&ctx, 6543, false).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1, 6544);

    alloc.rebase(&ctx, i64::MAX - 1, true).unwrap();
    assert!(alloc.alloc(&ctx, 1, 1, 1).is_err());
    alloc.rebase(&ctx, i64::MAX, true).unwrap();

    let alloc = allocator_with_step(store.clone(), 1, 4, false, step);
    assert_eq!(alloc.next_global_auto_id().unwrap(), 1);
    let (minv, maxv) = alloc.alloc(&ctx, 1, 1, 1).unwrap();
    assert_eq!(maxv - minv, 1);
    assert_eq!(minv + 1, 1);
    let (minv, maxv) = alloc.alloc(&ctx, 2, 1, 1).unwrap();
    assert_eq!(maxv - minv, 2);
    assert_eq!(minv + 1, 2);
    assert_eq!(maxv, 3);
    let (minv, maxv) = alloc.alloc(&ctx, 100, 1, 1).unwrap();
    assert_eq!(maxv - minv, 100);
    let mut expected = 4;
    for i in (minv + 1)..=maxv {
        assert_eq!(i, expected);
        expected += 1;
    }

    alloc.rebase(&ctx, 1000, false).unwrap();
    let (minv, maxv) = alloc.alloc(&ctx, 3, 1, 1).unwrap();
    assert_eq!(maxv - minv, 3);
    assert_eq!(minv + 1, 1001);
    assert_eq!(minv + 2, 1002);
    assert_eq!(maxv, 1003);

    let last_remain_one = alloc.end();
    alloc.rebase(&ctx, alloc.end() - 2, false).unwrap();
    let (minv, maxv) = alloc.alloc(&ctx, 5, 1, 1).unwrap();
    assert_eq!(maxv - minv, 5);
    assert!(minv + 1 > last_remain_one);

    let alloc = allocator_with_step(store, 1, 5, false, step);
    let increment = 2;
    let offset = 100;
    let (minv, maxv) = alloc.alloc(&ctx, 1, increment, offset).unwrap();
    assert_eq!(minv, 99);
    assert_eq!(maxv, 100);

    let (minv, maxv) = alloc.alloc(&ctx, 2, increment, offset).unwrap();
    assert_eq!(maxv - minv, 4);
    assert_eq!(
        calc_needed_batch_size(100, 2, increment, offset, false),
        maxv - minv
    );
    assert_eq!(minv, 100);
    assert_eq!(maxv, 104);

    let increment = 5;
    let (minv, maxv) = alloc.alloc(&ctx, 3, increment, offset).unwrap();
    assert_eq!(maxv - minv, 11);
    assert_eq!(
        calc_needed_batch_size(104, 3, increment, offset, false),
        maxv - minv
    );
    assert_eq!(minv, 104);
    assert_eq!(maxv, 115);
    assert_eq!(seek_to_first_auto_id_signed(104, increment, offset), 105);

    let increment = 15;
    let (minv, maxv) = alloc.alloc(&ctx, 2, increment, offset).unwrap();
    assert_eq!(maxv - minv, 30);
    assert_eq!(
        calc_needed_batch_size(115, 2, increment, offset, false),
        maxv - minv
    );
    assert_eq!(minv, 115);
    assert_eq!(maxv, 145);
    assert_eq!(seek_to_first_auto_id_signed(115, increment, offset), 130);

    let offset = 200;
    let (minv, maxv) = alloc.alloc(&ctx, 2, increment, offset).unwrap();
    assert_eq!(maxv - minv, 16);
    assert_eq!(
        calc_needed_batch_size(offset - 1, 2, increment, offset, false),
        maxv - minv
    );
    assert_eq!(minv, 199);
    assert_eq!(maxv, 215);
    assert_eq!(
        seek_to_first_auto_id_signed(offset - 1, increment, offset),
        200
    );
}

/// Corresponds to Go `TestUnsignedAutoid` (core path).
/// 覆盖无符号边界（接近 u64::MAX）与 force_rebase 路径。
#[test]
fn test_unsigned_autoid() {
    let store = store();
    let ctx = Context::background();
    let step = 30_000_i64;
    let alloc = allocator_with_step(store.clone(), 1, 1, true, step);

    assert_eq!(alloc.next_global_auto_id().unwrap() as u64, 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, 2);
    assert_eq!(alloc.next_global_auto_id().unwrap() as u64, step as u64 + 1);

    alloc.rebase(&ctx, 1, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, 3);
    alloc.rebase(&ctx, 10, true).unwrap();
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, 11);

    let alloc = allocator_with_step(store.clone(), 1, 2, true, step);
    // Match Go/MySQL unsigned boundary: rebase near MaxUint64 then alloc must fail.
    let near_max = (u64::MAX - 1) as i64;
    alloc.rebase(&ctx, near_max, true).unwrap();
    assert!(alloc.alloc(&ctx, 1, 1, 1).is_err());
    alloc.rebase(&ctx, (u64::MAX) as i64, true).unwrap();

    // force_rebase covers the NextGlobalAutoID = MaxUint64-1 path used by Go memid/unsigned suites.
    let alloc = allocator_with_step(store.clone(), 1, 3, true, step);
    alloc.force_rebase((u64::MAX - 2) as i64).unwrap();
    assert_eq!(alloc.next_global_auto_id().unwrap() as u64, u64::MAX - 1);
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap().1 as u64, u64::MAX - 1);
    assert!(matches!(
        alloc.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::AutoIncrementReadFailed(_))
    ));

    let alloc = allocator_with_step(store.clone(), 1, 5, true, step);
    let increment = 2_i64;
    let offset = 100_i64;
    let (minv, maxv) = alloc.alloc(&ctx, 1, increment, offset).unwrap();
    assert_eq!(minv as u64, 99);
    assert_eq!(maxv as u64, 100);
    let (minv, maxv) = alloc.alloc(&ctx, 2, increment, offset).unwrap();
    assert_eq!(
        calc_needed_batch_size(100, 2, increment, offset, true),
        maxv - minv
    );
    assert_eq!(seek_to_first_auto_id_unsigned(100, 2, 100), 102);

    // Go uses AutoRandomType here so the signed i64 representation of a
    // near-MaxUint64 offset is not rejected by AUTO_INCREMENT validation.
    let alloc = DefaultAllocator::with_options(
        store,
        1,
        6,
        true,
        AllocatorType::AutoRandom,
        &[AllocatorOption::CustomStep(step)],
    );
    let offset = (u64::MAX - 100) as i64;
    let (minv, maxv) = alloc.alloc(&ctx, 2, 2, offset).unwrap();
    assert_eq!(minv as u64, u64::MAX - 101);
    assert_eq!(maxv as u64, u64::MAX - 98);
    assert_eq!(
        calc_needed_batch_size(minv, 2, 2, offset, true),
        maxv - minv
    );
    assert_eq!(
        seek_to_first_auto_id_unsigned(minv as u64, 2, offset as u64),
        u64::MAX - 100
    );
}

/// Corresponds to Go `TestConcurrentAlloc`.
/// 多线程并发分配，校验 ID 全局唯一。
#[test]
fn test_concurrent_alloc() {
    let store = store();
    let step = 100_i64;
    let ids = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let mut joins = Vec::new();
    for _ in 0..10 {
        let store = store.clone();
        let ids = ids.clone();
        joins.push(thread::spawn(move || {
            let ctx = Context::background();
            let alloc = allocator_with_step(store, 2, 100, false, step);
            for _ in 0..(step as usize + 5) {
                let id = alloc.alloc(&ctx, 1, 1, 1).unwrap().1;
                assert!(ids.lock().unwrap().insert(id), "duplicate id {id}");
                let n = id as u64 % 20;
                let (minv, maxv) = alloc.alloc(&ctx, n, 1, 1).unwrap();
                let mut guard = ids.lock().unwrap();
                for i in (minv + 1)..=maxv {
                    assert!(guard.insert(i), "duplicate id {i}");
                }
            }
        }));
    }
    for join in joins {
        join.join().unwrap();
    }
}

/// Corresponds to Go `TestRollbackAlloc`.
/// 存储事务失败时本地 base/end 不得前进。
#[test]
fn test_rollback_alloc() {
    let store = store();
    store.fail_commit.store(true, Ordering::SeqCst);
    let alloc = allocator(store, 1, 2, false);
    let ctx = Context::background();
    assert!(alloc.alloc(&ctx, 1, 1, 1).is_err());
    assert_eq!(alloc.base(), 0);
    assert_eq!(alloc.end(), 0);
    assert!(alloc.rebase(&ctx, 100, true).is_err());
    assert_eq!(alloc.base(), 0);
    assert_eq!(alloc.end(), 0);
}

/// Corresponds to Go `TestNextStep`.
/// 动态步长在极快/正常/极慢消耗下的夹紧结果。
#[test]
fn test_next_step() {
    assert_eq!(next_step(2_000_000, Duration::from_nanos(1)), 2_000_000);
    assert_eq!(next_step(678_910, Duration::from_secs(10)), 678_910);
    assert_eq!(next_step(50_000, Duration::from_secs(600)), 30_000);
}

/// Go `NewAllocator` keeps the hidden RowID cache independent from
/// `AUTO_ID_CACHE 1` once AUTO_INCREMENT and RowID are separated in v5.
#[test]
fn row_id_ignores_auto_id_cache_one_since_table_info_v5() {
    let alloc = DefaultAllocator::with_options(
        store(),
        1,
        1,
        false,
        AllocatorType::RowId,
        &[
            AllocatorOption::CustomStep(1),
            AllocatorOption::TableInfoVersion(5),
        ],
    );

    assert_eq!(alloc.alloc(&Context::background(), 1, 1, 1).unwrap().1, 1);
    assert_eq!(alloc.end(), get_step());
    assert_eq!(alloc.next_global_auto_id().unwrap(), get_step() + 1);
}

/// Corresponds to Go `TestAllocComputationIssue`.
/// 注入局部 base/end 后验证批大小计算与分配结果。
#[test]
fn test_alloc_computation_issue() {
    let store = store();
    let ctx = Context::background();
    let unsigned = DefaultAllocator::with_options(
        store.clone(),
        1,
        1,
        true,
        AllocatorType::RowId,
        &[AllocatorOption::CustomStep(3)],
    );
    let signed = DefaultAllocator::with_options(
        store,
        1,
        2,
        false,
        AllocatorType::RowId,
        &[AllocatorOption::CustomStep(3)],
    );

    unsigned.rebase(&ctx, 10, false).unwrap();
    signed.rebase(&ctx, 7, false).unwrap();
    unsigned.modify_base_and_end_for_test(9, 9);
    signed.modify_base_and_end_for_test(4, 6);

    let (minv, maxv) = unsigned.alloc(&ctx, 2, 3, 1).unwrap();
    assert_eq!(minv, 10);
    assert_eq!(maxv, 16);
    let (minv, maxv) = signed.alloc(&ctx, 2, 3, 1).unwrap();
    assert_eq!(minv, 7);
    assert_eq!(maxv, 13);
}

/// Corresponds to Go `TestIssue40584` (alloc / base race window).
/// 并发调用 alloc 与 base，确认无死锁/崩溃。
#[test]
fn test_issue_40584() {
    let store = store();
    let alloc = Arc::new(allocator(store, 1, 1, false));
    let done = Arc::new(AtomicBool::new(false));
    let alloc_done = {
        let alloc = alloc.clone();
        let done = done.clone();
        thread::spawn(move || {
            let ctx = Context::background();
            while !done.load(Ordering::SeqCst) {
                let _ = alloc.alloc(&ctx, 1, 1, 1);
            }
        })
    };
    let base_done = {
        let alloc = alloc.clone();
        let done = done.clone();
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let _ = alloc.base();
            }
        })
    };
    thread::sleep(Duration::from_millis(200));
    done.store(true, Ordering::SeqCst);
    alloc_done.join().unwrap();
    base_done.join().unwrap();
}

/// Corresponds to Go `TestGetAutoIDServiceLeaderEtcdPath`.
/// Nullspace 与普通 keyspace 的 etcd 路径格式。
#[test]
fn test_get_auto_id_service_leader_etcd_path() {
    assert_eq!(
        get_auto_id_service_leader_etcd_path(NULLSPACE_ID),
        AUTO_ID_LEADER_PATH
    );
    assert_eq!(
        get_auto_id_service_leader_etcd_path(1),
        format!("/{AUTO_ID_LEADER_PATH}")
    );
}

/// Extra pure helpers exercised by Go signed/unsigned suites.
/// 纯函数：系统库 ID、AUTO_RANDOM 规范化、有符号编码与分片布局。
#[test]
fn test_pure_helpers_match_go() {
    assert!(is_mem_schema_id(INFORMATION_SCHEMA_DB_ID));
    assert!(!is_mem_schema_id(42));
    assert_eq!(auto_random_shard_bits_normalize(-1, "id").unwrap(), 5);
    assert!(auto_random_shard_bits_normalize(0, "id").is_err());
    assert_eq!(auto_random_range_bits_normalize(-1).unwrap(), 64);
    for value in [i64::MIN, -1, 0, 1, i64::MAX] {
        assert_eq!(decode_cmp_uint_to_int(encode_int_to_cmp_uint(value)), value);
    }
    let signed = ShardIdFormat::new(false, 5, 64);
    assert_eq!(signed.incremental_bits, 58);
    assert_eq!(signed.compose(3, 7), (3_i64 << 58) | 7);
}

/// Go integer arithmetic and shifts use fixed-width wrapping at these public
/// helper boundaries; Rust must not panic or narrow the unsigned 64-bit mask.
#[test]
fn test_go_wrapping_boundaries() {
    let alloc = allocator_with_step(store(), 1, 1, false, 1);
    alloc.force_rebase(i64::MAX).unwrap();
    assert_eq!(alloc.next_global_auto_id().unwrap(), i64::MIN);

    assert_eq!(seek_to_first_auto_id_signed(i64::MAX - 1, 2, 1), i64::MAX);
    assert_eq!(calc_needed_batch_size(i64::MAX - 1, 1, 2, 1, false), 1);

    let unsigned = ShardIdFormat::new(true, 0, 0);
    assert_eq!(unsigned.incremental_bits, 64);
    assert_eq!(unsigned.incremental_mask(), -1);
    assert_eq!(unsigned.incremental_bits_capacity(), u64::MAX);
    assert_eq!(unsigned.compose(0, 7), 7);
}

/// Go only observes context cancellation when an allocation reaches storage;
/// validation, zero-sized requests, cached allocation and no-op rebase return
/// before opening a context-aware transaction.
#[test]
fn test_context_cancellation_matches_go_control_flow() {
    let ctx = Context::background();
    ctx.cancel();

    let invalid = allocator(store(), 1, 0, false);
    assert!(matches!(
        invalid.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::InvalidTableId(_))
    ));

    let alloc = allocator_with_step(store(), 1, 1, false, 10);
    assert_eq!(alloc.alloc(&ctx, 0, 1, 1).unwrap(), (0, 0));

    let background = Context::background();
    assert_eq!(alloc.alloc(&background, 1, 1, 1).unwrap(), (0, 1));
    assert_eq!(alloc.alloc(&ctx, 1, 1, 1).unwrap(), (1, 2));
    alloc.rebase(&ctx, 2, true).unwrap();

    let refill = allocator_with_step(store(), 1, 2, false, 10);
    assert!(matches!(
        refill.alloc(&ctx, 1, 1, 1),
        Err(AutoIdError::Canceled)
    ));
}
