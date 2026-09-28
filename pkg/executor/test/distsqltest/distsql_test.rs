// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 对应 `pkg/executor/test/distsqltest/distsql_test.go`。
//
// 两项测试都经过真实 TestKit session、DDL/DML 与关系执行器。请求断言读取
// session 在 SELECT 执行边界产出的 `kv::Request`，不再手工构造 KeyRanges。

#![allow(non_snake_case)]

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use astersql_config::{GetTxnScopeFromConfig, restore_func, update_global};
use astersql_config_kerneltype::IsNextGen;
use astersql_kv::ReplicaReadType;
use astersql_sessionctx_vardef::DefDistSQLScanConcurrency;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows};

struct CleanupGuard<F: FnOnce()> {
    cleanup: Option<F>,
}

impl<F: FnOnce()> CleanupGuard<F> {
    fn new(cleanup: F) -> Self {
        Self {
            cleanup: Some(cleanup),
        }
    }
}

impl<F: FnOnce()> Drop for CleanupGuard<F> {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

#[test]
fn CleanupGuardRunsDuringUnwind() {
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleaned_by_guard = Arc::clone(&cleaned);
    let result = std::panic::catch_unwind(move || {
        let _guard = CleanupGuard::new(move || cleaned_by_guard.store(true, Ordering::SeqCst));
        panic!("exercise panic cleanup");
    });

    assert!(result.is_err());
    assert!(cleaned.load(Ordering::SeqCst));
}

/// 对应 Go `TestDistsqlPartitionTableConcurrency`。
#[test]
fn TestDistsqlPartitionTableConcurrency() {
    crate::main_test::setup_test_main();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    assert_eq!(store.runtime_topology().len(), 3);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1, t2, t3", Vec::new());
    tk.MustExec("create table t1(id int primary key, val int)", Vec::new());
    let partitions = (1..=20)
        .map(|pid| format!("PARTITION p{pid} VALUES LESS THAN ({pid}00)"))
        .collect::<Vec<_>>();
    tk.MustExec(
        &format!(
            "create table t2(id int primary key, val int) partition by range(id) ({})",
            partitions[..10].join(",")
        ),
        Vec::new(),
    );
    tk.MustExec(
        &format!(
            "create table t3(id int primary key, val int) partition by range(id) ({})",
            partitions.join(",")
        ),
        Vec::new(),
    );
    for i in 0..20 {
        for table in ["t1", "t2", "t3"] {
            tk.MustExec(
                &format!("insert into {table} values({}, {})", i * 50, i * 50),
                Vec::new(),
            );
        }
    }
    tk.MustExec("analyze table t1, t2, t3", Vec::new());

    let default_concurrency = i32::try_from(DefDistSQLScanConcurrency)
        .expect("DistSQL scan concurrency must fit kv::Request.Concurrency");
    for (table, partition_num, concurrency) in [
        ("t1", 1, 1),
        ("t2", 10, 10),
        ("t3", 20, default_concurrency),
    ] {
        for limit in [1, 5, 1, 5] {
            store
                .clear_select_request_for_test()
                .expect("clear SELECT request");
            assert_eq!(
                tk.MustQuery(&format!("select * from {table} limit {limit}"), Vec::new())
                    .len(),
                limit
            );
            let observed = store
                .last_select_request_for_test()
                .expect("read SELECT request")
                .expect("TableReader SELECT must emit kv::Request");
            assert_eq!(observed.access_path, "TableReader");
            assert!(observed.auxiliary_requests.is_empty());
            assert_eq!(
                observed
                    .request
                    .KeyRanges
                    .as_ref()
                    .expect("SELECT request KeyRanges")
                    .PartitionNum(),
                partition_num
            );
            assert_eq!(observed.request.Concurrency, concurrency);
        }
    }
    drop(tk);
    assert_eq!(store.active_session_count(), 0);
}

/// 对应 Go `TestDistSQLSharedKVRequestRace`（issue #60175）。
#[test]
fn TestDistSQLSharedKVRequestRace() {
    crate::main_test::setup_test_main();
    let (store, _domain) = CreateMockStoreAndDomain();
    let _restore = CleanupGuard::new(restore_func());
    update_global(|config| {
        config
            .labels
            .insert("zone".to_owned(), "us-east-1a".to_owned());
    });
    assert_eq!(GetTxnScopeFromConfig(), "us-east-1a");

    let mut tk = NewTestKit(store.clone());
    tk.MustExec(
        "set session tidb_partition_prune_mode='dynamic'",
        Vec::new(),
    );
    tk.MustExec("set session tidb_enable_index_merge = ON", Vec::new());
    tk.MustExec("use test;", Vec::new());
    tk.MustExec("drop table if exists t;", Vec::new());
    tk.MustExec(
        "create table t (
            a int,
            b int,
            c int,
            d int,
            primary key (a, d),
            index ib(b),
            index ic(c)
        )
        partition by range(d) (
            partition p1 values less than(1),
            partition p2 values less than(2),
            partition p3 values less than(3),
            partition p4 values less than (4)
        )",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    for i in 0..1000 {
        tk.MustExec(
            &format!(
                "insert into t values ({}, {}, {}, {});",
                i * 1000,
                i * 1000,
                i * 1000,
                i % 4
            ),
            Vec::new(),
        );
    }
    tk.MustExec("commit", Vec::new());

    let expects = (0..500)
        .map(|i| format!("{} {} {} {}", i * 1000, i * 1000, i * 1000, i % 4))
        .collect::<Vec<_>>();
    let expected_rows = expects.iter().map(String::as_str).collect::<Vec<_>>();
    let mut replica_read_modes = vec!["leader".to_owned()];
    if !IsNextGen() {
        replica_read_modes.extend([
            "follower".to_owned(),
            "leader-and-follower".to_owned(),
            "closest-adaptive".to_owned(),
            "closest-replicas".to_owned(),
        ]);
    }

    for mode in replica_read_modes {
        tk.MustExec(
            &format!("set session tidb_replica_read = '{mode}'"),
            Vec::new(),
        );
        let expected_replica = match mode.as_str() {
            "follower" => ReplicaReadType::ReplicaReadFollower,
            "leader-and-follower" => ReplicaReadType::ReplicaReadMixed,
            "closest-adaptive" => ReplicaReadType::ReplicaReadClosestAdaptive,
            "closest-replicas" => ReplicaReadType::ReplicaReadClosest,
            _ => ReplicaReadType::ReplicaReadLeader,
        };
        for _ in 0..20 {
            tk.MustQuery(
                "select * from t force index(ic) order by c asc limit 500",
                Vec::new(),
            )
            .Check(Rows(&expected_rows));
            let index_lookup = store
                .last_select_request_for_test()
                .expect("read index lookup request")
                .expect("index lookup must emit requests");
            assert_eq!(index_lookup.access_path, "IndexLookup");
            assert_eq!(index_lookup.auxiliary_requests.len(), 1);
            assert_eq!(index_lookup.request.ReplicaRead, expected_replica);
            assert_eq!(index_lookup.dispatches.len(), 2);
            assert_eq!(index_lookup.max_parallel_workers, 2);
            let lookup_addresses = std::iter::once(&index_lookup.request)
                .chain(index_lookup.auxiliary_requests.iter())
                .map(|request| Arc::as_ptr(request) as usize)
                .collect::<HashSet<_>>();
            assert_eq!(lookup_addresses.len(), 2);
            assert_eq!(
                index_lookup
                    .dispatches
                    .iter()
                    .map(|dispatch| dispatch.request_address)
                    .collect::<HashSet<_>>(),
                lookup_addresses
            );

            tk.MustQuery(
                "select * from t where b >= 0 or c >= 0 order by c asc limit 500",
                Vec::new(),
            )
            .Check(Rows(&expected_rows));
            let index_merge = store
                .last_select_request_for_test()
                .expect("read index merge request")
                .expect("index merge must emit requests");
            assert_eq!(index_merge.access_path, "IndexMerge");
            assert_eq!(index_merge.auxiliary_requests.len(), 2);
            assert_eq!(index_merge.request.ReplicaRead, expected_replica);
            assert_eq!(index_merge.dispatches.len(), 3);
            assert_eq!(index_merge.max_parallel_workers, 3);
            let merge_addresses = std::iter::once(&index_merge.request)
                .chain(index_merge.auxiliary_requests.iter())
                .map(|request| Arc::as_ptr(request) as usize)
                .collect::<HashSet<_>>();
            assert_eq!(merge_addresses.len(), 3);
            assert_eq!(
                index_merge
                    .dispatches
                    .iter()
                    .map(|dispatch| dispatch.request_address)
                    .collect::<HashSet<_>>(),
                merge_addresses
            );

            // The second query must not mutate the first query's request.
            assert_eq!(index_lookup.access_path, "IndexLookup");
            assert_eq!(index_lookup.request.ReplicaRead, expected_replica);
        }
    }
    drop(tk);
    assert_eq!(store.active_session_count(), 0);
}
