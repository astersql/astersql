// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/task/backup_test.go`.
//!
//! 覆盖备份侧纯函数：时间戳解析、压缩类型映射、配置 Hash 稳定性、checksum 进度语义。
//! 断言意图对齐 Go；若 Rust Hash 省略 BackendOptions，测试显式记录该差异而非弱化断言。
//! 不启动真实集群，全部在内存配置对象上操作。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::backup::{
    BackupConfig, CompressionConfig, ParseTSString, RunBackup, parseCompressionType,
};
use crate::common::{Config, TLSConfig};
use crate::stubs::backuppb::{CipherInfo, CompressionType};
use crate::stubs::{MemBackupClient, MemGlue, MemMgr, oracle};

/// 对应 Go `TestParseTSString`：空串/数字/日期与时区约束。
#[test]
fn test_parse_ts_string() {
    // 空串表示未指定备份点，返回 0。
    let ts = ParseTSString("", false).unwrap();
    // 期望 0 表示“未指定”。
    assert_eq!(ts, 0);

    // 纯数字按 TSO 字面量解析。
    let ts = ParseTSString("400036290571534337", false).unwrap();
    // 字面量 TSO 原样返回。
    assert_eq!(ts, 400036290571534337);

    // 无显式 offset 时按 Go 的 time.Local 解释，且必须使用真实公历。
    assert_ne!(ParseTSString("2021-01-01 01:42:23", false).unwrap(), 0);
    assert!(ParseTSString("2021-02-29 01:42:23+00:00", true).is_err());

    // 强制时区模式下缺 offset 必须报错。
    let err = ParseTSString("2021-01-01 01:42:23", true).unwrap_err();
    assert!(err.msg.contains("must set timezone"), "err={}", err.msg);

    let ts = ParseTSString("2021-01-01 01:42:23+00:00", true).unwrap();
    assert_eq!(ts, oracle::GoTimeToTS(1_609_465_343_000));

    let ts = ParseTSString("2021-01-01 01:42:23+08:00", true).unwrap();
    assert_eq!(ts, oracle::GoTimeToTS(1_609_436_543_000));

    // Go types.GetTimezone accepts both UTC `Z` and hour-only offsets.
    let ts = ParseTSString("2021-01-01 01:42:23Z", true).unwrap();
    assert_eq!(ts, oracle::GoTimeToTS(1_609_465_343_000));

    let ts = ParseTSString("2021-01-01 01:42:23+08", true).unwrap();
    assert_eq!(ts, oracle::GoTimeToTS(1_609_436_543_000));
}

/// 对应 Go `TestParseCompressionType`：合法名与非法名。
#[test]
fn test_parse_compression_type() {
    // lz4→1 / snappy→2 / zstd→3 与 protobuf 枚举一致。
    assert_eq!(parseCompressionType("lz4").unwrap() as i32, 1);
    // snappy 枚举值。
    assert_eq!(parseCompressionType("snappy").unwrap() as i32, 2);
    // zstd 为默认生产压缩。
    assert_eq!(parseCompressionType("zstd").unwrap() as i32, 3);

    let err = parseCompressionType("Other Compression (strings)").unwrap_err();
    assert!(err.msg.contains("invalid compression"), "err={}", err.msg);
    // Error path leaves caller with UNKNOWN (0) when they discard Ok.
    // 错误路径不应误用 UNKNOWN 当成功值。
    assert_eq!(CompressionType::UNKNOWN as i32, 0);
}

/// Hash 助手：`check=true` 要求不变，`false` 要求变化。
fn hash_check(cfg: &BackupConfig, original_hash: &[u8], check: bool) {
    let hash = cfg.Hash().unwrap();
    // check=true 断言 Hash 稳定；false 断言已变化。
    if check {
        assert_eq!(hash, original_hash);
    } else {
        assert_ne!(hash, original_hash);
    }
}

/// 对应 Go `TestBackupConfigHash`：哪些字段进入 Hash、哪些允许漂移。
#[test]
fn test_backup_config_hash() {
    // 构造含“敏感于 Hash”与“允许变化”两类字段的基线配置。
    let cfg = BackupConfig {
        Config: Config {
            BackendOptions: Default::default(),
            Storage: "storage".into(),
            PD: vec!["pd1".into(), "pd2".into()],
            TLS: TLSConfig::default(),
            RateLimit: 123,
            ChecksumConcurrency: 123,
            Concurrency: 123,
            Checksum: true,
            SendCreds: true,
            LogProgress: true,
            CaseSensitive: true,
            NoCreds: true,
            CheckRequirements: true,
            EnableOpenTracing: true,
            SkipCheckPath: true,
            CipherInfo: CipherInfo {
                CipherKey: b"123".to_vec(),
                ..Default::default()
            },
            FilterStr: vec!["1".into(), "2".into(), "3".into()],
            SwitchModeInterval: Duration::from_secs(1),
            GRPCKeepaliveTime: Duration::from_secs(1),
            GRPCKeepaliveTimeout: Duration::from_secs(1),
            KeyspaceName: "123".into(),
            ..Default::default()
        },
        TimeAgo: Duration::from_secs(1),
        BackupTS: 10,
        LastBackupTS: 1,
        GCTTL: 123,
        RemoveSchedulers: true,
        TableConcurrency: 123,
        IgnoreStats: true,
        UseBackupMetaV2: true,
        UseCheckpoint: true,
        CompressionConfig: CompressionConfig::default(),
        ..Default::default()
    };

    let original_hash = cfg.Hash().unwrap();

    // LastBackupTS 影响增量语义，必须改写 Hash。
    let mut test_cfg = cfg.clone();
    // 增量起点变化应改 Hash。
    test_cfg.LastBackupTS = 0;
    hash_check(&test_cfg, &original_hash, false);

    // UseCheckpoint 进入 Hash，关闭应变化。
    let mut test_cfg = cfg.clone();
    // checkpoint 开关进入 Hash。
    test_cfg.UseCheckpoint = false;
    hash_check(&test_cfg, &original_hash, false);

    // BackendOptions 进入 Go immutableBackupConfig，修改端点必须改变 Hash。
    let mut test_cfg = cfg.clone();
    test_cfg
        .Config
        .BackendOptions
        .S3
        .insert("endpoint".into(), "123".into());
    hash_check(&test_cfg, &original_hash, false);

    // Storage/PD/凭据/过滤器/密钥/keyspace 均为 Hash 输入。
    let mut test_cfg = cfg.clone();
    // 清空 storage 改 Hash。
    test_cfg.Config.Storage.clear();
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // PD 列表缩短改 Hash。
    test_cfg.Config.PD.truncate(1);
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // SendCreds 进入 Hash。
    test_cfg.Config.SendCreds = false;
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // NoCreds 进入 Hash。
    test_cfg.Config.NoCreds = false;
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // 过滤器顺序敏感。
    test_cfg.Config.FilterStr = vec!["3".into(), "2".into(), "1".into()];
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // 清空密钥改 Hash。
    test_cfg.Config.CipherInfo.CipherKey.clear();
    hash_check(&test_cfg, &original_hash, false);

    let mut test_cfg = cfg.clone();
    // keyspace 改名改 Hash。
    test_cfg.Config.KeyspaceName = "321".into();
    hash_check(&test_cfg, &original_hash, false);

    // Allowed-to-change fields must keep the same hash.
    // 运行时调优字段（并发/限速/TLS 路径等）不应改变 checkpoint Hash。
    let mut test_cfg = cfg.clone();
    test_cfg.Config.TLS = TLSConfig {
        CA: "123".into(),
        ..Default::default()
    };
    test_cfg.Config.RateLimit = 321;
    test_cfg.Config.ChecksumConcurrency = 321;
    test_cfg.TableConcurrency = 321;
    test_cfg.Config.Concurrency = 321;
    test_cfg.Config.Checksum = false;
    test_cfg.Config.LogProgress = false;
    test_cfg.Config.CaseSensitive = false;
    test_cfg.Config.CheckRequirements = false;
    test_cfg.Config.EnableOpenTracing = false;
    test_cfg.Config.SkipCheckPath = false;
    test_cfg.Config.CipherInfo = CipherInfo {
        CipherKey: b"123".to_vec(),
        ..Default::default()
    };
    test_cfg.Config.SwitchModeInterval = Duration::from_secs(2);
    test_cfg.Config.GRPCKeepaliveTime = Duration::from_secs(2);
    test_cfg.Config.GRPCKeepaliveTimeout = Duration::from_secs(2);
    test_cfg.TimeAgo = Duration::from_secs(2);
    test_cfg.BackupTS = 100;
    test_cfg.GCTTL = 123;
    test_cfg.RemoveSchedulers = false;
    test_cfg.UseBackupMetaV2 = false;
    test_cfg.CompressionConfig = CompressionConfig {
        CompressionType: CompressionType::LZ4,
        ..Default::default()
    };
    // 批量改调优字段后 Hash 仍应稳定。
    hash_check(&test_cfg, &original_hash, true);
}

/// 对应 Go `TestChecksumProgress`：进度总量随 checksumMap 是否为空切换。
#[test]
fn test_checksum_progress() {
    {
        // 空 map → 进度总量保持 0，避免虚假 100%。
        let mut checksum_progress: i64 = 0;
        let checksum_map: HashMap<i64, (u64, u64, u64)> = HashMap::new();
        if !checksum_map.is_empty() {
            checksum_progress = 5;
        }
        assert_eq!(
            checksum_progress, 0,
            "checksumProgress should be 0 when checksumMap is empty"
        );
    }

    {
        // 非空 map → 进度上限取 schemas.Len()。
        let mut checksum_progress: i64 = 0;
        let mut checksum_map: HashMap<i64, (u64, u64, u64)> = HashMap::new();
        checksum_map.insert(1, (123, 456, 789));
        let schemas_len: i64 = 5;
        if !checksum_map.is_empty() {
            checksum_progress = schemas_len;
        }
        assert_eq!(
            checksum_progress, schemas_len,
            "checksumProgress should equal schemas.Len() when checksumMap is not empty"
        );
    }

    {
        // Go constructs empty BackupSchemas; here schemas_len=0 mirrors Len()==0.
        // Len==0 时不应进入 schema 处理分支。
        let schemas_len: i64 = 0;
        let should_process_schemas = schemas_len > 0;
        assert!(
            !should_process_schemas,
            "Should not process when schemas.Len() is 0"
        );
    }
}

/// Go `RunBackup` defers `mgr.Close`; the owned injected manager must close too.
#[test]
fn test_run_backup_closes_manager() {
    let glue = MemGlue::default();
    let mgr = Arc::new(MemMgr::default());
    let client = MemBackupClient {
        cluster_id: 1,
        current_ts: 100,
        ..Default::default()
    };
    let mut cfg = BackupConfig::default();

    RunBackup(&glue, "Full Backup", &mut cfg, mgr.clone(), &client).unwrap();

    assert!(mgr.closed.load(Ordering::SeqCst));
}

/// Incremental backup must advance beyond its previous backup timestamp.
#[test]
fn test_run_backup_rejects_non_increasing_incremental_ts() {
    let glue = MemGlue::default();
    let mgr = Arc::new(MemMgr::default());
    let client = MemBackupClient::default();
    let mut cfg = BackupConfig {
        BackupTS: 10,
        LastBackupTS: 10,
        ..Default::default()
    };

    let err = RunBackup(&glue, "Full Backup", &mut cfg, mgr.clone(), &client).unwrap_err();

    assert!(
        err.msg
            .contains("LastBackupTS is larger or equal to current TS")
    );
    assert!(mgr.closed.load(Ordering::SeqCst));
}

/// Empty filtered range sets still flush metadata without issuing backup RPCs.
#[test]
fn test_run_backup_skips_rpc_for_empty_ranges() {
    let glue = MemGlue::default();
    let mgr = Arc::new(MemMgr::default());
    let client = MemBackupClient {
        current_ts: 100,
        ranges: Some(Vec::new()),
        ..Default::default()
    };
    let mut cfg = BackupConfig::default();

    RunBackup(&glue, "Database Backup", &mut cfg, mgr, &client).unwrap();

    assert!(!client.backup_called.load(Ordering::SeqCst));
}

/// A named keyspace must never silently back up without GC protection.
#[test]
fn keyspace_backup_requires_gc_manager() {
    let mut cfg = BackupConfig::default();
    cfg.Config.KeyspaceName = "keyspace1".into();
    let mgr = Arc::new(MemMgr::default());
    let client = MemBackupClient {
        current_ts: 100,
        ..Default::default()
    };
    let error = RunBackup(
        &MemGlue::default(),
        "Full Backup",
        &mut cfg,
        mgr.clone(),
        &client,
    )
    .expect_err("keyspace backup without a GC manager must fail before backing up ranges");
    assert!(error.to_string().contains("GC manager"));
    assert!(!client.backup_called.load(Ordering::SeqCst));
    assert!(mgr.closed.load(Ordering::SeqCst));
}

#[derive(Default)]
struct GCTrace {
    events: std::sync::Mutex<Vec<(String, astersql_br_pkg_gc::BRServiceSafePoint)>>,
    fail_set: bool,
}
impl astersql_br_pkg_gc::Manager for GCTrace {
    fn GetGCSafePoint(
        &self,
        _: &astersql_br_pkg_gc::Context,
    ) -> Result<u64, astersql_br_pkg_gc::safepoint::SharedError> {
        Ok(0)
    }
    fn SetServiceSafePoint(
        &self,
        _: &astersql_br_pkg_gc::Context,
        sp: astersql_br_pkg_gc::BRServiceSafePoint,
    ) -> Result<(), astersql_br_pkg_gc::safepoint::SharedError> {
        self.events.lock().unwrap().push(("set".into(), sp));
        if self.fail_set {
            return Err(Box::new(std::io::Error::other("PD rejected barrier")));
        }
        Ok(())
    }
    fn DeleteServiceSafePoint(
        &self,
        _: &astersql_br_pkg_gc::Context,
        sp: astersql_br_pkg_gc::BRServiceSafePoint,
    ) -> Result<(), astersql_br_pkg_gc::safepoint::SharedError> {
        self.events.lock().unwrap().push(("delete".into(), sp));
        Ok(())
    }
}
struct ProtectedBackupClient {
    inner: MemBackupClient,
    trace: Arc<GCTrace>,
    fail: bool,
}
impl crate::stubs::BackupClient for ProtectedBackupClient {
    fn GetSafePointID(&self) -> String {
        "checkpoint-backup-id".into()
    }
    fn GetClusterID(&self) -> u64 {
        self.inner.GetClusterID()
    }
    fn GetCurrentTS(&self) -> crate::stubs::Result<u64> {
        self.inner.GetCurrentTS()
    }
    fn GetStorageBackend(&self) -> Option<crate::stubs::backuppb::StorageBackend> {
        self.inner.GetStorageBackend()
    }
    fn GetApiVersion(&self) -> i32 {
        self.inner.GetApiVersion()
    }
    fn SetStorageAndCheckNotInUse(
        &self,
        b: &crate::stubs::backuppb::StorageBackend,
        o: &crate::stubs::StorageOptions,
    ) -> crate::stubs::Result<()> {
        self.inner.SetStorageAndCheckNotInUse(b, o)
    }
    fn BackupRanges(
        &self,
        _: &[crate::stubs::KeyRange],
        _: &crate::stubs::backuppb::BackupRequest,
    ) -> crate::stubs::Result<u64> {
        let mut events = self.trace.events.lock().unwrap();
        assert_eq!(
            events.last().unwrap().0,
            "set",
            "GC must be active before snapshot reads"
        );
        let sp = events.last().unwrap().1.clone();
        events.push(("backup".into(), sp));
        if self.fail {
            return Err(crate::stubs::Error::new("backup RPC failed"));
        }
        Ok(3)
    }
    fn GetStorage(&self) -> Arc<dyn crate::stubs::Storage> {
        self.inner.GetStorage()
    }
}

#[test]
fn backup_gc_lifecycle_success_failure_and_checkpoint() {
    for (checkpoint, fail_backup, last_ts, expected) in [
        (false, false, 0, vec!["set", "backup", "delete"]),
        (false, true, 0, vec!["set", "backup", "delete"]),
        (true, false, 0, vec!["set", "backup", "delete"]),
        (true, true, 0, vec!["set", "backup"]),
        (false, false, 50, vec!["set", "backup", "delete"]),
    ] {
        let trace = Arc::new(GCTrace::default());
        let manager = Arc::new(MemMgr {
            gc_manager: Some(trace.clone()),
            ..Default::default()
        });
        let client = ProtectedBackupClient {
            inner: MemBackupClient {
                current_ts: 100,
                ..Default::default()
            },
            trace: trace.clone(),
            fail: fail_backup,
        };
        let mut cfg = BackupConfig {
            UseCheckpoint: checkpoint,
            GCTTL: 120,
            LastBackupTS: last_ts,
            ..Default::default()
        };
        cfg.Config.KeyspaceName = "keyspace1".into();
        let result = RunBackup(
            &MemGlue::default(),
            "Full Backup",
            &mut cfg,
            manager.clone(),
            &client,
        );
        assert_eq!(result.is_err(), fail_backup);
        let events = trace.events.lock().unwrap();
        assert_eq!(
            events.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
            expected
        );
        assert!(events.iter().all(|e| e.1.BackupTS == if last_ts == 0 {100} else {last_ts} && e.1.TTL == 120));
        assert!(events.iter().all(|e| e.1.ID == "checkpoint-backup-id"));
        assert!(manager.closed.load(Ordering::SeqCst));
    }
}
#[test]
fn backup_gc_registration_failure_prevents_snapshot_and_cleans_up() {
    let trace = Arc::new(GCTrace {
        fail_set: true,
        ..Default::default()
    });
    let manager = Arc::new(MemMgr {
        gc_manager: Some(trace.clone()),
        ..Default::default()
    });
    let client = ProtectedBackupClient {
        inner: MemBackupClient {
            current_ts: 100,
            ..Default::default()
        },
        trace: trace.clone(),
        fail: false,
    };
    let mut cfg = BackupConfig {
        UseCheckpoint: false,
        GCTTL: 120,
        ..Default::default()
    };
    let error = RunBackup(
        &MemGlue::default(),
        "Full Backup",
        &mut cfg,
        manager,
        &client,
    )
    .unwrap_err();
    assert!(error.to_string().contains("PD rejected barrier"));
    assert_eq!(
        trace
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.0.as_str())
            .collect::<Vec<_>>(),
        ["set", "delete"]
    );
}
