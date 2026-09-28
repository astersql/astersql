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

// Checksum 相关单元测试。
//
// 覆盖 TiDB 远端校验和（checksum）执行器的标识符转义、GC lifetime 临时延长与恢复，
// 以及本地 `KVChecksum` 字节累计与远端结果比对；并验证取消令牌（CancellationToken）
// 可中断校验流程。

use std::sync::{Arc, Mutex};

use crate::checksum::{
    ChecksumManager, ChecksumSource, GCTTLManager, KVChecksum, NewTiDBChecksumExecutor,
    NewTiKVChecksumManager, PDClient, RemoteChecksum, SqlChecksumClient, TableInfo,
    serviceSafePointTTL,
};
use crate::{CancellationToken, Error, KvPair, Result};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct RecordingPD {
    updates: Mutex<Vec<(i64, u64)>>,
    get_ts_calls: AtomicUsize,
}

impl PDClient for RecordingPD {
    fn GetTS(&self, _token: &CancellationToken) -> Result<(i64, i64)> {
        self.get_ts_calls.fetch_add(1, Ordering::Relaxed);
        Ok((1, 1))
    }

    fn UpdateServiceGCSafePoint(
        &self,
        _token: &CancellationToken,
        _service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64> {
        self.updates.lock().unwrap().push((ttl, safe_point));
        Ok(safe_point)
    }
}

struct NonRetryableSource {
    calls: AtomicUsize,
}

struct RetryableSource {
    calls: AtomicUsize,
}

impl ChecksumSource for RetryableSource {
    fn checksum_table(
        &self,
        _token: &CancellationToken,
        _table: &TableInfo,
        _ts: u64,
    ) -> Result<KVChecksum> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(Error::Retryable("temporary checksum failure".into()))
    }
}

impl ChecksumSource for NonRetryableSource {
    fn checksum_table(
        &self,
        _token: &CancellationToken,
        _table: &TableInfo,
        _ts: u64,
    ) -> Result<KVChecksum> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(Error::InvalidData("bad checksum input".into()))
    }
}

/// 测试用 SQL 客户端：记录发出的 checksum 查询与 GC lifetime 更新。
#[derive(Default)]
struct SqlClient {
    /// 已执行的 checksum SQL 列表。
    queries: Mutex<Vec<String>>,
    /// 已写入的 GC lifetime 值（含临时延长与恢复）。
    lifetime_updates: Mutex<Vec<String>>,
}

impl SqlChecksumClient for SqlClient {
    /// 记录 SQL 并返回固定的远端校验和结果。
    fn query_checksum(&self, sql: &str) -> Result<RemoteChecksum> {
        self.queries.lock().unwrap().push(sql.to_owned());
        Ok(RemoteChecksum {
            Schema: "s`1".into(),
            Table: "t`1".into(),
            Checksum: 7,
            TotalKVs: 2,
            TotalBytes: 6,
        })
    }

    /// 返回模拟的当前 GC lifetime（垃圾回收可存活时间窗口）。
    fn obtain_gc_lifetime(&self) -> Result<String> {
        Ok("1h".into())
    }

    /// 记录对 GC lifetime 的更新（执行前延长、结束后恢复）。
    fn update_gc_lifetime(&self, value: &str) -> Result<()> {
        self.lifetime_updates.lock().unwrap().push(value.to_owned());
        Ok(())
    }
}

/// 验证标识符中的反引号被正确转义，且 GC lifetime 先延长再恢复。
#[test]
fn tidb_checksum_escapes_identifiers_and_restores_gc_lifetime() {
    let client = Arc::new(SqlClient::default());
    let executor = NewTiDBChecksumExecutor(client.clone());
    // schema/table 含反引号时，SQL 中应写成 `` `s``1`.`t``1` ``
    let result = executor
        .Checksum(
            &CancellationToken::default(),
            &TableInfo {
                schema: "s`1".into(),
                table: "t`1".into(),
                table_id: 1,
                index_ids: vec![2],
            },
        )
        .unwrap();
    assert_eq!(result.Checksum, 7);
    assert_eq!(
        client.queries.lock().unwrap().as_slice(),
        ["ADMIN CHECKSUM TABLE `s``1`.`t``1`"]
    );
    // 先设为 100h 再恢复为原始 1h
    assert_eq!(
        client.lifetime_updates.lock().unwrap().as_slice(),
        ["100h", "1h"]
    );
}

/// 验证本地 KV 校验和累计字节，以及取消令牌使 Checksum 返回 Cancelled。
#[test]
fn kv_checksum_counts_bytes_and_remote_equality_exactly() {
    let mut checksum = KVChecksum::default();
    checksum.update(&KvPair {
        key: b"k1".to_vec(),
        value: b"v1".to_vec(),
    });
    checksum.update(&KvPair {
        key: b"k2".to_vec(),
        value: b"v2".to_vec(),
    });
    // k1+v1 与 k2+v2 各 4 字节，合计 8
    assert_eq!(checksum.total_kvs, 2);
    assert_eq!(checksum.total_bytes, 8);
    assert!(
        RemoteChecksum {
            Schema: String::new(),
            Table: String::new(),
            Checksum: checksum.checksum,
            TotalKVs: 2,
            TotalBytes: 8,
        }
        .IsEqual(&checksum)
    );

    // 已取消的令牌应使远端校验立即失败
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    let client = Arc::new(SqlClient::default());
    assert_eq!(
        NewTiDBChecksumExecutor(client)
            .Checksum(&cancelled, &TableInfo::default())
            .unwrap_err(),
        Error::Cancelled
    );
}

#[test]
fn gc_ttl_manager_keeps_same_table_jobs_and_removes_one_without_refresh() {
    let pd = Arc::new(RecordingPD::default());
    let manager = GCTTLManager::new(pd.clone(), "test-service".into());
    let token = CancellationToken::default();

    manager.addOneJob(&token, "t", 20).unwrap();
    manager.addOneJob(&token, "t", 10).unwrap();
    assert_eq!(
        pd.updates.lock().unwrap().as_slice(),
        &[
            (
                serviceSafePointTTL.load(std::sync::atomic::Ordering::Relaxed),
                20
            ),
            (
                serviceSafePointTTL.load(std::sync::atomic::Ordering::Relaxed),
                10
            ),
        ]
    );

    manager.removeOneJob(&token, "t");
    assert_eq!(pd.updates.lock().unwrap().len(), 2);

    manager.close(&token);
    assert_eq!(pd.updates.lock().unwrap().last(), Some(&(0, 10)));
}

#[test]
fn tikv_checksum_does_not_retry_non_retryable_scan_errors() {
    let source = Arc::new(NonRetryableSource {
        calls: AtomicUsize::new(0),
    });
    let manager = NewTiKVChecksumManager(
        source.clone(),
        Arc::new(RecordingPD::default()),
        8,
        15,
        String::new(),
        "test-service",
    );

    assert_eq!(
        manager
            .Checksum(&CancellationToken::default(), &TableInfo::default())
            .unwrap_err(),
        Error::InvalidData("bad checksum input".into())
    );
    assert_eq!(source.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn tikv_checksum_matches_go_retry_count_and_constructor_concurrency() {
    let source = Arc::new(RetryableSource {
        calls: AtomicUsize::new(0),
    });
    let pd = Arc::new(RecordingPD::default());
    let manager = NewTiKVChecksumManager(
        source.clone(),
        pd.clone(),
        1,
        15,
        String::new(),
        "test-service",
    );

    assert_eq!(manager.dist_sql_scan_concurrency, 1);
    assert_eq!(
        manager
            .Checksum(&CancellationToken::default(), &TableInfo::default())
            .unwrap_err(),
        Error::Retryable("temporary checksum failure".into())
    );
    assert_eq!(source.calls.load(Ordering::Relaxed), 3);
    assert_eq!(pd.get_ts_calls.load(Ordering::Relaxed), 1);
}
