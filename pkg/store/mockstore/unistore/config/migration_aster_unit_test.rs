// Copyright 2026 AsterSQL.

// config 模块迁移后的单元测试（对齐 Go 默认值与解析语义）。
//
// 覆盖压缩算法解析、DefaultConf 字段、duration 单位与非法输入 panic。

use super::config::{CompressionType, DefaultConf, MB, ParseCompression, ParseDuration};
use std::time::Duration;

/// 验证 ParseCompression 与 Go switch 一致（仅小写 snappy/zstd）。
#[test]
fn parse_compression_matches_go_switch() {
    assert_eq!(ParseCompression("snappy"), CompressionType::Snappy);
    assert_eq!(ParseCompression("zstd"), CompressionType::Zstd);
    assert_eq!(ParseCompression("none"), CompressionType::None);
    assert_eq!(ParseCompression("SNAPPY"), CompressionType::None);
}

/// 验证 DefaultConf 各段关键字段与 Go 默认值一致。
#[test]
fn default_conf_matches_go_values() {
    let conf = &*DefaultConf;
    assert_eq!(MB, 1024 * 1024);
    assert_eq!(conf.Server.PDAddr, "127.0.0.1:2379");
    assert_eq!(conf.Server.StoreAddr, "127.0.0.1:9191");
    assert_eq!(conf.Server.StatusAddr, "127.0.0.1:9291");
    assert_eq!(conf.Server.RegionSize, 64 * MB);
    assert_eq!(conf.Server.LogLevel, "info");
    assert_eq!(conf.Server.MaxProcs, 0);
    assert!(conf.Server.Raft);
    assert_eq!(conf.Server.LogfilePath, "");

    assert_eq!(conf.RaftStore.PdHeartbeatTickInterval, "20s");
    assert_eq!(conf.RaftStore.RaftStoreMaxLeaderLease, "9s");
    assert_eq!(conf.RaftStore.RaftBaseTickInterval, "1s");
    assert_eq!(conf.RaftStore.RaftHeartbeatTicks, 2);
    assert_eq!(conf.RaftStore.RaftElectionTimeoutTicks, 10);
    assert!(conf.RaftStore.CustomRaftLog);

    assert_eq!(conf.Engine.DBPath, "/tmp/badger");
    assert_eq!(conf.Engine.ValueThreshold, 256);
    assert_eq!(conf.Engine.MaxMemTableSize, 64 * MB);
    assert_eq!(conf.Engine.MaxTableSize, 8 * MB);
    assert_eq!(conf.Engine.NumMemTables, 3);
    assert_eq!(conf.Engine.NumL0Tables, 4);
    assert_eq!(conf.Engine.NumL0TablesStall, 8);
    assert_eq!(conf.Engine.VlogFileSize, 256 * MB);
    assert!(!conf.Engine.SyncWrite);
    assert_eq!(conf.Engine.NumCompactors, 3);
    assert_eq!(conf.Engine.SurfStartLevel, 8);
    assert_eq!(conf.Engine.L1Size, 512 * MB);
    assert_eq!(conf.Engine.Compression, vec![String::new(); 7]);
    assert_eq!(conf.Engine.IngestCompression, "");
    assert_eq!(conf.Engine.BlockCacheSize, 0);
    assert_eq!(conf.Engine.IndexCacheSize, 0);
    assert!(!conf.Engine.VolatileMode);
    assert!(conf.Engine.CompactL0WhenClose);

    assert_eq!(conf.Coprocessor.RegionMaxKeys, 1_440_000);
    assert_eq!(conf.Coprocessor.RegionSplitKeys, 960_000);
    assert_eq!(conf.PessimisticTxn.WaitForLockTimeout, 1_000);
    assert_eq!(conf.PessimisticTxn.WakeUpDelayDuration, 100);
}

/// 验证带单位解析与纯数字按秒回退。
#[test]
fn parse_duration_matches_go_units_and_seconds_fallback() {
    assert_eq!(ParseDuration("250ms"), Duration::from_millis(250));
    assert_eq!(ParseDuration("1m30s"), Duration::from_secs(90));
    assert_eq!(ParseDuration("1.5s"), Duration::from_millis(1_500));
    assert_eq!(ParseDuration("2"), Duration::from_secs(2));
}

/// 验证非法或负数 duration 会 panic（对齐 Go Fatalf）。
#[test]
fn parse_duration_rejects_invalid_and_negative_values() {
    assert!(std::panic::catch_unwind(|| ParseDuration("invalid")).is_err());
    assert!(std::panic::catch_unwind(|| ParseDuration("-1s")).is_err());
    assert!(std::panic::catch_unwind(|| ParseDuration("1d")).is_err());
}
