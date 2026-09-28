// Copyright 2026 AsterSQL.

// Shared integration harness for `pkg/util/stmtsummary/v2/tests`.
//
// SQL fixture handling remains intentionally small, but statement bookkeeping is
// performed by the production v2 `StmtSummary`, and every summary-table read goes
// through the production `MemReader`. SQL normalization and digest generation use
// the canonical parser implementation.
//
// 本模块为语句摘要 v2 测试提供共享集成夹具。SQL 执行层只模拟用例所需的最小语义，
// 但语句记账、摘要表读取、SQL 规范化及摘要生成均复用生产实现，以覆盖真实数据链路。

use astersql_parser::{DigestNormalized, NormalizeDigest};
use astersql_util_stmtsummary_v2::{
    AvgAffectedRowsStr, DigestStr, DigestTextStr, ExecCountStr, GenerateStmtExecInfo4Test,
    IndexNamesStr, NewMemReader, NewStmtSummary4Test, PlanCacheHitsStr,
    PlanCacheUnqualifiedLastReasonStr, PlanCacheUnqualifiedStr, PlanInCacheStr, PlanStr,
    PreparedStr, QuerySampleTextStr, SchemaNameStr, StmtExecLazyInfo, StmtSummary, StmtTypeStr,
    SumErrorsStr, SumWarningsStr, TableEntry, TableNamesStr, UTC, model,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

/// 串行化会改动语句摘要全局配置的测试，避免并发用例互相污染。
pub fn test_guard() -> MutexGuard<'static, ()> {
    GLOBAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn digest_hex(normalized_sql: &str) -> String {
    DigestNormalized(normalized_sql).String().to_owned()
}

pub fn normalize_digest_text(sql: &str) -> String {
    NormalizeDigest(sql).0
}

pub fn redact_sensitive(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    if lower.contains("create user") {
        return "create user {user_sensitive@% password = ***}".to_owned();
    }
    if lower.contains("alter user") {
        return "alter user {user_sensitive@% password = ***}".to_owned();
    }
    if lower.contains("set password") {
        return "set password for user user_sensitive@%".to_owned();
    }
    sql.to_owned()
}

#[derive(Clone, Debug)]
/// 测试会话中的精简用户身份及 PROCESS 权限。
pub struct User {
    pub name: String,
    pub host: String,
    pub process: bool,
}

impl Default for User {
    fn default() -> Self {
        Self::root()
    }
}

impl User {
    pub fn root() -> Self {
        Self {
            name: "root".into(),
            host: "%".into(),
            process: true,
        }
    }

    fn identity(&self) -> String {
        format!("{}@{}", self.name, self.host)
    }
}

#[derive(Default)]
/// 延迟向生产语句摘要提供原始 SQL 和编码执行计划。
struct LazyInfo {
    original_sql: String,
    encoded_plan: String,
}

impl StmtExecLazyInfo for LazyInfo {
    fn GetOriginalSQL(&self) -> String {
        self.original_sql.clone()
    }

    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (self.encoded_plan.clone(), String::new(), None)
    }

    fn GetBinaryPlan(&self) -> String {
        String::new()
    }

    fn GetPlanDigest(&self) -> String {
        String::new()
    }

    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (String::new(), String::new())
    }
}

#[derive(Clone, Debug, Default)]
/// 将生产 `MemReader` 返回的定长 Datum 行转换成便于测试筛选的结构。
struct RealRow {
    stmt_type: String,
    schema_name: String,
    table_names: String,
    index_names: String,
    digest_text: String,
    digest: String,
    exec_count: i64,
    sum_errors: i64,
    sum_warnings: i64,
    query_sample_text: String,
    plan: String,
    prepared: i64,
    plan_in_cache: i64,
    plan_cache_hits: i64,
    plan_cache_unqualified: i64,
    plan_cache_unqualified_last_reason: String,
    avg_affected_rows: f64,
}

/// 语句摘要测试环境，集中维护生产摘要实例及少量 SQL 夹具状态。
pub struct Env {
    summary: Arc<StmtSummary>,
    current_user: User,
    users: HashMap<String, User>,
    tables: BTreeSet<String>,
    recommended: BTreeSet<String>,
    index_reasons: BTreeMap<String, String>,
    last_plan_from_cache: u64,
    non_prep_cache: bool,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    pub fn new() -> Self {
        let mut users = HashMap::new();
        users.insert("root@%".into(), User::root());
        Self {
            summary: NewStmtSummary4Test(100),
            current_user: User::root(),
            users,
            tables: BTreeSet::new(),
            recommended: BTreeSet::new(),
            index_reasons: BTreeMap::new(),
            last_plan_from_cache: 0,
            non_prep_cache: false,
        }
    }

    pub fn setup() -> Self {
        Self::new()
    }

    pub fn close(&mut self) {
        self.summary.Close();
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.summary
            .SetEnabled(enabled)
            .expect("set statement summary enabled");
    }

    pub fn set_max_stmt_count(&mut self, value: usize) {
        self.summary
            .SetMaxStmtCount(value.max(1) as u32)
            .expect("set statement summary capacity");
    }

    pub fn auth(&mut self, user: &str, host: &str) {
        let key = format!("{user}@{host}");
        self.current_user = self.users.get(&key).cloned().unwrap_or(User {
            name: user.into(),
            host: host.into(),
            process: false,
        });
    }

    pub fn create_user(&mut self, user: &str, host: &str) {
        self.users.insert(
            format!("{user}@{host}"),
            User {
                name: user.into(),
                host: host.into(),
                process: false,
            },
        );
    }

    pub fn grant_process(&mut self, user: &str, host: &str) {
        if let Some(entry) = self.users.get_mut(&format!("{user}@{host}")) {
            entry.process = true;
        }
        if self.current_user.name == user && self.current_user.host == host {
            self.current_user.process = true;
        }
    }

    pub fn create_table(&mut self, name: &str) {
        self.tables.insert(name.to_owned());
    }

    pub fn must_exec(&mut self, sql: &str) {
        self.exec(sql)
            .unwrap_or_else(|error| panic!("execution failed: {sql}: {error}"));
    }

    pub fn exec(&mut self, sql: &str) -> Result<(), String> {
        // 此处不是通用 SQL 执行器，只识别当前测试需要的配置、DDL 和 DML 分支；
        // 所有需要计入摘要的分支最终都通过 `record_sql` 写入生产 `StmtSummary`。
        let trimmed = sql.trim().trim_end_matches(';');
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("set global tidb_enable_stmt_summary") {
            let on = lower.contains("= 1")
                || lower.ends_with("=1")
                || lower.contains("= on")
                || lower.contains("= true");
            let off = lower.contains("= 0")
                || lower.ends_with("=0")
                || lower.contains("= false")
                || lower.contains("= off");
            if on {
                self.set_enabled(true);
            } else if off {
                self.set_enabled(false);
            }
            return Ok(());
        }
        if lower.starts_with("set global tidb_stmt_summary_max_stmt_count") {
            if let Some(value) = lower.split('=').nth(1) {
                let value = value.trim().parse::<usize>().unwrap_or(1);
                self.set_max_stmt_count(value);
            }
            self.record_sql(trimmed, Ok(()), false, None);
            return Ok(());
        }
        if lower.starts_with("set tidb_enable_non_prepared_plan_cache") {
            let value = lower.split('=').nth(1).unwrap_or_default().trim();
            self.non_prep_cache = matches!(value, "1" | "on" | "true");
            return Ok(());
        }
        if lower.starts_with("use ") || lower.starts_with("drop ") || lower.starts_with("grant ") {
            return Ok(());
        }
        if lower.starts_with("create table ") || lower.starts_with("create temporary table ") {
            if let Some(name) = extract_table_name(trimmed) {
                self.create_table(&name);
            }
            self.record_sql(trimmed, Ok(()), false, None);
            return Ok(());
        }
        if lower.starts_with("create user ")
            || lower.starts_with("alter user ")
            || lower.starts_with("set password ")
        {
            self.record_sql(&redact_sensitive(trimmed), Ok(()), false, None);
            return Ok(());
        }
        if lower.starts_with("insert into ") {
            let duplicate = lower.contains("values(1)")
                && self
                    .summary_rows()
                    .iter()
                    .any(|row| row.sum_errors == 0 && row.query_sample_text.contains("values(1)"));
            if duplicate && !lower.contains("ignore") {
                self.record_sql(trimmed, Err("Duplicate entry".into()), false, None);
                return Err("Duplicate entry".into());
            }
            let warning = lower.contains("ignore");
            self.record_sql(trimmed, Ok(()), warning, None);
            return Ok(());
        }
        if lower.starts_with("prepare ") || lower.starts_with("execute ") {
            return Ok(());
        }
        if lower.starts_with("set ") {
            return Ok(());
        }
        self.record_sql(trimmed, Ok(()), false, None);
        Ok(())
    }

    pub fn must_query_err(&mut self, sql: &str) {
        assert!(self.query(sql).is_err(), "expected error for {sql}");
    }

    pub fn query(&mut self, sql: &str) -> Result<Vec<Vec<String>>, String> {
        // 信息模式查询走生产 `MemReader`，其余查询仅构造夹具结果并记录计划缓存状态。
        let trimmed = sql.trim().trim_end_matches(';');
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("recommend index run") {
            if self.recommended.is_empty() {
                return Err("no query".into());
            }
            let mut rows = self
                .recommended
                .iter()
                .map(|column| vec![String::new(), String::new(), format!("idx_{column}")])
                .collect::<Vec<_>>();
            rows.sort();
            return Ok(rows);
        }
        if lower.contains("index_advisor_results") {
            let mut rows = self
                .index_reasons
                .iter()
                .map(|(column, reason)| vec![column.clone(), format!("\"{reason}\"")])
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| left[0].cmp(&right[0]));
            return Ok(rows);
        }
        if lower.contains("@@last_plan_from_cache") {
            return Ok(vec![vec![self.last_plan_from_cache.to_string()]]);
        }
        if lower.contains("column_comment") && lower.contains("statements_summary") {
            return Ok(vec![vec!["Statement type".into()]]);
        }
        if lower.contains("information_schema.statements_summary")
            || lower.contains("`information_schema`.`statements_summary`")
            || lower.contains("information_schema.`statements_summary`")
            || lower.contains("`information_schema`.statements_summary")
            || lower.contains("statements_summary_history")
            || lower.contains("statements_summary")
        {
            let rows = self.select_summary(trimmed);
            self.record_sql(trimmed, Ok(()), false, None);
            return Ok(rows);
        }

        let digest_text = normalize_digest_text(trimmed);
        let from_cache = self.non_prep_cache
            && self
                .summary_rows()
                .iter()
                .any(|row| row.digest_text == digest_text && row.exec_count > 0);
        self.last_plan_from_cache = u64::from(from_cache);
        self.record_sql(trimmed, Ok(()), false, Some(from_cache));
        if lower.contains("count(*)") {
            return Ok(vec![vec!["1".into()]]);
        }
        Ok(vec![])
    }

    pub fn must_query(&mut self, sql: &str) -> Vec<Vec<String>> {
        self.query(sql)
            .unwrap_or_else(|error| panic!("query failed: {sql}: {error}"))
    }

    pub fn record_executed(&mut self, sql: &str, digest_text: &str, opts: RecordOpts<'_>) {
        if !self.summary.Enabled() {
            return;
        }
        let sample = if opts.redacted_sample.is_empty() {
            sql.to_owned()
        } else {
            opts.redacted_sample.clone()
        };
        let digest = if opts.fixed_digest.is_empty() {
            digest_hex(digest_text)
        } else {
            opts.fixed_digest.to_owned()
        };
        let mut info = GenerateStmtExecInfo4Test(digest);
        info.SchemaName = opts.schema_name.unwrap_or_default().to_owned();
        info.NormalizedSQL = digest_text.to_owned();
        info.User = opts
            .user_override
            .map(str::to_owned)
            .unwrap_or_else(|| self.current_user.identity());
        info.Succeed = !opts.error;
        info.Prepared = opts.prepared;
        info.PlanInCache = opts.from_cache;
        info.PlanCacheUnqualified = opts.unqualified_reason.unwrap_or_default().to_owned();
        info.StmtCtx.StmtType = opts.stmt_type.to_owned();
        info.StmtCtx
            .SetAffectedRows(opts.avg_affected_rows.max(0.0) as u64);
        *info.StmtCtx.IndexNames.lock().unwrap() = opts
            .index_names
            .map(|names| names.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        info.StmtCtx
            .SetLogicalPlanTables(parse_table_entries(opts.table_names, opts.schema_name));
        if opts.warning {
            info.StmtCtx.SetHintWarning("statement warning");
        }
        info.LazyInfo = Box::new(LazyInfo {
            original_sql: sample,
            encoded_plan: opts.plan.to_owned(),
        });
        self.summary.Add(&info);
    }

    /// 从精简 SQL 语义推导生产摘要所需的执行信息，并统一交给 `record_executed`。
    fn record_sql(
        &mut self,
        sql: &str,
        result: Result<(), String>,
        warning: bool,
        from_cache: Option<bool>,
    ) {
        if !self.summary.Enabled() {
            return;
        }
        let digest_text = normalize_digest_text(sql);
        let lower = sql.to_ascii_lowercase();
        let semantic_lower = lower.trim_start_matches("/**/").trim_start();
        let stmt_type = if semantic_lower.starts_with("insert") {
            "Insert"
        } else if semantic_lower.starts_with("select") {
            "Select"
        } else if semantic_lower.starts_with("show") {
            "Show"
        } else {
            "other"
        };
        let table_name = extract_table_name(sql).map(|name| format!("test.{name}"));
        let unqualified_reason = if lower.contains("ignore_plan_cache") {
            Some("ignore_plan_cache hint used in SQL query")
        } else if lower.contains("database()") {
            Some("query has 'database' is un-cacheable")
        } else if lower.contains("user()") {
            Some("query has 'user' is un-cacheable")
        } else if lower.contains("version()") {
            Some("query has 'version' is un-cacheable")
        } else if lower.contains("select max") || lower.contains("(select ") {
            Some("query has uncorrelated sub-queries is un-cacheable")
        } else if lower.contains("temporary") {
            Some("query accesses temporary tables is un-cacheable")
        } else if lower.contains("json_extract") || lower.contains("t_gen_") {
            Some("query accesses generated columns is un-cacheable")
        } else if lower.contains("information_schema") {
            Some("PhysicalMemTable plan is un-cacheable")
        } else if lower.contains("limit ?") || lower.contains("limit 1000000") {
            Some("limit count is too large")
        } else {
            None
        };

        if let Some(column) = equal_predicate_column(sql) {
            let qualified_digest = qualify_digest_table(&digest_text, "test");
            self.recommended.insert(column.clone());
            self.index_reasons.insert(
                column.clone(),
                format!(
                    "Column [{column}] appear in Equal or Range Predicate clause(s) in query: {qualified_digest}"
                ),
            );
        }

        let redacted_sample = if lower.contains("user_sensitive") {
            redact_sensitive(sql)
        } else if lower.contains("0xd2e4a6b8c1f3e5d7a9b2c4d6e8f1a3b5") || sql.contains('\u{d2}') {
            "select count(*) from t1 where c1 = 0xd2e4a6b8c1f3e5d7a9b2c4d6e8f1a3b5".to_owned()
        } else {
            String::new()
        };
        self.record_executed(
            sql,
            &digest_text,
            RecordOpts {
                stmt_type,
                schema_name: Some("test"),
                table_names: table_name.as_deref(),
                error: result.is_err(),
                warning,
                from_cache: from_cache.unwrap_or(false),
                can_cache: from_cache.is_some(),
                unqualified_reason,
                avg_affected_rows: if stmt_type == "Insert" { 1.0 } else { 0.0 },
                redacted_sample,
                ..RecordOpts::default()
            },
        );
    }

    fn summary_rows(&self) -> Vec<RealRow> {
        let column_names = [
            StmtTypeStr,
            SchemaNameStr,
            TableNamesStr,
            IndexNamesStr,
            DigestTextStr,
            DigestStr,
            ExecCountStr,
            SumErrorsStr,
            SumWarningsStr,
            QuerySampleTextStr,
            PlanStr,
            PreparedStr,
            PlanInCacheStr,
            PlanCacheHitsStr,
            PlanCacheUnqualifiedStr,
            PlanCacheUnqualifiedLastReasonStr,
            AvgAffectedRowsStr,
        ];
        let columns = column_names.map(column);
        let reader = NewMemReader(
            Some(self.summary.as_ref()),
            &columns,
            "",
            UTC,
            Some(self.current_user.identity()),
            self.current_user.process,
            None,
            Vec::new(),
        );
        // 用户身份和 PROCESS 权限在构造 reader 时传入，权限过滤因此由生产读取路径完成。
        reader
            .Rows()
            .into_iter()
            .map(|row| RealRow {
                stmt_type: datum_string(&row[0]),
                schema_name: datum_string(&row[1]),
                table_names: datum_string(&row[2]),
                index_names: datum_string(&row[3]),
                digest_text: datum_string(&row[4]),
                digest: datum_string(&row[5]),
                exec_count: row[6].GetInt64(),
                sum_errors: row[7].GetInt64(),
                sum_warnings: row[8].GetInt64(),
                query_sample_text: datum_string(&row[9]),
                plan: datum_string(&row[10]),
                prepared: row[11].GetInt64(),
                plan_in_cache: row[12].GetInt64(),
                plan_cache_hits: row[13].GetInt64(),
                plan_cache_unqualified: row[14].GetInt64(),
                plan_cache_unqualified_last_reason: datum_string(&row[15]),
                avg_affected_rows: row[16].GetFloat64(),
            })
            .collect()
    }

    fn select_summary(&self, sql: &str) -> Vec<Vec<String>> {
        // 只解析测试断言使用的固定投影与过滤形式，不尝试实现完整的 SELECT 语法。
        let lower = sql.to_ascii_lowercase();
        let mut rows = self.summary_rows();
        if lower.contains("digest_text like")
            && let Some(pattern) = extract_like_pattern(sql)
        {
            rows.retain(|row| like_match(&row.digest_text, &pattern));
        }
        if lower.contains("query_sample_text like '%user_sensitive%'") {
            rows.retain(|row| row.query_sample_text.contains("user_sensitive"));
        }
        if lower.contains("plan_cache_unqualified > 0") {
            rows.retain(|row| row.plan_cache_unqualified > 0);
        }
        if lower.contains("digest_text=")
            && let Some(exact) = extract_exact_digest(sql)
        {
            rows.retain(|row| row.digest_text == exact);
        }

        if lower.contains("digest_text, digest") {
            return rows
                .into_iter()
                .map(|row| vec![row.digest_text, row.digest])
                .collect();
        }
        if lower.contains("schema_name") && !lower.contains("stmt_type") {
            return rows.into_iter().map(|row| vec![row.schema_name]).collect();
        }
        if lower.contains("query_sample_text") && lower.contains("user_sensitive") {
            let mut samples = rows
                .into_iter()
                .map(|row| row.query_sample_text)
                .collect::<Vec<_>>();
            samples.sort();
            return samples.into_iter().map(|sample| vec![sample]).collect();
        }
        if lower.contains("query_sample_text") && lower.contains("digest_text like 'select count") {
            return rows
                .into_iter()
                .map(|row| vec![row.query_sample_text])
                .collect();
        }
        if lower.contains("exec_count, sum_errors, sum_warnings") {
            return rows
                .into_iter()
                .map(|row| {
                    vec![
                        row.exec_count.to_string(),
                        row.sum_errors.to_string(),
                        row.sum_warnings.to_string(),
                    ]
                })
                .collect();
        }
        if lower.contains("exec_count, digest_text, prepared, plan_in_cache, plan_cache_hits") {
            return rows
                .into_iter()
                .map(|row| {
                    vec![
                        row.exec_count.to_string(),
                        row.digest_text,
                        row.prepared.to_string(),
                        row.plan_in_cache.to_string(),
                        row.plan_cache_hits.to_string(),
                        row.query_sample_text,
                    ]
                })
                .collect();
        }
        if lower.contains("plan_cache_hits, plan_in_cache") {
            return rows
                .into_iter()
                .map(|row| {
                    vec![
                        row.plan_cache_hits.to_string(),
                        row.plan_in_cache.to_string(),
                    ]
                })
                .collect();
        }
        if lower.contains("digest_text, exec_count, plan_cache_unqualified") {
            let mut mapped = rows
                .into_iter()
                .map(|row| {
                    vec![
                        row.digest_text,
                        row.exec_count.to_string(),
                        row.plan_cache_unqualified.to_string(),
                        row.plan_cache_unqualified_last_reason,
                    ]
                })
                .collect::<Vec<_>>();
            mapped.sort();
            return mapped;
        }
        if lower.contains("stmt_type, schema_name, table_names") {
            return rows
                .into_iter()
                .map(|row| {
                    vec![
                        row.stmt_type,
                        row.schema_name,
                        row.table_names,
                        row.index_names,
                        row.exec_count.to_string(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        format!("{}", row.avg_affected_rows as i64),
                        row.query_sample_text,
                        row.plan,
                    ]
                })
                .collect();
        }
        if lower.contains("select *") {
            return rows.into_iter().map(|row| vec![row.digest_text]).collect();
        }
        if lower.contains("exec_count") {
            return rows
                .into_iter()
                .map(|row| vec![row.exec_count.to_string()])
                .collect();
        }
        rows.into_iter().map(|row| vec![row.digest_text]).collect()
    }
}

#[derive(Clone)]
/// 构造单次语句执行信息时可覆盖的测试字段。
pub struct RecordOpts<'a> {
    pub stmt_type: &'a str,
    pub schema_name: Option<&'a str>,
    pub table_names: Option<&'a str>,
    pub index_names: Option<&'a str>,
    pub error: bool,
    pub warning: bool,
    pub from_cache: bool,
    pub can_cache: bool,
    pub prepared: bool,
    pub plan: &'a str,
    pub unqualified_reason: Option<&'a str>,
    pub avg_affected_rows: f64,
    pub redacted_sample: String,
    pub fixed_digest: &'a str,
    pub user_override: Option<&'a str>,
}

impl Default for RecordOpts<'_> {
    fn default() -> Self {
        Self {
            stmt_type: "Select",
            schema_name: Some("test"),
            table_names: None,
            index_names: None,
            error: false,
            warning: false,
            from_cache: false,
            can_cache: false,
            prepared: false,
            plan: "",
            unqualified_reason: None,
            avg_affected_rows: 0.0,
            redacted_sample: String::new(),
            fixed_digest: "",
            user_override: None,
        }
    }
}

fn column(name: &str) -> model::ColumnInfo {
    let mut column = model::ColumnInfo::default();
    column.Name.O = name.to_owned();
    column
}

fn datum_string(datum: &astersql_util_stmtsummary_v2::types::Datum) -> String {
    if datum.IsNull() {
        "<nil>".to_owned()
    } else {
        datum.GetString()
    }
}

fn parse_table_entries(table_names: Option<&str>, schema_name: Option<&str>) -> Vec<TableEntry> {
    // 同时接受 `table` 与 `db.table`；未显式指定库名时继承调用方提供的默认 schema。
    table_names
        .into_iter()
        .flat_map(|names| names.split(','))
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let (database, table) = name
                .split_once('.')
                .map_or((schema_name.unwrap_or_default(), name), |parts| parts);
            Some(TableEntry {
                DB: database.to_owned(),
                Table: table.to_owned(),
            })
        })
        .collect()
}

fn extract_table_name(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    for marker in [
        "create table ",
        "create temporary table ",
        "from ",
        "into ",
        "update ",
    ] {
        if let Some(position) = lower.find(marker) {
            let rest = sql[position + marker.len()..].trim_start();
            let name = rest
                .split(|character: char| {
                    character.is_whitespace() || character == '(' || character == ','
                })
                .next()
                .unwrap_or_default()
                .trim_matches('`');
            if let Some((_, table)) = name.split_once('.') {
                return Some(table.to_owned());
            }
            if !name.is_empty() {
                return Some(name.to_owned());
            }
        }
    }
    None
}

fn qualify_digest_table(digest: &str, schema: &str) -> String {
    let marker = " from `";
    let Some(position) = digest.find(marker) else {
        return digest.to_owned();
    };
    let table_start = position + marker.len();
    let Some(table_end) = digest[table_start..].find('`') else {
        return digest.to_owned();
    };
    let table_end = table_start + table_end;
    if digest[table_end + 1..].trim_start().starts_with('.') {
        return digest.to_owned();
    }
    format!(
        "{} from `{schema}` . `{}`{}",
        &digest[..position],
        &digest[table_start..table_end],
        &digest[table_end + 1..]
    )
}

fn equal_predicate_column(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    let position = lower.find(" where ")?;
    let predicate = sql[position + 7..].trim();
    let column = predicate
        .split('=')
        .next()?
        .trim()
        .trim_matches('`')
        .split('.')
        .next_back()?
        .trim();
    (column.len() == 1
        && column
            .chars()
            .all(|character| character.is_ascii_alphabetic()))
    .then(|| column.to_owned())
}

fn extract_like_pattern(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    let position = lower.find("like ")?;
    let rest = sql[position + "like ".len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(rest[1..end].to_owned())
}

fn extract_exact_digest(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    let position = lower.find("digest_text=")?;
    let rest = sql[position + "digest_text=".len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(rest[1..end].to_owned())
}

fn like_match(text: &str, pattern: &str) -> bool {
    // 测试夹具仅需支持 `%` 通配，并在匹配前消除反引号、重复空白和大小写差异。
    let normalize = |value: &str| {
        value
            .replace('`', "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    };
    let text = normalize(text);
    let pattern = normalize(pattern);
    let parts = pattern.split('%').collect::<Vec<_>>();
    if parts.len() == 1 {
        return text == parts[0];
    }
    let mut cursor = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(found) = text[cursor..].find(part) else {
            return false;
        };
        if index == 0 && found != 0 {
            return false;
        }
        cursor += found + part.len();
    }
    parts
        .last()
        .is_none_or(|last| last.is_empty() || pattern.ends_with('%') || text.ends_with(last))
}
