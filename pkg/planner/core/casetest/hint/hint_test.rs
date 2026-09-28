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

// Optimizer Hint 解析与查询块归属用例。
//
// 对应 Go `hint_test.go`：在 narrow runtime 无法跑完整 explain 时，直连
// `astersql-parser` 与 `astersql-util-hint` 生产 API，覆盖 hint 语法、
// QBHintHandler（查询块 hint 处理器）与 StmtHints/PlanHints 结构化解析。

// 本文件对应 pkg/planner/core/casetest/hint/hint_test.go。Go 版本几乎每个测试都要
// 通过 `testkit.RunTestUnderCascades(WithDomain)` 建立真实 join/view/CTE/TiFlash/
// partition 场景，再用 `explain`/`show warnings` 与 testdata 黄金文件比较完整物理计划
// 文本。见 planstats/plan_stats_test.rs 顶部同款注释：当前 `pkg/session/runtime.rs` 的
// narrow 会话运行时只在真实用户表上做谓词列收集，对 join/view/CTE/子查询等一律返回
// "0 record sets" 或直接报错，且不存在 Go 版 `RunTestUnderCascadesWithDomain` 那种可在
// Volcano/Cascades 两种优化器之间切换的 test helper；这些是本任务 writes 清单之外的生产
// 能力缺口。因此这里改为对 hint 处理链路里真正编译、真正跑起来的两层生产代码做直连测试：
//
//   1. `astersql-parser`：`/*+ ... */` 注释里的 hint 语法本身在词法/语法层解析，未知
//      hint 名会在 `Parser::Parse` 的返回值里产生 warning（不依赖 optimizer）。
//   2. `astersql-util-hint`：`QBHintHandler::Process`（query block 归属/去重）、
//      `ExtractTableHintsFromStmtNode`（从真实 AST 摘出 hint 列表）、
//      `ParseStmtHints`/`ParsePlanHints`（把 hint 列表转成语句级/计划级结构化选项）。
//
// 每个测试都先用真实 SQL 文本喂给 `Parser::default().ParseOneStmt`/`Parse`，而不是手工
// 构造 AST，这样才能覆盖 Go 测试真正依赖的“hint 注释语法 -> AST -> QBHintHandler ->
// PlanHints/StmtHints”整条链路；分支覆盖对齐 Go 用例名，而不是简化成空断言。

#![allow(non_snake_case, non_upper_case_globals)]

use astersql_parser::ast;
use astersql_parser::{New, Parser};
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::ConvertRowsToStrings;
use astersql_util_hint::{
    ExtractTableHintsFromStmtNode, HintHJ, HintLeading, HintMaxExecutionTime, HintMemoryQuota,
    HintSMJ, HintStraightJoin, HintUseIndex, NewQBHintHandler, ParsePlanHints, ParseStmtHints,
    QBHintHandler, hintWarnHandler,
};

use crate::main_test::HINT_INTEGRATION_TESTS;

/// 记录 hint 处理过程中产生的 warning，供断言无歧义绑定。
#[derive(Default)]
struct RecordingWarnHandler {
    warnings: Vec<String>,
}

impl hintWarnHandler for RecordingWarnHandler {
    fn SetHintWarning(&mut self, warn: String) {
        self.warnings.push(warn);
    }
    fn SetHintWarningFromError(&mut self, err: &dyn std::error::Error) {
        self.warnings.push(err.to_string());
    }
}

/// 允许任意 SET_VAR：ParseStmtHints 的校验回调恒返回通过。
fn allow_any_set_var(
    _name: String,
    _value: String,
) -> (bool, Option<astersql_util_hint::errors::Error>) {
    (true, None)
}

/// 无假设索引（hypothetical index）：返回 -1 表示未命中。
fn no_hypo_index(
    _db: ast::CIStr,
    _table: ast::CIStr,
    _column: ast::CIStr,
) -> (i32, Option<astersql_util_hint::errors::Error>) {
    (-1, None)
}

/// 用真实 Parser 解析单条 SELECT，失败则 panic 带上 SQL。
fn parse_select(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// Go hint fixture 中的每条 SQL 都必须仍能经过真实 parser，避免 TestMain 只加载
/// 文件而测试场景却悄悄失联。
#[test]
fn test_integration_fixture_sql_remains_parseable() {
    let suite = crate::main_test::load_integration_suite();
    for name in HINT_INTEGRATION_TESTS {
        let (input, _output) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("missing hint integration case {name}: {error}"));
        for sql in input
            .as_array()
            .unwrap_or_else(|| panic!("{name} input cases must be an array"))
        {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("{name} fixture SQL must be a string"));
            Parser::default()
                .Parse(sql, "", "")
                .unwrap_or_else(|error| {
                    panic!("{name} fixture SQL failed to parse `{sql}`: {error}")
                });
        }
    }
}

/// 对应 Go `TestReadFromStorageHint`：真实建立 TiKV/TiFlash 元数据并逐条回放黄金计划与警告。
#[test]
fn test_read_from_storage_hint_matches_go_fixture() {
    let mut mismatches = Vec::new();
    for cascades in [false, true] {
        let suite = crate::main_test::load_integration_suite();
        let (input, output) = suite
            .LoadTestCasesByName("TestReadFromStorageHint", cascades)
            .expect("load TestReadFromStorageHint fixture");
        let input = input.as_array().expect("hint input cases array");
        let output = output.as_array().expect("hint output cases array");
        assert_eq!(input.len(), output.len());

        let (store, domain) = CreateMockStoreAndDomain();
        let mut tk = TestKit::new(store);
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_cascades_planner={}",
                if cascades { "ON" } else { "OFF" }
            ),
            Vec::new(),
        );
        tk.MustExec("set session tidb_allow_mpp=OFF", Vec::new());
        tk.MustExec("set @@session.tidb_allow_tiflash_cop=ON", Vec::new());
        tk.MustExec("create table t(a int, b int, index ia(a))", Vec::new());
        tk.MustExec("create table tt(a int, b int, primary key(a))", Vec::new());
        tk.MustExec("create table ttt(a int, primary key (a desc))", Vec::new());
        for table in ["t", "tt", "ttt"] {
            domain
                .set_tiflash_replica_for_test("test", table, 1, true)
                .unwrap_or_else(|error| panic!("set test.{table} TiFlash replica: {error}"));
        }

        for (index, (sql, expected)) in input.iter().zip(output).enumerate() {
            let sql = sql
                .as_str()
                .unwrap_or_else(|| panic!("input[{index}] must be SQL text"));
            assert_eq!(
                expected.get("SQL").and_then(|value| value.as_str()),
                Some(sql)
            );
            let expected_plan = expected
                .get("Plan")
                .and_then(|value| value.as_array())
                .unwrap_or_else(|| panic!("output[{index}] lacks Plan"))
                .iter()
                .map(|row| row.as_str().expect("plan row must be text").to_owned())
                .collect::<Vec<_>>();
            let expected_warnings = expected
                .get("Warn")
                .and_then(|value| value.as_array())
                .map(|warnings| {
                    warnings
                        .iter()
                        .map(|warning| {
                            warning
                                .as_str()
                                .expect("warning row must be text")
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let actual_plan = ConvertRowsToStrings(&tk.MustQuery(sql, Vec::new()).Rows());
            if actual_plan != expected_plan {
                mismatches.push(format!(
                    "cascades={cascades}, sql={sql}\nactual plan: {actual_plan:?}\nexpected plan: {expected_plan:?}"
                ));
            }
            let actual_warnings =
                ConvertRowsToStrings(&tk.MustQuery("show warnings", Vec::new()).Rows())
                    .into_iter()
                    .map(|warning| {
                        if let Some(message) = warning.strip_prefix("Warning 1815 ") {
                            format!("[planner:1815]{message}")
                        } else {
                            warning
                        }
                    })
                    .collect::<Vec<_>>();
            if actual_warnings != expected_warnings {
                mismatches.push(format!(
                    "cascades={cascades}, sql={sql}\nactual warnings: {actual_warnings:?}\nexpected warnings: {expected_warnings:?}"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "TestReadFromStorageHint mismatches:\n{}",
        mismatches.join("\n\n")
    );
}

/// 从语句 AST 提取表级 optimizer hint 列表。
fn select_table_hints(stmt: &dyn ast::Node) -> Vec<ast::TableOptimizerHint> {
    ExtractTableHintsFromStmtNode(stmt, None)
}

// test_hints 对应 Go 的 TestHints：验证一批常见 optimizer hint 能从真实 SQL 文本被
// 正确解析并落到 `PlanHints`/`StmtHints` 的结构化字段上。Go 版本额外比较 explain
// plan_tree 文本，这里改为直接断言 hint 语义结构，因为 explain 文本比较依赖narrow
// runtime 不支持的完整物理计划生成。
/// 回归：常见 USE_INDEX / JOIN / LEADING / MAX_EXECUTION_TIME / MEMORY_QUOTA 解析。
#[test]
fn test_hints_recognizes_supported_optimizer_hints_from_real_sql() {
    let cases: &[(&str, &str)] = &[
        ("SELECT /*+ USE_INDEX(t1, idx) */ * FROM t1", HintUseIndex),
        ("SELECT /*+ MERGE_JOIN(t1) */ * FROM t1", HintSMJ),
        ("SELECT /*+ HASH_JOIN(t1) */ * FROM t1", HintHJ),
        ("SELECT /*+ LEADING(t1) */ * FROM t1", HintLeading),
        (
            "SELECT /*+ MAX_EXECUTION_TIME(1000) */ * FROM t1",
            HintMaxExecutionTime,
        ),
        (
            "SELECT /*+ MEMORY_QUOTA(1 MB) */ * FROM t1",
            HintMemoryQuota,
        ),
    ];
    for (sql, expected_name) in cases {
        let stmt = parse_select(sql);
        let hints = select_table_hints(stmt.as_ref());
        assert_eq!(hints.len(), 1, "{sql}: hints={hints:?}");
        assert_eq!(hints[0].HintName.L, *expected_name, "{sql}");

        let (stmt_hints, offs, warns) = ParseStmtHints(
            hints.clone(),
            allow_any_set_var,
            no_hypo_index,
            "test".into(),
            0,
        );
        assert!(warns.is_empty(), "{sql}: unexpected warnings {warns:?}");
        assert!(stmt_hints.QueryHasHints);

        let mut processor = QBHintHandler::default();
        let mut warn_handler = RecordingWarnHandler::default();
        let (plan_hints, _flags) = ParsePlanHints(
            hints,
            1,
            "test".into(),
            &mut processor,
            false,
            false,
            false,
            false,
            &mut warn_handler,
        )
        .unwrap_or_else(|error| panic!("{sql}: ParsePlanHints failed: {error}"));
        assert!(
            warn_handler.warnings.is_empty(),
            "{sql}: unexpected plan-hint warnings {:?}",
            warn_handler.warnings
        );
        match *expected_name {
            HintUseIndex => assert_eq!(plan_hints.IndexHintList.len(), 1),
            HintSMJ => assert_eq!(plan_hints.SortMergeJoin.len(), 1),
            HintHJ => assert_eq!(plan_hints.HashJoin.len(), 1),
            HintLeading => assert_eq!(plan_hints.LeadingJoinOrder.len(), 1),
            HintMaxExecutionTime => assert_eq!(stmt_hints.MaxExecutionTime, 1000),
            HintMemoryQuota => assert!(stmt_hints.MemQuotaQuery > 0),
            other => panic!("unexpected case {other}"),
        }
        let _ = offs;
    }
}

// test_hints_straight_join_and_leading_are_statement_and_plan_scoped 覆盖 Go 用例里
// straight_join（语句级，无需表名）与多表 leading（计划级，需要按 join 顺序排列表名）
// 两种不同的 hint 归类边界。
/// 回归：STRAIGHT_JOIN 与多表 LEADING 的语句级 / 计划级归类边界。
#[test]
fn test_hints_straight_join_and_leading_are_statement_and_plan_scoped() {
    let stmt = parse_select("SELECT /*+ STRAIGHT_JOIN() */ t1.a FROM t1, t2 WHERE t1.a = t2.a");
    let hints = select_table_hints(stmt.as_ref());
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].HintName.L, HintStraightJoin);

    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut warn_handler,
    )
    .expect("STRAIGHT_JOIN hint should parse without a table list");
    assert!(plan_hints.StraightJoinOrder);
    assert!(warn_handler.warnings.is_empty());

    let stmt = parse_select("SELECT /*+ LEADING(t2, t1) */ t1.a FROM t1, t2 WHERE t1.a = t2.a");
    let hints = select_table_hints(stmt.as_ref());
    let mut processor = QBHintHandler::default();
    let mut warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut warn_handler,
    )
    .expect("LEADING hint with two tables should parse");
    assert_eq!(plan_hints.LeadingJoinOrder.len(), 2);
    assert_eq!(plan_hints.LeadingJoinOrder[0].TblName.L, "t2");
    assert_eq!(plan_hints.LeadingJoinOrder[1].TblName.L, "t1");
}

// test_qb_hint_handler_duplicate_objects 对应 Go 的
// TestQBHintHandlerDuplicateObjects：该用例的核心断言是 hint 处理器在自连接（同一张表
// 出现两次、各自带别名）场景下，能把 hint 精确绑定到目标别名对应的查询块，不会因为两份
// "重复对象"（同一张物理表）互相干扰而产生 warning。Go 版通过 CTE + JOIN 触发这个路径；
// narrow runtime 不支持 CTE/JOIN 执行，这里改用等价的、真正会走 `QBHintHandler::Process`
// 里 `processJoin`/`processResultSet` 递归的自连接 AST，验证同样的“不产生 warning、
// 查询块归属正确”结论。
/// 回归：自连接双别名下 inl_join 精确绑定目标别名且无 warning。
#[test]
fn test_qb_hint_handler_duplicate_objects() {
    let (_domain, mut tk, _table_id) = {
        let (store, domain) = CreateMockStoreAndDomain();
        let mut tk = TestKit::new(store);
        tk.MustExec(
            "CREATE TABLE t_employees (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             fname VARCHAR(25) NOT NULL, lname VARCHAR(25) NOT NULL, \
             store_id INT NOT NULL, department_id INT NOT NULL)",
            Vec::new(),
        );
        tk.MustExec(
            "ALTER TABLE t_employees ADD INDEX idx(department_id)",
            Vec::new(),
        );
        let table_id = domain
            .table_by_name("test", "t_employees")
            .expect("t_employees metadata")
            .ID;
        (domain, tk, table_id)
    };
    // 上面两条 DDL 复刻 Go 用例的建表/加索引前置步骤，确认这部分在当前生产代码下真的可以
    // 执行（narrow runtime 支持单表 DDL）；后续的 hint 归属断言则直接驱动
    // `QBHintHandler`，不经过 narrow runtime 的 SELECT 执行路径。
    tk.MustExec("select id from t_employees", Vec::new());

    let sql = "SELECT /*+ inl_join(e) */ em.* FROM t_employees em JOIN t_employees e \
               ON em.store_id = e.department_id";
    let stmt = parse_select(sql);
    let mut processor = NewQBHintHandler(Some(Box::new(RecordingWarnHandler::default())));
    let stmt = processor.Process(stmt);

    let select = stmt
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .expect("top-level statement stays a SelectStmt after Process");
    assert_eq!(select.QueryBlockOffset, 1);

    let current_hints = processor.GetCurrentStmtHints(&select.TableHints, 1, None);
    assert_eq!(current_hints.len(), 1, "hints={current_hints:?}");
    assert_eq!(current_hints[0].HintName.L, "inl_join");
    assert_eq!(current_hints[0].Tables.len(), 1);
    assert_eq!(current_hints[0].Tables[0].TableName.L, "e");

    let mut plan_warn_handler = RecordingWarnHandler::default();
    let (plan_hints, _flags) = ParsePlanHints(
        current_hints,
        1,
        "test".into(),
        &mut processor,
        false,
        false,
        false,
        false,
        &mut plan_warn_handler,
    )
    .expect("inl_join hint targeting alias `e` should parse without ambiguity");
    assert_eq!(plan_hints.IndexJoin.INLJTables.len(), 1);
    assert_eq!(plan_hints.IndexJoin.INLJTables[0].TblName.L, "e");
    assert!(
        plan_warn_handler.warnings.is_empty(),
        "duplicate physical table object must not produce a warning: {:?}",
        plan_warn_handler.warnings
    );
}

// test_optimizer_cost_factor_hints 对应 Go 的 TestOptimizerCostFactorHints：真正决定
// 两条 explain 输出 plan cost 高低的是 SET_VAR hint 是否被解析并覆盖到对应 session
// 变量。narrow runtime 不会生成真实 verbose cost，因此这里直接验证 SET_VAR hint 被
// `ParseStmtHints` 正确解析进 `StmtHints.SetVars`，这是 Go 断言背后真正依赖的机制。
/// 回归：代价因子相关 SET_VAR 写入 StmtHints.SetVars。
#[test]
fn test_optimizer_cost_factor_hints_set_var_parses_into_stmt_hints() {
    for (var_name, value) in [
        ("tidb_opt_table_full_scan_cost_factor", "2"),
        ("tidb_opt_table_reader_cost_factor", "2"),
        ("tidb_opt_table_range_scan_cost_factor", "2"),
        ("tidb_opt_index_scan_cost_factor", "2"),
        ("tidb_opt_index_reader_cost_factor", "2"),
    ] {
        let sql = format!("SELECT /*+ SET_VAR({var_name}={value}) */ * FROM t");
        let stmt = parse_select(&sql);
        let hints = select_table_hints(stmt.as_ref());
        assert_eq!(hints.len(), 1, "{sql}");
        let (stmt_hints, _offs, warns) =
            ParseStmtHints(hints, allow_any_set_var, no_hypo_index, "test".into(), 0);
        assert!(warns.is_empty(), "{sql}: unexpected warnings {warns:?}");
        assert_eq!(
            stmt_hints.SetVars.get(var_name).map(String::as_str),
            Some(value),
            "{sql}"
        );
    }
}

// test_optimize_hint_on_partition_table_unknown_hint_warnings 对应 Go 用例结尾的
// MAX_EXECUTION_TIME + 未知 hint warning 计数断言。Go 里 `dtc(name=tt)`、
// `unknow(t1,t2)` 之所以产生 warning，是因为 hint 注释在词法/语法层就无法识别这些
// hint 名——这一步发生在 `astersql-parser::Parser::Parse` 里，不依赖 optimizer/
// executor，因此可以在不执行 SQL 的情况下如实复现同一条生产代码路径。
/// 回归：未知 hint 名在 Parser 层降级为 warning，计数与 Go 一致。
#[test]
fn test_optimize_hint_on_partition_table_unknown_hint_warnings() {
    let mut parser = New();
    let (_statements, warnings) = parser
        .Parse("SELECT /*+ MAX_EXECUTION_TIME(10) */ 1", "", "")
        .expect("fully recognized hint list should parse cleanly");
    assert_eq!(warnings.len(), 0);

    let mut parser = New();
    let (_statements, warnings) = parser
        .Parse(
            "SELECT /*+ MAX_EXECUTION_TIME(10), dtc(name=tt) */ 1",
            "",
            "",
        )
        .expect("unknown hint name degrades to a warning, not a parse error");
    assert_eq!(warnings.len(), 1, "warnings={warnings:?}");

    let mut parser = New();
    let (_statements, warnings) = parser
        .Parse(
            "SELECT /*+ MAX_EXECUTION_TIME(10), dtc(name=tt) unknow(t1,t2) */ 1",
            "",
            "",
        )
        .expect("multiple unknown hint names each degrade to their own warning");
    assert_eq!(warnings.len(), 2, "warnings={warnings:?}");
}
