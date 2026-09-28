// Copyright 2026 AsterSQL.

//! checkpoint 包 Rust/Go 公开契约的端到端对等测试。
//!
//! 不改业务逻辑，只验证：元数据存取、进度相位、Append 约束、value 压缩、
//! 加解密与 checksum 解析、ticker 零周期、表存储分片合并等关键路径
//! 与 Go 侧语义一致。失败时优先怀疑序列化字段名或默认 tick 常量漂移。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::backup::{
    AppendForBackup, CheckpointMetadataForBackup, LoadCheckpointMetadata, SaveCheckpointMetadata,
    StartCheckpointBackupRunnerForTest, WalkCheckpointFileForBackup,
};
use crate::checkpoint::{
    ChecksumInfo, ChecksumItem, ChecksumItems, RangeGroup, parseCheckpointChecksum,
};
use crate::log_restore::{
    AppendRangeForLogRestore, CheckpointIngestIndexRepairSQL, CheckpointIngestIndexRepairSQLs,
    CheckpointMetadataForLogRestore, CheckpointProgress, GetCheckpointTaskInfo,
    LogRestoreValueType, RestoreProgress, StartCheckpointLogRestoreRunnerForTest,
    valueMarshalerForLogRestore,
};
use crate::manager::{
    DefaultTickDurationConfig, NewLogStorageMetaManager, NewSnapshotStorageMetaManager,
};
use crate::restore::{
    AppendRangesForRestore, CheckpointMetadataForSnapshotRestore, NewCheckpointFileItem,
    NewCheckpointRangeKeyItem, StartCheckpointRestoreRunnerForTest,
};
use crate::storage::{
    CheckpointIdMapBlockSize, CustomSSTRestoreCheckpointDatabaseName, IsCheckpointDB,
    LogRestoreCheckpointDatabaseName, MemSession, SnapshotRestoreCheckpointDatabaseName,
    chunkInsertCheckpointData, mergeSelectCheckpoint,
};
use crate::stubs::{
    CIStr, CipherInfo, Context, Decrypt, Encrypt, EncryptionMethod, File, MemStorage, MockTimer,
    NowDureTime, RestrictedSQLExecutor, Session, SqlRow, SqlValue, Storage, TiFlashReplicaInfo,
};
use crate::ticker::dispatcherTicker;
use sha2::{Digest, Sha256};

// 下列 import 覆盖 backup/restore/log/manager/storage/ticker 全链路，
// 以便单测内完成跨模块契约核对，而无需拆成多个文件。

/// 覆盖 backup/restore/log/storage/ticker 公开 API 与 Go 契约的一致性。
///
/// 场景分段：正常存取 → 边界前缀/分片 → 错误 Append → 压缩与加解密 →
/// Runner 生命周期 → MemSession 合并与 gap 跳过。
#[test]
fn go_rust_public_contract_matches() {
    // --- 正常：backup 元数据 save/load 往返 ---
    // 断言 ConfigHash/BackupTS 经外部存储读写后不变
    // 使用内存 Storage，避免依赖真实对象存储
    let ctx = Context::Background();
    let storage = Arc::new(MemStorage::new());
    let meta = CheckpointMetadataForBackup {
        ConfigHash: b"123456".to_vec(),
        BackupTS: 123456,
        ..Default::default()
    };
    SaveCheckpointMetadata(&ctx, storage.as_ref(), &meta).unwrap();
    let loaded = LoadCheckpointMetadata(&ctx, storage.as_ref()).unwrap();
    assert_eq!(loaded.ConfigHash, meta.ConfigHash);
    assert_eq!(loaded.BackupTS, meta.BackupTS);
    // 到此 backup meta 路径与 Go Save/LoadCheckpointMetadata 对齐

    // --- 正常：restore 外部存储管理器的 meta / progress / task info ---
    // snapshot 与 log 共用同一 MemStorage，但 taskName 前缀不同，路径隔离
    // clusterID=1，prefix 区分 snapshot/log，restoreID=1
    let snapshot = NewSnapshotStorageMetaManager(storage.clone(), None, 1, "snapshot", 1);
    let log = NewLogStorageMetaManager(storage.clone(), None, 1, "log", 1);
    snapshot
        .SaveCheckpointMetadata(
            &ctx,
            &CheckpointMetadataForSnapshotRestore {
                UpstreamClusterID: 123,
                RestoredTS: 321,
                ..Default::default()
            },
        )
        .unwrap();
    let snap2 = snapshot.LoadCheckpointMetadata(&ctx).unwrap();
    assert_eq!(snap2.UpstreamClusterID, 123);
    assert_eq!(snap2.RestoredTS, 321);
    // 快照 meta JSON 字段名必须与 Go 一致，否则 RestoredTS 会丢默认值

    // 日志 meta 含 TiFlash 记录；进度初始不存在，写入后 IdMapSaved 应为 true
    log.SaveCheckpointMetadata(
        &ctx,
        &CheckpointMetadataForLogRestore {
            UpstreamClusterID: 123,
            RestoredTS: 222,
            StartTS: 111,
            RewriteTS: 333,
            GcRatio: "1.0".into(),
            TiFlashItems: HashMap::from([(1, TiFlashReplicaInfo { Count: 1 })]),
            ..Default::default()
        },
    )
    .unwrap();
    // 写入 progress 前 Exists 必须为 false，防止假阳性
    assert!(!log.ExistsCheckpointProgress(&ctx).unwrap());
    log.SaveCheckpointProgress(
        &ctx,
        &CheckpointProgress {
            Progress: RestoreProgress::InLogRestoreAndIdMapPersisted,
        },
    )
    .unwrap();
    let progress = log.LoadCheckpointProgress(&ctx).unwrap();
    assert_eq!(
        progress.Progress,
        RestoreProgress::InLogRestoreAndIdMapPersisted
    );

    // GetCheckpointTaskInfo 合并两侧：有 log meta、有 snapshot meta、id-map 已保存
    let task = GetCheckpointTaskInfo(&ctx, Some(snapshot.as_ref()), log.as_ref()).unwrap();
    assert_eq!(task.Metadata.as_ref().unwrap().UpstreamClusterID, 123);
    assert!(task.HasSnapshotMetadata);
    assert!(task.IdMapSaved());
    // IdMapSaved 依赖 Progress==InLogRestoreAndIdMapPersisted，而非仅有 meta

    // 摄入索引修复 SQL 往返：IndexID/IndexName 必须保留
    log.SaveCheckpointIngestIndexRepairSQLs(
        &ctx,
        &CheckpointIngestIndexRepairSQLs {
            SQLs: vec![CheckpointIngestIndexRepairSQL {
                IndexID: 1,
                SchemaName: CIStr::new("2"),
                TableName: CIStr::new("3"),
                IndexName: "4".into(),
                AddSQL: "5".into(),
                AddArgs: vec![
                    serde_json::json!("6"),
                    serde_json::json!("7"),
                    serde_json::json!("8"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let repair = log.LoadCheckpointIngestIndexRepairSQLs(&ctx).unwrap();
    assert_eq!(repair.SQLs[0].IndexID, 1);
    assert_eq!(repair.SQLs[0].IndexName, "4");
    // AddArgs 以 JSON Value 存储，往返后 IndexName 仍为明文字符串

    // --- 边界：IsCheckpointDB 识别固定前缀与带 restoreID 后缀的库名 ---
    assert!(IsCheckpointDB(LogRestoreCheckpointDatabaseName));
    assert!(IsCheckpointDB(&format!(
        "{SnapshotRestoreCheckpointDatabaseName}_1"
    )));
    assert!(IsCheckpointDB(CustomSSTRestoreCheckpointDatabaseName));
    assert!(!IsCheckpointDB("normal_db"));
    // 普通业务库名不得被误判为检查点库

    // --- 边界：id-map 分块写入，超过 BlockSize 拆成两段 (0, BlockSize)+(1, remainder) ---
    let mut segs = Vec::new();
    let big = vec![7u8; CheckpointIdMapBlockSize + 10];
    chunkInsertCheckpointData(&big, |id, chunk| {
        segs.push((id, chunk.len()));
        Ok(())
    })
    .unwrap();
    assert_eq!(segs, vec![(0, CheckpointIdMapBlockSize), (1, 10)]);
    // 分片大小常量 524288 与 Go CheckpointIdMapBlockSize / BLOB 上限一致

    // --- 错误：Append 既无 rangeKey 也无 name 必须失败 ---
    // NewCheckpointFileItem("", "") 仍可能留下空字段，显式构造 bad item 更稳妥
    // 缩短 tick/retry，加速测试；manager 复用上方 snapshot
    let runner = StartCheckpointRestoreRunnerForTest(
        &ctx,
        Duration::from_millis(200),
        Duration::from_millis(100),
        snapshot.as_ref(),
    )
    .unwrap();
    let err = AppendRangesForRestore(&ctx, &runner, &NewCheckpointFileItem(1, String::new()))
        .unwrap_err();
    let _ = err;
    // NewCheckpointFileItem with empty name still has empty rangeKey — use empty item
    let bad = crate::restore::CheckpointItem {
        tableID: 1,
        rangeKey: String::new(),
        name: String::new(),
    };
    let err = AppendRangesForRestore(&ctx, &runner, &bad).unwrap_err();
    assert!(err.msg.contains("either rangekey or name"));
    // 错误文案需与 Go 保持一致，便于跨语言日志检索

    // 成功 Append + checksum + WaitForFinish，再 Load 验证 range 已落盘
    // 先 Append range，再 FlushChecksum，最后 WaitForFinish 刷盘并停 Runner
    AppendRangesForRestore(&ctx, &runner, &NewCheckpointRangeKeyItem(10, "rk".into())).unwrap();
    runner.FlushChecksum(&ctx, 10, 1, 2, 3).unwrap();
    runner.WaitForFinish(&ctx, true);

    let mut seen = Vec::new();
    snapshot
        .LoadCheckpointData(&ctx, &mut |k, v| {
            seen.push((k, v.RangeKey));
            Ok(())
        })
        .unwrap();
    assert!(seen.iter().any(|(k, r)| *k == 10 && r == "rk"));
    // WaitForFinish(true) 后外部存储应能 Walk/Load 到刚 Append 的 range

    // --- 正常：日志 value 压缩——两 Goff 折叠为 Group.len()==2 ---
    // 构造跨表、跨 Goff 的稀疏条目以验证压缩折叠
    let group = RangeGroup {
        GroupKey: "g1".into(),
        Group: vec![
            LogRestoreValueType {
                TableID: 1,
                Goff: 0,
                Foff: 0,
            },
            LogRestoreValueType {
                TableID: 1,
                Goff: 0,
                Foff: 1,
            },
            LogRestoreValueType {
                TableID: 2,
                Goff: 1,
                Foff: 0,
            },
        ],
    };
    let bytes = valueMarshalerForLogRestore(&group).unwrap();
    let parsed: RangeGroup<String, crate::log_restore::LogRestoreValueMarshaled> =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.GroupKey, "g1");
    assert_eq!(parsed.Group.len(), 2);
    // Goff0 两条 Foff 合并进同一 Marshaled；Goff1 单独成组

    // --- 正常：AES256_CTR 加解密往返；checksum 内容哈希匹配才入 map ---
    // 32 字节密钥满足 AES-256；PLAINTEXT 路径不在此覆盖
    let cipher = CipherInfo {
        CipherType: EncryptionMethod::AES256_CTR,
        CipherKey: b"01234567890123456789012345678901".to_vec(),
    };
    let plain = b"hello-checkpoint";
    let (enc, iv) = Encrypt(plain, Some(&cipher)).unwrap();
    assert_ne!(enc, plain);
    let dec = Decrypt(&enc, Some(&cipher), &iv).unwrap();
    assert_eq!(dec, plain);
    // Encrypt 输出密文+IV；Decrypt 必须使用同一 CipherInfo 与 IV

    let items = ChecksumItems {
        Items: vec![ChecksumItem {
            TableID: 9,
            Crc64xor: 9,
            TotalKvs: 9,
            TotalBytes: 9,
        }],
    };
    let content = serde_json::to_vec(&items).unwrap();
    let digest = Sha256::digest(&content);
    let info = ChecksumInfo {
        Content: content,
        Checksum: digest.to_vec(),
        DureTime: NowDureTime(),
    };
    let data = serde_json::to_vec(&info).unwrap();
    let mut map = HashMap::new();
    let mut dure = Duration::ZERO;
    parseCheckpointChecksum(&data, &mut map, &mut dure).unwrap();
    assert_eq!(map.get(&9).unwrap().Crc64xor, 9);
    // parseCheckpointChecksum 校验 Content 的 SHA256 后才合并 TableID→ChecksumItem

    // 边界：checksum 字节被篡改时整包跳过，map 保持空
    let mut bad_info = info.clone();
    bad_info.Checksum[0] ^= 0xff;
    let bad_data = serde_json::to_vec(&bad_info).unwrap();
    let mut map2 = HashMap::new();
    parseCheckpointChecksum(&bad_data, &mut map2, &mut dure).unwrap();
    assert!(map2.is_empty());
    // 损坏校验和不报错退出，只是跳过该包，避免单文件拖垮整次加载

    // --- ticker：零时长得到 manual，无 channel ---
    let mut manual = dispatcherTicker(Duration::ZERO);
    assert!(manual.Ch().is_none());
    manual.Stop();
    // 与 Go dispatcherTicker(0) 行为一致：无定时投递

    // --- 日志 Runner：Append 单文件范围后 WaitForFinish 清理资源 ---
    // --- restore runner resource finish already exercised above ---
    // --- log restore runner append/finish ---
    let log_runner =
        StartCheckpointLogRestoreRunnerForTest(&ctx, Duration::from_millis(200), log.as_ref())
            .unwrap();
    AppendRangeForLogRestore(&ctx, &log_runner, "meta-1".into(), 7, 0, 1).unwrap();
    log_runner.WaitForFinish(&ctx, true);
    // 日志 Runner 使用压缩 marshaler；此处只验证 Append 不恐慌且能收尾

    // 默认 tick 必须与 Go 常量一致：flush 30s / checksum 5s / retry 3s
    let cfg = DefaultTickDurationConfig();
    assert_eq!(cfg.tickDurationForFlush, Duration::from_secs(30));
    assert_eq!(cfg.tickDurationForChecksum, Duration::from_secs(5));
    assert_eq!(cfg.retryDuration, Duration::from_secs(3));
    // 若默认常量漂移会导致生产刷盘频率偏离 Go

    // MemSession：同 uuid 连续 segment 合并为完整 payload
    // 表存储路径：用 MemSession 注入 REPLACE 行再 mergeSelect
    let mut sess = MemSession::new();
    let uuid = uuid::Uuid::new_v4();
    sess.ExecuteInternal(
        &ctx,
        "REPLACE INTO db.cpt_data (uuid, segment_id, data) VALUES (%?, %?, %?);",
        &[
            SqlValue::Bytes(uuid.as_bytes().to_vec()),
            SqlValue::U64(0),
            SqlValue::Bytes(b"abc".to_vec()),
        ],
    )
    .unwrap();
    sess.ExecuteInternal(
        &ctx,
        "REPLACE INTO db.cpt_data (uuid, segment_id, data) VALUES (%?, %?, %?);",
        &[
            SqlValue::Bytes(uuid.as_bytes().to_vec()),
            SqlValue::U64(1),
            SqlValue::Bytes(b"def".to_vec()),
        ],
    )
    .unwrap();
    let exec = sess.GetRestrictedSQLExecutor();
    let merged = mergeSelectCheckpoint(&ctx, exec.as_ref(), "db", "cpt_data").unwrap();
    assert_eq!(merged, vec![b"abcdef".to_vec()]);
    // segment 0+1 连续拼接；MemSession 模拟受限 SQL 执行器

    // 边界：segment_id 出现空洞（0 后跳到 2）则整组 uuid 丢弃，不影响完整组
    // 第二组 uuid：故意制造 segment 空洞
    let uuid2 = uuid::Uuid::new_v4();
    sess.ExecuteInternal(
        &ctx,
        "REPLACE INTO db.cpt_data (uuid, segment_id, data) VALUES (%?, %?, %?);",
        &[
            SqlValue::Bytes(uuid2.as_bytes().to_vec()),
            SqlValue::U64(0),
            SqlValue::Bytes(b"x".to_vec()),
        ],
    )
    .unwrap();
    sess.ExecuteInternal(
        &ctx,
        "REPLACE INTO db.cpt_data (uuid, segment_id, data) VALUES (%?, %?, %?);",
        &[
            SqlValue::Bytes(uuid2.as_bytes().to_vec()),
            SqlValue::U64(2),
            SqlValue::Bytes(b"y".to_vec()),
        ],
    )
    .unwrap();
    let merged2 = mergeSelectCheckpoint(&ctx, exec.as_ref(), "db", "cpt_data").unwrap();
    // first uuid intact, second skipped due to gap
    assert!(merged2.iter().any(|b| b == b"abcdef"));
    assert!(!merged2.iter().any(|b| b == b"xy"));
    // gap 策略：发现空洞即标记 uuid 无效并丢弃后续段

    // 显式 Close，避免测试泄漏 session/存储句柄
    // 关闭管理器释放资源，避免测试间互相干扰
    snapshot.Close();
    log.Close();
}
