// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `util_test.go`：校验 util 模块的隔离级别判定与 infiniteChan 行为。

use std::thread;

use crate::*;

// 矩阵覆盖各 ServerType × Consistency 组合，与 Go TestRepeatableRead 用例一致。
#[test]
fn test_repeatable_read() {
    let data = [
        (ServerType::ServerTypeUnknown, ConsistencyTypeNone, true),
        (ServerType::ServerTypeMySQL, ConsistencyTypeFlush, true),
        (ServerType::ServerTypeMariaDB, ConsistencyTypeLock, true),
        (ServerType::ServerTypeTiDB, ConsistencyTypeNone, true),
        (ServerType::ServerTypeTiDB, ConsistencyTypeSnapshot, false),
        (ServerType::ServerTypeTiDB, ConsistencyTypeLock, true),
    ];
    // TiDB + snapshot 是唯一返回 false 的组合，其余均需 RR。
    for (i, (server_tp, consistency, expect)) in data.iter().enumerate() {
        let rr = needRepeatableRead(*server_tp, consistency);
        assert_eq!(rr, *expect, "test case number: {i}");
    }
}

// 并发写入 10000 个整数，验证 infiniteChan 保序且不丢数据。
#[test]
fn test_infinite_chan() {
    let (tx, rx) = infiniteChan::<i32>();
    // 生产者 goroutine 连续 send，消费者主线程 recv，验证无阻塞死锁。
    thread::spawn(move || {
        for i in 0..10000 {
            tx.send(i).unwrap();
        }
    });
    for i in 0..10000 {
        // 严格保序：第 i 次 recv 必须等于 i。
        let j = rx.recv().unwrap();
        assert_eq!(i, j);
    }
}

// Go getPdDDLIDs extracts the final path component from every key returned by
// the /tidb/server/info prefix scan. Values are deliberately irrelevant.
#[test]
fn test_get_pd_ddl_ids() {
    let cli = EtcdClient::default();
    {
        let mut kvs = cli.kvs.lock().unwrap();
        kvs.insert(
            "/tidb/server/info/ddl-owner-b".to_string(),
            "ignored-b".to_string(),
        );
        kvs.insert(
            "/tidb/server/info/ddl-owner-a".to_string(),
            "ignored-a".to_string(),
        );
        kvs.insert("/unrelated/key".to_string(), "ignored".to_string());
    }

    let mut ids = getPdDDLIDs(&cli).unwrap();
    ids.sort();
    assert_eq!(ids, ["ddl-owner-a", "ddl-owner-b"]);
    assert_eq!(
        *cli.last_get_timeout.lock().unwrap(),
        Some(Duration::from_secs(10))
    );
}

#[test]
fn test_get_pd_ddl_ids_propagates_read_error() {
    let cli = EtcdClient::default();
    *cli.get_error.lock().unwrap() = Some(errors_new("etcd read failed"));

    assert_eq!(getPdDDLIDs(&cli).unwrap_err().msg, "etcd read failed");
}

#[test]
fn test_check_same_cluster_rejects_missing_pd_endpoints() {
    let err = checkSameCluster(&tcontext::Background(), &DB::default(), &[]).unwrap_err();
    assert_eq!(err.msg, "etcdclient: no available endpoints");
}

// Match Go's map construction semantics: later duplicate keys overwrite
// earlier entries while all distinct pairs are retained.
#[test]
fn test_string_to_map() {
    let keys = [
        "first".to_string(),
        "second".to_string(),
        "first".to_string(),
    ];
    let values = ["old".to_string(), "value".to_string(), "new".to_string()];

    let mapped = string2Map(&keys, &values);
    assert_eq!(mapped.len(), 2);
    assert_eq!(mapped.get("first").map(String::as_str), Some("new"));
    assert_eq!(mapped.get("second").map(String::as_str), Some("value"));
}

// Both Go and Rust treat unequal input lengths as a violated precondition.
#[test]
#[should_panic]
fn test_string_to_map_panics_when_values_are_shorter() {
    string2Map(&["missing".to_string()], &[]);
}

// Closing the input must drain queued values in order and then close output,
// matching the handleRead(!ok) branch in Go infiniteChan.
#[test]
fn test_infinite_chan_drains_and_closes() {
    let (tx, rx) = infiniteChan();
    for i in 0..128 {
        tx.send(i).unwrap();
    }
    drop(tx);

    assert_eq!(rx.iter().collect::<Vec<_>>(), (0..128).collect::<Vec<_>>());
}
