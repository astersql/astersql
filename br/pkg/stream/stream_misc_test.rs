// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/stream/stream_misc_test.go`.
//!
//! 杂项单测：对齐 Go `stream_misc_test.go`。
//! 覆盖 `GetMinStoreCheckpoint`、MetadataHelper 读缓存/并发、V1 FileGroups 保留、`FilterPathByTs`。
//! `GatedReadStorage` 用 Barrier 卡住 ReadFile，断言两路径可重叠 IO（对齐 Go 不跨读持全局锁）。
//! 断言依据与 Go 表驱动用例一致；不改生产逻辑，仅验证行为。

// GatedReadStorage 仅门控 ReadFile，写/列/删直通 MemStorage。
// max_active 用 CAS 更新峰值，避免漏计并发。
// test_get_checkpoint_of_task 不涉及 Global checkpoint 优先分支。
// filename2 拼接明文+zstd，验证同缓存两次切片。
// 并发段要求两路径同时进入 ReadFile，证明锁不跨 IO。
// ParseToMetadata 保留已有 FileGroups，避免二次包装。
// FilterPathByTs 表驱动含 legacy/tagged/非法名三类。
// shift/restore 参数对应 FilterPathByTs 的 left/right。
// 期望空串表示窗口不相交被过滤。
// 非法名期望原样返回，防止误删。
// data2 字节与 stream_mgr ZSTD FIXTURE 完全一致。
// Barrier 三方可避免主线程过早 wait 造成死锁。
// results 收集两线程布尔结果，均须 Ok(true)。
// 轮询 active==2 最多约 1s，失败则峰值断言暴露问题。
// protobuf Marshal 保留 FileGroups，区别于 MetadataHelper.Marshal。
// PauseV2/LastErrors 本文件未覆盖 StatusString 分支。
// StartTs=8 作为无 Store 时的回退基线未在此测触发。
// 缓存 ref=1 的两文件各读一次后应释放 data。
// UNKNOWN 压缩路径不做解压变换。
// ZSTD 路径依赖 fixture 明文长度断言。
// 表驱动最后一项 unexpected_format 验证容错。
// tagged 缺 u 的用例依赖 ParseName 失败放行。
// MaxTS<left 用例期望过滤为空串。
// min_begin=0 用例期望不过滤。
// 并发测试使用 Arc 共享 helper/gated/results。
// Join 所有句柄后再检查 results 长度。
// CollectTaskPrinter 相关逻辑不在本文件。
// MemStorage 无持久化，测试结束即释放。
// helper 在切片读之间共享同一 InitCacheEntry。
// filename1 未 Init，走整文件快速路径。
// 抬高 store2 后最小值变为 10，再抬高 store1 变为 12。
// Go 对照文件：br/pkg/stream/stream_misc_test.go。
// 补充说明：与 Go 语义对齐的约束与数据流备注（32）。
// 补充说明：与 Go 语义对齐的约束与数据流备注（33）。
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use crate::stream_mgr::{FilterPathByTs, MetadataHelper, NewMetadataHelper};
use crate::stream_status::{Checkpoint, TaskStatus};
use crate::stubs::MemStorage;
use crate::stubs::Storage;
use crate::stubs::backuppb::{
    CompressionType, DataFileGroup, DataFileInfo, MetaVersion, Metadata, StreamBackupTaskInfo,
};

/// 可门控的 Storage：统计并发 ReadFile 峰值，并可在读前 wait Barrier。
struct GatedReadStorage {
    inner: Arc<MemStorage>,
    read_gate: Option<Arc<Barrier>>,
    active: AtomicI32,
    max_active: AtomicI32,
}

impl Storage for GatedReadStorage {
    fn ReadFile(&self, path: &str) -> Result<Vec<u8>, String> {
        // 进入即计入 active，并用 CAS 维护历史峰值。
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let max = self.max_active.load(Ordering::SeqCst);
            if active <= max
                || self
                    .max_active
                    .compare_exchange(max, active, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                break;
            }
        }
        if let Some(gate) = &self.read_gate {
            gate.wait();
        }
        let out = self.inner.ReadFile(path);
        self.active.fetch_sub(1, Ordering::SeqCst);
        out
    }
    fn WriteFile(&self, path: &str, data: &[u8]) -> Result<(), String> {
        self.inner.WriteFile(path, data)
    }
    fn ListFiles(&self, sub_dir: &str) -> Result<Vec<(String, i64)>, String> {
        self.inner.ListFiles(sub_dir)
    }
    fn DeleteFile(&self, path: &str) -> Result<(), String> {
        self.inner.DeleteFile(path)
    }
}

#[test]
fn test_get_checkpoint_of_task() {
    // 三 store 取最小 TS；抬高较小者后最小值随之变化。
    let mut task = TaskStatus {
        Info: StreamBackupTaskInfo {
            StartTs: 8,
            ..Default::default()
        },
        paused: false,
        globalCheckpoint: 0,
        Checkpoints: vec![
            Checkpoint::Store(1, 10),
            Checkpoint::Store(2, 8),
            Checkpoint::Store(6, 12),
        ],
        QPS: 0.0,
        LastErrors: Default::default(),
        PauseV2: None,
    };
    assert_eq!(task.GetMinStoreCheckpoint().TS, 8);
    task.Checkpoints[1].TS = 18;
    assert_eq!(task.GetMinStoreCheckpoint().TS, 10);
    task.Checkpoints[0].TS = 14;
    assert_eq!(task.GetMinStoreCheckpoint().TS, 12);
}

#[test]
fn test_metadata_helper_read_file() {
    // 场景：整文件、缓存切片、ZSTD fixture 解压，以及两路径并发加载。
    let s = Arc::new(MemStorage::new());
    let helper = Arc::new(NewMetadataHelper());
    let filename1 = "full_data";
    let filename2 = "misc_data";
    // 与 stream_mgr decode fixture 明文一致。
    let data1 =
        b"Test MetadataHelper. The data contains bare data (or maybe compressed data).".to_vec();
    // Go stream_misc_test.go 中 data1 的 zstd 字节。
    let data2: Vec<u8> = vec![
        0x28, 0xb5, 0x2f, 0xfd, 0x0, 0x58, 0x15, 0x2, 0x0, 0x52, 0x44, 0xe, 0x14, 0xb0, 0x37, 0x1,
        0xe7, 0xa4, 0x1c, 0xd9, 0x3d, 0xc7, 0xee, 0xe7, 0x3e, 0x15, 0x14, 0x80, 0xdc, 0x25, 0x14,
        0xc, 0x60, 0x24, 0x70, 0xda, 0x6a, 0x47, 0xfc, 0x2d, 0xa4, 0x4e, 0x6f, 0xe, 0xa3, 0x9e,
        0x2, 0xce, 0xa6, 0x98, 0xa, 0x67, 0x52, 0xa7, 0x4e, 0x5b, 0x4f, 0x94, 0x92, 0xfb, 0x32,
        0x2e, 0x38, 0x6d, 0xce, 0xf, 0x4d, 0x7c, 0x9, 0x1, 0x0, 0xb2, 0x6c, 0x96, 0x1,
    ];
    s.WriteFile(filename1, &data1).unwrap();
    let mut concat = data1.clone();
    concat.extend_from_slice(&data2);
    s.WriteFile(filename2, &concat).unwrap();

    // ref=2：两次切片读共享同一次加载。
    helper.InitCacheEntry(filename2, 2);
    // 未缓存路径：整文件 UNKNOWN。
    assert_eq!(
        helper
            .ReadFile(
                filename1,
                0,
                0,
                data1.len() as u64,
                CompressionType::UNKNOWN,
                s.as_ref()
            )
            .unwrap(),
        data1
    );
    // 前半明文切片。
    assert_eq!(
        helper
            .ReadFile(
                filename2,
                0,
                data1.len() as u64,
                data1.len() as u64,
                CompressionType::UNKNOWN,
                s.as_ref()
            )
            .unwrap(),
        data1
    );
    // 后半 zstd 切片解压后应等于 data1。
    assert_eq!(
        helper
            .ReadFile(
                filename2,
                data1.len() as u64,
                data2.len() as u64,
                data1.len() as u64,
                CompressionType::ZSTD,
                s.as_ref()
            )
            .unwrap(),
        data1
    );

    let filename3 = "cached_data_1";
    let filename4 = "cached_data_2";
    s.WriteFile(filename3, &data1).unwrap();
    s.WriteFile(filename4, &data1).unwrap();
    helper.InitCacheEntry(filename3, 1);
    helper.InitCacheEntry(filename4, 1);

    // 三方 Barrier：两读线程 + 主线程，确保两路 ReadFile 同时进入。
    let gate = Arc::new(Barrier::new(3));
    let gated = Arc::new(GatedReadStorage {
        inner: s.clone(),
        read_gate: Some(gate.clone()),
        active: AtomicI32::new(0),
        max_active: AtomicI32::new(0),
    });
    let results = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for filename in [filename3.to_string(), filename4.to_string()] {
        let helper = helper.clone();
        let gated = gated.clone();
        let results = results.clone();
        let data1 = data1.clone();
        handles.push(thread::spawn(move || {
            let res = helper.ReadFile(
                &filename,
                0,
                data1.len() as u64,
                data1.len() as u64,
                CompressionType::UNKNOWN,
                gated.as_ref(),
            );
            results.lock().unwrap().push(res.map(|d| d == data1));
        }));
    }
    // 等待两读线程都挂在 gate 上（或超时放弃，由峰值断言兜底）。
    for _ in 0..200 {
        if gated.active.load(Ordering::SeqCst) == 2 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    // 关键：缓存加载不串行化，峰值并发应为 2。
    assert_eq!(gated.max_active.load(Ordering::SeqCst), 2);
    gate.wait();
    for h in handles {
        h.join().unwrap();
    }
    let results = results.lock().unwrap().clone();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| matches!(r, Ok(true))));
}

#[test]
fn test_metadata_helper_parse_to_metadata_preserves_existing_file_groups_for_v1() {
    // V1 已带 FileGroups 时，软/硬解析都不得重建或覆盖 Path。
    let original = Metadata {
        MetaVersion: MetaVersion::V1,
        FileGroups: vec![DataFileGroup {
            Path: "v1/log/store-1/flush-00000001-region-1.log".into(),
            DataFilesInfo: vec![DataFileInfo {
                Path: "v1/log/store-1/flush-00000001-region-1.log".into(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    // Go uses protobuf Metadata.Marshal, not MetadataHelper.Marshal.
    // 使用 protobuf Marshal，避免 MetadataHelper.Marshal 清空 FileGroups。
    let raw = original.Marshal().unwrap();
    let meta = MetadataHelper::ParseToMetadata(&raw).unwrap();
    assert_eq!(meta.FileGroups.len(), 1);
    assert_eq!(meta.FileGroups[0].Path, original.FileGroups[0].Path);
    assert_eq!(meta.FileGroups[0].DataFilesInfo.len(), 1);
    let hard = MetadataHelper::ParseToMetadataHard(&raw).unwrap();
    assert_eq!(hard.FileGroups.len(), 1);
    assert_eq!(hard.FileGroups[0].Path, original.FileGroups[0].Path);
}

#[test]
fn test_filter_path() {
    // 表驱动：legacy / tagged / 不相交 / 缺 tag / 非法名，期望与 Go 一致。
    let tests = [
        (
            "v1/backupmeta/000000000000000a-0000000000000005-000000000000000a-000000000000001e.meta",
            5,
            10,
            "v1/backupmeta/000000000000000a-0000000000000005-000000000000000a-000000000000001e.meta",
        ),
        (
            "v1/backupmeta/000000000000000a-000000000000000a-000000000000000a-000000000000001e.meta",
            5,
            10,
            "v1/backupmeta/000000000000000a-000000000000000a-000000000000000a-000000000000001e.meta",
        ),
        (
            // min_begin=0：不做窗口裁剪，原样返回。
            "v1/backupmeta/000000000000000a-0000000000000000-000000000000000a-000000000000001e.meta",
            5,
            10,
            "v1/backupmeta/000000000000000a-0000000000000000-000000000000000a-000000000000001e.meta",
        ),
        (
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
            5,
            11,
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
        ),
        (
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
            5,
            9,
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
        ),
        (
            // MaxTS < left → 过滤为空。
            "v1/backupmeta/000000000000000a-0000000000000005-000000000000000a-0000000000000004.meta",
            5,
            10,
            "",
        ),
        (
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
            5,
            8,
            "v1/backupmeta/000000000000000a-0000000000000014-000000000000000a-000000000000001e.meta",
        ),
        (
            // tagged 格式仍按 min_begin/max 过滤。
            "v1/backupmeta/000000000000000A000000000000000B-d0000000000000005l000000000000000Au000000000000001E.meta",
            5,
            10,
            "v1/backupmeta/000000000000000A000000000000000B-d0000000000000005l000000000000000Au000000000000001E.meta",
        ),
        (
            "v1/backupmeta/000000000000000A000000000000000B-u0000000000000004x0000000000000009d0000000000000002l0000000000000003.meta",
            3,
            4,
            "v1/backupmeta/000000000000000A000000000000000B-u0000000000000004x0000000000000009d0000000000000002l0000000000000003.meta",
        ),
        (
            "v1/backupmeta/000000000000000A000000000000000B-d0000000000000002l0000000000000003u0000000000000004.meta",
            5,
            10,
            "",
        ),
        (
            // 缺 u tag：ParseName 失败则放行原路径。
            "v1/backupmeta/000000000000000A000000000000000B-d0000000000000002l0000000000000003.meta",
            10,
            10,
            "v1/backupmeta/000000000000000A000000000000000B-d0000000000000002l0000000000000003.meta",
        ),
        (
            // 完全非法文件名：同样放行。
            "v1/backupmeta/unexpected_format.meta",
            10,
            10,
            "v1/backupmeta/unexpected_format.meta",
        ),
    ];
    for (path, shift, restore, expected) in tests {
        assert_eq!(FilterPathByTs(path, shift, restore), expected, "{path}");
    }
}

#[test]
fn test_fast_unmarshal_metadata_skip_condition_can_skip_empty_meta() {
    let storage = Arc::new(MemStorage::new());
    let empty = "v1/backupmeta/00000000000000140000000000000001-d0000000000000000l0000000000000000u0000000000000000p0000000000000002.meta";
    let normal = "v1/backupmeta/000000000000001E0000000000000001-d0000000000000005l000000000000000Au0000000000000014p0000000000000000.meta";
    storage
        .WriteFile(empty, b"invalid empty meta payload")
        .unwrap();
    storage.WriteFile(normal, b"normal meta payload").unwrap();
    assert_eq!(FilterPathByTs(empty, 5, 10), empty);
    let reads = AtomicI32::new(0);
    crate::stream_mgr::FastUnmarshalMetaDataWithOptions(
        storage,
        0,
        100,
        1,
        |path| {
            crate::stream_metas::TryParseTaggedBackupMetaFileNameWrapper(path)
                .is_ok_and(|parsed| parsed.IsEmpty())
        },
        |path, raw| {
            assert_eq!(path, normal);
            assert_eq!(raw, b"normal meta payload");
            reads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}
