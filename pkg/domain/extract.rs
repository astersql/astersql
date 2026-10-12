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
// 便于离线复现与诊断。真实 zip 与 statement-summary I/O 通过 `ExtractSource`
// 边界注入，本模块负责筛选、依赖展开和打包顺序。

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

struct TableDependencyVisitor {
    default_schema: String,
    tables: BTreeSet<TableNamePair>,
    cte_names: BTreeSet<String>,
}

impl astersql_parser_ast::InPlaceVisitor for TableDependencyVisitor {
    fn enter(&mut self, _input: &mut dyn astersql_parser_ast::Node) -> bool {
        false
    }

    fn leave(&mut self, input: &mut dyn astersql_parser_ast::Node) -> bool {
        if let Some(select) = input
            .as_any_mut()
            .downcast_mut::<astersql_parser_ast::SelectStmt>()
        {
            if let Some(with) = &select.With {
                self.cte_names
                    .extend(with.borrow().CTEs.iter().map(|cte| cte.Name.L.clone()));
            }
        }
        true
    }

    fn enter_table_name(&mut self, table: &mut astersql_parser_ast::TableName) -> bool {
        self.tables.insert(TableNamePair {
            database: if table.Schema.L.is_empty() {
                self.default_schema.clone()
            } else {
                table.Schema.L.clone()
            },
            table: table.Name.L.clone(),
            is_view: false,
        });
        false
    }
}

/// Parse a stored view definition and traverse its real AST, including nested
/// queries and table expressions. A CREATE VIEW wrapper visits its SELECT only.
pub fn view_dependencies_from_sql(
    select_sql: &str,
    default_schema: &str,
) -> Result<Vec<TableNamePair>, String> {
    let mut parser = astersql_parser::New();
    let mut node = parser
        .ParseOneStmt(select_sql, "utf8mb4", "utf8mb4_bin")
        .map_err(|error| format!("parse view definition: {error}"))?;
    let mut visitor = TableDependencyVisitor {
        default_schema: default_schema.to_owned(),
        tables: BTreeSet::new(),
        cte_names: BTreeSet::new(),
    };
    let select: &mut dyn astersql_parser_ast::Node = if let Some(view) =
        node.as_any_mut()
            .downcast_mut::<astersql_parser_ast::CreateViewStmt>()
    {
        view.Select.as_mut()
    } else {
        node.as_mut()
    };
    astersql_parser_ast::Walk(select, &mut visitor);
    Ok(visitor
        .tables
        .into_iter()
        .filter(|table| !visitor.cte_names.contains(&table.table))
        .collect())
}

/// Keep the existing summary/dump source while resolving view dependencies
/// through the Domain's current InfoSchema and the production parser AST.
struct DomainAstExtractSource {
    domain: Arc<crate::domain::Domain>,
    source: Arc<dyn ExtractSource>,
}

impl DomainAstExtractSource {
    fn collect_view_dependencies(
        &self,
        view: &TableNamePair,
        visited: &mut BTreeSet<TableNamePair>,
        dependencies: &mut BTreeSet<TableNamePair>,
    ) -> Result<(), String> {
        if !visited.insert(view.clone()) {
            return Ok(());
        }
        let (_, table) = self
            .domain
            .stats_table(&view.database, &view.table)
            .ok_or_else(|| format!("view {}.{} disappeared", view.database, view.table))?;
        let definition = table
            .View
            .as_ref()
            .ok_or_else(|| format!("{}.{} is not a view", view.database, view.table))?;
        for name in view_dependencies_from_sql(&definition.SelectStmt, &view.database)? {
            let dependency = self
                .source
                .table(&name.database, &name.table)?
                .ok_or_else(|| "view dependency disappeared".to_owned())?;
            dependencies.insert(dependency.clone());
            if dependency.is_view {
                self.collect_view_dependencies(&dependency, visited, dependencies)?;
            }
        }
        Ok(())
    }
}

impl ExtractSource for DomainAstExtractSource {
    fn statement_records(&self, task: &ExtractTask) -> Result<Vec<StatementRecord>, String> {
        self.source.statement_records(task)
    }

    fn table(&self, database: &str, table: &str) -> Result<Option<TableNamePair>, String> {
        self.source.table(database, table)
    }

    fn view_dependencies(&self, view: &TableNamePair) -> Result<Vec<TableNamePair>, String> {
        let mut dependencies = BTreeSet::new();
        self.collect_view_dependencies(view, &mut BTreeSet::new(), &mut dependencies)?;
        Ok(dependencies.into_iter().collect())
    }

    fn decode_binary_plan(&self, encoded: &str) -> Result<String, String> {
        self.source.decode_binary_plan(encoded)
    }

    fn dump_package(
        &self,
        file_name: &str,
        task: &ExtractTask,
        package: &ExtractPlanPackage,
    ) -> Result<(), String> {
        self.source.dump_package(file_name, task, package)
    }

    fn persistent_statement_summary_enabled(&self) -> bool {
        self.source.persistent_statement_summary_enabled()
    }
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

    /// Construct the extract worker with real InfoSchema view definitions.
    pub fn new_with_domain(
        domain: Arc<crate::domain::Domain>,
        source: Arc<dyn ExtractSource>,
    ) -> Self {
        Self::new(Arc::new(DomainAstExtractSource { domain, source }))
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
