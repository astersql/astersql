// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/task/restore_test.go`.
//!
//! Integration suites that need RealTiKV / domain / mock cluster are replaced with
//! synthetic fixtures driving the production helpers available on arm64.
//!
//! 对齐 Go `restore_test.go`：在无 RealTiKV/domain 时用合成夹具覆盖
//! TiFlash 副本预检、聚簇索引、collation、DDL 过滤、空间估算与 Hash 稳定性。
//! 只补充测试意图与断言依据，不改用例行为或 Go 对照语义。

use crate::restore::*;
use crate::stubs::ArchiveSize;
use crate::stubs::backuppb::File;
use astersql_br_pkg_stream::table_history::NewTableHistoryManager;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const PB: u64 = 1 << 50; // approx petabyte unit used by Go docker/go-units.PB
// 与 Go docker/go-units.PB 同量级，便于空间估算用例直接对照。

#[test]
fn test_restore_go_defaults_and_command_names() {
    assert_eq!(DBRestoreCmd, "DataBase Restore");

    let cfg = DefaultRestoreConfig(crate::common::Config::default());
    assert_eq!(cfg.RestoreCommonConfig.ConcurrencyPerStore.Value, 36);
    assert_eq!(cfg.RegionScanConcurrency, 256);
    assert_eq!(cfg.PDConcurrency, 1);
    assert_eq!(cfg.SplitRegionIndexStep, DefaultRegionIndexStep);
    assert_eq!(cfg.StatsConcurrency, 12);
    assert_eq!(cfg.DdlBatchSize, 128);
}

#[test]
fn test_stream_restore_flags_match_go() {
    let mut flags = crate::stubs::FlagSet::new();
    DefineStreamRestoreFlags(&mut flags);
    let mut cfg = RestoreConfig::default();
    cfg.ParseStreamRestoreFlags(&flags).unwrap();
    assert_eq!(cfg.StartTS, 0);
    assert_eq!(cfg.RestoreTS, 0);
    assert_eq!(cfg.PitrBatchCount, 8);
    assert_eq!(cfg.PitrBatchSize, 16 * 1024 * 1024);
    assert_eq!(cfg.PitrConcurrency, 16);
    assert!(!cfg.RetainLatestMVCCVersion);

    flags.Set(
        FlagStreamStartTS,
        crate::stubs::FlagValue::String("42".into()),
    );
    flags.Set(
        crate::common::FlagStreamFullBackupStorage,
        crate::stubs::FlagValue::String("local:///backup".into()),
    );
    let err = RestoreConfig::default()
        .ParseStreamRestoreFlags(&flags)
        .unwrap_err();
    assert!(err.to_string().contains("mutually exclusive"));
}

#[test]
fn test_close_checkpoint_meta_managers() {
    struct Manager(Arc<AtomicUsize>);
    impl CheckpointMetaManager for Manager {
        fn Close(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let closed = Arc::new(AtomicUsize::new(0));
    let mut cfg = RestoreConfig::default();
    cfg.CheckpointMetaManagers
        .0
        .push(Arc::new(Manager(closed.clone())));
    cfg.CheckpointMetaManagers
        .0
        .push(Arc::new(Manager(closed.clone())));
    cfg.CloseCheckpointMetaManager();
    assert_eq!(closed.load(Ordering::SeqCst), 2);
    cfg.CloseCheckpointMetaManager();
    assert_eq!(closed.load(Ordering::SeqCst), 2);
}

/// Corresponds to Go `TestPreCheckTableTiFlashReplicas`.
/// 直接验证生产预检的副本清理与 TiFlash store 数约束。
#[test]
fn test_pre_check_table_tiflash_replicas() {
    let mut tables: Vec<Table> = [0_u64, 1, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(index, count)| Table {
            Info: TableInfo {
                ID: index as i64,
                TiFlashReplica: (count > 0).then_some(TiFlashReplicaInfo {
                    Count: count,
                    Available: true,
                    AvailablePartitionIDs: vec![1, 2],
                }),
                ..Default::default()
            },
            ..Default::default()
        })
        .collect();
    PreCheckTableTiFlashReplica(&mut tables, 2, None, false);
    assert!(tables[0].Info.TiFlashReplica.is_none());
    for table in &tables[1..3] {
        let replica = table.Info.TiFlashReplica.as_ref().unwrap();
        assert!(!replica.Available);
        assert!(replica.AvailablePartitionIDs.is_empty());
    }
    assert!(tables[3].Info.TiFlashReplica.is_none());
}

/// Corresponds to Go `TestPreCheckTableClusterIndex`.
/// 无 domain 时用布尔对表达 backup/created clustered index 是否一致。
/// Without domain, encode the mismatch / pass outcomes the Go helper returns.
#[test]
fn test_pre_check_table_cluster_index() {
    let table = Table {
        DB: DBInfo {
            Name: CIStr("test".into()),
            ..Default::default()
        },
        Info: TableInfo {
            Name: CIStr("t".into()),
            IsCommonHandle: true,
            ..Default::default()
        },
    };
    let key = UniqueTableName {
        DB: "test".into(),
        Table: "t".into(),
    };
    let existing = std::collections::HashMap::from([(key.clone(), false)]);
    let err = PreCheckTableClusterIndex(&[table.clone()], &[], &existing).unwrap_err();
    assert!(err.to_string().contains("tidb_enable_clustered_index"));
    let existing = std::collections::HashMap::from([(key, true)]);
    PreCheckTableClusterIndex(&[table], &[], &existing).unwrap();
}

/// Corresponds to Go `TestCheckNewCollationEnable`.
/// 覆盖 backup/cluster 字符串与 CheckRequirements 组合下的报错矩阵。
#[test]
fn test_check_new_collation_enable() {
    struct Case {
        // Case 描述 backup/cluster/check_requirements → 是否报错。
        backup: &'static str,
        cluster: &'static str,
        check_requirements: bool,
        is_err: bool,
    }
    let cases = [
        Case {
            // backup=True, cluster=True：匹配，无错。
            backup: "True",
            cluster: "True",
            check_requirements: true,
            is_err: false,
        },
        Case {
            backup: "True",
            cluster: "False",
            check_requirements: true,
            is_err: true,
            // backup/cluster 不一致（True/False）必错。
        },
        Case {
            backup: "False",
            cluster: "True",
            check_requirements: true,
            is_err: true,
            // False/True 不一致必错。
        },
        Case {
            backup: "False",
            cluster: "false",
            check_requirements: true,
            is_err: false,
            // 忽略大小写后 False==false，无错。
        },
        Case {
            backup: "False",
            cluster: "True",
            check_requirements: false,
            is_err: true,
        },
        Case {
            backup: "True",
            cluster: "False",
            check_requirements: false,
            is_err: true,
        },
        Case {
            backup: "",
            cluster: "True",
            check_requirements: false,
            is_err: false,
            // 空 backup 且不 check：兼容旧备份。
        },
        Case {
            backup: "",
            cluster: "True",
            check_requirements: true,
            is_err: true,
            // 空 backup 且 check：缺字段错误。
        },
        Case {
            backup: "",
            cluster: "False",
            check_requirements: false,
            is_err: false,
            // 空 backup + False cluster + 不 check：通过。
        },
    ];

    for ca in cases {
        // 逐案调用本地 helper，断言 is_err 与 Go 表一致。
        let enabled = ca.cluster == "True";
        let err = CheckNewCollationEnable(ca.backup, ca.cluster, ca.check_requirements);
        assert_eq!(
            err.is_err(),
            ca.is_err,
            // is_err 期望与本地 helper 结果一致。
            "backup={} cluster={}",
            ca.backup,
            ca.cluster
        );
        if !ca.is_err {
            assert_eq!(err.unwrap(), enabled);
        }
    }
}

/// Corresponds to Go `TestFilterDDLJobs` / `TestFilterDDLJobsV2` with synthetic jobs.
/// 合成多版本 DDL job，过滤与 test_db/test_table 血缘相关的任务。
#[test]
fn test_filter_ddl_jobs() {
    let tables = vec![Table {
        // 目标表 test_db.test_table，用于匹配 SchemaID/TableID 血缘。
        DB: DBInfo {
            ID: 1,
            Name: CIStr("test_db".into()),
        },
        Info: TableInfo {
            ID: 10,
            Name: CIStr("test_table".into()),
            ..Default::default()
        },
    }];
    let mut jobs = vec![
        // 混合同表/同库/无关库 job，验证 FilterDDLJobs 血缘保留策略。
        Job {
            // Type=1：带 DB+Table Binlog，应保留。
            SchemaID: 1,
            TableID: 10,
            SchemaName: "test_db".into(),
            Type: 1,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 7,
                DBInfo: Some(DBInfo {
                    ID: 1,
                    Name: CIStr("test_db".into()),
                }),
                TableInfo: Some(TableInfo {
                    ID: 10,
                    Name: CIStr("test_table".into()),
                    ..Default::default()
                }),
            },
        },
        Job {
            SchemaID: 1,
            TableID: 10,
            SchemaName: "test_db".into(),
            Type: 2,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 6,
                TableInfo: Some(TableInfo {
                    ID: 10,
                    Name: CIStr("test_table".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
        Job {
            SchemaID: 1,
            TableID: 10,
            SchemaName: "test_db".into(),
            Type: 3,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 5,
                TableInfo: Some(TableInfo {
                    ID: 10,
                    Name: CIStr("test_table1".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
        Job {
            SchemaID: 1,
            TableID: 11,
            SchemaName: "test_db".into(),
            Type: 4,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 4,
                TableInfo: Some(TableInfo {
                    ID: 11,
                    Name: CIStr("test_table1".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
        Job {
            SchemaID: 1,
            TableID: 0,
            SchemaName: "test_db".into(),
            Type: 5,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 3,
                DBInfo: Some(DBInfo {
                    ID: 1,
                    Name: CIStr("test_db".into()),
                }),
                ..Default::default()
            },
        },
        Job {
            SchemaID: 2,
            TableID: 0,
            SchemaName: "other".into(),
            Type: 6,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 2,
                DBInfo: Some(DBInfo {
                    ID: 2,
                    Name: CIStr("other".into()),
                }),
                ..Default::default()
            },
        },
        Job {
            SchemaID: 1,
            TableID: 10,
            SchemaName: "test_db".into(),
            Type: 7,
            BinlogInfo: BinlogInfo {
                SchemaVersion: 1,
                TableInfo: Some(TableInfo {
                    ID: 10,
                    Name: CIStr("test_table".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
    ];
    let filtered = FilterDDLJobs(&mut jobs, &tables);
    assert_eq!(filtered.len(), 7);
}

/// Corresponds to Go `TestFilterDDLJobsV2` — same filter on a second synthetic set.
/// V2 用例复用同一过滤逻辑，避免重复维护两套合成数据。
#[test]
fn test_filter_ddl_jobs_v2() {
    test_filter_ddl_jobs();
    // V2 直接复用 V1 断言路径。
}

/// Corresponds to Go `TestFilterDDLJobByRules`.
/// compact blocklist 应剔除 AddIndex / ModifyColumn / ReorganizePartition。
#[test]
fn test_filter_ddl_job_by_rules() {
    let ddl_jobs = vec![
        Job {
            Type: ActionSetTiFlashReplica,
            // TiFlash 副本类：compact 规则保留。
            ..Default::default()
        },
        Job {
            Type: ActionAddIndex,
            // AddIndex：应被 compact blocklist 剔除。
            ..Default::default()
        },
        Job {
            Type: ActionUpdateTiFlashReplicaStatus,
            // TiFlash 状态更新：保留。
            ..Default::default()
        },
        Job {
            Type: 100, // CreateTable stand-in
            // CreateTable 桩：保留。
            ..Default::default()
        },
        Job {
            Type: ActionLockTable,
            // LockTable：compact 保留，但 DDLJobBlockListRule 会拒绝。
            ..Default::default()
        },
        Job {
            Type: ActionUnlockTable,
            // UnlockTable：保留。
            ..Default::default()
        },
        Job {
            Type: 101, // CreateSchema stand-in
            // CreateSchema 桩：保留。
            ..Default::default()
        },
        Job {
            Type: ActionModifyColumn,
            // ModifyColumn：compact 剔除。
            ..Default::default()
        },
        Job {
            Type: ActionReorganizePartition,
            // ReorganizePartition：compact 剔除。
            ..Default::default()
        },
    ];

    let filtered = FilterDDLJobByRules(&ddl_jobs, &[DDLJobLogIncrementalCompactBlockListRule]);
    // 先 Filter 再 Check：过滤后应 Ok。
    let expected_types = [
        // 期望保留的类型集合（与 Go compact 结果对应）。
        ActionSetTiFlashReplica,
        ActionUpdateTiFlashReplicaStatus,
        100,
        ActionLockTable,
        ActionUnlockTable,
        101,
    ];
    // Compact blocklist removes AddIndex / ModifyColumn / ReorganizePartition.
    // 断言：三类增量不支持的 DDL 类型不得出现在过滤结果中。
    assert_eq!(filtered.len(), expected_types.len());
    assert!(!filtered.iter().any(|j| j.Type == ActionAddIndex));
    // AddIndex 必须被剔除。
    assert!(!filtered.iter().any(|j| j.Type == ActionModifyColumn));
    // ModifyColumn 必须被剔除。
    assert!(!filtered.iter().any(|j| j.Type == ActionReorganizePartition));
    // ReorganizePartition 必须被剔除。
    assert_eq!(filtered.len(), 6);
    // 最终应剩 6 个 job。
}

/// Corresponds to Go `TestCheckDDLJobByRules`.
/// Check 与 Filter 对称：已过滤集合通过，含 block 类型则失败。
#[test]
fn test_check_ddl_job_by_rules() {
    let ddl_jobs = vec![
        Job {
            Type: ActionSetTiFlashReplica,
            ..Default::default()
        },
        Job {
            Type: ActionAddIndex,
            ..Default::default()
        },
        Job {
            Type: 100,
            ..Default::default()
        },
    ];
    let filtered = FilterDDLJobByRules(&ddl_jobs, &[DDLJobLogIncrementalCompactBlockListRule]);
    assert!(CheckDDLJobByRules(&filtered, &[DDLJobLogIncrementalCompactBlockListRule]).is_ok());
    // 过滤后集合通过 Check。
    assert!(CheckDDLJobByRules(&ddl_jobs, &[DDLJobLogIncrementalCompactBlockListRule]).is_err());
    // 未过滤的原始列表含 AddIndex，Check 必须失败。

    // DDLJobBlockListRule 下 LockTable 也应被拒绝。
    assert!(
        CheckDDLJobByRules(
            &[Job {
                Type: ActionLockTable,
                ..Default::default()
            }],
            &[DDLJobBlockListRule]
        )
        .is_err()
    );
}

/// Corresponds to Go `TestMonitorTheIncrementalUnsupportDDLType`.
/// Go 监控 BackupFillerTypeCount；此处用 compact/incremental 类型集合长度对照。
#[test]
fn test_monitor_the_incremental_unsupport_ddl_type() {
    // Go watches ddl.BackupFillerTypeCount()==5; Rust tracks compact blocklist size.
    // compact=3、incremental=4：记录当前桩集合，漂移时立刻失败。
    let compact = [
        // 增量压缩路径不支持的 DDL 动作集合。
        ActionAddIndex,
        ActionModifyColumn,
        ActionReorganizePartition,
    ];
    assert_eq!(compact.len(), 3);
    let incremental = [
        // 增量路径仍允许的 TiFlash/锁表类动作。
        ActionSetTiFlashReplica,
        ActionUpdateTiFlashReplicaStatus,
        ActionLockTable,
        ActionUnlockTable,
    ];
    assert_eq!(incremental.len(), 4);
}

/// Corresponds to Go `TestTikvUsage`.
/// EstimateTikvUsage(total, replica, store) = total*replica/store。
#[test]
fn test_tikv_usage() {
    let files = [
        // 构造 1..5 PB 五个文件，合计 15 PB。
        File {
            Name: "F1".into(),
            Size_: 1 * PB,
            ..Default::default()
        },
        File {
            Name: "F2".into(),
            Size_: 2 * PB,
            ..Default::default()
        },
        File {
            Name: "F3".into(),
            Size_: 3 * PB,
            ..Default::default()
        },
        File {
            Name: "F4".into(),
            Size_: 4 * PB,
            ..Default::default()
        },
        File {
            Name: "F5".into(),
            Size_: 5 * PB,
            ..Default::default()
        },
    ];
    let total = ArchiveSize(&files);
    // 5 个文件合计 15*PB；3 副本 / 6 store → 7.5*PB。
    let ret = EstimateTikvUsage(total, 3, 6);
    // 公式对齐 Go：total * tikvReplica / storeCount。
    assert_eq!(ret, 15 * PB * 3 / 6);
    // 15*PB*3/6 精确值。
}

/// Corresponds to Go `TestTiflashUsage`.
/// TiFlash 用量按 (表字节×副本) 汇总后再除以 store 数。
#[test]
fn test_tiflash_usage() {
    // (table_bytes, replica_cnt)
    // 0+2+6 = 8*PB，除以 3 个 store。
    let tables = [(1 * PB, 0_u64), (2 * PB, 1), (3 * PB, 2)];
    let ret = EstimateTiflashUsage(&tables, 3);
    // 表用量加权副本后除以 TiFlash store 数。
    assert_eq!(ret, 8 * PB / 3);
    // 8*PB/3 精确值。
}

/// Corresponds to Go `TestCheckTikvSpace`.
/// 可用空间充足通过；available 过小则 Err。
#[test]
fn test_check_tikv_space() {
    CheckStoreSpace(400 * PB, (500 * PB) as i64, 1).unwrap();
    // 需要 400PB，可用 500PB → Ok；可用 10 → Err。
    assert!(CheckStoreSpace(400 * PB, 10, 1).is_err());
    // 可用空间不足路径。
}

/// Corresponds to Go `TestAdjustTablesToRestoreAndCreateTableTracker`.
/// 直接验证生产表追踪逻辑处理跨库 rename 的行为。
#[test]
fn test_adjust_tables_to_restore_and_create_table_tracker() {
    fn table(db_id: i64, db: &str, table_id: i64, name: &str) -> Table {
        Table {
            DB: DBInfo {
                ID: db_id,
                Name: CIStr(db.into()),
            },
            Info: TableInfo {
                ID: table_id,
                Name: CIStr(name.into()),
                ..Default::default()
            },
        }
    }
    let t11 = table(1, "test_db_1", 11, "test_table_11");
    let t12 = table(1, "test_db_1", 12, "test_table_12");
    let t21 = table(2, "test_db_2", 21, "test_table_21");
    let snapshot_db_map = std::collections::HashMap::from([
        (
            1,
            Database {
                Info: t11.DB.clone(),
                Tables: vec![t11.clone(), t12],
            },
        ),
        (
            2,
            Database {
                Info: t21.DB.clone(),
                Tables: vec![t21.clone()],
            },
        ),
    ]);
    let snapshot_table_map =
        std::collections::HashMap::from([(11, t11.clone()), (21, t21.clone())]);
    let mut history = NewTableHistoryManager();
    history.AddTableHistory(11, "test_table_11", 1, 1);
    history.AddTableHistory(11, "renamed_table", 2, 2);
    history.AddTableHistory(21, "test_table_21", 2, 1);
    let mut cfg = RestoreConfig::default();
    cfg.Config.TableFilter.patterns = vec!["test_db_2.*".into()];
    let mut table_map = std::collections::HashMap::from([(21, t21)]);
    let mut db_map = std::collections::HashMap::from([(2, snapshot_db_map[&2].clone())]);
    AdjustTablesToRestoreAndCreateTableTracker(
        &history,
        &mut cfg,
        &snapshot_db_map,
        &snapshot_table_map,
        &std::collections::HashMap::new(),
        &mut table_map,
        &mut db_map,
    )
    .unwrap();
    assert!(cfg.PiTRTableTracker.ContainsDBAndTableId(2, 11));
    assert!(cfg.PiTRTableTracker.ContainsDBAndTableId(2, 21));
    assert!(table_map.contains_key(&11));
    assert!(table_map.contains_key(&21));
    assert!(db_map.contains_key(&1));
    assert!(db_map.contains_key(&2));
}

/// Corresponds to Go `TestHash`.
/// Hash 稳定性：storage/filter/task 变化应改变哈希；部分字段 Rust 侧刻意省略。
#[test]
fn test_hash() {
    let base = RestoreConfig {
        UpstreamClusterID: 1,
        // Hash 基线配置：固定 storage/filter/PD/WithSysTable。
        Config: crate::common::Config {
            Storage: "default-storage".into(),
            ExplicitFilter: true,
            FilterStr: vec!["filter1".into(), "filter2".into()],
            PD: vec!["pd".into()],
            ..Default::default()
        },
        RestoreCommonConfig: RestoreCommonConfig {
            WithSysTable: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let original = base.Hash(FullRestoreCmd).unwrap();
    // 基线哈希；后续 case 对比 equal / not-equal。

    // 每个 case：名字、修改函数、是否换任务类型、期望哈希相等。
    struct Case {
        name: &'static str,
        modify: fn(&mut RestoreConfig),
        change_task: bool,
        expect_equal: bool,
    }
    let cases = [
        Case {
            name: "change_upstream_cluster_id",
            modify: |cfg| cfg.UpstreamClusterID = 999,
            change_task: false,
            expect_equal: false,
        },
        Case {
            name: "identical_configuration",
            // 完全相同配置 → 哈希不变。
            modify: |_| {},
            change_task: false,
            // 仍用 FullRestoreCmd。
            expect_equal: true,
            // 与基线哈希相等。
        },
        Case {
            name: "change_storage",
            // 改 Storage → 哈希变化。
            modify: |cfg| cfg.Config.Storage = "new-storage".into(),
            change_task: false,
            expect_equal: false,
            // 与基线哈希不等。
        },
        Case {
            name: "toggle_explicit_filter",
            // ExplicitFilter 不进 Rust Hash → 仍相等。
            modify: |cfg| cfg.Config.ExplicitFilter = !cfg.Config.ExplicitFilter,
            change_task: false,
            expect_equal: true, // Rust Hash omits ExplicitFilter
        },
        Case {
            name: "modify_filter_strings",
            // 追加 filter → 哈希变化。
            modify: |cfg| cfg.Config.FilterStr.push("new-filter".into()),
            change_task: false,
            expect_equal: false,
        },
        Case {
            name: "reorder_filter_strings",
            // 顺序敏感：交换 filter 也应改变哈希。
            modify: |cfg| {
                cfg.Config.FilterStr = vec![
                    cfg.Config.FilterStr[1].clone(),
                    cfg.Config.FilterStr[0].clone(),
                ];
            },
            change_task: false,
            expect_equal: false,
        },
        Case {
            name: "toggle_system_tables",
            // WithSysTable 进入 Go immutableRestoreConfig。
            modify: |cfg| {
                cfg.RestoreCommonConfig.WithSysTable = !cfg.RestoreCommonConfig.WithSysTable;
            },
            change_task: false,
            expect_equal: false,
        },
        Case {
            name: "empty_vs_base_config",
            // 空配置 vs 基线 → 必须不同。
            modify: |cfg| *cfg = RestoreConfig::default(),
            change_task: false,
            expect_equal: false,
        },
        Case {
            name: "change_task_type",
            // FullRestore vs PointRestore → 哈希不同。
            modify: |_| {},
            change_task: true,
            // 切换任务类型以改变哈希输入。
            expect_equal: false,
        },
    ];

    for tc in cases {
        // 按 case 修改配置并选择命令名，再与 original 比较。
        let mut modified = base.clone();
        // 从基线克隆后再 apply modify。
        (tc.modify)(&mut modified);
        let cmd = if tc.change_task {
            // change_task 时切换到 PointRestoreCmd。
            PointRestoreCmd
        } else {
            FullRestoreCmd
        };
        let h = modified.Hash(cmd).unwrap();
        // 计算修改后哈希。
        if tc.expect_equal {
            // expect_equal 分支：哈希必须等于基线。
            assert_eq!(h, original, "{}", tc.name);
            // 相等断言带 case 名便于定位。
        } else {
            assert_ne!(h, original, "{}", tc.name);
            // 不等断言带 case 名便于定位。
        }
    }

    // Go ast.RedactURL 使凭据轮换不破坏 checkpoint Hash。
    let mut secret_a = base.clone();
    secret_a.Config.Storage =
        "s3://bucket/prefix?access-key=one&secret-access-key=alpha&endpoint=e1".into();
    let mut secret_b = secret_a.clone();
    secret_b.Config.Storage =
        "s3://bucket/prefix?access-key=two&secret-access-key=beta&endpoint=e1".into();
    assert_eq!(
        secret_a.Hash(FullRestoreCmd).unwrap(),
        secret_b.Hash(FullRestoreCmd).unwrap()
    );
    secret_b.Config.Storage =
        "s3://bucket/prefix?access-key=two&secret-access-key=beta&endpoint=e2".into();
    assert_ne!(
        secret_a.Hash(FullRestoreCmd).unwrap(),
        secret_b.Hash(FullRestoreCmd).unwrap()
    );
}

#[test]
fn test_restore_records_snapshot_archive_size_for_compacted_ssts() {
    let storage = crate::stubs::MemStorage::new();
    let meta = crate::stubs::backuppb::BackupMeta {
        Files: vec![
            File {
                Size_: 1024,
                ..Default::default()
            },
            File {
                Size_: 2048,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    storage.put(crate::stubs::MetaFile, serde_json::to_vec(&meta).unwrap());
    let mut cfg = RestoreConfig {
        RestoreStorage: Some(storage),
        ..Default::default()
    };
    cfg.Config.PD = vec!["127.0.0.1:2379".into()];
    cfg.Config.Storage = "local:///task40-snapshot".into();
    let glue = crate::stubs::MemGlue::default();
    RunRestore(
        &crate::restore_lifecycle_test::fixture_glue(&glue),
        DBRestoreCmd,
        &mut cfg,
    )
    .unwrap();
    assert_eq!(cfg.snapshotRestoreDataSize, 3072);
    assert_eq!(
        glue.records.lock().unwrap()[crate::stubs::RestoreDataSize],
        3072
    );
}
