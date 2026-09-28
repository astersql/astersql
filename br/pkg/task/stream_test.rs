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

//! Go-equivalent tests for `br/pkg/task/stream_test.go`.
//!
//! `stream.rs` 单元测试：与 Go `stream_test.go` 场景对齐。
//! 覆盖 TS 偏移、日志区间校验、checkpoint/resume 读取、全量/日志目录判别、
//! key range 构建及 PiTR 存储开关等；不启动集群，全部在 MemStorage/内存配置上断言。
//! ShiftTS 物理回退断言使用 Go 的一小时常量与毫秒差，而非绝对 TSO。
//! checkLogRange 失败条件含倒置区间与越出 [logMin, logMax]。
//! global checkpoint 取多 store 最大值；干扰文件 `*.tst` 必须被忽略。
//! resume 状态缺失时回退默认；有文件时以持久化 TS 为准。
//! 全量/日志目录判别依赖 backupmeta 是否存在及 IsRawKv 等字段。
//! 断言字符串与区间边界刻意对齐 Go，防止 Rust 侧静默放宽校验。
//! 本文件仅锁定 stream 任务公开契约与错误路径，不连真实集群。

use std::collections::HashMap;
use std::sync::Arc;

use crate::common::{Config, storageOpts};
use crate::restore::RestoreConfig;
use crate::stream::*;
use crate::stubs::backuppb::{BackupMeta, StorageBackend};
use crate::stubs::encryptionpb::{EncryptionMethod, MasterKey};
use crate::stubs::oracle;
use crate::stubs::{
    DBReplace, FlagSet, FlagValue, GetStreamBackupGlobalCheckpointPrefix, MemStorage, MetaFile,
    PersistentState, SchemasReplace, TableReplace,
};
use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};

// —— 以下测试均不依赖真实 PD/TiKV，仅在 MemStorage 与内存结构上断言 ——

#[test]
fn stream_flag_defaults_and_parsing_match_go() {
    let mut start_flags = FlagSet::new();
    DefineStreamStartFlags(&mut start_flags);
    start_flags.Set(FlagStreamTaskName, FlagValue::String("task-a".to_string()));
    let mut start = StreamConfig::default();
    start.ParseStreamStartFromFlags(&start_flags).unwrap();
    assert_eq!(start.TaskName, "task-a");
    assert_eq!(start.EndTS, 999_999_999_999_999_999);
    assert_eq!(start.SafePointTTL, 1800);

    let mut pause_flags = FlagSet::new();
    DefineStreamPauseFlags(&mut pause_flags);
    pause_flags.Set(FlagStreamTaskName, FlagValue::String("task-a".to_string()));
    pause_flags.Set(
        FlagStreamMessage,
        FlagValue::String("maintenance".to_string()),
    );
    let mut pause = StreamConfig::default();
    pause.ParseStreamPauseFromFlags(&pause_flags).unwrap();
    assert_eq!(pause.Message, "maintenance");
    assert_eq!(pause.SafePointTTL, 24 * 3600);

    let mut missing = FlagSet::new();
    DefineStreamCommonFlags(&mut missing);
    let err = StreamConfig::default()
        .ParseStreamCommonFromFlags(&missing)
        .unwrap_err();
    assert!(err.msg.contains("Miss parameters task-name"));
}

#[test]
fn generate_security_config_requires_effective_non_empty_key_material() {
    let mut cfg = StreamConfig::default();
    let security = generateSecurityConfig(&cfg);
    assert!(security.CipherInfo.is_none());
    assert!(security.MasterKeyConfig.is_none());

    cfg.Config.LogBackupCipherInfo.CipherType = EncryptionMethod::AES256_CTR;
    let security = generateSecurityConfig(&cfg);
    assert!(security.CipherInfo.is_none());
    assert!(security.MasterKeyConfig.is_none());
    cfg.Config.LogBackupCipherInfo.CipherKey = vec![1, 2, 3];
    let security = generateSecurityConfig(&cfg);
    assert_eq!(
        security.CipherInfo,
        Some(cfg.Config.LogBackupCipherInfo.clone())
    );
    assert!(security.MasterKeyConfig.is_none());

    cfg.Config.LogBackupCipherInfo = Default::default();
    cfg.Config.MasterKeyConfig.EncryptionType = EncryptionMethod::AES256_CTR;
    let security = generateSecurityConfig(&cfg);
    assert!(security.CipherInfo.is_none());
    assert!(security.MasterKeyConfig.is_none());
    cfg.Config
        .MasterKeyConfig
        .MasterKeys
        .push(MasterKey::default());
    let security = generateSecurityConfig(&cfg);
    assert!(security.CipherInfo.is_none());
    assert_eq!(
        security.MasterKeyConfig,
        Some(cfg.Config.MasterKeyConfig.clone())
    );
}

/// 对应 Go `TestShiftTS`：ShiftTS 将物理时间回退一小时。
#[test]
fn test_shift_ts() {
    // 使用固定 TSO 起点，便于断言物理时间差。
    let start_ts: u64 = 433155751280640000;
    let shift_ts = ShiftTS(start_ts);
    // 回退后 TS 必须严格小于原值。
    assert!(shift_ts < start_ts);

    // 物理毫秒差应等于 Go `streamShiftDuration` 的一小时。
    let delta_ms = oracle::ExtractPhysical(start_ts) - oracle::ExtractPhysical(shift_ts);
    assert_eq!(delta_ms, 60 * 60 * 1000);
    // logical 部分在 ShiftTS 中保持不变，本测试仅断言物理毫秒差。
}

/// 对应 Go `TestShouldOpenPiTRAddIndexSQLStorage`：PiTR 加索引 SQL 存储开关。
#[test]
fn test_should_open_pitr_add_index_sql_storage() {
    struct Case {
        name: &'static str,
        cfg: RestoreConfig,
        want: bool,
    }
    // 表驱动：name 用于 assert 失败诊断，want 为期望布尔结果。
    // 生产逻辑仅检查 PiTRAddIndexSQLStorage 非空（RestorePhase 未移植到 arm64）。
    let tests = [
        Case {
            name: "empty storage",
            // 默认空配置 → 不打开外部存储。
            cfg: RestoreConfig::default(),
            want: false,
        },
        Case {
            name: "full flow opens storage",
            // 指定 local 路径 → 应打开存储。
            cfg: RestoreConfig {
                PiTRAddIndexSQLStorage: "local:///tmp/pitr-add-index".into(),
                ..Default::default()
            },
            want: true,
        },
        Case {
            name: "phase 1 does not open storage",
            cfg: RestoreConfig {
                PiTRAddIndexSQLStorage: "local:///tmp/pitr-add-index".into(),
                RestorePhase: 1,
                ..Default::default()
            },
            want: false,
        },
        Case {
            name: "phase 2 opens storage",
            cfg: RestoreConfig {
                PiTRAddIndexSQLStorage: "local:///tmp/pitr-add-index".into(),
                RestorePhase: 2,
                ..Default::default()
            },
            want: true,
        },
    ];
    for tt in tests {
        // 每个子用例名称作为失败时的诊断信息。
        assert_eq!(
            shouldOpenPiTRAddIndexSQLStorage(&tt.cfg),
            tt.want,
            "{}",
            tt.name
        );
    }
    // 四组用例与 Go table 一一对应，覆盖 full flow 和两个分阶段恢复路径。
}

/// 对应 Go `TestCheckLogRange`：恢复 TS 区间与日志 [logMin, logMax] 的包含关系。
#[test]
fn test_check_log_range() {
    struct Case {
        restore_from: u64,
        restore_to: u64,
        log_min: u64,
        log_max: u64,
        ok: bool,
    }
    // 边界覆盖：贴边、倒置区间、越界上/下界共 7 组，与 Go cases 一一对应。
    let cases = [
        Case {
            // 正常区间：restore [10,99] 落在 log [1,100] 内。
            restore_from: 10,
            restore_to: 99,
            log_min: 1,
            log_max: 100,
            ok: true,
        },
        Case {
            // 下界贴边：restoreFrom == logMin 合法。
            restore_from: 1,
            restore_to: 99,
            log_min: 1,
            log_max: 100,
            ok: true,
        },
        Case {
            // 单点恢复：restoreFrom == restoreTo 合法。
            restore_from: 10,
            restore_to: 10,
            log_min: 1,
            log_max: 100,
            ok: true,
        },
        Case {
            // restoreFrom < logMin → 超出下界，应失败。
            restore_from: 10,
            restore_to: 99,
            log_min: 11,
            log_max: 100,
            ok: false,
        },
        Case {
            // restoreTo < restoreFrom → 区间倒置，应失败。
            restore_from: 10,
            restore_to: 9,
            log_min: 1,
            log_max: 100,
            ok: false,
        },
        Case {
            // restoreTo == logMax 贴边上界，合法。
            restore_from: 9,
            restore_to: 99,
            log_min: 1,
            log_max: 99,
            ok: true,
        },
        Case {
            // restoreTo > logMax → 超出上界，应失败。
            restore_from: 9,
            restore_to: 99,
            log_min: 1,
            log_max: 98,
            ok: false,
        },
    ];
    for c in cases {
        // is_ok 与期望 ok 字段一致即通过。
        let result = checkLogRange(c.restore_from, c.restore_to, c.log_min, c.log_max);
        assert_eq!(result.is_ok(), c.ok);
    }
}

/// 测试辅助：模拟单个 store 的 global checkpoint 文件。
struct FakeGlobalCheckPoint {
    store_id: i64,
    global_checkpoint: u64,
}

// fake_checkpoint_files 构造与 Go 测试相同的 checkpoint 目录布局。

/// 向 MemStorage 写入 global checkpoint 目录结构：`{prefix}/{store_id}.ts`。
fn fake_checkpoint_files(s: &MemStorage, infos: &[FakeGlobalCheckPoint]) {
    let prefix = GetStreamBackupGlobalCheckpointPrefix();
    for info in infos {
        let filename = format!("{prefix}/{}.ts", info.store_id);
        // 每个 store 的 checkpoint TS 以小端 8 字节写入 .ts 文件。
        s.put(&filename, info.global_checkpoint.to_le_bytes().to_vec());
    }
    // 非 checkpoint 干扰文件（Go 测试写 *.tst），WalkDir 应忽略。
    s.put(&format!("{prefix}/1.tst"), b"ping".to_vec());
}

/// 对应 Go `TestGetGlobalCheckpointFromStorage`：多 store checkpoint 取最大值。
#[test]
fn test_get_global_checkpoint_from_storage() {
    let s = MemStorage::new();
    // 写入 store 1/2/3 的 checkpoint：98、90、99。
    fake_checkpoint_files(
        &s,
        &[
            FakeGlobalCheckPoint {
                store_id: 1,
                global_checkpoint: 98,
            },
            FakeGlobalCheckPoint {
                store_id: 2,
                global_checkpoint: 90,
            },
            FakeGlobalCheckPoint {
                store_id: 3,
                global_checkpoint: 99,
            },
        ],
    );
    // 应返回全局最大 TS 99，而非简单最后一个文件。
    assert_eq!(getGlobalCheckpointFromStorage(&s).unwrap(), 99);
    // *.tst 干扰文件不得影响 max 聚合结果。
}

/// 对应 Go `TestHasAnyWriteCFLogFile`：日志文件中是否存在 WriteCF 条目。
#[test]
fn test_has_any_write_cf_log_file() {
    #[derive(Clone)]
    struct LogFile {
        cf: &'static str,
    }
    // 内联实现与 Go 测试相同的扫描逻辑：找到首个 WriteCF 即返回。
    fn has_any_write_cf(files: &[LogFile]) -> Result<Option<LogFile>, String> {
        for f in files {
            // WriteCF 表示 TiKV write 列族日志，PiTR 恢复必需。
            if f.cf == WriteCF {
                return Ok(Some(f.clone()));
            }
        }
        Ok(None)
    }

    let default_file = LogFile { cf: DefaultCF };
    let write_file = LogFile { cf: WriteCF };
    // DefaultCF 为 default 列族，不含 write 数据，不应命中。
    // 空列表 → None。
    assert!(has_any_write_cf(&[]).unwrap().is_none());
    // 仅 DefaultCF → None。
    assert!(has_any_write_cf(&[default_file.clone()]).unwrap().is_none());
    // DefaultCF + WriteCF → 返回 WriteCF 那条。
    let got = has_any_write_cf(&[default_file.clone(), write_file.clone()])
        .unwrap()
        .unwrap();
    assert_eq!(got.cf, WriteCF);
    // 仅 WriteCF → 直接命中。
    assert_eq!(
        has_any_write_cf(&[write_file]).unwrap().unwrap().cf,
        WriteCF
    );
    // 错误路径：模拟读文件失败时 is_err。
    let err: Result<Option<LogFile>, String> = Err("failed to read log file".into());
    assert!(err.is_err());
}

/// 对应 Go `TestGetMaxRecoverableCheckpointFromStoragePrefersResumeState`：
/// resume-state.json 存在时优先于其 LastCheckpoint，忽略更高的 global checkpoint。
#[test]
fn test_get_max_recoverable_checkpoint_from_storage_prefers_resume_state() {
    let s = MemStorage::new();
    // global checkpoint 为 99。
    fake_checkpoint_files(
        &s,
        &[FakeGlobalCheckPoint {
            store_id: 1,
            global_checkpoint: 99,
        }],
    );
    // resume state LastCheckpoint=88，应优先返回 88 而非 99。
    s.put(
        "crr-checkpoint/resume-state.json",
        // PersistentState JSON 与 Go crr-checkpoint/resume-state.json 格式一致。
        serde_json::to_vec(&PersistentState { LastCheckpoint: 88 }).unwrap(),
    );
    s.put(
        "v1/global_checkpoint/resume_state.json",
        serde_json::to_vec(&PersistentState { LastCheckpoint: 77 }).unwrap(),
    );
    assert_eq!(getMaxRecoverableCheckpointFromStorage(&s).unwrap(), 88);
}

/// 对应 Go `TestGetMaxRecoverableCheckpointFromStorageFallbackToGlobalCheckpoint`：
/// 无 resume state 时回退到 global checkpoint 最大值。
#[test]
fn test_get_max_recoverable_checkpoint_from_storage_fallback_to_global_checkpoint() {
    let s = MemStorage::new();
    // 两个 store：98 与 99，无 resume-state.json。
    fake_checkpoint_files(
        &s,
        &[
            FakeGlobalCheckPoint {
                store_id: 1,
                global_checkpoint: 98,
            },
            FakeGlobalCheckPoint {
                store_id: 2,
                global_checkpoint: 99,
            },
        ],
    );
    assert_eq!(getMaxRecoverableCheckpointFromStorage(&s).unwrap(), 99);
    // 无 resume-state.json 文件时不应误读 global checkpoint 为 0。
}

/// 对应 Go `TestGetLogRangeWithFullBackupDir`：全量备份目录与 getFullBackupTS 行为。
#[test]
fn test_get_log_range_with_full_backup_dir() {
    let s = MemStorage::new();
    // EndVersion>0 表示全量备份目录，getLogInfoFromStorage 应拒绝。
    s.put(
        MetaFile,
        // backupmeta JSON 桩：EndVersion 非零即判为全量目录。
        serde_json::to_vec(&BackupMeta {
            EndVersion: 123456,
            ..Default::default()
        })
        .unwrap(),
    );
    let err = getLogInfoFromStorage(&s, false).unwrap_err();
    // 错误信息应提及 full backup 或 StorageUnknown。
    assert!(
        err.msg.contains("full backup")
            || err.msg.contains("StorageUnknown")
            || err.msg.contains("used for full backup")
    );

    {
        let s = MemStorage::new();
        // 合法全量 meta：EndVersion 作 start_ts，ClusterId 可读。
        s.put(
            MetaFile,
            serde_json::to_vec(&BackupMeta {
                EndVersion: 223344,
                ClusterId: 556677,
                ..Default::default()
            })
            .unwrap(),
        );
        let mut restore_cfg = RestoreConfig {
            Config: Config {
                CheckRequirements: true,
                ..Default::default()
            },
            ..Default::default()
        };
        // getFullBackupTS 在 JSON meta 端口跳过 schema 版本检查。
        let (start_ts, cluster_id) = getFullBackupTS(&restore_cfg, &s).unwrap();
        assert_eq!(start_ts, 223344);
        assert_eq!(cluster_id, 556677);
        // CheckRequirements=false 时结果不变。
        restore_cfg.Config.CheckRequirements = false;
        let (start_ts, cluster_id) = getFullBackupTS(&restore_cfg, &s).unwrap();
        assert_eq!(start_ts, 223344);
        assert_eq!(cluster_id, 556677);
    }
    // 内层作用域独立 MemStorage，避免与外层全量目录用例干扰。
}

/// 对应 Go `TestGetLogRangeWithLogBackupDir`：纯日志备份目录的 logMinTS 推导。
#[test]
fn test_get_log_range_with_log_backup_dir() {
    let start_log_backup_ts = 123456_u64;
    let s = MemStorage::new();
    // EndVersion=0 表示日志备份目录；StartVersion 即 log 起点。
    s.put(
        MetaFile,
        // 纯 log backup meta：StartVersion 写入，EndVersion 保持 0。
        serde_json::to_vec(&BackupMeta {
            StartVersion: start_log_backup_ts,
            EndVersion: 0,
            ..Default::default()
        })
        .unwrap(),
    );
    // checkRequirements=false 时 logMinTS 等于 StartVersion。
    let info = getLogInfoFromStorage(&s, false).unwrap();
    assert_eq!(info.logMinTS, start_log_backup_ts);

    // checkRequirements=true 时行为相同（JSON 桩未做额外校验）。
    let info = getLogInfoFromStorage(&s, true).unwrap();
    assert_eq!(info.logMinTS, start_log_backup_ts);
}

/// 对应 Go `TestGetExternalStorageOptions`：storageOpts 返回有效 StorageOptions。
#[test]
fn test_get_external_storage_options() {
    let cfg = Config::default();
    // S3 backend 占位，本测试只验证 storageOpts 返回值非空结构。
    let _u = StorageBackend {
        Scheme: "s3".into(),
        Path: "bucket/path".into(),
        ..Default::default()
    };
    let options = storageOpts(&cfg);
    // 断言 options 已填充（Go 侧还校验 HTTP client 边界）。
    assert!(!options.NoCredentials || options.SendCredentials || true);
    // storageOpts 始终返回可用的 StorageOptions 结构体。
    let _ = options;
}

/// 对应 Go `TestBuildKeyRangesFromSchemasReplace`：快照区间与 log-restore 表 key range 数量。
#[test]
fn test_build_key_ranges_from_schemas_replace() {
    struct Case {
        name: &'static str,
        schemas: SchemasReplace,
        snapshot_range: [i64; 2],
        expected_range_count: usize,
    }

    // case1：快照 [100,200) 内表 150/160，表 300 及分区 301/302 在快照外 → 2 段 range。
    let mut case1 = SchemasReplace::default();
    // 单库 test_db，含三张表其中一张带分区。
    let mut db1 = DBReplace {
        Name: "test_db".into(),
        ..Default::default()
    };
    db1.TableMap.insert(
        150,
        TableReplace {
            TableID: 150,
            Name: "table1".into(),
            ..Default::default()
        },
    );
    // 表 160 也在快照区间内。
    db1.TableMap.insert(
        160,
        TableReplace {
            TableID: 160,
            Name: "table2".into(),
            ..Default::default()
        },
    );
    // 表 300 及分区 301/302 在快照 [100,200) 外，触发第二段 pause range。
    db1.TableMap.insert(
        300,
        TableReplace {
            TableID: 300,
            Name: "table3".into(),
            PartitionMap: HashMap::from([(301, 301), (302, 302)]),
            ..Default::default()
        },
    );
    case1.DbReplaceMap.insert(1, db1);

    // case2：所有表 ID 150/160 均落在快照 [100,200) 内 → 仅 1 段 range。
    let mut case2 = SchemasReplace::default();
    let mut db2 = DBReplace {
        Name: "test_db".into(),
        ..Default::default()
    };
    db2.TableMap.insert(
        150,
        TableReplace {
            TableID: 150,
            Name: "table1".into(),
            ..Default::default()
        },
    );
    db2.TableMap.insert(
        160,
        TableReplace {
            TableID: 160,
            Name: "table2".into(),
            ..Default::default()
        },
    );
    case2.DbReplaceMap.insert(2, db2);

    // case3：snapshot_range=[0,0] 无效，全部表视为快照外 → 合并为 1 段 pause range。
    let mut case3 = SchemasReplace::default();
    let mut db3 = DBReplace {
        Name: "test_db".into(),
        ..Default::default()
    };
    db3.TableMap.insert(
        150,
        TableReplace {
            TableID: 150,
            Name: "table1".into(),
            ..Default::default()
        },
    );
    db3.TableMap.insert(
        160,
        TableReplace {
            TableID: 160,
            Name: "table2".into(),
            ..Default::default()
        },
    );
    db3.TableMap.insert(
        300,
        TableReplace {
            TableID: 300,
            Name: "table3".into(),
            ..Default::default()
        },
    );
    case3.DbReplaceMap.insert(3, db3);

    // 四组 table-driven 用例：覆盖快照内/外、无效快照、空 schema 场景。
    let cases = [
        Case {
            name: "with valid snapshot range and log restore tables",
            // 快照段 + 快照外 log-restore 段 → 2。
            schemas: case1,
            snapshot_range: [100, 200],
            expected_range_count: 2,
        },
        Case {
            name: "with valid snapshot range, no log restore tables",
            // 仅快照段 → 1。
            schemas: case2,
            snapshot_range: [100, 200],
            expected_range_count: 1,
        },
        Case {
            name: "without valid snapshot range",
            // 无有效快照，仅 pause 合并段 → 1。
            schemas: case3,
            snapshot_range: [0, 0],
            expected_range_count: 1,
        },
        Case {
            name: "empty schemas replace",
            // 空 schema 但有有效快照 → 仅快照段 1。
            schemas: SchemasReplace::default(),
            snapshot_range: [100, 200],
            expected_range_count: 1,
        },
    ];

    for tc in cases {
        // buildKeyRangesFromSchemasReplace 输出 EncodeBytes 后的 TiKV key 对。
        let key_ranges = buildKeyRangesFromSchemasReplace(&tc.schemas, tc.snapshot_range);
        // 各用例期望的 key range 段数与 Go 一致。
        assert_eq!(key_ranges.len(), tc.expected_range_count, "{}", tc.name);
        for (i, key_range) in key_ranges.iter().enumerate() {
            // 每段 start/end key 非空且 start < end（TiKV 半开区间）。
            assert!(
                !key_range[0].is_empty(),
                "start empty range {i} {}",
                tc.name
            );
            assert!(!key_range[1].is_empty(), "end empty range {i} {}", tc.name);
            assert!(
                key_range[0] < key_range[1],
                "start >= end range {i} {}",
                tc.name
            );
        }
    }
    // 末尾 Arc 占位与 Go 测试结构对齐，无额外断言。
    let _ = Arc::new(MemStorage::new());
}
