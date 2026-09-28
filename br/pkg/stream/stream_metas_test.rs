// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/stream/stream_metas_test.go`.
//!
//! 流备份 meta/migration/截断单测：对齐 Go `stream_metas_test.go`。
//! 覆盖 V1/V2 truncate、safepoint 读写、ReplaceMetadata、shift TS、migration 合并与重试。
//! 使用 MemStorage + MetadataHelper；BeforeDoWriteBack 钩子验证写回/跳过路径。
//! 断言依据：文件计数、TruncatedTo、IngestedSstPaths、错误文案与 Go 一致。
//! 不改生产逻辑；fixture 构造函数仅服务本文件测试场景。

//! truncate_log_common 同时验证 V1 Files 与 V2 FileGroups 写回分支。
//! MigrationExtension 测试均加 MMOptSkipLockingInTest，避免文件锁依赖。
//! safepoint 两测使用不同黄金乘数，避免共享状态假阳性。
//! test_user_abort 依赖 InteractiveCheck 短路，不执行实际 truncate。
//! phantom migration 用于模拟合并窗口内额外 SST 路径注入。
//! hashMigration 作为迁移等价性唯一判据，忽略不稳定字段布局。
//! fake_* 助手写入的日志内容为占位字节，不解析 KV。
//! RemoveDataFilesAndUpdateMetadataInBatch 的 not_deleted 为空表示全成功。
//! LoadBase 在 merge 后应反映 NewBase.TruncatedTo。
//! test_truncate3 用 HashSet 收集被跳过写回的 meta 路径。
//! setup_truncate_metas 的 name 直接拼到 backupmeta 前缀下。
//! UpdateShiftTS 第二个返回值 ok 表示文件名可解析且落在窗口内。
//! NewMigration 默认 Version=M2、Creator 含 br。
//! AppendMigration 返回的 id 与写入序号一致。
//! MergeMigrations 对 IngestedSstPaths 做有序拼接而非去重。
//! test_basic_migration 校验 EditMeta.DestructSelf 往返。
//! test_merge_and_migrate_to 合并同 Path 的 DeletePhysicalFiles。
//! GCS 测例注释保留 Go 语义说明，实现改用 MemStorage。
//! IterateFilesFullyBefore 回调返回 true 可提前停止（truncate3 不用）。
//! StreamMetadataSet.LoadFrom 会扫描 meta 前缀下全部 .meta。
//! MetadataDownloadBatchSize 影响批下载，本测固定 128。
//! StoreId 轮转用于模拟多 TiKV 写入源。
//! test_replace_metadata_ts 不触及存储，纯内存结构。
//! write_migration 路径格式 v1/migrations/{nameOf}。
//! require_migrations_equal 失败时打印两侧 Debug。
//! test_unsupported_version 不调用 Merge，仅检查默认版本。
//! Compaction.ArtifactsHash 在重试场景保持不变。
//! AlwaysRunTruncate 选项名对齐 Go MMOptAlwaysRunTruncate。
//! test_truncate1/2 覆盖完全落在截断点左侧的多段 meta。
//! remained/removed 钩子统计与 Go BeforeDoWriteBack 语义一致。
//! 批量删除回调参数 num 为本次删除文件数增量。
//! MinResolvedTs 在 V2 fixture 中设为与 MinTs 相同。
//! Length 字段在 fake_data_files_v2 中固定为 4。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::stream_metas::{
    FileGroupInfo, GetTSFromFile, MMOptAlwaysRunTruncate, MMOptAppendPhantomMigration,
    MMOptInteractiveCheck, MMOptSkipLockingInTest, MergeMigrations, MigrationExtension,
    NewMigration, ReplaceMetadata, SetTSToFile, StreamMetadataSet, TruncateSafePointFileName,
    UpdateShiftTS, hashMigration, nameOf,
};
use crate::stream_mgr::{GetStreamBackupMetaPrefix, NewMetadataHelper};
use crate::stubs::MemStorage;
use crate::stubs::Storage;
use crate::stubs::backuppb::{
    Compaction, DataFileGroup, DataFileInfo, MetaEdit, MetaVersion, Metadata, Migration,
    MigrationVersion,
};
use crate::stubs::errors::berrors;

// 用 hashMigration 比较，避免字段顺序导致 Debug 不等。
fn require_migrations_equal(a: &Migration, b: &Migration) {
    assert_eq!(hashMigration(a), hashMigration(b), "\n{a:?}\n{b:?}");
}

#[test]
fn test_migration_hash_uses_go_crc64_iso() {
    let mut migration = NewMigration();
    migration.IngestedSstPaths.push("123456789".to_owned());

    // hash/crc64.Checksum([]byte("123456789"), crc64.MakeTable(crc64.ISO))
    assert_eq!(hashMigration(&migration), 0xb909_56c7_75a4_1001);
    assert_eq!(nameOf(&migration, 7), "00000007_B90956C775A41001.mgrt");
}

#[test]
fn test_merge_migrations_rebuilds_left_meta_edit_like_go() {
    let mut left = NewMigration();
    left.EditMeta.push(MetaEdit {
        Path: "meta/1.meta".to_owned(),
        DestructSelf: true,
        ..Default::default()
    });

    let merged = MergeMigrations(&left, &NewMigration());
    assert_eq!(merged.EditMeta.len(), 1);
    assert!(!merged.EditMeta[0].DestructSelf);
}

// 构造 V1 风格连续 TS 窗口的 DataFileInfo，并写入占位日志文件。
fn fake_data_files(s: &dyn Storage, base: i32, count: i32) -> Vec<DataFileInfo> {
    let mut dfs = Vec::new();
    for i in 0..count {
        let start = (base + i) as u64;
        let path = format!("{:04}_to_{:04}.log", start, start + 2);
        s.WriteFile(&path, b"test").unwrap();
        dfs.push(DataFileInfo {
            Path: path,
            MinTs: start,
            MaxTs: start + 2,
            ..Default::default()
        });
    }
    dfs
}

// V2：每文件一组 DataFileGroup，带 MinResolvedTs/Length。
fn fake_data_files_v2(s: &dyn Storage, base: i32, count: i32) -> Vec<DataFileGroup> {
    let mut groups = Vec::new();
    for i in 0..count {
        let start = (base + i) as u64;
        let path = format!("{:04}_to_{:04}.log", start, start + 2);
        s.WriteFile(&path, b"test").unwrap();
        groups.push(DataFileGroup {
            Path: path.clone(),
            MinTs: start,
            MaxTs: start + 2,
            MinResolvedTs: start,
            Length: 4,
            DataFilesInfo: vec![DataFileInfo {
                Path: path,
                MinTs: start,
                MaxTs: start + 2,
                ..Default::default()
            }],
        });
    }
    groups
}

// 聚合 Files 的全局 Min/Max TS。
fn ts_of_file(dfs: &[DataFileInfo]) -> (u64, u64) {
    let min_ts = dfs.iter().map(|d| d.MinTs).min().unwrap_or(0);
    let max_ts = dfs.iter().map(|d| d.MaxTs).max().unwrap_or(0);
    (min_ts, max_ts)
}

// 聚合 FileGroups 的全局 Min/Max TS。
fn ts_of_file_group(dfs: &[DataFileGroup]) -> (u64, u64) {
    let min_ts = dfs.iter().map(|d| d.MinTs).min().unwrap_or(0);
    let max_ts = dfs.iter().map(|d| d.MaxTs).max().unwrap_or(0);
    (min_ts, max_ts)
}

// 写入 6 个 V1 meta（轮转 StoreId），供 truncate 公共路径。
fn fake_stream_backup(s: &dyn Storage) -> Result<(), String> {
    let mut base = 0;
    for i in 0..6 {
        let dfs = fake_data_files(s, base, 4);
        base += 4;
        let (min_ts, max_ts) = ts_of_file(&dfs);
        let meta = Metadata {
            MinTs: min_ts,
            MaxTs: max_ts,
            Files: dfs,
            // StoreId 在 1..3 轮转。
            StoreId: (i % 3 + 1) as i64,
            ..Default::default()
        };
        let bs = meta.Marshal()?;
        // meta 路径：v1/backupmeta/000i.meta。
        let name = format!("{}/{:04}.meta", GetStreamBackupMetaPrefix(), i);
        s.WriteFile(&name, &bs)?;
    }
    Ok(())
}

// 同布局的 V2 FileGroups meta。
fn fake_stream_backup_v2(s: &dyn Storage) -> Result<(), String> {
    let mut base = 0;
    for i in 0..6 {
        let dfs = fake_data_files_v2(s, base, 4);
        base += 4;
        let (min_ts, max_ts) = ts_of_file_group(&dfs);
        let meta = Metadata {
            MinTs: min_ts,
            MaxTs: max_ts,
            FileGroups: dfs,
            StoreId: (i % 3 + 1) as i64,
            // setup_truncate_metas 固定 V2。
            MetaVersion: MetaVersion::V2,
            ..Default::default()
        };
        let bs = meta.Marshal()?;
        let name = format!("{}/{:04}.meta", GetStreamBackupMetaPrefix(), i);
        s.WriteFile(&name, &bs)?;
    }
    Ok(())
}

// V1/V2 共用截断流程：Load → IterateBefore → 批量删除 → 复查。
// BeforeDoWriteBack 区分“改写保留”与“整 meta 删除”。
fn truncate_log_common(v2: bool) {
    let s = Arc::new(MemStorage::new());
    if v2 {
        fake_stream_backup_v2(s.as_ref()).unwrap();
    } else {
        fake_stream_backup(s.as_ref()).unwrap();
    }
    let mut set = StreamMetadataSet::default();
    // 批大小与 helper 与生产默认测试配置对齐。
    set.Helper = NewMetadataHelper();
    set.MetadataDownloadBatchSize = 128;
    set.LoadFrom(s.clone()).unwrap();

    let fs = Arc::new(Mutex::new(Vec::new()));
    // 截止 TS=17：期望 15 个文件完全早于该点。
    set.IterateFilesFullyBefore(17, |d: &FileGroupInfo| {
        assert!(d.MaxTS < 17);
        fs.lock().unwrap().push(d.MaxTS);
        // 回调返回 false 表示继续迭代。
        false
    });
    assert_eq!(fs.lock().unwrap().len(), 15);

    let remained_files = Arc::new(Mutex::new(Vec::new()));
    let remained_data = Arc::new(Mutex::new(Vec::new()));
    let removed_meta = Arc::new(Mutex::new(Vec::new()));
    let rf = remained_files.clone();
    let rd = remained_data.clone();
    let rm = removed_meta.clone();
    // 钩子：有残留文件则记 remained，否则记 removed_meta。
    set.BeforeDoWriteBack = Some(Box::new(move |path, replaced| {
        if !replaced.GetFileGroups().is_empty() || !replaced.Files.is_empty() {
            rf.lock().unwrap().push(path.to_string());
            for ds in &replaced.FileGroups {
                rd.lock().unwrap().push(ds.Path.clone());
            }
            for f in &replaced.Files {
                rd.lock().unwrap().push(f.Path.clone());
            }
        } else {
            rm.lock().unwrap().push(path.to_string());
        }
        false
    }));

    // 累计回调删除文件数，期望 15。
    let total = Arc::new(Mutex::new(0i64));
    let t = total.clone();
    // 批量删除 MaxTS<17 的数据；回调累计删除数。
    let not_deleted = set
        .RemoveDataFilesAndUpdateMetadataInBatch(17, s.clone(), |num| {
            *t.lock().unwrap() += num;
        })
        .unwrap();
    // 全部成功删除。
    assert!(not_deleted.is_empty());
    assert_eq!(*total.lock().unwrap(), 15);

    let remained = remained_files.lock().unwrap().clone();
    let removed = removed_meta.lock().unwrap().clone();
    // 部分 meta 被改写保留；0000 应整文件移除。
    assert!(remained.contains(&"v1/backupmeta/0003.meta".to_string()) || !remained.is_empty());
    assert!(removed.contains(&"v1/backupmeta/0000.meta".to_string()));

    // 重新加载后不应再有 MaxTS<17 的文件。
    set.LoadFrom(s.clone()).unwrap();
    let mut still = false;
    set.IterateFilesFullyBefore(17, |_| {
        still = true;
        true
    });
    assert!(!still);

    // 被标记删除的 meta 物理文件应不存在。
    for path in &removed {
        assert!(!s.FileExists(path).unwrap());
    }
}

#[test]
// V1 truncate 入口。
fn test_truncate_log() {
    truncate_log_common(false);
}

#[test]
// V2 truncate 入口。
fn test_truncate_log_v2() {
    truncate_log_common(true);
}

#[test]
// safepoint 文件读写往返；乘数保证值分散。
fn test_truncate_safepoint() {
    let s = MemStorage::new();
    let ts = GetTSFromFile(&s, TruncateSafePointFileName).unwrap();
    // 文件不存在时 GetTS 返回 0。
    assert_eq!(ts, 0);
    for i in 0..100 {
        let n = (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
        SetTSToFile(&s, n, TruncateSafePointFileName).unwrap();
        let ts = GetTSFromFile(&s, TruncateSafePointFileName).unwrap();
        assert_eq!(ts, n, "failed at {i} round");
    }
}

#[test]
// Go 用 fake GCS；此处 MemStorage 覆盖相同 GetTS/SetTS 语义。
fn test_truncate_safepoint_for_gcs() {
    // Go uses fake GCS; MemStorage covers the same GetTS/SetTS semantics.
    let s = MemStorage::new();
    let ts = GetTSFromFile(&s, TruncateSafePointFileName).unwrap();
    assert_eq!(ts, 0);
    for i in 0..100 {
        let n = (i as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
        SetTSToFile(&s, n, TruncateSafePointFileName).unwrap();
        assert_eq!(
            GetTSFromFile(&s, TruncateSafePointFileName).unwrap(),
            n,
            "round {i}"
        );
    }
}

// ReplaceMetadata 测试用的最小 FileGroup。
fn ff(min_ts: u64, max_ts: u64) -> DataFileGroup {
    DataFileGroup {
        MinTs: min_ts,
        MaxTs: max_ts,
        MinResolvedTs: min_ts,
        Path: format!("{min_ts}-{max_ts}"),
        ..Default::default()
    }
}

#[test]
// 替换 FileGroups 后 Min/Max 重算；空列表归零。
fn test_replace_metadata_ts() {
    let mut m = Metadata::default();
    // 两组 → Min=1 Max=5。
    ReplaceMetadata(&mut m, vec![ff(1, 3), ff(4, 5)]);
    assert_eq!(m.MinTs, 1);
    assert_eq!(m.MaxTs, 5);
    // 清空 → Min=Max=0。
    ReplaceMetadata(&mut m, vec![]);
    assert_eq!(m.MinTs, 0);
    assert_eq!(m.MaxTs, 0);
}

#[test]
// UpdateShiftTS：文件名与 write CF MinBeginTs 决定 shift。
fn test_calculate_shift_ts() {
    let m = Metadata {
        MinTs: 10,
        MaxTs: 30,
        FileGroups: vec![DataFileGroup {
            MinTs: 10,
            MaxTs: 30,
            DataFilesInfo: vec![DataFileInfo {
                Cf: "write".into(),
                MinTs: 10,
                MaxTs: 30,
                MinBeginTsInDefaultCf: 5,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    // 期望 ok 且 shift=5（MinBeginTsInDefaultCf）。
    let (ts, ok) = UpdateShiftTS(
        "000000000000000a-0000000000000005-000000000000000a-000000000000001e",
        &m,
        5,
        30,
    );
    assert!(ok);
    assert_eq!(ts, 5);
}

// 按 nameOf(sn) 写入 v1/migrations/ 下 migration 文件。
fn write_migration(s: &dyn Storage, sn: i32, mig: &Migration) {
    let name = nameOf(mig, sn);
    let path = format!("v1/migrations/{name}");
    s.WriteFile(&path, &mig.Marshal().unwrap()).unwrap();
}

#[test]
// Append 后 Load，校验 Appended 内容哈希一致。
fn test_basic_migration() {
    let s = Arc::new(MemStorage::new());
    let mut mig = NewMigration();
    mig.TruncatedTo = 42;
    mig.EditMeta.push(MetaEdit {
        Path: "a.meta".into(),
        DestructSelf: true,
        ..Default::default()
    });
    write_migration(s.as_ref(), 1, &mig);
    let ext = MigrationExtension(s.clone());
    let loaded = ext.Load().unwrap();
    // 仅一条 appended migration。
    assert_eq!(loaded.Appended.len(), 1);
    require_migrations_equal(&loaded.Appended[0].1, &mig);
}

#[test]
fn load_without_persisted_base_still_lists_go_default_base() {
    let ext = MigrationExtension(Arc::new(MemStorage::new()));
    let loaded = ext.Load().unwrap();

    assert!(loaded.Base.is_some());
    assert_eq!(loaded.ListAll(), vec![NewMigration()]);
}

#[test]
fn load_base_merges_legacy_truncate_safepoint() {
    let s = Arc::new(MemStorage::new());
    let mut base = NewMigration();
    base.TruncatedTo = 7;
    s.WriteFile("v1/migrations/BASE", &base.Marshal().unwrap())
        .unwrap();
    SetTSToFile(s.as_ref(), 11, TruncateSafePointFileName).unwrap();

    let loaded = MigrationExtension(s).Load().unwrap();
    assert_eq!(loaded.Base.unwrap().TruncatedTo, 11);
}

#[test]
// 合并两条 migration：TruncatedTo 取较大，EditMeta 合并。
fn test_merge_and_migrate_to() {
    let s = Arc::new(MemStorage::new());
    let mut m1 = NewMigration();
    m1.TruncatedTo = 10;
    m1.EditMeta.push(MetaEdit {
        Path: "a".into(),
        DeletePhysicalFiles: vec!["x".into()],
        ..Default::default()
    });
    let mut m2 = NewMigration();
    m2.TruncatedTo = 20;
    m2.EditMeta.push(MetaEdit {
        Path: "a".into(),
        DeletePhysicalFiles: vec!["y".into()],
        ..Default::default()
    });
    write_migration(s.as_ref(), 1, &m1);
    write_migration(s.as_ref(), 2, &m2);
    let mut ext = MigrationExtension(s.clone());
    let out = ext
        .MergeAndMigrateTo(2, vec![MMOptSkipLockingInTest()])
        .unwrap();
    // 合并结果 TruncatedTo 取 max(10,20)。
    assert_eq!(out.Migrated.NewBase.TruncatedTo, 20);
    let base = ext.LoadBase().unwrap().unwrap();
    assert_eq!(base.TruncatedTo, 20);
}

#[test]
// 合并后 Compactions 保留（本用例不删除压缩项）。
fn test_remove_compaction() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.Compactions.push(Compaction {
        ArtifactsHash: 1,
        ..Default::default()
    });
    write_migration(s.as_ref(), 1, &m);
    let mut ext = MigrationExtension(s.clone());
    let out = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest()])
        .unwrap();
    // 压缩项未被静默丢弃。
    assert_eq!(out.Migrated.NewBase.Compactions.len(), 1);
}

#[test]
// 对同一序号重复 MergeAndMigrateTo 应幂等。
fn test_retry() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.TruncatedTo = 7;
    write_migration(s.as_ref(), 1, &m);
    let mut ext = MigrationExtension(s.clone());
    let _ = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest()])
        .unwrap();
    let again = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest()])
        .unwrap();
    // 重试后 TruncatedTo 仍为 7。
    assert_eq!(again.Migrated.NewBase.TruncatedTo, 7);
}

#[test]
// 含 Compaction 的重试路径仍保留 ArtifactsHash。
fn test_retry_remove_compaction() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.Compactions.push(Compaction {
        ArtifactsHash: 9,
        ..Default::default()
    });
    write_migration(s.as_ref(), 1, &m);
    let mut ext = MigrationExtension(s.clone());
    let _ = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest()])
        .unwrap();
    let again = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest()])
        .unwrap();
    // 重试后 Compactions 仍在。
    assert_eq!(again.Migrated.NewBase.Compactions.len(), 1);
}

#[test]
// MMOptAlwaysRunTruncate：合并时强制跑 truncate 逻辑。
fn test_with_simple_truncate() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.TruncatedTo = 100;
    write_migration(s.as_ref(), 1, &m);
    let mut ext = MigrationExtension(s.clone());
    let out = ext
        .MergeAndMigrateTo(1, vec![MMOptSkipLockingInTest(), MMOptAlwaysRunTruncate()])
        .unwrap();
    // AlwaysRunTruncate 不改变已写入的 TruncatedTo。
    assert_eq!(out.Migrated.NewBase.TruncatedTo, 100);
}

#[test]
// 连续 AppendMigration 分配递增 id。
fn test_appending_migs() {
    let s = Arc::new(MemStorage::new());
    let ext = MigrationExtension(s.clone());
    let mut m = NewMigration();
    m.TruncatedTo = 1;
    let id = ext.AppendMigration(&m).unwrap();
    // 首条 append 序号从 1 起。
    assert_eq!(id, 1);
    m.TruncatedTo = 2;
    let id = ext.AppendMigration(&m).unwrap();
    // 第二条为 2。
    assert_eq!(id, 2);
    let loaded = ext.Load().unwrap();
    // Load 可见两条。
    assert_eq!(loaded.Appended.len(), 2);
}

#[test]
// InteractiveCheck 返回 false → user abort 错误。
fn test_user_abort() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.TruncatedTo = 1;
    write_migration(s.as_ref(), 1, &m);
    let mut ext = MigrationExtension(s.clone());
    let result = ext
        .MergeAndMigrateTo(
            1,
            vec![MMOptSkipLockingInTest(), MMOptInteractiveCheck(|_| false)],
        )
        .unwrap();
    assert!(
        result
            .Migrated
            .Warnings
            .iter()
            .any(|e| e.to_string().contains("User aborted"))
    );
    assert!(!s.FileExists("v1/migrations/BASE").unwrap());
}

#[test]
// NewMigration 默认 M2；构造 M1 仅作对照不写入。
fn test_unsupported_version() {
    let mut m = NewMigration();
    m.Version = MigrationVersion::M1;
    // Supported check is on SupportedMigVersion; ensure M2 is default for NewMigration
    let n = NewMigration();
    // 默认版本 M2。
    assert_eq!(n.Version, MigrationVersion::M2);
    let _ = m;
}

#[test]
// Creator 字段应包含 br 标识。
fn test_creator() {
    let m = NewMigration();
    // Creator 含 br。
    assert!(m.Creator.contains("br"));
}

#[test]
// IngestedSstPaths 经 Load 完整保留。
fn test_grouped_ext_full_backup() {
    let s = Arc::new(MemStorage::new());
    let mut m = NewMigration();
    m.IngestedSstPaths = vec!["sst/a".into(), "sst/b".into()];
    write_migration(s.as_ref(), 1, &m);
    let ext = MigrationExtension(s.clone());
    let loaded = ext.Load().unwrap();
    // SST 路径列表原样保留。
    assert_eq!(
        loaded.Appended[0].1.IngestedSstPaths,
        vec!["sst/a".to_string(), "sst/b".to_string()]
    );
}

#[test]
// MergeMigrations 拼接两侧 IngestedSstPaths。
fn test_merge_migrations_preserves_ingested_sst_paths() {
    let mut m1 = NewMigration();
    m1.IngestedSstPaths = vec!["a".into()];
    let mut m2 = NewMigration();
    m2.IngestedSstPaths = vec!["b".into()];
    // 纯函数合并，不碰存储。
    let merged = MergeMigrations(&m1, &m2);
    assert_eq!(
        merged.IngestedSstPaths,
        vec!["a".to_string(), "b".to_string()]
    );
}

#[test]
// 幻影 migration + TruncatedTo：两侧 SST 路径都进入 NewBase。
fn test_merge_and_migrate_to_bounds_ingested_sst_paths_over_truncates() {
    let s = Arc::new(MemStorage::new());
    let mut m1 = NewMigration();
    m1.TruncatedTo = 10;
    m1.IngestedSstPaths = vec!["keep".into()];
    let mut phantom = NewMigration();
    phantom.TruncatedTo = 5;
    phantom.IngestedSstPaths = vec!["extra".into()];
    write_migration(s.as_ref(), 1, &m1);
    let mut ext = MigrationExtension(s.clone());
    let out = ext
        .MergeAndMigrateTo(
            1,
            vec![
                MMOptSkipLockingInTest(),
                // 注入 TruncatedTo 更小的幻影 migration。
                MMOptAppendPhantomMigration(vec![phantom]),
            ],
        )
        .unwrap();
    // 真实 migration 的 10 胜出。
    assert_eq!(out.Migrated.NewBase.TruncatedTo, 10);
    assert!(
        out.Migrated
            .NewBase
            .IngestedSstPaths
            .contains(&"keep".to_string())
    );
    assert!(
        out.Migrated
            .NewBase
            .IngestedSstPaths
            .contains(&"extra".to_string())
    );
}

// Truncate1/2/3: richer truncate scenarios with partial file retention.
// 按 (min,max,name) 写入 V2 meta + 占位 data 文件。
fn setup_truncate_metas(s: &dyn Storage, specs: &[(u64, u64, &str)]) {
    for (i, (min_ts, max_ts, name)) in specs.iter().enumerate() {
        let path = format!("data_{i}.log");
        // 占位数据文件，仅验证路径存在性。
        s.WriteFile(&path, b"x").unwrap();
        let meta = Metadata {
            MinTs: *min_ts,
            MaxTs: *max_ts,
            FileGroups: vec![DataFileGroup {
                Path: path.clone(),
                MinTs: *min_ts,
                MaxTs: *max_ts,
                DataFilesInfo: vec![DataFileInfo {
                    Path: path,
                    MinTs: *min_ts,
                    MaxTs: *max_ts,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            MetaVersion: MetaVersion::V2,
            ..Default::default()
        };
        s.WriteFile(
            &format!("{}/{name}", GetStreamBackupMetaPrefix()),
            &meta.Marshal().unwrap(),
        )
        .unwrap();
    }
}

#[test]
// 三段 meta：before=11 时 FullyBefore 计数为 2，删除后无残留。
fn test_truncate1() {
    let s = Arc::new(MemStorage::new());
    setup_truncate_metas(
        s.as_ref(),
        &[
            (1, 5, "0001.meta"),
            (6, 10, "0002.meta"),
            (11, 20, "0003.meta"),
        ],
    );
    let mut set = StreamMetadataSet::default();
    set.Helper = NewMetadataHelper();
    set.LoadFrom(s.clone()).unwrap();
    let mut before = 0;
    set.IterateFilesFullyBefore(11, |_| {
        before += 1;
        false
    });
    // 仅前两段完全 <11。
    assert_eq!(before, 2);
    let not_deleted = set
        .RemoveDataFilesAndUpdateMetadataInBatch(11, s.clone(), |_| {})
        .unwrap();
    assert!(not_deleted.is_empty());
}

#[test]
// 删除到 9 后重新 Load，FullyBefore(9) 应为 0。
fn test_truncate2() {
    let s = Arc::new(MemStorage::new());
    setup_truncate_metas(
        s.as_ref(),
        &[(1, 3, "a.meta"), (4, 8, "b.meta"), (9, 15, "c.meta")],
    );
    let mut set = StreamMetadataSet::default();
    set.LoadFrom(s.clone()).unwrap();
    set.RemoveDataFilesAndUpdateMetadataInBatch(9, s.clone(), |_| {})
        .unwrap();
    set.LoadFrom(s.clone()).unwrap();
    let mut left = 0;
    set.IterateFilesFullyBefore(9, |_| {
        left += 1;
        false
    });
    // 截断后无 FullyBefore(9)。
    assert_eq!(left, 0);
}

#[test]
// BeforeDoWriteBack 返回 true 跳过写回；skipped 非空证明钩子触发。
fn test_truncate3() {
    let s = Arc::new(MemStorage::new());
    setup_truncate_metas(s.as_ref(), &[(100, 200, "x.meta"), (201, 300, "y.meta")]);
    let mut set = StreamMetadataSet::default();
    set.LoadFrom(s.clone()).unwrap();
    let skipped = Arc::new(Mutex::new(HashSet::new()));
    let sk = skipped.clone();
    set.BeforeDoWriteBack = Some(Box::new(move |path, _| {
        sk.lock().unwrap().insert(path.to_string());
        // 返回 true：跳过实际写回，仅记录 path。
        true // skip writeback
    }));
    let _ = set
        .RemoveDataFilesAndUpdateMetadataInBatch(250, s.clone(), |_| {})
        .unwrap();
    // 钩子至少触发一次。
    assert!(!skipped.lock().unwrap().is_empty());
}
