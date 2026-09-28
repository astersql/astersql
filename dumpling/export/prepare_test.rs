// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `prepare_test.go`.
//!
//! 这个测试文件围绕 prepare 阶段最关键的几类契约展开：
//! 数据库清单选择、表清单来源、指定 SQL 配置校验，以及 auto consistency 的限制。
//! 它不验证真正的导出执行，而是把“进入导出前准备出的输入”锁定下来。

use crate::main_test::{app_logger, default_config_for_test};
use crate::*;

fn seed_show_databases(conn: &Conn, dbs: &[&str]) {
    // helper：把 SHOW DATABASES 结果按最小 rows 形态灌进 fake conn。
    // 后续多个测试都依赖它快速切换实例可见的数据库集合。
    let data: Vec<Vec<Option<Vec<u8>>>> = dbs
        .iter()
        .map(|d| vec![Some(d.as_bytes().to_vec())])
        .collect();
    conn.seed_query("SHOW DATABASES", vec!["Database".into()], data);
}

#[test]
fn test_default_output_file_template_matches_go_definitions() {
    let template = DefaultOutputFileTemplate();
    let cases = [
        ("schema", "db-schema-create"),
        ("event", "db.object-schema-post"),
        ("function", "db.object-schema-post"),
        ("procedure", "db.object-schema-post"),
        ("sequence", "db.object-schema-sequence"),
        ("trigger", "db.object-schema-triggers"),
        ("view", "db.object-schema-view"),
        ("table", "db.object-schema"),
        ("data", "db.object.000000001"),
        ("placement-policy", "policy-placement-policy-create"),
    ];

    for (name, expected) in cases {
        assert_eq!(
            template
                .Execute(name, "db", "object", "000000001", "policy")
                .unwrap(),
            expected,
            "default template definition {name} diverges from prepare.go"
        );
    }
}

#[test]
fn test_prepare_dumping_databases() {
    // 第一组：显式指定的数据库全部存在时，应按配置顺序返回。
    // 这里特意不按 SHOW DATABASES 返回顺序断言，强调白名单顺序由配置决定。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    seed_show_databases(&conn, &["db1", "db2", "db3", "db5"]);
    let mut conf = default_config_for_test();
    conf.Databases = vec!["db1".into(), "db2".into(), "db3".into()];
    let result = prepareDumpingDatabases(&tctx, &conf, &conn).unwrap();
    assert_eq!(result, vec!["db1", "db2", "db3"]);

    // 第二组：未显式指定数据库时，应返回 SHOW DATABASES 的过滤结果。
    // 这条语义决定了“默认导出全部可见库”仍然受 filter 控制。
    conf.Databases = vec![];
    seed_show_databases(&conn, &["db1", "db2"]);
    let result = prepareDumpingDatabases(&tctx, &conf, &conn).unwrap();
    assert_eq!(result, vec!["db1", "db2"]);

    // 第三组：底层 SHOW DATABASES 失败时，错误要原样向上传递。
    // prepare 阶段不吞错误，方便上层把失败定位到探测数据库清单这一步。
    conn.push_fail(errors_new("err"));
    assert!(prepareDumpingDatabases(&tctx, &conf, &conn).is_err());

    // 第四组：显式白名单里出现不存在的数据库时，要把缺失项集中报出来。
    // 这样用户一次就能看到所有拼写错误，而不是循环修一个再报下一个。
    seed_show_databases(&conn, &["db1", "db2", "db3", "db5"]);
    conf.Databases = vec!["db1".into(), "db2".into(), "db4".into(), "db6".into()];
    let err = prepareDumpingDatabases(&tctx, &conf, &conn).unwrap_err();
    assert_eq!(err.msg, "Unknown databases [db4,db6]");
}

#[test]
fn test_list_all_tables() {
    // 这组测试覆盖 information_schema 路径下的表清单准备。
    // 先准备一个同时包含 base table 和 view 的期望集合。
    // 这里既验证查询结果解析，也验证类型过滤不会打乱每库内部顺序。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut data = NewDatabaseTables();
    data.AppendTables("db1", &["t1".into(), "t2".into()], &[1, 2])
        .AppendTables("db2", &["t3".into(), "t4".into(), "t5".into()], &[3, 4, 5])
        .AppendViews("db3", &["t6", "t7", "t8"]);

    // 逐库为 INFORMATION_SCHEMA.TABLES 种入结果，模拟真实查询返回。
    // 视图在第一轮场景里故意不写入 rows，因为过滤目标只请求 base table。
    for (db_name, infos) in &data {
        let query = format!(
            "SELECT TABLE_NAME,TABLE_TYPE,AVG_ROW_LENGTH FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA='{}'",
            db_name
        );
        let mut rows = Vec::new();
        for tb in infos {
            if tb.Type == TableType::TableTypeView {
                continue;
            }
            rows.push(vec![
                Some(tb.Name.as_bytes().to_vec()),
                Some(tb.Type.String().as_bytes().to_vec()),
                Some(tb.AvgRowLength.to_string().into_bytes()),
            ]);
        }
        conn.seed_query(
            &query,
            vec![
                "TABLE_NAME".into(),
                "TABLE_TYPE".into(),
                "AVG_ROW_LENGTH".into(),
            ],
            rows,
        );
    }
    // 第一组断言：只请求 base table 时，结果里不应夹带视图。
    // 同时还要检查 AvgRowLength 等附带字段没有在解析时丢失。
    let db_names: Vec<String> = data.keys().cloned().collect();
    let tables = ListAllDatabasesTables(
        &tctx,
        &conn,
        &db_names,
        listTableType::listTableByInfoSchema,
        &[TableType::TableTypeBase],
    )
    .unwrap();
    for (d, table) in &tables {
        let expected = &data[d];
        let expected_base: Vec<_> = expected
            .iter()
            .filter(|t| t.Type == TableType::TableTypeBase)
            .cloned()
            .collect();
        assert_eq!(table.len(), expected_base.len());
        for i in 0..table.len() {
            assert!(table[i].Equals(&expected_base[i]));
        }
    }

    // 第二组断言：当调用方允许 view 时，同一库里应同时保留 base table 与 view。
    // 这一段补的是第一组未覆盖的“混合类型结果集”路径。
    let mut data = NewDatabaseTables();
    data.AppendTables("db", &["t1".into()], &[1])
        .AppendViews("db", &["t2"]);
    let query = "SELECT TABLE_NAME,TABLE_TYPE,AVG_ROW_LENGTH FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA='db'";
    conn.seed_query(
        query,
        vec![
            "TABLE_NAME".into(),
            "TABLE_TYPE".into(),
            "AVG_ROW_LENGTH".into(),
        ],
        vec![
            vec![
                Some(b"t1".to_vec()),
                Some(b"BASE TABLE".to_vec()),
                Some(b"1".to_vec()),
            ],
            vec![Some(b"t2".to_vec()), Some(b"VIEW".to_vec()), None],
        ],
    );
    let tables = ListAllDatabasesTables(
        &tctx,
        &conn,
        &["db".into()],
        listTableType::listTableByInfoSchema,
        &[TableType::TableTypeBase, TableType::TableTypeView],
    )
    .unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables["db"].len(), 2);
}

#[test]
fn test_list_all_tables_by_table_status() {
    // 这一组走 SHOW TABLE STATUS 路径，主要锁住“只要能列出行，就能收集到表名”的契约。
    // 它对应某些实例上 information_schema 不适合作为首选来源的兼容分支。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut data = NewDatabaseTables();
    data.AppendTables("db1", &["t1".into(), "t2".into()], &[1, 2])
        .AppendTables("db2", &["t3".into(), "t4".into(), "t5".into()], &[3, 4, 5]);
    // fake rows 只补 prepare 逻辑真正会读取的列位，其他位置保持空值即可。
    // 这样可以明确说明测试关注点仅是表名和结果行数，而不是所有 status 字段。
    for db_name in data.keys() {
        let query = format!("SHOW TABLE STATUS FROM `{db_name}`");
        let mut rows = Vec::new();
        for tb in &data[db_name] {
            let mut row = vec![None; 18];
            row[0] = Some(tb.Name.as_bytes().to_vec());
            row[1] = Some(b"InnoDB".to_vec());
            rows.push(row);
        }
        conn.seed_query(&query, (0..18).map(|i| format!("c{i}")).collect(), rows);
    }
    let db_names: Vec<String> = data.keys().cloned().collect();
    let tables = ListAllDatabasesTables(
        &tctx,
        &conn,
        &db_names,
        listTableType::listTableByShowTableStatus,
        &[TableType::TableTypeBase],
    )
    .unwrap();
    // 每个库返回的表数应与预设数据一致，表明 status 路径没有漏表。
    // 由于 SHOW TABLE STATUS 不返回 view，这里只构造 base table 数据。
    for (d, table) in &tables {
        assert_eq!(table.len(), data[d].len());
    }
}

#[test]
fn test_list_all_tables_by_show_full_tables() {
    // SHOW FULL TABLES 路径用于某些兼容分支，这里锁住 base/view 同时返回的行为。
    // 该语句直接给出类型列，因此适合验证 TableType 解析分支。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let query = "SHOW FULL TABLES FROM `db`";
    conn.seed_query(
        query,
        vec!["Tables_in_db".into(), "Table_type".into()],
        vec![
            vec![Some(b"t1".to_vec()), Some(b"BASE TABLE".to_vec())],
            vec![Some(b"v1".to_vec()), Some(b"VIEW".to_vec())],
        ],
    );
    let tables = ListAllDatabasesTables(
        &tctx,
        &conn,
        &["db".into()],
        listTableType::listTableByShowFullTables,
        &[TableType::TableTypeBase, TableType::TableTypeView],
    )
    .unwrap();
    // 只要类型过滤同时允许两类对象，就应该完整保留两行。
    // 如果将来某个分支误把 VIEW 过滤掉，这里会立即暴露回归。
    assert_eq!(tables["db"].len(), 2);
}

#[test]
fn test_config_validation() {
    // validateSpecifiedSQL 的核心约束是：
    // 一旦使用原始 SQL，就不能再叠加 where/partitions 这类二次改写条件。
    // 这保证导出 SQL 的最终语义只来自一个入口，避免条件重复拼接。
    let mut conf = default_config_for_test();
    conf.SQL = "select 1".into();
    conf.Where = "a>1".into();
    assert!(validateSpecifiedSQL(&conf).is_err());
    conf.Where.clear();
    conf.Partitions = vec!["p0".into()];
    assert!(validateSpecifiedSQL(&conf).is_err());
    conf.Partitions.clear();
    // 清掉冲突项后恢复为合法配置。
    // 这里也顺带证明验证器不会对“只有 SQL”这一合法形态误报错误。
    assert!(validateSpecifiedSQL(&conf).is_ok());
}

#[test]
fn test_validate_resolve_auto_consistency() {
    // 这里不走完整 Dumper 初始化，而是手工拼出最小对象来验证 consistency 判定。
    // 第一组：MySQL 上请求 snapshot 应失败。
    // 这样能避开 DB/http/storage 等初始化副作用，把断言聚焦在 server type 判断。
    let mut d = Dumper {
        tctx: tcontext::Background().WithLogger(app_logger()),
        conf: std::sync::Arc::new({
            let mut c = default_config_for_test();
            c.Consistency = ConsistencyTypeSnapshot.to_string();
            c.ServerInfo.ServerType = ServerType::ServerTypeMySQL;
            c
        }),
        db: None,
        ext_storage: None,
        metrics: newMetrics(NewDefaultFactory().as_ref(), &Labels::default()),
        speedRecorder: std::sync::Mutex::new(NewSpeedRecorder()),
        totalTables: std::sync::atomic::AtomicI64::new(0),
        cancel: None,
        http: None,
        pd_client: None,
    };
    assert!(validateResolveAutoConsistency(&mut d).is_err());
    // 第二组：切成 TiDB 后，同样的配置应被接受。
    // 这条断言锁住 auto/snapshot 解析和 TiDB 兼容策略的交汇点。
    let mut conf = (*d.conf).clone_for_mutate();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    d.conf = std::sync::Arc::new(conf);
    assert!(validateResolveAutoConsistency(&mut d).is_ok());
}
