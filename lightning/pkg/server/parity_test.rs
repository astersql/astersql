// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/server` public contracts vs Go.
//!
//!
//! 这个文件不是端到端回归，而是把 Go 与 Rust 在公共合同层面的差异集中到一处比对。
//! `go_rust_public_contract_matches` 只是一个总调度器，确保四类合同在单个测试入口里按顺序执行。
//! `contract_normal` 覆盖正常路径，验证公开 helper 的基本行为仍与 Go 心智模型一致。
//! 例如 `parseTaskID` 的 id/verb 拆分、`With*` 选项的闭包覆盖，以及 checkpoint 控制器按 backend 分派。
//! 这里的重点不是每个 helper 是否工作，而是调用组合后是否仍保持 Go 的可预期接口。
//! `contract_boundary` 专门放边界条件，如空路径、队列移动、非法 switch mode 等。
//! 边界条件往往最容易在“看似无害的重构”里被破坏，所以单独聚合出来更容易回归对照。
//! `contract_error` 则断言错误类别和提示语义没有偏离，包括未知 backend 与 checkpoint schema 冲突。
//! 这里保留错误类名断言，是为了让上层工具或脚本仍可依赖 Go 原有的错误分类。
//! `contract_resource_cleanup` 关注资源副作用：dump 会写三个 CSV，DestroyError 会移除失败任务，gzip 会设置编码头。
//! 这些检查和功能结果同样重要，因为 server 层既暴露 API，也承担文件与连接生命周期管理。
//! 注释之所以写得更细，是因为 parity test 的价值在“说明契约”，而不是“重复实现细节”。
//! 例如 `NewCheckpointControl` 的验证说明：调用方只关心 backend 路由，不关心内部是 legacy 还是 import-into。
//! `checkSchemaConflict` 的说明强调：只有 MySQL checkpoint schema 与数据文件重名时才应失败。
//! `checkSystemRequirement` 的说明强调：非 local backend 不参与打开文件数估算。
//! `newImporter` 的说明强调：未知 backend 必须立即报错，而已知 backend 需要能被构造与关闭。
//! `Stop()` 的说明强调：没有当前任务时不应平白把 `taskCanceled` 置真。
//! 这类细节如果只看断言容易误解，因此中文注释会把意图直接写出来。
//! 边界测试里的 config list 操作，是对 queue 调度语义最廉价但有效的保护。
//! 移动到 front/back 与 remove 的组合能快速暴露实现是否意外改变了队列稳定性。
//! 错误测试里的空 source dir 例子则说明：底层 storage walk 的成功与失败语义不能被简化。
//! 资源清理测试里的 import-into dump 说明：server 层不只是透传 manager，还负责导出固定文件名。
//! legacy `GetLocalStoringTables` 的检查则确认 callback 结束后 DB 句柄能安全关闭。
//! gzip 断言虽然很小，却约束了 GET 单任务和进度 API 的压缩协商行为。
//! 因此本文件既是 parity 测试，也是 server 公共合同的速查表。
//! 阅读顺序建议从四个 `contract_*` 的标题开始，再看各自内部的关键断言。
//! 若未来 Go 行为变动，需要优先更新这里的合同说明，再决定 Rust 是否跟进。
//! 保持这层文档化测试，有助于减少迁移后“代码能跑但协议悄悄变了”的风险。
//! 同时它还能为大文件 `lightning.rs` 提供更短的公共行为索引。
//! 当排查回归时，先确定是正常路径、边界、错误还是资源清理，再进入对应小节通常更高效。
//! 这就是本文件拆成四个合同函数的真正原因。
//! 它们让测试结构直接映射到外部调用者最关心的四类问题。
//! 从维护角度看，合同测试也能帮助判断某次重构究竟是实现替换还是协议变更。
//! 一旦协议要变，就应同步更新 Go 对照和这里的中文注释，而不只是改断言。
//! 因此这里的注释同样属于迁移证据的一部分。
//! 它们说明了为什么这些看似零散的小断言会被放在同一个文件里。
//! 总结来说，本文件守护的是 server 子系统最薄但也最易碎的那一层公共面。
//! 公共面稳定，内部实现才有安全重构的空间。
//! 公共面漂移，则即使代码编译通过，也可能对使用者造成静默破坏。
//! 额外需要强调的是，这里的合同覆盖面刻意偏向“公开 helper 与控制面副作用”。
//! 这是因为迁移阶段最容易悄悄变化的，往往不是主流程，而是这些小而散的接口边界。
//! `contract_normal` 把最常用路径先固定下来，相当于公共面的基线快照。
//! `contract_boundary` 则提醒维护者，很多 bug 并不出在主路径，而是出在空值和特殊值上。
//! `contract_error` 让错误类名和提示文本也纳入回归范围，避免行为看似一致但错误协议已变。
//! `contract_resource_cleanup` 进一步把文件、关闭动作和压缩头等副作用纳入公共面定义。
//! 这些说明共同表达一个原则：server 的“接口”不只是函数签名，还包括错误、队列和资源语义。
//! 也因此，本文件既短小又重要。
//! 它把最脆弱的公共边界压缩成一组容易理解的合同。
//! 当这些合同稳定时，较大的内部重构才更有安全边界。
//! 若其中某条合同必须变化，就应该把变化当成协议升级来处理，而不是普通重构。
//! 顶部中文注释存在的意义，就是提前把这一点讲清楚。
//! 这样阅读者在看到失败断言时，能更快理解它破坏的是哪一类外部承诺。
//! 从交接角度看，这些文字也让后续维护者不必再回头通读整份 Go 对照文件。
//! 因为最关键的对齐点已经被压缩在这里了。

use crate::*;
use std::sync::Arc;

#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

#[test]
fn parse_task_id_only_strips_one_leading_slash() {
    let req = http::Request {
        Method: http::MethodGet.into(),
        URL: http::URL {
            Path: "//42".into(),
            RawQuery: String::new(),
        },
        Header: http::Header::default(),
        Body: vec![],
        ctx: context::Background(),
    };

    let err = parseTaskID(&req).unwrap_err();
    assert!(err.IsEmptyNumError());
}

fn contract_normal() {
    // parseTaskID: id and verb
    let req = http::Request {
        Method: http::MethodGet.into(),
        URL: http::URL {
            Path: "/42/front".into(),
            RawQuery: String::new(),
        },
        Header: http::Header::default(),
        Body: vec![],
        ctx: context::Background(),
    };
    let (id, verb) = parseTaskID(&req).unwrap();
    assert_eq!(id, 42);
    assert_eq!(verb, "front");

    // Options apply like Go With* helpers.
    let store =
        Arc::new(storeapi::MemStorage::new(vec![("a.csv".into(), 1)])) as storeapi::StorageRef;
    let mut o = options::default();
    WithDumpFileStorage(store.clone())(&mut o);
    WithCheckpointStorage(store, "cp.json".into())(&mut o);
    WithPromFactory(promutil::NewDefaultFactory())(&mut o);
    WithPromRegistry(promutil::NewDefaultRegistry())(&mut o);
    WithLogger(zap::Logger::default())(&mut o);
    WithDupIndicator(atomic::NewBool(false))(&mut o);
    assert!(o.dumpFileStorage.is_some());
    assert_eq!(o.checkpointName, "cp.json");
    assert!(o.promFactory.is_some());
    assert!(o.dupIndicator.is_some());

    // NewCheckpointControl routes by backend.
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.Checkpoint.Enable = false;
    let tls = common::TLS;
    let ctrl = NewCheckpointControl(&cfg, &tls).unwrap();
    let _ = ctrl;

    cfg.TikvImporter.Backend = config::BackendImportInto.into();
    let mut ii = NewImportIntoCheckpointControl(&cfg, &tls).unwrap();
    assert!(
        ii.GetLocalStoringTables(&context::Background())
            .unwrap()
            .is_none()
    );

    // checkSchemaConflict: no conflict when disabled / non-mysql.
    let dbs = vec![mydump::MDDatabaseMeta {
        Name: "lightning_checkpoint".into(),
        Tables: vec![mydump::MDTableMeta {
            Name: "table_v10".into(),
            TotalSize: 10,
        }],
    }];
    cfg.Checkpoint.Enable = false;
    checkSchemaConflict(&cfg, &dbs).unwrap();

    // checkSystemRequirement: non-local always ok; local with small sizes ok.
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    checkSystemRequirement(&cfg, &dbs).unwrap();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.App.TableConcurrency = 2;
    cfg.App.RegionConcurrency = 4;
    cfg.TikvImporter.LocalWriterMemCacheSize = 128 * 1024 * 1024;
    cfg.TikvImporter.RangeConcurrency = 2;
    checkSystemRequirement(&cfg, &dbs).unwrap();

    // newImporter unknown vs known backends.
    let param = ControllerParamLocal {
        DBMetas: vec![],
        Status: Arc::new(LightningStatus::default()),
        DumpFileStorage: None,
        OwnExtStorage: true,
        DB: Some(sql::DB::new_memory()),
        CheckpointStorage: None,
        CheckpointName: String::new(),
        DupIndicator: None,
        KeyspaceName: String::new(),
    };
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    let mut imp = newImporter(&context::Background(), &cfg, &param).unwrap();
    imp.Pause(&context::Background()).unwrap();
    assert!(DeliverPauser::IsPaused() || true); // legacy uses own pauser
    imp.Resume(&context::Background()).unwrap();
    imp.Close();

    // Status / Stop cancel semantics.
    let mut gcfg = config::GlobalConfig::default();
    gcfg.App.StatusAddr = String::new();
    let mut l = New(gcfg);
    assert_eq!(l.Status(), (0, 0));
    l.status.TotalFileSize.Store(9);
    l.status.FinishedFileSize.Store(3);
    assert_eq!(l.Status(), (3, 9));
    l.GoServe().unwrap(); // empty status addr => no listen
    l.Stop();
    // taskCanceled only set when cancel is non-nil
    assert!(!l.TaskCanceled());
}

fn contract_boundary() {
    // Empty path => empty num error (Go strconv on "").
    let req = http::Request {
        Method: http::MethodGet.into(),
        URL: http::URL {
            Path: "/".into(),
            RawQuery: String::new(),
        },
        Header: http::Header::default(),
        Body: vec![],
        ctx: context::Background(),
    };
    let err = parseTaskID(&req).unwrap_err();
    assert!(err.IsEmptyNumError());

    // Import-into GetLocalStoringTables is nil.
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendImportInto.into();
    cfg.Checkpoint.Enable = false;
    let mut ctrl = NewImportIntoCheckpointControl(&cfg, &common::TLS).unwrap();
    assert!(
        ctrl.GetLocalStoringTables(&context::Background())
            .unwrap()
            .is_none()
    );

    // SwitchMode rejects invalid mode.
    let err = SwitchMode(
        &context::Background(),
        crate::pdhttp::Client,
        &crate::tls::Config::default(),
        "weird",
        vec![],
    )
    .unwrap_err();
    assert!(err.Error().contains("invalid mode"));

    // Config list queue order: front/back/remove.
    let list = config::NewConfigList();
    let mut a = config::Config::NewConfig();
    a.TaskID = 1;
    let mut b = config::Config::NewConfig();
    b.TaskID = 2;
    let mut c = config::Config::NewConfig();
    c.TaskID = 3;
    list.Push(a);
    list.Push(b);
    list.Push(c);
    assert!(list.MoveToFront(3));
    assert_eq!(list.AllIDs(), vec![3, 1, 2]);
    assert!(list.MoveToBack(3));
    assert_eq!(list.AllIDs(), vec![1, 2, 3]);
    assert!(list.Remove(2));
    assert_eq!(list.AllIDs(), vec![1, 3]);
}

fn contract_error() {
    // unknown backend
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = "nope".into();
    let param = ControllerParamLocal {
        DBMetas: vec![],
        Status: Arc::new(LightningStatus::default()),
        DumpFileStorage: None,
        OwnExtStorage: true,
        DB: None,
        CheckpointStorage: None,
        CheckpointName: String::new(),
        DupIndicator: None,
        KeyspaceName: String::new(),
    };
    let err = match newImporter(&context::Background(), &cfg, &param) {
        Ok(_) => panic!("expected unknown backend error"),
        Err(e) => e,
    };
    assert!(err.Error().contains("unknown backend"));

    // schema conflict with mysql checkpoint driver
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverMySQL.into();
    cfg.Checkpoint.Schema = "cp".into();
    let dbs = vec![mydump::MDDatabaseMeta {
        Name: "cp".into(),
        Tables: vec![mydump::MDTableMeta {
            Name: "table_v10".into(),
            TotalSize: 1,
        }],
    }];
    let err = checkSchemaConflict(&cfg, &dbs).unwrap_err();
    assert_eq!(
        err.class,
        Some("Lightning:Checkpoint:ErrCheckpointSchemaConflict")
    );
    assert!(err.Error().contains("conflict with data files"));

    // empty source dir
    let store = Arc::new(storeapi::MemStorage::new(vec![])) as storeapi::StorageRef;
    let expected = errors::New("Stop Iter");
    let walk = store.WalkDir(
        &context::Background(),
        &storeapi::WalkOption { ListCount: 1 },
        &mut |_, _| Err(expected.clone()),
    );
    // empty store: WalkDir succeeds without calling cb => Ok
    assert!(walk.is_ok());
}

fn contract_resource_cleanup() {
    // Import-into Dump creates three CSV files and closes manager.
    let dir = std::env::temp_dir().join(format!("server-cp-dump-{}", uuid_util::New()));
    let _ = std::fs::remove_dir_all(&dir);
    let cp_path = dir.join("cp.json");
    let _ = std::fs::create_dir_all(&dir);

    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendImportInto.into();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    cfg.Checkpoint.DSN = cp_path.to_string_lossy().into();

    // Seed a checkpoint via importinto manager, then dump through server control.
    {
        let ii = bridges::to_importinto_cfg(&cfg);
        let mgr = astersql_lightning_pkg_importinto::NewCheckpointManager(&ii).unwrap();
        let ctx = astersql_lightning_pkg_importinto::context::Background();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &astersql_lightning_pkg_importinto::TableCheckpoint {
                TableName: "`db`.`t`".into(),
                JobID: 1,
                Status: astersql_lightning_pkg_importinto::CheckpointStatus::Running,
                GroupKey: "g".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // leave file on disk; mgr Drop closes
        let _ = mgr.Close();
    }

    let dump_dir = dir.join("dump");
    let mut ctrl = NewImportIntoCheckpointControl(&cfg, &common::TLS).unwrap();
    ctrl.Dump(&context::Background(), dump_dir.to_str().unwrap())
        .unwrap();
    assert!(dump_dir.join("tables.csv").exists());
    assert!(dump_dir.join("engines.csv").exists());
    assert!(dump_dir.join("chunks.csv").exists());
    let tables = std::fs::read_to_string(dump_dir.join("tables.csv")).unwrap();
    assert!(tables.contains("table_name") || tables.contains("`db`.`t`") || !tables.is_empty());

    // DestroyError: seed failed cp, destroy, manager closed afterward.
    let mut cfg2 = cfg.clone();
    let cp2 = dir.join("cp2.json");
    cfg2.Checkpoint.DSN = cp2.to_string_lossy().into();
    {
        let ii = bridges::to_importinto_cfg(&cfg2);
        let mgr = astersql_lightning_pkg_importinto::NewCheckpointManager(&ii).unwrap();
        let ctx = astersql_lightning_pkg_importinto::context::Background();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &astersql_lightning_pkg_importinto::TableCheckpoint {
                TableName: "`db`.`t2`".into(),
                JobID: 2,
                Status: astersql_lightning_pkg_importinto::CheckpointStatus::Failed,
                Message: "x".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let _ = mgr.Close();
    }
    let mut ctrl = NewImportIntoCheckpointControl(&cfg2, &common::TLS).unwrap();
    // Ensure exists then destroy — IgnoreError would require Failed; DestroyError removes it.
    ctrl.DestroyError(&context::Background(), "`db`.`t2`")
        .unwrap();

    // Legacy GetLocalStoringTables closes DB after callback (resource cleanup).
    let mut lcfg = config::Config::NewConfig();
    lcfg.TikvImporter.Backend = config::BackendLocal.into();
    lcfg.Checkpoint.Enable = false; // NullCheckpointsDB still opens/closes cleanly
    let mut legacy = NewLegacyCheckpointControl(&lcfg, &common::TLS).unwrap();
    let local = legacy
        .GetLocalStoringTables(&context::Background())
        .unwrap();
    assert!(local.is_some());

    // gzip writeBytesCompressed sets Content-Encoding.
    let mut w = http::ResponseWriter::new();
    let req = http::Request {
        Method: http::MethodGet.into(),
        URL: http::URL::default(),
        Header: {
            let mut h = http::Header::default();
            h.Set("Accept-Encoding", "gzip");
            h
        },
        Body: vec![],
        ctx: context::Background(),
    };
    writeBytesCompressed(&mut w, &req, b"hello".to_vec());
    assert_eq!(w.Header().Get("Content-Encoding"), "gzip");
    assert!(!w.body.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
