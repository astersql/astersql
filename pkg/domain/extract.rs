// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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
// limitations under the License.

// Extract Plan：从语句摘要收集 SQL/执行计划并打包导出。
//
// 语句摘要（statement summary）汇总近期 SQL 的指纹（digest）与二进制计划；
// 本模块解析涉及的表/视图，过滤内部库，解码 binary plan，并生成 zip 产物，
// 便于离线复现与诊断。下方大段注释保留 Go 侧机械翻译草稿；可执行实现自
// `EXTRACT_META_FILE` 起。

// extract plan 任务如何从 stmt summary 收集 SQL、解析表/视图、打包 schema/stats/SQL 到 zip。
//
// use std::collections::{HashMap, HashSet};
//
// ExtractMetaFile indicates meta file for extract
// pub const ExtractMetaFile: &str = "extract_meta.txt";
//
// ExtractTaskType indicates type of extract task
// pub const ExtractTaskType: &str = "taskType";
// ExtractPlanTaskSkipStats indicates skip stats for extract plan task
// pub const ExtractPlanTaskSkipStats: &str = "SkipStats";
// ExtractTaskDirName indicates directory name for extract task
// pub const ExtractTaskDirName: &str = "extract";
//
// ExtractType indicates type
// ExtractType 对应 Go 的 uint8 枚举；当前只迁移 ExtractPlanType。
// pub enum ExtractType {
//     ExtractPlanType = 0,
// }
//
// taskTypeToString 对应 Go 的同名函数，把 extract 类型转换成外部元数据字符串。
// pub fn taskTypeToString(t: ExtractType) -> &'static str {
//     if matches!(t, ExtractType::ExtractPlanType) {
//         return "Plan";
//     }
//     "Unknown"
// }
//
// ExtractHandle handles the extractWorker to run extract the information task like Plan or any others.
// extractHandle will provide 2 mode for extractWorker:
// 1. submit a background extract task, the response will be returned after the task is started to be solved
// 2. submit a task and wait until the task is solved, the result will be returned to the response.
// ExtractHandle 对应 Go 的同名结构体，当前只持有一个 extractWorker。
// pub struct ExtractHandle {
//     pub worker: Box<extractWorker>,
// }
//
// newExtractHandler new extract handler
// newExtractHandler 对应 Go 构造函数：使用第一个 session context 创建 worker。
// pub fn newExtractHandler(
//     ctx: context::Context,
//     sctxs: Vec<sessionctx::Context>,
// ) -> ExtractHandle {
//     let worker = newExtractWorker(ctx, sctxs[0].clone(), false);
//     ExtractHandle {
//         worker: Box::new(worker),
//     }
// }
//
// impl ExtractHandle {
// ExtractTask extract tasks
// ExtractTask 对应 Go 方法：后台任务当前尚未支持，非后台任务交给 worker 同步执行。
//     pub fn ExtractTask(
//         &mut self,
//         ctx: context::Context,
//         task: &mut ExtractTask,
//     ) -> Result<String, errors::Error> {
// TODO: support background job later
//         if task.IsBackgroundJob {
//             return Ok(String::new());
//         }
//         self.worker.extractTask(ctx, task)
//     }
// }
//
// extractWorker 对应 Go 的内部 worker，保留 context、session、后台标记和互斥锁字段。
// pub struct extractWorker {
//     pub ctx: context::Context,
//     pub sctx: sessionctx::Context,
//     pub isBackgroundWorker: bool,
//     pub mutex: sync::Mutex<()>,
// }
//
// ExtractTask indicates task
// ExtractTask 对应 Go 任务参数结构，字段顺序保持一致。
// pub struct ExtractTask {
//     pub ExtractType: ExtractType,
//     pub IsBackgroundJob: bool,
//
// Param for Extract Plan
//     pub SkipStats: bool,
//     pub UseHistoryView: bool,
//
// variables for plan task type
//     pub Begin: time::Time,
//     pub End: time::Time,
// }
//
// NewExtractPlanTask returns extract plan task
// NewExtractPlanTask 对应 Go 构造函数，只填充时间范围和 ExtractPlanType。
// pub fn NewExtractPlanTask(begin: time::Time, end: time::Time) -> ExtractTask {
//     ExtractTask {
//         Begin: begin,
//         End: end,
//         ExtractType: ExtractType::ExtractPlanType,
//         IsBackgroundJob: false,
//         SkipStats: false,
//         UseHistoryView: false,
//     }
// }
//
// newExtractWorker 对应 Go 的内部 worker 构造函数。
// pub fn newExtractWorker(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     isBackgroundWorker: bool,
// ) -> extractWorker {
//     extractWorker {
//         ctx,
//         sctx,
//         isBackgroundWorker,
//         mutex: sync::Mutex::new(()),
//     }
// }
//
// impl extractWorker {
// extractTask 对应 Go 的任务分发；未知类型返回错误。
//     pub fn extractTask(
//         &mut self,
//         ctx: context::Context,
//         task: &mut ExtractTask,
//     ) -> Result<String, errors::Error> {
//         if matches!(task.ExtractType, ExtractType::ExtractPlanType) {
//             return self.extractPlanTask(ctx, task);
//         }
//         Err(errors::New("unknown extract task"))
//     }
//
// extractPlanTask 对应 Go 的 plan extract 主流程。
//     pub fn extractPlanTask(
//         &mut self,
//         ctx: context::Context,
//         task: &mut ExtractTask,
//     ) -> Result<String, errors::Error> {
//         if task.UseHistoryView
//             && !config::GetGlobalConfig()
//                 .Instance
//                 .StmtSummaryEnablePersistent
//         {
//             return Err(errors::New(
//                 "tidb_stmt_summary_enable_persistent should be enabled for extract task",
//             ));
//         }
//         let records = match self.collectRecords(ctx.clone(), task) {
//             Ok(records) => records,
//             Err(err) => {
//                 logutil::BgLogger().Error(
//                     "collect stmt summary records failed for extract plan task",
//                     zap::Error(err.clone()),
//                 );
//                 return Err(err);
//             }
//         };
//         let p = match self.packageExtractPlanRecords(ctx.clone(), records) {
//             Ok(p) => p,
//             Err(err) => {
//                 logutil::BgLogger().Error(
//                     "package stmt summary records failed for extract plan task",
//                     zap::Error(err.clone()),
//                 );
//                 return Err(err);
//             }
//         };
//         self.dumpExtractPlanPackage(ctx, task, p)
//     }
//
// collectRecords 对应 Go 中查询 stmt summary 并组装 digest/planDigest 去重 map 的逻辑。
//     pub fn collectRecords(
//         &mut self,
//         ctx: context::Context,
//         task: &ExtractTask,
//     ) -> Result<HashMap<stmtSummaryHistoryKey, stmtSummaryHistoryRecord>, errors::Error> {
// Go 使用 mutex + defer Unlock 防止并发收集；用 guard 体现作用域释放。
//         let _guard = self.mutex.lock();
//         let exec = self.sctx.GetRestrictedSQLExecutor();
//         let ctx1 = kv::WithInternalSourceType(ctx, kv::InternalTxnStatsForegroundPriority);
//         let mut sourceTable = "STATEMENTS_SUMMARY_HISTORY";
//         if !task.UseHistoryView {
//             sourceTable = "STATEMENTS_SUMMARY";
//         }
//         let sql = format!(
//             "SELECT STMT_TYPE, DIGEST, PLAN_DIGEST,QUERY_SAMPLE_TEXT, BINARY_PLAN, TABLE_NAMES, SAMPLE_USER FROM INFORMATION_SCHEMA.{} WHERE SUMMARY_END_TIME > '{}' AND SUMMARY_BEGIN_TIME < '{}'",
//             sourceTable,
//             task.Begin.Format(types::TimeFormat),
//             task.End.Format(types::TimeFormat)
//         );
//         let (rows, _, err) = exec.ExecRestrictedSQL(ctx1, None, sql);
//         if let Some(err) = err {
//             return Err(err);
//         }
//
//         let mut collectMap: HashMap<stmtSummaryHistoryKey, stmtSummaryHistoryRecord> =
//             HashMap::new();
//         for row in rows {
//             let mut record = stmtSummaryHistoryRecord::default();
//             record.stmtType = row.GetString(0);
//             record.digest = row.GetString(1);
//             record.planDigest = row.GetString(2);
//             record.sql = row.GetString(3);
//             record.binaryPlan = row.GetString(4);
//             let tableNames = row.GetString(5);
//             let key = stmtSummaryHistoryKey {
//                 digest: record.digest.clone(),
//                 planDigest: record.planDigest.clone(),
//             };
//             record.userName = row.GetString(6);
//             record.tables = Vec::new();
//             let setRecord = self.handleTableNames(&tableNames, &mut record)?;
//             if setRecord && checkRecordValid(&record) {
//                 collectMap.insert(key, record);
//             }
//         }
//         Ok(collectMap)
//     }
//
// handleTableNames 对应 Go 中解析 TABLE_NAMES 字段并过滤内部库、缺失表和非法格式。
//     pub fn handleTableNames(
//         &mut self,
//         tableNames: &str,
//         record: &mut stmtSummaryHistoryRecord,
//     ) -> Result<bool, errors::Error> {
//         let is = GetDomain(&self.sctx).InfoSchema();
//         for t in tableNames.split(',') {
//             let names: Vec<&str> = t.split('.').collect();
//             if names.len() != 2 {
//                 return Ok(false);
//             }
//             let dbName = names[0].to_string();
//             let tblName = names[1].to_string();
//             record.schemaName = dbName.clone();
// skip internal schema record
//             match record.schemaName.to_lowercase().as_str() {
//                 metadef::PerformanceSchemaName::L
//                 | metadef::InformationSchemaName::L
//                 | metadef::MetricSchemaName::L
//                 | "mysql" => return Ok(false),
//                 _ => {}
//             }
//             let exists = is.TableExists(ast::NewCIStr(&dbName), ast::NewCIStr(&tblName));
//             if !exists {
//                 return Ok(false);
//             }
//             let t = is.TableByName(self.ctx.clone(), ast::NewCIStr(&dbName), ast::NewCIStr(&tblName))?;
//             record.tables.push(tableNamePair {
//                 DBName: dbName,
//                 TableName: tblName,
//                 IsView: t.Meta().IsView(),
//             });
//         }
//         Ok(true)
//     }
//
// packageExtractPlanRecords 对应 Go 中把有效记录解码 plan 并收集涉及表集合的逻辑。
//     pub fn packageExtractPlanRecords(
//         &mut self,
//         ctx: context::Context,
//         records: HashMap<stmtSummaryHistoryKey, stmtSummaryHistoryRecord>,
//     ) -> Result<extractPlanPackage, errors::Error> {
//         let mut p = extractPlanPackage {
//             records,
//             tables: HashSet::new(),
//         };
//         for record in p.records.values_mut() {
// skip the sql which has been cut off
//             if record.sql.contains("(len:") {
//                 record.skip = true;
//                 continue;
//             }
//             let plan = self.decodeBinaryPlan(ctx.clone(), &record.binaryPlan)?;
//             record.plan = plan;
//             for tbl in &record.tables {
//                 p.tables.insert(tbl.clone());
//             }
//         }
//         self.handleIsView(ctx, &mut p)?;
//         Ok(p)
//     }
//
// handleIsView 对应 Go 中解析 view definition 并把视图依赖的真实表加入 package 的逻辑。
//     pub fn handleIsView(
//         &mut self,
//         ctx: context::Context,
//         p: &mut extractPlanPackage,
//     ) -> Result<(), errors::Error> {
//         let is = GetDomain(&self.sctx).InfoSchema();
//         let mut tne = tableNameExtractor {
//             ctx,
//             executor: self.sctx.GetRestrictedSQLExecutor(),
//             is,
//             curDB: ast::NewCIStr(""),
//             names: HashSet::new(),
//             cteNames: HashSet::new(),
//             err: None,
//         };
//         for v in p.tables.clone() {
//             if v.IsView {
//                 let table = is.TableByName(self.ctx.clone(), ast::NewCIStr(&v.DBName), ast::NewCIStr(&v.TableName))?;
//                 let sql = table.Meta().View.SelectStmt;
//                 let node = tne.executor.ParseWithParams(tne.ctx.clone(), sql)?;
// Go AST visitor 会在 Accept 中填充 tne.names/err；这里保留访问调用形状。
//                 node.Accept(&mut tne);
//             }
//         }
//         if let Some(err) = tne.err {
//             return Err(err);
//         }
//         let r = tne.getTablesAndViews()?;
//         for t in r {
//             p.tables.insert(t);
//         }
//         Ok(())
//     }
//
// decodeBinaryPlan 对应 Go 中调用 tidb_decode_binary_plan 并 trim 换行的逻辑。
//     pub fn decodeBinaryPlan(
//         &mut self,
//         ctx: context::Context,
//         bPlan: &str,
//     ) -> Result<String, errors::Error> {
//         let exec = self.sctx.GetRestrictedSQLExecutor();
//         let ctx1 = kv::WithInternalSourceType(ctx, kv::InternalTxnStatsForegroundPriority);
//         let (rows, _, err) = exec.ExecRestrictedSQL(
//             ctx1,
//             None,
//             format!("SELECT tidb_decode_binary_plan('{}')", bPlan),
//         );
//         if let Some(err) = err {
//             return Err(err);
//         }
//         let plan = rows[0].GetString(0);
//         Ok(plan.trim_matches('\n').to_string())
//     }
//
// dumpExtractPlanPackage will dump the information about sqls collected in stmt_summary_history
// The files will be organized into the following format:
//     /*
//      |-extract_meta.txt
//      |-meta.txt
//      |-config.toml
//      |-variables.toml
//      |-bindings.sql
//      |-schema
//      |   |-schema_meta.txt
//      |   |-db1.table1.schema.txt
//      |   |-db2.table2.schema.txt
//      |   |-....
//      |-view
//      |   |-db1.view1.view.txt
//      |   |-db2.view2.view.txt
//      |   |-....
//      |-stats
//      |   |-stats1.json
//      |   |-stats2.json
//      |   |-....
//      |-table_tiflash_replica.txt
//      |-sql
//      |   |-digest1.sql
//      |   |-digest2.sql
//      |   |-....
//      |-skippedSQLs
//      |   |-digest1.sql
//      |   |-...
//     */
// dumpExtractPlanPackage 对应 Go 中创建 zip writer 并顺序 dump 各类文件的逻辑。
//     pub fn dumpExtractPlanPackage(
//         &mut self,
//         ctx: context::Context,
//         task: &ExtractTask,
//         p: extractPlanPackage,
//     ) -> Result<String, errors::Error> {
//         let (f, name) = GenerateExtractFile(ctx)?;
//         let mut zw = zip::NewWriter(f);
//
// Go 使用 defer 统一记录错误并关闭 zip/file；保留显式收尾注释，未实际执行 IO。
//         let result = (|| -> Result<(), errors::Error> {
// Dump config
//             dumpConfig(&mut zw)?;
// Dump meta
//             dumpMeta(&mut zw)?;
// dump extract plan task meta
//             dumpExtractMeta(task, &mut zw)?;
// Dump Schema and View
//             dumpSchemas(&self.sctx, &mut zw, &p.tables)?;
// Dump tables tiflash replicas
//             dumpTiFlashReplica(&self.sctx, &mut zw, &p.tables)?;
// Dump variables
//             dumpVariables(&self.sctx, self.sctx.GetSessionVars(), &mut zw)?;
// Dump global bindings
//             dumpGlobalBindings(&self.sctx, &mut zw)?;
// Dump stats
//             if !task.SkipStats {
//                 dumpStats(&mut zw, &p.tables, GetDomain(&self.sctx), 0)?;
//             }
// Dump sqls and plan
//             dumpSQLRecords(&p.records, &mut zw)?;
//             Ok(())
//         })();
//
//         if let Err(err) = &result {
//             logutil::BgLogger().Error(
//                 "dump extract plan task failed",
//                 zap::Error(err.clone()),
//             );
//         }
//         if let Err(err1) = zw.Close() {
//             logutil::BgLogger().Warn(
//                 "close zip writer failed",
//                 zap::String("file", name.clone()),
//                 zap::Error(err1),
//             );
//         }
//         if let Err(err1) = zw.into_inner().Close() {
//             logutil::BgLogger().Warn(
//                 "close file failed",
//                 zap::String("file", name.clone()),
//                 zap::Error(err1),
//             );
//         }
//
//         result?;
//         Ok(name)
//     }
// }
//
// checkRecordValid 对应 Go 的记录有效性检查，只接受 Select、非空 schema、非空 plan digest。
// pub fn checkRecordValid(r: &stmtSummaryHistoryRecord) -> bool {
//     if r.stmtType != "Select" {
//         return false;
//     }
//     if r.schemaName.is_empty() {
//         return false;
//     }
//     if r.planDigest.is_empty() {
//         return false;
//     }
//     true
// }
//
// dumpSQLRecords 对应 Go 中按 skip 标记把记录写入 SQLs 或 skippedSQLs 目录。
// pub fn dumpSQLRecords(
//     records: &HashMap<stmtSummaryHistoryKey, stmtSummaryHistoryRecord>,
//     zw: &mut zip::Writer,
// ) -> Result<(), errors::Error> {
//     for (key, record) in records {
//         if record.skip {
//             dumpSQLRecord(record, format!("skippedSQLs/{}.json", key.digest), zw)?;
//         } else {
//             dumpSQLRecord(record, format!("SQLs/{}.json", key.digest), zw)?;
//         }
//     }
//     Ok(())
// }
//
// singleSQLRecord 对应 Go 的 JSON 输出结构，字段名保留原 json tag 语义。
// pub struct singleSQLRecord {
//     pub Schema: String,
//     pub Plan: String,
//     pub SQL: String,
//     pub Digest: String,
//     pub BinaryPlan: String,
//     pub UserName: String,
// }
//
// dumpSQLRecord dumps sql records into one file for each record, the format is in json.
// dumpSQLRecord 对应 Go 中创建 zip entry、JSON marshal 并写入内容的流程。
// pub fn dumpSQLRecord(
//     record: &stmtSummaryHistoryRecord,
//     path: String,
//     zw: &mut zip::Writer,
// ) -> Result<(), errors::Error> {
//     let mut zf = zw.Create(path)?;
//     let singleSQLRecord = singleSQLRecord {
//         Schema: record.schemaName.clone(),
//         Plan: record.plan.clone(),
//         SQL: record.sql.clone(),
//         Digest: record.digest.clone(),
//         BinaryPlan: record.binaryPlan.clone(),
//         UserName: record.userName.clone(),
//     };
//     let content = json::Marshal(&singleSQLRecord)?;
//     zf.Write(content)?;
//     Ok(())
// }
//
// dumpExtractMeta 对应 Go 中写 extract_meta.txt 的 toml 元数据逻辑。
// pub fn dumpExtractMeta(
//     task: &ExtractTask,
//     zw: &mut zip::Writer,
// ) -> Result<(), errors::Error> {
//     let mut cf = zw.Create(ExtractMetaFile).map_err(errors::AddStack)?;
//     let mut varMap: HashMap<String, String> = HashMap::new();
//     varMap.insert(
//         ExtractTaskType.to_string(),
//         taskTypeToString(task.ExtractType).to_string(),
//     );
//     if matches!(task.ExtractType, ExtractType::ExtractPlanType) {
//         varMap.insert(
//             ExtractPlanTaskSkipStats.to_string(),
//             strconv::FormatBool(task.SkipStats),
//         );
//     }
//
//     toml::NewEncoder(&mut cf)
//         .Encode(varMap)
//         .map_err(errors::AddStack)?;
//     Ok(())
// }
//
// extractPlanPackage 对应 Go 内部包对象：包含待 dump 表集合和 SQL 记录集合。
// pub struct extractPlanPackage {
//     pub tables: HashSet<tableNamePair>,
//     pub records: HashMap<stmtSummaryHistoryKey, stmtSummaryHistoryRecord>,
// }
//
// stmtSummaryHistoryKey 对应 Go map key，用 digest 和 planDigest 去重。
// #[derive(Clone, Eq, PartialEq, Hash)]
// pub struct stmtSummaryHistoryKey {
//     pub digest: String,
//     pub planDigest: String,
// }
//
// stmtSummaryHistoryRecord 对应 Go 中从 stmt summary 采集出的单条记录。
// #[derive(Default, Clone)]
// pub struct stmtSummaryHistoryRecord {
//     pub stmtType: String,
//     pub schemaName: String,
//     pub tables: Vec<tableNamePair>,
//     pub digest: String,
//     pub planDigest: String,
//     pub sql: String,
//     pub binaryPlan: String,
//     pub userName: String,
//
//     pub plan: String,
//     pub skip: bool,
// }
//
// GenerateExtractFile generates extract stmt file
// GenerateExtractFile 对应 Go 中获取全局外部存储、创建 writer 并包成 replayer file writer。
// pub fn GenerateExtractFile(
//     ctx: context::Context,
// ) -> Result<(io::WriteCloser, String), errors::Error> {
//     let path = GetExtractTaskDirName();
//     let fileName = generateExtractStmtFile().map_err(errors::AddStack)?;
//     let storage = extstore::GetGlobalExtStorage(ctx.clone()).map_err(errors::AddStack)?;
//     let writer = storage
//         .Create(ctx.clone(), filepath::Join(path, &fileName), None)
//         .map_err(errors::AddStack)?;
//     let zf = replayer::NewFileWriter(ctx, writer);
//     Ok((zf, fileName))
// }
//
// generateExtractStmtFile 对应 Go 中用随机字节和纳秒时间生成 zip 文件名的逻辑。
// pub fn generateExtractStmtFile() -> Result<String, errors::Error> {
// Generate key and create zip file
//     let now = time::Now().UnixNano();
//     let mut b = vec![0_u8; 16];
// Go 标注 gosec 例外，表示该随机数只用于文件名，不承担安全密钥职责。
//     rand::Read(&mut b)?;
//     let key = base64::URLEncoding.EncodeToString(&b);
//     Ok(format!("extract_{}_{}.zip", key, now))
// }
//
// GetExtractTaskDirName get extract dir name
// GetExtractTaskDirName 对应 Go 的目录名辅助函数。
// pub fn GetExtractTaskDirName() -> &'static str {
//     ExtractTaskDirName
// }
// */
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// 导出包内元数据文件名（对应 Go `ExtractMetaFile`）。
pub const EXTRACT_META_FILE: &str = "extract_meta.txt";
/// 元数据中任务类型键名。
pub const EXTRACT_TASK_TYPE: &str = "taskType";
/// Plan 任务是否跳过统计信息（stats）的元数据键。
pub const EXTRACT_PLAN_TASK_SKIP_STATS: &str = "SkipStats";
/// Extract 产物在外部存储中的目录名。
pub const EXTRACT_TASK_DIR_NAME: &str = "extract";

/// Extract 任务类型；当前仅迁移 Plan（执行计划）抽取。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractType {
    /// 抽取执行计划及相关 schema/stats/SQL。
    Plan,
}

impl ExtractType {
    /// 转为对外元数据字符串（如 `"Plan"`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
        }
    }
}

/// Extract 任务参数：类型、是否后台、时间窗与 Plan 相关开关。
#[derive(Clone, Debug)]
pub struct ExtractTask {
    pub extract_type: ExtractType,
    /// 后台任务目前未实现，为 true 时直接返回。
    pub is_background_job: bool,
    /// 为 true 时跳过统计信息（stats）导出。
    pub skip_stats: bool,
    /// 是否从 STATEMENTS_SUMMARY_HISTORY 视图读取历史摘要。
    pub use_history_view: bool,
    pub begin: SystemTime,
    pub end: SystemTime,
}

impl ExtractTask {
    /// 构造时间范围内的 Plan 抽取任务（默认非后台、不跳过 stats）。
    pub fn new_plan(begin: SystemTime, end: SystemTime) -> Self {
        Self {
            extract_type: ExtractType::Plan,
            is_background_job: false,
            skip_stats: false,
            use_history_view: false,
            begin,
            end,
        }
    }
}

/// 库表对；`is_view` 标记是否为视图（view）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableNamePair {
    pub database: String,
    pub table: String,
    pub is_view: bool,
}

/// 语句摘要去重键：SQL digest + plan digest。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatementKey {
    pub digest: String,
    pub plan_digest: String,
}

/// 从语句摘要采集的单条记录（含解码后的 plan 与 skip 标记）。
#[derive(Clone, Debug)]
pub struct StatementRecord {
    pub statement_type: String,
    pub schema_name: String,
    pub tables: Vec<TableNamePair>,
    pub digest: String,
    pub plan_digest: String,
    pub sql: String,
    pub binary_plan: String,
    pub user_name: String,
    pub decoded_plan: String,
    /// SQL 被截断（含 `(len:`）时标记为跳过正式导出。
    pub skipped: bool,
}

impl StatementRecord {
    /// 与 Go `checkRecordValid` 一致：仅 Select、非空 schema、非空 plan digest。
    pub fn is_valid(&self) -> bool {
        self.statement_type == "Select"
            && !self.schema_name.is_empty()
            && !self.plan_digest.is_empty()
    }
}

/// Plan 抽取打包对象：涉及表集合与按 digest 去重的 SQL 记录。
#[derive(Clone, Debug, Default)]
pub struct ExtractPlanPackage {
    pub tables: BTreeSet<TableNamePair>,
    pub records: BTreeMap<StatementKey, StatementRecord>,
}

/// 数据源抽象：查询摘要、解析表/视图依赖、解码计划并落盘 zip。
pub trait ExtractSource: Send + Sync {
    fn statement_records(&self, task: &ExtractTask) -> Result<Vec<StatementRecord>, String>;
    fn table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String>;
    fn view_dependencies(&self, view: &TableNamePair) -> Result<Vec<TableNamePair>, String>;
    fn decode_binary_plan(&self, encoded: &str) -> Result<String, String>;
    fn dump_package(
        &self,
        file_name: &str,
        task: &ExtractTask,
        package: &ExtractPlanPackage,
    ) -> Result<(), String>;
    /// 是否开启持久化语句摘要（历史视图依赖该开关）。
    fn persistent_statement_summary_enabled(&self) -> bool;
}

/// Extract 入口句柄：串行执行抽取任务（对应 Go `ExtractHandle` + worker）。
pub struct ExtractHandle {
    source: Arc<dyn ExtractSource>,
    /// 互斥保证同一时刻仅一个抽取在跑，对齐 Go worker mutex。
    serial: Mutex<()>,
}

impl ExtractHandle {
    /// 使用给定数据源构造句柄。
    pub fn new(source: Arc<dyn ExtractSource>) -> Self {
        Self {
            source,
            serial: Mutex::new(()),
        }
    }

    /// 同步执行抽取：收集记录 → 解析视图依赖 → dump zip。
    ///
    /// 后台任务返回 `Ok(None)`；成功时返回产物文件名。
    pub fn extract_task(&self, task: &ExtractTask) -> Result<Option<String>, String> {
        if task.is_background_job {
            return Ok(None);
        }
        // 历史视图要求持久化语句摘要已开启。
        if task.use_history_view && !self.source.persistent_statement_summary_enabled() {
            return Err(
                "tidb_stmt_summary_enable_persistent should be enabled for extract task"
                    .to_string(),
            );
        }
        // Go 只在 collectRecords 阶段持锁，并在该阶段按 digest pair 去重。
        let records = {
            let _guard = self
                .serial
                .lock()
                .map_err(|_| "extract worker lock poisoned".to_string())?;
            let mut records = BTreeMap::new();
            for mut record in self.source.statement_records(task)? {
                let mut resolved = Vec::new();
                // 解析表名：遇内部库或缺失表则整条记录丢弃。
                for table in &record.tables {
                    if is_internal_schema(&table.database) {
                        resolved.clear();
                        break;
                    }
                    match self.source.table(&table.database, &table.table)? {
                        Some(table) => resolved.push(table),
                        None => {
                            resolved.clear();
                            break;
                        }
                    }
                }
                record.tables = resolved;
                record.schema_name = record
                    .tables
                    .last()
                    .map(|table| table.database.clone())
                    .unwrap_or_default();
                if !record.is_valid() || record.tables.is_empty() {
                    continue;
                }
                records.insert(
                    StatementKey {
                        digest: record.digest.clone(),
                        plan_digest: record.plan_digest.clone(),
                    },
                    record,
                );
            }
            records
        };

        let mut package = ExtractPlanPackage {
            records,
            ..ExtractPlanPackage::default()
        };
        for record in package.records.values_mut() {
            // 截断 SQL 写入 skipped；否则解码 binary plan。
            record.skipped = record.sql.contains("(len:");
            if record.skipped {
                continue;
            }
            record.decoded_plan = self
                .source
                .decode_binary_plan(&record.binary_plan)?
                .trim_matches('\n')
                .to_string();
            package.tables.extend(record.tables.iter().cloned());
        }
        // 展开视图依赖的真实表，并入 package.tables。
        let views = package
            .tables
            .iter()
            .filter(|table| table.is_view)
            .cloned()
            .collect::<Vec<_>>();
        for view in views {
            package.tables.extend(self.source.view_dependencies(&view)?);
        }
        let name = generate_extract_file_name();
        self.source.dump_package(&name, task, &package)?;
        Ok(Some(name))
    }
}

/// 判断是否为内部系统库（performance_schema / information_schema 等）。
fn is_internal_schema(schema: &str) -> bool {
    matches!(
        schema.to_ascii_lowercase().as_str(),
        "performance_schema" | "information_schema" | "metrics_schema" | "mysql"
    )
}

/// 生成唯一 zip 文件名（纳秒时间戳与序列号混合，对应 Go 随机文件名语义）。
pub fn generate_extract_file_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "extract_{:016x}_{nanos}.zip",
        (nanos as u64).rotate_left(17) ^ sequence
    )
}

/// 返回 Extract 任务目录名常量。
pub fn get_extract_task_dir_name() -> &'static str {
    EXTRACT_TASK_DIR_NAME
}
