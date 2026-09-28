// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/utiltest/crr` vs Go production files.
//!
//! Covers normal contracts, layout boundaries, checkpoint errors, and
//! harness resource cleanup (Close).
//!
//! 与 Go utiltest/crr 公开契约对等：布局构建、PD checkpoint、Flush/复制与 harness 清理。
//! 断言锁定 StoreID 轮转、空布局拒绝、checkpoint 回滚错误文案及 newest-first 复制顺序。
//! 空路径事件应被跳过；未知任务名上传 checkpoint 必须失败。
//! Close 后不再要求额外断言，但 AssertDownstreamCanRestoreTo 落后目标应报错。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::builder::{
    AddRegion, AddRegionsBySplitKeys, AddRoundRobinRegions, BuildRegionLayout, StoreIDRange,
};
use crate::crr_sim::{
    NewCRRUpstreamStorage, NewCRRWorker, NewVersionCreatedEvent, new_event_channel,
};
use crate::pd_sim::NewPDSimWithTestContext;
use crate::stubs::{ArcMemStorage, Context, LocalStorage, Storage};
use crate::types::{
    DEFAULT_TASK_NAME, NewTestContextWithSeed, RegionBoundary, deriveDeterministicSeed,
};
use crate::{NewFlushSimWithTestContext, NewLocalTestHarnessWithTestContext};
use astersql_br_pkg_stream::MetadataHelper;
use astersql_br_pkg_streamhelper::StreamMeta;

#[test]
fn go_rust_public_contract_matches() {
    // --- Normal: builder layout (mirrors builder_test.go) ---
    // 正常：StoreIDRange 含端点；倒序区间返回空切片。
    assert_eq!(StoreIDRange(1, 4), vec![1, 2, 3, 4]);
    assert!(StoreIDRange(1, 0).is_empty());

    // 轮转布局：5 个 region、store 1..3；首尾键分别为空与 k01 / k04 与空。
    let stores = StoreIDRange(1, 3);
    let boundaries = BuildRegionLayout(vec![AddRoundRobinRegions(5, stores)]).unwrap();
    assert_eq!(boundaries.len(), 5);
    // 全范围扫描要求首 region 从空 StartKey 开始。
    assert!(boundaries[0].StartKey.is_empty());
    assert_eq!(boundaries[0].EndKey, b"k01");
    // 轮转：第 0 个落 store 1，第 4 个落 store 2（5%3→2）。
    assert_eq!(boundaries[0].StoreID, 1);
    assert_eq!(boundaries[4].StartKey, b"k04");
    // 末 region EndKey 必须为空，闭合全 key 空间。
    assert!(boundaries[4].EndKey.is_empty());
    assert_eq!(boundaries[4].StoreID, 2);

    // 按分裂键布局：a/d 产生三段，store 在 10/11 间轮转。
    let split = BuildRegionLayout(vec![AddRegionsBySplitKeys(
        vec!["a".into(), "d".into()],
        vec![10, 11],
    )])
    .unwrap();
    // 两个分裂键切出三段：(-∞,a)/[a,d)/[d,+∞)。
    assert_eq!(split.len(), 3);
    assert_eq!(split[0].StartKey, b"");
    assert_eq!(split[0].EndKey, b"a");
    assert_eq!(split[0].StoreID, 10);
    assert_eq!(split[1].StartKey, b"a");
    assert_eq!(split[1].EndKey, b"d");
    // 相邻段 store 交替，第三段回到 10。
    assert_eq!(split[1].StoreID, 11);
    assert_eq!(split[2].StartKey, b"d");
    assert!(split[2].EndKey.is_empty());
    assert_eq!(split[2].StoreID, 10);

    // --- Boundary: empty layout / non-closed last region ---
    // 边界：空布局、未闭合末段、零 region、空 store 列表均应失败。
    assert!(BuildRegionLayout(vec![]).is_err());
    assert!(BuildRegionLayout(vec![AddRegion("a", "", 1)]).is_err());
    assert!(BuildRegionLayout(vec![AddRegion("", "z", 1)]).is_err());
    assert!(AddRoundRobinRegions(0, vec![1])(Vec::new()).is_err());
    assert!(AddRegionsBySplitKeys(vec!["a".into()], vec![])(Vec::new()).is_err());

    // Seed derivation is stable (FNV-1a algorithm from Go).
    // 种子派生稳定且为正，保证跨运行复现。
    let s1 = deriveDeterministicSeed(42, "pd-sim");
    let s2 = deriveDeterministicSeed(42, "pd-sim");
    assert_eq!(s1, s2);
    assert!(s1 > 0);

    let tc = NewTestContextWithSeed(42);
    assert_eq!(tc.Seed(), 42);
    assert!(tc.logs.iter().any(|l| l.contains("SEED:")));

    // --- Error: PDSim validation + checkpoint rollback ---
    // 错误：首段非空 StartKey 的布局被 PDSim 拒绝（须从空键覆盖）。
    let bad = NewPDSimWithTestContext(
        vec![RegionBoundary {
            StartKey: b"x".to_vec(),
            EndKey: Vec::new(),
            StoreID: 1,
        }],
        DEFAULT_TASK_NAME.to_string(),
        &tc,
    );
    assert!(bad.is_err());

    // 合法布局：三 region、两 store；初始全局 checkpoint 为 0。
    let layout = BuildRegionLayout(vec![AddRoundRobinRegions(3, StoreIDRange(1, 2))]).unwrap();
    let pd = NewPDSimWithTestContext(layout.clone(), DEFAULT_TASK_NAME.to_string(), &tc).unwrap();
    // RegionIDs 非空说明 PDSim 已把边界物化进 fakecluster。
    assert!(!pd.RegionIDs().is_empty());
    assert_eq!(pd.GlobalCheckpoint(), 0);

    // 全局 checkpoint 单调递增；回滚与未知任务名返回含关键字的错误。
    pd.UploadV3GlobalCheckpointForTask(DEFAULT_TASK_NAME, 100)
        .unwrap();
    assert_eq!(pd.GlobalCheckpoint(), 100);
    let rollback = pd.UploadV3GlobalCheckpointForTask(DEFAULT_TASK_NAME, 50);
    assert!(rollback.unwrap_err().contains("checkpoint rollback"));
    let unknown = pd.UploadV3GlobalCheckpointForTask("nope", 200);
    assert!(unknown.unwrap_err().contains("unknown task"));

    // FlushSim + CRR worker normal path
    // 正常刷盘：产生元数据与日志路径，且上游存储可见；空 store 失败。
    let ctx = Context::background();
    let mem = ArcMemStorage::new();
    let storage: Arc<dyn Storage> = mem.as_storage();
    let (tx, rx) = new_event_channel();
    let upstream = Arc::new(NewCRRUpstreamStorage(Arc::clone(&storage), tx));
    let flush = NewFlushSimWithTestContext(Arc::clone(&pd), Arc::clone(&upstream) as Arc<_>, &tc);
    let record = flush.FlushStore(&ctx, 1).unwrap();
    assert_eq!(record.StoreID, 1);
    // FlushRecord 必须带上可定位的元数据与至少一条日志路径。
    assert!(!record.MetadataPath.is_empty());
    assert!(!record.LogPaths.is_empty());
    // 元数据文件应已写入上游 Storage，供下游复制读取。
    assert!(storage.FileExists(&ctx, &record.MetadataPath).unwrap());

    // Go writes the source store on Metadata and puts the object path on the
    // DataFileGroup itself. Keep that wire shape: downstream restore code
    // prefers group.Path and only falls back to DataFilesInfo.Path.
    let raw_meta = storage.ReadFile(&ctx, &record.MetadataPath).unwrap();
    let parsed_meta = MetadataHelper::ParseToMetadata(&raw_meta).unwrap();
    assert_eq!(parsed_meta.StoreId, 1);
    assert_eq!(parsed_meta.FileGroups.len(), record.LogPaths.len());
    assert_eq!(parsed_meta.FileGroups[0].Path, record.LogPaths[0]);
    assert!(parsed_meta.FileGroups[0].DataFilesInfo[0].Path.is_empty());

    // 不存在 region 的 storeID 刷盘应失败。
    let empty_store = flush.FlushStore(&ctx, 999);
    assert!(empty_store.is_err());

    let downstream = ArcMemStorage::new().as_storage();
    let mut worker = NewCRRWorker(Arc::clone(&storage), Arc::clone(&downstream), rx);
    // Write triggers event
    // 写文件触发版本事件，Pull/Replicate 后下游应看到相同载荷。
    upstream.WriteFile(&ctx, "extra.bin", b"payload").unwrap();
    let pulled = worker.PullMessages(0);
    assert!(pulled >= 1);
    let replicated = worker.ReplicateBuffered(&ctx, 0).unwrap();
    assert!(replicated >= 1);
    assert_eq!(downstream.ReadFile(&ctx, "extra.bin").unwrap(), b"payload");

    // Newest-first: buffer [a,b] replicates b then a
    // 最新优先：限制复制 1 条时只落地 b，a 仍未出现。
    let (tx2, rx2) = new_event_channel();
    let up2 = ArcMemStorage::new().as_storage();
    let down2 = ArcMemStorage::new().as_storage();
    let wrap = NewCRRUpstreamStorage(Arc::clone(&up2), tx2);
    wrap.WriteFile(&ctx, "a", b"1").unwrap();
    wrap.WriteFile(&ctx, "b", b"2").unwrap();
    let mut w2 = NewCRRWorker(up2, Arc::clone(&down2), rx2);
    assert_eq!(w2.PullMessages(0), 2);
    assert_eq!(w2.ReplicateBuffered(&ctx, 1).unwrap(), 1);
    assert!(down2.FileExists(&ctx, "b").unwrap());
    assert!(!down2.FileExists(&ctx, "a").unwrap());
    let mut choose_first = |_n: usize| 0;
    assert_eq!(
        w2.ReplicateBufferedRandom(&ctx, 0, Some(&mut choose_first))
            .unwrap(),
        1
    );
    assert!(down2.FileExists(&ctx, "a").unwrap());

    // Empty path events are skipped
    // 空路径事件丢弃，仅有效路径计入 PullMessages。
    let (tx3, rx3) = new_event_channel();
    tx3.send(NewVersionCreatedEvent {
        Path: String::new(),
    })
    .unwrap();
    tx3.send(NewVersionCreatedEvent { Path: "ok".into() })
        .unwrap();
    let mem3 = ArcMemStorage::new().as_storage();
    mem3.WriteFile(&ctx, "ok", b"x").unwrap();
    let mut w3 = NewCRRWorker(Arc::clone(&mem3), ArcMemStorage::new().as_storage(), rx3);
    assert_eq!(w3.PullMessages(0), 1);

    // Go accepts nil channels/functions at these public boundaries and returns
    // the documented zero/error result instead of forcing callers to panic.
    let mut nil_worker = NewCRRWorker(Arc::clone(&mem3), ArcMemStorage::new().as_storage(), None);
    assert_eq!(nil_worker.PullMessages(0), 0);
    assert!(
        nil_worker
            .ReplicateBufferedRandom(&ctx, 0, None)
            .unwrap_err()
            .to_string()
            .contains("nil intN")
    );
    let nil_events = NewCRRUpstreamStorage(Arc::clone(&mem3), None);
    let nil_event_error = nil_events.WriteFile(&ctx, "written-before-event-error", b"x");
    assert!(
        nil_event_error
            .unwrap_err()
            .to_string()
            .contains("event channel is nil")
    );
    assert!(mem3.FileExists(&ctx, "written-before-event-error").unwrap());

    // --- Resource cleanup: harness Close ---
    // harness：刷盘→拉取→复制→上传全局 checkpoint，再断言可恢复；落后目标失败。
    // LocalTestHarness 组合 PD/Flush/CRR，走完一次本地端到端刷盘复制。
    let mut h = NewLocalTestHarnessWithTestContext(&ctx, &tc, layout).unwrap();
    let flushed = h.FlushSim.FlushStore(&ctx, 1).unwrap();
    assert!(!flushed.MetadataPath.is_empty());
    let n = h.PullMessages(0);
    // 刷盘应至少产生一条可拉取的版本事件。
    assert!(n >= 1);
    h.Replicate(&ctx, 0).unwrap();
    // 上传刷盘 checkpoint 后，下游应能恢复到该 TS。
    h.UploadGlobalCheckpoint(&ctx, flushed.CheckpointTS)
        .unwrap();
    assert!(
        h.AssertDownstreamCanRestoreTo(&ctx, flushed.CheckpointTS)
            .is_ok()
    );
    // behind target errors
    // 目标 TS 超前全局 checkpoint 时 Assert 应失败。
    assert!(
        h.AssertDownstreamCanRestoreTo(&ctx, flushed.CheckpointTS + 1_000_000)
            .is_err()
    );
    h.Close();
}

#[test]
fn local_storage_matches_go_uri_presign_and_delete_contracts() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "astersql-crr-local-storage-{}-{unique}",
        std::process::id()
    ));
    let ctx = Context::background();
    let mut storage = LocalStorage::new(&root).unwrap();

    assert_eq!(storage.URI(), format!("file://{}", root.display()));
    assert_eq!(
        storage
            .PresignFile(&ctx, "nested/object.log", Duration::from_secs(60))
            .unwrap(),
        "object.log"
    );

    let missing = storage.DeleteFile(&ctx, "missing.log").unwrap_err();
    assert!(missing.is_not_exist);
    storage.IgnoreEnoentForDelete = true;
    storage.DeleteFile(&ctx, "missing.log").unwrap();

    std::fs::remove_dir_all(root).unwrap();
}
