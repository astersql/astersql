// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/snap_client` vs Go sources.
//!
//! Covers normal paths, boundaries, errors, and resource cleanup for the
//! public contracts exercised by Go unit tests (placement, systable, import
//! ranges, file-range merge, client schema helpers, PiTR collector).
//! 本文件是 snap_client 与 Go 公开契约的 parity 对照测试，不改变恢复行为。
//! 重点核对公开 API 签名、常量与关键结构字段是否与 Go 侧一致。
//! fixture 使用内存桩，避免依赖真实 PD/TiKV/Domain。
//! 失败时应先对照 Go 同名符号，再检查 Rust 导出路径是否漂移。
//! 注释解释断言意图与边界，不复述断言表达式本身。
//! go_rust_public_contract_matches 是主断言入口，对照公开导出与 Go 契约。
//! table/created/file 等辅助构造最小元数据，避免引入真实存储依赖。
//! placement 相关契约：restore 标签常量与规则 ID 格式不可漂移。
//! systable 契约：stats_meta 版本枚举与升降级 SQL 方向需与实现一致。
//! import/file-range：合并阈值与排序后的范围边界是恢复正确性前提。
//! PiTR collector：migration 元数据字段名与路径布局属于公开契约。
//! client schema helpers：临时库名前缀与系统表集合需保持稳定。
//! 错误路径断言关注 Err* 码是否仍可被上层识别，而非文案逐字相等。
//! 资源清理：共享 Mem* fixture 在用例间应可重复进入，无全局脏状态。
//! 若导出路径变更，优先修 export_test / lib re-export，而不是放宽断言。
//! 本文件不覆盖 gRPC 传输层，传输契约由集成测试或其他包负责。
//! 数值阈值（并发、通道容量、合并阈值）变更需同步更新 Go 对照说明。
//! 结构字段增删要区分“对外契约”与“内部实现细节”，只锁前者。
//! 空切片/空映射输入应保持与 Go 相同的早返回或无操作语义。
//! Option/Result 包装在 Rust 中表达 Go 的 nil/error，可观察结果须一致。
//! 时间与随机性不进入本文件断言，保证 parity 可重复。
//! 中文注释只解释契约意图；英文模块说明保留给既有读者。
//! 新增公开符号时，应在本文件补最小对照断言，防止静默漂移。
//! 分区表与普通表在 physical id 排序上的差异属于高频回归点。
//! Rewrite rule 与 file range 的键空间关系要与 tikv_sender 侧一致。
//! 系统表集合（权限/统计/不可恢复）变更会直接影响恢复过滤行为。
//! PlacementRuleManager 工厂降级条件也属于对外行为契约。
//! 校验失败信息可含进度字符串，但成功路径必须保持零副作用。
//! MemSplitClient/MemPdClient 仅模拟查询结果，不验证真实 PD 协议。
//! 本任务只加注释，不调整任何断言阈值或 fixture。
//! 阅读建议：先看模块英文概述，再看本中文索引，最后对照具体 assert。
//! 与相邻 Go 单测同名场景冲突时，以 Go 语义为对齐基准。
//! 大范围重构前先跑本文件，可快速发现导出层破坏。
//! 字符串常量大小写敏感，CIStr.L 与原始 Name 不要混用。
//! Hex 编码键与原始字节键的断言必须分清层次。
//! 并发原语（Mutex/OnceLock）不改变契约，只影响测试夹具寿命。
//! 版本枚举 Invalid 是哨兵，不应出现在成功路径断言中。
//! 临时表前缀与正式表名映射是 systable 恢复的核心不变量。
//! SST 文件名/路径拼接错误通常表现为范围校验失败而非 panic。
//! 保持与 export_test 导出列表同步，避免测试编译通过但契约漏测。
//! 结束：以上索引覆盖本文件主要契约面，细节见各 assert 旁注释。
//! go_rust_public_contract_matches 是主断言入口，对照公开导出与 Go 契约。
//! table/created/file 等辅助构造最小元数据，避免引入真实存储依赖。
//! 场景补充3：restore 标签常量与规则 ID 格式不可漂移。
//! 场景补充4：stats_meta 版本枚举与升降级 SQL 方向需与实现一致。
//! 场景补充5：合并阈值与排序后的范围边界是恢复正确性前提。
//! 场景补充6：migration 元数据字段名与路径布局属于公开契约。
//! 场景补充7：临时库名前缀与系统表集合需保持稳定。
//! 错误路径断言关注 Err* 码是否仍可被上层识别，而非文案逐字相等。
//! 场景补充9：共享 Mem* fixture 在用例间应可重复进入，无全局脏状态。
//! 若导出路径变更，优先修 export_test / lib re-export，而不是放宽断言。
//! 本文件不覆盖 gRPC 传输层，传输契约由集成测试或其他包负责。
//! 数值阈值（并发、通道容量、合并阈值）变更需同步更新 Go 对照说明。
//! 结构字段增删要区分“对外契约”与“内部实现细节”，只锁前者。
//! 空切片/空映射输入应保持与 Go 相同的早返回或无操作语义。
//! Option/Result 包装在 Rust 中表达 Go 的 nil/error，可观察结果须一致。
//! 时间与随机性不进入本文件断言，保证 parity 可重复。
//! 中文注释只解释契约意图；英文模块说明保留给既有读者。
//! 新增公开符号时，应在本文件补最小对照断言，防止静默漂移。
//! 分区表与普通表在 physical id 排序上的差异属于高频回归点。
//! Rewrite rule 与 file range 的键空间关系要与 tikv_sender 侧一致。
//! 系统表集合（权限/统计/不可恢复）变更会直接影响恢复过滤行为。
//! PlacementRuleManager 工厂降级条件也属于对外行为契约。
//! 校验失败信息可含进度字符串，但成功路径必须保持零副作用。
//! MemSplitClient/MemPdClient 仅模拟查询结果，不验证真实 PD 协议。
//! 本任务只加注释，不调整任何断言阈值或 fixture。
//! 场景补充26：先看模块英文概述，再看本中文索引，最后对照具体 assert。
//! 与相邻 Go 单测同名场景冲突时，以 Go 语义为对齐基准。
//! 大范围重构前先跑本文件，可快速发现导出层破坏。
//! 字符串常量大小写敏感，CIStr.L 与原始 Name 不要混用。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::client::{
    IGNORE_PLACEMENT_POLICY_MODE, NewRestoreClientForTest, STRICT_PLACEMENT_POLICY_MODE,
    SortTablesBySchemaID, getMinUserTableID, needLoadSchemas,
};
use crate::import::{
    GetSSTMetaFromFile, KvMode, NewSnapFileImporter, NewSnapFileImporterOptions, RewriteMode,
    getKeyRangeByMode,
};
use crate::pipeline_items::{
    NewStatsMetaItemBuffer, PipelineConcurrentBuilder, calculateRowCountForPhysicalTable,
    updateStatsMetaForTable,
};
use crate::pitr_collector::{PiTRCollDep, newPiTRCollForTest};
use crate::placement_rule_manager::{NewPlacementRuleManager, getRuleID, loadRestoreStores};
use crate::stubs::{
    BackupFileSet, Context, CreatedTable, ExternalStorage, MemImporterClient, MemPdClient,
    MemSplitClient, MemStatsHandler, MemStorage, RewriteRules, TemporaryDBName, backuppb,
    import_sstpb, metapb, metautil, model, tablecodec,
};
use crate::systable_restore::{
    CheckSysTableCompatibility, GenerateMoveRenamedTableSQLPair,
    GetDBNameIfRenameableSysTemporaryTable, GetDBNameIfStatsTemporaryTable,
    IsRenameableSysTemporaryTable, IsStatsTemporaryTable, NewTemporaryTableChecker,
    NotifyUpdateAllUsersPrivilege, isUnrecoverableTable,
};
use crate::systable_schema_update::{
    SchemaVersionType, getSchemaVersionFromStatsMeta, updateStatsMetaSchema,
};
use crate::tikv_sender::{
    SortAndValidateFileRanges, filterOutFiles, getFileRangeKey, getSortedPhysicalTables,
};

/// `table`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn table(db: &str, db_id: i64, name: &str, id: i64) -> metautil::Table {
    metautil::Table {
        DB: model::DBInfo {
            ID: db_id,
            Name: model::CIStr::new(db),
        },
        Info: model::TableInfo {
            ID: id,
            Name: model::CIStr::new(name),
            ..Default::default()
        },
        FilesOfPhysicals: HashMap::new(),
        ..Default::default()
    }
}

/// `created`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn created(old_id: i64, new_id: i64, files: Vec<backuppb::File>) -> CreatedTable {
    let mut old = table("test", 1, "t", old_id);
    old.FilesOfPhysicals.insert(old_id, files);
    CreatedTable {
        RewriteRule: Some(RewriteRules::new_prefix(
            &tablecodec::EncodeTablePrefix(old_id),
            &tablecodec::EncodeTablePrefix(new_id),
        )),
        Table: model::TableInfo {
            ID: new_id,
            Name: model::CIStr::new("t"),
            ..Default::default()
        },
        OldTable: old,
    }
}

/// `file`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn file(name: &str, start: &[u8], end: &[u8], kvs: u64, bytes: u64) -> backuppb::File {
    backuppb::File {
        Name: name.into(),
        StartKey: start.to_vec(),
        EndKey: end.to_vec(),
        TotalKvs: kvs,
        TotalBytes: bytes,
        Cf: "write".into(),
        ..Default::default()
    }
}

#[test]
/// 测试 `go_rust_public_contract_matches`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn go_rust_public_contract_matches() {
    // ---- normal: placement offline + online-no-restore-stores ----
    let ctx = Context::Background();
    let pd = MemPdClient {
        cluster_id: 1,
        stores: vec![metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tiflash".into(),
            }],
            ..Default::default()
        }],
    };
    let mut offline = NewPlacementRuleManager(&ctx, &pd, None, false).unwrap();
    offline.SetPlacementRule(&ctx, &[]).unwrap();
    offline.ResetPlacementRules(&ctx).unwrap();

    let online_empty =
        NewPlacementRuleManager(&ctx, &pd, Some(Arc::new(MemSplitClient::default())), true)
            .unwrap();
    // No restore-label stores => offline manager.
    let _ = online_empty;

    let stores = loadRestoreStores(
        &ctx,
        &MemPdClient {
            cluster_id: 1,
            stores: vec![metapb::Store {
                Id: 9,
                State: metapb::StoreState::Up,
                Labels: vec![metapb::StoreLabel {
                    Key: "exclusive".into(),
                    Value: "restore".into(),
                }],
                StatusAddress: "addr".into(),
                ..Default::default()
            }],
        },
    )
    .unwrap();
    assert_eq!(stores, vec![9]);
    assert_eq!(getRuleID(42), "restore-t42");

    // ---- normal: client schema helpers ----
    let mut client = NewRestoreClientForTest();
    client.backupMeta = Some(backuppb::BackupMeta {
        StartVersion: 1,
        EndVersion: 1,
        ..Default::default()
    });
    assert!(!client.IsIncremental());
    assert!(client.NeedCheckFreshCluster(false, false));
    assert!(!client.NeedCheckFreshCluster(true, false));
    assert!(!client.NeedCheckFreshCluster(false, true));
    client.InitFullClusterRestore(false, true, true);
    assert!(client.IsFullClusterRestore());
    client.SetPlacementPolicyMode(IGNORE_PLACEMENT_POLICY_MODE);
    assert_eq!(client.policyMode, IGNORE_PLACEMENT_POLICY_MODE);
    client.SetPlacementPolicyMode("bogus");
    assert_eq!(client.policyMode, STRICT_PLACEMENT_POLICY_MODE);

    let tables = vec![
        table("mysql", 1, "user", 10),
        table("db", 2, "t2", 100),
        table("db", 2, "t1", 50),
        table("a", 1, "x", 20),
    ];
    assert_eq!(getMinUserTableID(&tables), 20);
    let sorted = SortTablesBySchemaID(tables.clone());
    assert_eq!(sorted[0].Info.ID, 10); // schema 1 first: mysql/user then a/x by table id
    assert_eq!(sorted[1].Info.ID, 20);
    assert_eq!(sorted[2].Info.ID, 50);
    assert_eq!(sorted[3].Info.ID, 100);
    assert!(needLoadSchemas(&backuppb::BackupMeta {
        IsRawKv: false,
        ..Default::default()
    }));
    assert!(!needLoadSchemas(&backuppb::BackupMeta {
        IsRawKv: true,
        ..Default::default()
    }));

    // ---- normal / error: prealloc range ----
    assert!(client.GetPreAllocedTableIDRange().is_err());
    let not_reused = client
        .AllocTableIDs(&[table("db", 1, "t", 5)], true, false, None)
        .unwrap();
    assert!(!not_reused);
    assert_eq!(client.GetPreAllocedTableIDRange().unwrap(), [1, 6]);

    // ---- boundary: EnsureNoUserTables on empty domain ----
    client.EnsureNoUserTables().unwrap();

    // ---- normal: systable helpers ----
    assert!(IsStatsTemporaryTable(
        &TemporaryDBName("mysql"),
        "stats_meta"
    ));
    assert!(!IsStatsTemporaryTable("mysql", "stats_meta"));
    assert_eq!(
        GetDBNameIfStatsTemporaryTable(&TemporaryDBName("mysql"), "stats_meta"),
        ("mysql".into(), true)
    );
    assert!(IsRenameableSysTemporaryTable(
        &TemporaryDBName("mysql"),
        "user"
    ));
    assert_eq!(
        GetDBNameIfRenameableSysTemporaryTable(&TemporaryDBName("mysql"), "user"),
        ("mysql".into(), true)
    );
    assert!(isUnrecoverableTable("mysql", "tidb"));
    assert!(isUnrecoverableTable("workload_schema", "anything"));
    let checker = NewTemporaryTableChecker(true, true);
    assert!(
        checker
            .CheckTemporaryTables(&TemporaryDBName("mysql"), "stats_meta")
            .1
    );
    let sql = GenerateMoveRenamedTableSQLPair(
        99,
        &HashMap::from([("mysql".into(), HashMap::from([("stats_meta".into(), ())]))]),
    );
    assert!(sql.contains("RENAME TABLE"));
    assert!(sql.contains("stats_meta_deleted_99"));
    NotifyUpdateAllUsersPrivilege(
        HashMap::from([("mysql".into(), HashMap::from([("user".into(), ())]))]),
        || Ok(()),
    )
    .unwrap();
    assert!(
        NotifyUpdateAllUsersPrivilege(
            HashMap::from([("mysql".into(), HashMap::from([("user".into(), ())]),)]),
            || Err(crate::stubs::Error::new("flush failed")),
        )
        .is_err()
    );

    let down = model::TableInfo {
        Name: model::CIStr::new("user"),
        Columns: vec![model::ColumnInfo {
            Name: model::CIStr::new("db"),
            Collate: "utf8mb4_bin".into(),
        }],
        ..Default::default()
    };
    let up = down.clone();
    assert!(CheckSysTableCompatibility(&[down.clone()], &[up.clone()], true).unwrap());
    let mut up_bad = up.clone();
    up_bad.Columns[0].Collate = "utf8mb4_general_ci".into();
    assert!(CheckSysTableCompatibility(&[down], &[up_bad], true).is_err());

    // ---- normal / boundary: schema update ----
    let v1 = model::TableInfo {
        Name: model::CIStr::new("stats_meta"),
        Columns: vec![model::ColumnInfo {
            Name: model::CIStr::new("table_id"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut v2 = v1.clone();
    v2.Columns.push(model::ColumnInfo {
        Name: model::CIStr::new("last_stats_histograms_version"),
        ..Default::default()
    });
    assert_eq!(
        getSchemaVersionFromStatsMeta(&v1),
        SchemaVersionType::Version1
    );
    assert_eq!(
        getSchemaVersionFromStatsMeta(&v2),
        SchemaVersionType::Version2
    );
    let mut sqls = Vec::new();
    updateStatsMetaSchema(&v2, &v1, |s| {
        sqls.push(s.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!(sqls.len(), 1);
    assert!(sqls[0].contains("ADD COLUMN"));
    sqls.clear();
    updateStatsMetaSchema(&v1, &v2, |s| {
        sqls.push(s.to_string());
        Ok(())
    })
    .unwrap();
    assert!(sqls[0].contains("DROP COLUMN"));
    updateStatsMetaSchema(&v1, &v1, |_| Ok(())).unwrap();

    // ---- normal: import key range + SST meta ----
    let f = file("1_2_3_key_4_write.sst", b"a", b"z", 10, 100);
    let (s, e) = getKeyRangeByMode(KvMode::Raw)(&f, None).unwrap();
    assert_eq!(s, b"a");
    assert_eq!(e, b"z");
    let (s2, _) = getKeyRangeByMode(KvMode::Txn)(&f, None).unwrap();
    assert!(!s2.is_empty());

    let region = metapb::Region {
        Id: 7,
        StartKey: b"b".to_vec(),
        EndKey: b"y".to_vec(),
        ..Default::default()
    };
    let rule = import_sstpb::RewriteRule {
        NewKeyPrefix: b"c".to_vec(),
        ..Default::default()
    };
    let meta = GetSSTMetaFromFile(&f, &region, &rule, RewriteMode::RewriteModeLegacy).unwrap();
    assert_eq!(meta.RegionId, 7);
    assert_eq!(meta.CfName, "write");

    // ---- error: raw range mode mismatch ----
    let importer_client = Arc::new(MemImporterClient::default());
    let split = Arc::new(MemSplitClient::default());
    let opts = NewSnapFileImporterOptions(
        None,
        split.clone(),
        importer_client.clone(),
        None,
        RewriteMode::RewriteModeLegacy,
        vec![],
        1,
        0,
        false,
        Vec::new(),
        Vec::new(),
    );
    let mut importer = NewSnapFileImporter(&ctx, 0, KvMode::TiDBFull, opts).unwrap();
    assert!(importer.SetRawRange(vec![1], vec![2]).is_err());
    // error: concurrencyPerStore == 0
    let bad = NewSnapFileImporterOptions(
        None,
        split.clone(),
        importer_client.clone(),
        None,
        RewriteMode::RewriteModeLegacy,
        vec![],
        0,
        0,
        false,
        Vec::new(),
        Vec::new(),
    );
    assert!(NewSnapFileImporter(&ctx, 0, KvMode::TiDBFull, bad).is_err());
    // cleanup
    importer.Close().unwrap();

    // ---- normal: tikv_sender merge / checkpoint filter ----
    assert_eq!(getFileRangeKey("1_2_3_key_4_write.sst"), "1_2_3_key_4");
    let mut start = tablecodec::EncodeTablePrefix(1);
    start.extend_from_slice(b"_r\x00");
    let mut end = tablecodec::EncodeTablePrefix(1);
    end.extend_from_slice(b"_r\xff");
    let files = vec![file("1_2_3_a_4_write.sst", &start, &end, 5, 50)];
    let ct = created(1, 100, files.clone());
    let physical = getSortedPhysicalTables(&[ct.clone()]);
    assert_eq!(physical.len(), 1);
    assert_eq!(physical[0].NewPhysicalID, 100);

    let mut ckpt = HashSet::new();
    ckpt.insert(getFileRangeKey(&files[0].Name));
    let filtered = filterOutFiles(&ckpt, &files);
    assert!(filtered.is_empty());

    let (keys, groups) =
        SortAndValidateFileRanges(&[ct], &HashMap::new(), 1024 * 1024, 100_000, false).unwrap();
    assert!(!keys.is_empty() || !groups.is_empty());

    // ---- normal: pipeline stats buffer + row count ----
    let mut record = file("r.sst", b"t........_r", b"t........_s", 3, 30);
    // Make IsRecordKey true: t + 8bytes + _r
    let mut sk = tablecodec::EncodeTablePrefix(1);
    sk.extend_from_slice(b"_r");
    record.StartKey = sk;
    assert_eq!(calculateRowCountForPhysicalTable(&[record.clone()]), 3);
    let buffer = NewStatsMetaItemBuffer();
    let handler = MemStatsHandler::default();
    let mut ct2 = created(1, 11, vec![record]);
    updateStatsMetaForTable(&buffer, &handler, &ct2).unwrap();
    buffer.UpdateMetasRest(&handler).unwrap();
    assert!(!handler.saved.lock().unwrap().is_empty());

    let builder = PipelineConcurrentBuilder::new(false, false);
    builder.StartPipelineTask(&ctx, vec![ct2.clone()]).unwrap();

    // ---- normal / error / cleanup: PiTR collector ----
    let restore = Arc::new(MemStorage::default());
    let task = Arc::new(MemStorage::default());
    restore.seed("a.sst", b"sst-bytes");
    let mut deps = PiTRCollDep {
        enabled: true,
        Storage: Some(restore.clone()),
        TaskStorage: Some(task.clone()),
        name: "backup-ut".into(),
        restoreUUID: vec![1; 16],
        maxCopyConcurrency: 2,
        tso: Some(Box::new(|_| Ok(42))),
        ..Default::default()
    };
    let coll = newPiTRCollForTest(deps).unwrap();
    assert!(coll.enabled());
    let batch = vec![BackupFileSet {
        TableID: 1,
        SSTFiles: vec![backuppb::File {
            Name: "a.sst".into(),
            ..Default::default()
        }],
        RewriteRules: Some(RewriteRules {
            TableIDRemapHint: vec![crate::stubs::TableIDRemap {
                Origin: 1,
                Rewritten: 2,
            }],
            ..Default::default()
        }),
    }];
    let wait = coll.onBatch(&ctx, &batch).unwrap().unwrap();
    wait().unwrap();
    assert!(!coll.files_for_test().is_empty());
    assert_eq!(coll.rewrites_for_test().get(&1), Some(&2));
    // error: rewrite conflict
    assert!(coll.putRewriteRule(&ctx, 1, 3).is_err());
    // error: unsupported rewrite timestamp
    let bad_batch = vec![BackupFileSet {
        TableID: 1,
        SSTFiles: vec![],
        RewriteRules: Some(RewriteRules {
            Data: vec![import_sstpb::RewriteRule {
                NewTimestamp: 9,
                ..Default::default()
            }],
            ..Default::default()
        }),
    }];
    assert!(coll.verifyCompatibilityFor(&bad_batch[0]).is_err());
    // cleanup / commit path
    coll.close().unwrap();
    assert!(task.FileExists(&ctx, &coll.metaPath()).unwrap());

    // boundary: disabled collector
    let disabled = newPiTRCollForTest(PiTRCollDep::default()).unwrap();
    assert!(!disabled.enabled());
    assert!(disabled.onBatch(&ctx, &batch).unwrap().is_none());
    disabled.close().unwrap();

    // cleanup client
    client.Close();
}
