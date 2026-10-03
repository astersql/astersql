// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// Dumpling 导出用 SQL 辅助：元数据查询、SELECT/WHERE 构建、TiDB 一致性 region 采样。
//
// 对应 Go `sql.go`；命名与 Go 导出符号一致以便对照。涵盖 SHOW/INFORMATION_SCHEMA、
// 主键排序、快照 TSO、chunk WHERE lexicographic 边界及 FLUSH/LOCK 等会话语句。
// 本模块不执行导出写盘，仅生成/执行探测 SQL 供 dump 与 consistency 子系统调用。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 列举库表的三种策略；与 Go `listTableType` 枚举值一致。
pub enum listTableType {
    // 查 information_schema.tables。
    listTableByInfoSchema = 0,
    // 查 SHOW TABLE STATUS。
    listTableByShowTableStatus = 2,
    // 查 SHOW FULL TABLES。
    listTableByShowFullTables = 1,
}

/// `SHOW DATABASES` 返回库名列表；Go `ShowDatabases`。
pub fn ShowDatabases(db: &Conn) -> Result<Vec<String>> {
    // 带 context 的查询。
    let mut rows = db.QueryContext("SHOW DATABASES")?;
    let mut out = Vec::new();
    // 逐行读取结果集。
    while rows.Next() {
        let mut dest = [RawBytes(None)];
        // 扫描当前行到 RawBytes 缓冲区。
        rows.Scan(&mut dest)?;
        // 首列 PD 地址。
        if let Some(b) = dest[0].as_opt() {
            // 字节列转 UTF-8 字符串（有损）。
            out.push(String::from_utf8_lossy(b).to_string());
        }
    }
    // 释放结果集；Go defer Close 等价。
    // 关闭 Rows 释放资源。
    rows.Close()?;
    // 返回拼接结果。
    Ok(out)
}

/// 当前库 `SHOW TABLES`；未指定 schema 时用连接默认库。
pub fn ShowTables(db: &Conn) -> Result<Vec<String>> {
    let mut rows = db.QueryContext("SHOW TABLES")?;
    let mut out = Vec::new();
    while rows.Next() {
        let mut dest = [RawBytes(None)];
        rows.Scan(&mut dest)?;
        if let Some(b) = dest[0].as_opt() {
            out.push(String::from_utf8_lossy(b).to_string());
        }
    }
    rows.Close()?;
    Ok(out)
}

/// 导出库 DDL；列名 `Create Database`。
pub fn ShowCreateDatabase(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
) -> Result<String> {
    // MySQL 标识符反引号 quoting。
    let query = format!("SHOW CREATE DATABASE {}", wrapBackTicks(database));
    // 按列名投影为 Vec<Vec<String>>。
    let results = db.QuerySQLWithColumns(tctx, &["Create Database"], &query)?;
    // 简单错误构造。
    results
        .first()
        .and_then(|r| r.first())
        .cloned()
        .ok_or_else(|| errors_new("no create database sql"))
}

/// 导出表 DDL；`database.table` 均反引号包裹。
pub fn ShowCreateTable(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    table: &str,
) -> Result<String> {
    let query = format!(
        "SHOW CREATE TABLE {}.{}",
        wrapBackTicks(database),
        wrapBackTicks(table)
    );
    let results = db.QuerySQLWithColumns(tctx, &["Create Table"], &query)?;
    results
        .first()
        .and_then(|r| r.first())
        .cloned()
        .ok_or_else(|| errors_new("no create table sql"))
}

/// TiDB Placement Policy DDL；列 `Create Placement Policy`。
pub fn ShowCreatePlacementPolicy(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    policy: &str,
) -> Result<String> {
    let query = format!("SHOW CREATE PLACEMENT POLICY {}", wrapBackTicks(policy));
    let results = db.QuerySQLWithColumns(tctx, &["Create Placement Policy"], &query)?;
    results
        .first()
        .and_then(|r| r.first())
        .cloned()
        .ok_or_else(|| errors_new("no create policy sql"))
}

/// 视图导出：合成占位 CREATE TABLE + charset 包裹的 CREATE VIEW。
pub fn ShowCreateView(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    view: &str,
) -> Result<(String, String)> {
    let query = format!(
        "SHOW FIELDS FROM `{}`.`{}`",
        // 字符串字面量 escape（非 identifier）。
        escapeString(database),
        escapeString(view)
    );
    let field_rows = db.QuerySQLWithColumns(tctx, &["Field"], &query)?;
    let mut field_names = Vec::new();
    for row in field_rows {
        if let Some(name) = row.first() {
            // 占位 CREATE TABLE 字段统一 int。
            field_names.push(format!("`{}` int", escapeString(name)));
        }
    }
    let mut create_table = format!(
        "CREATE TABLE `{}`(
",
        escapeString(view)
    );
    create_table.push_str(&field_names.join(
        ",
",
    ));
    create_table.push_str(
        "
)ENGINE=MyISAM;
",
    );

    let query = format!(
        "SHOW CREATE VIEW `{}`.`{}`",
        escapeString(database),
        escapeString(view)
    );
    let results = db.QuerySQLWithColumns(
        tctx,
        &[
            "View",
            "Create View",
            "character_set_client",
            "collation_connection",
        ],
        &query,
    )?;
    let row = results
        .first()
        .ok_or_else(|| errors_new("no create view sql"))?;
    let mut create_view = String::new();
    create_view.push_str(&format!(
        "DROP TABLE IF EXISTS `{}`;
",
        escapeString(view)
    ));
    create_view.push_str(&format!(
        "DROP VIEW IF EXISTS `{}`;
",
        escapeString(view)
    ));
    // CREATE VIEW 前切换导出 charset。
    SetCharset(
        &mut create_view,
        row.get(2).map(|s| s.as_str()).unwrap_or(""),
        row.get(3).map(|s| s.as_str()).unwrap_or(""),
    );
    create_view.push_str(row.get(1).map(|s| s.as_str()).unwrap_or(""));
    create_view.push_str(
        ";
",
    );
    // DDL 后恢复会话 charset。
    RestoreCharset(&mut create_view);
    Ok((create_table, create_view))
}

/// 序列 DDL；TiDB/MariaDB 附加 SETVAL 恢复 NEXT 值。
pub fn ShowCreateSequence(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    sequence: &str,
    conf: &Config,
) -> Result<String> {
    let query = format!(
        "SHOW CREATE SEQUENCE `{}`.`{}`",
        escapeString(database),
        escapeString(sequence)
    );
    let mut rows = db.DBConn.as_ref().unwrap().QueryContext(&query)?;
    let cols = rows.Columns().unwrap_or_default();
    let mut create_sql = String::new();
    if rows.Next() {
        let mut dest = vec![RawBytes(None); cols.len().max(2)];
        rows.Scan(&mut dest)?;
        create_sql = String::from_utf8_lossy(dest.get(1).and_then(|d| d.as_opt()).unwrap_or(b""))
            .to_string();
        // Scan 第二列空则回退第一列。
        if create_sql.is_empty() {
            create_sql =
                String::from_utf8_lossy(dest.first().and_then(|d| d.as_opt()).unwrap_or(b""))
                    .to_string();
        }
    }
    rows.Close()?;
    if create_sql.is_empty() {
        // 错误上抛路径。
        return Err(errors_new("no create sequence sql"));
    }
    let mut out = String::new();
    out.push_str(&create_sql);
    out.push_str(
        ";
",
    );
    match conf.ServerInfo.ServerType {
        // TiDB：SHOW TABLE NEXT_ROW_ID + SETVAL。
        ServerType::ServerTypeTiDB => {
            let q = format!(
                "SHOW TABLE `{}`.`{}` NEXT_ROW_ID",
                escapeString(database),
                escapeString(sequence)
            );
            let rows = db.QuerySQLWithColumns(tctx, &["NEXT_GLOBAL_ROW_ID", "ID_TYPE"], &q)?;
            let mut next_val: i64 = 0;
            for row in rows {
                if row.get(1).map(|s| s.as_str()) == Some("SEQUENCE") {
                    next_val = row.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                }
            }
            out.push_str(&format!(
                "SELECT SETVAL(`{}`,{});
",
                escapeString(sequence),
                next_val
            ));
        }
        // MariaDB：NEXT_NOT_CACHED_VALUE + SETVAL。
        ServerType::ServerTypeMariaDB => {
            let q = format!(
                "SELECT NEXT_NOT_CACHED_VALUE FROM `{}`.`{}`",
                escapeString(database),
                escapeString(sequence)
            );
            let rows = db.QuerySQLWithColumns(tctx, &["next_not_cached_value"], &q)?;
            let next_val: i64 = rows
                .first()
                .and_then(|r| r.first())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            out.push_str(&format!(
                "SELECT SETVAL(`{}`,{});
",
                escapeString(sequence),
                next_val
            ));
        }
        // 其它引擎不追加 SETVAL。
        _ => {}
    }
    Ok(out)
}

/// 在 DDL 前保存并切换 character_set/collation 会话变量。
pub fn SetCharset(w: &mut String, character_set: &str, collation_connection: &str) {
    w.push_str(
        "SET @PREV_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT;
",
    );
    w.push_str(
        "SET @PREV_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS;
",
    );
    w.push_str(
        "SET @PREV_COLLATION_CONNECTION=@@COLLATION_CONNECTION;
",
    );
    w.push_str(&format!(
        "SET character_set_client = {character_set};
"
    ));
    w.push_str(&format!(
        "SET character_set_results = {character_set};
"
    ));
    w.push_str(&format!(
        "SET collation_connection = {collation_connection};
"
    ));
}

/// 恢复 SetCharset 保存的 @@ 变量。
pub fn RestoreCharset(w: &mut String) {
    w.push_str(
        "SET character_set_client = @PREV_CHARACTER_SET_CLIENT;
",
    );
    w.push_str(
        "SET character_set_results = @PREV_CHARACTER_SET_RESULTS;
",
    );
    w.push_str(
        "SET collation_connection = @PREV_COLLATION_CONNECTION;
",
    );
}

/// SQL 字符串字面量 quoting；单引号加倍。
fn quote_sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 按 list_type 枚举库中表/视图/序列；可过滤 TableType。
pub fn ListAllDatabasesTables(
    tctx: &tcontext::Context,
    db: &Conn,
    database_names: &[String],
    list_type: listTableType,
    table_types: &[TableType],
) -> Result<DatabaseTables> {
    let mut db_tables = DatabaseTables::new();
    // 空 slice 表示不过滤表类型。
    let type_set: HashMap<TableType, ()> = table_types.iter().map(|t| (*t, ())).collect();
    // 逐库列举并写入 DatabaseTables。
    for schema in database_names {
        // 预置空表列表便于 get_mut。
        db_tables.insert(schema.clone(), Vec::new());
        let query = match list_type {
            // information_schema 路径可读取 AVG_ROW_LENGTH。
            listTableType::listTableByInfoSchema => format!(
                "SELECT TABLE_NAME,TABLE_TYPE,AVG_ROW_LENGTH FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA={}",
                // schema 名 SQL 字面量。
                quote_sql_string(schema)
            ),
            // SHOW TABLE STATUS 通过 engine/comment 推断视图。
            listTableType::listTableByShowTableStatus => {
                format!("SHOW TABLE STATUS FROM {}", wrapBackTicks(schema))
            }
            // SHOW FULL TABLES 无行宽信息。
            listTableType::listTableByShowFullTables => {
                format!("SHOW FULL TABLES FROM {}", wrapBackTicks(schema))
            }
        };
        let mut rows = db.QueryContext(&query)?;
        while rows.Next() {
            match list_type {
                listTableType::listTableByInfoSchema => {
                    let mut dest = [RawBytes(None), RawBytes(None), RawBytes(None)];
                    rows.Scan(&mut dest)?;
                    let table =
                        String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b"")).to_string();
                    let table_type_str =
                        String::from_utf8_lossy(dest[1].as_opt().unwrap_or(b"")).to_string();
                    let avg = String::from_utf8_lossy(dest[2].as_opt().unwrap_or(b"0"))
                        .parse::<u64>()
                        .unwrap_or(0);
                    // 解析 TABLE_TYPE/Comment 为 TableType 枚举。
                    let table_type =
                        ParseTableType(&table_type_str).unwrap_or(TableType::TableTypeBase);
                    if type_set.is_empty() || type_set.contains_key(&table_type) {
                        // 表元信息：名、平均行宽、类型。
                        db_tables.get_mut(schema).unwrap().push(TableInfo {
                            Name: table,
                            AvgRowLength: avg,
                            Type: table_type,
                        });
                    }
                }
                listTableType::listTableByShowFullTables => {
                    let mut dest = [RawBytes(None), RawBytes(None)];
                    rows.Scan(&mut dest)?;
                    let table =
                        String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b"")).to_string();
                    let table_type_str =
                        String::from_utf8_lossy(dest[1].as_opt().unwrap_or(b"")).to_string();
                    let table_type =
                        ParseTableType(&table_type_str).unwrap_or(TableType::TableTypeBase);
                    if type_set.is_empty() || type_set.contains_key(&table_type) {
                        db_tables.get_mut(schema).unwrap().push(TableInfo {
                            Name: table,
                            AvgRowLength: 0,
                            Type: table_type,
                        });
                    }
                }
                listTableType::listTableByShowTableStatus => {
                    let mut dest = vec![RawBytes(None); 18];
                    rows.Scan(&mut dest)?;
                    let table =
                        String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b"")).to_string();
                    let engine =
                        String::from_utf8_lossy(dest[1].as_opt().unwrap_or(b"")).to_string();
                    let comment = String::from_utf8_lossy(
                        dest.get(17).and_then(|d| d.as_opt()).unwrap_or(b""),
                    )
                    .to_string();
                    let mut table_type = TableType::TableTypeBase;
                    // 空 engine + 视图 comment → TableTypeView。
                    if engine.is_empty() && (comment.is_empty() || comment == TableTypeViewStr) {
                        table_type = TableType::TableTypeView;
                    }
                    if type_set.is_empty() || type_set.contains_key(&table_type) {
                        db_tables.get_mut(schema).unwrap().push(TableInfo {
                            Name: table,
                            AvgRowLength: 0,
                            Type: table_type,
                        });
                    }
                }
            }
        }
        rows.Close()?;
        // 调试日志：每库表数量。
        tctx.L().Debug(
            "list tables",
            [
                Field::string("schema", schema.clone()),
                Field::string(
                    "count",
                    db_tables
                        .get(schema)
                        .map(|t| t.len())
                        .unwrap_or(0)
                        .to_string(),
                ),
            ],
        );
    }
    Ok(db_tables)
}

/// information_schema.placement_policies 去重 policy 名。
pub fn ListAllPlacementPolicyNames(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
) -> Result<Vec<String>> {
    const QUERY: &str = "select distinct policy_name from information_schema.placement_policies where policy_name is not null;";
    // placement policy 名。
    let results = db.QuerySQLWithColumns(tctx, &["policy_name"], QUERY)?;
    // policy 名列表。
    Ok(results
        .into_iter()
        .filter_map(|r| r.into_iter().next())
        .collect())
}

/// `SELECT VERSION()` 探测服务端版本字符串。
pub fn SelectVersion(db: &DB) -> Result<String> {
    // 版本探测。
    let mut rows = db.Query("SELECT VERSION()")?;
    if !rows.Next() {
        return Err(errors_new("empty version"));
    }
    let mut dest = [RawBytes(None)];
    rows.Scan(&mut dest)?;
    rows.Close()?;
    Ok(String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b"")).to_string())
}

/// 构造全表/分区 SELECT 的 TableDataIR；不含 chunk 边界。
pub fn SelectAllFromTable(
    conf: &Config,
    meta: &dyn TableMeta,
    partition: &str,
    order_by_clause: &str,
) -> Box<dyn TableDataIR> {
    // 组装无 chunk 边界的全表查询。
    let query = buildSelectQuery(
        // TableMeta 库名。
        meta.DatabaseName(),
        // TableMeta 表名。
        meta.TableName(),
        // 已选 SELECT 列清单。
        meta.SelectedField(),
        partition,
        // 全局 WHERE 不含 chunk extra。
        &buildWhereCondition(conf, ""),
        order_by_clause,
    );
    // 延迟执行 SELECT 的 IR。
    Box::new(newTableData(query, meta.SelectedLen() as usize, false))
}

/// 拼接 SELECT ... FROM db.table [PARTITION] [WHERE] [ORDER BY]。
pub fn buildSelectQuery(
    database: &str,
    table: &str,
    fields: &str,
    partition: &str,
    where_clause: &str,
    order_by_clause: &str,
) -> String {
    let mut query = String::from("SELECT ");
    // Go 在所有列均为生成列时用空字符串字面量保持 SELECT 合法。
    if fields.is_empty() {
        query.push_str("''");
    } else {
        query.push_str(fields);
    }
    query.push_str(" FROM ");
    query.push_str(&wrapBackTicks(database));
    query.push('.');
    query.push_str(&wrapBackTicks(table));
    // 分区表 SELECT 带 PARTITION 子句。
    if !partition.is_empty() {
        query.push_str(" PARTITION(");
        query.push_str(&wrapBackTicks(partition));
        query.push(')');
    }
    // 追加 WHERE（含 chunk 边界）。
    if !where_clause.is_empty() {
        query.push(' ');
        query.push_str(where_clause);
    }
    // 追加 ORDER BY 保证确定性导出顺序。
    if !order_by_clause.is_empty() {
        query.push(' ');
        query.push_str(order_by_clause);
    }
    query
}

/// 将列名列表格式化为 ORDER BY 子句。
pub fn buildOrderByClauseString(handle_col_names: &[String]) -> String {
    if handle_col_names.is_empty() {
        return String::new();
    }
    let cols: Vec<String> = handle_col_names.iter().map(|c| wrapBackTicks(c)).collect();
    format!("ORDER BY {}", cols.join(","))
}

/// 合并 conf.Where 与 chunk where_extra；空则返回空串。
pub fn buildWhereCondition(conf: &Config, where_extra: &str) -> String {
    match (conf.Where.is_empty(), where_extra.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("WHERE {} ", conf.Where),
        (true, false) => format!("WHERE {where_extra} "),
        (false, false) => format!("WHERE ({}) AND ({where_extra}) ", conf.Where),
    }
}

/// 按 handle 边界值生成 lexicographic chunk WHERE 列表。
pub fn buildWhereClauses(handle_col_names: &[String], handle_vals: &[Vec<String>]) -> Vec<String> {
    // 无 handle 列则无 chunk 边界。
    if handle_col_names.is_empty() || handle_vals.is_empty() {
        return vec![];
    }
    // 列名反引号 quoting。
    let quota_cols: Vec<String> = handle_col_names.iter().map(|c| wrapBackTicks(c)).collect();
    let mut clauses = Vec::new();
    for i in 0..handle_vals.len() {
        let mut buf = String::new();
        // 首段：列向量 < 第一边界。
        if i == 0 {
            buildCompareClause(&mut buf, &quota_cols, &handle_vals[i], b'<', false);
        } else {
            // 中间段：相邻边界的 lexicographic 区间。
            buildBetweenClause(&mut buf, &quota_cols, &handle_vals[i - 1], &handle_vals[i]);
        }
        clauses.push(buf);
    }
    let mut buf = String::new();
    // 末段：> 最后边界（末列含等号）。
    buildCompareClause(
        &mut buf,
        &quota_cols,
        handle_vals.last().unwrap(),
        b'>',
        true,
    );
    clauses.push(buf);
    clauses
}

/// 多列字典序比较子句；末列可写 >=/<=。
pub fn buildCompareClause(
    buf: &mut String,
    quota_cols: &[String],
    bound: &[String],
    compare: u8,
    write_equal: bool,
) {
    for i in 0..quota_cols.len() {
        if i > 0 {
            buf.push_str("or(");
        }
        for j in 0..i {
            buf.push_str(&quota_cols[j]);
            buf.push('=');
            buf.push_str(&bound[j]);
            buf.push_str(" and ");
        }
        buf.push_str(&quota_cols[i]);
        buf.push(compare as char);
        if write_equal && i == quota_cols.len() - 1 {
            buf.push('=');
        }
        buf.push_str(&bound[i]);
        if i > 0 {
            buf.push(')');
        } else if i != quota_cols.len() - 1 {
            buf.push(' ');
        }
    }
}

/// 两边界向量前缀相等长度；用于 between 优化。
pub fn getCommonLength(low: &[String], up: &[String]) -> usize {
    let mut i = 0;
    while i < low.len() && i < up.len() && low[i] == up[i] {
        i += 1;
    }
    i
}

/// 闭区间 [low, up) 的多列 WHERE 片段。
pub fn buildBetweenClause(buf: &mut String, quota_cols: &[String], low: &[String], up: &[String]) {
    let common = getCommonLength(low, up);
    if common == low.len() {
        buf.push_str("false");
        return;
    }
    for i in 0..common {
        if i > 0 {
            buf.push_str(" and ");
        }
        buf.push_str(&format!("{}={}", quota_cols[i], low[i]));
    }
    let (cols, low, up) = if common > 0 {
        buf.push_str(" and(");
        (&quota_cols[common..], &low[common..], &up[common..])
    } else {
        (quota_cols, low, up)
    };
    if cols.len() == 1 {
        buf.push_str(&format!(
            "{}>={} and {}<{}",
            cols[0], low[0], cols[0], up[0]
        ));
        if common > 0 {
            buf.push(')');
        }
        return;
    }
    buf.push('(');
    buf.push_str(&format!("{}>{} and {}<{}", cols[0], low[0], cols[0], up[0]));
    buf.push_str(")or(");
    buf.push_str(&format!("{}={} and(", cols[0], low[0]));
    buildCompareClause(buf, &cols[1..], &low[1..], b'>', true);
    buf.push_str("))or(");
    buf.push_str(&format!("{}={} and(", cols[0], up[0]));
    buildCompareClause(buf, &cols[1..], &up[1..], b'<', false);
    buf.push_str("))");
    if common < quota_cols.len() {
        if common > 0 {
            buf.push(')');
        }
    }
}

/// 非 block 基表生成 LOCK TABLES ... READ。
pub fn buildLockTablesSQL(
    all_tables: &DatabaseTables,
    block_list: &HashMap<String, HashMap<String, ()>>,
) -> String {
    let mut parts = Vec::new();
    for (db, tables) in all_tables {
        for table in tables {
            // LOCK TABLES 仅锁基表。
            if table.Type != TableType::TableTypeBase {
                continue;
            }
            // block-list 表跳过。
            if block_list
                .get(db)
                .map(|bl| bl.contains_key(&table.Name))
                .unwrap_or(false)
            {
                continue;
            }
            parts.push(format!(
                "{}.{} READ",
                wrapBackTicks(db),
                wrapBackTicks(&table.Name)
            ));
        }
    }
    // 多表 READ 锁拼接。
    if parts.is_empty() {
        String::new()
    } else {
        format!("LOCK TABLES {}", parts.join(", "))
    }
}

/// 全局读锁 FLUSH TABLES WITH READ LOCK。
pub fn FlushTableWithReadLock(_tctx: &tcontext::Context, db: &Conn) -> Result<()> {
    // 带 context 的执行。
    db.ExecContext("FLUSH TABLES WITH READ LOCK")?;
    Ok(())
}

/// UNLOCK TABLES 释放表锁。
pub fn UnlockTables(db: &Conn) -> Result<()> {
    db.ExecContext("UNLOCK TABLES")?;
    Ok(())
}

/// MySQL 8.4+ 用 SHOW BINARY LOG STATUS，否则 MASTER STATUS。
pub fn ShowMasterStatus(db: &Conn, server_info: &ServerInfo) -> Result<Vec<String>> {
    let q = if server_info.ServerType == ServerType::ServerTypeMySQL {
        // MySQL 8.4+ 使用 SHOW BINARY LOG STATUS。
        if let Some(v) = &server_info.ServerVersion {
            if !v.LessThan(&parse_semver("8.4.0")) {
                "SHOW BINARY LOG STATUS"
            } else {
                "SHOW MASTER STATUS"
            }
        } else {
            "SHOW MASTER STATUS"
        }
    } else {
        "SHOW MASTER STATUS"
    };
    let mut rows = db.QueryContext(q)?;
    // 无 master status 返回空。
    if !rows.Next() {
        rows.Close()?;
        return Ok(vec![]);
    }
    let cols = rows.Columns()?;
    let mut dest = vec![RawBytes(None); cols.len()];
    rows.Scan(&mut dest)?;
    rows.Close()?;
    Ok(dest
        .into_iter()
        .map(|d| String::from_utf8_lossy(d.as_opt().unwrap_or(b"")).to_string())
        .collect())
}

/// 按列名投影多行并 Close；列名大小写不敏感。
pub fn GetSpecifiedColumnValuesAndClose(
    rows: &mut Rows,
    column_names: &[&str],
) -> Result<Vec<Vec<String>>> {
    let cols = rows.Columns()?;
    let mut idxs = Vec::new();
    for name in column_names {
        let idx = cols
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
            // 列名不存在即报错。
            // 格式化错误。
            .ok_or_else(|| errors_errorf(format!("column {name} not found")))?;
        idxs.push(idx);
    }
    let mut out = Vec::new();
    while rows.Next() {
        let mut dest = vec![RawBytes(None); cols.len()];
        rows.Scan(&mut dest)?;
        let mut row = Vec::new();
        for &i in &idxs {
            row.push(String::from_utf8_lossy(dest[i].as_opt().unwrap_or(b"")).to_string());
        }
        out.push(row);
    }
    rows.Close()?;
    Ok(out)
}

/// 单列版 GetSpecifiedColumnValuesAndClose。
pub fn GetSpecifiedColumnValueAndClose(rows: &mut Rows, column_name: &str) -> Result<Vec<String>> {
    Ok(GetSpecifiedColumnValuesAndClose(rows, &[column_name])?
        .into_iter()
        .filter_map(|r| r.into_iter().next())
        .collect())
}

/// CLUSTER_INFO 中 TYPE=pd 的地址列表。
pub fn GetPdAddrs(tctx: &tcontext::Context, db: &DB) -> Result<Vec<String>> {
    let mut rows = db.Query("SELECT * FROM INFORMATION_SCHEMA.CLUSTER_INFO WHERE TYPE='pd'")?;
    let mut addrs = Vec::new();
    while rows.Next() {
        let mut dest = [RawBytes(None)];
        let _ = rows.Scan(&mut dest);
        if let Some(b) = dest[0].as_opt() {
            addrs.push(String::from_utf8_lossy(b).to_string());
        }
    }
    rows.Close()?;
    tctx.L()
        .Debug("GetPdAddrs", [Field::string("n", addrs.len().to_string())]);
    // PD 地址列表。
    Ok(addrs)
}

/// 集群 TiDB 实例 ID 列表。
pub fn GetTiDBDDLIDs(_tctx: &tcontext::Context, db: &DB) -> Result<Vec<String>> {
    let mut rows =
        db.Query("SELECT DISTINCT TIDB_INSTANCE_ID FROM INFORMATION_SCHEMA.CLUSTER_INFO")?;
    let mut ids = Vec::new();
    while rows.Next() {
        let mut dest = [RawBytes(None)];
        rows.Scan(&mut dest)?;
        if let Some(b) = dest[0].as_opt() {
            ids.push(String::from_utf8_lossy(b).to_string());
        }
    }
    rows.Close()?;
    // TiDB 实例 ID。
    Ok(ids)
}

/// @@tidb_config 含 tikv 判定存储为 TiKV。
pub fn CheckTiDBWithTiKV(db: &DB) -> Result<bool> {
    let mut rows = db.Query("SELECT @@tidb_config")?;
    if rows.Next() {
        let mut dest = [RawBytes(None)];
        rows.Scan(&mut dest)?;
        rows.Close()?;
        let s = String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b""));
        // @@tidb_config 含 tikv 子串。
        return Ok(s.contains("tikv"));
    }
    rows.Close()?;
    // 非 TiKV / 无 rowid 等 false 路径。
    Ok(false)
}

/// SHOW CONFIG enable-table-lock 是否为 true/1。
pub fn CheckTiDBEnableTableLock(db: &Conn) -> Result<bool> {
    let mut rows = db.QueryContext("SHOW CONFIG WHERE name='enable-table-lock'")?;
    if rows.Next() {
        let cols = rows.Columns()?.len().max(1);
        let mut dest = vec![RawBytes(None); cols];
        rows.Scan(&mut dest)?;
        rows.Close()?;
        // SHOW CONFIG 值列。
        let val = String::from_utf8_lossy(dest.last().and_then(|d| d.as_opt()).unwrap_or(b""));
        return Ok(val == "true" || val == "1");
    }
    rows.Close()?;
    Ok(false)
}

/// 按 avg_row_length 估算单文件行数（目标 128MiB，上限 1e6）。
pub fn GetSuitableRows(avg_row_length: u64) -> u64 {
    // 无 avg_row_length 时默认 chunk 行数。
    const DEFAULT_ROWS: u64 = 200000;
    // chunk 行数上限。
    const MAX_ROWS: u64 = 1_000_000;
    // 单文件 128MiB 目标体积。
    const BYTES_PER_FILE: u64 = 128 * 1024 * 1024;
    if avg_row_length == 0 {
        return DEFAULT_ROWS;
    }
    let estimate = BYTES_PER_FILE / avg_row_length;
    // 超过 MAX_ROWS 则截断。
    if estimate > MAX_ROWS {
        MAX_ROWS
    } else {
        estimate
    }
}

pub fn simpleQueryWithArgs<F>(
    _tctx: &tcontext::Context,
    conn: &Conn,
    handle_one_row: &mut F,
    query: &str,
) -> Result<()>
where
    F: FnMut(&mut Rows) -> Result<()>,
{
    let mut rows = conn.QueryContext(query)?;
    // 逐行回调。
    while rows.Next() {
        handle_one_row(&mut rows)?;
    }
    // 扫描错误时 Close。
    if let Some(err) = rows.Err() {
        let _ = rows.Close();
        return Err(err);
    }
    rows.Close()
}

/// EXPLAIN 行数估计；兼容 rows/estRows/count 列名。
pub fn estimateCount(
    tctx: &tcontext::Context,
    db_name: &str,
    table_name: &str,
    db: &mut BaseConn,
    field: &str,
    _conf: &Config,
) -> u64 {
    let query = format!(
        "EXPLAIN SELECT {} FROM {}.{}",
        if field.is_empty() { "*" } else { field },
        wrapBackTicks(db_name),
        wrapBackTicks(table_name)
    );
    detectEstimateRows(tctx, db, &query, &["rows", "estRows", "count"])
}

/// 解析 EXPLAIN 结果首个可解析浮点单元格。
pub fn detectEstimateRows(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    query: &str,
    field_names: &[&str],
) -> u64 {
    match db.QuerySQLWithColumns(tctx, field_names, query) {
        Ok(results) => {
            for row in results {
                for cell in row {
                    // 取首个可解析行数估计。
                    if let Ok(v) = cell.parse::<f64>() {
                        return v as u64;
                    }
                }
            }
            0
        }
        // EXPLAIN 失败时行数估计为 0。
        Err(_) => 0,
    }
}

/// SHOW MASTER STATUS 第二列 binlog 位点/TSO 字符串。
pub fn getSnapshot(db: &Conn) -> Result<String> {
    // binlog 位点作 snapshot。
    let mut rows = db.QueryContext("SHOW MASTER STATUS")?;
    // 无 replication 信息则空 snapshot。
    if !rows.Next() {
        rows.Close()?;
        return Ok(String::new());
    }
    let cols = rows.Columns()?.len().max(2);
    let mut dest = vec![RawBytes(None); cols];
    rows.Scan(&mut dest)?;
    rows.Close()?;
    Ok(String::from_utf8_lossy(dest.get(1).and_then(|d| d.as_opt()).unwrap_or(b"")).to_string())
}

/// 忽略未知系统变量错误（老版本 MySQL）。
pub fn isUnknownSystemVariableErr(err: &Error) -> bool {
    // 老 MySQL 未知变量可忽略。
    err.msg
        .to_ascii_lowercase()
        .contains("unknown system variable")
}

/// RR + CONSISTENT SNAPSHOT 事务连接。
pub fn createConnWithConsistency(db: &DB, repeatable_read: bool) -> Result<Conn> {
    let conn = db.Conn()?;
    // 一致性快照事务。
    if repeatable_read {
        // 忽略 SET 失败以兼容旧 MySQL。
        let _ = conn.ExecContext("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ");
        // 开启一致性读快照。
        let _ = conn.ExecContext("START TRANSACTION WITH CONSISTENT SNAPSHOT");
    }
    // 返回一致性连接。
    Ok(conn)
}

/// Values to dump and whether SELECT * would omit or include unwanted columns.
pub fn getWritableColumnNames(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    db_name: &str,
    table_name: &str,
    include_stored_generated: bool,
) -> Result<(Vec<String>, bool)> {
    let query = format!(
        "SHOW COLUMNS FROM `{}`.`{}`",
        escapeString(db_name),
        escapeString(table_name)
    );
    let results = db.QuerySQLWithColumns(tctx, &["FIELD", "EXTRA"], &query)?;
    let mut names = Vec::with_capacity(results.len());
    let mut need_explicit_fields = false;
    for row in results {
        let extra = row[1].to_uppercase();
        if extra.contains("VIRTUAL GENERATED")
            || (extra.contains("STORED GENERATED") && !include_stored_generated)
        {
            need_explicit_fields = true;
            continue;
        }
        if extra.contains("INVISIBLE") {
            need_explicit_fields = true;
        }
        names.push(row[0].clone());
    }
    Ok((names, need_explicit_fields))
}

pub fn buildSelectField(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    db_name: &str,
    table_name: &str,
    complete_insert: bool,
) -> Result<(String, i32)> {
    let (names, generated) = getWritableColumnNames(tctx, db, db_name, table_name, false)?;
    Ok((
        if complete_insert || generated {
            columnNamesToSelectFields(&names).join(",")
        } else {
            "*".into()
        },
        names.len() as i32,
    ))
}

/// information_schema.partitions 非空 PARTITION_NAME。
pub fn GetPartitionNames(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let query = format!(
        "SELECT PARTITION_NAME FROM INFORMATION_SCHEMA.PARTITIONS WHERE TABLE_SCHEMA={} AND TABLE_NAME={} AND PARTITION_NAME IS NOT NULL",
        quote_sql_string(schema),
        // table 名 SQL 字面量。
        quote_sql_string(table)
    );
    let rows = db.QuerySQLWithColumns(tctx, &["PARTITION_NAME"], &query)?;
    // 分区名列表。
    Ok(rows
        .into_iter()
        .filter_map(|r| r.into_iter().next())
        .collect())
}

/// TiDB 隐式 rowid 排序常量。
pub const orderByTiDBRowID: &str = "ORDER BY `_tidb_rowid`";

/// SHOW INDEX 中 KEY_NAME=PRIMARY 的列序。
pub fn GetPrimaryKeyColumns(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    table: &str,
) -> Result<Vec<String>> {
    let query = format!(
        "SHOW INDEX FROM `{}`.`{}`",
        escapeString(database),
        escapeString(table)
    );
    // SHOW INDEX 主键列。
    let results = db.QuerySQLWithColumns(tctx, &["KEY_NAME", "COLUMN_NAME"], &query)?;
    let mut cols = Vec::new();
    for row in results {
        if row.first().map(|s| s.as_str()) == Some("PRIMARY") {
            if let Some(c) = row.get(1) {
                cols.push(c.clone());
            }
        }
    }
    // 主键列名列表。
    Ok(cols)
}

/// SortByPk 时选 rowid 或主键 ORDER BY。
pub fn buildOrderByClause(
    tctx: &tcontext::Context,
    conf: &Config,
    db: &mut BaseConn,
    database: &str,
    table: &str,
    has_implicit_row_id: bool,
) -> Result<String> {
    // 禁用主键排序则返回空 ORDER BY。
    if !conf.SortByPk {
        return Ok(String::new());
    }
    // 使用 TiDB 隐式 _tidb_rowid 排序。
    if has_implicit_row_id {
        // 隐式 rowid 常量。
        return Ok(orderByTiDBRowID.to_string());
    }
    let cols = GetPrimaryKeyColumns(tctx, db, database, table)?;
    // 多列主键 ORDER BY。
    Ok(buildOrderByClauseString(&cols))
}

/// LIMIT 1 探测 `_tidb_rowid`；1054 视为无隐式 rowid。
pub fn SelectTiDBRowID(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    table: &str,
) -> Result<bool> {
    let query = format!(
        "SELECT _tidb_rowid from `{}`.`{}` LIMIT 1",
        escapeString(database),
        escapeString(table)
    );
    let mut has = false;
    // 探测查询走 ExecSQL 以便捕获错误码。
    match db.ExecSQL(
        tctx,
        |_res, err| {
            if let Some(e) = err {
                let msg = e.msg.to_ascii_lowercase();
                // 1054/unknown column：无 _tidb_rowid。
                if msg.contains("1054")
                    || msg.contains("unknown column")
                    || msg.contains("bad field")
                {
                    has = false;
                    return Ok(());
                }
                // 附加上下文 annotate。
                return Err(errors_annotatef(e.clone(), format!("sql: {query}")));
            }
            has = true;
            Ok(())
        },
        &query,
    ) {
        // 返回 rowid 探测结果。
        Ok(()) => Ok(has),
        Err(e) => {
            let msg = e.msg.to_ascii_lowercase();
            // 多种驱动错误文本统一判定。
            if msg.contains("1054") || msg.contains("unknown column") || msg.contains("bad field") {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// 快照字符串 → PD TSO；支持数字或 datetime。
pub fn parseSnapshotToTSO(pool: &DB, snapshot: &str) -> Result<u64> {
    // 数字形式快照即 TSO。
    if let Ok(ts) = snapshot.parse::<u64>() {
        return Ok(ts);
    }
    let key = "SELECT unix_timestamp(?)";
    if let Some(v) = pool.query_row.lock().unwrap().get(key).cloned() {
        return match v {
            Some(tso) => Ok(((tso as u64) << 18) * 1000),
            None => Err(errors_errorf(format!(
                "snapshot {snapshot} format not supported. please use tso or '2006-01-02 15:04:05' format time"
            ))),
        };
    }
    // 未缓存则 unix_timestamp 查询。
    let query = format!("SELECT unix_timestamp(\"{snapshot}\")");
    let mut rows = pool.Query(&query)?;
    if !rows.Next() {
        let _ = rows.Close();
        return Err(errors_errorf(format!(
            "snapshot {snapshot} format not supported. please use tso or '2006-01-02 15:04:05' format time"
        )));
    }
    let mut dest = [RawBytes(None)];
    rows.Scan(&mut dest)?;
    rows.Close()?;
    match dest[0].as_opt() {
        None => Err(errors_errorf(format!(
            "snapshot {snapshot} format not supported. please use tso or '2006-01-02 15:04:05' format time"
        ))),
        Some(b) => {
            let tso: i64 = String::from_utf8_lossy(b).parse().unwrap_or(0);
            // unix_timestamp → TSO 物理时间编码。
            Ok(((tso as u64) << 18) * 1000)
        }
    }
}

/// 为 chunk 切分选数值索引列；委托 getNumericIndex。
pub fn pickupPossibleField(
    tctx: &tcontext::Context,
    meta: &dyn TableMeta,
    db: &mut BaseConn,
) -> Result<String> {
    if meta.HasImplicitRowID() {
        return Ok("_tidb_rowid".to_string());
    }
    getNumericIndex(tctx, db, meta)
}

/// 优先 PRIMARY，其次 UNIQUE，再 cardinality 最大非唯一索引。
pub fn getNumericIndex(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    meta: &dyn TableMeta,
) -> Result<String> {
    let database = meta.DatabaseName();
    let table = meta.TableName();
    // 列名→类型映射供整数判定。
    let col_map = string2Map(&tableSourceColumnNames(meta), &tableSourceColumnTypes(meta));
    let query = format!(
        "SHOW INDEX FROM `{}`.`{}`",
        escapeString(database),
        escapeString(table)
    );
    let results = db.QuerySQLWithColumns(
        tctx,
        &[
            "NON_UNIQUE",
            "SEQ_IN_INDEX",
            "KEY_NAME",
            "COLUMN_NAME",
            "CARDINALITY",
        ],
        &query,
    )?;
    let mut unique_key_map: HashMap<String, (String, u64)> = HashMap::new();
    let mut key_column = String::new();
    let mut max_card: i64 = -1;
    for row in results {
        let non_unique = row.first().map(|s| s.as_str()).unwrap_or("");
        let seq = row.get(1).map(|s| s.as_str()).unwrap_or("1");
        let key_name = row.get(2).map(|s| s.as_str()).unwrap_or("");
        let col_name = row.get(3).map(|s| s.as_str()).unwrap_or("");
        let card = row.get(4).map(|s| s.as_str()).unwrap_or("0");
        // 仅索引首列参与 handle 选择。
        if seq != "1" {
            continue;
        }
        let tp = col_map.get(col_name).map(|s| s.as_str()).unwrap_or("");
        // split field 必须是整数类型。
        if !dataTypeIntContains(tp) {
            continue;
        }
        // 主键列优先。
        if key_name == "PRIMARY" {
            return Ok(col_name.to_string());
        // 唯一索引候选。
        } else if non_unique == "0" {
            unique_key_map.insert(key_name.to_string(), (col_name.to_string(), 1));
        // 无非唯一索引时才比 cardinality。
        } else if unique_key_map.is_empty() {
            // 返回选中索引列名。
            if let Ok(c) = card.parse::<i64>() {
                if c > max_card {
                    key_column = col_name.to_string();
                    max_card = c;
                }
            }
        }
    }
    if let Some((c, _)) = unique_key_map.into_values().min_by_key(|(_, n)| *n) {
        return Ok(c);
    }
    // 回退 cardinality 最大列。
    Ok(key_column)
}

/// information_schema 中 table_type=SEQUENCE 存在性。
pub fn CheckIfSeqExists(
    tctx: &tcontext::Context,
    db: &mut BaseConn,
    database: &str,
    sequence: &str,
) -> Result<bool> {
    let query = format!(
        "SELECT 1 FROM information_schema.tables WHERE table_schema={} AND table_name={} AND table_type='SEQUENCE'",
        quote_sql_string(database),
        quote_sql_string(sequence)
    );
    let rows = db.QuerySQLWithColumns(tctx, &["1"], &query)?;
    // 有行即序列存在。
    // 序列存在。
    Ok(!rows.is_empty())
}

/// SHOW CHARACTER SET → charset→default collation。
pub fn GetCharsetAndDefaultCollation(db: &Conn) -> Result<HashMap<String, String>> {
    let mut rows = db.QueryContext("SHOW CHARACTER SET")?;
    let mut map = HashMap::new();
    while rows.Next() {
        let mut dest = [
            RawBytes(None),
            RawBytes(None),
            RawBytes(None),
            RawBytes(None),
        ];
        rows.Scan(&mut dest)?;
        // charset 名小写化作 map 键。
        let charset = String::from_utf8_lossy(dest[0].as_opt().unwrap_or(b"")).to_ascii_lowercase();
        let collation = String::from_utf8_lossy(dest[2].as_opt().unwrap_or(b"")).to_string();
        map.insert(charset, collation);
    }
    rows.Close()?;
    // charset 映射。
    Ok(map)
}

/// TiDB TABLESAMPLE REGIONS() 按 PK 列采样。
pub fn buildTiDBTableSampleQuery(
    pk_fields: &[String],
    db_name: &str,
    tbl_name: &str,
    partitions: &[&str],
) -> String {
    let fields: Vec<String> = pk_fields.iter().map(|f| wrapBackTicks(f)).collect();
    let mut q = format!(
        "SELECT {} FROM `{}`.`{}`",
        fields.join(","),
        escapeString(db_name),
        escapeString(tbl_name)
    );
    if !partitions.is_empty() {
        let parts: Vec<String> = partitions.iter().map(|p| wrapBackTicks(p)).collect();
        q.push_str(&format!(" PARTITION ({})", parts.join(",")));
    }
    // TiDB region 采样语句。
    q.push_str(" TABLESAMPLE REGIONS()");
    q
}

/// 分区名 → ` PARTITION (name)` 片段向量。
pub fn buildPartitionClauses(partitions: &[String]) -> Vec<String> {
    partitions
        .iter()
        .map(|p| format!(" PARTITION ({})", wrapBackTicks(p)))
        .collect()
}

/// conf.Partitions 为空单查，否则每分区一条 sample。
pub fn buildTableSampleQueries(
    conf: &Config,
    meta: &dyn TableMeta,
    pk_fields: &[String],
) -> Vec<String> {
    // 无分区配置则单条 sample SQL。
    if conf.Partitions.is_empty() {
        vec![buildTiDBTableSampleQuery(
            pk_fields,
            meta.DatabaseName(),
            meta.TableName(),
            &[],
        )]
    } else {
        conf.Partitions
            .iter()
            .map(|p| {
                buildTiDBTableSampleQuery(
                    pk_fields,
                    meta.DatabaseName(),
                    meta.TableName(),
                    &[p.as_str()],
                )
            })
            .collect()
    }
}

/// TIKV_REGION_STATUS 按 handle 列 DISTINCT region。
pub fn buildRegionQueriesWithoutPartition(
    db_name: &str,
    table_name: &str,
    handle_col: &str,
) -> String {
    format!(
        // 非索引 region，按 START_KEY 排序。
        "SELECT DISTINCT {} FROM INFORMATION_SCHEMA.TIKV_REGION_STATUS WHERE DB_NAME={} AND TABLE_NAME={} AND IS_INDEX=0 ORDER BY START_KEY",
        wrapBackTicks(handle_col),
        // db 名 SQL 字面量。
        quote_sql_string(db_name),
        quote_sql_string(table_name)
    )
}

/// 分区表 region 查询，带 PARTITION_NAME 过滤。
pub fn buildRegionQueriesWithPartitions(
    db_name: &str,
    table_name: &str,
    handle_col: &str,
    partitions: &[String],
) -> Vec<String> {
    partitions
        .iter()
        .map(|p| {
            format!(
                // 分区 region 过滤。
                "SELECT DISTINCT {} FROM INFORMATION_SCHEMA.TIKV_REGION_STATUS WHERE DB_NAME={} AND TABLE_NAME={} AND PARTITION_NAME={} AND IS_INDEX=0 ORDER BY START_KEY",
                wrapBackTicks(handle_col),
                quote_sql_string(db_name),
                quote_sql_string(table_name),
                // 分区名 SQL 字面量。
                quote_sql_string(p)
            )
        })
        .collect()
}

/// v3 一致性 START_KEY/END_KEY region 枚举。
pub fn buildVersion3RegionQueries(db_name: &str, table_name: &str) -> String {
    format!(
        // v3 仅需键范围。
        "SELECT START_KEY,END_KEY FROM INFORMATION_SCHEMA.TIKV_REGION_STATUS WHERE DB_NAME={} AND TABLE_NAME={} AND IS_INDEX=0 ORDER BY START_KEY",
        quote_sql_string(db_name),
        quote_sql_string(table_name)
    )
}
