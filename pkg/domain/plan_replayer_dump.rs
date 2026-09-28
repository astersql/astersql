// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and

// Plan Replayer dump 包内容组织与写入。
//
// 将 SQL、schema/view、统计信息、会话变量、bindings、explain/plan 与 debug trace
// 按固定目录布局写入归档（对齐 Go 侧 zip）。并提供从 SQL 文本抽取表引用
//（忽略 CTE）的轻量解析，供 capture/dump 组装依赖表集合。

// limitations under the License.

// plan replayer dump zip 包的组织、表名抽取、schema/stats/variables/bindings/explain/debug trace 写入流程。
//
// PlanReplayerSQLMetaFile indicates sql meta path for plan replayer
// pub const PlanReplayerSQLMetaFile: &str = "sql_meta.toml";
// PlanReplayerConfigFile indicates config file path for plan replayer
// pub const PlanReplayerConfigFile: &str = "config.toml";
// PlanReplayerMetaFile meta file path for plan replayer
// pub const PlanReplayerMetaFile: &str = "meta.txt";
// PlanReplayerVariablesFile indicates for session variables file path for plan replayer
// pub const PlanReplayerVariablesFile: &str = "variables.toml";
// PlanReplayerTiFlashReplicasFile indicates for table tiflash replica file path for plan replayer
// pub const PlanReplayerTiFlashReplicasFile: &str = "table_tiflash_replica.txt";
// PlanReplayerSessionBindingFile indicates session binding file path for plan replayer
// pub const PlanReplayerSessionBindingFile: &str = "session_bindings.sql";
// PlanReplayerGlobalBindingFile indicates global binding file path for plan replayer
// pub const PlanReplayerGlobalBindingFile: &str = "global_bindings.sql";
// PlanReplayerSchemaMetaFile indicates the schema meta
// pub const PlanReplayerSchemaMetaFile: &str = "schema_meta.txt";
// PlanReplayerErrorMessageFile is the file name for error messages
// pub const PlanReplayerErrorMessageFile: &str = "errors.txt";
//
// PlanReplayerSQLMetaStartTS indicates the startTS in plan replayer sql meta
// pub const PlanReplayerSQLMetaStartTS: &str = "startTS";
// PlanReplayerTaskMetaIsCapture indicates whether this task is capture task
// pub const PlanReplayerTaskMetaIsCapture: &str = "isCapture";
// PlanReplayerTaskMetaIsContinues indicates whether this task is continues task
// pub const PlanReplayerTaskMetaIsContinues: &str = "isContinues";
// PlanReplayerTaskMetaSQLDigest indicates the sql digest of this task
// pub const PlanReplayerTaskMetaSQLDigest: &str = "sqlDigest";
// PlanReplayerTaskMetaPlanDigest indicates the plan digest of this task
// pub const PlanReplayerTaskMetaPlanDigest: &str = "planDigest";
// PlanReplayerTaskEnableHistoricalStats indicates whether the task is using historical stats
// pub const PlanReplayerTaskEnableHistoricalStats: &str = "enableHistoricalStats";
// PlanReplayerHistoricalStatsTS indicates the expected TS of the historical stats if it's specified by the user.
// pub const PlanReplayerHistoricalStatsTS: &str = "historicalStatsTS";
//
// tableNamePair 对应 Go 结构体：记录库名、表名以及该名称是否为 view。
// #[derive(Clone, Debug, Eq, Hash, PartialEq)]
// pub struct tableNamePair {
//     pub DBName: String,
//     pub TableName: String,
//     pub IsView: bool,
// }
//
// tableNameExtractor 对应 Go 的 AST Visitor：从语句中提取普通表、视图和 CTE 名称。
// pub struct tableNameExtractor {
//     pub ctx: context::Context,
//     pub executor: sqlexec::RestrictedSQLExecutor,
//     pub is: infoschema::InfoSchema,
//     pub curDB: ast::CIStr,
//     pub names: std::collections::HashMap<tableNamePair, ()>,
//     pub cteNames: std::collections::HashMap<String, ()>,
//     pub err: Option<errors::Error>,
// }
//
// impl tableNameExtractor {
// getTablesAndViews 对应 Go 方法：过滤掉 CTE，并递归补充外键引用表。
//     pub fn getTablesAndViews(&mut self) -> Result<std::collections::HashMap<tableNamePair, ()>, errors::Error> {
//         let mut r = std::collections::HashMap::new();
//         for tablePair in self.names.keys() {
//             if tablePair.IsView {
//                 r.insert(tablePair.clone(), ());
//                 continue;
//             }
// CTE 名称不应作为真实表写入 dump。
//             if !self.cteNames.contains_key(&tablePair.TableName) {
//                 r.insert(tablePair.clone(), ());
//             }
//             findFK(self.is.clone(), &tablePair.DBName, &tablePair.TableName, &mut r)?;
//         }
//         Ok(r)
//     }
//
// handleIsView 对应 Go 方法：识别 view 后解析 view SQL，继续提取其底层表。
//     pub fn handleIsView(&mut self, t: &ast::TableName) -> Result<bool, errors::Error> {
//         let mut schema = t.Schema.clone();
//         if schema.L.is_empty() {
//             schema = self.curDB.clone();
//         }
//         let table = t.Name.clone();
//         let isView = infoschema::TableIsView(self.is.clone(), schema.clone(), table.clone());
//         if !isView {
//             return Ok(false);
//         }
//         let viewTbl = self.is.TableByName(context::Background(), schema, table)?;
//         let sql = viewTbl.Meta().View.SelectStmt;
//         let node = self.executor.ParseWithParams(self.ctx.clone(), &sql)?;
//         node.Accept(self);
//         Ok(true)
//     }
// }
//
// findFK 对应 Go 函数：发现外键引用表，并用 tableMap 防止循环外键导致无限递归。
// pub fn findFK(
//     is: infoschema::InfoSchema,
//     dbName: &str,
//     tableName: &str,
//     tableMap: &mut std::collections::HashMap<tableNamePair, ()>,
// ) -> Result<(), errors::Error> {
//     let tblInfo = is.TableByName(context::Background(), ast::NewCIStr(dbName), ast::NewCIStr(tableName))?;
//     for fk in tblInfo.Meta().ForeignKeys {
//         let key = tableNamePair {
//             DBName: fk.RefSchema.L,
//             TableName: fk.RefTable.L,
//             IsView: false,
//         };
//         if tableMap.contains_key(&key) {
// 已访问过的表直接跳过，保持 Go 中避免循环外键递归的保护。
//             continue;
//         }
//         tableMap.insert(key.clone(), ());
//         findFK(is.clone(), &key.DBName, &key.TableName, tableMap)?;
//     }
//     Ok(())
// }
// */
use crate::plan_replayer::{PlanReplayerDumpTask, PlanReplayerStatusRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

/// SQL 元数据文件名（startTS、digest、capture 标志等）。
pub const PLAN_REPLAYER_SQL_META_FILE: &str = "sql_meta.toml";
/// 集群/实例配置快照文件名。
pub const PLAN_REPLAYER_CONFIG_FILE: &str = "config.toml";
/// 通用元信息文件名。
pub const PLAN_REPLAYER_META_FILE: &str = "meta.txt";
/// 会话变量快照文件名。
pub const PLAN_REPLAYER_VARIABLES_FILE: &str = "variables.toml";
/// TiFlash 副本信息文件名。
pub const PLAN_REPLAYER_TIFLASH_REPLICAS_FILE: &str = "table_tiflash_replica.txt";
/// 会话级 Binding 文件名。
pub const PLAN_REPLAYER_SESSION_BINDING_FILE: &str = "session_bindings.sql";
/// 全局 Binding 文件名。
pub const PLAN_REPLAYER_GLOBAL_BINDING_FILE: &str = "global_bindings.sql";
/// schema 元数据清单（库/表/是否 view）。
pub const PLAN_REPLAYER_SCHEMA_META_FILE: &str = "schema_meta.txt";
/// 非致命错误汇总文件名。
pub const PLAN_REPLAYER_ERROR_MESSAGE_FILE: &str = "errors.txt";

/// 库表名对，并标记是否为视图（view）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableNamePair {
    /// 数据库名。
    pub database: String,
    /// 表或视图名。
    pub table: String,
    /// 是否为视图。
    pub is_view: bool,
}

/// 内存中的 dump 归档：路径 → 文件内容。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayArchive {
    /// 归档内相对路径到字节内容的映射。
    pub files: BTreeMap<String, Vec<u8>>,
}

impl ReplayArchive {
    /// 写入一个归档条目；拒绝空路径、绝对路径、`..` 以及重复路径。
    pub fn write(
        &mut self,
        path: impl Into<String>,
        body: impl Into<Vec<u8>>,
    ) -> Result<(), String> {
        let path = path.into();
        if path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..") {
            return Err(format!("invalid archive path: {path}"));
        }
        if self.files.insert(path.clone(), body.into()).is_some() {
            return Err(format!("duplicate archive path: {path}"));
        }
        Ok(())
    }
}

/// 将内存归档编码为 Go `archive/zip` 可读的 ZIP 字节。
pub fn encode_replay_archive(archive: &ReplayArchive) -> Result<Vec<u8>, String> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, body) in &archive.files {
        writer
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        writer.write_all(body).map_err(|error| error.to_string())?;
    }
    writer
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|error| error.to_string())
}

/// 解码 ZIP 并保留所有非目录条目。
pub fn decode_replay_archive(bytes: &[u8]) -> Result<ReplayArchive, String> {
    let mut reader = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut archive = ReplayArchive::default();
    for index in 0..reader.len() {
        let mut file = reader.by_index(index).map_err(|error| error.to_string())?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let mut body = Vec::new();
        file.read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        archive.write(name, body)?;
    }
    Ok(archive)
}

/// dump 所需的外部数据源：解析表、取 schema/stats、explain 等。
pub trait PlanReplaySource {
    /// 当前会话默认库名。
    fn current_database(&self) -> String;
    /// 解析库表是否存在，并返回规范化的 `TableNamePair`。
    fn resolve_table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String>;
    /// 展开视图依赖的底层表。
    fn view_dependencies(&self, view: &TableNamePair) -> Result<Vec<TableNamePair>, String>;
    /// 展开外键等 schema 依赖；默认无依赖，便于纯数据源实现。
    fn table_dependencies(&self, _table: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        Ok(Vec::new())
    }
    /// `SHOW CREATE` 结果文本。
    fn show_create(&self, table: &TableNamePair) -> Result<String, String>;
    /// 导出表统计；可选返回 fallback 警告信息。
    fn stats(
        &self,
        table: &TableNamePair,
        historical_ts: u64,
    ) -> Result<(String, Option<String>), String>;
    /// 统计信息内存状态文本。
    fn stats_memory_status(&self, table: &TableNamePair) -> Result<String, String>;
    /// TiFlash 副本描述。
    fn tiflash_replica(&self, table: &TableNamePair) -> Result<String, String>;
    /// 配置快照。
    fn config(&self) -> Result<String, String>;
    /// 元信息文本。
    fn metadata(&self) -> Result<String, String>;
    /// 全局 Binding 列表。
    fn global_bindings(&self) -> Result<Vec<String>, String>;
    /// 执行 explain[/analyze]，可选附带 debug trace。
    fn explain(&self, sql: &str, analyze: bool) -> Result<(String, Option<String>), String>;
    /// 解码已编码的执行计划，对应 Go 的 `tidb_decode_plan` 查询。
    fn decode_plan(&self, encoded_plan: &str) -> Result<String, String>;
}

/// 组装完整 Plan Replayer 归档与 status 记录。
///
/// 流程：写 SQL meta/config/meta → 抽取并解析表/视图依赖 → 写 schema/stats/变量/
/// bindings/SQL → 按是否已有 encoded plan 写 explain 或 plan.txt → 汇总错误。
pub fn dump_plan_replayer_info(
    source: &dyn PlanReplaySource,
    task: &PlanReplayerDumpTask,
    historical_stats_for_capture_enabled: bool,
) -> Result<(ReplayArchive, Vec<PlanReplayerStatusRecord>), String> {
    let mut archive = ReplayArchive::default();
    let sql_meta = format!(
        "startTS = {}\nisCapture = {}\nisContinues = {}\nsqlDigest = {:?}\nplanDigest = {:?}\nenableHistoricalStats = {}\n",
        task.start_ts,
        task.is_capture,
        task.is_continuous_capture,
        task.key.sql_digest,
        task.key.plan_digest,
        historical_stats_for_capture_enabled,
    );
    let sql_meta = if task.historical_stats_ts > 0 {
        format!(
            "{sql_meta}historicalStatsTS = {}\n",
            task.historical_stats_ts
        )
    } else {
        sql_meta
    };
    archive.write(PLAN_REPLAYER_SQL_META_FILE, sql_meta)?;
    archive.write(PLAN_REPLAYER_CONFIG_FILE, source.config()?)?;
    archive.write(PLAN_REPLAYER_META_FILE, source.metadata()?)?;
    let current_database = source.current_database();
    // 从语句抽取表引用，再经 InfoSchema 解析为真实表/视图。
    let mut tables = BTreeSet::new();
    for statement in &task.statements {
        for (database, table) in extract_table_references(statement, &current_database) {
            if let Some(table) = source.resolve_table(&database, &table)? {
                tables.insert(table);
            }
        }
    }
    let mut pending = tables.iter().cloned().collect::<Vec<_>>();
    while let Some(table) = pending.pop() {
        let mut dependencies = source.table_dependencies(&table)?;
        if table.is_view {
            dependencies.extend(source.view_dependencies(&table)?);
        }
        for dependency in dependencies {
            if tables.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    let mut schema_meta = String::new();
    let mut tiflash = String::new();
    let mut errors = Vec::new();
    // 持续 capture 且启用历史统计时跳过即时 stats，避免与历史路径冲突。
    let skip_stats =
        task.is_capture && task.is_continuous_capture && historical_stats_for_capture_enabled;
    for table in &tables {
        let safe = format!("{}.{}", table.database, table.table);
        let schema_path = if table.is_view {
            format!("view/{safe}.view.txt")
        } else {
            format!("schema/{safe}.schema.txt")
        };
        archive.write(schema_path, source.show_create(table)?)?;
        if !table.is_view {
            schema_meta.push_str(&format!("{};{}\n", table.database, table.table));
        }
        let replica = source.tiflash_replica(table)?;
        if !replica.is_empty() {
            tiflash.push_str(&replica);
            tiflash.push('\n');
        }
        if !table.is_view {
            archive.write(
                format!("statsMem/{safe}.txt"),
                source.stats_memory_status(table)?,
            )?;
        }
        if !table.is_view && !skip_stats {
            let (stats, fallback) = source.stats(table, task.historical_stats_ts)?;
            archive.write(format!("stats/{safe}.json"), stats)?;
            if let Some(message) = fallback {
                errors.push(message);
            }
        }
    }
    archive.write(
        format!("schema/{PLAN_REPLAYER_SCHEMA_META_FILE}"),
        schema_meta,
    )?;
    archive.write(PLAN_REPLAYER_TIFLASH_REPLICAS_FILE, tiflash)?;
    let variables = task
        .session_variables
        .iter()
        .map(|(name, value)| format!("{name:?} = {value:?}\n"))
        .collect::<String>();
    archive.write(PLAN_REPLAYER_VARIABLES_FILE, variables)?;
    for (index, sql) in task.statements.iter().enumerate() {
        archive.write(format!("sql/sql{index}.sql"), sql.clone())?;
    }
    archive.write(
        PLAN_REPLAYER_SESSION_BINDING_FILE,
        task.session_bindings
            .iter()
            .map(|binding| format!("{binding};\n"))
            .collect::<String>(),
    )?;
    archive.write(
        PLAN_REPLAYER_GLOBAL_BINDING_FILE,
        source
            .global_bindings()?
            .into_iter()
            .map(|binding| format!("{binding}\n"))
            .collect::<String>(),
    )?;
    // 无预编码计划则逐条 explain；否则解码后写入 explain/sql.txt。
    if task.encoded_plan.is_empty() {
        let mut debug = Vec::new();
        for (index, sql) in task.statements.iter().enumerate() {
            match source.explain(sql, task.analyze) {
                Ok((explain, trace)) => {
                    let path = if task.statements.len() == 1 {
                        "explain.txt".to_string()
                    } else {
                        format!("explain/explain{index}.txt")
                    };
                    archive.write(path, explain)?;
                    if let Some(trace) = trace {
                        debug.push(trace);
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        if debug.is_empty() {
            debug.push(String::new());
        }
        for (index, trace) in debug.into_iter().enumerate() {
            archive.write(format!("debug_trace/debug_trace{index}.json"), trace)?;
        }
    } else {
        archive.write("explain/sql.txt", source.decode_plan(&task.encoded_plan)?)?;
        let debug = if task.debug_trace.is_empty() {
            vec![String::new()]
        } else {
            task.debug_trace.clone()
        };
        for (index, trace) in debug.into_iter().enumerate() {
            archive.write(format!("debug_trace/debug_trace{index}.json"), trace)?;
        }
    }
    if !errors.is_empty() {
        archive.write(PLAN_REPLAYER_ERROR_MESSAGE_FILE, errors.join("\n") + "\n")?;
    }
    let records = task
        .statements
        .iter()
        .map(|sql| PlanReplayerStatusRecord {
            sql_digest: if task.encoded_plan.is_empty() {
                String::new()
            } else {
                task.key.sql_digest.clone()
            },
            plan_digest: if task.encoded_plan.is_empty() {
                String::new()
            } else {
                task.key.plan_digest.clone()
            },
            origin_sql: sql.clone(),
            token: task.file_name.clone(),
            failed_reason: String::new(),
        })
        .collect();
    Ok((archive, records))
}

/// 轻量抽取 SQL 中的表引用（库名, 表名）。
///
/// 识别 `FROM`/`JOIN`/`UPDATE`/`INTO` 后的标识符；忽略 CTE（`name AS (...)`）名称。
/// 无库名前缀时使用 `current_database`。
pub fn extract_table_references(sql: &str, current_database: &str) -> BTreeSet<(String, String)> {
    let cleaned = sql
        .replace(',', " , ")
        .replace('(', " ( ")
        .replace(')', " ) ");
    let tokens = cleaned.split_whitespace().collect::<Vec<_>>();
    // 先收集 CTE 名，后续表引用中过滤掉。
    let mut ctes = BTreeSet::new();
    for triple in tokens.windows(3) {
        if triple[1].eq_ignore_ascii_case("as") && triple[2] == "(" {
            ctes.insert(trim_identifier(triple[0]).to_ascii_lowercase());
        }
    }
    let mut result = BTreeSet::new();
    for pair in tokens.windows(2) {
        if !(pair[0].eq_ignore_ascii_case("from")
            || pair[0].eq_ignore_ascii_case("join")
            || pair[0].eq_ignore_ascii_case("update")
            || pair[0].eq_ignore_ascii_case("into"))
        {
            continue;
        }
        let raw = trim_identifier(pair[1]);
        if raw.is_empty() || raw == "(" || ctes.contains(&raw.to_ascii_lowercase()) {
            continue;
        }
        let (database, table) = raw
            .split_once('.')
            .map(|(db, table)| {
                (
                    trim_identifier(db).to_string(),
                    trim_identifier(table).to_string(),
                )
            })
            .unwrap_or_else(|| (current_database.to_string(), raw.to_string()));
        if !database.is_empty() && !table.is_empty() {
            result.insert((database, table));
        }
    }
    result
}

/// 去掉标识符两端的引号与常见分隔符。
fn trim_identifier(value: &str) -> &str {
    value.trim_matches(|ch: char| matches!(ch, '`' | '"' | '\'' | ';' | ','))
}
/*

impl ast::Visitor for tableNameExtractor {
    // Enter 对应 Go Visitor：TableName 节点继续进入，其它节点也保持遍历。
    fn Enter(&mut self, input: ast::Node) -> (ast::Node, bool) {
        if input.is::<ast::TableName>() {
            return (input, true);
        }
        (input, false)
    }

    // Leave 对应 Go Visitor：在离开 TableName / SelectStmt 时收集表名或 CTE 名。
    fn Leave(&mut self, input: ast::Node) -> (ast::Node, bool) {
        if self.err.is_some() {
            return (input, true);
        }
        if let Some(t) = input.downcast_ref::<ast::TableName>() {
            match self.handleIsView(t) {
                Ok(isView) => {
                    let mut schema = t.Schema.clone();
                    if schema.L.is_empty() {
                        schema = self.curDB.clone();
                    }
                    if self.is.TableExists(schema.clone(), t.Name.clone()) {
                        self.names.insert(tableNamePair { DBName: schema.L, TableName: t.Name.L.clone(), IsView: isView }, ());
                    }
                }
                Err(err) => {
                    self.err = Some(err);
                    return (input, true);
                }
            }
        } else if let Some(s) = input.downcast_ref::<ast::SelectStmt>() {
            if let Some(with_) = &s.With {
                for cte in &with_.CTEs {
                    self.cteNames.insert(cte.Name.L.clone(), ());
                }
            }
        }
        (input, true)
    }
}

// DumpPlanReplayerInfo will dump the information about sqls.
// The files will be organized into the same zip layout as Go:
// sql_meta.toml、meta.txt、schema/、view/、stats/、statsMem/、config.toml、variables.toml、bindings、sql 和 explain。
pub fn DumpPlanReplayerInfo(
    ctx: context::Context,
    sctx: sessionctx::Context,
    task: &mut PlanReplayerDumpTask,
) -> Result<(), errors::Error> {
    let mut zf = task.Zf.take().unwrap();
    let fileName = task.FileName.clone();
    let sessionVars = unsafe { &mut *task.SessionVars };
    let execStmts = task.ExecStmts.clone();
    let mut zw = zip::NewWriter(zf.as_mut());
    let mut records: Vec<PlanReplayerStatusRecord> = Vec::new();
    let mut errMsgs: Vec<String> = Vec::new();
    let mut sqls = Vec::new();
    for execStmt in &task.ExecStmts {
        sqls.push(execStmt.Text());
    }

    if task.IsCapture {
        logutil::BgLogger().Info("start to dump plan replayer result", zap::String("category", "plan-replayer-dump"), zap::String("sql-digest", &task.SQLDigest), zap::String("plan-digest", &task.PlanDigest), zap::Strings("sql", sqls.clone()), zap::Bool("isContinues", task.IsContinuesCapture));
    } else {
        logutil::BgLogger().Info("start to dump plan replayer result", zap::String("category", "plan-replayer-dump"), zap::Strings("sqls", sqls.clone()));
    }

    // Go defer 负责关闭 zip writer/file、汇总错误并写入 status；用闭包保留资源收尾语义。
    let mut finish = |result: &Result<(), errors::Error>, records: &mut Vec<PlanReplayerStatusRecord>| {
        let mut errMsg = String::new();
        if let Err(err) = result {
            if task.IsCapture {
                logutil::BgLogger().Info("dump file failed", zap::String("category", "plan-replayer-dump"), zap::String("sql-digest", &task.SQLDigest), zap::String("plan-digest", &task.PlanDigest), zap::Strings("sql", sqls.clone()), zap::Bool("isContinues", task.IsContinuesCapture));
            } else {
                logutil::BgLogger().Info("start to dump plan replayer result", zap::String("category", "plan-replayer-dump"), zap::Strings("sqls", sqls.clone()));
            }
            errMsg = err.Error();
            domain_metrics::PlanReplayerDumpTaskFailed.Inc();
        } else {
            domain_metrics::PlanReplayerDumpTaskSuccess.Inc();
        }
        if let Err(err1) = zw.Close() {
            logutil::BgLogger().Warn("Closing zip writer failed", zap::String("category", "plan-replayer-dump"), zap::Error(err1.clone()), zap::String("filename", &fileName));
            errMsg.push_str(&format!(",{}", err1.Error()));
        }
        if let Err(err2) = zf.Close() {
            logutil::BgLogger().Warn("Closing zip file failed", zap::String("category", "plan-replayer-dump"), zap::Error(err2.clone()), zap::String("filename", &fileName));
            errMsg.push_str(&format!(",{}", err2.Error()));
        }
        if !errMsg.is_empty() {
            for record in records.iter_mut() {
                record.FailedReason = errMsg.clone();
            }
        }
        insertPlanReplayerStatus(ctx.clone(), sctx.clone(), records.clone());
    };

    let result = (|| -> Result<(), errors::Error> {
        dumpSQLMeta(&mut zw, task)?;
        dumpConfig(&mut zw)?;
        dumpMeta(&mut zw)?;

        let dbName = ast::NewCIStr(&sessionVars.CurrentDB());
        let do_ = GetDomain(sctx.clone());
        let pairs = extractTableNames(ctx.clone(), sctx.clone(), execStmts.clone(), dbName)
            .map_err(|err| errors::AddStack(fmt::Errorf(format!("plan replayer: invalid SQL text, err: {}", err))))?;

        dumpSchemas(sctx.clone(), &mut zw, pairs.clone())?;
        dumpTiFlashReplica(sctx.clone(), &mut zw, pairs.clone())?;

        if task.IsCapture && task.IsContinuesCapture {
            if !vardef::EnableHistoricalStatsForCapture.Load() {
                let fallbackMsg = dumpStats(&mut zw, pairs.clone(), do_.clone(), 0)?;
                if !fallbackMsg.is_empty() {
                    errMsgs.push(fallbackMsg);
                }
            } else {
                failpoint::Inject("shouldDumpStats", |val| {
                    if val.(bool) {
                        panic!("shouldDumpStats");
                    }
                });
            }
        } else {
            let fallbackMsg = dumpStats(&mut zw, pairs.clone(), do_.clone(), task.HistoricalStatsTS)?;
            if !fallbackMsg.is_empty() {
                errMsgs.push(fallbackMsg);
            }
        }

        dumpStatsMemStatus(&mut zw, pairs.clone(), do_.clone())?;
        dumpVariables(sctx.clone(), sessionVars, &mut zw)?;
        dumpSQLs(execStmts.clone(), &mut zw)?;

        if !task.SessionBindings.is_empty() {
            dumpSessionBindRecords(task.SessionBindings.clone(), &mut zw)?;
        } else {
            dumpSessionBindings(sctx.clone(), &mut zw)?;
        }
        dumpGlobalBindings(sctx.clone(), &mut zw)?;

        if !task.EncodedPlan.is_empty() {
            records = generateRecords(ctx.clone(), task);
            dumpEncodedPlan(sctx.clone(), &mut zw, &task.EncodedPlan)?;
        } else if let Err(err) = dumpPlanReplayerExplain(ctx.clone(), sctx.clone(), &mut zw, task, &mut records) {
            // Go 对 explain 错误只记录到 errors.txt，不直接中断整个 dump。
            errMsgs.push(err.Error());
        }

        dumpDebugTrace(&mut zw, task.DebugTrace.clone())?;
        if !errMsgs.is_empty() {
            dumpErrorMsgs(&mut zw, errMsgs)?;
        }
        Ok(())
    })();

    finish(&result, &mut records);
    result
}

// generateRecords 对应 Go 函数：为 encoded plan 场景生成 status 表记录。
pub fn generateRecords(ctx: context::Context, task: &mut PlanReplayerDumpTask) -> Vec<PlanReplayerStatusRecord> {
    let mut records = Vec::new();
    setTaskPresignedURL(ctx, task);
    if !task.ExecStmts.is_empty() {
        for execStmt in &task.ExecStmts {
            records.push(PlanReplayerStatusRecord {
                SQLDigest: task.SQLDigest.clone(),
                PlanDigest: task.PlanDigest.clone(),
                OriginSQL: execStmt.Text(),
                Token: task.FileName.clone(),
                FailedReason: String::new(),
            });
        }
    }
    records
}

// setTaskPresignedURL 对应 Go 函数：手动 dump 任务才需要生成预签名下载 URL。
pub fn setTaskPresignedURL(ctx: context::Context, task: &mut PlanReplayerDumpTask) {
    if task.IsCapture {
        return;
    }
    match getPresignedURL(ctx, task) {
        Ok(url) => task.PresignedURL = url,
        Err(err) => logutil::BgLogger().Warn("failed to get plan replayer presigned URL", zap::String("category", "plan-replayer-dump"), zap::Error(err), zap::String("filename", &task.FileName)),
    }
}

// PlanReplayerPresignExpire is how long a plan replayer presigned download URL stays valid.
pub const PlanReplayerPresignExpire: time::Duration = time::Hour;

// getPresignedURL 对应 Go 函数：从全局外部存储为 dump 文件生成下载地址。
pub fn getPresignedURL(ctx: context::Context, task: &PlanReplayerDumpTask) -> Result<String, errors::Error> {
    let storage = extstore::GetGlobalExtStorage(ctx.clone())?;
    storage.PresignFile(ctx, filepath::Join(replayer::GetPlanReplayerDirName(), &task.FileName), PlanReplayerPresignExpire)
}

// dumpSQLMeta 对应 Go 函数：写 sql_meta.toml。
pub fn dumpSQLMeta(zw: &mut zip::Writer, task: &PlanReplayerDumpTask) -> Result<(), errors::Error> {
    let cf = zw.Create(PlanReplayerSQLMetaFile).map_err(errors::AddStack)?;
    let mut varMap = std::collections::HashMap::new();
    varMap.insert(PlanReplayerSQLMetaStartTS.to_string(), strconv::FormatUint(task.StartTS, 10));
    varMap.insert(PlanReplayerTaskMetaIsCapture.to_string(), strconv::FormatBool(task.IsCapture));
    varMap.insert(PlanReplayerTaskMetaIsContinues.to_string(), strconv::FormatBool(task.IsContinuesCapture));
    varMap.insert(PlanReplayerTaskMetaSQLDigest.to_string(), task.SQLDigest.clone());
    varMap.insert(PlanReplayerTaskMetaPlanDigest.to_string(), task.PlanDigest.clone());
    varMap.insert(PlanReplayerTaskEnableHistoricalStats.to_string(), strconv::FormatBool(vardef::EnableHistoricalStatsForCapture.Load()));
    if task.HistoricalStatsTS > 0 {
        varMap.insert(PlanReplayerHistoricalStatsTS.to_string(), strconv::FormatUint(task.HistoricalStatsTS, 10));
    }
    toml::NewEncoder(cf).Encode(varMap).map_err(errors::AddStack)
}

// dumpConfig 对应 Go 函数：写入当前全局 config。
pub fn dumpConfig(zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let cf = zw.Create(PlanReplayerConfigFile).map_err(errors::AddStack)?;
    toml::NewEncoder(cf).Encode(config::GetGlobalConfig()).map_err(errors::AddStack)
}

// dumpMeta 对应 Go 函数：写入 TiDB 版本和构建信息。
pub fn dumpMeta(zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let mut mt = zw.Create(PlanReplayerMetaFile).map_err(errors::AddStack)?;
    mt.Write(printer::GetTiDBInfo().as_bytes()).map_err(errors::AddStack)?;
    Ok(())
}

// dumpTiFlashReplica 对应 Go 函数：为每个表写出 TiFlash replica 数量。
pub fn dumpTiFlashReplica(sctx: sessionctx::Context, zw: &mut zip::Writer, pairs: std::collections::HashMap<tableNamePair, ()>) -> Result<(), errors::Error> {
    let mut bf = zw.Create(PlanReplayerTiFlashReplicasFile).map_err(errors::AddStack)?;
    let is = GetDomain(sctx).InfoSchema();
    let ctx = infoschema::WithRefillOption(context::Background(), false);
    for pair in pairs.keys() {
        let dbName = ast::NewCIStr(&pair.DBName);
        let tableName = ast::NewCIStr(&pair.TableName);
        let t = match is.TableByName(ctx.clone(), dbName.clone(), tableName.clone()) {
            Ok(t) => t,
            Err(err) => {
                logutil::BgLogger().Warn("failed to find table info", zap::Error(err), zap::String("dbName", &dbName.L), zap::String("tableName", &tableName.L));
                continue;
            }
        };
        if t.Meta().TiFlashReplica.is_some() && t.Meta().TiFlashReplica.Count > 0 {
            let row = vec![pair.DBName.clone(), pair.TableName.clone(), strconv::FormatUint(t.Meta().TiFlashReplica.Count, 10)];
            fmt::Fprintf(&mut bf, "%s\n", strings::Join(row, "\t"))?;
        }
    }
    Ok(())
}

// dumpSchemas 对应 Go 函数：写 schema/view 文件，并为普通表额外写 schema_meta.txt。
pub fn dumpSchemas(ctx: sessionctx::Context, zw: &mut zip::Writer, pairs: std::collections::HashMap<tableNamePair, ()>) -> Result<(), errors::Error> {
    let mut tables = std::collections::HashMap::new();
    for pair in pairs.keys() {
        getShowCreateTable(pair.clone(), zw, ctx.clone())?;
        if !pair.IsView {
            tables.insert(pair.clone(), ());
        }
    }
    dumpSchemaMeta(zw, tables)
}

// dumpSchemaMeta 对应 Go 函数：写 schema/<schema_meta.txt> 中的 db.table 列表。
pub fn dumpSchemaMeta(zw: &mut zip::Writer, tables: std::collections::HashMap<tableNamePair, ()>) -> Result<(), errors::Error> {
    let mut zf = zw.Create(format!("schema/{}", PlanReplayerSchemaMetaFile))?;
    for table in tables.keys() {
        fmt::Fprintf(&mut zf, "%s.%s;", table.DBName.clone(), table.TableName.clone())?;
    }
    Ok(())
}

// dumpStatsMemStatus 对应 Go 函数：写每个表的内存 stats 状态。
pub fn dumpStatsMemStatus(zw: &mut zip::Writer, pairs: std::collections::HashMap<tableNamePair, ()>, do_: *mut Domain) -> Result<(), errors::Error> {
    let statsHandle = unsafe { (*do_).StatsHandle() };
    let is = unsafe { (*do_).InfoSchema() };
    let ctx = infoschema::WithRefillOption(context::Background(), false);
    for pair in pairs.keys() {
        if pair.IsView {
            continue;
        }
        let tbl = is.TableByName(ctx.clone(), ast::NewCIStr(&pair.DBName), ast::NewCIStr(&pair.TableName))?;
        let tblStats = statsHandle.GetPhysicalTableStats(tbl.Meta().ID, tbl.Meta());
        if tblStats.is_none() {
            continue;
        }
        let tblStats = tblStats.unwrap();
        let mut statsMemFw = zw.Create(format!("statsMem/{}.{}.txt", pair.DBName, pair.TableName)).map_err(errors::AddStack)?;
        fmt::Fprintf(&mut statsMemFw, "[INDEX]\n")?;
        tblStats.ForEachIndexImmutable(|_id: i64, idx: *mut statistics::Index| {
            fmt::Fprintf(&mut statsMemFw, "%s\n", format!("{}={}", unsafe { (*idx).Info.Name.String() }, unsafe { (*idx).StatusToString() }));
            false
        });
        fmt::Fprintf(&mut statsMemFw, "[COLUMN]\n")?;
        tblStats.ForEachColumnImmutable(|_id: i64, c: *mut statistics::Column| {
            fmt::Fprintf(&mut statsMemFw, "%s\n", format!("{}={}", unsafe { (*c).Info.Name.String() }, unsafe { (*c).StatusToString() }));
            false
        });
    }
    Ok(())
}

// dumpStats 对应 Go 函数：写 stats/<db.table>.json，并汇总历史统计回退提示。
pub fn dumpStats(zw: &mut zip::Writer, pairs: std::collections::HashMap<tableNamePair, ()>, do_: *mut Domain, historyStatsTS: u64) -> Result<String, errors::Error> {
    let mut allFallBackTbls: Vec<String> = Vec::new();
    for pair in pairs.keys() {
        if pair.IsView {
            continue;
        }
        let (jsonTbl, fallBackTbls) = getStatsForTable(do_, pair.clone(), historyStatsTS)?;
        let mut statsFw = zw.Create(format!("stats/{}.{}.json", pair.DBName, pair.TableName)).map_err(errors::AddStack)?;
        let data = json::Marshal(jsonTbl).map_err(errors::AddStack)?;
        statsFw.Write(&data).map_err(errors::AddStack)?;
        allFallBackTbls.extend(fallBackTbls);
    }
    if !allFallBackTbls.is_empty() {
        return Ok(format!("Historical stats for {} are unavailable, fallback to latest stats", strings::Join(allFallBackTbls, ", ")));
    }
    Ok(String::new())
}

// dumpSQLs 对应 Go 函数：把每条原 SQL 写入 sql/sqlN.sql。
pub fn dumpSQLs(execStmts: Vec<ast::StmtNode>, zw: &mut zip::Writer) -> Result<(), errors::Error> {
    for (i, stmtExec) in execStmts.iter().enumerate() {
        let mut zf = zw.Create(format!("sql/sql{}.sql", i))?;
        zf.Write(stmtExec.Text().as_bytes())?;
    }
    Ok(())
}

// dumpVariables 对应 Go 函数：遍历系统变量，跳过 noop/SEM 隐藏变量，并写 variables.toml。
pub fn dumpVariables(sctx: sessionctx::Context, sessionVars: &mut variable::SessionVars, zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let mut varMap = std::collections::HashMap::new();
    for v in variable::GetSysVars() {
        if v.IsNoop && !vardef::EnableNoopVariables.Load() {
            continue;
        }
        if infoschema::SysVarHiddenForSem(sctx.clone(), &v.Name) {
            continue;
        }
        let value = sessionVars.GetSessionOrGlobalSystemVar(context::Background(), &v.Name).map_err(errors::Trace)?;
        varMap.insert(v.Name, value);
    }
    let vf = zw.Create(PlanReplayerVariablesFile).map_err(errors::AddStack)?;
    toml::NewEncoder(vf).Encode(varMap).map_err(errors::AddStack)
}

// dumpSessionBindRecords 对应 Go 函数：把任务携带的 session binding 记录写入制表符分隔文件。
pub fn dumpSessionBindRecords(records: Vec<Vec<*mut bindinfo::Binding>>, zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let mut sRows: Vec<Vec<String>> = Vec::new();
    for bindData in records {
        for hint in bindData {
            unsafe {
                sRows.push(vec![
                    (*hint).OriginalSQL.clone(),
                    (*hint).BindSQL.clone(),
                    (*hint).Db.clone(),
                    (*hint).Status.clone(),
                    (*hint).CreateTime.String(),
                    (*hint).UpdateTime.String(),
                    (*hint).Charset.clone(),
                    (*hint).Collation.clone(),
                    (*hint).Source.clone(),
                ]);
            }
        }
    }
    let mut bf = zw.Create(PlanReplayerSessionBindingFile).map_err(errors::AddStack)?;
    for row in sRows {
        fmt::Fprintf(&mut bf, "%s\n", strings::Join(row, "\t"))?;
    }
    Ok(())
}

// dumpSessionBindings 对应 Go 函数：执行 show bindings 并写 session_bindings.sql。
pub fn dumpSessionBindings(ctx: sessionctx::Context, zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let recordSets = ctx.GetSQLExecutor().Execute(context::Background(), "show bindings")?;
    let sRows = resultSetToStringSlice(context::Background(), recordSets[0].clone(), true)?;
    let mut bf = zw.Create(PlanReplayerSessionBindingFile).map_err(errors::AddStack)?;
    for row in sRows {
        fmt::Fprintf(&mut bf, "%s\n", strings::Join(row, "\t"))?;
    }
    if !recordSets.is_empty() {
        // Go 在写完后显式 Close 第一个 record set，这里保留资源释放分支。
        recordSets[0].Close()?;
    }
    Ok(())
}

// dumpGlobalBindings 对应 Go 函数：执行 show global bindings 并写 global_bindings.sql。
pub fn dumpGlobalBindings(ctx: sessionctx::Context, zw: &mut zip::Writer) -> Result<(), errors::Error> {
    let recordSets = ctx.GetSQLExecutor().Execute(context::Background(), "show global bindings")?;
    let sRows = resultSetToStringSlice(context::Background(), recordSets[0].clone(), false)?;
    let mut bf = zw.Create(PlanReplayerGlobalBindingFile).map_err(errors::AddStack)?;
    for row in sRows {
        fmt::Fprintf(&mut bf, "%s\n", strings::Join(row, "\t"))?;
    }
    if !recordSets.is_empty() {
        recordSets[0].Close()?;
    }
    Ok(())
}

// dumpEncodedPlan 对应 Go 函数：通过 tidb_decode_plan 解码 encoded plan 并写 explain/sql.txt。
pub fn dumpEncodedPlan(ctx: sessionctx::Context, zw: &mut zip::Writer, encodedPlan: &str) -> Result<(), errors::Error> {
    let recordSets = ctx.GetSQLExecutor().Execute(context::Background(), format!("select tidb_decode_plan('{}')", encodedPlan))?;
    let sRows = resultSetToStringSlice(context::Background(), recordSets[0].clone(), false)?;
    let mut fw = zw.Create("explain/sql.txt").map_err(errors::AddStack)?;
    for row in sRows {
        fmt::Fprintf(&mut fw, "%s\n", strings::Join(row, "\t"))?;
    }
    if !recordSets.is_empty() {
        recordSets[0].Close()?;
    }
    Ok(())
}

// dumpExplain 对应 Go 函数：执行 explain 或 explain analyze，并按单/多 SQL 写入不同文件。
pub fn dumpExplain(ctx: sessionctx::Context, zw: &mut zip::Writer, isAnalyze: bool, sqls: Vec<String>, emptyAsNil: bool) -> Result<Vec<Box<dyn std::any::Any>>, errors::Error> {
    ctx.GetSessionVars().InPlanReplayer = true;
    let _guard = scopeguard::guard(ctx.clone(), |ctx| {
        ctx.GetSessionVars().InPlanReplayer = false;
    });

    let useSeparateFiles = sqls.len() > 1;
    let mut fw: Option<Box<dyn io::Writer>> = None;
    if !useSeparateFiles && !sqls.is_empty() {
        fw = Some(Box::new(zw.Create("explain.txt").map_err(errors::AddStack)?));
    }

    for (i, sql) in sqls.iter().enumerate() {
        if useSeparateFiles {
            fw = Some(Box::new(zw.Create(format!("explain/explain{}.txt", i)).map_err(errors::AddStack)?));
        }
        let explainSQL = if isAnalyze {
            format!("explain analyze {}", sql)
        } else {
            format!("explain {}", sql)
        };
        let recordSets = ctx.GetSQLExecutor().Execute(context::Background(), explainSQL)?;
        let sRows = resultSetToStringSlice(context::Background(), recordSets[0].clone(), emptyAsNil)?;
        for row in sRows {
            fmt::Fprintf(fw.as_mut().unwrap(), "%s\n", strings::Join(row, "\t"))?;
        }
        if !recordSets.is_empty() {
            recordSets[0].Close()?;
        }
        if !useSeparateFiles && i < sqls.len() - 1 {
            fmt::Fprintf(fw.as_mut().unwrap(), "<--------->\n")?;
        }
    }
    Ok(Vec::new())
}

// dumpPlanReplayerExplain 对应 Go 函数：收集 SQL、生成 status record，并调用 dumpExplain。
pub fn dumpPlanReplayerExplain(
    ctx: context::Context,
    sctx: sessionctx::Context,
    zw: &mut zip::Writer,
    task: &mut PlanReplayerDumpTask,
    records: &mut Vec<PlanReplayerStatusRecord>,
) -> Result<(), errors::Error> {
    setTaskPresignedURL(ctx, task);
    let mut sqls = Vec::new();
    for execStmt in &task.ExecStmts {
        let sql = execStmt.Text();
        sqls.push(sql.clone());
        records.push(PlanReplayerStatusRecord {
            SQLDigest: String::new(),
            PlanDigest: String::new(),
            OriginSQL: sql,
            Token: task.FileName.clone(),
            FailedReason: String::new(),
        });
    }
    let debugTraces = dumpExplain(sctx, zw, task.Analyze, sqls, false)?;
    task.DebugTrace = debugTraces;
    Ok(())
}

// extractTableNames extracts table names from the given stmts.
pub fn extractTableNames(
    ctx: context::Context,
    sctx: sessionctx::Context,
    execStmts: Vec<ast::StmtNode>,
    curDB: ast::CIStr,
) -> Result<std::collections::HashMap<tableNamePair, ()>, errors::Error> {
    let mut tableExtractor = tableNameExtractor {
        ctx,
        executor: sctx.GetRestrictedSQLExecutor(),
        is: GetDomain(sctx).InfoSchema(),
        curDB,
        names: std::collections::HashMap::new(),
        cteNames: std::collections::HashMap::new(),
        err: None,
    };
    for execStmt in execStmts {
        execStmt.Accept(&mut tableExtractor);
    }
    if let Some(err) = tableExtractor.err {
        return Err(err);
    }
    tableExtractor.getTablesAndViews()
}

// getStatsForTable 对应 Go 函数：按 historyStatsTS 选择历史统计或最新统计导出路径。
pub fn getStatsForTable(do_: *mut Domain, pair: tableNamePair, historyStatsTS: u64) -> Result<(*mut statistics_util::JSONTable, Vec<String>), errors::Error> {
    let is = unsafe { (*do_).InfoSchema() };
    let h = unsafe { (*do_).StatsHandle() };
    let tbl = is.TableByName(context::Background(), ast::NewCIStr(&pair.DBName), ast::NewCIStr(&pair.TableName))?;
    if historyStatsTS > 0 {
        return h.DumpHistoricalStatsBySnapshot(&pair.DBName, tbl.Meta(), historyStatsTS);
    }
    let jt = h.DumpStatsToJSON(&pair.DBName, tbl.Meta(), None, true)?;
    Ok((jt, Vec::new()))
}

// getShowCreateTable 对应 Go 函数：执行 show create table，并按 view/table 写到不同目录。
pub fn getShowCreateTable(pair: tableNamePair, zw: &mut zip::Writer, ctx: sessionctx::Context) -> Result<(), errors::Error> {
    let recordSets = ctx.GetSQLExecutor().Execute(context::Background(), format!("show create table `{}`.`{}`", pair.DBName, pair.TableName))?;
    let sRows = resultSetToStringSlice(context::Background(), recordSets[0].clone(), false)?;
    let mut fw: Box<dyn io::Writer>;
    if pair.IsView {
        fw = Box::new(zw.Create(format!("view/{}.{}.view.txt", pair.DBName, pair.TableName)).map_err(errors::AddStack)?);
        if sRows.is_empty() || sRows[0].len() != 4 {
            return Err(fmt::Errorf(format!("plan replayer: get create view {}.{} failed", pair.DBName, pair.TableName)));
        }
    } else {
        fw = Box::new(zw.Create(format!("schema/{}.{}.schema.txt", pair.DBName, pair.TableName)).map_err(errors::AddStack)?);
        if sRows.is_empty() || sRows[0].len() != 2 {
            return Err(fmt::Errorf(format!("plan replayer: get create table {}.{} failed", pair.DBName, pair.TableName)));
        }
    }
    fmt::Fprintf(&mut fw, "create database if not exists `{}`; use `{}`;", pair.DBName, pair.DBName)?;
    fmt::Fprintf(&mut fw, "{}", sRows[0][1].clone())?;
    if !recordSets.is_empty() {
        recordSets[0].Close()?;
    }
    Ok(())
}

// resultSetToStringSlice 对应 Go 函数：把 RecordSet 的所有行转为字符串矩阵。
pub fn resultSetToStringSlice(ctx: context::Context, rs: sqlexec::RecordSet, emptyAsNil: bool) -> Result<Vec<Vec<String>>, errors::Error> {
    let rows = getRows(ctx, rs.clone())?;
    rs.Close()?;
    let mut sRows = vec![Vec::new(); rows.len()];
    for (i, row) in rows.iter().enumerate() {
        let mut iRow = Vec::with_capacity(row.Len());
        for j in 0..row.Len() {
            if row.IsNull(j) {
                iRow.push("<nil>".to_string());
            } else {
                let d = row.GetDatum(j, &rs.Fields()[j].Column.FieldType);
                let mut s = d.ToString()?;
                if s.len() < 1 && emptyAsNil {
                    // Go 的 emptyAsNil 用于 explain 输出，空字符串按 <nil> 展示。
                    s = "<nil>".to_string();
                }
                iRow.push(s);
            }
        }
        sRows[i] = iRow;
    }
    Ok(sRows)
}

// getRows 对应 Go 函数：循环 RecordSet.Next，并复制 chunk 中的每一行。
pub fn getRows(ctx: context::Context, rs: sqlexec::RecordSet) -> Result<Vec<chunk::Row>, errors::Error> {
    if rs.is_nil() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    let mut req = rs.NewChunk(None);
    loop {
        rs.Next(ctx.clone(), &mut req)?;
        if req.NumRows() == 0 {
            break;
        }
        // 必须复用 req 来模仿 server.(*clientConn).writeChunks；CopyConstruct 后再遍历。
        let iter = chunk::NewIterator4Chunk(req.CopyConstruct());
        let mut row = iter.Begin();
        while row != iter.End() {
            rows.push(row.clone());
            row = iter.Next();
        }
    }
    Ok(rows)
}

// dumpDebugTrace 对应 Go 函数：没有 trace 时也写一个空文件，保持兼容性。
pub fn dumpDebugTrace(zw: &mut zip::Writer, mut debugTraces: Vec<Box<dyn std::any::Any>>) -> Result<(), errors::Error> {
    if debugTraces.is_empty() {
        debugTraces.push(Box::new(()));
    }
    for (i, trace) in debugTraces.into_iter().enumerate() {
        let mut fw = zw.Create(format!("debug_trace/debug_trace{}.json", i)).map_err(errors::AddStack)?;
        dumpOneDebugTrace(&mut fw, trace).map_err(errors::AddStack)?;
    }
    Ok(())
}

// dumpOneDebugTrace 对应 Go 函数：JSON 编码单个 debug trace，并关闭 HTML escaping。
pub fn dumpOneDebugTrace(w: &mut dyn io::Writer, debugTrace: Box<dyn std::any::Any>) -> Result<(), errors::Error> {
    if debugTrace.is::<()>() {
        return Ok(());
    }
    let mut jsonEncoder = json::NewEncoder(w);
    // 不关闭 HTML escaping 时，">"、"<"、"&" 会被编码成 \u003c 等，Go 代码显式禁用。
    jsonEncoder.SetEscapeHTML(false);
    jsonEncoder.Encode(debugTrace)
}

// dumpErrorMsgs 对应 Go 函数：把 dump 过程中收集的非致命错误写入 errors.txt。
pub fn dumpErrorMsgs(zw: &mut zip::Writer, msgs: Vec<String>) -> Result<(), errors::Error> {
    let mut mt = zw.Create(PlanReplayerErrorMessageFile).map_err(errors::AddStack)?;
    for msg in msgs {
        mt.Write(msg.as_bytes()).map_err(errors::AddStack)?;
        mt.Write(&[b'\n']).map_err(errors::AddStack)?;
    }
    Ok(())
}
*/
