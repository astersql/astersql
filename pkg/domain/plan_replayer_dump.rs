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
