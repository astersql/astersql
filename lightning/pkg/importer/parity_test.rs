// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/importer` public contracts vs Go.
//!
//! 这个文件不像传统单元测试那样围绕单个函数展开，
//! 而是把 importer 对外暴露的一组历史契约按主题打包验证。
//! 主题划分与前面几个子包保持一致：
//! `contract_normal` 锁住主路径常量、状态机和对象装配，
//! `contract_boundary` 锁住跳过分支与默认值，
//! `contract_error` 锁住错误时机和报错文本，
//! `contract_resource_cleanup` 锁住 pause/resume/close 等生命周期语义。
//! 这样做的目的，是在 Rust slim port 持续演进时，
//! 第一时间识别“外部行为变了”而不是“内部实现细节变了”。

use crate::duplicate::Handler;
use crate::*;
use astersql_lightning_pkg_precheck as precheck;
use std::collections::HashSet;
use std::sync::Arc;

#[test]
// 语义说明：`go_rust_public_contract_matches` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn go_rust_public_contract_matches() {
    // 顶层入口只负责串起四类契约，失败后能直接按分组回溯。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

// `contract_normal` 锁住 importer 在主路径下最常被外部依赖的公开契约。
// 语义说明：`contract_normal` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn contract_normal() {
    // Constants match Go check_info.go / import.go.
    // 这些常量和状态字符串一旦偏移，很多调用点和测试都会同步失效。
    assert_eq!(DEFAULT_CSV_SIZE, 10 * 1024 * 1024 * 1024);
    assert_eq!(MAX_SAMPLE_DATA_SIZE, 10 * 1024 * 1024);
    assert_eq!(WARN_EMPTY_REGION_CNT_PER_STORE, 500);
    assert_eq!(ERROR_EMPTY_REGION_CNT_PER_STORE, 1000);
    assert_eq!(TaskMetaTableName, "task_meta_v2");
    assert_eq!(TableMetaTableName, "table_meta");
    assert_eq!(metaStatusInitial.String(), "initialized");
    assert_eq!(parseMetaStatus("finished").unwrap(), metaStatusFinished);
    assert_eq!(taskMetaStatusInitial.String(), "initialized");
    assert_eq!(
        parseTaskMetaStatus("schedule_set").unwrap(),
        taskMetaStatusScheduleSet
    );

    // SimpleTemplate Collect / Success / FailedCount / FailedMsg / Output.
    // 这组断言锁住错误汇总模板的最小展示契约：
    // 是否记录失败、按级别统计、以及输出是否包含失败线索。
    let mut tmpl = NewSimpleTemplate();
    tmpl.Collect(precheck::Critical, true, "ok critical".into());
    tmpl.Collect(precheck::Warn, false, "warn fail".into());
    tmpl.Collect(precheck::Critical, false, "crit fail".into());
    assert!(!tmpl.Success());
    assert_eq!(tmpl.FailedCount(precheck::Warn), 1);
    assert_eq!(tmpl.FailedCount(precheck::Critical), 1);
    assert_eq!(tmpl.FailedMsg(), "crit fail");
    let out = tmpl.Output();
    assert!(out.contains("Check Item") || out.contains("CHECK ITEM") || out.contains("crit fail"));
    assert!(out.contains("\x1b[31m") || out.contains("crit fail"));

    // adjustIDBase / filterColumns / createColumnPermutation / decodeIndexID / simplifyTable.
    // 下半段转向更细粒度的纯函数和轻对象装配，
    // 目的是覆盖 importer 在真正导入前要做的列过滤、键解析和简化表结构步骤。
    assert_eq!(adjustIDBase(10), 10);
    assert_eq!(adjustIDBase(u64::MAX), i64::MAX);

    let table = model::TableInfo {
        Name: model::CIStr::new("t"),
        Columns: vec![
            model::ColumnInfo {
                Name: model::CIStr::new("a"),
                Offset: 0,
                ..Default::default()
            },
            model::ColumnInfo {
                Name: model::CIStr::new("b"),
                Offset: 1,
                Hidden: false,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let (cols, vals) = filterColumns(
        &["a".into(), "b".into()],
        mydump::ExtendColumnData {
            Columns: vec!["ext".into()],
            Values: vec!["v".into()],
        },
        &HashSet::from(["b".into()]),
        &table,
    );
    assert_eq!(cols, vec!["a".to_string(), "ext".to_string()]);
    // 扩展列值应当继续保留在输出值列表中。
    assert_eq!(vals.len(), 1);

    let perm = createColumnPermutation(&[], &HashSet::new(), &table, &log::Logger::L()).unwrap();
    // 空 header 走“按表列顺序直出”路径，并为隐式 rowid 额外补一个占位。
    assert_eq!(perm.len(), 3); // 2 cols + auto row id
    assert_eq!(perm[0], 0);
    assert_eq!(perm[1], 1);
    assert_eq!(perm[2], -1);

    // record key -> conflictOnHandle
    // 冲突键类型识别是重复键处理逻辑的最小前提。
    let mut record = vec![0u8; 19];
    record[0] = b't';
    record[10] = b'r';
    assert_eq!(decodeIndexID(&record).unwrap(), CONFLICT_ON_HANDLE);

    let mut idx = vec![0u8; 19];
    idx[0] = b't';
    idx[10] = b'i';
    idx[11..19].copy_from_slice(&7i64.to_be_bytes());
    assert_eq!(decodeIndexID(&idx).unwrap(), 7);

    // `simplifyTable` 应保留后续冲突处理真正关心的最小列/索引子集。
    let mut tbl = table.clone();
    tbl.Indices.push(model::IndexInfo {
        Primary: true,
        Unique: true,
        Columns: vec![model::IndexColumn {
            Name: model::CIStr::new("a"),
            Offset: 0,
            Length: -1,
        }],
        ..Default::default()
    });
    let (simple, new_perm) = simplifyTable(&tbl, &[0, 1, -1]);
    assert_eq!(simple.Indices.len(), 1);
    assert!(!simple.Columns.is_empty());
    assert!(!new_perm.is_empty());

    // errorOnDup / replaceOnDup handlers.
    // 这里不复现全量冲突处理流程，只验证 handler 在 begin/append/end 上的契约。
    let mut h = errorOnDup::default();
    h.Begin(&idx).unwrap();
    h.Append(b"k1").unwrap();
    h.Append(b"k2").unwrap();
    h.Append(b"k3").unwrap(); // ignored beyond 2
    assert_eq!(h.keyIDs.len(), 2);
    assert!(h.End().is_err());
    assert!(h.Close().is_ok());

    let sorter =
        Arc::new(extsort::OpenDiskSorter("/tmp", &extsort::DiskSorterOptions::default()).unwrap());
    let ctor = makeDupHandlerConstructor(sorter, config::ReplaceOnDup);
    let mut rh = ctor(context::Background()).unwrap();
    rh.Begin(&idx).unwrap();
    rh.Append(b"a").unwrap();
    rh.Append(b"b").unwrap();
    rh.End().unwrap();
    rh.Close().unwrap();

    // meta status SQL cleanup + RemoveTableMetaByTableName.
    // 元数据清理辅助函数要么发出删除 SQL，要么在 allFinished 时删除整组元数据表。
    let db = sql::DB::new_memory();
    RemoveTableMetaByTableName(context::Background(), &db, "meta.table_meta", "db.t").unwrap();
    let log = db.exec_log();
    assert!(log.iter().any(|(q, _)| q.contains("DELETE FROM")));

    MaybeCleanupAllMetas(context::Background(), &db, "lightning_task_info", false).unwrap();
    let before = db.exec_log().len();
    MaybeCleanupAllMetas(context::Background(), &db, "lightning_task_info", true).unwrap();
    assert!(db.exec_log().len() > before);

    // Backend helpers / initGlobalConfig.
    // 本段锁住 backend 判断和全局安全配置传播。
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    assert!(isLocalBackend(&cfg));
    assert!(!isTiDBBackend(&cfg));
    cfg.Security.ClusterSSLCA = "ca".into();
    initGlobalConfig(&cfg.Security);
    assert_eq!(config::GetGlobalConfig().Security.ClusterSSLCA, "ca");

    // NewChecksumManager decision tree.
    // TiDB backend 下返回 `None`，表示不走 TiKV checksum 管理器。
    let mut rc_cfg = config::Config::NewConfig();
    rc_cfg.TikvImporter.Backend = config::BackendTiDB.into();
    let rc = Controller {
        cfg: rc_cfg,
        db: Some(sql::DB::new_memory()),
        pdHTTPCli: Some(pdutil::PdHTTPClient::default()),
        resourceGroupName: "rg".into(),
        taskType: "lightning".into(),
        checkTemplate: NewSimpleTemplate(),
        errorSummaries: makeErrorSummaries(log::Logger::L()),
        metaMgrBuilder: Arc::new(noopMetaMgrBuilder),
        store: storeapi::Storage::new("file:///tmp"),
        pauser: common::NewPauser(),
        engineMgr: backend::EngineManager::default(),
        diskQuotaState: atomic::NewInt32(0),
        compactState: atomic::NewInt32(0),
        saveCpCh: std::sync::Mutex::new(Vec::new()),
        taskCtx: context::Background(),
        dbMetas: vec![],
        dbInfos: Default::default(),
        tableWorkers: None,
        indexWorkers: None,
        regionWorkers: None,
        ioWorkers: None,
        checksumWorks: None,
        backend: None,
        pdCli: pd::Client::default(),
        sysVars: Default::default(),
        tls: None,
        checkpointsDB: None,
        closedEngineLimit: None,
        addIndexLimit: None,
        ownStore: false,
        errorMgr: None,
        taskMgr: None,
        status: None,
        dupIndicator: None,
        preInfoGetter: None,
        precheckItemBuilder: None,
        encBuilder: None,
        tikvModeSwitcher: None,
        keyspaceName: String::new(),
        apiContext: pd::APIContext::default(),
        closed: false,
    };
    assert!(
        NewChecksumManager(context::Background(), &rc, &kv::Storage::default())
            .unwrap()
            .is_none()
    );

    // Precheck builder dispatch.
    // 再补一次接线层冒烟，确认主 importer crate 仍能构造 precheck item。
    let builder = NewPrecheckItemBuilder(
        &config::Config::NewConfig(),
        vec![],
        NewPreImportInfoGetter(
            &config::Config::NewConfig(),
            vec![],
            storeapi::Storage::new("file:///data"),
            NewTargetInfoGetterImpl(&config::Config::NewConfig(), sql::DB::new_memory(), None)
                .unwrap(),
            None,
            None,
            vec![],
        )
        .unwrap(),
        None,
        None,
        None,
    );
    let checker = builder
        .BuildPrecheckItem(precheck::CheckLargeDataFile)
        .unwrap();
    assert_eq!(checker.GetCheckItemID(), precheck::CheckLargeDataFile);

    // estimateCompactionThreshold uses ingestctrl bound.
    // 空文件列表和空 checkpoint 时阈值应回到零。
    let thr = estimateCompactionThreshold(
        &[],
        &astersql_lightning_pkg_checkpoints::TableCheckpoint::default(),
        1,
    );
    assert_eq!(thr, 0);
}

// 语义说明：`contract_boundary` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn contract_boundary() {
    // checkTableEmpty early-return on TiDB backend / parallel import.
    // 这两个分支在 Go 中都会跳过“目标表必须为空”的强校验。
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    let mut rc = minimal_controller(cfg);
    assert!(rc.checkTableEmpty(context::Background()).is_ok());

    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.ParallelImport = true;
    let mut rc = minimal_controller(cfg);
    assert!(rc.checkTableEmpty(context::Background()).is_ok());

    // checkCheckpoints skipped when disabled.
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = false;
    let mut rc = minimal_controller(cfg);
    assert!(rc.checkCheckpoints(context::Background()).is_ok());

    // ObtainNewCollationEnabled treats missing row as false.
    // 缺失行不应被放大为错误，否则会影响老集群兼容路径。
    let db = sql::DB::new_memory();
    let enabled = ObtainNewCollationEnabled(context::Background(), &db).unwrap();
    assert!(!enabled);

    // ObtainImportantVariables fills defaults on query failure.
    // 查询失败后要回落到默认变量集，避免导入前接线因为观测失败而完全中断。
    let vars = ObtainImportantVariables(context::Background(), &db, true);
    assert!(vars.contains_key("tidb_row_format_version"));
    assert!(vars.contains_key("tidb_placement_mode"));

    // parseColumnPermutations unknown column error.
    // 这里锁住“发现未知列立即返回错误”的防御性行为。
    let table = model::TableInfo {
        Name: model::CIStr::new("t"),
        Columns: vec![model::ColumnInfo {
            Name: model::CIStr::new("a"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = parseColumnPermutations(
        &table,
        &["missing".into()],
        &HashSet::new(),
        &log::Logger::L(),
    )
    .unwrap_err();
    assert!(err.Error().contains("unknown columns") || err.class == Some("ErrUnknownColumns"));

    // Large file check with StrictFormat skips.
    // strict-format 代表调用方已接受更严格文件形状，因此这里允许跳过大文件告警。
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.StrictFormat = true;
    let mut item = NewLargeFileCheckItem(&cfg, &[]);
    let res = item
        .Check(precheck::context::Background())
        .unwrap()
        .unwrap();
    assert!(res.Passed);
}

// 语义说明：`contract_error` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn contract_error() {
    // 先覆盖几个轻量纯函数的失败路径，确保未知状态和非法 key 不会被静默吞掉。
    assert!(parseMetaStatus("nope").is_err());
    assert!(parseTaskMetaStatus("nope").is_err());
    assert!(decodeIndexID(b"xx").is_err());

    let builder = NewPrecheckItemBuilder(
        &config::Config::NewConfig(),
        vec![],
        NewPreImportInfoGetter(
            &config::Config::NewConfig(),
            vec![],
            storeapi::Storage::new("file:///data"),
            NewTargetInfoGetterImpl(&config::Config::NewConfig(), sql::DB::new_memory(), None)
                .unwrap(),
            None,
            None,
            vec![],
        )
        .unwrap(),
        None,
        None,
        None,
    );
    assert!(builder.BuildPrecheckItem("NOT_A_REAL_CHECK").is_err());

    // DoChecksum without manager in context.
    // 缺失管理器时必须返回可识别错误，而不是 panic。
    let err = DoChecksum(
        context::Background(),
        &importdef::TableInfo {
            Name: "t".into(),
            DB: "db".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.Error().contains("No gcLifeTimeManager"));

    // NewTableImporter missing name.
    // 表名缺失属于构造期错误，应尽早失败。
    let err = match NewTableImporter(
        &importdef::DBInfo {
            Name: "db".into(),
            Tables: Default::default(),
        },
        &importdef::TableInfo::default(),
        None,
        log::Logger::L(),
    ) {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(err.Error().contains("missing name"));

    // verifyCheckpoint always rejects backend mismatch.
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let err = verifyCheckpoint(
        &cfg,
        &astersql_lightning_pkg_checkpoints::TaskCheckpoint {
            Backend: config::BackendTiDB.into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.Error().contains("tikv-importer.backend"));
}

// 语义说明：`contract_resource_cleanup` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn contract_resource_cleanup() {
    // Go context.WithCancel makes cancellation observable from every clone of
    // the derived context while retaining values inherited from the parent.
    let parent = context::WithValue(
        context::Background(),
        "cancel-marker",
        Arc::new("present".to_string()),
    );
    let (cancel_ctx, cancel) = context::WithCancel(parent);
    let cancel_ctx_clone = cancel_ctx.clone();
    assert!(cancel_ctx.Err().is_none());
    cancel.cancel();
    assert!(cancel_ctx.Err().is_some());
    assert!(cancel_ctx_clone.Err().is_some());
    assert_eq!(
        cancel_ctx
            .Value("cancel-marker")
            .and_then(|value| value.downcast_ref::<String>().cloned()),
        Some("present".to_string())
    );

    // Go errgroup.WithContext cancels its derived context on the first worker
    // error, allowing sibling work to observe the failure promptly.
    let (group, group_ctx) = errgroup::WithContext(context::Background());
    group.Go(|| Err(Error::new("worker failed")));
    assert!(group.Wait().is_err());
    assert!(group_ctx.Err().is_some());

    // The derived context is also canceled when Wait returns successfully.
    let (group, group_ctx) = errgroup::WithContext(context::Background());
    group.Go(|| Ok(()));
    assert!(group.Wait().is_ok());
    assert!(group_ctx.Err().is_some());

    // `Controller` 的 pause/resume/close 组合经常被上层恢复逻辑复用，
    // 因此这里锁住其最基本的生命周期副作用。
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let mut rc = NewImportController(
        context::Background(),
        &cfg,
        ControllerParam {
            DBMetas: vec![],
            Status: None,
            DumpFileStorage: storeapi::Storage::new("file:///tmp/data"),
            OwnExtStorage: true,
            Pauser: None,
            DB: Some(sql::DB::new_memory()),
            CheckpointStorage: None,
            CheckpointName: String::new(),
            DupIndicator: None,
            KeyspaceName: String::new(),
            ResourceGroupName: String::new(),
            TaskType: String::new(),
        },
    )
    .unwrap();
    assert!(rc.Pause(context::Background()).is_ok());
    assert!(rc.pauser.IsPaused());
    assert!(rc.Resume(context::Background()).is_ok());
    assert!(!rc.pauser.IsPaused());
    rc.Close();
    // 关闭后自身和底层 store 都要处于 closed 状态。
    assert!(rc.closed);
    assert!(rc.store.is_closed());
    // idempotent close
    rc.Close();

    // TiDBManager::Close 需要把底层 DB 一并关闭。
    let db = sql::DB::new_memory();
    let timgr = NewTiDBManagerWithDB(db.clone(), 0);
    timgr.Close();
    assert!(db.is_closed());
}

// 语义说明：`minimal_controller` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
fn minimal_controller(cfg: config::Config) -> Controller {
    // 这里复用与其他测试文件相同的最小 Controller 装配，
    // 让 parity 测试能只关注 importer 契约，而不是完整运行时环境。
    Controller {
        cfg,
        db: Some(sql::DB::new_memory()),
        pdHTTPCli: None,
        resourceGroupName: String::new(),
        taskType: String::new(),
        checkTemplate: NewSimpleTemplate(),
        errorSummaries: makeErrorSummaries(log::Logger::L()),
        metaMgrBuilder: Arc::new(noopMetaMgrBuilder),
        store: storeapi::Storage::new("file:///tmp"),
        pauser: common::NewPauser(),
        engineMgr: backend::EngineManager::default(),
        diskQuotaState: atomic::NewInt32(0),
        compactState: atomic::NewInt32(0),
        saveCpCh: std::sync::Mutex::new(Vec::new()),
        taskCtx: context::Background(),
        dbMetas: vec![],
        dbInfos: Default::default(),
        tableWorkers: None,
        indexWorkers: None,
        regionWorkers: None,
        ioWorkers: None,
        checksumWorks: None,
        backend: None,
        pdCli: pd::Client::default(),
        sysVars: Default::default(),
        tls: None,
        checkpointsDB: None,
        closedEngineLimit: None,
        addIndexLimit: None,
        ownStore: false,
        errorMgr: None,
        taskMgr: None,
        status: None,
        dupIndicator: None,
        preInfoGetter: None,
        precheckItemBuilder: None,
        encBuilder: None,
        tikvModeSwitcher: None,
        keyspaceName: String::new(),
        apiContext: pd::APIContext::default(),
        closed: false,
    }
}
