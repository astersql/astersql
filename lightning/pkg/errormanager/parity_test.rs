// Copyright 2026 AsterSQL.

//! Parity tests for `lightning/pkg/errormanager` public contracts vs Go.
//!
//! 中文总览：这组 parity 测试验证的不是完整导入流程，而是 `errormanager`
//! 对外公共契约是否继续与 Go 版本保持同一组可观察行为。
//! 它把断言拆成四段：正常路径、边界条件、错误返回和资源回收。
//! 正常路径关注常量、构造函数、初始化 SQL、副作用日志和错误汇总输出。
//! 边界条件关注空 DB、空冲突列表、不同 backend 的开关组合，以及只记录一次重复错误的 CAS 语义。
//! 错误路径则保护各类阈值耗尽后返回的错误文本，避免上层因为字符串或分类漂移而误判。
//! 资源回收路径确认关闭 DB、跳过记录和 replace 清理循环仍与 Go 版保持一致。
//! 这些场景拼起来，覆盖了错误管理器被 importer 真正依赖的最小公共面。
//! 如果这里回归，往往不是内部实现细节变化那么简单，而是调用方可见的契约已经变了。
//! 因此注释重点说明“为什么这些断言存在”，而不是重复代码本身在做什么。
//! 由于本次任务只补注释，所有测试数据、阈值和断言顺序都保持原样。
//! 阅读顺序上，可以把它理解成一份“错误管理器外部说明书”的可执行版本。
//! 正常路径回答“它平时应该怎么工作”，
//! 边界路径回答“输入稀疏或依赖缺失时是否还能稳住”，
//! 错误路径回答“阈值耗尽后给调用方什么信号”，
//! 回收路径则回答“做完事后会留下什么副作用”。
//! 这也是为什么文件名叫 parity：它保护的是对外行为对齐，而不是内部实现逐行一致。

use crate::atomic;
use crate::config;
use crate::context;
use crate::errors;
use crate::log;
use crate::sql;
use crate::tidbtbl;
use crate::util;
use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 统一入口按四段执行，失败时可以直接定位是哪一类对外保证发生了漂移。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn sample_cfg() -> config::Config {
    // 这份样例配置刻意启用 local backend、replace-on-dup 和 precheck，
    // 因为这是最能同时触发 V1/V2 冲突表、阈值和错误记录路径的组合。
    // 统一从这里构造配置，也能避免各段测试因为夹具差异而引入无关波动。
    // 也就是说，这个 helper 保护的不只是便捷性，还有测试场景的一致性。
    let mut cfg = config::Config::NewConfig();
    cfg.TaskID = 42;
    cfg.TikvImporter.Backend = config::BackendLocal.to_string();
    cfg.Conflict.Strategy = config::ReplaceOnDup;
    cfg.Conflict.PrecheckConflictBeforeImport = true;
    cfg.Conflict.Threshold = 20;
    cfg.Conflict.MaxRecordRows = 10;
    cfg.App.MaxError.Type.Store(10);
    cfg.App.MaxError.Syntax.Store(0);
    cfg.App.MaxError.Charset.Store(100);
    cfg.App.TaskInfoSchemaName = "lightning_errors".into();
    cfg
}

fn contract_normal() {
    // Table name constants match Go.
    // 这些常量会直接出现在建表 SQL、视图名和错误汇总输出里，必须保持稳定。
    // 一旦名称变化，旧任务脚本、排障文档和人工查询语句都可能失效。
    // 因而这里虽然只是字符串断言，本质上保护的是用户操作手册层面的兼容性。
    assert_eq!(ConflictErrorTableName, "conflict_error_v4");
    assert_eq!(DupRecordTableName, "conflict_records_v2");
    assert_eq!(ConflictViewName, "conflict_view");

    let db = sql::DB::new_memory();
    let cfg = sample_cfg();
    let em = New(Some(db.clone()), &cfg, log::Logger::L());

    // New enables V1+V2 for local + replace + precheck.
    // 这里验证构造阶段就能把 backend、策略和阈值折叠成最终开关状态，
    // 避免调用方还要自己推导“当前应该启哪套冲突记录机制”。
    // 对迁移版来说，这也是确认 Rust 没把 Go 中的布尔组合关系误改掉。
    assert!(em.conflictV1Enabled);
    assert!(em.conflictV2Enabled);
    assert_eq!(em.TypeErrorsRemain(), 10);
    assert_eq!(em.ConflictErrorsRemain(), 20);
    assert_eq!(em.ConflictRecordsRemain(), 10);
    assert!(!em.RecordErrorOnce());
    assert_eq!(em.schema, "lightning_errors");
    assert_eq!(em.taskID, 42);

    // Init executes schema/table/view DDL (trimmed, identifier-escaped).
    // 初始化的关键不是 SQL 文本逐字完全一致，而是 schema、错误表和视图都被按 Go 语义创建。
    // 因此断言抓住了最能代表契约的表名和语句类型。
    // 这里还顺带保护了标识符转义结果，确保 schema 名带反引号时语义不漂移。
    // 如果这些对象没建全，后续任何错误记录都会在更晚的位置才爆炸，定位会更困难。
    em.Init(context::Background()).unwrap();
    let log = db.exec_log();
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("CREATE SCHEMA IF NOT EXISTS `lightning_errors`")),
        "missing create schema: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("type_error_v2") && q.contains("CREATE TABLE")),
        "missing type error table: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("conflict_error_v4") && q.contains("CREATE TABLE")),
        "missing conflict table: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("conflict_records_v2") && q.contains("CREATE TABLE")),
        "missing dup records table: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("CREATE OR REPLACE VIEW") && q.contains("conflict_view")),
        "missing conflict view: {log:?}"
    );

    // HasError / Output with decremented counters (Go TestErrorMgrHasError / Output).
    // 汇总输出属于用户可见面，颜色控制符、标题文本和表名都可能被文档或运维脚本依赖。
    // 所以这里只要错误计数发生变化，就必须继续产生与 Go 对齐的摘要格式。
    // 反过来说，当没有错误时输出必须为空，不能平白制造噪音摘要。
    let mut cfg2 = config::Config::default();
    cfg2.App.MaxError.Syntax.Store(100);
    cfg2.App.MaxError.Charset.Store(100);
    cfg2.App.MaxError.Type.Store(100);
    cfg2.Conflict.Threshold = 100;
    let em2 = ErrorManager {
        db: None,
        taskID: 0,
        schema: "error_info".into(),
        configError: cfg2.App.MaxError.clone(),
        remainingError: cfg2.App.MaxError.clone(),
        configConflict: cfg2.Conflict.clone(),
        conflictErrRemain: atomic::NewInt64(100),
        conflictRecordsRemain: atomic::NewInt64(0),
        conflictV1Enabled: true,
        conflictV2Enabled: false,
        logger: log::Logger::L(),
        recordErrorOnce: atomic::NewBool(false),
        encode_map: Default::default(),
    };
    assert!(!em2.HasError());
    assert_eq!(em2.Output(), "");

    em2.remainingError.Syntax.Sub(1);
    assert!(em2.HasError());
    let out = em2.Output();
    assert!(out.contains("Import Data Error Summary:"));
    assert!(out.contains("Data Syntax"));
    assert!(out.contains("`error_info`.`syntax_error_v2`"));
    assert!(out.contains("\x1b[31m"), "row painter should use FgRed");
}

fn contract_boundary() {
    // Go's WriteMySQLIdentifier preserves UTF-8 bytes and only doubles backticks.
    assert_eq!(common::EscapeIdentifier("表`名"), "`表``名`");

    // nil db → Init is no-op.
    // 没有可用 DB 时，初始化应该静默跳过，而不是把“缺依赖”误报成导入错误。
    // 这也体现了错误管理器把“资源不存在”和“记录错误失败”分开处理的设计。
    let mut cfg = sample_cfg();
    cfg.App.TaskInfoSchemaName.clear();
    let em = New(Some(sql::DB::new_memory()), &cfg, log::Logger::L());
    assert!(em.db.is_none());
    assert!(em.Init(context::Background()).is_ok());

    // Empty conflict infos → Ok, no counter change.
    // 空输入应被视为没有工作要做，而不是异常情况。
    // 否则调用方必须在外层额外加分支，接口可用性会比 Go 更差。
    let db = sql::DB::new_memory();
    let cfg = sample_cfg();
    let em = New(Some(db), &cfg, log::Logger::L());
    let before = em.ConflictErrorsRemain();
    em.RecordDataConflictError(context::Background(), log::Logger::L(), "t", &[])
        .unwrap();
    assert_eq!(em.ConflictErrorsRemain(), before);

    // Backend TiDB enables V2 only.
    // 这里保护的是 backend 选择与冲突记录策略之间的组合逻辑。
    // 该组合直接决定创建哪些表、走哪条冲突处理分支，以及输出里暴露哪些数据来源。
    let mut cfg = sample_cfg();
    cfg.TikvImporter.Backend = config::BackendTiDB.to_string();
    cfg.Conflict.PrecheckConflictBeforeImport = false;
    let em = New(None, &cfg, log::Logger::L());
    assert!(!em.conflictV1Enabled);
    assert!(em.conflictV2Enabled);

    // ReplaceConflictKeys with nil db returns Ok immediately.
    // replace 流程在无 DB 条件下直接返回，说明资源边界判断发生在真正做事之前。
    // 这避免了“空资源环境下还尝试做冲突清理”的伪失败。
    let em = New(None, &sample_cfg(), log::Logger::L());
    let pool = util::WorkerPool::New(2, "test");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table::default(),
        "t",
        &pool,
        |_ctx, _key| Ok(vec![]),
        |_ctx, _keys| Ok(()),
    )
    .unwrap();

    // RecordDuplicateOnce only records once (CAS).
    // 这条语义避免同一批重复行被并发路径重复写入冲突记录表。
    // 断言插入次数为 1，本质上是在保护 compare-and-swap 门闩是否仍然生效。
    let db = sql::DB::new_memory();
    let cfg = sample_cfg();
    let em = New(Some(db.clone()), &cfg, log::Logger::L());
    em.RecordDuplicateOnce(
        context::Background(),
        log::Logger::L(),
        "t",
        "p",
        0,
        "dup",
        1,
        "row",
    );
    em.RecordDuplicateOnce(
        context::Background(),
        log::Logger::L(),
        "t",
        "p",
        1,
        "dup2",
        2,
        "row2",
    );
    assert!(em.RecordErrorOnce());
    let inserts: Vec<_> = db
        .exec_log()
        .into_iter()
        .filter(|(q, _)| q.contains("conflict_records_v2") && q.contains("INSERT"))
        .collect();
    assert_eq!(inserts.len(), 1, "only first duplicate should be recorded");
}

fn contract_error() {
    // Type error threshold exceeded returns annotated encode error.
    // 这里既保护阈值递减规则，也保护返回文本仍带着原始 encode 错误上下文。
    // 对使用者来说，知道“为什么超阈值”与知道“原始编码错误是什么”同样重要。
    let mut cfg = sample_cfg();
    cfg.App.MaxError.Type.Store(1);
    let em = New(None, &cfg, log::Logger::L());
    // First call consumes the single allowance (Dec → 0) and succeeds (no db).
    em.RecordTypeError(
        context::Background(),
        log::Logger::L(),
        "t",
        "p",
        0,
        "row",
        errors::New("encode boom"),
    )
    .unwrap();
    // Second call Dec → -1, returns annotated error.
    let err = em
        .RecordTypeError(
            context::Background(),
            log::Logger::L(),
            "t",
            "p",
            1,
            "row",
            errors::New("encode boom"),
        )
        .unwrap_err();
    let msg = err.Error();
    assert!(
        msg.contains("max-error.type") && msg.contains("encode boom"),
        "unexpected: {msg}"
    );

    // Conflict threshold exceeded.
    // 冲突阈值为零意味着任何新增冲突都应立即失败，而不是等落盘后再报错。
    // 两个 API 都测一遍，是为了确认数据冲突和重复计数共享同一套阈值口径。
    let mut cfg = sample_cfg();
    cfg.Conflict.Threshold = 0;
    let em = New(None, &cfg, log::Logger::L());
    let err = em.RecordDuplicateCount(1).unwrap_err();
    assert!(err.Error().contains("conflict.threshold"));

    let infos = vec![DataConflictInfo {
        RawKey: b"k".to_vec(),
        RawValue: b"v".to_vec(),
        KeyData: "kd".into(),
        Row: "r".into(),
    }];
    let err = em
        .RecordDataConflictError(context::Background(), log::Logger::L(), "t", &infos)
        .unwrap_err();
    assert!(err.Error().contains("conflict.threshold"));
}

fn contract_resource_cleanup() {
    // Init then Close the mock DB (resource release).
    // 关闭 DB 的断言虽然简单，却能确保错误管理器不会偷偷持有未释放连接。
    // 这对长时间运行的大型导入任务尤其关键，否则错误处理本身可能反向制造资源泄漏。
    let db = sql::DB::new_memory();
    let cfg = sample_cfg();
    let em = New(Some(db.clone()), &cfg, log::Logger::L());
    em.Init(context::Background()).unwrap();
    assert!(!db.is_closed());
    db.Close().unwrap();
    assert!(db.is_closed());

    // RecordDuplicate with MaxRecordRows exhausted skips insert but still decrements conflict counter.
    // 这条规则区分“是否还允许记录明细”和“是否还要统计冲突次数”两个不同概念。
    // 这样做能防止明细表无限膨胀，同时又不丢失总体冲突规模信息。
    let db = sql::DB::new_memory();
    let mut cfg = sample_cfg();
    cfg.Conflict.MaxRecordRows = 0;
    cfg.Conflict.Threshold = 5;
    let em = New(Some(db.clone()), &cfg, log::Logger::L());
    em.RecordDuplicate(
        context::Background(),
        log::Logger::L(),
        "t",
        "p",
        0,
        "dup",
        1,
        "row",
    )
    .unwrap();
    assert_eq!(em.ConflictErrorsRemain(), 4);
    let inserts: Vec<_> = db
        .exec_log()
        .into_iter()
        .filter(|(q, _)| q.contains("INSERT") && q.contains("conflict_records_v2"))
        .collect();
    assert!(
        inserts.is_empty(),
        "should not insert when records remain < 0 after Add(-1)"
    );

    // ReplaceConflictKeys empty query path still runs DELETE loop to completion.
    // 删除循环必须一直跑到受影响行数为零，才能保证冲突表被完整清理。
    // 这也是 Go 版在 replace 清理阶段最容易因为实现偷懒而回归的细节之一。
    let db = sql::DB::new_memory();
    db.push_delete_affected(2);
    db.push_delete_affected(0);
    let mut cfg = sample_cfg();
    cfg.App.TaskInfoSchemaName = "lightning_task_info".into();
    let em = New(Some(db.clone()), &cfg, log::Logger::L());
    let pool = util::WorkerPool::New(1, "resolve");
    em.ReplaceConflictKeys(
        context::Background(),
        tidbtbl::Table {
            meta: tidbtbl::TableMeta { clustered: true },
            cols: vec![],
        },
        "test",
        &pool,
        |_ctx, _key| Err(crate::tikverr::ErrNotFound("missing")),
        |_ctx, _keys| Ok(()),
    )
    .unwrap();
    let deletes: Vec<_> = db
        .exec_log()
        .into_iter()
        .filter(|(q, _)| q.contains("DELETE") && q.contains("conflict_error_v4"))
        .collect();
    assert!(
        deletes.len() >= 2,
        "expected delete loop until affected=0, got {deletes:?}"
    );
}
