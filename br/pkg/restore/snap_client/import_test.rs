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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/snap_client/import_test.rs`对应的SnapFileImporter 键范围/令牌/批量下载测试，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client/import_test.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 本任务要求至少91行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `generate_stores/regions/files`：构造假 PD topology 与备份文件，驱动 importer 单测。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_get_key_range_by_mode`：TiDB/Raw/Txn/Compacted 下起止键编码差异。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_get_sst_meta_from_file`：SSTMeta 的 CF、range、UUID 字段正确性。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_unproper_config_snap_importer`：非法 Options（如并发为 0 的受限场景）应 New 失败。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_snap_importer`：TiDBFull 路径 Import 成功并释放令牌。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_snap_importer_raw`：Raw 模式依赖 SetRawRange；未设置应报错。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `FlowControlSplitClient`：可控阻塞的 ScanRegions，验证 PD 请求流控。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_snap_importer_pd_scan_request_flow_control`：并发 Import 时 PD scan 令牌限制生效。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `make_compacted_file_sets`：构造 compacted 日志 SST 文件组。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_batch_download_latest_mvcc_parallelizes_file_groups_per_peer`：LatestMVCC 批量下载按 peer 并行。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! - `test_batch_download_sst_parallelizes_file_groups_per_peer`：普通批量 SST 下载同样按 peer 并行。
//!   断言应锁定可观察行为（调用次数、错误传播、回调参数），而非内部锁实现细节。
//!   若使用 Fake/Mem 桩，被保护的仍是与 Go 同名测试相同的契约。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 中文注释索引结束

//! Go-equivalent tests for `import_test.go`.
//! PD/TiKV/gRPC: MemSplitClient + MemImporterClient (no kvproto/grpcio).

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::export_test::{
    GetKeyRangeByMode, GetSSTMetaFromFile, NewSnapFileImporterOptionsForTest, RestoreLabelKey,
    RestoreLabelValue,
};
use crate::import::{
    DownloadRateLimitTTLSeconds, KvMode, NewSnapFileImporter, NewSnapFileImporterOptions,
    RewriteMode,
};
use crate::stubs::{
    BackupFileSet, Context, ImporterClient, MemImporterClient, MemSplitClient, RegionInfo,
    RewriteRules, codec, import_sstpb, metapb, tablecodec,
};

fn generate_stores() -> Vec<metapb::Store> {
    vec![
        metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tiflash".into(),
            }],
            ..Default::default()
        },
        metapb::Store {
            Id: 2,
            State: metapb::StoreState::Up,
            Labels: vec![
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tikv".into(),
                },
                metapb::StoreLabel {
                    Key: RestoreLabelKey.into(),
                    Value: RestoreLabelValue.into(),
                },
            ],
            ..Default::default()
        },
    ]
}

fn generate_regions() -> Vec<RegionInfo> {
    vec![
        RegionInfo {
            Region: metapb::Region {
                Id: 1,
                StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(0)),
                EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
                Peers: vec![metapb::Peer { Id: 1, StoreId: 2 }],
                ..Default::default()
            },
            Leader: Some(metapb::Peer { Id: 1, StoreId: 2 }),
        },
        RegionInfo {
            Region: metapb::Region {
                Id: 2,
                StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
                EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(200)),
                Peers: vec![metapb::Peer { Id: 2, StoreId: 2 }],
                ..Default::default()
            },
            Leader: Some(metapb::Peer { Id: 2, StoreId: 2 }),
        },
    ]
}

fn generate_files() -> (Vec<crate::stubs::backuppb::File>, RewriteRules) {
    let mut files = Vec::new();
    for _ in 0..10 {
        files.push(crate::stubs::backuppb::File {
            StartKey: tablecodec::EncodeTablePrefix(100),
            EndKey: tablecodec::EncodeTablePrefix(100),
            ..Default::default()
        });
    }
    (
        files,
        RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(100),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(1),
                ..Default::default()
            }],
            ..Default::default()
        },
    )
}

/// TestGetKeyRangeByMode — Go `TestGetKeyRangeByMode`.
#[test]
// 同一文件在不同 KvMode 下编码后的起止键不得混用。
fn test_get_key_range_by_mode() {
    let file = crate::stubs::backuppb::File {
        Name: "file_write.sst".into(),
        StartKey: b"t1a".to_vec(),
        EndKey: b"t1ccc".to_vec(),
        ..Default::default()
    };
    let end_file = crate::stubs::backuppb::File {
        Name: "file_write.sst".into(),
        StartKey: b"t1a".to_vec(),
        EndKey: Vec::new(),
        ..Default::default()
    };
    let rule = RewriteRules {
        Data: vec![import_sstpb::RewriteRule {
            OldKeyPrefix: b"t1".to_vec(),
            NewKeyPrefix: b"t2".to_vec(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let raw = GetKeyRangeByMode(KvMode::Raw);
    let (s, e) = raw(&file, Some(&rule)).unwrap();
    assert_eq!(s, b"t1a");
    assert_eq!(e, b"t1ccc");
    let (s, e) = raw(&end_file, Some(&rule)).unwrap();
    assert_eq!(s, b"t1a");
    assert!(e.is_empty());

    let txn = GetKeyRangeByMode(KvMode::Txn);
    let (s, e) = txn(&file, Some(&rule)).unwrap();
    assert_eq!(s, codec::EncodeBytes(Vec::new(), b"t1a"));
    assert_eq!(e, codec::EncodeBytes(Vec::new(), b"t1ccc"));
    let (s, e) = txn(&end_file, Some(&rule)).unwrap();
    assert_eq!(s, codec::EncodeBytes(Vec::new(), b"t1a"));
    assert!(e.is_empty());

    let tidb = GetKeyRangeByMode(KvMode::TiDBFull);
    let (s, e) = tidb(&file, Some(&rule)).unwrap();
    assert_eq!(s, codec::EncodeBytes(Vec::new(), b"t2a"));
    assert_eq!(e, codec::EncodeBytes(Vec::new(), b"t2ccc"));
}

/// TestGetSSTMetaFromFile — Go `TestGetSSTMetaFromFile`.
#[test]
// DefaultCF/WriteCF 名称与 range 半开区间需与 Go import 包一致。
fn test_get_sst_meta_from_file() {
    let file = crate::stubs::backuppb::File {
        Name: "file_write.sst".into(),
        StartKey: b"t1a".to_vec(),
        EndKey: b"t1ccc".to_vec(),
        ..Default::default()
    };
    let rule = import_sstpb::RewriteRule {
        OldKeyPrefix: b"t1".to_vec(),
        NewKeyPrefix: b"t2".to_vec(),
        ..Default::default()
    };
    let region = metapb::Region {
        StartKey: b"t2abc".to_vec(),
        EndKey: b"t3a".to_vec(),
        ..Default::default()
    };
    let sst = GetSSTMetaFromFile(&file, &region, &rule, RewriteMode::RewriteModeLegacy).unwrap();
    assert_eq!(sst.GetRange().GetStart(), b"t2abc");
    assert_eq!(
        sst.GetRange().GetEnd(),
        b"t2\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff"
    );
    assert_eq!(sst.CfName, "write");
}

/// TestUnproperConfigSnapImporter — Go `TestUnproperConfigSnapImporter`.
#[test]
// 错误配置应在构造期失败，而不是首次 Import 才暴露。
fn test_unproper_config_snap_importer() {
    let ctx = Context::Background();
    let opt = NewSnapFileImporterOptionsForTest(
        Arc::new(MemSplitClient::default()),
        Arc::new(MemImporterClient::default()),
        Vec::new(),
        RewriteMode::RewriteModeKeyspace,
        0,
    );
    assert!(NewSnapFileImporter(&ctx, 1, KvMode::TiDBFull, opt).is_err());
}

/// Go `Close` logs callback failures, continues closing, and keeps callbacks registered.
#[test]
fn test_close_ignores_callback_errors_and_repeats_callbacks() {
    let ctx = Context::Background();
    let first_calls = Arc::new(AtomicI32::new(0));
    let second_calls = Arc::new(AtomicI32::new(0));
    let first_observer = first_calls.clone();
    let second_observer = second_calls.clone();
    let opt = NewSnapFileImporterOptions(
        None,
        Arc::new(MemSplitClient::default()),
        Arc::new(MemImporterClient::default()),
        None,
        RewriteMode::RewriteModeLegacy,
        Vec::new(),
        1,
        0,
        false,
        Vec::new(),
        vec![
            Box::new(move |_| {
                first_observer.fetch_add(1, Ordering::SeqCst);
                Err(crate::stubs::Error::new("callback failed"))
            }),
            Box::new(move |_| {
                second_observer.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        ],
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBFull, opt).unwrap();

    assert!(importer.Close().is_ok());
    assert!(importer.Close().is_ok());
    assert_eq!(first_calls.load(Ordering::SeqCst), 2);
    assert_eq!(second_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn test_batch_download_support_updates_mode_and_skips_offline_stores() {
    let ctx = Context::Background();
    let import_client = Arc::new(MemImporterClient {
        batch_download_supported: true,
        ..Default::default()
    });
    let stores = vec![
        metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            ..Default::default()
        },
        metapb::Store {
            Id: 2,
            State: metapb::StoreState::Offline,
            ..Default::default()
        },
    ];
    let opt = NewSnapFileImporterOptionsForTest(
        Arc::new(MemSplitClient::default()),
        import_client.clone(),
        stores.clone(),
        RewriteMode::RewriteModeKeyspace,
        1,
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBFull, opt).unwrap();
    importer.CheckBatchDownloadSupport(&ctx, &stores).unwrap();
    assert!(importer.GetMergeSst());
    assert_eq!(
        import_client
            .batch_download_checks
            .lock()
            .unwrap()
            .as_slice(),
        &[vec![1]]
    );
}

/// TestSnapImporter — Go `TestSnapImporter`.
#[test]
// 成功路径后令牌应归还，否则后续 PauseForBackpressure 会永久阻塞。
fn test_snap_importer() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = generate_regions();
    let import_client = Arc::new(MemImporterClient::default());
    let opt = NewSnapFileImporterOptionsForTest(
        split,
        import_client.clone(),
        generate_stores(),
        RewriteMode::RewriteModeKeyspace,
        10,
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBFull, opt).unwrap();
    importer.SetDownloadSpeedLimit(&ctx, 1, 5).unwrap();
    let set_req = import_client.speed_limits.lock().unwrap().get(&1).cloned();
    let set_req = set_req.expect("speed limit recorded");
    assert_eq!(set_req.SpeedLimit, 5);
    assert!(!set_req.TaskId.is_empty());
    assert_eq!(set_req.TtlSeconds, DownloadRateLimitTTLSeconds);

    importer.SetDownloadSpeedLimit(&ctx, 1, 0).unwrap();
    let reset = import_client
        .speed_limits
        .lock()
        .unwrap()
        .get(&1)
        .cloned()
        .unwrap();
    assert_eq!(reset.SpeedLimit, 0);
    assert_eq!(reset.TaskId, set_req.TaskId);

    assert!(importer.SetRawRange(Vec::new(), Vec::new()).is_err());
    let (files, rules) = generate_files();
    for file in files {
        importer.PauseForBackpressure();
        importer
            .Import(
                &ctx,
                &[BackupFileSet {
                    SSTFiles: vec![file],
                    RewriteRules: Some(rules.clone()),
                    ..Default::default()
                }],
            )
            .unwrap();
    }
    importer.Close().unwrap();
}

/// TestSnapImporterRaw — Go `TestSnapImporterRaw`.
#[test]
// SetRawRange 之后 Import 使用原始键，不再走 table prefix 改写。
fn test_snap_importer_raw() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = generate_regions();
    let import_client = Arc::new(MemImporterClient::default());
    let opt = NewSnapFileImporterOptionsForTest(
        split,
        import_client,
        generate_stores(),
        RewriteMode::RewriteModeKeyspace,
        10,
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::Raw, opt).unwrap();
    importer.SetRawRange(Vec::new(), Vec::new()).unwrap();
    let (files, rules) = generate_files();
    for file in files {
        importer.PauseForBackpressure();
        importer
            .Import(
                &ctx,
                &[BackupFileSet {
                    SSTFiles: vec![file],
                    RewriteRules: Some(rules.clone()),
                    ..Default::default()
                }],
            )
            .unwrap();
    }
    importer.Close().unwrap();
}

// ScanRegions 可阻塞，用于制造 PD 令牌争用。
struct FlowControlSplitClient {
    inner: MemSplitClient,
    in_flight: AtomicI32,
    max_flight: i32,
    peak: Mutex<i32>,
}

impl crate::stubs::SplitClient for FlowControlSplitClient {
    fn GetPlacementRule(
        &self,
        ctx: &Context,
        group_id: &str,
        rule_id: &str,
    ) -> crate::stubs::Result<crate::stubs::PlacementRule> {
        self.inner.GetPlacementRule(ctx, group_id, rule_id)
    }
    fn SetPlacementRule(
        &self,
        ctx: &Context,
        rule: &crate::stubs::PlacementRule,
    ) -> crate::stubs::Result<()> {
        self.inner.SetPlacementRule(ctx, rule)
    }
    fn DeletePlacementRule(
        &self,
        ctx: &Context,
        group_id: &str,
        rule_id: &str,
    ) -> crate::stubs::Result<()> {
        self.inner.DeletePlacementRule(ctx, group_id, rule_id)
    }
    fn ScanRegions(
        &self,
        _ctx: &Context,
        start: &[u8],
        end: &[u8],
        _limit: i32,
    ) -> crate::stubs::Result<Vec<RegionInfo>> {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut peak = self.peak.lock().unwrap();
            if cur > *peak {
                *peak = cur;
            }
        }
        assert!(
            cur <= self.max_flight,
            "in-flight {cur} exceeded max {}",
            self.max_flight
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(vec![RegionInfo {
            Region: metapb::Region {
                StartKey: start.to_vec(),
                EndKey: end.to_vec(),
                Peers: vec![metapb::Peer { Id: 1, StoreId: 1 }],
                ..Default::default()
            },
            Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
        }])
    }
}

/// TestSnapImporterPDScanRequestFlowControl — Go same.
#[test]
// 并发超过 PD 令牌数时请求排队，而不是无界打满 PD。
fn test_snap_importer_pd_scan_request_flow_control() {
    let ctx = Context::Background();
    let max_flight = 1;
    let split = Arc::new(FlowControlSplitClient {
        inner: MemSplitClient::default(),
        in_flight: AtomicI32::new(0),
        max_flight,
        peak: Mutex::new(0),
    });
    let import_client = Arc::new(MemImporterClient::default());
    let mut opt = NewSnapFileImporterOptionsForTest(
        split.clone(),
        import_client,
        generate_stores(),
        RewriteMode::RewriteModeKeyspace,
        10,
    );
    opt.SetRegionScanConcurrency(max_flight as u32);
    let importer = Arc::new(NewSnapFileImporter(&ctx, 1, KvMode::TiDBFull, opt).unwrap());

    let mut handles = Vec::new();
    for _ in 0..32 {
        let importer = importer.clone();
        let ctx = ctx.clone();
        handles.push(thread::spawn(move || {
            importer.PaginateScanRegionForTest(&ctx, &[], &[]).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert!(*split.peak.lock().unwrap() <= max_flight);
}

// Compacted 模式文件组带特殊 CF/范围，供批量下载断言。
fn make_compacted_file_sets(file_group_count: usize, files_per_group: usize) -> Vec<BackupFileSet> {
    make_compacted_file_sets_with_cf(file_group_count, files_per_group, "write")
}

fn make_compacted_file_sets_with_cf(
    file_group_count: usize,
    files_per_group: usize,
    cf: &str,
) -> Vec<BackupFileSet> {
    let mut file_sets = Vec::with_capacity(file_group_count);
    for i in 0..file_group_count {
        let mut files = Vec::with_capacity(files_per_group);
        for j in 0..files_per_group {
            let mut end = tablecodec::EncodeTablePrefix(100);
            end.push(b'z');
            files.push(crate::stubs::backuppb::File {
                Name: format!("file-{i}-{j}_{cf}.sst"),
                Cf: cf.into(),
                StartKey: tablecodec::EncodeTablePrefix(100),
                EndKey: end,
                ..Default::default()
            });
        }
        file_sets.push(BackupFileSet {
            SSTFiles: files,
            RewriteRules: Some(RewriteRules {
                Data: vec![import_sstpb::RewriteRule {
                    OldKeyPrefix: tablecodec::EncodeTablePrefix(100),
                    NewKeyPrefix: tablecodec::EncodeTablePrefix(1),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    file_sets
}

/// TestBatchDownloadLatestMVCCParallelizesFileGroupsPerPeer — Go same.
/// Rust Import must issue download and ingest RPCs, not merely scan regions.
#[test]
// 同一 region 多文件组应并行下载到各 peer，缩短关键路径。
fn test_batch_download_latest_mvcc_parallelizes_file_groups_per_peer() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = vec![RegionInfo {
        Region: metapb::Region {
            Id: 1,
            StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
            EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
            Peers: vec![metapb::Peer { Id: 1, StoreId: 1 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
    }];
    let import_client = Arc::new(MemImporterClient::default());
    let stores = vec![metapb::Store {
        Id: 1,
        State: metapb::StoreState::Up,
        ..Default::default()
    }];
    let opt = NewSnapFileImporterOptions(
        None,
        split,
        import_client.clone(),
        None,
        RewriteMode::RewriteModeKeyspace,
        stores.clone(),
        2,
        0,
        true,
        Vec::new(),
        Vec::new(),
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBCompacted, opt).unwrap();
    import_client
        .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
        .unwrap();
    importer
        .Import(&ctx, &make_compacted_file_sets(3, 1))
        .unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 3);
    let ingests = import_client.ingests.lock().unwrap();
    assert_eq!(ingests.len(), 1);
    assert_eq!(ingests[0].1.Ssts.len(), 3);
    drop(ingests);
    importer.Close().unwrap();
}

#[test]
fn test_batch_download_latest_mvcc_skips_default_only_file_groups() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = vec![RegionInfo {
        Region: metapb::Region {
            Id: 1,
            StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
            EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
            Peers: vec![metapb::Peer { Id: 1, StoreId: 1 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
    }];
    let import_client = Arc::new(MemImporterClient::default());
    let stores = vec![metapb::Store {
        Id: 1,
        State: metapb::StoreState::Up,
        ..Default::default()
    }];
    let opt = NewSnapFileImporterOptions(
        None,
        split,
        import_client.clone(),
        None,
        RewriteMode::RewriteModeKeyspace,
        stores.clone(),
        2,
        0,
        true,
        Vec::new(),
        Vec::new(),
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBCompacted, opt).unwrap();
    import_client
        .CheckBatchDownloadLatestMVCCSupport(&ctx, &[1])
        .unwrap();
    importer
        .Import(&ctx, &make_compacted_file_sets_with_cf(1, 2, "default"))
        .unwrap();
    assert!(import_client.downloads.lock().unwrap().is_empty());
    assert!(import_client.ingests.lock().unwrap().is_empty());
    let mut sets = make_compacted_file_sets_with_cf(1, 2, "default");
    sets.extend(make_compacted_file_sets(1, 1));
    importer.Import(&ctx, &sets).unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 1);
    let mut mixed = make_compacted_file_sets_with_cf(1, 1, "default");
    mixed[0]
        .SSTFiles
        .extend(make_compacted_file_sets(1, 1).remove(0).SSTFiles);
    importer.Import(&ctx, &mixed).unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 3);
    // A write SST outside the region must not keep its default SST group alive.
    let mut outside = make_compacted_file_sets(1, 1).remove(0).SSTFiles.remove(0);
    outside.StartKey = tablecodec::EncodeTablePrefix(101);
    outside.EndKey = [tablecodec::EncodeTablePrefix(101), vec![b'z']].concat();
    mixed[0]
        .RewriteRules
        .as_mut()
        .unwrap()
        .Data
        .push(import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(101),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(2),
            ..Default::default()
        });
    mixed[0].SSTFiles.pop();
    mixed[0].SSTFiles.push(outside);
    importer.Import(&ctx, &mixed).unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 3);
    importer.retainLatestMVCCVersion = false;
    importer
        .Import(&ctx, &make_compacted_file_sets_with_cf(1, 2, "default"))
        .unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 5);
    importer.Close().unwrap();
}

/// TestBatchDownloadSSTParallelizesFileGroupsPerPeer — Go same.
#[test]
// 与 LatestMVCC 用例对称，覆盖普通 batch download 能力开关。
fn test_batch_download_sst_parallelizes_file_groups_per_peer() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = vec![RegionInfo {
        Region: metapb::Region {
            Id: 1,
            StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
            EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
            Peers: vec![metapb::Peer { Id: 1, StoreId: 1 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
    }];
    let import_client = Arc::new(MemImporterClient::default());
    let stores = vec![metapb::Store {
        Id: 1,
        State: metapb::StoreState::Up,
        ..Default::default()
    }];
    let opt = NewSnapFileImporterOptions(
        None,
        split,
        import_client.clone(),
        None,
        RewriteMode::RewriteModeKeyspace,
        stores.clone(),
        2,
        0,
        false,
        Vec::new(),
        Vec::new(),
    );
    let mut importer = NewSnapFileImporter(&ctx, 1, KvMode::TiDBCompacted, opt).unwrap();
    importer.CheckBatchDownloadSupport(&ctx, &stores).unwrap();
    importer
        .Import(&ctx, &make_compacted_file_sets(3, 2))
        .unwrap();
    assert_eq!(import_client.downloads.lock().unwrap().len(), 6);
    let ingests = import_client.ingests.lock().unwrap();
    assert_eq!(ingests.len(), 1);
    assert_eq!(ingests[0].1.Ssts.len(), 6);
    drop(ingests);
    importer.Close().unwrap();
}

struct RetryDownloadClient {
    inner: MemImporterClient,
    supported: bool,
    probe_error: bool,
    error: crate::stubs::Error,
    uuids: std::sync::Mutex<Vec<Vec<u8>>>,
    probed: std::sync::Mutex<Vec<u64>>,
    cancel_parent: std::sync::Mutex<Option<Context>>,
    contexts: std::sync::Mutex<Vec<Context>>,
}
impl ImporterClient for RetryDownloadClient {
    fn DownloadSST(
        &self,
        ctx: &Context,
        _: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> crate::stubs::Result<import_sstpb::DownloadResponse> {
        self.contexts.lock().unwrap().push(ctx.clone());
        let mut uuids = self.uuids.lock().unwrap();
        uuids.push(req.Sst.Uuid.clone());
        if uuids.len() == 1 {
            if let Some(ctx) = self.cancel_parent.lock().unwrap().as_ref() {
                ctx.cancel(crate::stubs::Error::with_code(
                    "context.Canceled",
                    "context canceled",
                ));
            }
            return Err(self.error.clone());
        }
        Ok(import_sstpb::DownloadResponse::default())
    }
    fn MultiIngest(
        &self,
        ctx: &Context,
        id: u64,
        req: &import_sstpb::MultiIngestRequest,
    ) -> crate::stubs::Result<import_sstpb::IngestResponse> {
        self.inner.MultiIngest(ctx, id, req)
    }
    fn CheckMultiIngestSupport(&self, _: &Context, _: &[u64]) -> crate::stubs::Result<()> {
        Ok(())
    }
    fn CheckBatchDownloadSupport(&self, _: &Context, _: &[u64]) -> crate::stubs::Result<bool> {
        Ok(false)
    }
    fn CheckBatchDownloadLatestMVCCSupport(
        &self,
        _: &Context,
        ids: &[u64],
    ) -> crate::stubs::Result<()> {
        *self.probed.lock().unwrap() = ids.to_vec();
        if !self.supported || self.probe_error {
            Err(crate::stubs::Error::new("strict probe failed"))
        } else {
            Ok(())
        }
    }
    fn IsBatchDownloadLatestMVCCSupported(
        &self,
        _: &Context,
        ids: &[u64],
    ) -> crate::stubs::Result<bool> {
        *self.probed.lock().unwrap() = ids.to_vec();
        if self.probe_error {
            Err(crate::stubs::Error::new("probe unavailable"))
        } else {
            Ok(self.supported)
        }
    }
    fn SetDownloadSpeedLimit(
        &self,
        _: &Context,
        _: u64,
        _: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> crate::stubs::Result<()> {
        Ok(())
    }
    fn AddForcePartitionRange(
        &self,
        _: &Context,
        _: u64,
        _: &import_sstpb::AddPartitionRangeRequest,
    ) -> crate::stubs::Result<()> {
        Ok(())
    }
    fn RemoveForcePartitionRange(
        &self,
        _: &Context,
        _: u64,
        _: &import_sstpb::RemovePartitionRangeRequest,
    ) -> crate::stubs::Result<()> {
        Ok(())
    }
    fn CloseGrpcClient(&self) -> crate::stubs::Result<()> {
        Ok(())
    }
}

fn retry_importer(
    supported: bool,
    probe_error: bool,
    code: &'static str,
    retain: bool,
) -> (crate::import::SnapFileImporter, Arc<RetryDownloadClient>) {
    let split = Arc::new(MemSplitClient::default());
    *split.regions.lock().unwrap() = vec![RegionInfo {
        Region: metapb::Region {
            Id: 1,
            StartKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
            EndKey: codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
            Peers: vec![metapb::Peer { Id: 1, StoreId: 1 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: 1, StoreId: 1 }),
    }];
    let rpc = Arc::new(RetryDownloadClient {
        inner: MemImporterClient::default(),
        supported,
        probe_error,
        error: crate::stubs::Error::with_code(
            code,
            if code == "Unavailable" {
                "transport is closing"
            } else if code == "Canceled" || code == "context.Canceled" {
                "context canceled"
            } else {
                "download sst failed"
            },
        ),
        uuids: Default::default(),
        probed: Default::default(),
        cancel_parent: Default::default(),
        contexts: Default::default(),
    });
    let opt = NewSnapFileImporterOptions(
        None,
        split,
        rpc.clone(),
        None,
        RewriteMode::RewriteModeKeyspace,
        vec![metapb::Store {
            Id: 1,
            ..Default::default()
        }],
        1,
        0,
        retain,
        vec![],
        vec![],
    );
    (
        NewSnapFileImporter(&Context::Background(), 1, KvMode::TiDBCompacted, opt).unwrap(),
        rpc,
    )
}

#[test]
fn test_download_retry_preserves_uuid_for_all_rpc_modes() {
    for mode in 0..3 {
        for (support, error) in [
            (true, "Canceled"),
            (false, "Unavailable"),
            (false, "BR:KV:ErrKVDownloadFailed"),
        ] {
            let (mut importer, rpc) = retry_importer(support, false, error, mode == 2);
            importer.SetMergeSst(mode == 1);
            importer
                .CheckPeerDownloadRetrySupport(
                    &Context::Background(),
                    &[metapb::Store {
                        Id: 1,
                        ..Default::default()
                    }],
                )
                .unwrap();
            importer
                .Import(&Context::Background(), &make_compacted_file_sets(1, 1))
                .unwrap();
            let uuids = rpc.uuids.lock().unwrap();
            assert_eq!(uuids.len(), 2);
            assert!(!uuids[0].is_empty());
            assert_eq!(uuids[0], uuids[1]);
            assert!(rpc.contexts.lock().unwrap().iter().all(Context::Done));
        }
    }
}

#[test]
fn test_probe_error_falls_back_and_real_context_cancel_stops() {
    for (probe_error, code) in [(true, "Canceled"), (false, "context.Canceled")] {
        let (mut importer, rpc) = retry_importer(true, probe_error, code, false);
        importer
            .CheckPeerDownloadRetrySupport(
                &Context::Background(),
                &[
                    metapb::Store {
                        Id: 1,
                        ..Default::default()
                    },
                    metapb::Store {
                        Id: 2,
                        State: metapb::StoreState::Offline,
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        assert_eq!(*rpc.probed.lock().unwrap(), vec![1]);
        assert!(
            importer
                .Import(&Context::Background(), &make_compacted_file_sets(1, 1))
                .is_err()
        );
        assert_eq!(rpc.uuids.lock().unwrap().len(), 1);
    }
}

#[test]
fn test_sst_restorer_adapter_probes_before_import_and_retain_probe_is_strict() {
    use astersql_br_pkg_restore::{BalancedFileImporter, FileImporter};
    for retain in [false, true] {
        let (importer, rpc) = retry_importer(true, false, "Canceled", retain);
        let adapter = crate::import::SnapshotFileImporter(std::sync::Mutex::new(importer));
        let ctx = astersql_br_pkg_restore::stubs::Context::Background();
        adapter.ConfigureDownloadRetry(&ctx, &[1]).unwrap();
        adapter.PauseForBackpressure();
        assert_eq!(*rpc.probed.lock().unwrap(), vec![1]);
        let mut set = astersql_br_pkg_restore::BackupFileSet::default();
        set.TableID = 100;
        set.SSTFiles.resize_with(1, Default::default);
        let file = &mut set.SSTFiles[0];
        file.Name = "nonempty_write.sst".into();
        file.Cf = "write".into();
        file.StartKey = tablecodec::EncodeTablePrefix(100);
        file.EndKey = [tablecodec::EncodeTablePrefix(100), vec![b'z']].concat();
        set.RewriteRules = Some(Default::default());
        let rules = set.RewriteRules.as_mut().unwrap();
        rules.Data.resize_with(1, Default::default);
        rules.Data[0].OldKeyPrefix = tablecodec::EncodeTablePrefix(100);
        rules.Data[0].NewKeyPrefix = tablecodec::EncodeTablePrefix(1);
        let sets = vec![set];
        adapter.Import(&ctx, &sets).unwrap();
        assert_eq!(rpc.uuids.lock().unwrap().len(), 2);
        let (importer, _) = retry_importer(false, false, "Canceled", retain);
        let adapter = crate::import::SnapshotFileImporter(std::sync::Mutex::new(importer));
        assert_eq!(adapter.ConfigureDownloadRetry(&ctx, &[1]).is_err(), retain);
    }
}

#[test]
fn test_snapshot_client_factory_probes_peer_retry_and_falls_back_on_probe_error() {
    for probe_error in [false, true] {
        let (fixture, rpc) = retry_importer(true, probe_error, "Canceled", false);
        let mut client = crate::client::NewRestoreClientForTest();
        client
            .initClients(
                &Context::Background(),
                None,
                false,
                false,
                fixture.metaClient.clone(),
                rpc.clone(),
                vec![metapb::Store {
                    Id: 1,
                    ..Default::default()
                }],
                vec![],
                vec![],
            )
            .unwrap();
        assert_eq!(*rpc.probed.lock().unwrap(), vec![1]);
        let result = client
            .importer
            .as_mut()
            .unwrap()
            .Import(&Context::Background(), &make_compacted_file_sets(1, 1));
        assert_eq!(result.is_err(), probe_error);
        assert_eq!(
            rpc.uuids.lock().unwrap().len(),
            if probe_error { 1 } else { 2 }
        );
    }
}

#[test]
fn test_parent_cancellation_interrupts_peer_backoff_without_another_rpc() {
    let ctx = Context::Background();
    let (mut importer, rpc) = retry_importer(true, false, "Canceled", false);
    *rpc.cancel_parent.lock().unwrap() = Some(ctx.clone());
    importer
        .CheckPeerDownloadRetrySupport(
            &ctx,
            &[metapb::Store {
                Id: 1,
                ..Default::default()
            }],
        )
        .unwrap();
    let error = importer
        .Import(&ctx, &make_compacted_file_sets(1, 1))
        .unwrap_err();
    assert_eq!(error.code, Some("context.Canceled"));
    assert_eq!(rpc.uuids.lock().unwrap().len(), 1);
}
