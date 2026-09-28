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
// limitations under the License.

// 非事务 DML（Non-transactional DML / BATCH DML）：将大语句按分片列拆成多个自动提交作业。
//
// 在 auto-commit 下扫描分片列（shard column）有序值，按 `limit` 批大小切成 job，
// 再为每个区间重写 WHERE 并串行执行；支持 dry-run 仅输出 SELECT 或拆分后的 DML 示例。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::cmp::Ordering;
use std::fmt::{self, Write};

/// TiDB 隐式行句柄列名（无显式主键时的 `_tidb_rowid`）。
pub const EXTRA_HANDLE_NAME: &str = "_tidb_rowid";
/// 非 dry-run：真正执行拆分后的 DML。
pub const DRY_RUN_NONE: i32 = 0;
/// Dry-run：仅返回用于扫描分片值的 SELECT 语句。
pub const DRY_RUN_QUERY: i32 = 1;
/// Dry-run：返回拆分后的首尾 DML 示例而不执行。
pub const DRY_RUN_SPLIT_DML: i32 = 2;
/// 非事务作业失败时的错误消息常量。
pub const ErrNonTransactionalJobFailure: &str = "non-transactional job failed";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 非事务 DML 路径上的错误类型。
pub struct NonTransactionalError {
    pub message: String,
}

impl NonTransactionalError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for NonTransactionalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NonTransactionalError {}

/// 本模块结果别名。
pub type Result<T> = std::result::Result<T, NonTransactionalError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 分片列的字段类型分类，决定比较与 SQL 字面量编码方式。
pub enum FieldType {
    Signed,
    Unsigned,
    Text,
    Binary,
    Decimal,
}

#[derive(Clone, Debug, PartialEq)]
/// 分片列上的一个数据值（Datum）。
pub enum Datum {
    Null,
    Signed(i64),
    Unsigned(u64),
    Text(String),
    Binary(Vec<u8>),
    Decimal(String),
}

impl Datum {
    /// 是否为 NULL。
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// 估算该值占用的内存字节数，供作业构建时累计配额。
    pub fn estimated_memory_usage(&self) -> i64 {
        match self {
            Self::Null => 0,
            Self::Signed(_) | Self::Unsigned(_) => 8,
            Self::Text(value) | Self::Decimal(value) => value.len() as i64,
            Self::Binary(value) => value.len() as i64,
        }
    }

    /// 按类型比较两个 Datum；文本可选择忽略大小写。
    pub fn compare(&self, other: &Self, case_insensitive: bool) -> Result<Ordering> {
        match (self, other) {
            (Self::Null, Self::Null) => Ok(Ordering::Equal),
            (Self::Null, _) => Ok(Ordering::Less),
            (_, Self::Null) => Ok(Ordering::Greater),
            (Self::Signed(left), Self::Signed(right)) => Ok(left.cmp(right)),
            (Self::Unsigned(left), Self::Unsigned(right)) => Ok(left.cmp(right)),
            (Self::Signed(left), Self::Unsigned(right)) => {
                if *left < 0 {
                    Ok(Ordering::Less)
                } else {
                    Ok((*left as u64).cmp(right))
                }
            }
            (Self::Unsigned(left), Self::Signed(right)) => {
                if *right < 0 {
                    Ok(Ordering::Greater)
                } else {
                    Ok(left.cmp(&(*right as u64)))
                }
            }
            (Self::Text(left), Self::Text(right)) if case_insensitive => {
                Ok(left.to_lowercase().cmp(&right.to_lowercase()))
            }
            (Self::Text(left), Self::Text(right)) => Ok(left.cmp(right)),
            (Self::Binary(left), Self::Binary(right)) => Ok(left.cmp(right)),
            (Self::Decimal(left), Self::Decimal(right)) => compare_decimal(left, right),
            _ => Err(NonTransactionalError::new(
                "Non-transactional DML, inconsistent shard column datum types",
            )),
        }
    }

    /// 将 Datum 编码为可嵌入 SQL 的字面量。
    pub fn to_sql_literal(&self, field_type: FieldType) -> Result<String> {
        match (self, field_type) {
            (Self::Null, _) => Ok("NULL".to_owned()),
            (Self::Signed(value), FieldType::Signed) => Ok(value.to_string()),
            (Self::Unsigned(value), FieldType::Unsigned) => Ok(value.to_string()),
            (Self::Text(value), FieldType::Text) => Ok(format!("'{}'", value.replace('\'', "''"))),
            (Self::Binary(value), FieldType::Binary) => {
                let mut encoded = String::with_capacity(value.len() * 2 + 3);
                encoded.push_str("X'");
                for byte in value {
                    write!(encoded, "{byte:02X}").expect("writing to String cannot fail");
                }
                encoded.push('\'');
                Ok(encoded)
            }
            (Self::Decimal(value), FieldType::Decimal) => {
                validate_decimal(value)?;
                Ok(value.clone())
            }
            _ => Err(NonTransactionalError::new(
                "Failed to restore the DML statement, probably because of unsupported type of the shard column",
            )),
        }
    }
}

/// 校验十进制分片值的文本格式是否合法。
fn validate_decimal(value: &str) -> Result<()> {
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let mut pieces = unsigned.split('.');
    let integral = pieces.next().unwrap_or_default();
    let fractional = pieces.next();
    let valid = !integral.is_empty()
        && integral.bytes().all(|byte| byte.is_ascii_digit())
        && fractional
            .is_none_or(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && pieces.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(NonTransactionalError::new("invalid decimal shard value"))
    }
}

/// 比较两个十进制文本值的大小。
fn compare_decimal(left: &str, right: &str) -> Result<Ordering> {
    validate_decimal(left)?;
    validate_decimal(right)?;
    let (left_negative, left_integral, left_fractional) = normalize_decimal(left);
    let (right_negative, right_integral, right_fractional) = normalize_decimal(right);
    if left_negative != right_negative {
        return Ok(if left_negative {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let absolute_order = left_integral
        .len()
        .cmp(&right_integral.len())
        .then_with(|| left_integral.cmp(right_integral))
        .then_with(|| {
            let length = left_fractional.len().max(right_fractional.len());
            (0..length)
                .map(|index| {
                    left_fractional
                        .as_bytes()
                        .get(index)
                        .copied()
                        .unwrap_or(b'0')
                        .cmp(
                            &right_fractional
                                .as_bytes()
                                .get(index)
                                .copied()
                                .unwrap_or(b'0'),
                        )
                })
                .find(|ordering| *ordering != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
    Ok(if left_negative {
        absolute_order.reverse()
    } else {
        absolute_order
    })
}

/// 归一化十进制：返回 (负数?, 整数部, 小数部)。
fn normalize_decimal(value: &str) -> (bool, &str, &str) {
    let negative = value.starts_with('-');
    let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (integral, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let integral = integral.trim_start_matches('0');
    let integral = if integral.is_empty() { "0" } else { integral };
    let fractional = fractional.trim_end_matches('0');
    let negative = negative && (integral != "0" || !fractional.is_empty());
    (negative, integral, fractional)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列的三段式名称：schema / table / name。
pub struct ColumnName {
    pub schema: String,
    pub table: String,
    pub name: String,
}

impl ColumnName {
    /// 仅填充列名、schema/table 为空的列引用。
    pub fn unqualified(name: impl Into<String>) -> Self {
        Self {
            schema: String::new(),
            table: String::new(),
            name: name.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// UPDATE / ON DUPLICATE 中的列赋值目标。
pub struct Assignment {
    pub column: ColumnName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表列元信息：类型、是否主键、排序规则是否大小写不敏感。
pub struct ColumnInfo {
    pub name: String,
    pub field_type: FieldType,
    pub primary_key: bool,
    pub collation_case_insensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引元信息：可见性、是否主键索引及列序。
pub struct IndexInfo {
    pub public: bool,
    pub invisible: bool,
    pub primary: bool,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表元信息：列、索引，以及整数/聚簇句柄标志。
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<IndexInfo>,
    pub pk_is_handle: bool,
    pub common_handle: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// FROM 子句中的表源：物理表加可选别名。
pub struct TableSource {
    pub table: TableInfo,
    pub alias: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表引用树：Join、单表或暂不支持的节点。
pub enum ResultSetNode {
    Join(Box<ResultSetNode>, Box<ResultSetNode>),
    Table(TableSource),
    Unsupported(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 非事务路径支持的 DML 种类。
pub enum DmlKind {
    Delete,
    Update { assignments: Vec<Assignment> },
    InsertSelect { on_duplicate: Vec<Assignment> },
    InsertValues,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 待拆分的 DML 语句描述与带 `{WHERE}` 占位符的可还原模板。
pub struct DmlStatement {
    pub kind: DmlKind,
    pub table_refs: Option<ResultSetNode>,
    pub where_condition: Option<String>,
    pub has_limit: bool,
    pub has_order_by: bool,
    /// Restorable SQL with exactly one `{WHERE}` marker at the DML condition site.
    /// 可还原 SQL：在 DML 条件处恰好有一个 `{WHERE}` 标记。
    pub sql_template: String,
}

impl DmlStatement {
    /// 用实际条件替换唯一的 `{WHERE}` 标记，生成可执行 SQL。
    pub fn restore_with_condition(&self, condition: &str) -> Result<String> {
        if self.sql_template.matches("{WHERE}").count() != 1 {
            return Err(NonTransactionalError::new(
                "Failed to restore the DML statement, missing unique {WHERE} marker",
            ));
        }
        Ok(self.sql_template.replacen("{WHERE}", condition, 1))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 非事务 DML 语句：内嵌 DML、分片列、批大小与 dry-run 模式。
pub struct NonTransactionalDMLStmt {
    pub dml_stmt: DmlStatement,
    pub shard_column: Option<ColumnName>,
    pub limit: i64,
    pub dry_run: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 结果集单元格值（整型或文本）。
pub enum ResultValue {
    Integer(i64),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 结果集列描述。
pub struct ResultField {
    pub name: String,
    pub field_type: FieldType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 简易结果集：字段、行与最大 chunk 大小。
pub struct SimpleRecordSet {
    pub fields: Vec<ResultField>,
    pub rows: Vec<Vec<ResultValue>>,
    pub max_chunk_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 非事务 DML 指标种类，用于递增会话 metrics。
pub enum MetricKind {
    Delete,
    Update,
    Insert,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 运行时日志级别。
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 非事务路径关心的会话变量子集。
pub struct SessionVars {
    pub read_staleness: u64,
    pub bulk_dml_enabled: bool,
    pub autocommit: bool,
    pub in_transaction: bool,
    pub global_batch_dml_enabled: bool,
    pub dml_batch_size: u64,
    pub batch_delete: bool,
    pub batch_insert: bool,
    pub weak_read_consistency: bool,
    pub snapshot_ts: u64,
    pub select_limit: u64,
    pub max_execution_time: u64,
    pub memory_quota_query: i64,
    pub ignore_error: bool,
    pub redact_log: String,
    pub current_db: String,
    pub max_chunk_size: usize,
}

/// 执行 DML 后可选返回的结果集，需可关闭。
pub trait RuntimeRecordSet {
    fn close(&mut self) -> Result<()>;
}

/// Every production side effect is mandatory: implementations cannot silently succeed.
/// 生产侧效应均为强制：实现不得静默成功。定义预处理、扫描、执行与取消等钩子。
pub trait NonTransactionalRuntime {
    fn session_vars(&self) -> &SessionVars;
    fn session_vars_mut(&mut self) -> &mut SessionVars;
    fn preprocess(&mut self, statement: &mut NonTransactionalDMLStmt) -> Result<()>;
    fn increment_metric(&mut self, metric: MetricKind);
    fn attach_memory_tracker(&mut self, quota: i64) -> Result<()>;
    fn consume_memory(&mut self, bytes: i64) -> Result<()>;
    fn detach_memory_tracker(&mut self) -> Result<()>;
    fn scan_shard_values(&mut self, sql: &str) -> Result<Vec<Datum>>;
    fn compare_shard_values(
        &mut self,
        left: &Datum,
        right: &Datum,
        column: Option<&ColumnInfo>,
    ) -> Result<Ordering>;
    fn execute_dml(&mut self, sql: &str) -> Result<Option<Box<dyn RuntimeRecordSet>>>;
    fn is_cancelled(&self) -> bool;
    fn cancellation_error(&self) -> NonTransactionalError;
    fn log(&mut self, level: LogLevel, message: &str);
}

// job: handle keys in [start, end]
// 作业：处理分片键闭区间 [start, end] 上的一批行。
#[derive(Clone, Debug, PartialEq)]
pub struct job {
    pub start: Datum,
    pub end: Datum,
    pub err: Option<NonTransactionalError>,
    pub jobID: usize,
    pub jobSize: usize,
    pub sql: String,
}

impl job {
    /// 格式化作业描述；按 redact 设置脱敏 SQL。
    pub fn String(&self, redacted: &str) -> String {
        format!(
            "job id: {}, estimated size: {}, sql: {}",
            self.jobID,
            self.jobSize,
            redact_sql(redacted, &self.sql)
        )
    }
}

// statementBuildInfo contains information that is needed to build the split statement in a job.
// 构建单个作业拆分语句所需的上下文信息。
#[derive(Clone, Debug, PartialEq)]
pub struct statementBuildInfo {
    pub stmt: NonTransactionalDMLStmt,
    pub shardColumnType: FieldType,
    pub shardColumnRefer: ColumnInfo,
    pub originalCondition: Option<String>,
}

/// 非事务 DML 入口：校验约束、建 SELECT、切分作业并执行或 dry-run。
pub fn HandleNonTransactionalDML(
    stmt: &mut NonTransactionalDMLStmt,
    runtime: &mut dyn NonTransactionalRuntime,
) -> Result<SimpleRecordSet> {
    // 暂时关闭陈旧读与 bulk DML，函数返回前恢复。
    let original_read_staleness = runtime.session_vars().read_staleness;
    let original_bulk_dml_enabled = runtime.session_vars().bulk_dml_enabled;
    runtime.session_vars_mut().read_staleness = 0;
    runtime.session_vars_mut().bulk_dml_enabled = false;

    let outcome = (|| {
        runtime.preprocess(stmt)?;
        checkConstraint(stmt, runtime)?;

        let (table_name, select_sql, shard_column_info, table_sources) =
            buildSelectSQL(stmt, runtime.session_vars().current_db.as_str())?;
        checkConstraintWithShardColumn(
            runtime.session_vars(),
            stmt,
            &table_name,
            shard_column_info.as_ref(),
            &table_sources,
        )?;

        // 仅返回扫描分片值的 SELECT。
        if stmt.dry_run == DRY_RUN_QUERY {
            return buildDryRunResults(
                stmt.dry_run,
                vec![select_sql],
                runtime.session_vars().max_chunk_size,
            );
        }

        // 挂载查询内存配额跟踪器后构建并运行作业。
        runtime.attach_memory_tracker(runtime.session_vars().memory_quota_query)?;
        let tracked_outcome = (|| {
            let mut jobs = buildShardJobs(stmt, runtime, &select_sql, shard_column_info.as_ref())?;
            let split_statements = runJobs(
                &mut jobs,
                stmt,
                &table_name,
                runtime,
                stmt.dml_stmt.where_condition.as_deref(),
            )?;
            // Dry-run 拆分模式：仅收集首尾作业的 SQL 示例。
            if stmt.dry_run == DRY_RUN_SPLIT_DML {
                buildDryRunResults(
                    stmt.dry_run,
                    split_statements,
                    runtime.session_vars().max_chunk_size,
                )
            } else {
                let max_chunk_size = runtime.session_vars().max_chunk_size;
                let redact_log = runtime.session_vars().redact_log.clone();
                buildExecuteResults(&jobs, max_chunk_size, redact_log.as_str(), runtime)
            }
        })();
        let detach_outcome = runtime.detach_memory_tracker();
        match (tracked_outcome, detach_outcome) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(detach)) => Err(detach),
            (Ok(result), Ok(())) => Ok(result),
        }
    })();

    runtime.session_vars_mut().read_staleness = original_read_staleness;
    runtime.session_vars_mut().bulk_dml_enabled = original_bulk_dml_enabled;
    outcome
}

/// 检查 UPDATE / INSERT...ON DUPLICATE 是否试图修改分片列。
pub fn checkConstraintWithShardColumn(
    session_vars: &SessionVars,
    stmt: &NonTransactionalDMLStmt,
    table_name: &TableInfo,
    shard_column_info: Option<&ColumnInfo>,
    table_sources: &[TableSource],
) -> Result<()> {
    match &stmt.dml_stmt.kind {
        DmlKind::Update { assignments } => checkUpdateShardColumn(
            session_vars,
            assignments,
            shard_column_info,
            table_name,
            table_sources,
            true,
        ),
        DmlKind::InsertSelect { on_duplicate } => checkUpdateShardColumn(
            session_vars,
            on_duplicate,
            shard_column_info,
            table_name,
            table_sources,
            false,
        ),
        _ => Ok(()),
    }
}

/// 遍历赋值列表，禁止更新分片列。
pub fn checkUpdateShardColumn(
    session_vars: &SessionVars,
    assignments: &[Assignment],
    shard_column_info: Option<&ColumnInfo>,
    table_name: &TableInfo,
    table_sources: &[TableSource],
    is_update: bool,
) -> Result<()> {
    let Some(shard_column_info) = shard_column_info else {
        return Ok(());
    };
    let aliased_table_name = table_sources
        .iter()
        .find(|source| eq_ci(&source.table.name, &table_name.name) && !source.alias.is_empty())
        .map_or(table_name.name.as_str(), |source| source.alias.as_str());

    for assignment in assignments {
        let same_database = eq_ci(&assignment.column.schema, &table_name.schema)
            || (assignment.column.schema.is_empty()
                && eq_ci(&table_name.schema, &session_vars.current_db));
        if !same_database {
            continue;
        }
        let same_table = eq_ci(&assignment.column.table, aliased_table_name)
            || (is_update && table_sources.len() == 1);
        if same_table && eq_ci(&assignment.column.name, &shard_column_info.name) {
            return Err(NonTransactionalError::new(
                "Non-transactional DML, shard column cannot be updated",
            ));
        }
    }
    Ok(())
}

/// 检查 auto-commit、batch-dml、弱一致读、快照等前置约束，并累计指标。
pub fn checkConstraint(
    stmt: &NonTransactionalDMLStmt,
    runtime: &mut dyn NonTransactionalRuntime,
) -> Result<()> {
    let vars = runtime.session_vars();
    if !(vars.autocommit && !vars.in_transaction) {
        return Err(NonTransactionalError::new(format!(
            "non-transactional DML can only run in auto-commit mode. auto-commit:{}, inTxn:{}",
            vars.autocommit, vars.in_transaction
        )));
    }
    if vars.global_batch_dml_enabled
        && vars.dml_batch_size > 0
        && (vars.batch_delete || vars.batch_insert)
    {
        return Err(NonTransactionalError::new(
            "can't run non-transactional DML with batch-dml",
        ));
    }
    if vars.weak_read_consistency {
        return Err(NonTransactionalError::new(
            "can't run non-transactional under weak read consistency",
        ));
    }
    if vars.snapshot_ts != 0 {
        return Err(NonTransactionalError::new(
            "can't do non-transactional DML when tidb_snapshot is set",
        ));
    }

    match &stmt.dml_stmt.kind {
        DmlKind::Delete => {
            checkTableRef(stmt.dml_stmt.table_refs.as_ref(), true)?;
            checkReadClauses(stmt.dml_stmt.has_limit, stmt.dml_stmt.has_order_by)?;
            runtime.increment_metric(MetricKind::Delete);
        }
        DmlKind::Update { .. } => {
            checkTableRef(stmt.dml_stmt.table_refs.as_ref(), true)?;
            checkReadClauses(stmt.dml_stmt.has_limit, stmt.dml_stmt.has_order_by)?;
            runtime.increment_metric(MetricKind::Update);
        }
        DmlKind::InsertSelect { .. } => {
            checkTableRef(stmt.dml_stmt.table_refs.as_ref(), true)?;
            checkReadClauses(stmt.dml_stmt.has_limit, stmt.dml_stmt.has_order_by)?;
            runtime.increment_metric(MetricKind::Insert);
        }
        DmlKind::InsertValues => {
            return Err(NonTransactionalError::new(
                "Non-transactional insert supports insert select stmt only",
            ));
        }
        DmlKind::Unsupported => {
            return Err(NonTransactionalError::new(
                "Unsupported DML type for non-transactional DML",
            ));
        }
    }
    Ok(())
}

/// 检查表引用是否存在，以及是否允许多表 Join。
pub fn checkTableRef(
    table_refs: Option<&ResultSetNode>,
    allow_multiple_tables: bool,
) -> Result<()> {
    let Some(table_refs) = table_refs else {
        return Err(NonTransactionalError::new("table reference is nil"));
    };
    if !allow_multiple_tables && matches!(table_refs, ResultSetNode::Join(_, _)) {
        return Err(NonTransactionalError::new(
            "Non-transactional statements don't support multiple tables",
        ));
    }
    Ok(())
}

/// 非事务语句不支持 LIMIT / ORDER BY。
pub fn checkReadClauses(has_limit: bool, has_order_by: bool) -> Result<()> {
    if has_limit {
        return Err(NonTransactionalError::new(
            "Non-transactional statements don't support limit",
        ));
    }
    if has_order_by {
        return Err(NonTransactionalError::new(
            "Non-transactional statements don't support order by",
        ));
    }
    Ok(())
}

/// The Go worker is deliberately single-threaded. Error observation therefore follows job order.
/// Go 侧 worker 故意单线程，错误观察顺序与 job 顺序一致；串行执行各作业。
pub fn runJobs(
    jobs: &mut [job],
    stmt: &NonTransactionalDMLStmt,
    table_name: &TableInfo,
    runtime: &mut dyn NonTransactionalRuntime,
    original_condition: Option<&str>,
) -> Result<Vec<String>> {
    let shard_column_name = stmt.shard_column.as_ref().ok_or_else(|| {
        NonTransactionalError::new("Non-transactional DML, shard column not found")
    })?;
    let shard_column = table_name
        .columns
        .iter()
        .find(|column| eq_ci(&column.name, &shard_column_name.name))
        .cloned()
        .or_else(|| {
            eq_ci(&shard_column_name.name, EXTRA_HANDLE_NAME).then_some(ColumnInfo {
                name: EXTRA_HANDLE_NAME.to_owned(),
                field_type: FieldType::Signed,
                primary_key: false,
                collation_case_insensitive: false,
            })
        })
        .ok_or_else(|| {
            NonTransactionalError::new("Non-transactional DML, shard column not found")
        })?;

    let build_info = statementBuildInfo {
        stmt: stmt.clone(),
        shardColumnType: shard_column.field_type,
        shardColumnRefer: shard_column,
        originalCondition: original_condition.map(str::to_owned),
    };
    let total_jobs = jobs.len();
    let mut split_statements = Vec::with_capacity(total_jobs.min(2));
    for (index, current_job) in jobs.iter_mut().enumerate() {
        // 上下文取消：汇总已完成作业中的失败后返回取消错误。
        if runtime.is_cancelled() {
            let failed_jobs = jobs_failure_summary(&jobs[..index], runtime.session_vars());
            if failed_jobs.is_empty() {
                runtime.log(
                    LogLevel::Warning,
                    &format!(
                        "Non-transactional DML worker exit because context canceled. No errors; finished={index}, total={total_jobs}"
                    ),
                );
            } else {
                runtime.log(
                    LogLevel::Warning,
                    &format!(
                        "Non-transactional DML worker exit because context canceled. Errors found; finished={index}, total={total_jobs}, errors={failed_jobs}"
                    ),
                );
            }
            return Err(runtime.cancellation_error());
        }

        if stmt.dry_run == DRY_RUN_SPLIT_DML {
            if index == 0 || index + 1 == total_jobs {
                let sql = doOneJob(current_job, total_jobs, &build_info, runtime, true);
                split_statements.push(sql);
            }
        } else {
            doOneJob(current_job, total_jobs, &build_info, runtime, false);
        }

        // 第一个作业失败则立即取消全部作业。
        if index == 0 {
            if let Some(error) = &current_job.err {
                return Err(NonTransactionalError::new(format!(
                    "Early return: error occurred in the first job. All jobs are canceled: {error}"
                )));
            }
        }
        if let Some(error) = &current_job.err {
            if !runtime.session_vars().ignore_error {
                return Err(NonTransactionalError::new(format!(
                    "non-transactional job {}/{} failed for range [{}, {}]: {}; {error}",
                    current_job.jobID,
                    total_jobs,
                    datum_display(&current_job.start),
                    datum_display(&current_job.end),
                    current_job.String(runtime.session_vars().redact_log.as_str())
                )));
            }
        }
    }
    Ok(split_statements)
}

/// 构建单个作业的 WHERE 与 SQL，并可选真正执行。
pub fn doOneJob(
    current_job: &mut job,
    total_job_count: usize,
    options: &statementBuildInfo,
    runtime: &mut dyn NonTransactionalRuntime,
    dry_run: bool,
) -> String {
    let where_condition = match build_job_condition(current_job, options) {
        Ok(condition) => condition,
        Err(error) => {
            current_job.err = Some(error);
            return String::new();
        }
    };
    let dml_sql = match options
        .stmt
        .dml_stmt
        .restore_with_condition(&where_condition)
    {
        Ok(sql) => sql,
        Err(error) => {
            runtime.log(
                LogLevel::Error,
                "Non-transactional DML, failed to restore the DML statement",
            );
            current_job.err = Some(error);
            return String::new();
        }
    };
    if dry_run {
        return dml_sql;
    }

    current_job.sql.clone_from(&dml_sql);
    runtime.log(
        LogLevel::Info,
        &format!(
            "start a Non-transactional DML: {}; totalJobCount={total_job_count}",
            current_job.String(runtime.session_vars().redact_log.as_str())
        ),
    );
    let executable_sql = format!(
        "/* job {}/{} */ {}",
        current_job.jobID, total_job_count, dml_sql
    );
    match runtime.execute_dml(&executable_sql) {
        Err(error) => {
            runtime.log(
                LogLevel::Info,
                &format!(
                    "Non-transactional DML SQL failed: jobID={}, jobSize={}, error={error}",
                    current_job.jobID, current_job.jobSize
                ),
            );
            current_job.err = Some(error);
        }
        Ok(record_set) => {
            runtime.log(
                LogLevel::Info,
                &format!(
                    "Non-transactional DML SQL finished successfully: jobID={}, jobSize={}",
                    current_job.jobID, current_job.jobSize
                ),
            );
            if let Some(mut record_set) = record_set {
                let _ = record_set.close();
            }
        }
    }
    String::new()
}

/// 根据作业起止 Datum 生成分片列区间条件，并与原始 WHERE 合取。
fn build_job_condition(current_job: &job, options: &statementBuildInfo) -> Result<String> {
    let column = quote_identifier(
        options
            .stmt
            .shard_column
            .as_ref()
            .ok_or_else(|| NonTransactionalError::new("shard column is missing"))?
            .name
            .as_str(),
    );
    let range = if current_job.start.is_null() {
        if current_job.end.is_null() {
            format!("{column} IS NULL")
        } else {
            format!(
                "(({column} <= {}) OR ({column} IS NULL))",
                current_job.end.to_sql_literal(options.shardColumnType)?
            )
        }
    } else {
        format!(
            "({column} BETWEEN {} AND {})",
            current_job.start.to_sql_literal(options.shardColumnType)?,
            current_job.end.to_sql_literal(options.shardColumnType)?
        )
    };
    Ok(options
        .originalCondition
        .as_ref()
        .map_or(range.clone(), |original| {
            format!("({range}) AND ({original})")
        }))
}

/// 扫描分片列有序值，按批大小切分为多个 job。
pub fn buildShardJobs(
    stmt: &NonTransactionalDMLStmt,
    runtime: &mut dyn NonTransactionalRuntime,
    select_sql: &str,
    shard_column_info: Option<&ColumnInfo>,
) -> Result<Vec<job>> {
    // 扫描时放开 select_limit / max_execution_time，结束后恢复。
    let original_select_limit = runtime.session_vars().select_limit;
    let original_max_execution_time = runtime.session_vars().max_execution_time;
    runtime.session_vars_mut().select_limit = u64::MAX;
    runtime.session_vars_mut().max_execution_time = 0;
    let scan_result = runtime.scan_shard_values(select_sql);
    runtime.session_vars_mut().select_limit = original_select_limit;
    runtime.session_vars_mut().max_execution_time = original_max_execution_time;
    let values = scan_result?;

    // Go 在完成扫描并关闭结果集后才校验批大小；保持相同的副作用与错误优先级。
    let batch_size = usize::try_from(stmt.limit).map_err(|_| {
        NonTransactionalError::new("Non-transactional DML, batch size should be positive")
    })?;
    if batch_size == 0 {
        return Err(NonTransactionalError::new(
            "Non-transactional DML, batch size should be positive",
        ));
    }

    let mut jobs = Vec::new();
    let mut current_start: Option<Datum> = None;
    let mut current_end: Option<Datum> = None;
    let mut current_size = 0usize;
    for value in values {
        if current_size == 0 {
            current_start = Some(value.clone());
        } else if current_size >= batch_size
            && runtime.compare_shard_values(
                &value,
                current_end
                    .as_ref()
                    .expect("a non-empty batch always has an end"),
                shard_column_info,
            )? != Ordering::Equal
        {
            appendNewJob(
                &mut jobs,
                current_start.take().expect("a non-empty batch has a start"),
                current_end.take().expect("a non-empty batch has an end"),
                current_size,
                runtime,
            )?;
            current_start = Some(value.clone());
            current_size = 0;
        }
        current_end = Some(value);
        current_size += 1;
    }
    if current_size > 0 {
        appendNewJob(
            &mut jobs,
            current_start.expect("remaining work has a start"),
            current_end.expect("remaining work has an end"),
            current_size,
            runtime,
        )?;
    }
    Ok(jobs)
}

/// 追加一个新作业并计入起止 Datum 的内存用量。
pub fn appendNewJob(
    jobs: &mut Vec<job>,
    start: Datum,
    end: Datum,
    size: usize,
    runtime: &mut dyn NonTransactionalRuntime,
) -> Result<()> {
    let id = jobs.len() + 1;
    runtime.consume_memory(start.estimated_memory_usage() + end.estimated_memory_usage() + 64)?;
    jobs.push(job {
        start,
        end,
        err: None,
        jobID: id,
        jobSize: size,
        sql: String::new(),
    });
    Ok(())
}

/// 收集表源、选定分片列，生成按分片列排序的扫描 SELECT。
pub fn buildSelectSQL(
    stmt: &mut NonTransactionalDMLStmt,
    current_db: &str,
) -> Result<(TableInfo, String, Option<ColumnInfo>, Vec<TableSource>)> {
    let join = stmt.dml_stmt.table_refs.as_ref().ok_or_else(|| {
        NonTransactionalError::new("Non-transactional DML, table source not found")
    })?;
    let mut table_sources = Vec::new();
    collectTableSourcesInJoin(join, &mut table_sources)?;
    let leftmost = table_sources.first().cloned().ok_or_else(|| {
        NonTransactionalError::new("Non-transactional DML, no tables found in table refs")
    })?;
    let (shard_column, table_name) = selectShardColumn(stmt, &table_sources, &leftmost)?;
    let condition = stmt.dml_stmt.where_condition.as_deref().unwrap_or("TRUE");
    let schema = if table_name.schema.is_empty() {
        current_db
    } else {
        table_name.schema.as_str()
    };
    let shard_name = stmt
        .shard_column
        .as_ref()
        .expect("selectShardColumn fills the shard column")
        .name
        .as_str();
    let select_sql = format!(
        "SELECT {column} FROM {schema}.{table} WHERE {condition} ORDER BY IF(ISNULL({column}),0,1),{column}",
        column = quote_identifier(shard_name),
        schema = quote_identifier(schema),
        table = quote_identifier(&table_name.name),
    );
    Ok((table_name, select_sql, shard_column, table_sources))
}

/// 按单表/多表与是否指定列名，选择可用的分片列（须有索引）。
pub fn selectShardColumn(
    stmt: &mut NonTransactionalDMLStmt,
    table_sources: &[TableSource],
    leftmost: &TableSource,
) -> Result<(Option<ColumnInfo>, TableInfo)> {
    let (indexed, info, selected_table) = if table_sources.len() == 1 {
        let (indexed, info) = selectShardColumnFromTheOnlyTable(stmt, leftmost)?;
        (indexed, info, leftmost.table.clone())
    } else if stmt.shard_column.is_none() {
        let (indexed, info) = selectShardColumnAutomatically(stmt, leftmost)?;
        (indexed, info, leftmost.table.clone())
    } else {
        let specified = stmt.shard_column.as_ref().expect("checked above");
        if specified.schema.is_empty() || specified.table.is_empty() || specified.name.is_empty() {
            return Err(NonTransactionalError::new(
                "Non-transactional DML, shard column must be fully specified (i.e. `BATCH ON dbname.tablename.colname`) when multiple tables are involved",
            ));
        }
        let selected = table_sources.iter().find(|source| {
            let visible_name = if source.alias.is_empty() {
                source.table.name.as_str()
            } else {
                source.alias.as_str()
            };
            eq_ci(&source.table.schema, &specified.schema) && eq_ci(visible_name, &specified.table)
        });
        let Some(selected) = selected else {
            return Err(NonTransactionalError::new(format!(
                "Non-transactional DML, shard column {}.{}.{} is not in the tables involved in the join",
                specified.schema, specified.table, specified.name
            )));
        };
        let (indexed, info) = selectShardColumnByGivenName(&specified.name, &selected.table)?;
        (indexed, info, selected.table.clone())
    };

    if !indexed {
        let name = stmt
            .shard_column
            .as_ref()
            .map_or("", |column| column.name.as_str());
        return Err(NonTransactionalError::new(format!(
            "Non-transactional DML, shard column {name} is not indexed"
        )));
    }
    Ok((info, selected_table))
}

/// 递归收集 Join 树中的所有表源。
pub fn collectTableSourcesInJoin(
    node: &ResultSetNode,
    table_sources: &mut Vec<TableSource>,
) -> Result<()> {
    match node {
        ResultSetNode::Join(left, right) => {
            collectTableSourcesInJoin(left, table_sources)?;
            collectTableSourcesInJoin(right, table_sources)
        }
        ResultSetNode::Table(table_source) => {
            table_sources.push(table_source.clone());
            Ok(())
        }
        ResultSetNode::Unsupported(kind) => Err(NonTransactionalError::new(format!(
            "Non-transactional DML, unknown type {kind} in table refs"
        ))),
    }
}

/// 单表场景下选择分片列：未指定则自动选择，已指定则按名查找。
pub fn selectShardColumnFromTheOnlyTable(
    stmt: &mut NonTransactionalDMLStmt,
    table_source: &TableSource,
) -> Result<(bool, Option<ColumnInfo>)> {
    match &stmt.shard_column {
        None => selectShardColumnAutomatically(stmt, table_source),
        Some(column) => selectShardColumnByGivenName(&column.name, &table_source.table),
    }
}

/// 按给定列名判断是否可作为分片列（主键句柄或公开可见索引首列）。
pub fn selectShardColumnByGivenName(
    shard_column_name: &str,
    table: &TableInfo,
) -> Result<(bool, Option<ColumnInfo>)> {
    if eq_ci(shard_column_name, EXTRA_HANDLE_NAME) && !(table.pk_is_handle || table.common_handle) {
        return Ok((true, None));
    }
    let column = table
        .columns
        .iter()
        .find(|column| eq_ci(&column.name, shard_column_name))
        .cloned()
        .ok_or_else(|| {
            NonTransactionalError::new(format!("shard column {shard_column_name} not found"))
        })?;
    if column.primary_key && table.pk_is_handle {
        return Ok((true, Some(column)));
    }
    let indexed = table.indices.iter().any(|index| {
        index.public
            && !index.invisible
            && index
                .columns
                .first()
                .is_some_and(|name| eq_ci(name, shard_column_name))
    });
    Ok((indexed, Some(column)))
}

/// 自动选择分片列：整数主键、单列聚簇索引，或回退到 `_tidb_rowid`。
pub fn selectShardColumnAutomatically(
    stmt: &mut NonTransactionalDMLStmt,
    table_source: &TableSource,
) -> Result<(bool, Option<ColumnInfo>)> {
    let table = &table_source.table;
    let shard_column = if table.pk_is_handle {
        Some(
            table
                .columns
                .iter()
                .find(|column| column.primary_key)
                .cloned()
                .ok_or_else(|| {
                    NonTransactionalError::new(
                        "Non-transactional DML, the integer handle column is not found",
                    )
                })?,
        )
    } else if table.common_handle {
        let primary_index = table
            .indices
            .iter()
            .find(|index| index.primary)
            .ok_or_else(|| {
                NonTransactionalError::new(
                    "Non-transactional DML, the clustered index is not found",
                )
            })?;
        if primary_index.columns.len() != 1 {
            return Err(NonTransactionalError::new(
                "Non-transactional DML, the clustered index contains multiple columns. Please specify a shard column",
            ));
        }
        let primary_name = &primary_index.columns[0];
        Some(
            table
                .columns
                .iter()
                .find(|column| eq_ci(&column.name, primary_name))
                .cloned()
                .ok_or_else(|| {
                    NonTransactionalError::new(
                        "Non-transactional DML, the clustered index column is not found",
                    )
                })?,
        )
    } else {
        None
    };
    let shard_name = shard_column
        .as_ref()
        .map_or(EXTRA_HANDLE_NAME, |column| column.name.as_str());
    stmt.shard_column = Some(ColumnName {
        schema: table.schema.clone(),
        table: if table_source.alias.is_empty() {
            table.name.clone()
        } else {
            table_source.alias.clone()
        },
        name: shard_name.to_owned(),
    });
    Ok((true, shard_column))
}

/// 构造 dry-run 结果集：字段名随 dry-run 模式变化。
pub fn buildDryRunResults(
    dry_run_option: i32,
    results: Vec<String>,
    max_chunk_size: usize,
) -> Result<SimpleRecordSet> {
    let field_name = if dry_run_option == DRY_RUN_SPLIT_DML {
        "split statement examples"
    } else {
        "query statement"
    };
    Ok(SimpleRecordSet {
        fields: vec![ResultField {
            name: field_name.to_owned(),
            field_type: FieldType::Text,
        }],
        rows: results
            .into_iter()
            .map(|result| vec![ResultValue::Text(result)])
            .collect(),
        max_chunk_size,
    })
}

/// 汇总作业执行结果；全部成功返回状态行，否则记录日志并报错。
pub fn buildExecuteResults(
    jobs: &[job],
    max_chunk_size: usize,
    redact_log: &str,
    runtime: &mut dyn NonTransactionalRuntime,
) -> Result<SimpleRecordSet> {
    let failed_jobs: Vec<&job> = jobs
        .iter()
        .filter(|current_job| current_job.err.is_some())
        .collect();
    if failed_jobs.is_empty() {
        return Ok(SimpleRecordSet {
            fields: vec![
                ResultField {
                    name: "number of jobs".to_owned(),
                    field_type: FieldType::Signed,
                },
                ResultField {
                    name: "job status".to_owned(),
                    field_type: FieldType::Text,
                },
            ],
            rows: vec![vec![
                ResultValue::Integer(jobs.len() as i64),
                ResultValue::Text("all succeeded".to_owned()),
            ]],
            max_chunk_size,
        });
    }

    let mut errors = String::new();
    for current_job in &failed_jobs {
        let error = current_job
            .err
            .as_ref()
            .expect("failed_jobs contains only errors");
        writeln!(errors, "{}, {error};", current_job.String(redact_log))
            .expect("writing to String cannot fail");
    }
    runtime.log(
        LogLevel::Error,
        &format!(
            "Non-transactional DML failed: num_failed_jobs={}, failed_jobs={errors}",
            failed_jobs.len()
        ),
    );
    // Go removes the final newline emitted for the last failed job before it
    // appends the summary suffix.  Preserve that exact user-visible layout.
    let preview: String = errors
        .strip_suffix('\n')
        .unwrap_or(&errors)
        .chars()
        .take(500)
        .collect();
    Err(NonTransactionalError::new(format!(
        "{}/{} jobs failed in the non-transactional DML: {preview}, ...(more in logs)",
        failed_jobs.len(),
        jobs.len()
    )))
}

/// 用反引号引用标识符，内部反引号加倍转义。
fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

/// ASCII 大小写不敏感的字符串相等比较。
fn eq_ci(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// 将 Datum 格式化为日志可读字符串。
fn datum_display(value: &Datum) -> String {
    match value {
        Datum::Null => "NULL".to_owned(),
        Datum::Signed(value) => value.to_string(),
        Datum::Unsigned(value) => value.to_string(),
        Datum::Text(value) | Datum::Decimal(value) => value.clone(),
        Datum::Binary(value) => format!("{} bytes", value.len()),
    }
}

/// 汇总已失败作业的简短描述，用于取消时的告警日志。
fn jobs_failure_summary(jobs: &[job], vars: &SessionVars) -> String {
    jobs.iter()
        .filter_map(|current_job| {
            current_job.err.as_ref().map(|error| {
                format!(
                    "job:{}, error: {error}",
                    current_job.String(vars.redact_log.as_str())
                )
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 按 Go `redact.String` 契约处理 SQL：OFF 原样、ON 清空、MARKER 包裹并转义标记。
fn redact_sql(redacted: &str, sql: &str) -> String {
    astersql_util_redact::String(redacted, sql)
}
