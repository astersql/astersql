// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/checksum/executor_test.go`.
//!
//! Darwin arm64: no kv/domain/kvproto/grpcio. Mock cluster / testkit / TiKV are
//! replaced by in-memory `TableInfo` fixtures + `MemClient` that returns the same
//! dummy checksum payloads Go's mock cluster does (all fields 1). Failpoint uses
//! `inject_checksum_retry_err` matching Go `checksumRetryErr`.
//!
//! 模块职责：对齐 Go `executor_test.go` 的 checksum Executor 行为契约，
//! 在无 mock.Cluster / testkit 环境下用内存 Client 与静态 TableInfo 复现。
//! 数据流：Builder → 请求列表 → MemClient.Send → 聚合 ChecksumResponse。
//! 约束：不启动真实 TiKV；退避睡眠可关闭以稳定取消/重试路径；
//! 失败注入通过 `inject_checksum_retry_err` 对应 Go failpoint。
//! 夹具原则：TableInfo 字段只填场景所需最小集，避免暗示已接入真实 schema。
//! 聚合语义：多请求 Checksum 按 XOR 合并，TotalKvs/TotalBytes 按加法累加。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    CIStr, ChecksumRequest, ChecksumResponse, ChecksumScanOn, Client, Context, Error,
    FullNotNullRange, GenTableRecordPrefix, IndexInfo, MetaTable, NewExecutorBuilder, Request,
    RequestBuilder, Response, StatePublic, TableInfo, inject_checksum_retry_err,
    set_skip_backoff_sleep, updateChecksumResponse,
};

/// Go `vardef.DefChecksumTableConcurrency` (= 4).
/// 与 Go 变量定义保持同值，供并发度断言与 Builder 配置共用。
const DEF_CHECKSUM_TABLE_CONCURRENCY: u32 = 4;

/// 内存 Response：按序吐出已序列化的 checksum chunk，末尾由调用方追加 None。
/// chunks 中 Option::None 表示流结束哨兵；idx 为下一次读取位置。
struct MemResponse {
    chunks: Vec<Option<Vec<u8>>>,
    idx: usize,
    closed: bool,
}

impl Response for MemResponse {
    fn NextRaw(&mut self, _ctx: &Context) -> Result<Option<Vec<u8>>, Error> {
        // 耗尽后返回 None，模拟 Go kv.Response 流结束。
        if self.idx >= self.chunks.len() {
            return Ok(None);
        }
        let item = self.chunks[self.idx].clone();
        self.idx += 1;
        Ok(item)
    }

    fn Close(&mut self) -> Result<(), Error> {
        // 标记关闭；TrackingResponse 另计 close 次数。
        self.closed = true;
        Ok(())
    }
}

/// In-memory Client: one dummy ChecksumResponse{1,1,1} per Send, matching
/// Go mock cluster's dummy checksum.
///
/// 替代 Go mock.Cluster 的 Storage.GetClient：按 Send 次序或请求 Data
/// 回放预设响应；默认每请求一个全 1 的 ChecksumResponse。
struct MemClient {
    /// 尚未绑定到具体 Data 的响应队列（FIFO）。
    pending: Mutex<Vec<Vec<ChecksumResponse>>>,
    /// 按请求 Data 缓存，保证同一请求重复 Send 结果稳定。
    by_data: Mutex<HashMap<Vec<u8>, Vec<ChecksumResponse>>>,
    send_calls: AtomicUsize,
    close_calls: Arc<AtomicUsize>,
}

impl MemClient {
    /// 为 n_reqs 个请求各准备一份相同响应向量。
    fn with_uniform(n_reqs: usize, resp: ChecksumResponse) -> Self {
        Self {
            pending: Mutex::new(vec![vec![resp]; n_reqs]),
            by_data: Mutex::new(HashMap::new()),
            send_calls: AtomicUsize::new(0),
            close_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 与 Go mock 集群一致：Checksum/TotalKvs/TotalBytes 皆为 1。
    fn dummy(n_reqs: usize) -> Self {
        Self::with_uniform(
            n_reqs,
            ChecksumResponse {
                Checksum: 1,
                TotalKvs: 1,
                TotalBytes: 1,
            },
        )
    }

    /// 优先按 Data 命中缓存；否则从 pending 弹出，空则回落默认全 1。
    fn chunks_for(&self, req: &Request) -> Vec<ChecksumResponse> {
        let mut by_data = self.by_data.lock().unwrap();
        if let Some(chunks) = by_data.get(&req.Data) {
            return chunks.clone();
        }
        let mut pending = self.pending.lock().unwrap();
        let chunks = if pending.is_empty() {
            // 超额 Send 仍返回合法 dummy，避免测试因长度估计偏差崩溃。
            vec![ChecksumResponse {
                Checksum: 1,
                TotalKvs: 1,
                TotalBytes: 1,
            }]
        } else {
            pending.remove(0)
        };
        by_data.insert(req.Data.clone(), chunks.clone());
        chunks
    }
}

impl Client for MemClient {
    /// 实现 kv.Client.Send 契约：返回可迭代 Response，vars 在此桩中忽略。
    fn Send(
        &self,
        _ctx: &Context,
        req: &Request,
        _vars: &crate::Variables,
    ) -> Result<Option<Box<dyn Response>>, Error> {
        // 计数 Send 便于排查并发路径；响应体 Marshal 后流式交给 TrackingResponse。
        // chain(None) 保证消费者读到明确 EOF，与 Go 侧流协议一致。
        self.send_calls.fetch_add(1, Ordering::SeqCst);
        let chunks = self
            .chunks_for(req)
            .into_iter()
            .map(|r| Some(r.Marshal().unwrap()))
            .chain(std::iter::once(None))
            .collect::<Vec<_>>();
        let close_calls = self.close_calls.clone();
        Ok(Some(Box::new(TrackingResponse {
            inner: MemResponse {
                chunks,
                idx: 0,
                closed: false,
            },
            close_calls,
        })))
    }
}

/// 包装 MemResponse，在 Close 时累加 close_calls，便于断言资源释放。
struct TrackingResponse {
    inner: MemResponse,
    close_calls: Arc<AtomicUsize>,
}

impl Response for TrackingResponse {
    fn NextRaw(&mut self, ctx: &Context) -> Result<Option<Vec<u8>>, Error> {
        self.inner.NextRaw(ctx)
    }

    fn Close(&mut self) -> Result<(), Error> {
        // 先计数再委托，保证即使 Close 失败也能观察到尝试关闭。
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.Close()
    }
}

/// 无索引、非分区、非 common-handle 的最小表元数据夹具。
fn table_plain(id: i64, name: &str) -> TableInfo {
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Indices: vec![],
        Partition: None,
        IsCommonHandle: false,
    }
}

/// 单条 StatePublic 二级索引夹具，驱动表+索引双请求（Len==2）。
fn table_with_index(id: i64, name: &str, idx_id: i64, idx_name: &str) -> TableInfo {
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Indices: vec![IndexInfo {
            ID: idx_id,
            Name: CIStr::new(idx_name),
            State: StatePublic,
        }],
        Partition: None,
        IsCommonHandle: false,
    }
}

/// Corresponds to Go `distsql.BuildTableRanges` for a common-handle table:
/// record prefix + FullNotNullRange (low=0x01, high=0xfb).
///
/// 用 RequestBuilder.SetHandleRanges 复现 Go distsql.BuildTableRanges 首分区范围，
/// 供 common-handle 场景与 Executor 生成的 KeyRanges 对比。
fn build_table_ranges_common_handle(table_id: i64) -> Vec<crate::KeyRange> {
    // FullNotNullRange：common-handle 非空主键的标准扫描区间。
    let ranges = FullNotNullRange();
    let mut builder = RequestBuilder::default();
    // is_common_handle=true，与 Go BuildTableRanges(tableInfo3) 语义对齐。
    builder.SetHandleRanges(None, table_id, true, ranges);
    builder.Request.KeyRanges.ranges
}

/// Go `uint64` arithmetic wraps modulo 2^64; Rust aggregation must retain that contract.
#[test]
fn test_update_checksum_response_wraps_counters_like_go() {
    let mut resp = ChecksumResponse {
        Checksum: 0b1010,
        TotalKvs: u64::MAX,
        TotalBytes: u64::MAX - 1,
    };
    let update = ChecksumResponse {
        Checksum: 0b1100,
        TotalKvs: 1,
        TotalBytes: 2,
    };

    updateChecksumResponse(&mut resp, &update);

    assert_eq!(0b0110, resp.Checksum);
    assert_eq!(0, resp.TotalKvs);
    assert_eq!(0, resp.TotalBytes);
}

/// Corresponds to Go `TestChecksumContextDone`.
///
/// 意图：Execute 前取消 Context，必须返回取消类错误而非成功响应。
/// Go 用 mock 集群+真实 SQL；此处用带索引的 TableInfo + MemClient 等价施压。
#[test]
fn test_checksum_context_done() {
    // Fixture mirrors SQL: create table t1 (a int, b int, key i1(a, b), primary key (a)); insert (10,10);
    // 静态元数据替代 getTableInfo；并发度与 Go DefChecksumTableConcurrency 对齐。
    let table_info = table_with_index(1, "t1", 1, "i1");
    let exe = NewExecutorBuilder(table_info, u64::MAX)
        .SetConcurrency(DEF_CHECKSUM_TABLE_CONCURRENCY)
        .Build()
        .expect("Build");

    // 立即 cancel，模拟 Go 在 Execute 前调用 cancel()。
    let (cctx, cancel) = Context::WithCancel(&Context::Background());
    cancel.cancel();

    // 跳过退避睡眠，避免取消路径被 sleep 拖慢测试。
    set_skip_backoff_sleep(true);
    let client = MemClient::dummy(exe.Len());
    let err = exe
        .Execute(&cctx, &client, || {
            // Go t.Log("request done")
        })
        .expect_err("context cancel after checksum Execute must error");
    set_skip_backoff_sleep(false);
    // 接受包装后的取消文案或裸 cancel 子串，兼容错误包装差异。
    assert!(
        err.msg.contains("context is cancelled by other error") || err.msg.contains("cancel"),
        "got {}",
        err.msg
    );
}

/// Corresponds to Go `TestChecksum`.
///
/// 覆盖：裸表 checksum、表+索引 XOR 聚合、rewrite Rule、common-handle 范围、
/// failpoint 启停后仍可成功 Execute。各子场景断言与 Go 同序。
#[test]
fn test_checksum() {
    // 全用例关闭退避，保证断言时序稳定。
    set_skip_backoff_sleep(true);

    // --- t1: plain table, concurrency override, dummy checksum all-1s ---
    // 对应 Go：create table t1 (a int); 仅一条表扫描请求。
    let table_info_1 = table_plain(1, "t1");
    let exe1 = NewExecutorBuilder(table_info_1.clone(), u64::MAX)
        .SetConcurrency(DEF_CHECKSUM_TABLE_CONCURRENCY)
        .Build()
        .expect("Build t1");
    // Each：校验 NotFillCache 与并发度被写入每个 kv.Request。
    exe1.Each(|req: &Request| {
        assert!(req.NotFillCache);
        assert_eq!(DEF_CHECKSUM_TABLE_CONCURRENCY as i32, req.Concurrency);
        Ok(())
    })
    .expect("Each t1");
    assert_eq!(1, exe1.Len());

    let client1 = MemClient::dummy(1);
    let resp = exe1
        .Execute(&Context::TODO(), &client1, || {})
        .expect("Execute t1");
    // Cluster returns a dummy checksum (all fields are 1).
    // 单请求无 XOR，三项均为 1。
    assert_eq!(1_u64, resp.Checksum, "{resp:?}");
    assert_eq!(1_u64, resp.TotalKvs, "{resp:?}");
    assert_eq!(1_u64, resp.TotalBytes, "{resp:?}");

    // --- t2: table + public index → Len==2; XOR of two dummy 1s ---
    // 对应 Go：t2 加索引后两条请求；Checksum 1^1=0，Kvs/Bytes 累加为 2。
    let table_info_2 = table_with_index(2, "t2", 10, "i2");
    let mut exe2 = NewExecutorBuilder(table_info_2.clone(), u64::MAX)
        .Build()
        .expect("Build t2");
    assert_eq!(2, exe2.Len(), "{table_info_2:?}");
    let client2 = MemClient::dummy(2);
    let resp2 = exe2
        .Execute(&Context::TODO(), &client2, || {})
        .expect("Execute t2");
    assert_eq!(0_u64, resp2.Checksum, "{resp2:?}"); // 1 ^ 1
    assert_eq!(2_u64, resp2.TotalKvs, "{resp2:?}");
    assert_eq!(2_u64, resp2.TotalBytes, "{resp2:?}");

    // --- rewrite rules: old table = t1+index, new = t2 ---
    // SetOldTable 后每条 RawRequest 必须带 Rule；ScanOn 依次为 Table / Index。
    let table_info_1_with_idx = table_with_index(1, "t1", 10, "i2");
    let old_table = MetaTable {
        Info: table_info_1_with_idx,
    };
    exe2 = NewExecutorBuilder(table_info_2.clone(), u64::MAX)
        .SetOldTable(old_table)
        .Build()
        .expect("Build rewrite");
    assert_eq!(2, exe2.Len());
    let raw_reqs = exe2.RawRequests().expect("RawRequests");
    assert_eq!(2, raw_reqs.len());
    for raw_req in &raw_reqs {
        assert!(raw_req.Rule.is_some(), "rewrite rule on each raw request");
    }
    // 请求顺序固定：先表扫描再索引扫描，与 Builder 展开规则一致。
    assert_eq!(raw_reqs[0].ScanOn, ChecksumScanOn::Table);
    assert_eq!(raw_reqs[1].ScanOn, ChecksumScanOn::Index);
    let client_rw = MemClient::dummy(2);
    let resp_rw = exe2
        .Execute(&Context::TODO(), &client_rw, || {})
        .expect("Execute rewrite");
    // 此处仅确认 Execute 成功返回，对应 Go require.NotNil。
    let _ = resp_rw; // Go require.NotNil

    // --- commonHandle ranges ---
    // 对应 Go CLUSTERED PK；首请求 KeyRanges 须对齐 BuildTableRanges 首分区，
    // StartKey = 表记录前缀 + 0x00 下界。
    let mut table_info_3 = table_plain(3, "t3");
    table_info_3.IsCommonHandle = true;
    let exe3 = NewExecutorBuilder(table_info_3.clone(), u64::MAX)
        .Build()
        .expect("Build t3");
    let mut first = true;
    exe3.Each(|req: &Request| {
        if first {
            first = false;
            let ranges = build_table_ranges_common_handle(table_info_3.ID);
            assert_eq!(
                &ranges[..1],
                req.KeyRanges.FirstPartitionRange(),
                "common handle first req matches BuildTableRanges first partition"
            );
            // sanity: prefix + codec MinNotNull flag (0x01) low bound
            let prefix = GenTableRecordPrefix(table_info_3.ID);
            assert!(
                req.KeyRanges.FirstPartitionRange()[0]
                    .StartKey
                    .starts_with(&prefix)
            );
            assert!(
                req.KeyRanges.FirstPartitionRange()[0]
                    .StartKey
                    .ends_with(&[0x01])
            );
        }
        Ok(())
    })
    .expect("Each t3");

    // --- failpoint enable then disable (Go order), then Execute succeeds ---
    // 先 Enable 再 Disable（与 Go 同序），确认关闭后 Execute 不再注入错误。
    let exe4 = NewExecutorBuilder(table_info_3, u64::MAX)
        .Build()
        .expect("Build retry");
    // true→false 对应 Go Enable 后立即 Disable，确保不会在 Execute 中触发重试错误。
    inject_checksum_retry_err(true);
    inject_checksum_retry_err(false); // Go failpoint.Disable before Execute
    let client4 = MemClient::dummy(exe4.Len());
    let resp4 = exe4
        .Execute(&Context::TODO(), &client4, || {})
        .expect("Execute after failpoint disable");
    let _ = resp4;

    // 恢复默认退避行为，避免影响同进程其他测试。
    set_skip_backoff_sleep(false);
}
