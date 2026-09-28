// Copyright 2026 AsterSQL.

//! Go/Rust 公开契约对等测试：覆盖 checksum Executor 的正常、边界、错误与重试路径。
//!
//! 模块职责：在无真实 TiKV/mock.Cluster 时，用 MemClient + 静态 TableInfo
//! 固定与 Go `br/pkg/checksum` 一致的可观察行为。
//! 数据流：Builder 展开请求 → Client.Send → 流式 Response → XOR/累加聚合。
//! 约束：退避可关闭；failpoint 与 Close 错误注入仅作用于测试桩，不代表生产能力。
//! 夹具：MemResponse 可注入 close_err；MemClient 支持首 N 次 Send 失败与按 Data 缓存。
//! 场景索引：默认并发、索引展开、rewrite Rule、RequestSource、非 Public 索引跳过、
//! common-handle 范围、updateChecksumResponse、Context 取消、分区 rewrite 失败、
//! failpoint 重试、Close 覆盖成功、updateFn 按请求计数。
//! 与 executor_test 的差异：本文件聚焦契约矩阵与错误注入组合，而非 SQL 夹具复刻。
//! 断言优先比对聚合结果、请求元数据与错误文案，不依赖真实 store 拓扑。
//! 若扩展新公开 API，应在此测试追加对应子场景，避免 Go/Rust 行为漂移。
//! 本文件不启动集群，所有成功/失败均由内存桩可控复现。
//! MemClient 按请求 Data 缓存响应，便于断言 rewrite 后的键范围。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::executor::{Executor, NewExecutorBuilder, checkContextDone, updateChecksumResponse};
use crate::stubs::{
    CIStr, ChecksumAlgorithm, ChecksumRequest, ChecksumResponse, ChecksumScanOn, Client, Context,
    DefDistSQLScanConcurrency, EncodeInt, EncodeTableIndexPrefix, Error, GenTableRecordPrefix,
    IndexInfo, MetaTable, Request, RequestSource, Response, StatePublic, TableInfo, Variables,
    inject_checksum_retry_err, set_skip_backoff_sleep,
};

/// 内存 kv.Response：按序产出 Marshal 后的 checksum chunk。
/// close_err 用于模拟 Close 失败覆盖成功路径的契约。
struct MemResponse {
    chunks: Vec<Option<Vec<u8>>>,
    idx: usize,
    closed: bool,
    close_err: Option<Error>,
}

// NextRaw 耗尽返回 None；Close 可一次性取出 close_err。
impl Response for MemResponse {
    fn NextRaw(&mut self, _ctx: &Context) -> Result<Option<Vec<u8>>, Error> {
        // 流结束哨兵，与 Go Response 读尽语义一致。
        if self.idx >= self.chunks.len() {
            return Ok(None);
        }
        let item = self.chunks[self.idx].clone();
        self.idx += 1;
        Ok(item)
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.closed = true;
        // take() 保证 close 错误只表面一次。
        if let Some(err) = self.close_err.take() {
            return Err(err);
        }
        Ok(())
    }
}

/// 替代 Go mock Storage.GetClient：按请求绑定响应并记录观测字段。
/// fail_first_n / close_err / last_vars 分别服务重试、关闭失败与退避权重断言。
struct MemClient {
    /// Unbound per-request response templates (popped on first sight of req.Data).
    pending: Mutex<Vec<Vec<ChecksumResponse>>>,
    /// Bound templates reused across retries of the same request.
    by_data: Mutex<HashMap<Vec<u8>, Vec<ChecksumResponse>>>,
    /// Send 调用次数，用于断言 failpoint 触发后的重试。
    send_calls: AtomicUsize,
    /// Response.Close 累计次数，校验资源释放。
    close_calls: Arc<AtomicUsize>,
    /// 前 N 次 Send 返回瞬时错误，驱动退避重试。
    fail_first_n: Mutex<usize>,
    /// 注入到后续 Response.Close 的错误（可覆盖成功）。
    close_err: Mutex<Option<Error>>,
    /// 最近一次 Send 收到的 Variables，核对 BackOffWeight 传递。
    last_vars: Mutex<Option<Variables>>,
}

impl MemClient {
    /// 按请求次序提供响应模板；首次见到 req.Data 时绑定并缓存。
    fn new(per_req: Vec<Vec<ChecksumResponse>>) -> Self {
        Self {
            pending: Mutex::new(per_req),
            by_data: Mutex::new(HashMap::new()),
            send_calls: AtomicUsize::new(0),
            close_calls: Arc::new(AtomicUsize::new(0)),
            fail_first_n: Mutex::new(0),
            close_err: Mutex::new(None),
            last_vars: Mutex::new(None),
        }
    }

    /// n_reqs 个请求共享同一响应体模板。
    fn with_uniform(n_reqs: usize, resp: ChecksumResponse) -> Self {
        Self::new(vec![vec![resp]; n_reqs])
    }

    /// 优先 by_data 命中（重试稳定）；否则 pending FIFO；空则默认零值。
    fn chunks_for(&self, req: &Request) -> Vec<ChecksumResponse> {
        let mut by_data = self.by_data.lock().unwrap();
        if let Some(chunks) = by_data.get(&req.Data) {
            return chunks.clone();
        }
        let mut pending = self.pending.lock().unwrap();
        // 超额 Send 回落默认响应，避免测试因长度估计偏差直接 panic。
        let chunks = if pending.is_empty() {
            vec![ChecksumResponse::default()]
        } else {
            pending.remove(0)
        };
        by_data.insert(req.Data.clone(), chunks.clone());
        chunks
    }
}

// 实现 Client::Send：记录 vars、可选失败注入，再封装 TrackingResponse。
impl Client for MemClient {
    fn Send(
        &self,
        _ctx: &Context,
        req: &Request,
        vars: &Variables,
    ) -> Result<Option<Box<dyn Response>>, Error> {
        // 保留 Variables 供 BackOffWeight 等字段事后断言。
        *self.last_vars.lock().unwrap() = Some(vars.clone());
        self.send_calls.fetch_add(1, Ordering::SeqCst);
        {
            let mut n = self.fail_first_n.lock().unwrap();
            // 瞬时错误路径：不消费 pending，便于重试后成功。
            if *n > 0 {
                *n -= 1;
                return Err(Error::new("transient send error"));
            }
        }
        let chunks = self
            .chunks_for(req)
            .into_iter()
            .map(|r| Some(r.Marshal().unwrap()))
            .chain(std::iter::once(None))
            .collect::<Vec<_>>();
        let close_err = self.close_err.lock().unwrap().clone();
        Ok(Some(Box::new(TrackingResponse {
            inner: MemResponse {
                chunks,
                idx: 0,
                closed: false,
                close_err,
            },
            close_calls: self.close_calls.clone(),
        })))
    }
}

/// 包装 MemResponse，Close 时累加 close_calls。
struct TrackingResponse {
    inner: MemResponse,
    /// Response.Close 累计次数，校验资源释放。
    close_calls: Arc<AtomicUsize>,
}

impl Response for TrackingResponse {
    fn NextRaw(&mut self, ctx: &Context) -> Result<Option<Vec<u8>>, Error> {
        self.inner.NextRaw(ctx)
    }

    fn Close(&mut self) -> Result<(), Error> {
        // 先计数再委托，确保观察到关闭尝试。
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.Close()
    }
}

/// 无索引/非分区/非 common-handle 的最小表夹具。
fn table_plain(id: i64, name: &str) -> TableInfo {
    TableInfo {
        ID: id,
        Name: CIStr::new(name),
        Indices: vec![],
        Partition: None,
        IsCommonHandle: false,
    }
}

/// 含一条 StatePublic 二级索引，驱动表+索引双请求。
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

/// Go `FullIntRange(false)` is converted by `encodeHandleKey`: both bounds use
/// `codec.EncodeInt`, and the inclusive MaxInt64 upper bound uses `PrefixNext`.
#[test]
fn integer_handle_range_matches_go_codec_boundaries() {
    let table_id = 42;
    let executor = NewExecutorBuilder(table_plain(table_id, "ints"), 1)
        .Build()
        .expect("build integer handle request");

    executor
        .Each(|request| {
            let range = &request.KeyRanges.FirstPartitionRange()[0];
            let prefix = GenTableRecordPrefix(table_id);

            let mut expected_start = prefix.clone();
            expected_start.extend_from_slice(&EncodeInt(Vec::new(), i64::MIN));

            let mut encoded_high = EncodeInt(Vec::new(), i64::MAX);
            // EncodeInt(MaxInt64) is eight 0xff bytes. Go kv.Key.PrefixNext
            // therefore appends 0x00 when the inclusive upper bound is raised.
            encoded_high.push(0);
            let mut expected_end = prefix;
            expected_end.extend_from_slice(&encoded_high);

            assert_eq!(range.StartKey, expected_start);
            assert_eq!(range.EndKey, expected_end);
            Ok(())
        })
        .expect("inspect integer handle request");
}

/// Go `EncodeIndexKey` encodes Null/MinNotNull/MaxValue using codec flags
/// 0x00/0x01/0xfa, then raises the inclusive MaxValue bound to 0xfb.
#[test]
fn index_and_common_handle_ranges_match_go_codec_flags() {
    let mut common_table = table_plain(43, "common");
    common_table.IsCommonHandle = true;
    let common = NewExecutorBuilder(common_table, 1).Build().unwrap();
    common
        .Each(|request| {
            let range = &request.KeyRanges.FirstPartitionRange()[0];
            let mut expected_start = GenTableRecordPrefix(43);
            expected_start.push(0x01);
            let mut expected_end = GenTableRecordPrefix(43);
            expected_end.push(0xfb);
            assert_eq!(range.StartKey, expected_start);
            assert_eq!(range.EndKey, expected_end);
            Ok(())
        })
        .unwrap();

    let indexed = NewExecutorBuilder(table_with_index(44, "indexed", 7, "i"), 1)
        .Build()
        .unwrap();
    let mut request_number = 0;
    indexed
        .Each(|request| {
            if request_number == 1 {
                let range = &request.KeyRanges.FirstPartitionRange()[0];
                let mut expected_start = EncodeTableIndexPrefix(44, 7);
                expected_start.push(0x00);
                let mut expected_end = EncodeTableIndexPrefix(44, 7);
                expected_end.push(0xfb);
                assert_eq!(range.StartKey, expected_start);
                assert_eq!(range.EndKey, expected_end);
            }
            request_number += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(request_number, 2);
}

struct ReadAndCloseErrorResponse;

impl Response for ReadAndCloseErrorResponse {
    fn NextRaw(&mut self, _ctx: &Context) -> Result<Option<Vec<u8>>, Error> {
        Err(Error::new("read failed"))
    }

    fn Close(&mut self) -> Result<(), Error> {
        Err(Error::new("close failed"))
    }
}

struct ReadAndCloseErrorClient;

impl Client for ReadAndCloseErrorClient {
    fn Send(
        &self,
        _ctx: &Context,
        _req: &Request,
        _vars: &Variables,
    ) -> Result<Option<Box<dyn Response>>, Error> {
        Ok(Some(Box::new(ReadAndCloseErrorResponse)))
    }
}

/// Go's named-return defer assigns `res.Close()` errors even when `NextRaw`
/// already failed, so the close error is the retry-visible error.
#[test]
fn close_error_overrides_response_read_error() {
    set_skip_backoff_sleep(true);
    let executor = NewExecutorBuilder(table_plain(45, "errors"), 1)
        .Build()
        .unwrap();
    let error = executor
        .Execute(&Context::TODO(), &ReadAndCloseErrorClient, || {})
        .expect_err("read and close must fail");
    set_skip_backoff_sleep(false);
    assert!(error.msg.contains("close failed"), "{}", error.msg);
}

/// Go compares `ast.CIStr` values directly when locating the old index, so a
/// case-only difference in `O` must not silently match through `L`.
#[test]
#[should_panic(expected = "index not found in origin table")]
fn rewrite_requires_exact_index_cistr_match() {
    let new_table = table_with_index(46, "new", 2, "IndexName");
    let old_table = MetaTable {
        Info: table_with_index(47, "old", 3, "indexname"),
    };
    let _ = NewExecutorBuilder(new_table, 1)
        .SetOldTable(old_table)
        .Build();
}

/// A Go child context observes cancellation of its parent even when the parent
/// is cancelled after `WithCancel` returns.
#[test]
fn child_context_observes_late_parent_cancellation() {
    let parent = Context::Background();
    let (child, _cancel_child) = Context::WithCancel(&parent);
    parent.cancel(Error::new("parent canceled"));
    let error = checkContextDone(&child).expect_err("parent cancellation propagates");
    assert!(error.msg.contains("parent canceled"), "{}", error.msg);
}

/// Go protobuf unmarshalling rejects truncated unknown length-delimited fields
/// instead of silently accepting a message whose declared payload is missing.
#[test]
fn protobuf_unknown_length_delimited_fields_reject_short_payloads() {
    // Unknown field 9, wire type 2, declares two bytes but carries only one.
    let truncated = [0x4a, 0x02, 0x01];

    assert!(
        crate::stubs::ChecksumRewriteRule::Unmarshal(&truncated).is_err(),
        "rewrite rule must reject a truncated unknown field"
    );
    assert!(
        ChecksumResponse::Unmarshal(&truncated).is_err(),
        "checksum response must reject a truncated unknown field"
    );
    assert!(
        ChecksumRequest::Unmarshal(&truncated).is_err(),
        "checksum request must reject a truncated unknown field"
    );
}

#[test]
/// 综合契约测试：对齐 Go executor / nokit 用例的可观察语义集合。
/// 子场景按正常 → 边界 → 错误 → 重试 → 清理 → 回调计数排列。
fn go_rust_public_contract_matches() {
    // 正常路径：显式并发、请求标志位、单表聚合与 Close 一次。
    // --- normal: builder defaults, Len, NotFillCache, Concurrency, Execute aggregate ---
    let t1 = table_plain(1, "t1");
    let exe1 = NewExecutorBuilder(t1.clone(), u64::MAX)
        .SetConcurrency(4)
        .Build()
        .expect("build t1");
    assert_eq!(exe1.Len(), 1);
    // 校验 checksum 请求固定字段：不填缓存、低优先级、类型为 Checksum。
    exe1.Each(|r| {
        assert!(r.NotFillCache);
        assert_eq!(r.Concurrency, 4);
        assert_eq!(r.Priority, crate::stubs::PriorityLow);
        assert_eq!(r.Tp, crate::stubs::ReqTypeChecksum);
        Ok(())
    })
    .unwrap();

    let client1 = MemClient::with_uniform(
        1,
        ChecksumResponse {
            Checksum: 1,
            TotalKvs: 1,
            TotalBytes: 1,
        },
    );
    let resp1 = exe1
        .Execute(&Context::TODO(), &client1, || {})
        .expect("exec t1");
    // 单表 dummy 响应：三项均为 1，无 XOR 参与。
    assert_eq!(resp1.Checksum, 1);
    assert_eq!(resp1.TotalKvs, 1);
    assert_eq!(resp1.TotalBytes, 1);
    // 单请求成功后必须恰好 Close 一次。
    assert_eq!(client1.close_calls.load(Ordering::SeqCst), 1);

    // 表+索引：Checksum 按 XOR，Kvs/Bytes 累加。
    // table + public index => Len == 2; XOR aggregate across requests
    let t2 = table_with_index(2, "t2", 10, "i2");
    let exe2 = NewExecutorBuilder(t2.clone(), u64::MAX)
        .Build()
        .expect("build t2");
    assert_eq!(exe2.Len(), 2);
    let client2 = MemClient::new(vec![
        vec![ChecksumResponse {
            Checksum: 0x11,
            TotalKvs: 1,
            TotalBytes: 1,
        }],
        vec![ChecksumResponse {
            Checksum: 0x11,
            TotalKvs: 1,
            TotalBytes: 1,
        }],
    ]);
    let resp2 = exe2
        .Execute(&Context::TODO(), &client2, || {})
        .expect("exec t2");
    // 0x11 ^ 0x11 = 0；计数类字段按请求数相加。
    assert_eq!(resp2.Checksum, 0); // 0x11 ^ 0x11
    assert_eq!(resp2.TotalKvs, 2);
    assert_eq!(resp2.TotalBytes, 2);

    // SetOldTable：每条 RawRequest 带 Rule，算法为 Crc64_Xor。
    // rewrite rules when SetOldTable is set
    let old = MetaTable {
        Info: table_with_index(1, "t1", 10, "i2"),
    };
    let exe_rewrite = NewExecutorBuilder(t2.clone(), u64::MAX)
        .SetOldTable(old)
        .Build()
        .expect("build rewrite");
    assert_eq!(exe_rewrite.Len(), 2);
    let raw = exe_rewrite.RawRequests().expect("raw");
    assert_eq!(raw.len(), 2);
    // Rule 与算法是 rewrite 路径的硬约束，缺失即契约破坏。
    for r in &raw {
        assert!(r.Rule.is_some(), "rewrite rule required");
        assert_eq!(r.Algorithm, ChecksumAlgorithm::Crc64_Xor);
    }
    // 展开顺序：先表扫描后索引扫描。
    assert_eq!(raw[0].ScanOn, ChecksumScanOn::Table);
    assert_eq!(raw[1].ScanOn, ChecksumScanOn::Index);
    // Old/New 前缀均非空，证明 rewrite 前缀映射已生成。
    let rule0 = raw[0].Rule.as_ref().unwrap();
    assert!(
        rule0.OldPrefix.windows(1).any(|_| true)
            && !rule0.OldPrefix.is_empty()
            && !rule0.NewPrefix.is_empty()
    );

    // RequestSource：零值 → 显式类型 → 整对象替换，对齐 nokit 测试。
    // request source setters (executor_nokit_test.go)
    let mut b = NewExecutorBuilder(table_plain(9, "t"), 0);
    assert_eq!(b.request_source(), &RequestSource::default());
    b = b.SetExplicitRequestSourceType("aaa".into());
    assert_eq!(b.request_source().ExplicitRequestSourceType, "aaa");
    let src = RequestSource {
        RequestSourceInternal: true,
        RequestSourceType: "type".into(),
        ExplicitRequestSourceType: "bbb".into(),
    };
    b = b.SetRequestSource(src.clone());
    assert_eq!(b.request_source(), &src);

    // 未 SetConcurrency 时回落 DistSQL 默认扫描并发。
    // default concurrency from DefDistSQLScanConcurrency
    let exe_def = NewExecutorBuilder(table_plain(3, "t3"), 1).Build().unwrap();
    exe_def
        .Each(|r| {
            assert_eq!(r.Concurrency, DefDistSQLScanConcurrency as i32);
            Ok(())
        })
        .unwrap();

    // 边界：非 Public 索引不计入 Len；common-handle StartKey 以 0x01 结尾。
    // --- boundary: non-public index skipped; common handle ranges ---
    let mut t_skip = table_with_index(4, "t4", 1, "idx");
    // State!=Public 时索引请求被跳过，仅剩表扫描。
    t_skip.Indices[0].State = 0; // not public
    let exe_skip = NewExecutorBuilder(t_skip, 1).Build().unwrap();
    assert_eq!(exe_skip.Len(), 1);

    let mut t_ch = table_plain(5, "t5");
    // common-handle 使用 FullNotNullRange，下界字节为 MinNotNull flag 0x01。
    t_ch.IsCommonHandle = true;
    let exe_ch = NewExecutorBuilder(t_ch, 1).Build().unwrap();
    let mut first = true;
    exe_ch
        .Each(|req| {
            if first {
                first = false;
                assert!(!req.KeyRanges.FirstPartitionRange().is_empty());
                let kr = &req.KeyRanges.FirstPartitionRange()[0];
                // common-handle path uses FullNotNullRange low=0x01
                assert!(kr.StartKey.ends_with(&[0x01]));
            }
            Ok(())
        })
        .unwrap();

    // 单元契约：Checksum XOR，TotalKvs/TotalBytes 加法。
    // 0b1010 ^ 0b1100 = 0b0110；1+3=4；2+4=6。
    // updateChecksumResponse unit contract
    let mut acc = ChecksumResponse {
        Checksum: 0b1010,
        TotalKvs: 1,
        TotalBytes: 2,
    };
    updateChecksumResponse(
        &mut acc,
        &ChecksumResponse {
            Checksum: 0b1100,
            TotalKvs: 3,
            TotalBytes: 4,
        },
    );
    // 验证位运算与加法结果，防止聚合函数被误改为覆盖赋值。
    assert_eq!(acc.Checksum, 0b0110);
    assert_eq!(acc.TotalKvs, 4);
    assert_eq!(acc.TotalBytes, 6);

    // 错误路径：Execute 前取消 Context，须返回包装后的取消错误。
    // --- error: context cancelled after successful send still fails checkContextDone ---
    let exe_ctx = NewExecutorBuilder(table_plain(6, "t6"), 1).Build().unwrap();
    let (cctx, cancel) = Context::WithCancel(&Context::Background());
    cancel.cancel();
    let client_ctx = MemClient::with_uniform(
        1,
        ChecksumResponse {
            Checksum: 1,
            TotalKvs: 1,
            TotalBytes: 1,
        },
    );
    let err = exe_ctx
        .Execute(&cctx, &client_ctx, || {})
        .expect_err("context done");
    // 文案与 Go 包装错误对齐，避免仅匹配裸 cancel 导致假阴性。
    assert!(
        err.msg.contains("context is cancelled by other error"),
        "got {}",
        err.msg
    );

    // 直接探测 checkContextDone：Background 成功，cancel 后失败。
    // checkContextDone alone
    assert!(checkContextDone(&Context::Background()).is_ok());
    // 手动 cancel 注入原因后，checkContextDone 必须失败。
    let cancelled = Context::Background();
    cancelled.cancel(Error::new("context canceled"));
    let e = checkContextDone(&cancelled).unwrap_err();
    assert!(e.msg.contains("context is cancelled by other error"));

    // 分区表 rewrite 时旧表缺同名分区 → Build 失败（保留 Go 拼写 parition）。
    // missing partition name when rewriting partitioned table
    let mut part_table = table_plain(7, "tp");
    part_table.Partition = Some(crate::stubs::PartitionInfo {
        Definitions: vec![crate::stubs::PartitionDefinition {
            ID: 70,
            Name: CIStr::new("p0"),
        }],
    });
    let old_no_part = MetaTable {
        Info: table_plain(8, "tp_old"),
    };
    let err = match NewExecutorBuilder(part_table, 1)
        .SetOldTable(old_no_part)
        .Build()
    {
        Ok(_) => panic!("expected partition missing error"),
        Err(e) => e,
    };
    // 兼容历史拼写 "parition" 与更正后的 "partition is not found"。
    assert!(
        err.msg.contains("does not have parition") || err.msg.contains("partition is not found"),
        "{}",
        err.msg
    );

    // 重试：注入 checksumRetryErr 后仍应成功，且 Send>=2、BackOffWeight=3。
    // --- retry + failpoint inject then success ---
    // 关闭退避睡眠以稳定耗时；用例结束须恢复。
    set_skip_backoff_sleep(true);
    // 对应 Go failpoint checksumRetryErr：至少触发一次重试。
    inject_checksum_retry_err(true);
    let exe_retry = NewExecutorBuilder(table_plain(11, "tr"), 1)
        .SetBackoffWeight(3)
        .Build()
        .unwrap();
    let client_retry = MemClient::with_uniform(
        1,
        ChecksumResponse {
            Checksum: 9,
            TotalKvs: 2,
            TotalBytes: 3,
        },
    );
    let resp_retry = exe_retry
        .Execute(&Context::TODO(), &client_retry, || {})
        .expect("retry then ok");
    // 注入错误后最终结果仍为客户端响应值 9。
    assert_eq!(resp_retry.Checksum, 9);
    // Send 至少两次证明发生了重试而非首次侥幸成功。
    assert!(client_retry.send_calls.load(Ordering::SeqCst) >= 2);
    // Builder.SetBackoffWeight(3) 必须透传到 Send 的 Variables。
    assert_eq!(
        client_retry
            .last_vars
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .BackOffWeight,
        3
    );
    // 恢复退避，避免污染后续用例。
    set_skip_backoff_sleep(false);

    // Close 失败必须覆盖已读成功的 checksum，表面 close failed。
    // --- resource cleanup: Close error overrides success ---
    let exe_close = NewExecutorBuilder(table_plain(12, "tc"), 1)
        .Build()
        .unwrap();
    let client_close = MemClient::with_uniform(
        1,
        ChecksumResponse {
            Checksum: 1,
            TotalKvs: 1,
            TotalBytes: 1,
        },
    );
    // 注入 Close 错误后再 Execute，断言错误文案与至少一次 Close。
    *client_close.close_err.lock().unwrap() = Some(Error::new("close failed"));
    // Close 错误场景同样跳过退避，避免测试抖动。
    set_skip_backoff_sleep(true);
    let err = exe_close
        .Execute(&Context::TODO(), &client_close, || {})
        .expect_err("close err");
    // 允许重试耗尽后仍以 close failed 为最终错误。
    // retries exhaust or surface close failed
    assert!(
        err.msg.contains("close failed"),
        "unexpected err {}",
        err.msg
    );
    assert!(client_close.close_calls.load(Ordering::SeqCst) >= 1);
    // 清理退避开关，与重试段对称。
    set_skip_backoff_sleep(false);

    // updateFn 每完成一个请求回调一次；双请求表+索引故为 2。
    // updateFn invoked once per request
    let exe_fn = NewExecutorBuilder(table_with_index(13, "tf", 1, "i"), 1)
        .Build()
        .unwrap();
    let client_fn = MemClient::with_uniform(
        2,
        ChecksumResponse {
            Checksum: 0,
            TotalKvs: 1,
            TotalBytes: 1,
        },
    );
    // 原子计数器避免闭包捕获可变性困扰。
    let calls = AtomicUsize::new(0);
    exe_fn
        .Execute(&Context::TODO(), &client_fn, || {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    // Len==2 时回调次数必须为 2，防止漏调或重复调。
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
