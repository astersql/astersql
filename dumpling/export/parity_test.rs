// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/export` public contracts vs Go sources.
//!
//! 这个文件把若干对外可观察的 Rust 行为整理成一组“契约快照”，
//! 用来确认迁移后的实现仍然和 Go 版本保持相同的默认值、边界处理、
//! 错误语义以及资源清理顺序。
//! 这里不追求覆盖所有分支，而是优先锁住最容易因为重构而漂移的公共接口。

use std::sync::Arc;

use astersql_dumpling_context as tcontext;
use astersql_dumpling_log::{Level, Logger, ZapLogger};

use crate::*;

#[test]
fn go_rust_public_contract_matches() {
    // 入口只负责串起四类契约，失败时由子函数给出更接近语义的断点。
    // 这种拆分和 Go 中按主题分段校验的思路一致，便于后续继续加案例。
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn contract_normal() {
    // 正常路径契约：
    // 这里集中校验“默认配置 + 常见 helper + 轻量状态对象”在理想输入下的行为。
    initColumnTypeSets();

    // 先锁住 DefaultConfig 的关键默认值，避免配置迁移时无意偏离 Go 习惯。
    // DefaultConfig matches Go defaults
    let conf = DefaultConfig();
    assert_eq!(conf.Host, "127.0.0.1");
    assert_eq!(conf.Port, 3306);
    assert_eq!(conf.User, "root");
    assert_eq!(conf.Threads, 4);
    assert_eq!(conf.Consistency, ConsistencyTypeAuto);
    assert!(conf.NoViews);
    assert!(conf.DumpEmptyDatabase);
    assert_eq!(conf.CsvNullValue, "\\N");
    assert_eq!(conf.StatementSize, DefaultStatementSize);

    // 未显式指定导出格式时，应回退到 SQL 文本导出。
    // adjustFileFormat empty -> sql
    let mut conf = DefaultConfig();
    adjustFileFormat(&mut conf).unwrap();
    assert_eq!(conf.FileType, FileFormatSQLTextString);

    // 指定原始 SQL 后输出会切成 CSV，保持 Go 侧“查询结果导出”约束。
    // SQL specified -> csv
    let mut conf = DefaultConfig();
    conf.SQL = "select 1".into();
    adjustFileFormat(&mut conf).unwrap();
    assert_eq!(conf.FileType, FileFormatCSVString);

    // Brief 文案属于外部可见字符串，连 Go 里遗留的拼写也要保持一致。
    // Task Brief strings (including Go typo "dababase")
    assert_eq!(
        NewTaskDatabaseMeta("db1", "create database db1").Brief(),
        "meta of dababase 'db1'"
    );
    assert_eq!(
        NewTaskTableMeta("db", "t", "create table t(a int)").Brief(),
        "meta of table 'db'.'t'"
    );
    assert_eq!(
        NewTaskPolicyMeta("p1", "create placement policy p1").Brief(),
        "meta of placement policy 'p1'"
    );

    // 转义 helper 既影响 SQL writer，也影响 CSV/文本路径中的转义兼容性。
    // escape SQL / CSV
    let mut bf = Vec::new();
    astersql_dumpformat_sqlfile::append_value(
        &mut bf,
        b"a'b\\c\n",
        false,
        astersql_dumpformat_csvfile::FieldKind::String,
        true,
    );
    assert_eq!(String::from_utf8_lossy(&bf), "'a\\'b\\\\c\\n'");

    // MySQL 错误文本解析要能还原出 schema/table，供重试和提示逻辑复用。
    // getTableFromMySQLError
    let (db, tbl) = getTableFromMySQLError("Table 'pingcap.t1' doesn't exist").unwrap();
    assert_eq!(db, "pingcap");
    assert_eq!(tbl, "t1");

    // RR 需求依赖服务类型判断，TiDB 的 snapshot 路径不要求再切 repeatable read。
    // needRepeatableRead
    assert!(needRepeatableRead(
        ServerType::ServerTypeMySQL,
        ConsistencyTypeSnapshot
    ));
    assert!(!needRepeatableRead(
        ServerType::ServerTypeTiDB,
        ConsistencyTypeSnapshot
    ));

    // 过滤器要只保留匹配到的库表，同时不破坏预先整理好的 DatabaseTables 结构。
    // DatabaseTables + filter
    let logger = Logger {
        Logger: ZapLogger::capture(Level::Debug),
    };
    let tctx = tcontext::NewContext(tcontext::GoBackground(), logger);
    let mut conf = DefaultConfig();
    conf.TableFilter = filter_parse(&["db.*".into()]).unwrap();
    conf.DumpEmptyDatabase = true;
    conf.Tables = NewDatabaseTables();
    conf.Tables
        .AppendTables("db", &["t1".into(), "t2".into()], &[1, 2]);
    conf.Tables.AppendTables("other", &["x".into()], &[1]);
    filterTables(&tctx, &mut conf);
    assert!(conf.Tables.contains_key("db"));
    assert!(!conf.Tables.contains_key("other"));

    // SpeedRecorder 的关键语义是“无新增字节时保留上一轮速度”，避免瞬时归零抖动。
    // SpeedRecorder
    let mut sr = NewSpeedRecorder();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let s1 = sr.GetSpeed(100.0);
    assert!(s1 > 0.0);
    let s2 = sr.GetSpeed(100.0); // no progress -> keep old
    assert_eq!(s2, s1);

    // metrics 包装函数允许直接读写底层值，便于测试快速锁定 registry 之外的语义。
    // metrics Read/Add
    let f = NewDefaultFactory();
    let m = newMetrics(f.as_ref(), &Labels::default());
    AddCounter(Some(&m.finishedTablesCounter), 3.0);
    assert_eq!(ReadCounter(Some(&m.finishedTablesCounter)), 3.0);
    AddGauge(Some(&m.finishedSizeGauge), 10.0);
    assert_eq!(ReadGauge(Some(&m.finishedSizeGauge)), 10.0);

    // SQL 构造 helper 要保证字面输出稳定，否则 writer / lock 流程都会连带漂移。
    // buildSelectQuery / lock SQL
    let q = buildSelectQuery("db", "t", "a,b", "", "WHERE a>1", "ORDER BY a");
    assert_eq!(q, "SELECT a,b FROM `db`.`t` WHERE a>1 ORDER BY a");
    let mut tables = NewDatabaseTables();
    tables.AppendTables("db", &["t".into()], &[1]);
    let lock = buildLockTablesSQL(&tables, &std::collections::HashMap::new());
    assert_eq!(lock, "LOCK TABLES `db`.`t` READ");

    // 版本串解析承担后续多处分支判断，这里先锁住 TiDB 识别结果。
    // ParseServerInfo
    let si = ParseServerInfo("8.0.11-TiDB-v7.5.0");
    assert_eq!(si.ServerType, ServerType::ServerTypeTiDB);

    // 特殊注释集合是导出 schema 时的兼容补丁点，长度变化通常意味着语义漂移。
    // special comments
    let cmts = getSpecialComments(ServerType::ServerTypeTiDB);
    assert_eq!(cmts.len(), 2);

    // RowReceiver 负责把数据库原始字节绑定到 writer 可消费的序列化路径。
    // MakeRowReceiver number write
    let mut rec = MakeRowReceiver(&["INT".into()]);
    let mut args = [RawBytes(Some(b"42".to_vec()))];
    rec.BindAddress(&mut args);
    let mut out = Vec::new();
    let mut sw = astersql_dumpformat_sqlfile::Writer::new(
        &mut out,
        vec![],
        columnKinds(&["INT".into()]),
        astersql_dumpformat_sqlfile::Config::default(),
    );
    sw.write(
        &rec.GetRawBytes()
            .into_iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    drop(sw);
    assert_eq!(String::from_utf8_lossy(&out), "(42)");
}

fn contract_boundary() {
    // 边界契约：
    // 这里覆盖去重、模板、版本窗口和空实现等“不会经常改，但一改就会出兼容问题”的点。
    // 分组方式对应 Go 里大量 table-driven case 的核心边界。

    // 分区名需要 trim、转小写并去重，确保后续拼接 SQL 时结果稳定。
    // normalizePartitions dedupe/lowercase/trim
    assert_eq!(
        normalizePartitions(&[" A ".into(), "a".into(), "".into(), "b".into()]),
        vec!["a".to_string(), "b".to_string()]
    );

    // 该 helper 只在特定 MySQL 版本窗口内返回 true，越界后必须立即失效。
    // matchMysqlBugversion window
    let mut info = ServerInfo {
        ServerType: ServerType::ServerTypeMySQL,
        ServerVersion: Some(parse_semver("8.0.10")),
        HasTiKV: false,
    };
    assert!(matchMysqlBugversion(&info));
    info.ServerVersion = Some(parse_semver("8.0.23"));
    assert!(!matchMysqlBugversion(&info));
    info.ServerType = ServerType::ServerTypeTiDB;
    assert!(!matchMysqlBugversion(&info));

    // 显式指定的库表列表要么完整解析，要么因为非法字面直接报错。
    // GetConfTables
    let dt = GetConfTables(&["db.t1".into(), "db.t2".into()]).unwrap();
    assert_eq!(dt.get("db").unwrap().len(), 2);
    assert!(GetConfTables(&["bad".into()]).is_err());

    // 匿名输出模板至少要保留可区分 chunk 的名称片段，避免不同块互相覆盖。
    // output template anonymous
    let tmpl = ParseOutputFileTemplate(DefaultAnonymousOutputFileTemplateText).unwrap();
    let name = tmpl.Execute("data", "db", "t", "000000001", "").unwrap();
    assert!(name.contains("result") || name.contains("000000001"));

    // 标识符包裹逻辑必须正确逃逸反引号，否则生成 SQL 会直接失效。
    // wrapBackTicks
    assert_eq!(wrapBackTicks("a`b"), "`a``b`");

    // Go 的 regexp 只转义文件名禁用字符，合法 UTF-8 数据库名必须逐字保留。
    assert_eq!(filename_escape("数据库.表"), "数据库%2E表");
    assert_eq!(filename_escape("表-schema"), "表%2Dschema");

    // where 子句构造会额外带上结束哨兵，因此长度应始终是边界值数 + 1。
    // buildWhereClauses length = n+1
    let clauses = buildWhereClauses(&["id".into()], &[vec!["10".into()], vec!["20".into()]]);
    assert_eq!(clauses.len(), 3);

    // None 一致性控制器本身不做任何事，但 setup/teardown/ping 仍需是可调用的 no-op。
    // ConsistencyNone
    let mut c = ConsistencyNone;
    let tctx = tcontext::Background();
    c.Setup(&tctx).unwrap();
    c.TearDown().unwrap();
    c.PingContext().unwrap();
}

fn contract_error() {
    // 错误路径契约：
    // 这些断言重点验证“何时必须拒绝继续执行”，而不是只看错误类型是否存在。

    // 指定 SQL 和 where 不能并存，否则会让导出条件的责任边界变得不清晰。
    // validateSpecifiedSQL
    let mut conf = DefaultConfig();
    conf.SQL = "select 1".into();
    conf.Where = "a=1".into();
    assert!(validateSpecifiedSQL(&conf).is_err());

    // 未知文件格式必须尽早返回配置错误，不能偷偷回退到默认值。
    // adjustFileFormat unknown
    let mut conf = DefaultConfig();
    conf.FileType = "rand_str".into();
    assert!(
        adjustFileFormat(&mut conf)
            .unwrap_err()
            .msg
            .contains("unknown config.FileType")
    );

    // snapshot 只允许 TiDB 走，非 TiDB 需要在控制器构造阶段就失败。
    // snapshot on non-tidb
    let db = DB::new();
    let mut conf = DefaultConfig();
    conf.Consistency = ConsistencyTypeSnapshot.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
    assert!(NewConsistencyController(&conf, &db).is_err());

    // flush 模式在 TiDB 上仍会因为缺少对应支持而在 setup 阶段报错。
    // flush on tidb setup error
    let mut conf = DefaultConfig();
    conf.Consistency = ConsistencyTypeFlush.to_string();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    let mut c = NewConsistencyController(&conf, &db).unwrap();
    assert!(c.Setup(&tcontext::Background()).is_err());

    // 三段式名字不符合当前解析器假设，应明确返回 unsupported。
    // getTableFromMySQLError unsupported
    assert!(getTableFromMySQLError("Table 'a.b.c' doesn't exist").is_err());

    // 未知表类型不能吞掉，否则后续 prepare 流程会误把异常元数据当正常表。
    // ParseTableType unknown
    assert!(ParseTableType("NOPE").is_err());

    // 网络关闭判断直接影响 HTTP 服务收尾日志是否会被误报成真实错误。
    // isErrNetClosing
    assert!(isErrNetClosing(&errors_new(
        "use of closed network connection"
    )));
    assert!(!isErrNetClosing(&errors_new("other")));

    // pingcap/errors 注释错误时保留底层 cause；dbutil 的重试分类因此仍应看到
    // 原始 MySQL 错误码，而不是退化为对注释文本做关键字猜测。
    let retryable = Error::with_mysql(MySQLError {
        Number: 1213,
        Message: "Deadlock found when trying to get lock".into(),
    });
    let annotated = errors_annotate(retryable, "query failed");
    assert_eq!(annotated.mysql.as_ref().map(|err| err.Number), Some(1213));
    assert!(IsRetryableError(&annotated));

    // database/sql 要求 Scan 的目标数量与结果列数严格一致，测试桩不能把
    // 少传目标悄悄当成成功，否则会掩盖 Rust 调用方的列映射错误。
    let mut rows = Rows::new(vec!["a".into()], vec![vec![Some(b"1".to_vec())]]);
    assert!(rows.Next());
    assert!(rows.Scan(&mut []).is_err());
}

fn contract_resource_cleanup() {
    // 资源清理契约：
    // 这里锁住注册反注册、句柄停止、连接关闭和 Dumper.Close 的收尾副作用，
    // 确保迁移后的实现不会留下脏状态或漏掉 cancel。

    // metrics 允许重复注册后显式反注册，避免测试或多次运行时积累脏 registry 状态。
    // SpeedRecorder / metrics unregister
    let reg = NewDefaultRegistry();
    let f = NewDefaultFactory();
    let m = newMetrics(f.as_ref(), &Labels::default());
    m.registerTo(reg.as_ref());
    m.unregisterFrom(reg.as_ref());

    // HTTP 句柄 stop 之后要显式翻转状态位，便于上层判断服务是否已关闭。
    // HttpServiceHandle stop
    let h = HttpServiceHandle::start(":0").unwrap();
    assert!(h.started.load(std::sync::atomic::Ordering::SeqCst));
    h.stop();
    assert!(h.stopped.load(std::sync::atomic::Ordering::SeqCst));

    // 内存存储同时覆盖整文件写入和增量 writer 关闭路径，模拟导出落盘最小闭环。
    // MemStorage write/read/close writer
    let store = MemStorage::new("/tmp/dumpling-export-parity");
    store.WriteFile("a.sql", b"hello").unwrap();
    assert_eq!(store.ReadFile("a.sql").unwrap(), b"hello");
    let mut w = store.Create("b.sql").unwrap();
    w.Write(b"x").unwrap();
    w.Close().unwrap();
    assert_eq!(store.ReadFile("b.sql").unwrap(), b"x");

    // Conn 在 Close 后继续 Ping 必须失败，表示资源状态已真正切换为不可用。
    // Conn close -> ping error
    let c = Conn::new();
    c.Close().unwrap();
    assert!(c.PingContext().is_err());

    // Dumper.Close 是这里最关键的资源回收检查：
    // 既要取消 context，也要释放 db / pd client 等可观察句柄。
    // Dumper Close cancels + cleans
    let mut conf = DefaultConfig();
    conf.StatusAddr = String::new();
    conf.Logger = Some(Logger {
        Logger: ZapLogger::capture(Level::Error),
    });
    // 手工拼出最小 Dumper，避免走完整初始化路径带来的文件和网络副作用。
    // Seed version query for detectServerInfo
    let mut dumper_conf = conf;
    // Use open path manually to avoid InitAppLogger file side effects
    let (tctx, cancel) = tcontext::Background().WithCancel();
    let factory = dumper_conf.PromFactory.clone();
    let labels = dumper_conf.Labels.clone();
    let mut d = Dumper {
        tctx: tctx.WithLogger(dumper_conf.Logger.clone().unwrap()),
        conf: Arc::new(dumper_conf),
        db: Some(DB::new()),
        ext_storage: Some(Arc::new(MemStorage::new("."))),
        metrics: newMetrics(factory.as_ref(), &labels),
        speedRecorder: std::sync::Mutex::new(NewSpeedRecorder()),
        totalTables: std::sync::atomic::AtomicI64::new(0),
        cancel: Some(cancel),
        http: None,
        pd_client: Some(PdClient::default()),
    };
    // script version
    d.db.as_ref().unwrap().seed_query(
        "SELECT VERSION()",
        vec!["VERSION()".into()],
        vec![vec![Some(b"8.0.11-TiDB-v7.5.0".to_vec())]],
    );
    d.Close().unwrap();
    assert!(d.cancel.is_none());
    assert!(d.db.is_none());
    assert!(d.pd_client.is_none());
}
