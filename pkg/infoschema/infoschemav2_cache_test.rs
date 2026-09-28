// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema V2 表缓存（SIEVE）行为的单元测试。
//
// 对应 Go 的 `infoschemav2_cache_test.go`。通过 `infoschema_v2::Data` 驱动生产级
// SIEVE 表缓存，并用计数钩子（CountingHook）替代 Go 侧的 testify/mock，
// 验证按 ID / 按名查找时的命中、未命中与驱逐序列。
//
// SIEVE：一种缓存淘汰算法；InfoSchema：库表等元数据的内存视图。

// Rust counterpart of pkg/infoschema/infoschemav2_cache_test.go.
// Drives the production SIEVE table cache via infoschema_v2::Data with a
// counting status hook (Go used testify/mock).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::infoschema::{CiString, ColumnInfo, DBInfo, InfoSchema, Table, TableInfo};
use crate::infoschema_v2::{NewData, NewInfoSchemaV2};
use crate::sieve::{Sieve, SieveStatusHook};

/// 统计 SIEVE 钩子事件次数：命中 / 未命中 / 驱逐 / 更新 / 更新容量上限。
#[derive(Default)]
struct CountingHook {
    hit: AtomicU64,
    miss: AtomicU64,
    evict: AtomicU64,
    update: AtomicU64,
    update_limit: AtomicU64,
}

impl SieveStatusHook for CountingHook {
    fn on_hit(&self) {
        self.hit.fetch_add(1, Ordering::SeqCst);
    }
    fn on_miss(&self) {
        self.miss.fetch_add(1, Ordering::SeqCst);
    }
    fn on_evict(&self) {
        self.evict.fetch_add(1, Ordering::SeqCst);
    }
    fn on_update(&self, _size: u64, _count: u64) {
        self.update.fetch_add(1, Ordering::SeqCst);
    }
    fn on_update_limit(&self, _limit: u64) {
        self.update_limit.fetch_add(1, Ordering::SeqCst);
    }
}

impl CountingHook {
    fn counts(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.hit.load(Ordering::SeqCst),
            self.miss.load(Ordering::SeqCst),
            self.evict.load(Ordering::SeqCst),
            self.update.load(Ordering::SeqCst),
            self.update_limit.load(Ordering::SeqCst),
        )
    }
}

/// 构造大小写不敏感的标识符（CiString）。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

/// 计算生产环境中单条缓存条目的字节大小，用于按条目数设置容量。
fn table_entry_size() -> u64 {
    // Match the production SIEVE entry-size formula for the cache key / Table pair.
    // 与生产 SIEVE 对 (cache key, Table) 的条目大小公式保持一致。
    #[derive(Clone, Eq, PartialEq, Hash)]
    struct Key {
        table_id: i64,
        schema_version: i64,
    }
    Sieve::<Key, Table>::entry_size()
}

/// 入口测试：分别跑按表 ID 与按表名的缓存命中/未命中/驱逐用例。
#[test]
fn test_infoschema_cache() {
    // Case 1: TableByID hit/miss/evict sequence (cacheCnt=3).
    // 用例 1：按 TableByID 的命中/未命中/驱逐（容量 3 条）。
    run_cache_case_by_id();
    // Case 2: TableByName hit/miss/evict sequence (cacheCnt=3).
    // 用例 2：按 TableByName 的命中/未命中/驱逐（容量 3 条）。
    run_cache_case_by_name();
}

/// 向 Data 写入 db1 与表 t1..t4，并按 cache_cnt 条数设置缓存容量。
fn seed_schema(data: &Arc<crate::infoschema_v2::Data>, cache_cnt: usize) -> Arc<CountingHook> {
    let hook = Arc::new(CountingHook::default());
    data.SetStatusHook(hook.clone());
    let entry = table_entry_size();
    // Go 的 mockTableSize 略小于真实表条目，因此第 4 次 Set 会在插入前驱逐。
    // Rust 条目是固定大小；减一字节复现同一“容量 3 条”的可观察事件序列。
    data.SetCacheCapacity((cache_cnt as u64 * entry).saturating_sub(1));

    let db = DBInfo {
        id: 1,
        name: ci("db1"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, db.clone());
    // 与 Go Data.add 一致，注册表时同时写入缓存。
    for i in 1..=4 {
        let tbl = Table::new(TableInfo {
            id: i,
            db_id: 1,
            name: ci(&format!("t{i}")),
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("a"),
                auto_increment: false,
            }],
            ..Default::default()
        });
        data.add(&db, tbl, i);
    }
    hook
}

/// 验证 TableByID：先预热 t1..t3，再访问 t4 触发未命中（及可能的驱逐），随后应命中。
fn run_cache_case_by_id() {
    let data = NewData();
    let hook = seed_schema(&data, 3);
    let is = NewInfoSchemaV2(data.clone(), 4, 1);
    for id in 1..=4 {
        assert!(is.TableByID(id).is_some(), "load table {id}");
    }
    for id in 2..=4 {
        assert!(is.TableByID(id).is_some(), "hit table {id}");
    }
    assert_eq!(hook.counts(), (3, 4, 5, 13, 1));
}

/// 验证 TableByName：同样先预热 t1..t3，再按名访问 t4。
fn run_cache_case_by_name() {
    let data = NewData();
    let hook = seed_schema(&data, 3);
    let is = NewInfoSchemaV2(data.clone(), 4, 1);
    for id in 1..=4 {
        let name = ci(&format!("t{id}"));
        assert!(is.TableByName(&ci("db1"), &name).is_ok());
    }
    for id in 2..=4 {
        let name = ci(&format!("t{id}"));
        assert!(is.TableByName(&ci("db1"), &name).is_ok());
    }
    assert_eq!(hook.counts(), (3, 4, 5, 13, 1));
}
