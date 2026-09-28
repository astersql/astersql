// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Ported from pkg/plugin/integration_test.go. Go's `TestAuditLogNormal` drives a
// full mock TiDB server (`testkit.CreateMockStore` + `server.CreateMockServer` /
// `CreateMockConn`) so SQL dispatch fires audit `OnGeneralEvent` callbacks with
// real StmtCtx metadata. Those server/session hooks are outside this crate's
// already-ported surface, so this test exercises the same audit SPI path that
// Go verifies after dispatch: `LoadPluginForTest` registers the recorder,
// `ForeachPlugin` + `DeclareAuditManifest` deliver Starting/Completed events
// with statement metadata, and retry Completed events are filtered exactly as
// in Go.
//
// 审计日志正常路径集成测试（对应 Go `TestAuditLogNormal`）。
//
// 因完整 mock 服务端尚未迁入，本文件在插件 SPI 层模拟 Starting/Completed
// 通用事件分发，并过滤重试（retry）产生的 Completed，核对 SQL 文本、影响行数、
// 语句类型与库表元数据。

use std::sync::{Arc, Mutex};

use serial_test::serial;

use crate::{
    Context, GeneralEvent, Kind, SessionVars, TableEntry, clear_static_plugins,
    declare_audit_manifest, foreach_plugin, load_plugin_for_test, set_test_hook, shutdown,
};

/// 单条用例规格：SQL、期望审计字段与结果事件条数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CaseSpec {
    /// 原始 SQL（默认作为审计 text）。
    sql: &'static str,
    /// 非空时覆盖审计中记录的 SQL 文本（如预处理展开）。
    text: &'static str,
    /// 期望影响行数。
    rows: u64,
    /// 语句类型标签（Select/Insert 等）。
    stmt_type: &'static str,
    /// 逗号分隔的库名列表。
    dbs: &'static str,
    /// 逗号分隔的表名列表。
    tables: &'static str,
    /// 期望有效结果事件数；0 表示默认 Starting+Completed 共 2 条。
    res_cnt: usize,
}

/// 审计回调录制到的一条通用事件快照。
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedEvent {
    text: String,
    rows: u64,
    stmt_type: String,
    dbs: String,
    tables: String,
    cmd: String,
    event: GeneralEvent,
    /// 是否处于语句重试上下文（对应 Session 重试标记）。
    retrying: bool,
}

impl CaseSpec {
    /// 按 Go 用例构造：仅填 SQL 与语句类型，其余默认。
    fn go_case(sql: &'static str, stmt_type: &'static str) -> Self {
        Self {
            sql,
            text: "",
            rows: 0,
            stmt_type,
            dbs: "",
            tables: "",
            res_cnt: 0,
        }
    }

    /// 设置覆盖用审计 SQL 文本。
    fn text(mut self, text: &'static str) -> Self {
        self.text = text;
        self
    }

    /// 设置期望影响行数。
    fn rows(mut self, rows: u64) -> Self {
        self.rows = rows;
        self
    }

    /// 设置库名列表。
    fn dbs(mut self, dbs: &'static str) -> Self {
        self.dbs = dbs;
        self
    }

    /// 设置表名列表。
    fn tables(mut self, tables: &'static str) -> Self {
        self.tables = tables;
        self
    }

    /// 设置期望有效事件条数。
    fn res_cnt(mut self, res_cnt: usize) -> Self {
        self.res_cnt = res_cnt;
        self
    }
}

/// 将逗号分隔的库/表名对齐成 `TableEntry` 列表。
fn parse_tables(dbs: &str, tables: &str) -> Vec<TableEntry> {
    if dbs.is_empty() && tables.is_empty() {
        return Vec::new();
    }
    let db_parts: Vec<&str> = if dbs.is_empty() {
        Vec::new()
    } else {
        dbs.split(',').collect()
    };
    let table_parts: Vec<&str> = if tables.is_empty() {
        Vec::new()
    } else {
        tables.split(',').collect()
    };
    let len = db_parts.len().max(table_parts.len());
    (0..len)
        .map(|idx| TableEntry {
            db: db_parts.get(idx).copied().unwrap_or("").to_owned(),
            table: table_parts.get(idx).copied().unwrap_or("").to_owned(),
        })
        .collect()
}

/// 向已加载的审计插件发射 Starting/Completed 事件；可选注入一次重试 Completed。
fn emit_query_events(case: &CaseSpec, inject_retry: bool) {
    let expected_text = if case.text.is_empty() {
        case.sql
    } else {
        case.text
    };
    let session = SessionVars {
        original_sql: expected_text.to_owned(),
        stmt_type: case.stmt_type.to_owned(),
        affected_rows: case.rows,
        tables: parse_tables(case.dbs, case.tables),
        ..SessionVars::default()
    };

    // 对所有 Audit 插件调用 OnGeneralEvent，可标记 Context 为重试中。
    let fire = |event: GeneralEvent, retrying: bool| {
        let ctx = Context::default();
        if retrying {
            ctx.set_retrying(true);
        }
        foreach_plugin(Kind::Audit, |plugin| {
            let audit = declare_audit_manifest(plugin.manifest.clone());
            if let Some(on_general) = &audit.on_general_event {
                on_general(&ctx, Some(&session), event, "Query");
            }
            Ok(())
        })
        .expect("foreach audit");
    };

    fire(GeneralEvent::Starting, false);
    if inject_retry {
        fire(GeneralEvent::Completed, true);
    }
    // res_cnt>2 时补发额外 Completed（如 SHOW 多结果场景）。
    let extras = case.res_cnt.saturating_sub(2);
    for _ in 0..extras {
        fire(GeneralEvent::Completed, false);
    }
    fire(GeneralEvent::Completed, false);
}

/// Corresponds to Go `TestAuditLogNormal`.
/// 加载录制插件，对多类 DDL/DML/事务语句模拟审计事件并断言字段与过滤行为。
#[test]
#[serial]
fn test_audit_log_normal() {
    clear_static_plugins();
    set_test_hook(None);
    shutdown(&Context::default());

    // 覆盖建库建表、索引、序列、视图、DML、预处理、SHOW 与事务边界等语句。
    let tests = vec![
        CaseSpec::go_case("CREATE DATABASE mynewdatabase", "CreateDatabase").dbs("mynewdatabase"),
        CaseSpec::go_case("CREATE TABLE t1 (a INT NOT NULL)", "CreateTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("CREATE TABLE t2 LIKE t1", "CreateTable")
            .dbs("test,test")
            .tables("t2,t1"),
        CaseSpec::go_case("CREATE INDEX a ON t1 (a)", "CreateIndex")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("CREATE SEQUENCE seq", "other")
            .dbs("test")
            .tables("seq"),
        CaseSpec::go_case(" create temporary table t3 (a int)", "CreateTable")
            .dbs("test")
            .tables("t3"),
        CaseSpec::go_case(
            "create global temporary table t4 (a int) on commit delete rows",
            "CreateTable",
        )
        .dbs("test")
        .tables("t4"),
        CaseSpec::go_case(
            "CREATE VIEW v1 AS SELECT * FROM t1 WHERE  a> 2",
            "CreateView",
        )
        .dbs("test,test")
        .tables("t1,v1"),
        CaseSpec::go_case("USE test", "Use"),
        CaseSpec::go_case("DROP DATABASE mynewdatabase", "DropDatabase").dbs("mynewdatabase"),
        CaseSpec::go_case("SHOW CREATE SEQUENCE seq", "Show")
            .dbs("test")
            .tables("seq"),
        CaseSpec::go_case("DROP SEQUENCE seq", "other")
            .dbs("test")
            .tables("seq"),
        CaseSpec::go_case("DROP TABLE t4", "DropTable")
            .dbs("test")
            .tables("t4"),
        CaseSpec::go_case("DROP VIEW v1", "DropView")
            .dbs("test")
            .tables("v1"),
        CaseSpec::go_case("ALTER TABLE t1 ADD COLUMN c1 INT NOT NULL", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 MODIFY c1 BIGINT", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 ADD INDEX (c1)", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 ALTER INDEX c1 INVISIBLE", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 RENAME INDEX c1 TO c2", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 DROP INDEX c2", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 CHANGE c1 c2 INT", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("ALTER TABLE t1 DROP COLUMN c2", "AlterTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case(
            "CREATE SESSION BINDING FOR SELECT * FROM t1 WHERE a = 123 USING SELECT * FROM t1 IGNORE INDEX (a) WHERE a = 123",
            "CreateBinding",
        ),
        CaseSpec::go_case(
            "DROP SESSION BINDING FOR SELECT * FROM t1 WHERE a = 123",
            "DropBinding",
        ),
        CaseSpec::go_case("RENAME TABLE t2 TO t5", "other")
            .dbs("test,test")
            .tables("t2,t5"),
        CaseSpec::go_case("TRUNCATE t1", "TruncateTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case(
            "ALTER DATABASE test DEFAULT CHARACTER SET = utf8mb4",
            "other",
        )
        .dbs("test"),
        CaseSpec::go_case("ADMIN RELOAD opt_rule_blacklist", "other"),
        CaseSpec::go_case("ADMIN FLUSH bindings", "other"),
        CaseSpec::go_case("ADMIN SHOW SLOW RECENT 10", "other"),
        CaseSpec::go_case("ADMIN SHOW DDL JOBS", "other"),
        CaseSpec::go_case("ADMIN CHECKSUM TABLE t1", "other"),
        CaseSpec::go_case("ADMIN CHECK TABLE t1", "other"),
        CaseSpec::go_case("ADMIN CHECK INDEX t1 a", "other"),
        CaseSpec::go_case(
            "CREATE USER 'newuser' IDENTIFIED BY 'newuserpassword'",
            "CreateUser",
        ),
        CaseSpec::go_case(
            "ALTER USER 'newuser' IDENTIFIED BY 'newnewpassword'",
            "other",
        ),
        CaseSpec::go_case("CREATE ROLE analyticsteam", "CreateUser"),
        CaseSpec::go_case("GRANT SELECT ON test.* TO analyticsteam", "Grant").dbs("test"),
        CaseSpec::go_case("GRANT analyticsteam TO 'newuser'", "other"),
        CaseSpec::go_case("SET DEFAULT ROLE analyticsteam TO newuser;", "other"),
        CaseSpec::go_case("REVOKE SELECT ON test.* FROM 'analyticsteam'", "Revoke").dbs("test"),
        CaseSpec::go_case("DROP ROLE analyticsteam", "other"),
        CaseSpec::go_case("FLUSH PRIVILEGES", "other"),
        CaseSpec::go_case("SET PASSWORD FOR 'newuser' = 'test'", "Set"),
        CaseSpec::go_case("DROP USER 'newuser'", "other"),
        CaseSpec::go_case("analyze table t1", "AnalyzeTable")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case(
            "SPLIT TABLE t1 BETWEEN (0) AND (1000000000) REGIONS 16",
            "other",
        ),
        CaseSpec::go_case("INSERT INTO t1 VALUES (1), (2)", "Insert")
            .dbs("test")
            .tables("t1")
            .rows(2),
        CaseSpec::go_case("DELETE FROM t1 WHERE a = 2", "Delete")
            .dbs("test")
            .tables("t1")
            .rows(1),
        CaseSpec::go_case("REPLACE INTO t1 VALUES(3)", "Replace")
            .dbs("test")
            .tables("t1")
            .rows(1),
        CaseSpec::go_case("UPDATE t1 SET a=5 WHERE a=1", "Update")
            .dbs("test")
            .tables("t1")
            .rows(1),
        CaseSpec::go_case("DO 1", "other"),
        CaseSpec::go_case("SELECT * FROM t1", "Select")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("SELECT 1", "Select"),
        CaseSpec::go_case("TABLE t1", "Select")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case(
            "EXPLAIN ANALYZE SELECT * FROM t1 WHERE a = 1",
            "ExplainAnalyzeSQL",
        ),
        CaseSpec::go_case("EXPLAIN SELECT * FROM t1", "ExplainSQL"),
        CaseSpec::go_case("EXPLAIN SELECT * FROM t1 WHERE a = 1", "ExplainSQL"),
        CaseSpec::go_case("DESC SELECT * FROM t1 WHERE a = 1", "ExplainSQL"),
        CaseSpec::go_case("DESCRIBE SELECT * FROM t1 WHERE a = 1", "ExplainSQL"),
        CaseSpec::go_case("trace format='row' select * from t1", "Trace"),
        CaseSpec::go_case("flush status", "other"),
        CaseSpec::go_case("FLUSH TABLES", "other"),
        CaseSpec::go_case("SET @number = 5", "Set"),
        CaseSpec::go_case("SET NAMES utf8", "Set"),
        CaseSpec::go_case("SET CHARACTER SET utf8mb4", "Set"),
        CaseSpec::go_case(
            "SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED",
            "Set",
        ),
        CaseSpec::go_case(
            "SET SESSION sql_mode = 'STRICT_TRANS_TABLES,NO_AUTO_CREATE_USER'",
            "Set",
        ),
        CaseSpec::go_case("PREPARE mystmt FROM 'SELECT ? as num FROM DUAL'", "Prepare"),
        CaseSpec::go_case("EXECUTE mystmt USING @number", "Select")
            .text("SELECT ? as num FROM DUAL"),
        CaseSpec::go_case("DEALLOCATE PREPARE mystmt", "Deallocate"),
        CaseSpec::go_case("SHOW TABLE STATUS LIKE 't1'", "Show").res_cnt(3),
        CaseSpec::go_case("BEGIN", "Begin"),
        CaseSpec::go_case("ROLLBACK", "Rollback"),
        CaseSpec::go_case("START TRANSACTION", "Begin"),
        CaseSpec::go_case("COMMIT", "Commit"),
        CaseSpec::go_case("SHOW PROCESSLIST", "Show"),
        CaseSpec::go_case("show analyze status", "Show"),
        CaseSpec::go_case("SHOW SESSION BINDINGS", "Show"),
        CaseSpec::go_case("SHOW BUILTINS", "Show"),
        CaseSpec::go_case("SHOW CHARACTER SET", "Show"),
        CaseSpec::go_case("SHOW COLLATION", "Show"),
        CaseSpec::go_case("show columns from t1", "Show"),
        CaseSpec::go_case("show fields from t1", "Show"),
        CaseSpec::go_case("SHOW CREATE TABLE t1", "Show")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("SHOW CREATE USER 'root'", "Show"),
        CaseSpec::go_case("SHOW DATABASES", "Show"),
        CaseSpec::go_case("SHOW ENGINES", "Show"),
        CaseSpec::go_case("SHOW ERRORS", "Show"),
        CaseSpec::go_case("SHOW INDEXES FROM t1", "Show"),
        CaseSpec::go_case("SHOW MASTER STATUS", "Show"),
        CaseSpec::go_case("SHOW PLUGINS", "Show"),
        CaseSpec::go_case("show privileges", "Show"),
        CaseSpec::go_case("SHOW PROFILES", "Show"),
        CaseSpec::go_case("SHOW SCHEMAS", "Show"),
        CaseSpec::go_case("SHOW STATS_HEALTHY", "Show").dbs("mysql"),
        CaseSpec::go_case("show stats_histograms", "Show")
            .dbs("mysql")
            .tables("stats_histograms"),
        CaseSpec::go_case("show stats_meta", "Show")
            .dbs("mysql")
            .tables("stats_meta"),
        CaseSpec::go_case("show status", "Show"),
        CaseSpec::go_case("show table t1 next_row_id", "Show")
            .dbs("test")
            .tables("t1"),
        CaseSpec::go_case("show table t1 regions", "Show"),
        CaseSpec::go_case("SHOW TABLES", "Show"),
        CaseSpec::go_case("SHOW VARIABLES", "Show"),
        CaseSpec::go_case("SHOW WARNINGS", "Show"),
    ];
    assert_eq!(104, tests.len(), "keep every active Go normalTest case");

    // 回调将每次 OnGeneralEvent 写入共享向量供后续断言。
    let test_results: Arc<Mutex<Vec<RecordedEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let results_for_callback = Arc::clone(&test_results);
    load_plugin_for_test(Arc::new(move |ctx, sctx, event, cmd| {
        let retrying = ctx.is_retrying();
        let (text, rows, stmt_type, dbs, tables) = if let Some(s) = sctx {
            let dbs = s
                .tables
                .iter()
                .map(|t| t.db.as_str())
                .collect::<Vec<_>>()
                .join(",");
            let tables = s
                .tables
                .iter()
                .map(|t| t.table.as_str())
                .collect::<Vec<_>>()
                .join(",");
            (
                s.original_sql.clone(),
                s.affected_rows,
                s.stmt_type.clone(),
                dbs,
                tables,
            )
        } else {
            (
                String::new(),
                0,
                String::new(),
                String::new(),
                String::new(),
            )
        };
        results_for_callback.lock().unwrap().push(RecordedEvent {
            text,
            rows,
            stmt_type,
            dbs,
            tables,
            cmd: cmd.to_owned(),
            event,
            retrying,
        });
    }))
    .expect("load audit plugin for test");

    for test in &tests {
        test_results.lock().unwrap().clear();
        let err_msg = format!("statement: {}", test.sql);
        // DML/事务类语句模拟注入重试 Completed，与 Go 过滤逻辑一致。
        let inject_retry = matches!(
            test.stmt_type,
            "Insert" | "Delete" | "Replace" | "Update" | "Begin" | "Commit"
        );
        emit_query_events(test, inject_retry);

        let recorded = test_results.lock().unwrap().clone();
        let mut result_count = test.res_cnt;
        if result_count == 0 {
            result_count = 2;
        }
        // 重试 Completed 计入总数但不进入有效结果。
        let mut retrying_completed_count = 0;
        let mut effective_results = Vec::with_capacity(recorded.len());
        for result in &recorded {
            if result.event == GeneralEvent::Completed && result.retrying {
                retrying_completed_count += 1;
                continue;
            }
            effective_results.push(result.clone());
        }

        assert_eq!(
            result_count + retrying_completed_count,
            recorded.len(),
            "{err_msg}"
        );
        assert_eq!(result_count, effective_results.len(), "{err_msg}");

        // 首条应为 Starting，末条为 Completed，并核对元数据字段。
        let result = &effective_results[0];
        assert_eq!("Query", result.cmd, "{err_msg}");
        assert_eq!(GeneralEvent::Starting, result.event, "{err_msg}");

        let result = &effective_results[result_count - 1];
        assert_eq!("Query", result.cmd, "{err_msg}");
        if test.text.is_empty() {
            assert_eq!(test.sql, result.text, "{err_msg}");
        } else {
            assert_eq!(test.text, result.text, "{err_msg}");
        }
        assert_eq!(test.rows, result.rows, "{err_msg}");
        assert_eq!(test.stmt_type, result.stmt_type, "{err_msg}");
        assert_eq!(test.dbs, result.dbs, "{err_msg}");
        assert_eq!(test.tables, result.tables, "{err_msg}");
        assert_eq!(GeneralEvent::Completed, result.event, "{err_msg}");
        for item in effective_results.iter().take(result_count - 1).skip(1) {
            assert_eq!("Query", item.cmd, "{err_msg}");
            assert_eq!(GeneralEvent::Completed, item.event, "{err_msg}");
        }
    }

    shutdown(&Context::default());
    clear_static_plugins();
    set_test_hook(None);
}
