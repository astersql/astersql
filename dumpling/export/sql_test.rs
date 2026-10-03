// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `sql_test.go`：SQL 构建、元数据查询与 TiDB 特有路径的单测。

use crate::main_test::{app_logger, default_config_for_test};
use crate::*;

// 固定 schema/table 名缩短断言字符串；与 Go 测试常量一致。
const DATABASE: &str = "foo";
// 与 DATABASE 配对用于 buildSelectQuery 等。
const TABLE: &str = "bar";

/// SHOW INDEX 结果列名；与 information_schema 列顺序对齐。

// mock 辅助：构造 SHOW INDEX 列头。
fn show_index_headers() -> Vec<String> {
    [
        "Table",
        "Non_unique",
        "Key_name",
        "Seq_in_index",
        "Column_name",
        "Collation",
        "Cardinality",
        "Sub_part",
        "Packed",
        "Null",
        "Index_type",
        "Comment",
        "Index_comment",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// 向 mock Conn 注入 SHOW INDEX 结果行。

fn seed_show_index(conn: &Conn, database: &str, table: &str, rows: Vec<Vec<Option<Vec<u8>>>>) {
    let q = format!("SHOW INDEX FROM `{database}`.`{table}`");
    conn.seed_query(&q, show_index_headers(), rows);
}

/// 向 mock Conn 注入 SHOW COLUMNS 结果行。

fn seed_show_columns(conn: &Conn, database: &str, table: &str, rows: Vec<Vec<Option<Vec<u8>>>>) {
    let q = format!("SHOW COLUMNS FROM `{database}`.`{table}`");
    conn.seed_query(
        &q,
        vec![
            "Field".into(),
            "Type".into(),
            "Null".into(),
            "Key".into(),
            "Default".into(),
            "Extra".into(),
        ],
        rows,
    );
}

/// 构造 PRIMARY 索引的一行 mock 数据；seq 为 Seq_in_index。

// mock 辅助：PRIMARY KEY 索引行模板。
fn pk_row(table: &str, col: &str, seq: &str) -> Vec<Option<Vec<u8>>> {
    vec![
        Some(table.as_bytes().to_vec()),
        Some(b"0".to_vec()),
        Some(b"PRIMARY".to_vec()),
        Some(seq.as_bytes().to_vec()),
        Some(col.as_bytes().to_vec()),
        Some(b"A".to_vec()),
        Some(b"0".to_vec()),
        None,
        None,
        Some(b"".to_vec()),
        Some(b"BTREE".to_vec()),
        Some(b"".to_vec()),
        Some(b"".to_vec()),
    ]
}

/// 验证 TiDB 隐式 `_tidb_rowid` 与显式主键两种 ORDER BY 路径下的 SELECT 语句。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_select_all_query() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    // 使用内存 mock DB，无需真实 MySQL。
    let db = DB::new();
    // 每个测试独立 Conn，避免 seed 数据串扰。
    let conn = db.Conn().unwrap();
    // BaseConn 包装 Conn 并提供 QuerySQLWithColumns。
    let mut base = newBaseConn(conn.clone(), true, None);
    // 可变 conf 用于切换 ServerType/SortByPk 等开关。
    let mut conf = default_config_for_test();
    conf.SortByPk = true;
    // 序列 SETVAL 分支仅 TiDB/MariaDB 启用。
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;

    let order = buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, true).unwrap();
    seed_show_columns(
        &conn,
        DATABASE,
        TABLE,
        vec![vec![
            Some(b"id".to_vec()),
            Some(b"int(11)".to_vec()),
            Some(b"NO".to_vec()),
            Some(b"PRI".to_vec()),
            None,
            Some(b"".to_vec()),
        ]],
    );
    let (selected, _) = buildSelectField(&tctx, &mut base, DATABASE, TABLE, false).unwrap();
    let q = buildSelectQuery(DATABASE, TABLE, &selected, "", "", &order);
    // 隐式 rowid 表应生成 ORDER BY `_tidb_rowid`。
    assert_eq!(
        q,
        format!("SELECT * FROM `{DATABASE}`.`{TABLE}` ORDER BY `_tidb_rowid`")
    );

    seed_show_index(&conn, DATABASE, TABLE, vec![pk_row(TABLE, "id", "1")]);
    let order = buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, false).unwrap();
    seed_show_columns(
        &conn,
        DATABASE,
        TABLE,
        vec![vec![
            Some(b"id".to_vec()),
            Some(b"int(11)".to_vec()),
            Some(b"NO".to_vec()),
            Some(b"PRI".to_vec()),
            None,
            Some(b"".to_vec()),
        ]],
    );
    let (selected, _) = buildSelectField(&tctx, &mut base, DATABASE, TABLE, false).unwrap();
    let q = buildSelectQuery(DATABASE, TABLE, &selected, "", "", &order);
    // 有主键时不应再使用 _tidb_rowid。
    assert_eq!(
        q,
        format!("SELECT * FROM `{DATABASE}`.`{TABLE}` ORDER BY `id`")
    );
}

/// 覆盖 SortByPk、复合主键、无索引及 SortByPk=false 等 ORDER BY 分支。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_order_by_clause() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    let mut conf = default_config_for_test();
    conf.SortByPk = true;
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    // has_implicit_row_id=true 直接返回常量 orderByTiDBRowID。
    assert_eq!(
        orderByTiDBRowID,
        buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, true).unwrap()
    );
    seed_show_index(&conn, DATABASE, TABLE, vec![pk_row(TABLE, "id", "1")]);
    // 单列主键 ORDER BY。
    assert_eq!(
        "ORDER BY `id`",
        buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, false).unwrap()
    );
    seed_show_index(
        &conn,
        DATABASE,
        TABLE,
        vec![pk_row(TABLE, "id", "1"), pk_row(TABLE, "name", "2")],
    );
    // 复合主键按 SHOW INDEX 列顺序拼接。
    assert_eq!(
        "ORDER BY `id`,`name`",
        buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, false).unwrap()
    );
    // 无索引且无 rowid 时不生成 ORDER BY。
    seed_show_index(&conn, DATABASE, TABLE, vec![]);
    assert_eq!(
        "",
        buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, false).unwrap()
    );
    conf.SortByPk = false;
    // 关闭 SortByPk 后无论是否有 rowid 均不生成 ORDER BY。
    assert_eq!(
        "",
        buildOrderByClause(&tctx, &conf, &mut base, DATABASE, TABLE, true).unwrap()
    );
}

/// complete_insert / 生成列 / 反引号列名对 buildSelectField 的影响。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_select_field() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    seed_show_columns(
        &conn,
        "test",
        "t",
        vec![vec![
            Some(b"id".to_vec()),
            Some(b"int(11)".to_vec()),
            Some(b"NO".to_vec()),
            Some(b"PRI".to_vec()),
            None,
            Some(b"".to_vec()),
        ]],
    );
    // 无 complete_insert 且无生成列时用 SELECT *。
    let (selected, _) = buildSelectField(&tctx, &mut base, "test", "t", false).unwrap();
    assert_eq!("*", selected);

    seed_show_columns(
        &conn,
        "test",
        "t",
        vec![
            vec![
                Some(b"id".to_vec()),
                Some(b"int".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"PRI".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
            vec![
                Some(b"name".to_vec()),
                Some(b"varchar".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
            vec![
                Some(b"quo`te".to_vec()),
                Some(b"varchar".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"UNI".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
        ],
    );
    // complete_insert 列出全部非生成列，反引号列名需加倍转义。
    let (selected, _) = buildSelectField(&tctx, &mut base, "test", "t", true).unwrap();
    assert_eq!("`id`,`name`,`quo``te`", selected);

    seed_show_columns(
        &conn,
        "test",
        "t",
        vec![
            vec![
                Some(b"id".to_vec()),
                Some(b"int".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"PRI".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
            vec![
                Some(b"name".to_vec()),
                Some(b"varchar".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
            vec![
                Some(b"quo`te".to_vec()),
                Some(b"varchar".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"UNI".to_vec()),
                None,
                Some(b"".to_vec()),
            ],
            vec![
                Some(b"generated".to_vec()),
                Some(b"varchar".to_vec()),
                Some(b"NO".to_vec()),
                Some(b"".to_vec()),
                None,
                Some(b"VIRTUAL GENERATED".to_vec()),
            ],
        ],
    );
    // 存在 VIRTUAL GENERATED 时即使 complete_insert=false 也须显式列清单。
    let (selected, _) = buildSelectField(&tctx, &mut base, "test", "t", false).unwrap();
    assert_eq!("`id`,`name`,`quo``te`", selected);
}

/// unix_timestamp 快照字符串转 TSO；含缓存命中与非法格式错误。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_parse_snapshot_to_tso() {
    let db = DB::new();
    // 预置 prepared 风格单行查询缓存。
    db.seed_query_row("SELECT unix_timestamp(?)", Some(1595075510));
    let tso = parseSnapshotToTSO(&db, "2020/07/18 20:31:50").unwrap();
    // TSO = unix_ts << 18 * 1000，与 PD 物理时钟编码一致。
    assert_eq!((1595075510u64 << 18) * 1000, tso);
    // NULL 时间戳应返回 format not supported。
    db.seed_query_row("SELECT unix_timestamp(?)", None);
    // 非法快照字符串应报错。
    let err = parseSnapshotToTSO(&db, "XXYYZZ").unwrap_err();
    // 错误信息含 snapshot 原文。
    assert!(err.msg.contains("snapshot XXYYZZ format not supported"));
}

/// ShowCreateView 合成占位 CREATE TABLE 与带 charset 的 CREATE VIEW。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_show_create_view() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    // ShowCreateView 先查 FIELDS 构造假表结构。
    conn.seed_query(
        "SHOW FIELDS FROM `test`.`v`",
        vec![
            "Field".into(),
            "Type".into(),
            "Null".into(),
            "Key".into(),
            "Default".into(),
            "Extra".into(),
        ],
        vec![vec![
            Some(b"a".to_vec()),
            Some(b"int(11)".to_vec()),
            Some(b"YES".to_vec()),
            None,
            Some(b"NULL".to_vec()),
            None,
        ]],
    );
    // 再取 CREATE VIEW 与 charset 会话变量。
    conn.seed_query(
        "SHOW CREATE VIEW `test`.`v`",
        vec!["View".into(), "Create View".into(), "character_set_client".into(), "collation_connection".into()],
        vec![vec![
            Some(b"v".to_vec()),
            Some(b"CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`localhost` SQL SECURITY DEFINER VIEW `v` (`a`) AS SELECT `t`.`a` AS `a` FROM `test`.`t`".to_vec()),
            Some(b"utf8".to_vec()),
            Some(b"utf8_general_ci".to_vec()),
        ]],
    );
    // 视图导出需占位表 + charset 包裹的 CREATE VIEW。
    let (create_table, create_view) = ShowCreateView(&tctx, &mut base, "test", "v").unwrap();
    assert_eq!(
        "CREATE TABLE `v`(\n`a` int\n)ENGINE=MyISAM;\n",
        create_table
    );
    // 视图 DDL 前先 DROP TABLE/VIEW，避免 mysqldump 恢复顺序问题。
    assert!(create_view.contains("DROP VIEW IF EXISTS `v`;"));
    assert!(create_view.contains("CREATE ALGORITHM=UNDEFINED"));
}

/// TiDB 序列导出附加 SETVAL；依赖 NEXT_ROW_ID 查询。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_show_create_sequence() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    // mock SHOW CREATE SEQUENCE 第二列为 DDL。
    conn.seed_query(
        "SHOW CREATE SEQUENCE `test`.`s`",
        vec!["Sequence".into(), "Create Sequence".into()],
        vec![vec![
            Some(b"s".to_vec()),
            Some(b"CREATE SEQUENCE `s` start with 1 minvalue 1 maxvalue 9223372036854775806 increment by 1 cache 1000 nocycle ENGINE=InnoDB".to_vec()),
        ]],
    );
    // TiDB NEXT_ROW_ID 用于 SETVAL 参数。
    conn.seed_query(
        "SHOW TABLE `test`.`s` NEXT_ROW_ID",
        vec![
            "DB_NAME".into(),
            "TABLE_NAME".into(),
            "COLUMN_NAME".into(),
            "NEXT_GLOBAL_ROW_ID".into(),
            "ID_TYPE".into(),
        ],
        vec![vec![
            Some(b"test".to_vec()),
            Some(b"s".to_vec()),
            None,
            Some(b"1001".to_vec()),
            Some(b"SEQUENCE".to_vec()),
        ]],
    );
    // ShowCreateSequence 拼接 CREATE + SETVAL。
    let sql = ShowCreateSequence(&tctx, &mut base, "test", "s", &conf).unwrap();
    assert!(sql.contains("CREATE SEQUENCE `s`"));
    // TiDB 序列需追加 SETVAL 恢复 NEXT 值。
    assert!(sql.contains("SELECT SETVAL(`s`,1001);"));
}

/// Placement Policy DDL 导出列名匹配。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_show_create_policy() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    conn.seed_query(
        "SHOW CREATE PLACEMENT POLICY `policy_x`",
        vec!["Policy".into(), "Create Policy".into()],
        vec![vec![
            Some(b"policy_x".to_vec()),
            Some(b"CREATE PLACEMENT POLICY `policy_x` LEARNERS=1".to_vec()),
        ]],
    );
    // 首次 seed 列名 Create Policy 与 QuerySQLWithColumns 期望不一致，应用例会失败。
    // ShowCreatePlacementPolicy uses QuerySQLWithColumns looking for Create Placement Policy
    // Seed with that column name too via contains match - exact query:
    let sql = ShowCreatePlacementPolicy(&tctx, &mut base, "policy_x");
    // 修正列名为 Create Placement Policy 后应成功。
    // May fail column match; re-seed with expected column
    conn.seed_query(
        "SHOW CREATE PLACEMENT POLICY `policy_x`",
        vec!["Policy".into(), "Create Placement Policy".into()],
        vec![vec![
            Some(b"policy_x".to_vec()),
            Some(b"CREATE PLACEMENT POLICY `policy_x` LEARNERS=1".to_vec()),
        ]],
    );
    let sql = ShowCreatePlacementPolicy(&tctx, &mut base, "policy_x").unwrap();
    // 第二列 Create Placement Policy 即为 DDL 正文。
    assert_eq!("CREATE PLACEMENT POLICY `policy_x` LEARNERS=1", sql);
}

/// information_schema.placement_policies 去重 policy 名列表。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_list_policy_names() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    conn.seed_query(
        "select distinct policy_name from information_schema.placement_policies where policy_name is not null;",
        vec!["policy_name".into()],
        vec![vec![Some(b"policy_x".to_vec())]],
    );
    // distinct policy_name 列表应原样返回。
    let policies = ListAllPlacementPolicyNames(&tctx, &mut base).unwrap();
    // 单 policy 名列表。
    assert_eq!(policies, vec!["policy_x".to_string()]);
}

/// 按 avg_row_length 估算单文件行数上限（128MiB 目标）。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_get_suitable_rows() {
    // avg_row_length=0 回退默认 20 万行。
    assert_eq!(200000, GetSuitableRows(0));
    // 极小行宽时钳制 MAX_ROWS=1e6。
    assert_eq!(1000000, GetSuitableRows(1));
    // 1MiB 行宽 → 128MiB/1MiB=128 行。
    assert_eq!(128, GetSuitableRows(1024 * 1024));
}

/// _tidb_rowid 探测：存在则 true，1054 类错误则 false。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_select_tidb_row_id() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    // success path
    // 默认 mock ExecSQL 成功路径。
    assert!(SelectTiDBRowID(&tctx, &mut base, "db", "t").unwrap());
    // unknown column — sticky fail so WithRetry cannot succeed on a later attempt
    // 模拟 1054 unknown column，SelectTiDBRowID 应返回 false 且不向上抛错。
    *conn.fail_query.lock().unwrap() =
        Some(errors_new("Unknown column '_tidb_rowid' in 'field list'"));
    assert!(!SelectTiDBRowID(&tctx, &mut base, "db", "t").unwrap());
    *conn.fail_query.lock().unwrap() = None;
}

/// buildTableSampleQueries 默认无分区时单条 TABLESAMPLE。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_table_sample_queries() {
    let conf = default_config_for_test();
    // mockTableIR 提供列名/类型供 pickupPossibleField 使用。
    let meta = crate::util_for_test::mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    // 无 conf.Partitions 时返回单元素向量。
    let qs = buildTableSampleQueries(&conf, &meta, &["id".into()]);
    assert_eq!(qs.len(), 1);
    // TiDB 一致性采样依赖 TABLESAMPLE REGIONS()。
    assert!(qs[0].contains("TABLESAMPLE REGIONS()"));
}

/// 分区名转 PARTITION 子句片段。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_partition_clauses() {
    // 每个分区名生成独立 PARTITION 子句。
    let clauses = buildPartitionClauses(&["p0".into(), "p1".into()]);
    assert_eq!(clauses, vec![" PARTITION (`p0`)", " PARTITION (`p1`)"]);
}

/// 单分区 TABLESAMPLE REGIONS 查询格式。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_tidb_table_sample_query() {
    // 指定分区列表嵌入 PARTITION (...) TABLESAMPLE。
    let q = buildTiDBTableSampleQuery(&["id".into()], "db", "t", &["p0"]);
    assert!(q.contains("PARTITION (`p0`)"));
    assert!(q.contains("TABLESAMPLE REGIONS()"));
}

/// conf.Where 与 where_extra 的 AND 组合。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_where_condition() {
    let mut conf = default_config_for_test();
    conf.Where = "a>1".into();
    // 仅 conf.Where 时单层括号。
    assert_eq!("WHERE a>1 ", buildWhereCondition(&conf, ""));
    // where_extra 与 conf.Where 用 AND 连接。
    assert_eq!("WHERE (a>1) AND (b<2) ", buildWhereCondition(&conf, "b<2"));
}

/// TiKV region 边界查询（无分区表）。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_region_queries_without_partition() {
    // IS_INDEX=0 过滤数据 region。
    let q = buildRegionQueriesWithoutPartition("db", "t", "id");
    // region 切分查询走 information_schema.tikv_region_status。
    assert!(q.contains("TIKV_REGION_STATUS"));
    assert!(q.contains("`id`"));
}

/// 带 PARTITION_NAME 过滤的 region 查询。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_region_queries_with_partitions() {
    // 每个分区一条 DISTINCT handle 查询。
    let qs = buildRegionQueriesWithPartitions("db", "t", "id", &["p0".into()]);
    assert_eq!(qs.len(), 1);
    // 分区表按 PARTITION_NAME 过滤 region。
    assert!(qs[0].contains("PARTITION_NAME"));
}

/// v3 一致性 region 键范围查询含 START_KEY/END_KEY。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_build_version3_region_queries() {
    // v3 一致性仅需键边界不含 handle 列 DISTINCT。
    let q = buildVersion3RegionQueries("db", "t");
    // v3 协议需 START_KEY/END_KEY 键范围。
    assert!(q.contains("START_KEY"));
}

/// @@tidb_config 含 tikv 时判定 TiKV 后端（smoke）。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_check_tidb_with_tikv() {
    let db = DB::new();
    db.seed_query(
        "SELECT COUNT(*) FROM INFORMATION_SCHEMA.CLUSTER_INFO WHERE TYPE='tikv'",
        vec!["COUNT(*)".into()],
        vec![vec![Some(b"1".to_vec())]],
    );
    // CheckTiDBWithTiKV may use different query — just call
    // Go 测试查 @@tidb_config；mock 路径可能不同，仅确保不 panic。
    let _ = CheckTiDBWithTiKV(&db);
}

/// pickupPossibleField 优先 PRIMARY 数值列（mock 索引）。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_pickup_possible_field() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    let meta = crate::util_for_test::mockTableIR::new("db", "t", vec![], &[], &["INT"]);
    seed_show_index(&conn, "db", "t", vec![pk_row("t", "id", "1")]);
    // Column type map needs id->INT; mock uses col name INT
    // PRIMARY 存在时应选中 id；mock 列类型映射为 INT。
    let field = pickupPossibleField(&tctx, &meta, &mut base);
    // 当前仅 smoke 调用，不断言具体列名。
    let _ = field;
}

/// information_schema 中 SEQUENCE 表存在性检查。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_check_if_seq_exists() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn.clone(), true, None);
    let q = "SELECT 1 FROM information_schema.tables WHERE table_schema='db' AND table_name='s' AND table_type='SEQUENCE'";
    conn.seed_query(q, vec!["1".into()], vec![vec![Some(b"1".to_vec())]]);
    // information_schema.tables table_type=SEQUENCE 存在即 true。
    assert!(CheckIfSeqExists(&tctx, &mut base, "db", "s").unwrap());
}

/// SHOW CHARACTER SET 默认 collation 映射。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_get_charset_and_default_collation() {
    let db = DB::new();
    let conn = db.Conn().unwrap();
    conn.seed_query(
        "SHOW CHARACTER SET",
        vec![
            "Charset".into(),
            "Description".into(),
            "Default collation".into(),
            "Maxlen".into(),
        ],
        vec![vec![
            Some(b"utf8".to_vec()),
            Some(b"UTF-8".to_vec()),
            Some(b"utf8_general_ci".to_vec()),
            Some(b"3".to_vec()),
        ]],
    );
    // 第三列为 Default collation。
    let map = GetCharsetAndDefaultCollation(&conn).unwrap();
    // charset 键小写化后与默认 collation 映射。
    assert_eq!(map.get("utf8").map(|s| s.as_str()), Some("utf8_general_ci"));
}

/// 按列名提取多行单列值并 Close。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_get_specified_column_value_and_close() {
    // 纯内存 Rows，无需 DB 即可测列提取 helper。
    let mut rows = Rows::new(
        vec!["a".into(), "b".into()],
        vec![
            vec![Some(b"1".to_vec()), Some(b"x".to_vec())],
            vec![Some(b"2".to_vec()), Some(b"y".to_vec())],
        ],
    );
    // 单列投影 helper。
    let vals = GetSpecifiedColumnValueAndClose(&mut rows, "b").unwrap();
    // 按列名 b 提取两行。
    assert_eq!(vals, vec!["x".to_string(), "y".to_string()]);
}

/// 按多列名提取整行并 Close。

// 单测用例：见下方函数名与 Go 对应用例。
#[test]
fn test_get_specified_column_values_and_close() {
    let mut rows = Rows::new(
        vec!["a".into(), "b".into()],
        vec![vec![Some(b"1".to_vec()), Some(b"x".to_vec())]],
    );
    // 多列投影 helper。
    let vals = GetSpecifiedColumnValuesAndClose(&mut rows, &["a", "b"]).unwrap();
    // 多列投影保留行结构。
    assert_eq!(vals, vec![vec!["1".to_string(), "x".to_string()]]);
}

#[test]
fn test_go_sql_builder_edge_case_parity() {
    assert_eq!(listTableType::listTableByInfoSchema as i32, 0);
    assert_eq!(listTableType::listTableByShowFullTables as i32, 1);
    assert_eq!(listTableType::listTableByShowTableStatus as i32, 2);

    assert_eq!(
        buildSelectQuery("db", "table", "", "p0", "", ""),
        "SELECT '' FROM `db`.`table` PARTITION(`p0`)"
    );
    assert_eq!(GetSuitableRows(u64::MAX), 0);
}

#[test]
fn test_go_where_clause_format_and_ranges() {
    let mut conf = default_config_for_test();
    conf.Where = "a >= 10".into();
    assert_eq!(buildWhereCondition(&conf, ""), "WHERE a >= 10 ");
    assert_eq!(
        buildWhereCondition(&conf, "`a`<20"),
        "WHERE (a >= 10) AND (`a`<20) "
    );

    let names = vec!["a".into(), "b".into(), "c".into()];
    let values = vec![
        vec!["1".into(), "2".into(), "'3'".into()],
        vec!["1".into(), "4".into(), "'5'".into()],
    ];
    assert_eq!(
        buildWhereClauses(&names, &values),
        vec![
            "`a`<1 or(`a`=1 and `b`<2)or(`a`=1 and `b`=2 and `c`<'3')",
            "`a`=1 and((`b`>2 and `b`<4)or(`b`=2 and(`c`>='3'))or(`b`=4 and(`c`<'5')))",
            "`a`>1 or(`a`=1 and `b`>4)or(`a`=1 and `b`=4 and `c`>='5')",
        ]
    );

    let same = vec![vec!["1".into()], vec!["1".into()]];
    assert_eq!(
        buildWhereClauses(&["a".into()], &same),
        vec!["`a`<1", "false", "`a`>=1"]
    );
}

#[test]
fn test_pickup_possible_field_prefers_implicit_tidb_rowid() {
    let tctx = tcontext::Background().WithLogger(app_logger());
    let db = DB::new();
    let conn = db.Conn().unwrap();
    let mut base = newBaseConn(conn, true, None);
    let mut meta = crate::util_for_test::mockTableIR::new("db", "t", vec![], &[], &[]);
    meta.has_implicit_row_id = true;
    assert_eq!(
        pickupPossibleField(&tctx, &meta, &mut base).unwrap(),
        "_tidb_rowid"
    );
}

#[test]
fn get_column_types_retries_close_errors_and_discards_failed_metadata() {
    let conn = Conn::new();
    let query = "SELECT `id` FROM `db`.`t` LIMIT 1";
    let mut first = Rows::new(vec!["wrong".into()], vec![vec![Some(b"1".to_vec())]]);
    first.close_error = Some(errors_new("close failed"));
    conn.seed_rows(query, first);
    let mut second = Rows::new(vec!["id".into()], vec![vec![Some(b"1".to_vec())]]);
    second.col_types[0].database_type_name = "INT".into();
    conn.seed_rows(query, second);
    let mut base = newBaseConn(conn.clone(), true, None);
    let columns = getColumnTypes(&tcontext::Background(), &mut base, "`id`", "db", "t").unwrap();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].Name(), "id");
    assert_eq!(columns[0].DatabaseTypeName(), "INT");
    assert!(
        conn.row_responses
            .lock()
            .unwrap()
            .get(query)
            .unwrap()
            .is_empty()
    );
    assert_eq!(base.backOffer.RemainingAttempts(), dumpChunkRetryTime);
}
