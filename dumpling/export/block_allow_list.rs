// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这里负责把已经探测到的数据库/表集合再按 block-allow 过滤器缩一遍，
// 让后续导出逻辑只看到真正允许进入任务的对象。
// 过滤分成 schema 级和 table 级两层，都会把被忽略的对象写到 debug 日志，
// 方便调用方理解“为什么某个库或表没有出现在最终导出列表里”。

pub fn filterDatabases(
    tctx: &tcontext::Context,
    conf: &Config,
    databases: Vec<String>,
) -> Vec<String> {
    // 先记录开始过滤，便于和下游 table 过滤日志形成完整链路。
    tctx.L().Debug("start to filter databases", []);
    let mut new_databases = Vec::with_capacity(databases.len());
    let mut ignore_databases = Vec::with_capacity(databases.len());
    for database in databases {
        // schema 级匹配直接复用 TableFilter 的 MatchSchema 语义。
        if conf.TableFilter.MatchSchema(&database) {
            new_databases.push(database);
        } else {
            ignore_databases.push(database);
        }
    }
    if !ignore_databases.is_empty() {
        // 只在确实有忽略项时打印，避免空日志噪声。
        tctx.L().Debug(
            "ignore database",
            [Field::string("databases", ignore_databases.join(","))],
        );
    }
    new_databases
}

pub fn filterTables(tctx: &tcontext::Context, conf: &mut Config) {
    // 默认路径直接把 Config 自带的 TableFilter 适配成 match 回调。
    let filter = conf.TableFilter.clone();
    filterTablesFunc(tctx, conf, |db, table| filter.MatchTable(db, table));
}

pub fn filterTablesFunc<F>(tctx: &tcontext::Context, conf: &mut Config, match_table: F)
where
    F: Fn(&str, &str) -> bool,
{
    // 允许注入自定义匹配函数，测试就能只验证过滤骨架而不依赖真实过滤器实现。
    tctx.L().Debug("start to filter tables", []);
    let mut db_tables = DatabaseTables::new();
    let mut ignored = DatabaseTables::new();

    // Collect keys first to avoid borrow issues
    let entries: Vec<(String, Vec<TableInfo>)> = conf
        .Tables
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (db_name, tables) in entries {
        for table in tables {
            // 命中的表进入新集合，未命中的表则只保留在 ignored 里做日志说明。
            if match_table(&db_name, &table.Name) {
                db_tables.AppendTable(db_name.clone(), table);
            } else {
                ignored.AppendTable(db_name.clone(), table);
            }
        }
        if conf.DumpEmptyDatabase {
            // 即使库里没有任何命中表，也允许在“导出空库”模式下保留空库占位。
            if !db_tables.contains_key(&db_name) && conf.TableFilter.MatchSchema(&db_name) {
                db_tables.insert(db_name.clone(), Vec::new());
            }
        }
    }

    if !ignored.is_empty() {
        // 被过滤掉的表以字面量形式输出，方便排查规则是否写错。
        tctx.L()
            .Debug("ignore table", [Field::string("tables", ignored.Literal())]);
    }
    // 最终直接覆写 Config，让调用方后续只处理筛选后的集合。
    conf.Tables = db_tables;
}
