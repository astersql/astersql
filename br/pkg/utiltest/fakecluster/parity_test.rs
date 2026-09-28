// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/utiltest/fakecluster` vs Go `core.go`.
//!
//! Covers normal cluster ops, boundary key/limit cases, error paths, and
//! subscription resource cleanup on context cancel.
//! 与 Go fakecluster/core.go 公开契约对等测试。
//! 覆盖基本集群、分裂、checkpoint 推进、flush、TSO 与 GC safepoint。
//! 边界：RegionScan limit=0、FlushExcept 键内外、编码空键形态。
//! 错误：safepoint 回退、缺失 store、legacy RPC、取消订阅清理。
//! 断言锁定文案与 epoch/FlushedEpoch 关系，不改测试行为。
//! 订阅取消后 subscriber_count 应下降；legacy RPC 关闭返回 Unimplemented。
//! FlushNow/OnGetClient 等钩子路径仅验证错误面，不代表生产可用。

use std::sync::Arc;
use std::time::Duration;

use crate::{
    Code, Context, FlushNowRequest, GetLastFlushTSOfRegionRequest, New, NewBasicCluster, NewRegion,
    RegionIdentity, SubscribeFlushEventRequest, oracle, status_error,
};

#[test]
// 聚合正常、边界、错误与资源清理断言，对齐 Go 契约。
fn go_rust_public_contract_matches() {
    // 正常路径：建簇、分裂、推进 checkpoint、flush、任务事件与 TSO。
    // --- Normal: basic cluster, split, checkpoint advance, flush, task event ---
    // 三 store 且启用 flush 仿真。
    let c = NewBasicCluster(3, true);
    // Store 分配 ID 1..3。
    assert_eq!(c.StoreIDs(), vec![1, 2, 3]);
    let regions = c.RegionList();
    // 初始仅一个全范围 region。
    assert_eq!(regions.len(), 1);
    // 初始 region 使用下一个分配 ID。
    assert_eq!(regions[0].ID, 4);
    assert_eq!(
        regions[0].Leader.load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // 在键 m 分裂为两段。
    c.SplitAt("m");
    let regions = c.RegionList();
    // 分裂后应有两个 region。
    assert_eq!(regions.len(), 2);
    // 列表按 StartKey 排序验证。
    // Sorted by start key: "" then "m"
    // 左段从空键开始。
    assert!(regions[0].Range.lock().unwrap().StartKey.is_empty());
    // 左段结束于分裂键。
    assert_eq!(regions[0].Range.lock().unwrap().EndKey, b"m");
    // 右段从分裂键开始。
    assert_eq!(regions[1].Range.lock().unwrap().StartKey, b"m");
    // 右段延伸到正无穷。
    assert!(regions[1].Range.lock().unwrap().EndKey.is_empty());
    // 分裂后两侧 epoch 同为旧值加一。
    // Both epochs bumped to 1
    assert_eq!(
        regions[0].Epoch.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        regions[1].Epoch.load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // 分别设置两段 checkpoint 后推进。
    c.SetRegionCheckpoint(regions[0].ID, 100);
    c.SetRegionCheckpoint(regions[1].ID, 200);
    // 返回的最小 checkpoint 必须前进。
    let min_cp = c.AdvanceCheckpoints();
    // 全局最小推进值超过原设置。
    assert!(min_cp > 100);
    assert!(
        regions[0]
            .Checkpoint
            .load(std::sync::atomic::Ordering::SeqCst)
            > 100
    );
    assert_eq!(
        regions[0]
            .FlushSim
            .FlushedEpoch
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    // FlushAll 后 FlushedEpoch 等于当前 Epoch。
    c.FlushAll();
    assert_eq!(
        regions[0]
            .FlushSim
            .FlushedEpoch
            .load(std::sync::atomic::Ordering::SeqCst),
        regions[0].Epoch.load(std::sync::atomic::Ordering::SeqCst)
    );

    // NewTaskEvent 生成 EventAdd。
    let ev = c.NewTaskEvent("task-a", 42);
    // 事件名与入参一致。
    assert_eq!(ev.Name, "task-a");
    // StartTs 透传。
    assert_eq!(ev.Info.as_ref().unwrap().StartTs, 42);
    assert_eq!(ev.Type, astersql_br_pkg_streamhelper::EventType::EventAdd);

    // TSO 物理时钟：Alloc 加一毫秒，AdvanceClusterTimeBy 按秒换算。
    // TSO helpers
    c.SetCurrentTS(oracle::ComposeTS(1000, 0));
    // SetCurrentTS 立即可见。
    assert_eq!(c.CurrentTSO(), oracle::ComposeTS(1000, 0));
    let next = c.AllocTSO();
    // AllocTSO 物理时间加一。
    assert_eq!(next, oracle::ComposeTS(1001, 0));
    let advanced = c.AdvanceClusterTimeBy(Duration::from_secs(1));
    assert_eq!(
        oracle::ExtractPhysical(advanced),
        oracle::ExtractPhysical(next) + 1000
    );

    // 边界：手动 AddRegion、scan limit、FlushExcept 排除规则。
    // --- Boundary: empty cluster AddRegion; RegionScan limit; empty-key flush except ---
    // 空集群再挂自定义 region 与 peers。
    let c2 = New();
    let r = NewRegion(10, b"a".to_vec(), b"z".to_vec(), 7, 0, 5, false);
    c2.AddRegion(Arc::clone(&r), &[7, 8]);
    // AddRegion peers 创建对应 store。
    assert_eq!(c2.StoreIDs(), vec![7, 8]);
    let scanned = c2
        // limit 为 0 时不返回任何 region。
        .RegionScan(&Context::background(), b"a", b"z", 0)
        .unwrap();
    assert!(scanned.is_empty());
    let scanned = c2
        .RegionScan(&Context::background(), b"a", b"z", 10)
        .unwrap();
    // 正 limit 扫到自定义 region。
    assert_eq!(scanned.len(), 1);
    // 扫描结果 ID 匹配。
    assert_eq!(scanned[0].Region.Id, 10);

    let store7 = c2.GetLogBackupClient(&Context::background(), 7).unwrap();
    // 排除键落在 region 内则不 flush。
    store7.FlushExcept(&["m"]); // key inside [a,z) — region skipped
    assert_eq!(
        r.FlushSim
            .FlushedEpoch
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    // 排除键在外则执行 flush。
    store7.FlushExcept(&["0"]); // key outside — flush
    assert_eq!(
        r.FlushSim
            .FlushedEpoch
            .load(std::sync::atomic::Ordering::SeqCst),
        r.Epoch.load(std::sync::atomic::Ordering::SeqCst)
    );

    // 空键 EncodeBytes 黄金字节，供事件编码对照。
    // codec.EncodeBytes empty key shape used by flush events
    assert_eq!(
        crate::codec::EncodeBytes(Vec::new(), &[]),
        vec![0, 0, 0, 0, 0, 0, 0, 0, 247]
    );

    // 错误路径：GC 回退、缺失 store、legacy 或未实现订阅等。
    // --- Error: BlockGCUntil regression; missing store; checkpoint not found; unimplemented sub ---
    let c3 = NewBasicCluster(1, true);
    // 先成功提升 safepoint。
    c3.BlockGCUntil(&Context::background(), 100).unwrap();
    // 回退必须带 minimal safe point 文案。
    let err = c3.BlockGCUntil(&Context::background(), 50).unwrap_err();
    assert!(
        err.msg
            .contains("minimal safe point 100 is greater than the target 50")
    );
    // Unblock 置 Deleted 计数。
    c3.UnblockGC(&Context::background()).unwrap();
    assert_eq!(
        c3.ServiceGCSafePointDeleted
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // 缺失 store 错误含 does not exist 语义。
    let missing = c3.GetLogBackupClient(&Context::background(), 999);
    assert!(
        missing
            .err()
            .expect("missing store")
            .msg
            .contains("doesn't exist")
    );

    let store = c3.GetLogBackupClient(&Context::background(), 1).unwrap();
    let resp = store
        .GetLastFlushTSOfRegion(
            &Context::background(),
            &GetLastFlushTSOfRegionRequest {
                Regions: vec![RegionIdentity {
                    Id: 999,
                    EpochVersion: 0,
                }],
            },
        )
        .unwrap();
    assert_eq!(
        resp.Checkpoints[0].Err.as_ref().unwrap().Message,
        "not found"
    );

    // flush sim: not flushed (epoch 0 flush still reports "not flushed" — Go semantics)
    let rid = c3.RegionIDs()[0];
    let region = c3.FindRegionByID(rid).unwrap();
    let resp = store
        .GetLastFlushTSOfRegion(
            &Context::background(),
            &GetLastFlushTSOfRegionRequest {
                Regions: vec![RegionIdentity {
                    Id: rid,
                    EpochVersion: region.Epoch.load(std::sync::atomic::Ordering::SeqCst),
                }],
            },
        )
        .unwrap();
    assert_eq!(
        resp.Checkpoints[0].Err.as_ref().unwrap().Message,
        "not flushed"
    );

    // bump epoch so FlushedEpoch after Flush is non-zero
    c3.BumpRegionEpoch(rid);
    let epoch = region.Epoch.load(std::sync::atomic::Ordering::SeqCst);
    region.Flush();
    let resp = store
        .GetLastFlushTSOfRegion(
            &Context::background(),
            &GetLastFlushTSOfRegionRequest {
                Regions: vec![RegionIdentity {
                    Id: rid,
                    EpochVersion: epoch,
                }],
            },
        )
        .unwrap();
    assert!(
        resp.Checkpoints[0].Err.is_none(),
        "unexpected err: {:?}",
        resp.Checkpoints[0].Err
    );

    let sub_err =
        match store.SubscribeFlushEvent(Context::background(), &SubscribeFlushEventRequest {}) {
            Ok(_) => panic!("expected Unimplemented"),
            Err(e) => e,
        };
    assert!(matches!(
        sub_err.status.as_ref().map(|s| s.code),
        Some(Code::Unimplemented)
    ));
    assert!(sub_err.msg.contains("meow?"));

    store
        // legacy RPC 开关：关闭时应返回 Unimplemented。
        .LegacyRegionCheckpointRPCEnabled
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let legacy_err = store
        .GetLastFlushTSOfRegion(
            &Context::background(),
            &GetLastFlushTSOfRegionRequest { Regions: vec![] },
        )
        .unwrap_err();
    assert!(legacy_err.msg.contains("legacy and disabled"));

    // 按 store 批量推进 checkpoint。
    let apply_err = c3.ApplyCheckpointToStore(&Context::background(), 1, 0);
    assert!(
        apply_err
            .unwrap_err()
            .msg
            .contains("must be greater than current")
    );

    // FlushNow success path
    store
        .LegacyRegionCheckpointRPCEnabled
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let flush_now = store
        .FlushNow(&Context::background(), &FlushNowRequest {})
        .unwrap();
    assert!(flush_now.Results[0].Success);
    assert_eq!(flush_now.Results[0].TaskName, "Universe");

    // 资源清理：取消 Context 后订阅者应被移除。
    // --- Resource cleanup: subscribe then cancel removes subscriber ---
    let c4 = NewBasicCluster(1, false);
    let store = c4.GetLogBackupClient(&Context::background(), 1).unwrap();
    store.SetSupportFlushSub(true);
    assert_eq!(store.BootstrapAt(), 1);
    assert!(store.SupportsSub());

    let (ctx, cancel) = Context::with_cancel();
    let _stream = store
        .SubscribeFlushEvent(ctx, &SubscribeFlushEventRequest {})
        .unwrap();
    // subscriber present
    // 取消后活跃订阅数应下降。
    assert_eq!(store.subscriber_count(), 1);
    cancel.cancel();
    // wait for cleanup thread
    for _ in 0..50 {
        if store.subscriber_count() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(store.subscriber_count(), 0);

    // ApplyCheckpointToStore broadcasts to subscribers
    let (ctx2, _c2) = Context::with_cancel();
    store.SetSupportFlushSub(true);
    let stream = store
        .SubscribeFlushEvent(ctx2, &SubscribeFlushEventRequest {})
        .unwrap();
    let rid = c4.RegionIDs()[0];
    c4.SetRegionCheckpoint(rid, 10);
    let states = c4
        .ApplyCheckpointToStore(&Context::background(), 1, 100)
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].Checkpoint, 100);
    let ev = stream.Recv().unwrap();
    assert_eq!(ev.Events.len(), 1);
    assert_eq!(ev.Events[0].Checkpoint, 100);

    // 状态错误码构造与文案对照。
    let _ = status_error(Code::Canceled, "x");
}

#[test]
fn split_and_scatter_requires_three_stores_like_go() {
    let cluster = NewBasicCluster(2, false);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cluster.SplitAndScatter(&[]);
    }));

    assert!(
        result.is_err(),
        "Go chooseStores slices to three entries and panics when fewer stores exist"
    );
}
