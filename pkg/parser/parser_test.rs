// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// parser 主测试集使用 Rust 自有合约用例，覆盖 SELECT/DDL/分区/窗口函数/
// 字符集与绑定等语法；并用自定义分配器观测 INSERT 内存分配。

#![allow(dead_code)]

use crate::parser_contract_cases::{parser_contract_cases, parser_contract_strings};
use parser_ast::{RecommendIndexStmt, util::IsReadOnly};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;

/// 跟踪分配字节数的全局分配器，用于内存分配测试。
struct TestAllocator;

thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static THREAD_ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        TRACK_ALLOCATIONS.with(|tracking| {
            if tracking.get() {
                THREAD_ALLOCATED.with(|allocated| allocated.set(allocated.get() + layout.size()));
            }
        });
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, new_size) };
        TRACK_ALLOCATIONS.with(|tracking| {
            if tracking.get() {
                THREAD_ALLOCATED.with(|allocated| allocated.set(allocated.get() + new_size));
            }
        });
        pointer
    }
}

#[global_allocator]
/// 进程级测试分配器实例。
static TEST_ALLOCATOR: TestAllocator = TestAllocator;

/// 字符集相关测试互斥锁，避免全局状态竞态。
static CHARSET_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 确保 parser::New 工厂可链接。
const _PARSER_FACTORY: fn() -> Box<parser::Parser> = parser::New;

/// 按选项运行 Rust 自有用例表（窗口函数/MariaDB 开关）。
fn run_contract_table_with_options(name: &str, enable_window_func: bool, maria_db: bool) {
    let cases = parser_contract_cases(name);
    eprintln!("{name}: executing {} Rust contract cases", cases.len());
    for case in cases {
        let mut parser = parser::New();
        parser.EnableWindowFunc(enable_window_func);
        parser.SetMariaDB(maria_db);
        let result = parser.Parse(case.sql, "", "");
        assert_eq!(
            result.is_ok(),
            case.ok,
            "{name}: SQL {:?}: {:?}",
            case.sql,
            result.as_ref().err().map(ToString::to_string)
        );
        if !case.ok {
            continue;
        }
        let (original, _) = result.unwrap();
        let (restored, _) = parser
            .Parse(&case.restored, "", "")
            .unwrap_or_else(|error| panic!("{name}: restore target {:?}: {error}", case.restored));
        assert_eq!(original.len(), restored.len(), "{name}: SQL {:?}", case.sql);
        for (source_node, restored_node) in original.iter().zip(&restored) {
            assert_eq!(
                source_node.as_any().type_id(),
                restored_node.as_any().type_id(),
                "{name}: AST type differs for {:?} -> {:?}",
                case.sql,
                case.restored
            );
        }
    }
}

/// 运行 Rust 自有用例表的便捷封装。
fn run_contract_table(name: &str, maria_db: bool) {
    run_contract_table_with_options(name, false, maria_db);
}

macro_rules! contract_table_tests {
    ($($rust_name:ident => $contract_name:literal;)+) => {
        $(
            #[test]
            pub fn $rust_name() {
                run_contract_table($contract_name, false);
            }
        )+
    };
}

contract_table_tests! {
    test_recommend_index => "TestRecommendIndex";
    test_admin_stmt => "TestAdminStmt";
    test_dml_stmt => "TestDMLStmt";
    test_dba_stmt => "TestDBAStmt";
    test_expression => "TestExpression";
    test_builtin => "TestBuiltin";
    test_identifier => "TestIdentifier";
    test_ddl => "TestDDL";
    test_type => "TestType";
    test_privilege => "TestPrivilege";
    test_comment => "TestComment";
    test_set_operator => "TestSetOperator";
    test_like_escape => "TestLikeEscape";
    test_lock_unlock_tables => "TestLockUnlockTables";
    test_with_rollup => "TestWithRollup";
    test_index_hint => "TestIndexHint";
    test_sql_result => "TestSQLResult";
    test_escape => "TestEscape";
    test_explain => "TestExplain";
    test_prepare => "TestPrepare";
    test_deallocate => "TestDeallocate";
    test_execute => "TestExecute";
    test_trace => "TestTrace";
    test_session_manage => "TestSessionManage";
    test_parse_show_open_tables => "TestParseShowOpenTables";
}

#[test]
fn recommend_index_return_result_ast_is_read_only() {
    let statement = parser::New()
        .ParseOneStmt(
            "recommend index return result for 'select * from t where a=1' with A = 1",
            "",
            "",
        )
        .expect("RETURN RESULT should parse");
    let recommend = statement
        .as_any()
        .downcast_ref::<RecommendIndexStmt>()
        .expect("RETURN RESULT should build RecommendIndexStmt");

    assert_eq!(recommend.Action, "return");
    assert_eq!(recommend.SQL, "select * from t where a=1");
    assert_eq!(recommend.Options.len(), 1);
    assert!(IsReadOnly(statement.as_ref(), true));
}

#[test]
fn analyze_default_ndvrate() {
    let sql = "analyze table t with default ndvrate";
    let statement = parser::New()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    let analyze = statement
        .as_any()
        .downcast_ref::<parser_ast::AnalyzeTableStmt>()
        .expect("ANALYZE TABLE AST");

    assert_eq!(analyze.AnalyzeOpts.len(), 1);
    assert_eq!(
        analyze.AnalyzeOpts[0].Type,
        parser_ast::AnalyzeOptionType::NDVRate
    );
    assert!(analyze.AnalyzeOpts[0].Value.is_none());
    assert_eq!(
        analyze.restore().unwrap(),
        "ANALYZE TABLE `t` WITH DEFAULT NDVRATE"
    );
}

#[test]
/// 基础 SQL 正反例解析。
pub fn test_simple() {
    let mut parser = parser::New();
    for keyword in parser_contract_strings("TestSimple", "reservedKws") {
        for sql in [
            format!("SELECT * FROM db.{keyword};"),
            format!("SELECT * FROM {keyword}.desc"),
            format!("SELECT t.{keyword} FROM t"),
        ] {
            parser
                .ParseOneStmt(&sql, "", "")
                .unwrap_or_else(|error| panic!("{sql}: {error}"));
        }
    }
    for keyword in parser_contract_strings("TestSimple", "unreservedKws") {
        let sql = format!("SELECT {keyword} FROM tbl");
        parser
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let (statements, _) = parser
        .Parse(
            "CREATE TABLE foo (a SMALLINT UNSIGNED, b INT UNSIGNED); -- foo\nSELECT --1 FROM foo",
            "",
            "",
        )
        .unwrap();
    assert_eq!(statements.len(), 2);
    let statement = parser
        .ParseOneStmt("/*!40101 SET character_set_client = utf8 */", "", "")
        .unwrap();
    assert!(statement.as_any().is::<parser_ast::SetStmt>());
    let statement = parser
        .ParseOneStmt(
            "INSERT INTO blobtable(a) VALUES ('/*! truncated */')",
            "",
            "",
        )
        .unwrap();
    let insert = statement
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert_eq!(insert.Lists.len(), 1);
    assert_eq!(insert.Lists[0].len(), 1);
    assert!(matches!(
        &insert.Lists[0][0].Kind,
        parser_ast::ExprKind::Value(value) if value == "/*! truncated */"
    ));
    let statement = parser
        .ParseOneStmt("SELECT CONVERT('111', SIGNED)", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().map(|expr| &expr.Kind),
        Some(parser_ast::ExprKind::Cast {
            FunctionType: parser_ast::CastFunctionType::Convert,
            ..
        })
    ));
    let statement = parser
        .ParseOneStmt("CREATE TABLE t(c INT KEY)", "", "")
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    assert_eq!(
        create.Cols[0].Options[0].Tp,
        parser_ast::ColumnOptionType::PrimaryKey
    );
    for sql in [
        "SELECT id+?, id+? FROM t",
        "CREATE TABLE t1(a NVARCHAR(100))",
        "USE quote",
        "SELECT b''",
        "SELECT B''",
        "CREATE TABLE t(_sms SMALLINT SIGNED, _smu SMALLINT UNSIGNED)",
        "CREATE TABLE t(c1 NATIONAL CHARACTER(10))",
        "INSERT INTO tb(v) (SELECT v FROM tb)",
        "SELECT a AS c HAVING c=a",
    ] {
        parser
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    let statement = parser
        .ParseOneStmt("SELECT 99e+r10 FROM t1", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::Binary { Op, L, R }
            if Op == "+"
                && matches!(&L.Kind, parser_ast::ExprKind::Column(name) if name.Name.O == "99e")
                && matches!(&R.Kind, parser_ast::ExprKind::Column(name) if name.Name.O == "r10")
    ));
    parser.SetSQLMode(parser::mysql::ModeANSIQuotes);
    let statement = parser
        .ParseOneStmt(r#"SELECT t."dot"=10 FROM t"#, "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::Binary { Op, L, .. }
            if Op == "=" && matches!(&L.Kind, parser_ast::ExprKind::Column(name) if name.Table.O == "t" && name.Name.O == "dot")
    ));
}

#[test]
/// 特殊注释 /*! */ 解析。
pub fn test_special_comments() {
    let mut parser = parser::New();
    assert!(parser.ParseOneStmt(r#"SELECT /*! '\' */"#, "", "").is_err());
    parser.SetSQLMode(parser::mysql::ModeNoBackslashEscapes);
    let statement = parser.ParseOneStmt(r#"SELECT /*! '\' */"#, "", "").unwrap();
    assert!(statement.as_any().is::<parser_ast::SelectStmt>());
    let (statements, _) = parser.Parse("/*! SET x = 1; SELECT 2 */", "", "").unwrap();
    assert_eq!(statements.len(), 2);
    assert!(statements[0].as_any().is::<parser_ast::SetStmt>());
    assert!(statements[1].as_any().is::<parser_ast::SelectStmt>());
    let statement = parser
        .ParseOneStmt("SELECT /*+ 😅 */ SLEEP(1)", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(select.TableHints.is_empty());
}

#[test]
/// SET 变量语句。
pub fn test_set_variable() {
    let cases = [
        ("set xx.xx = 666", "xx.xx", false, false, true),
        ("set session xx.xx = 666", "xx.xx", false, false, true),
        ("set local xx.xx = 666", "xx.xx", false, false, true),
        ("set global xx.xx = 666", "xx.xx", true, false, true),
        ("set instance xx.xx = 666", "xx.xx", false, true, true),
        ("set @@xx.xx = 666", "xx.xx", false, false, true),
        ("set @@session.xx.xx = 666", "xx.xx", false, false, true),
        ("set @@local.xx.xx = 666", "xx.xx", false, false, true),
        ("set @@global.xx.xx = 666", "xx.xx", true, false, true),
        ("set @@instance.xx.xx = 666", "xx.xx", false, true, true),
        ("set @xx.xx = 666", "xx.xx", false, false, false),
    ];
    let mut parser = parser::New();
    for (sql, name, global, instance, system) in cases {
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        let set = statement
            .as_any()
            .downcast_ref::<parser_ast::SetStmt>()
            .unwrap();
        assert_eq!(set.Variables.len(), 1, "{sql}");
        let variable = &set.Variables[0];
        assert_eq!(variable.Name, name, "{sql}");
        assert_eq!(variable.IsGlobal, global, "{sql}");
        assert_eq!(variable.IsInstance, instance, "{sql}");
        assert_eq!(variable.IsSystem, system, "{sql}");
    }
    assert!(parser.ParseOneStmt("set xx.xx.xx = 666", "", "").is_err());
}

#[test]
/// FLUSH TABLE 语法。
pub fn test_flush_table() {
    let statement = parser::New()
        .ParseOneStmt("FLUSH LOCAL TABLES tbl1,tbl2 WITH READ LOCK", "", "")
        .unwrap();
    let flush = statement
        .as_any()
        .downcast_ref::<parser_ast::FlushStmt>()
        .unwrap();
    assert_eq!(flush.Tp, parser_ast::FlushStmtType::Tables);
    assert_eq!(
        flush
            .Tables
            .iter()
            .map(|table| table.Name.L.as_str())
            .collect::<Vec<_>>(),
        ["tbl1", "tbl2"]
    );
    assert!(flush.NoWriteToBinLog);
    assert!(flush.ReadLock);
}

#[test]
/// FLUSH PRIVILEGES。
pub fn test_flush_privileges() {
    let statement = parser::New()
        .ParseOneStmt("FLUSH PRIVILEGES", "", "")
        .unwrap();
    let flush = statement
        .as_any()
        .downcast_ref::<parser_ast::FlushStmt>()
        .unwrap();
    assert_eq!(flush.Tp, parser_ast::FlushStmtType::Privileges);
}

#[test]
/// 内置函数名作标识符。
pub fn test_builtin_func_as_identifier() {
    let whitespace_functions = [
        ("BIT_AND", "`c1`"),
        ("BIT_OR", "`c1`"),
        ("BIT_XOR", "`c1`"),
        ("CAST", "1 AS FLOAT"),
        ("COUNT", "1"),
        ("CURDATE", ""),
        ("CURTIME", ""),
        (
            "DATE_ADD",
            "_UTF8MB4'2011-11-11 10:10:10', INTERVAL 10 SECOND",
        ),
        (
            "DATE_SUB",
            "_UTF8MB4'2011-11-11 10:10:10', INTERVAL 10 SECOND",
        ),
        ("EXTRACT", "SECOND FROM _UTF8MB4'2011-11-11 10:10:10'"),
        ("GROUP_CONCAT", "`c2`, `c1` SEPARATOR ','"),
        ("MAX", "`c1`"),
        ("MID", "_UTF8MB4'Sakila', -5, 3"),
        ("MIN", "`c1`"),
        ("NOW", ""),
        ("POSITION", "_UTF8MB4'bar' IN _UTF8MB4'foobarbar'"),
        ("STDDEV_POP", "`c1`"),
        ("STDDEV_SAMP", "`c1`"),
        ("SUBSTR", "_UTF8MB4'Quadratically', 5"),
        ("SUBSTRING", "_UTF8MB4'Quadratically', 5"),
        ("SUM", "`c1`"),
        ("SYSDATE", ""),
        ("TRIM", "_UTF8MB4' foo '"),
        ("VAR_POP", "`c1`"),
        ("VAR_SAMP", "`c1`"),
    ];
    for ignore_space in [false, true] {
        let mut parser = parser::New();
        if ignore_space {
            parser.SetSQLMode(parser::mysql::ModeIgnoreSpace);
        }
        for (name, args) in whitespace_functions {
            let compact_call = format!("SELECT {name}({args})");
            assert!(
                parser.Parse(&compact_call, "", "").is_ok(),
                "{compact_call}"
            );
            if ignore_space {
                let spaced_call = format!("SELECT {name} ({args})");
                assert!(parser.Parse(&spaced_call, "", "").is_ok(), "{spaced_call}");
            }
            let spaced_table = format!("CREATE TABLE {name} (a INT)");
            assert_eq!(
                parser.Parse(&spaced_table, "", "").is_ok(),
                !ignore_space,
                "{spaced_table}"
            );
            let compact_table = format!("CREATE TABLE {name}(a INT)");
            assert!(
                parser.Parse(&compact_table, "", "").is_err(),
                "{compact_table}"
            );
        }
    }
    for ignore_space in [false, true] {
        let mut parser = parser::New();
        if ignore_space {
            parser.SetSQLMode(parser::mysql::ModeIgnoreSpace);
        }
        for (name, args) in [
            (
                "ADDDATE",
                "_UTF8MB4'2011-11-11 10:10:10', INTERVAL 10 SECOND",
            ),
            ("SESSION_USER", ""),
            (
                "SUBDATE",
                "_UTF8MB4'2011-11-11 10:10:10', INTERVAL 10 SECOND",
            ),
            ("SYSTEM_USER", ""),
        ] {
            for sql in [
                format!("SELECT {name}({args})"),
                format!("SELECT {name} ({args})"),
                format!("CREATE TABLE {name} (a INT)"),
                format!("CREATE TABLE {name}(a INT)"),
            ] {
                parser
                    .Parse(&sql, "", "")
                    .unwrap_or_else(|error| panic!("{sql}: {error}"));
            }
        }
    }
}

#[test]
/// 优化器 Hint 错误信息。
pub fn test_hint_error() {
    let mut parser = parser::New();
    let (statements, warnings) = parser
        .Parse("SELECT /*+ TIDB_UNKNOWN(t1,t2) */ c1 FROM t1", "", "")
        .unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0]
            .to_string()
            .to_lowercase()
            .contains("tidb_unknown")
    );
    let select = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(select.TableHints.is_empty());
    let (statements, warnings) = parser
        .Parse(
            "SELECT /*+ TIDB_INLJ(t1,t2) TIDB_UNKNOWN(t1,t2,1) */ c1 FROM t1,t2",
            "",
            "",
        )
        .unwrap();
    assert_eq!(warnings.len(), 1);
    let select = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.TableHints.len(), 1);
    assert!(
        parser
            .Parse("SELECT /*+ TIDB_INLJ(t1,t2) */ c1 FROMT t1", "", "")
            .is_err()
    );
    let (statements, _) = parser
        .Parse(
            "INSERT INTO t SELECT /*+ MEMORY_QUOTA(1 MB) */ * FROM t",
            "",
            "",
        )
        .unwrap();
    let insert = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert!(insert.TableHints.is_empty());
    let select = insert
        .Select
        .as_ref()
        .unwrap()
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.TableHints.len(), 1);
}

#[test]
/// 通用解析错误文案。
pub fn test_error_msg() {
    let cases = [
        ("select1 1", "line 1 column 7"),
        ("select 1 from1 dual", "line 1 column 19"),
        (
            "select * from t1 join t2 from t1.a = t2.a",
            "line 1 column 29",
        ),
        (
            "create table t(f_year year(5))ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
            "[parser:1818]Supports only YEAR or YEAR(4) column",
        ),
        ("create table ``.t (id int)", "Incorrect database name ''"),
        (
            "select 1 collate some_unknown_collation",
            "Unknown collation: 'some_unknown_collation'",
        ),
    ];
    for (sql, expected) in cases {
        let error = match parser::New().Parse(sql, "", "") {
            Ok(_) => panic!("{sql}: expected error"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(expected), "{sql}: {error}");
    }
}

#[test]
/// GROUP_CONCAT SEPARATOR 字符集/排序规则。
pub fn test_group_concat_separator_charset_collation() {
    for (sql, charset, collate, separator) in [
        ("SELECT GROUP_CONCAT('x')", "latin1", "latin1_bin", ","),
        (
            "SELECT GROUP_CONCAT('x' SEPARATOR ';')",
            "latin1",
            "latin1_bin",
            ";",
        ),
        (
            "SELECT GROUP_CONCAT('x')",
            parser::mysql::DefaultCharset,
            parser::mysql::DefaultCollationName,
            ",",
        ),
    ] {
        let statement = parser::New().ParseOneStmt(sql, charset, collate).unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let parser_ast::ExprKind::AggregateFunction { Name, Args, .. } =
            &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
        else {
            panic!("{sql}: expected aggregate")
        };
        assert!(Name.eq_ignore_ascii_case("group_concat"));
        assert!(Args.len() >= 2);
        assert!(
            matches!(&Args.last().unwrap().Kind,
            parser_ast::ExprKind::IntroducedValue { Value, Charset, Collation, .. }
                if Value == separator && Charset == charset && Collation == collate),
            "{sql}: separator AST {:?}",
            Args.last().unwrap().Kind
        );
    }
}

#[test]
/// 优化器 Hint 解析。
pub fn test_optimizer_hints() {
    let statement = parser::New()
        .ParseOneStmt(
            "SELECT /*+ USE_INDEX(t1,t2), USE_INDEX(t3,t4) */ c1,c2 FROM t1,t2 WHERE t1.c1=t2.c1",
            "",
            "",
        )
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.TableHints.len(), 2, "hints: {:?}", select.TableHints);
    assert_eq!(select.TableHints[0].HintName.L, "use_index");
    assert_eq!(select.TableHints[0].Tables.len(), 1);
    assert_eq!(select.TableHints[0].Tables[0].TableName.L, "t1");
    assert_eq!(select.TableHints[0].Indexes[0].L, "t2");
    assert_eq!(select.TableHints[1].Tables[0].TableName.L, "t3");
    assert_eq!(select.TableHints[1].Indexes.len(), 1);
    assert_eq!(select.TableHints[1].Indexes[0].L, "t4");
    for (sql, hint) in [
        (
            "SELECT /*+ MAX_EXECUTION_TIME(1000) */ 1",
            "max_execution_time",
        ),
        ("SELECT /*+ MEMORY_QUOTA(10 MB) */ 1", "memory_quota"),
        (
            "SELECT /*+ READ_FROM_STORAGE(TIFLASH[t]) */ * FROM t",
            "read_from_storage",
        ),
    ] {
        let statement = parser::New().ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(select.TableHints.len(), 1, "{sql}");
        assert_eq!(select.TableHints[0].HintName.L, hint, "{sql}");
    }
}

#[test]
/// MariaDB 权限语法（开启）。
pub fn test_privilege_maria_db_enabled() {
    run_contract_table("TestPrivilegeMariaDBEnabled", true);
}

#[test]
/// MariaDB 权限语法（关闭）。
pub fn test_privilege_maria_db_disabled() {
    run_contract_table("TestPrivilegeMariaDBDisabled", false);
}

#[test]
/// 系统版本列（MariaDB 开启）。
pub fn test_system_versioned_column_maria_db_enabled() {
    run_contract_table("TestSystemVersionedColumnMariaDBEnabled", true);
}

#[test]
/// 系统版本列（MariaDB 关闭）。
pub fn test_system_versioned_column_maria_db_disabled() {
    run_contract_table("TestSystemVersionedColumnMariaDBDisabled", false);
}

#[test]
/// 子查询解析。
pub fn test_subquery() {
    run_contract_table("TestSubquery", false);
    for (sql, set_operation) in [
        ("SELECT 1 > (select 1)", false),
        ("SELECT 1 > (select 1 union select 2)", true),
    ] {
        let statement = parser::New().ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expression = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let parser_ast::ExprKind::Binary { R, .. } = &expression.Kind else {
            panic!(
                "{sql}: expected binary comparison, got {:?}",
                expression.Kind
            )
        };
        let parser_ast::ExprKind::Subquery { Query, .. } = &R.Kind else {
            panic!("{sql}: expected subquery, got {:?}", R.Kind)
        };
        Query
            .with_node(|query| {
                assert_eq!(
                    query.as_any().is::<parser_ast::SetOprStmt>(),
                    set_operation,
                    "{sql}"
                );
                assert_eq!(
                    query.as_any().is::<parser_ast::SelectStmt>(),
                    !set_operation,
                    "{sql}"
                );
            })
            .expect("subquery node");
    }
}

/// 收集 Select 是否带 ORDER BY 的布尔序列。
fn collect_select_order_by(node: &dyn parser_ast::Node, output: &mut Vec<bool>) {
    if let Some(select) = node.as_any().downcast_ref::<parser_ast::SelectStmt>() {
        output.push(!select.OrderBy.is_empty());
    } else if let Some(set) = node.as_any().downcast_ref::<parser_ast::SetOprStmt>() {
        for select in &set.select_list.selects {
            collect_select_order_by(select.as_ref(), output);
        }
    } else if let Some(list) = node.as_any().downcast_ref::<parser_ast::SetOprSelectList>() {
        for select in &list.selects {
            collect_select_order_by(select.as_ref(), output);
        }
    }
}

#[test]
/// UNION 与 ORDER BY 归属。
pub fn test_union_order_by() {
    let cases: [(&str, &[bool]); 5] = [
        (
            "select 2 as a from dual union select 1 as b from dual order by a",
            &[false, false, true],
        ),
        (
            "select 2 as a from dual union (select 1 as b from dual order by a)",
            &[false, true, false],
        ),
        (
            "(select 2 as a from dual order by a) union select 1 as b from dual order by a",
            &[true, false, true],
        ),
        ("select 1 a, 2 b from dual order by a", &[true]),
        ("select 1 a, 2 b from dual", &[false]),
    ];
    let mut parser = parser::New();
    parser.EnableWindowFunc(false);
    for (sql, expected) in cases {
        let (statements, _) = parser.Parse(sql, "", "").unwrap();
        let statement = statements[0].as_ref();
        let mut actual = Vec::new();
        collect_select_order_by(statement, &mut actual);
        if let Some(set) = statement.as_any().downcast_ref::<parser_ast::SetOprStmt>() {
            actual.push(!set.OrderBy.is_empty());
        }
        assert_eq!(actual, expected, "{sql}");
    }
}

#[test]
/// 语句优先级修饰。
pub fn test_priority() {
    run_contract_table("TestPriority", false);
    let statement = parser::New()
        .ParseOneStmt("select HIGH_PRIORITY * from t", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.SelectStmtOpts.Priority, 2);
}

#[test]
/// SQL_NO_CACHE 选项。
pub fn test_sql_no_cache() {
    for (sql, expected) in [
        ("select SQL_NO_CACHE * from t", false),
        ("select SQL_CACHE * from t", true),
        ("select * from t", true),
    ] {
        let statement = parser::New().ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(select.SelectStmtOpts.SQLCache, expected, "{sql}");
    }
}

#[test]
/// 执行计划绑定（BINDING）。
pub fn test_binding() {
    run_contract_table("TestBinding", false);
    let statement = parser::New()
        .ParseOneStmt(
            "create global binding for select * from t using select * from t use index(a)",
            "",
            "",
        )
        .unwrap();
    let binding = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateBindingStmt>()
        .unwrap();
    assert!(binding.GlobalScope);
    assert!(
        binding
            .OriginNode
            .as_ref()
            .is_some_and(|node| node.as_any().is::<parser_ast::SelectStmt>())
    );
    let hinted = binding
        .HintedNode
        .as_ref()
        .and_then(|node| node.as_any().downcast_ref::<parser_ast::SelectStmt>())
        .expect("hinted select");
    let table = match hinted
        .From
        .as_ref()
        .unwrap()
        .TableRefs
        .Left
        .as_deref()
        .unwrap()
    {
        parser_ast::ResultSetNode::TableSource(table) => &table.Source,
        parser_ast::ResultSetNode::Join(_) => panic!("expected table source"),
    };
    assert_eq!(table.Name.L, "t");
    assert_eq!(table.IndexHints.len(), 1);
    assert_eq!(table.IndexHints[0].IndexNames[0].L, "a");
}

#[test]
/// VIEW 相关语句。
pub fn test_view() {
    run_contract_table("TestView", false);
    let mut parser = parser::New();
    let statement = parser
        .ParseOneStmt("create view v as select * from t", "", "")
        .unwrap();
    let view = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateViewStmt>()
        .unwrap();
    assert_eq!(view.Algorithm, parser_ast::ViewAlgorithm::Undefined);
    assert!(view.Select.as_any().is::<parser_ast::SelectStmt>());
    assert_eq!(view.Security, parser_ast::ViewSecurity::Definer);
    assert_eq!(view.CheckOption, parser_ast::ViewCheckOption::Cascaded);

    let statement = parser
        .ParseOneStmt(
            "CREATE OR REPLACE ALGORITHM = UNDEFINED DEFINER = root@localhost\n\
             SQL SECURITY DEFINER VIEW V(a,b,c) AS select c,d,e from t\n\
             WITH CASCADED CHECK OPTION",
            "",
            "",
        )
        .unwrap();
    let view = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateViewStmt>()
        .unwrap();
    assert!(view.OrReplace);
    assert_eq!(view.Algorithm, parser_ast::ViewAlgorithm::Undefined);
    assert_eq!(view.Definer.username, "root");
    assert_eq!(view.Definer.hostname, "localhost");
    assert_eq!(
        view.Cols
            .iter()
            .map(|column| column.L.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert!(view.Select.as_any().is::<parser_ast::SelectStmt>());
    assert_eq!(view.Security, parser_ast::ViewSecurity::Definer);
    assert_eq!(view.CheckOption, parser_ast::ViewCheckOption::Cascaded);

    let (statements, _) = parser
        .Parse(
            "CREATE VIEW v1 AS SELECT * FROM t;\nCREATE VIEW v2 AS SELECT 123123123123123;",
            "",
            "",
        )
        .unwrap();
    assert_eq!(statements.len(), 2);
    assert!(statements.iter().all(|statement| {
        statement
            .as_any()
            .downcast_ref::<parser_ast::CreateViewStmt>()
            .is_some_and(|view| view.Select.as_any().is::<parser_ast::SelectStmt>())
    }));
}

#[test]
/// A bare WITH CHECK OPTION is accepted as CASCADED and excluded from select text.
pub fn view_check_option() {
    for sql in [
        "create view v as select * from t with check option",
        "create view v as (select * from t) with check option",
        "create view v as select * from t union select * from t with check option",
        "create view v as select * from t union all select * from t with check option",
    ] {
        let statement = parser::New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        let view = statement
            .as_any()
            .downcast_ref::<parser_ast::CreateViewStmt>()
            .expect("CREATE VIEW AST");
        assert_eq!(
            view.CheckOption,
            parser_ast::ViewCheckOption::Cascaded,
            "{sql}"
        );
        assert!(
            !view
                .Select
                .Text()
                .to_ascii_lowercase()
                .contains("check option"),
            "select text retained the view clause for {sql}: {:?}",
            view.Select.Text()
        );
    }

    for sql in [
        "create view v as select * from t with check",
        "create view v as select * from t with cascaded local check option",
    ] {
        assert!(
            parser::New().ParseOneStmt(sql, "", "").is_err(),
            "invalid view check option was accepted: {sql}"
        );
    }
}

#[test]
/// TIMESTAMPDIFF 时间单位。
pub fn test_timestamp_diff_unit() {
    let (statements, _) = parser::New()
        .Parse(
            "SELECT TIMESTAMPDIFF(MONTH,'2003-02-01','2003-05-01'), \
             TIMESTAMPDIFF(month,'2003-02-01','2003-05-01')",
            "",
            "",
        )
        .unwrap();
    let select = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.Fields.Fields.len(), 2);
    for field in &select.Fields.Fields {
        let parser_ast::ExprKind::Function { Args, .. } = &field.Expr.as_ref().unwrap().Kind else {
            panic!("expected TIMESTAMPDIFF function")
        };
        assert!(matches!(
            Args.first().map(|argument| &argument.Kind),
            Some(parser_ast::ExprKind::TimeUnit(
                parser_ast::TimeUnitType::Month
            ))
        ));
    }
    run_contract_table("TestTimestampDiffUnit", false);
}

#[test]
/// 函数调用表达式源码偏移。
pub fn test_func_call_expr_offset() {
    let (statements, _) = parser::New().Parse("SELECT s.a(), b();", "", "").unwrap();
    let select = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.Fields.Fields.len(), 2);
    for (field, expected) in select.Fields.Fields.iter().zip([7, 14]) {
        let expression = field.Expr.as_ref().unwrap();
        assert!(matches!(
            expression.Kind,
            parser_ast::ExprKind::Function { .. }
        ));
        assert_eq!(expression.OriginTextPosition, expected);
    }
}

#[test]
/// ANSI_QUOTES SQL Mode。
pub fn test_sql_mode_ansi_quotes() {
    let mut parser = parser::New();
    parser.SetSQLMode(parser::mysql::ModeANSIQuotes);
    for sql in [
        r#"CREATE TABLE "table" ("id" int)"#,
        r#"select * from t "tt""#,
    ] {
        parser
            .Parse(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
}

/// 判断类型 flag 是否含 Binary 位。
fn has_binary_flag(flag: usize) -> bool {
    flag & 128 != 0
}

#[test]
/// DDL 语句集。
pub fn test_ddl_statements() {
    let mut parser = parser::New();
    let (statements, _) = parser
        .Parse(
            "CREATE TABLE t (\n\
             a varchar(64) binary,\n\
             b char(10) charset utf8 collate utf8_general_ci,\n\
             c text charset latin1) ENGINE=innoDB DEFAULT CHARSET=utf8 COLLATE=utf8_bin",
            "",
            "",
        )
        .unwrap();
    let create = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    assert!(has_binary_flag(create.Cols[0].Tp.GetFlag()));
    assert!(
        create.Cols[1..]
            .iter()
            .all(|column| !has_binary_flag(column.Tp.GetFlag()))
    );
    for option in &create.Options {
        match option.Tp {
            parser_ast::TableOptionType::Charset => assert_eq!(option.StrValue, "utf8"),
            parser_ast::TableOptionType::Collate => assert_eq!(option.StrValue, "utf8_bin"),
            _ => {}
        }
    }

    let (statements, _) = parser
        .Parse(
            "CREATE TABLE t (a varbinary(64), b binary(10), c blob)",
            "",
            "",
        )
        .unwrap();
    let create = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    for column in &create.Cols {
        assert_eq!(column.Tp.GetCharset(), parser::charset::CharsetBin);
        assert_eq!(column.Tp.GetCollate(), "binary");
        assert!(has_binary_flag(column.Tp.GetFlag()));
    }

    parser
        .Parse(
            "CREATE TABLE t (\n\
             c_int int collate utf8_bin, c_real real collate utf8_bin,\n\
             c_float float collate utf8_bin, c_bool bool collate utf8_bin,\n\
             c_char char collate utf8_bin, c_binary binary collate utf8_bin,\n\
             c_varchar varchar(2) collate utf8_bin, c_year year collate utf8_bin,\n\
             c_date date collate utf8_bin, c_time time collate utf8_bin,\n\
             c_datetime datetime collate utf8_bin, c_timestamp timestamp collate utf8_bin,\n\
             c_tinyblob tinyblob collate utf8_bin, c_blob blob collate utf8_bin,\n\
             c_mediumblob mediumblob collate utf8_bin, c_longblob longblob collate utf8_bin,\n\
             c_bit bit collate utf8_bin, c_long_varchar long varchar collate utf8_bin,\n\
             c_tinytext tinytext collate utf8_bin, c_text text collate utf8_bin,\n\
             c_mediumtext mediumtext collate utf8_bin, c_longtext longtext collate utf8_bin,\n\
             c_decimal decimal collate utf8_bin, c_numeric numeric collate utf8_bin,\n\
             c_enum enum('1') collate utf8_bin, c_set set('1') collate utf8_bin,\n\
             c_json json collate utf8_bin)",
            "",
            "",
        )
        .unwrap();

    let error = match parser.Parse("CREATE TABLE t (c_double double(10))", "", "") {
        Err(error) => error,
        Ok(_) => panic!("DOUBLE(10) must fail in strict mode"),
    };
    assert_eq!(
        error.to_string(),
        "[parser:1149]You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use"
    );
    parser.SetStrictDoubleTypeCheck(false);
    parser
        .Parse("CREATE TABLE t (c_double double(10))", "", "")
        .unwrap();
    parser.SetStrictDoubleTypeCheck(true);
    parser
        .Parse("CREATE TABLE t (c_double double(10, 2))", "", "")
        .unwrap();

    let error = match parser.Parse(
        "create global temporary table t010(local_01 int, local_03 varchar(20))",
        "",
        "",
    ) {
        Err(error) => error,
        Ok(_) => panic!("GLOBAL TEMPORARY without ON COMMIT must fail"),
    };
    assert_eq!(
        error.to_string(),
        "line 1 column 70 near \"\"GLOBAL TEMPORARY and ON COMMIT DELETE ROWS must appear together "
    );
    parser
        .Parse(
            "create global temporary table t010(local_01 int, local_03 varchar(20)) on commit preserve rows",
            "",
            "",
        )
        .unwrap();
}

contract_table_tests! {
    test_analyze => "TestAnalyze";
    test_start_transaction => "TestStartTransaction";
    test_brie => "TestBRIE";
    test_cte => "TestCTE";
    test_cte_merge => "TestCTEMerge";
    test_as_of_clause => "TestAsOfClause";
}

#[test]
/// TABLESAMPLE。
pub fn test_table_sample() {
    run_contract_table("TestTableSample", false);
    let mut parser = parser::New();
    for sql in parser_contract_strings("TestTableSample", "cases") {
        parser
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
}

#[test]
/// 生成列。
pub fn test_generated_column() {
    let mut parser = parser::New();
    for (sql, stored, left_is_column) in [
        (
            "create table t (c int, d int generated always as (c + 1) virtual)",
            false,
            true,
        ),
        (
            "create table t (c int, d int as (   c + 1   ) virtual)",
            false,
            true,
        ),
        (
            "create table t (c int, d int as (1 + 1) stored)",
            true,
            false,
        ),
    ] {
        let (statements, _) = parser.Parse(sql, "", "").unwrap();
        let create = statements[0]
            .as_any()
            .downcast_ref::<parser_ast::CreateTableStmt>()
            .unwrap();
        let generated = create
            .Cols
            .iter()
            .flat_map(|column| &column.Options)
            .find(|option| option.Tp == parser_ast::ColumnOptionType::Generated)
            .unwrap_or_else(|| panic!("{sql}: missing generated column option"));
        assert_eq!(generated.Stored, stored, "{sql}");
        let parser_ast::ExprKind::Binary { Op, L, R } = &generated.Expr.as_ref().unwrap().Kind
        else {
            panic!("{sql}: expected generated binary expression")
        };
        assert_eq!(Op, "+", "{sql}");
        assert_eq!(
            matches!(&L.Kind, parser_ast::ExprKind::Column(name) if name.Name.O == "c"),
            left_is_column,
            "{sql}"
        );
        if !left_is_column {
            assert!(matches!(&L.Kind, parser_ast::ExprKind::Value(value) if value == "1"));
        }
        assert!(matches!(&R.Kind, parser_ast::ExprKind::Value(value) if value == "1"));
    }
    for (sql, expected) in [
        (
            "create table t1 (a int, b int as (a + 1) default 10)",
            "[ddl:1221]Incorrect usage of DEFAULT and generated column",
        ),
        (
            "create table t1 (a int, b int as (a + 1) on update now())",
            "[ddl:1221]Incorrect usage of ON UPDATE and generated column",
        ),
        (
            "create table t1 (a int, b int as (a + 1) auto_increment)",
            "[ddl:1221]Incorrect usage of AUTO_INCREMENT and generated column",
        ),
    ] {
        let error = match parser.Parse(sql, "", "") {
            Err(error) => error,
            Ok(_) => panic!("{sql}: expected generated-column option error"),
        };
        assert_eq!(error.to_string(), expected, "{sql}");
    }
}

#[test]
/// SET TRANSACTION。
pub fn test_set_transaction() {
    for (sql, global, value) in [
        (
            "SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED",
            false,
            "READ-COMMITTED",
        ),
        (
            "SET GLOBAL TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            true,
            "REPEATABLE-READ",
        ),
    ] {
        let statement = parser::New().ParseOneStmt(sql, "", "").unwrap();
        let set = statement
            .as_any()
            .downcast_ref::<parser_ast::SetStmt>()
            .unwrap();
        assert_eq!(set.Variables.len(), 1, "{sql}");
        let variable = &set.Variables[0];
        assert_eq!(variable.Name, "tx_isolation", "{sql}");
        assert_eq!(variable.IsGlobal, global, "{sql}");
        assert!(variable.IsSystem, "{sql}");
        assert!(
            matches!(&variable.Value.Kind, parser_ast::ExprKind::Value(actual) if actual == value)
        );
    }
}

#[test]
/// 解析副作用/状态。
pub fn test_side_effect() {
    let mut parser = parser::New();
    assert!(
        parser
            .ParseOneStmt("create table t /*!50100 'abc', 'abc' */;", "", "")
            .is_err()
    );
    let statement = parser.ParseOneStmt("show tables;", "", "").unwrap();
    assert!(statement.as_any().is::<parser_ast::ShowStmt>());
}

#[test]
/// 表分区定义。
pub fn test_table_partition() {
    run_contract_table("TestTablePartition", false);
    let statement = parser::New()
        .ParseOneStmt(
            "create table t (id int) partition by range (id) (partition p0 values less than (10) comment 'check')",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    let definition = &create.Partition.as_ref().unwrap().Definitions[0];
    let comment = definition
        .Options
        .iter()
        .find(|option| option.Tp == parser_ast::TableOptionType::Comment)
        .unwrap();
    assert_eq!(comment.StrValue, "check");
}

#[test]
/// 分区名列表。
pub fn test_table_partition_name_list() {
    let statement = parser::New()
        .ParseOneStmt("select * from t partition (p0,p1)", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let left = select
        .From
        .as_ref()
        .unwrap()
        .TableRefs
        .Left
        .as_ref()
        .unwrap();
    let parser_ast::ResultSetNode::TableSource(source) = left.as_ref() else {
        panic!("expected table source")
    };
    assert_eq!(
        source
            .Source
            .PartitionNames
            .iter()
            .map(|name| name.L.as_str())
            .collect::<Vec<_>>(),
        ["p0", "p1"]
    );
}

#[test]
/// NOT EXISTS 子查询。
pub fn test_not_exists_subquery() {
    let statement = parser::New()
        .ParseOneStmt(
            "select * from t1 where not exists (select * from t2 where t1.a = t2.a)",
            "",
            "",
        )
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Where.as_ref().map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::ExistsSubquery { Not: true, .. })
    ));
}

/// 窗口函数关键字列表，用于标识符冲突用例。
const WINDOW_FUNCTION_KEYWORDS: [&str; 14] = [
    "CUME_DIST",
    "DENSE_RANK",
    "FIRST_VALUE",
    "GROUPS",
    "LAG",
    "LAST_VALUE",
    "LEAD",
    "NTH_VALUE",
    "NTILE",
    "OVER",
    "PERCENT_RANK",
    "RANK",
    "ROW_NUMBER",
    "WINDOW",
];

#[test]
/// 窗口函数关键字作标识符。
pub fn test_window_function_identifier() {
    for enable_window in [true, false] {
        let mut parser = parser::New();
        parser.EnableWindowFunc(enable_window);
        for keyword in WINDOW_FUNCTION_KEYWORDS {
            let sql = format!("select 1 {keyword}");
            assert_eq!(
                parser.Parse(&sql, "", "").is_ok(),
                !enable_window,
                "{sql}, enable_window={enable_window}"
            );
        }
    }
}

#[test]
/// 窗口函数语法。
pub fn test_window_functions() {
    run_contract_table_with_options("TestWindowFunctions", true, false);
}

#[test]
/// 窗口帧边界访问。
pub fn test_visit_frame_bound() {
    for (sql, has_expression, unit) in [
        (
            "SELECT AVG(val) OVER (RANGE INTERVAL 1+3 MINUTE_SECOND PRECEDING) FROM t",
            true,
            parser_ast::TimeUnitType::MinuteSecond,
        ),
        (
            "SELECT AVG(val) OVER (RANGE 5 PRECEDING) FROM t",
            true,
            parser_ast::TimeUnitType::Invalid,
        ),
        (
            "SELECT AVG(val) OVER () FROM t",
            false,
            parser_ast::TimeUnitType::Invalid,
        ),
    ] {
        let mut parser = parser::New();
        parser.EnableWindowFunc(true);
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let parser_ast::ExprKind::WindowFunction { Spec, .. } =
            &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
        else {
            panic!("{sql}: expected window function")
        };
        if let Some(frame) = &Spec.Frame {
            assert_eq!(frame.Extent.Start.Expr.is_some(), has_expression, "{sql}");
            assert_eq!(frame.Extent.Start.Unit, unit, "{sql}");
        } else {
            assert!(!has_expression, "{sql}");
            assert_eq!(unit, parser_ast::TimeUnitType::Invalid, "{sql}");
        }
    }
}

#[test]
/// 字段原始文本保留。
pub fn test_field_text() {
    let mut parser = parser::New();
    let statement = parser.ParseOneStmt("select a from t", "", "").unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::Column(name)) if name.Name.O == "a"
    ));
    for (sql, expected_format) in [
        ("trace select a from t", "row"),
        ("trace format = 'row' select a from t", "row"),
        ("trace format = 'json' select a from t", "json"),
    ] {
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        let trace = statement
            .as_any()
            .downcast_ref::<parser_ast::TraceStmt>()
            .unwrap();
        assert_eq!(trace.Format, expected_format, "{sql}");
        assert!(trace.Stmt.as_any().is::<parser_ast::SelectStmt>(), "{sql}");
    }
}

#[test]
/// 带引号系统变量。
pub fn test_quoted_system_variables() {
    let statement = parser::New()
        .ParseOneStmt(
            "select @@Sql_Mode, @@`SQL_MODE`, @@session.`sql_mode`, @@global.`s ql``mode`, @@session.'sql\\nmode', @@local.\"sql\\\"mode\", @@instance.sql_mode",
            "",
            "",
        )
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let expected = [
        ("sql_mode", false, false, false),
        ("sql_mode", false, false, false),
        ("sql_mode", false, false, true),
        ("s ql`mode", true, false, true),
        ("sql\nmode", false, false, true),
        ("sql\"mode", false, false, true),
        ("sql_mode", false, true, true),
    ];
    assert_eq!(select.Fields.Fields.len(), expected.len());
    for (field, (name, global, instance, explicit)) in select.Fields.Fields.iter().zip(expected) {
        let parser_ast::ExprKind::Variable {
            Name,
            IsGlobal,
            IsInstance,
            IsSystem,
            ExplicitScope,
            ..
        } = &field.Expr.as_ref().unwrap().Kind
        else {
            panic!("expected system variable")
        };
        assert_eq!(Name, name);
        assert_eq!(*IsGlobal, global);
        assert_eq!(*IsInstance, instance);
        assert!(*IsSystem);
        assert_eq!(*ExplicitScope, explicit);
    }
}

#[test]
/// 带引号变量/列名。
pub fn test_quoted_variable_column_name() {
    let statement = parser::New()
        .ParseOneStmt(
            "select @abc, @`abc`, @'aBc', @\"AbC\", @6, @`6`, @'6', @\"6\", @@sql_mode, @@`sql_mode`, @",
            "",
            "",
        )
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let expected = [
        ("abc", false),
        ("abc", false),
        ("aBc", false),
        ("AbC", false),
        ("6", false),
        ("6", false),
        ("6", false),
        ("6", false),
        ("sql_mode", true),
        ("sql_mode", true),
        ("", false),
    ];
    assert_eq!(select.Fields.Fields.len(), expected.len());
    for (field, (expected_name, expected_system)) in select.Fields.Fields.iter().zip(expected) {
        let parser_ast::ExprKind::Variable { Name, IsSystem, .. } =
            &field.Expr.as_ref().unwrap().Kind
        else {
            panic!("expected variable expression")
        };
        assert_eq!(Name, expected_name);
        assert_eq!(*IsSystem, expected_system);
    }
}

#[test]
/// 字符集子句。
pub fn test_charset() {
    let mut parser = parser::New();
    for sql in [
        "ALTER SCHEMA GLOBAL DEFAULT CHAR SET utf8mb4",
        "ALTER DATABASE CHAR SET = utf8mb4",
        "ALTER DATABASE DEFAULT CHAR SET = utf8mb4",
    ] {
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        assert!(
            statement.as_any().is::<parser_ast::AlterDatabaseStmt>(),
            "{sql}"
        );
    }
}

#[test]
/// 下划线字符集 introducer。
pub fn test_underscore_charset() {
    let _charset_guard = CHARSET_TEST_LOCK.lock().unwrap();
    let mut parser = parser::New();
    for (charset, expected_error) in [
        ("utf8", None),
        (
            "gbk",
            Some("[ddl:1115]Unsupported character introducer: 'gbk'"),
        ),
        (
            "ujis",
            Some("[ddl:1115]Unsupported character introducer: 'ujis'"),
        ),
        ("gbk1", Some("line 1 column 21 near \"'3F')\" ")),
        ("ujisx", Some("line 1 column 22 near \"'3F')\" ")),
    ] {
        let sql = format!("select hex(_{charset} '3F')");
        match (parser.ParseOneStmt(&sql, "", ""), expected_error) {
            (Ok(_), None) => {}
            (Err(error), Some(expected)) => assert_eq!(error.to_string(), expected, "{sql}"),
            (Err(error), None) => panic!("{sql}: {error}"),
            (Ok(_), Some(expected)) => panic!("{sql}: expected {expected}"),
        }
    }
}

#[test]
/// 全文检索 MATCH/AGAINST。
pub fn test_fulltext_search() {
    let mut parser = parser::New();
    for (sql, expected_columns, expected_modifier) in [
        (
            "SELECT * FROM fulltext_test WHERE MATCH(content) AGAINST('search')",
            &["content"][..],
            0,
        ),
        (
            "SELECT * FROM fulltext_test WHERE MATCH(title,content) AGAINST('search' IN NATURAL LANGUAGE MODE)",
            &["title", "content"][..],
            0,
        ),
        (
            "SELECT * FROM fulltext_test WHERE MATCH(title,content) AGAINST('search' IN BOOLEAN MODE)",
            &["title", "content"][..],
            1,
        ),
        (
            "SELECT * FROM fulltext_test WHERE MATCH(title,content) AGAINST('search' WITH QUERY EXPANSION)",
            &["title", "content"][..],
            16,
        ),
    ] {
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let parser_ast::ExprKind::MatchAgainst {
            ColumnNames,
            Against,
            Modifier,
        } = &select.Where.as_ref().unwrap().Kind
        else {
            panic!("{sql}: expected MATCH AGAINST expression")
        };
        assert_eq!(
            ColumnNames
                .iter()
                .map(|column| column.Name.L.as_str())
                .collect::<Vec<_>>(),
            expected_columns,
            "{sql}"
        );
        assert!(matches!(&Against.Kind, parser_ast::ExprKind::Value(value) if value == "search"));
        assert_eq!(*Modifier, expected_modifier, "{sql}");
    }
    for sql in [
        "SELECT * FROM fulltext_test WHERE MATCH() AGAINST('search')",
        "SELECT * FROM fulltext_test WHERE MATCH(content) AGAINST()",
        "SELECT * FROM fulltext_test WHERE MATCH(content) AGAINST('search' IN)",
        "SELECT * FROM fulltext_test WHERE MATCH(content) AGAINST('search' IN BOOLEAN MODE WITH QUERY EXPANSION)",
    ] {
        assert!(parser.ParseOneStmt(sql, "", "").is_err(), "{sql}");
    }
}

#[test]
/// 有符号 int64 越界。
pub fn test_signed_int64_out_of_range() {
    let mut parser = parser::New();
    for sql in parser_contract_strings("TestSignedInt64OutOfRange", "cases") {
        let error = match parser.ParseOneStmt(&sql, "", "") {
            Err(error) => error,
            Ok(_) => panic!("{sql}: expected integer range error"),
        };
        assert!(error.to_string().contains("out of range"), "{sql}: {error}");
    }
}

#[test]
/// 统计信息相关语句。
pub fn test_statistics_ops() {
    run_contract_table("TestStatisticsOps", false);
    let (statements, _) = parser::New()
        .Parse(
            "create statistics if not exists stats1 (cardinality) on t(a,b,c)",
            "",
            "",
        )
        .unwrap();
    let statistics = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateStatisticsStmt>()
        .unwrap();
    assert!(statistics.IfNotExists);
    assert_eq!(statistics.StatsName, "stats1");
    assert_eq!(statistics.StatsType, 0);
    assert_eq!(statistics.Table.Name.L, "t");
    assert_eq!(
        statistics
            .Columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
}

#[test]
/// HIGH_NOT_PRECEDENCE 模式。
pub fn test_high_not_precedence_mode() {
    let mut parser = parser::New();
    let statement = parser
        .ParseOneStmt("SELECT NOT 1 BETWEEN -5 AND 5", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::Unary { Op, .. }) if Op == "NOT"
    ));
    let statement = parser
        .ParseOneStmt("SELECT !1 BETWEEN -5 AND 5", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0]
            .Expr
            .as_ref()
            .map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::Between { .. })
    ));
    parser.SetSQLMode(parser::mysql::ModeHighNotPrecedence);
    let statement = parser
        .ParseOneStmt("SELECT NOT 1 BETWEEN -5 AND 5", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0]
            .Expr
            .as_ref()
            .map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::Between { .. })
    ));
}

#[test]
/// 解析器错误消息细节。
pub fn test_parser_err_msg() {
    let cases = [
        (
            "delete from t where a = 7 or 1=1/*' and b = 'p'",
            Some("near '/*' and b = 'p'' at line 1"),
        ),
        (
            "delete from t where a = 7 or\n 1=1/*' and b = 'p'",
            Some("near '/*' and b = 'p'' at line 2"),
        ),
        ("select 1/*", Some("near '/*' at line 1")),
        ("select 1/* comment */", None),
        ("select a.b()", None),
        ("SELECT foo.bar('baz');", None),
    ];
    let mut parser = parser::New();
    for (sql, expected_error) in cases {
        match (parser.Parse(sql, "", ""), expected_error) {
            (Err(error), Some(expected)) => assert_eq!(error.to_string(), expected, "{sql}"),
            (Ok(_), None) => {}
            (Err(error), None) => panic!("{sql}: unexpected error: {error}"),
            (Ok(_), Some(expected)) => panic!("{sql}: expected error {expected:?}"),
        }
    }
}

#[test]
/// 分区 KEY 算法。
pub fn test_partition_key_algorithm() {
    run_contract_table("TestPartitionKeyAlgorithm", false);
}

#[test]
/// HELP 语句。
pub fn test_help() {
    run_contract_table("TestHelp", false);
}

#[test]
/// 无字符集 flag 路径。
pub fn test_without_charset_flags() {
    for (source, restored) in [
        ("select 'a'", "SELECT 'a'"),
        ("select _utf8'a'", "SELECT 'a'"),
        ("select _utf8mb4'a'", "SELECT 'a'"),
        ("select _utf8 X'D0B1'", "SELECT x'd0b1'"),
        ("select _utf8mb4'a'", "SELECT 'a'"),
        ("select _utf8'a'", "SELECT _utf8'a'"),
        ("select _utf8'a'", "SELECT _utf8'a'"),
        ("select _utf8 X'D0B1'", "SELECT _utf8 x'd0b1'"),
    ] {
        let source = parser::New().ParseOneStmt(source, "", "").unwrap();
        let restored = parser::New().ParseOneStmt(restored, "", "").unwrap();
        assert_eq!(source.as_any().type_id(), restored.as_any().type_id());
    }
}

#[test]
/// 二元运算 Restore 括号。
pub fn test_restore_bin_op_with_brackets() {
    run_contract_table("TestRestoreBinOpWithBrackets", false);
}

#[test]
/// CTE 与绑定。
pub fn test_cte_bindings() {
    run_contract_table("TestCTEBindings", false);
}

#[test]
/// PLAN REPLAYER。
pub fn test_plan_replayer() {
    run_contract_table("TestPlanReplayer", false);
    let mut parser = parser::New();
    let statement = parser
        .ParseOneStmt("PLAN REPLAYER DUMP EXPLAIN SELECT a FROM t", "", "")
        .unwrap();
    let plan_replayer = statement
        .as_any()
        .downcast_ref::<parser_ast::PlanReplayerStmt>()
        .unwrap();
    assert!(!plan_replayer.Analyze);
    assert!(
        plan_replayer
            .Stmt
            .as_ref()
            .is_some_and(|statement| statement.as_any().is::<parser_ast::SelectStmt>())
    );

    let statement = parser
        .ParseOneStmt("PLAN REPLAYER DUMP EXPLAIN ANALYZE SELECT a FROM t", "", "")
        .unwrap();
    let plan_replayer = statement
        .as_any()
        .downcast_ref::<parser_ast::PlanReplayerStmt>()
        .unwrap();
    assert!(plan_replayer.Analyze);
    assert!(plan_replayer.Stmt.is_some());

    for (sql, analyze, expected) in [
        (
            "PLAN REPLAYER DUMP EXPLAIN ('SELECT * FROM t1', 'SELECT * FROM t2')",
            false,
            &["SELECT * FROM t1", "SELECT * FROM t2"][..],
        ),
        (
            "PLAN REPLAYER DUMP EXPLAIN ANALYZE ('SELECT * FROM t1')",
            true,
            &["SELECT * FROM t1"][..],
        ),
    ] {
        let statement = parser.ParseOneStmt(sql, "", "").unwrap();
        let plan_replayer = statement
            .as_any()
            .downcast_ref::<parser_ast::PlanReplayerStmt>()
            .unwrap();
        assert_eq!(plan_replayer.Analyze, analyze, "{sql}");
        assert!(plan_replayer.Stmt.is_none(), "{sql}");
        assert_eq!(plan_replayer.StmtList, expected, "{sql}");
    }
}

#[test]
/// TRAFFIC 语句。
pub fn test_traffic_stmt() {
    for case in parser_contract_cases("TestTrafficStmt") {
        let result = parser::New().ParseOneStmt(case.sql, "", "");
        assert_eq!(result.is_ok(), case.ok, "{}", case.sql);
        if !case.ok {
            continue;
        }
        let statement = result.unwrap();
        let traffic = statement
            .as_any()
            .downcast_ref::<parser_ast::TrafficStmt>()
            .unwrap();
        if matches!(
            traffic.OpType,
            parser_ast::TrafficOpType::Capture | parser_ast::TrafficOpType::Replay
        ) {
            assert_eq!(traffic.Dir, "/tmp", "{}", case.sql);
        }
        assert_eq!(restore_traffic(traffic), case.restored, "{}", case.sql);
    }
}

/// 将 TrafficStmt Restore 为 SQL 文本。
fn restore_traffic(statement: &parser_ast::TrafficStmt) -> String {
    let mut output = match statement.OpType {
        parser_ast::TrafficOpType::Capture => format!("TRAFFIC CAPTURE TO '{}'", statement.Dir),
        parser_ast::TrafficOpType::Replay => format!("TRAFFIC REPLAY FROM '{}'", statement.Dir),
        parser_ast::TrafficOpType::Show => "SHOW TRAFFIC JOBS".to_owned(),
        parser_ast::TrafficOpType::Cancel => "CANCEL TRAFFIC JOBS".to_owned(),
    };
    for option in &statement.Options {
        let restored = match option.OptionType {
            parser_ast::TrafficOptionType::Duration => {
                format!("DURATION = '{}'", option.StrValue)
            }
            parser_ast::TrafficOptionType::EncryptionMethod => {
                format!("ENCRYPTION_METHOD = '{}'", option.StrValue)
            }
            parser_ast::TrafficOptionType::Compress => {
                format!(
                    "COMPRESS = {}",
                    if option.BoolValue { "TRUE" } else { "FALSE" }
                )
            }
            parser_ast::TrafficOptionType::Username => format!("USER = '{}'", option.StrValue),
            parser_ast::TrafficOptionType::Password => {
                format!("PASSWORD = '{}'", option.StrValue)
            }
            parser_ast::TrafficOptionType::Speed => {
                match option.FloatValue.as_ref().map(|value| &value.Kind) {
                    Some(parser_ast::ExprKind::Value(value)) => format!("SPEED = {value}"),
                    value => panic!("unexpected traffic speed {value:?}"),
                }
            }
            parser_ast::TrafficOptionType::ReadOnly => {
                format!(
                    "READONLY = {}",
                    if option.BoolValue { "TRUE" } else { "FALSE" }
                )
            }
        };
        output.push(' ');
        output.push_str(&restored);
    }
    output
}

/// 断言指定字符集下建表默认值解析路径正确。
fn assert_encoding_parser_path(charset: &str, default_value: &str) {
    let _charset_guard = CHARSET_TEST_LOCK.lock().unwrap();
    let sql = format!("create table 测试表 (测试列 varchar(255) default '{default_value}');");
    let encoding = parser::charset::encoding::FindEncoding(charset);
    let encoded = encoding
        .Transform(
            &mut Vec::new(),
            sql.as_bytes(),
            parser::charset::encoding::OpEncode,
        )
        .unwrap();
    let decoded = encoding
        .Transform(
            &mut Vec::new(),
            &encoded,
            parser::charset::encoding::OpDecode,
        )
        .unwrap();
    assert_eq!(decoded, sql.as_bytes());

    let statement = parser::New().ParseOneStmt(&sql, "", "").unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    assert_eq!(create.Table.Name.O, "测试表");
    assert_eq!(create.Cols[0].Name.Name.O, "测试列");
    assert!(create.Cols[0].Options.iter().any(|option| {
        matches!(
            option.Expr.as_ref().map(|expression| &expression.Kind),
            Some(parser_ast::ExprKind::Value(value)) if value == default_value
        )
    }));

    let option = parser::CharsetClient(charset.to_owned());
    let (statements, _) = parser::New()
        .ParseSQL(
            "create table t (c varchar(255) default 'ascii')",
            &[&option],
        )
        .unwrap();
    assert!(statements[0].as_any().is::<parser_ast::CreateTableStmt>());
}

#[test]
/// GBK 编码解析路径。
pub fn test_gbk_encoding() {
    assert_encoding_parser_path("gbk", "GBK测试用例");
}

#[test]
/// GB18030 编码解析路径。
pub fn test_gb18030_encoding() {
    assert_encoding_parser_path("gb18030", "GB18030测试用例");
}

#[test]
/// INSERT 解析内存分配观测。
pub fn test_insert_statement_memory_allocation() {
    let sql = format!("insert t values (1){}", ",(1)".repeat(1000));
    parser::New()
        .ParseOneStmt("insert t values (1)", "", "")
        .unwrap();
    THREAD_ALLOCATED.with(|allocated| allocated.set(0));
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(true));
    let statement = parser::New().ParseOneStmt(&sql, "", "").unwrap();
    TRACK_ALLOCATIONS.with(|tracking| tracking.set(false));
    let allocated = THREAD_ALLOCATED.with(Cell::get);
    let insert = statement
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert_eq!(insert.Lists.len(), 1001);
    assert!(allocated < 500 * 1024, "allocated {allocated} bytes");
}

/// 字符集 introducer Restore 期望对照。
struct CharsetRestore {
    charset: Option<parser::charset::charset::Charset>,
    collations: Vec<parser::charset::charset::Collation>,
}

impl Drop for CharsetRestore {
    fn drop(&mut self) {
        if let Some(charset) = self.charset.take() {
            parser::charset::charset::AddCharset(charset);
            for collation in self.collations.drain(..) {
                parser::charset::charset::AddCollation(collation);
            }
        }
    }
}

#[test]
/// 字符集 introducer Restore。
pub fn test_charset_introducer() {
    let _charset_guard = CHARSET_TEST_LOCK.lock().unwrap();
    let charset = parser::charset::charset::GetCharsetInfo("gbk").unwrap();
    let collations = charset.Collations.values().cloned().collect();
    let _restore = CharsetRestore {
        charset: Some(charset),
        collations,
    };
    parser::charset::charset::RemoveCharset("gbk");

    let mut parser = parser::New();
    for sql in [
        "select _gbk 'a';",
        "select _gbk 0x1234;",
        "select _gbk 0b101001;",
    ] {
        let error = match parser.Parse(sql, "", "") {
            Err(error) => error,
            Ok(_) => panic!("{sql}: expected unsupported introducer error"),
        };
        assert_eq!(
            error.to_string(),
            "[ddl:1115]Unsupported character introducer: 'gbk'",
            "{sql}"
        );
    }
}

#[test]
/// 非事务 DML 拆分语句。
pub fn test_non_transactional_dml() {
    run_contract_table("TestNonTransactionalDML", false);
}

#[test]
/// INTERVAL 分区。
pub fn test_interval_partition() {
    run_contract_table("TestIntervalPartition", false);
}

#[test]
/// TTL 表选项。
pub fn test_ttl_table_option() {
    run_contract_table("TestTTLTableOption", false);
}

#[test]
/// 回归 issue 45898。
pub fn test_issue45898() {
    let mut parser = parser::New();
    assert!(parser.ParseSQL("a.", &[]).is_err());
    let (statements, _) = parser.ParseSQL("select count(1) from t", &[]).unwrap();
    let select = statements[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().map(|expression| &expression.Kind),
        Some(parser_ast::ExprKind::AggregateFunction { Name, Args, .. })
            if Name.eq_ignore_ascii_case("count") && Args.len() == 1
    ));
}

#[test]
/// 测试用例：test_multi_stmt。
pub fn test_multi_stmt() {
    let (statements, _) = parser::New()
        .Parse(
            "SELECT 'foo'; SELECT 'foo;bar','baz'; select 'foo' , 'bar' , 'baz' ;select 1",
            "",
            "",
        )
        .unwrap();
    assert_eq!(statements.len(), 4);
    let expected: [&[&str]; 4] = [
        &["foo"],
        &["foo;bar", "baz"],
        &["foo", "bar", "baz"],
        &["1"],
    ];
    for (statement, expected) in statements.iter().zip(expected) {
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let actual = select
            .Fields
            .Fields
            .iter()
            .map(|field| match &field.Expr.as_ref().unwrap().Kind {
                parser_ast::ExprKind::Value(value) => value.text(),
                kind => panic!("expected value expression, got {kind:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
/// 测试用例：test_compat_types。
pub fn test_compat_types() {
    run_contract_table("TestCompatTypes", false);
}

#[test]
/// 测试用例：test_vector。
pub fn test_vector() {
    run_contract_table("TestVector", false);
}

#[test]
/// 测试用例：test_explain_explore。
pub fn test_explain_explore() {
    run_contract_table("TestExplainExplore", false);
    let statement = parser::New()
        .ParseOneStmt("explain explore replayer '/tmp/replayer.zip'", "", "")
        .unwrap();
    let explain = statement
        .as_any()
        .downcast_ref::<parser_ast::ExplainStmt>()
        .unwrap();
    assert!(explain.Explore);
    assert_eq!(explain.ReplayerFile, "/tmp/replayer.zip");
    assert!(explain.SQLDigest.is_empty());
    assert!(explain.stmt.is_none());
}

#[test]
/// 测试用例：test_compat_maria_db。
pub fn test_compat_maria_db() {
    run_contract_table("TestCompatMariaDB", false);
}

#[test]
/// 测试用例：test_uuid_type_maria_db_enabled。
pub fn test_uuid_type_maria_db_enabled() {
    run_contract_table("TestUUIDTypeMariaDBEnabled", true);
}

#[test]
/// 测试用例：test_uuid_keyword_compatibility。
pub fn test_uuid_keyword_compatibility() {
    run_contract_table("TestUUIDKeywordCompatibility", false);
}

#[test]
/// 测试用例：test_uuid_type_maria_db_disabled。
pub fn test_uuid_type_maria_db_disabled() {
    run_contract_table("TestUUIDTypeMariaDBDisabled", false);
}

#[test]
/// 测试用例：test_secondary_engine_attribute。
pub fn test_secondary_engine_attribute() {
    run_contract_table("TestSecondaryEngineAttribute", false);
}

#[test]
/// 测试用例：test_partial_index。
pub fn test_partial_index() {
    run_contract_table("TestPartialIndex", false);
}

#[test]
/// 测试用例：test_table_affinity_option。
pub fn test_table_affinity_option() {
    run_contract_table("TestTableAffinityOption", false);
}

#[test]
/// 测试用例：test_split_partition。
pub fn test_split_partition() {
    run_contract_table("TestSplitPartition", false);
}

#[test]
fn full_outer_join_syntax_preserves_alias_and_restore() {
    for (sql, expected, kind) in [
        (
            "select * from t1 full join t2 on t1.a = t2.a",
            "SELECT * FROM `t1` AS `full` JOIN `t2` ON `t1`.`a`=`t2`.`a`",
            parser_ast::JoinType::CrossJoin,
        ),
        (
            "select * from t1 full outer join t2 on t1.a <=> t2.a",
            "SELECT * FROM `t1` FULL OUTER JOIN `t2` ON `t1`.`a`<=>`t2`.`a`",
            parser_ast::JoinType::FullJoin,
        ),
        (
            "select * from t1 full /* lookahead */ outer join t2 on t1.a <=> t2.a",
            "SELECT * FROM `t1` FULL OUTER JOIN `t2` ON `t1`.`a`<=>`t2`.`a`",
            parser_ast::JoinType::FullJoin,
        ),
        (
            "select * from full",
            "SELECT * FROM `full`",
            parser_ast::JoinType::CrossJoin,
        ),
        (
            "select * from t1 as full",
            "SELECT * FROM `t1` AS `full`",
            parser_ast::JoinType::CrossJoin,
        ),
    ] {
        let statement = crate::New().ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(select.From.as_ref().unwrap().TableRefs.Tp, kind, "{sql}");
        assert_eq!(
            parser_ast::sql_restore::restore_node(statement.as_ref()).unwrap(),
            expected,
            "{sql}"
        );
    }
}

/// Preserve temporal INTERVAL disambiguation and the restored AST of nested rows.
#[test]
fn temporal_interval_expressions_restore() {
    let cases = [
        ("SELECT INTERVAL()", false, ""),
        ("SELECT INTERVAL(1)", false, ""),
        ("SELECT INTERVAL(1, 0)", true, "SELECT INTERVAL(1, 0)"),
        (
            "SELECT NOW() + INTERVAL(1+2) DAY `add`",
            true,
            "SELECT DATE_ADD(NOW(), INTERVAL (1+2) DAY) AS `add`",
        ),
        (
            "SELECT d + INTERVAL (q - 1) QUARTER",
            true,
            "SELECT DATE_ADD(`d`, INTERVAL (`q`-1) QUARTER)",
        ),
        (
            "SELECT d - INTERVAL (q - 1) QUARTER",
            true,
            "SELECT DATE_SUB(`d`, INTERVAL (`q`-1) QUARTER)",
        ),
        (
            "SELECT INTERVAL (q - 1) QUARTER + d",
            true,
            "SELECT DATE_ADD(`d`, INTERVAL (`q`-1) QUARTER)",
        ),
        (
            "SELECT ADDDATE(d, INTERVAL (q - 1) QUARTER)",
            true,
            "SELECT ADDDATE(`d`, INTERVAL (`q`-1) QUARTER)",
        ),
        (
            "SELECT SUBDATE(d, INTERVAL (q - 1) QUARTER)",
            true,
            "SELECT SUBDATE(`d`, INTERVAL (`q`-1) QUARTER)",
        ),
        (
            "SELECT ROW(ROW(1,2),3), ROW(1,ROW(2,3))",
            true,
            "SELECT ROW(ROW(1,2),3),ROW(1,ROW(2,3))",
        ),
        (
            "SELECT (1,2), ((1,2),3), (1,(2,3))",
            true,
            "SELECT ROW(1,2),ROW(ROW(1,2),3),ROW(1,ROW(2,3))",
        ),
        (
            "SELECT MAKEDATE(YEAR(d), 1) + INTERVAL (QUARTER(d) - 1) QUARTER",
            true,
            "SELECT DATE_ADD(MAKEDATE(YEAR(`d`), 1), INTERVAL (QUARTER(`d`)-1) QUARTER)",
        ),
        (
            "SELECT INTERVAL(1, 0, 1, 2)",
            true,
            "SELECT INTERVAL(1, 0, 1, 2)",
        ),
        (
            "SELECT (INTERVAL(1, 0, 1, 2)+5)*7+INTERVAL(1, 0, 1, 2)/2",
            true,
            "SELECT (INTERVAL(1, 0, 1, 2)+5)*7+INTERVAL(1, 0, 1, 2)/2",
        ),
        (
            "SELECT INTERVAL(0, (1*5)/2)+INTERVAL(5, 4, 3)",
            true,
            "SELECT INTERVAL(0, (1*5)/2)+INTERVAL(5, 4, 3)",
        ),
        (
            "SELECT DATE_ADD('2008-01-02', INTERVAL INTERVAL(1, 0, 1) DAY);",
            true,
            "SELECT DATE_ADD(_UTF8MB4'2008-01-02', INTERVAL INTERVAL(1, 0, 1) DAY)",
        ),
    ];
    for (sql, accepted, expected) in cases {
        let result = crate::New().ParseOneStmt(sql, "", "");
        assert_eq!(
            result.is_ok(),
            accepted,
            "{sql}: {:?}",
            result.as_ref().err()
        );
        if accepted {
            let statement = result.unwrap();
            assert_eq!(
                crate::ast::sql_restore::restore_node(statement.as_ref()).unwrap(),
                expected,
                "{sql}"
            );
        }
    }
}

#[test]
fn pg_core_integer_cast_syntax() {
    use parser_ast::{ExprKind, SqlDialect};
    let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 1::int4::int8",
        "SELECT ((1)::int4)::int8",
        "SELECT (1+2)::int4",
        "SELECT 1+2::int4",
        "SELECT CAST(1 AS int4)",
        "SELECT CAST(1 AS integer)",
        "SELECT CAST(1 AS int)",
        "SELECT CAST(1 AS int8)",
        "SELECT CAST(1 AS bigint)",
        "SELECT ($1)::int4",
        "SELECT '::int4' /* ::int8 */",
    ] {
        let stmt = pg
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        if sql == "SELECT 1+2::int4" {
            assert!(
                matches!(&expr.Kind, ExprKind::Binary { R, .. } if matches!(R.Kind, ExprKind::Cast { .. }))
            );
        } else if !sql.contains("'::") {
            assert!(
                matches!(expr.Kind, ExprKind::Cast { .. }),
                "{sql}: {expr:?}"
            );
        }
    }
    for sql in [
        "SELECT 1::",
        "SELECT 1::text",
        "SELECT CAST(1 AS int2)",
        "SELECT CAST(1 AS int4 ARRAY)",
        "SELECT 1::int4(2)",
    ] {
        assert!(pg.ParseOneStmt(sql, "", "").is_err(), "{sql}");
    }
    let mut mysql = parser::New();
    assert!(mysql.ParseOneStmt("SELECT 1::int4", "", "").is_err());
    assert!(
        mysql
            .ParseOneStmt("SELECT CAST(1 AS SIGNED)", "", "")
            .is_ok()
    );
}

#[test]
fn pg_core_integer_cast_syntax_source_and_tree() {
    use parser_ast::{ExprKind, SqlDialect};
    use parser_mysql::r#type as mysql;
    fn expression(sql: &str) -> parser_ast::ExprNode {
        let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseOneStmt(sql, "", "")
            .unwrap();
        stmt.as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap()
            .Fields
            .Fields[0]
            .Expr
            .clone()
            .unwrap()
    }
    fn cast(expr: &parser_ast::ExprNode, tp: u8) -> &parser_ast::ExprNode {
        match &expr.Kind {
            ExprKind::Cast { Expr, Tp, .. } => {
                assert_eq!(Tp.GetType(), tp);
                Expr
            }
            _ => panic!("expected cast: {expr:?}"),
        }
    }
    for (name, tp) in [
        ("int4", mysql::TypeLong),
        ("integer", mysql::TypeLong),
        ("int", mysql::TypeLong),
        ("int8", mysql::TypeLonglong),
        ("bigint", mysql::TypeLonglong),
    ] {
        let suffix = expression(&format!("SELECT 1::{name}"));
        let standard = expression(&format!("SELECT CAST(1 AS {name})"));
        cast(&suffix, tp);
        cast(&standard, tp);
        let (ExprKind::Cast { Tp: left, .. }, ExprKind::Cast { Tp: right, .. }) =
            (&suffix.Kind, &standard.Kind)
        else {
            unreachable!()
        };
        assert_eq!(
            left, right,
            "both syntaxes must produce the same canonical target"
        );
    }
    let literal = expression("SELECT '$12' /* ::int8 $1 */");
    assert!(matches!(&literal.Kind, ExprKind::Value(v) if v == "$12"));
    assert_eq!(literal.PostgreSQLParameterIndex(), None);
    let outer = expression("SELECT 1::iNt4::InT8");
    let inner = cast(&outer, mysql::TypeLonglong);
    cast(inner, mysql::TypeLong);
    assert_eq!(outer.node_text.OriginalText(), b"1::iNt4::InT8");
    assert_eq!(inner.node_text.OriginalText(), b"1::iNt4");
    assert_eq!(outer.OriginTextPosition, 7);
    let outer = expression("SELECT ((1)::int4)::int8");
    let inner = cast(&outer, mysql::TypeLonglong);
    let ExprKind::Parentheses(inner) = &inner.Kind else {
        panic!("{inner:?}")
    };
    let value = cast(inner, mysql::TypeLong);
    assert!(matches!(value.Kind, ExprKind::Parentheses(_)));
    let outer = expression("SELECT (1+2)::int4");
    let inner = cast(&outer, mysql::TypeLong);
    assert!(
        matches!(&inner.Kind, ExprKind::Parentheses(v) if matches!(v.Kind, ExprKind::Binary { .. }))
    );
    let sql = "SELECT /*中*/ ($12)::InTeGeR";
    let outer = expression(sql);
    assert_eq!(outer.node_text.OriginalText(), b"($12)::InTeGeR");
    assert_eq!(outer.OriginTextPosition as usize, sql.find('(').unwrap());
    let inner = cast(&outer, mysql::TypeLong);
    let ExprKind::Parentheses(param) = &inner.Kind else {
        panic!("{inner:?}")
    };
    assert_eq!(param.PostgreSQLParameterIndex(), Some(12));
    assert!(
        matches!(param.Kind, ExprKind::ParamMarker { Offset } if Offset == sql.find('$').unwrap())
    );
    let outer = expression("SELECT CAST(($2)::int4 AS BiGiNt)");
    assert_eq!(
        outer.node_text.OriginalText(),
        b"CAST(($2)::int4 AS BiGiNt)"
    );
    cast(cast(&outer, mysql::TypeLonglong), mysql::TypeLong);
}

#[test]
fn pg_core_integer_cast_syntax_unicode_position() {
    for sql in ["SELECT 1::", "SELECT '中', 1::", "/*中*/ SELECT 1::"] {
        let error = parser::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL)
            .ParseOneStmt(sql, "", "")
            .err()
            .expect("missing target must fail");
        let expected = format!("line 1 column {} ", sql.chars().count() + 1);
        assert!(
            error.to_string().contains(&expected),
            "{sql}: {error}; expected {expected}"
        );
    }
}

#[test]
fn pg_core_int4_ast_semantics() {
    let mut pg = parser::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 42::int4",
        "SELECT CAST(42 AS integer)",
        "SELECT CAST(42 AS int)",
    ] {
        let stmt = pg.ParseOneStmt(sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(matches!(
            expr.Kind,
            parser_ast::ExprKind::Cast {
                FunctionType: parser_ast::CastFunctionType::PostgreSQLInt4,
                ..
            }
        ));
        let restored = parser_ast::sql_restore::restore_expr(expr).unwrap();
        assert_eq!(restored, "CAST(42 AS INT4)");
        let reparsed = pg
            .ParseOneStmt(&format!("SELECT {restored}"), "", "")
            .unwrap();
        let reparsed = reparsed
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let roundtrip = reparsed.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(
            matches!(roundtrip.Kind, parser_ast::ExprKind::Cast { FunctionType: parser_ast::CastFunctionType::PostgreSQLInt4, ref Tp, .. } if Tp.GetType() == parser_mysql::r#type::TypeLong)
        );
        assert_eq!(
            parser_ast::sql_restore::restore_expr(roundtrip).unwrap(),
            restored
        );
    }
    let stmt = parser::New()
        .ParseOneStmt("SELECT CAST(42 AS SIGNED)", "", "")
        .unwrap();
    let select = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::Cast {
            FunctionType: parser_ast::CastFunctionType::Cast,
            ..
        }
    ));
}

#[test]
fn pg_core_int8_ast_semantics() {
    let mut pg = parser::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 42::int8",
        "SELECT CAST(42 AS bigint)",
        "SELECT CAST(42 AS int8)",
    ] {
        let stmt = pg.ParseOneStmt(sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(matches!(
            expr.Kind,
            parser_ast::ExprKind::Cast {
                FunctionType: parser_ast::CastFunctionType::PostgreSQLInt8,
                ..
            }
        ));
        let restored = parser_ast::sql_restore::restore_expr(expr).unwrap();
        assert_eq!(restored, "CAST(42 AS INT8)");
        let reparsed = pg
            .ParseOneStmt(&format!("SELECT {restored}"), "", "")
            .unwrap();
        let reparsed = reparsed
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let roundtrip = reparsed.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(
            matches!(roundtrip.Kind, parser_ast::ExprKind::Cast { FunctionType: parser_ast::CastFunctionType::PostgreSQLInt8, ref Tp, .. } if Tp.GetType() == parser_mysql::r#type::TypeLonglong)
        );
        assert_eq!(
            parser_ast::sql_restore::restore_expr(roundtrip).unwrap(),
            restored
        );
    }
    let stmt = parser::New()
        .ParseOneStmt("SELECT CAST(42 AS SIGNED)", "", "")
        .unwrap();
    let select = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::Cast {
            FunctionType: parser_ast::CastFunctionType::Cast,
            ..
        }
    ));
}

#[test]
fn pg_dialect_01_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    fn expression(sql: &str, dialect: SqlDialect) -> parser_ast::ExprNode {
        let stmt = parser::Parser::with_dialect(dialect)
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        stmt.as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap()
            .Fields
            .Fields[0]
            .Expr
            .clone()
            .unwrap()
    }
    fn predicate(expr: &parser_ast::ExprNode, expected_not: bool) {
        let ExprKind::Like {
            IsLike,
            Not,
            Expr,
            Pattern,
            ..
        } = &expr.Kind
        else {
            panic!("ILIKE predicate lost: {expr:?}");
        };
        assert!(!IsLike, "ILIKE became LIKE");
        assert_eq!(*Not, expected_not);
        assert!(matches!(&Expr.Kind, ExprKind::Value(v) if v == "Ab"));
        assert!(matches!(&Pattern.Kind, ExprKind::Value(v) if v == "a%"));
    }
    for (sql, not) in [
        ("SELECT 'Ab' ILIKE 'a%'", false),
        ("SELECT 'Ab' NOT ILIKE 'a%'", true),
    ] {
        let expr = expression(sql, SqlDialect::PostgreSQL);
        predicate(&expr, not);
        assert_eq!(
            expr.OriginTextPosition, 7,
            "original predicate start: {sql}"
        );
        assert_eq!(expr.node_text.OriginalText(), &sql.as_bytes()[7..]);
        // Capture the existing MySQL extension before any acceptance changes.
        predicate(&expression(sql, SqlDialect::MySQL), not);
    }
    let outer = expression("SELECT NOT ('Ab' ILIKE 'a%')", SqlDialect::PostgreSQL);
    let ExprKind::Unary { Op, V } = &outer.Kind else {
        panic!("outer NOT lost: {outer:?}")
    };
    assert_eq!(Op, "NOT");
    let ExprKind::Parentheses(inner) = &V.Kind else {
        panic!("parentheses lost: {V:?}")
    };
    predicate(inner, false);
    let protected = expression("SELECT 'ILIKE' /* ILIKE */", SqlDialect::PostgreSQL);
    assert!(matches!(&protected.Kind, ExprKind::Value(v) if v == "ILIKE"));
    let alias = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt("SELECT 'Ab' ILIKE", "", "")
        .expect("PostgreSQL18.6 treats trailing ILIKE as a bare alias");
    let select = alias
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.Fields.Fields[0].AsName.O, "ilike");
    assert!(
        matches!(&select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        ExprKind::Value(v) if v == "Ab")
    );
    assert!(
        parser::New()
            .ParseOneStmt("SELECT 'Ab' ILIKE", "", "")
            .is_err()
    );
    assert!(
        parser::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseOneStmt("SELECT 'Ab' NOT ILIKE", "", "")
            .is_err()
    );
    let sql = "SELECT 'Ab' ILIKE (";
    let error = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(sql, "", "")
        .err()
        .expect("missing operand must fail");
    assert!(
        error
            .to_string()
            .contains(&format!("column {} ", sql.chars().count() + 1)),
        "{error}"
    );
    let null = expression(
        "SELECT CAST(NULL AS CHAR) ILIKE 'a%'",
        SqlDialect::PostgreSQL,
    );
    let ExprKind::Like {
        Expr,
        IsLike: false,
        ..
    } = &null.Kind
    else {
        panic!("{null:?}")
    };
    let ExprKind::Cast { Expr, .. } = &Expr.Kind else {
        panic!("typed NULL cast lost")
    };
    assert!(matches!(&Expr.Kind, ExprKind::Value(v) if v.Datum == parser_ast::ValueDatum::Null));
    let sql = "SELECT /*中*/ $2 ILIKE 'a%'";
    let parameter = expression(sql, SqlDialect::PostgreSQL);
    let ExprKind::Like {
        Expr,
        IsLike: false,
        ..
    } = &parameter.Kind
    else {
        panic!("{parameter:?}")
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(2));
    assert!(
        matches!(Expr.Kind, ExprKind::ParamMarker { Offset } if Offset == sql.find('$').unwrap())
    );
    // Source ends must follow consumed tokens, not decoded literal lengths or
    // the next lookahead token; multibyte comments must not shift byte offsets.
    for (sql, expected) in [
        (
            "SELECT /*中*/ 'Ab' /* ILIKE */ ILIKE 'a%' AS matched",
            "'Ab' /* ILIKE */ ILIKE 'a%'",
        ),
        (
            "SELECT 'Ab' ILIKE 'a''%' /* trailing */",
            "'Ab' ILIKE 'a''%'",
        ),
        ("SELECT ('Ab' ILIKE ('a%'))", "'Ab' ILIKE ('a%')"),
    ] {
        let expr = expression(sql, SqlDialect::PostgreSQL);
        let predicate = match &expr.Kind {
            ExprKind::Parentheses(inner) => inner.as_ref(),
            _ => &expr,
        };
        assert_eq!(
            predicate.node_text.OriginalText(),
            expected.as_bytes(),
            "{sql}"
        );
        let start = predicate.OriginTextPosition as usize;
        assert_eq!(
            &sql.as_bytes()[start..start + expected.len()],
            expected.as_bytes()
        );
    }
    for sql in ["/*中*/ SELECT 'Ab' ILIKE (", "SELECT\n'Ab' ILIKE ("] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err());
        assert_eq!(
            parser.PostgreSQLEofErrorPosition(),
            Some(sql.chars().count() + 1)
        );
        assert!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
        assert_eq!(parser.PostgreSQLEofErrorPosition(), None);
    }
    println!(
        "pg_dialect_01_behavior: 13 primary inputs, reset probes and MySQL extension baselines"
    );
}

#[test]
fn pg_dialect_02_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    for (suffix, escape, explicit) in [
        (" ESCAPE '!'", "!", true),
        ("", "\\", false),
        (" ESCAPE ''", "", true),
        (" ESCAPE '中'", "中", true),
        (" ESCAPE 'xy'", "xy", true),
    ] {
        let sql = format!("SELECT /*中*/ 'A_b' ILIKE 'a!_b'{suffix}");
        let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let expr = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap()
            .Fields
            .Fields[0]
            .Expr
            .as_ref()
            .unwrap();
        let ExprKind::Like {
            Escape,
            Explicit,
            IsLike,
            ..
        } = &expr.Kind
        else {
            panic!("{expr:?}")
        };
        assert_eq!(Escape, escape);
        assert_eq!(*Explicit, explicit);
        assert!(!IsLike);
        let ExprKind::Like { EscapeExpr, .. } = &expr.Kind else {
            unreachable!()
        };
        assert_eq!(EscapeExpr.is_some(), explicit);
        if let Some(escape_expr) = EscapeExpr {
            assert!(matches!(&escape_expr.Kind, ExprKind::Value(v) if v == escape));
            let offset = sql.rfind('\'').unwrap() - escape.len() - 1;
            assert_eq!(escape_expr.OriginTextPosition as usize, offset);
            assert_eq!(
                escape_expr.node_text.OriginalText(),
                &sql.as_bytes()[offset..]
            );
        }
        let start = sql.find("'A_b'").unwrap();
        assert_eq!(expr.OriginTextPosition as usize, start);
        assert_eq!(expr.node_text.OriginalText(), &sql.as_bytes()[start..]);
    }
    for sql in [
        "SELECT 'A_b' ILIKE 'a!_b' ESCAPE ($1)",
        "SELECT 'A_b' ILIKE 'a_b' ESCAPE NULL",
    ] {
        assert!(
            parser::Parser::with_dialect(SqlDialect::PostgreSQL)
                .ParseOneStmt(sql, "", "")
                .is_ok(),
            "{sql}"
        );
    }
    for sql in [
        "SELECT 'A_b' ILIKE 'a_b' ESCAPE",
        "/*中*/ SELECT 'A_b' ILIKE 'a_b' ESCAPE",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err());
        assert_eq!(
            parser.PostgreSQLEofErrorPosition(),
            Some(sql.chars().count() + 1)
        );
    }
    let sql = "SELECT /*中*/ 'A_b' ILIKE 'a!_b' ESCAPE ($1)";
    let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(sql, "", "")
        .unwrap();
    let expr = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .as_ref()
        .unwrap();
    let ExprKind::Like {
        EscapeExpr: Some(escape),
        ..
    } = &expr.Kind
    else {
        panic!("missing escape expression")
    };
    let ExprKind::Parentheses(marker) = &escape.Kind else {
        panic!("parentheses lost")
    };
    assert_eq!(marker.PostgreSQLParameterIndex(), Some(1));
    assert!(
        matches!(marker.Kind, ExprKind::ParamMarker { Offset } if Offset == sql.find('$').unwrap())
    );
    assert_eq!(
        escape.OriginTextPosition as usize,
        sql.find("($1)").unwrap()
    );
    assert_eq!(escape.node_text.OriginalText(), b"($1)");
    assert!(
        parser_ast::sql_restore::restore_expr(expr)
            .unwrap()
            .ends_with(" ESCAPE ($1)")
    );
    struct Markers(Vec<usize>);
    impl parser_ast::Visitor for Markers {
        fn enter(&mut self, node: &dyn parser_ast::Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                if let Some(index) = expr.PostgreSQLParameterIndex() {
                    self.0.push(usize::from(index));
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn parser_ast::Node) -> bool {
            true
        }
    }
    let mut markers = Markers(Vec::new());
    assert!(stmt.accept(&mut markers));
    assert_eq!(
        markers.0,
        vec![1],
        "ESCAPE child must reach the shared visitor"
    );
    let null_sql = "SELECT 'A_b' ILIKE 'a_b' ESCAPE NULL";
    let null_stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(null_sql, "", "")
        .unwrap();
    let null_expr = null_stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .as_ref()
        .unwrap();
    assert!(
        matches!(&null_expr.Kind, ExprKind::Like { EscapeExpr: Some(expr), .. } if matches!(&expr.Kind, ExprKind::Value(v) if v.Datum == parser_ast::ValueDatum::Null))
    );
    assert!(
        parser_ast::sql_restore::restore_expr(null_expr)
            .unwrap()
            .ends_with(" ESCAPE NULL")
    );
    // MySQL retains its pre-existing byte-literal grammar and validation.
    assert!(
        parser::New()
            .ParseOneStmt("SELECT 'A_b' ILIKE 'a_b' ESCAPE '!'", "", "")
            .is_ok()
    );
    for suffix in ["'中'", "'xy'", "($1)", "NULL"] {
        assert!(
            parser::New()
                .ParseOneStmt(&format!("SELECT 'A_b' ILIKE 'a_b' ESCAPE {suffix}"), "", "")
                .is_err()
        );
    }
    println!("pg_dialect_02_behavior: 9 parser cases plus MySQL baseline probes");
}

#[test]
fn pg_dialect_03_behavior() {
    use parser_ast::{ExprKind, ExprNode, SqlDialect};
    fn shape(expr: &ExprNode) -> String {
        match &expr.Kind {
            ExprKind::Binary { Op, L, R } => format!("{Op}({},{})", shape(L), shape(R)),
            ExprKind::Unary { Op, V } => format!("{Op}({})", shape(V)),
            ExprKind::Parentheses(v) => format!("({})", shape(v)),
            ExprKind::Like {
                Expr,
                Pattern,
                Not,
                IsLike,
                ..
            } => format!(
                "{}{}({},{})",
                if *Not { "NOT " } else { "" },
                if *IsLike { "LIKE" } else { "ILIKE" },
                shape(Expr),
                shape(Pattern)
            ),
            ExprKind::IsTruth { Expr, Not, True } => format!("IS{}{}({})", Not, True, shape(Expr)),
            _ => "v".into(),
        }
    }
    let cases = [
        ("SELECT 'Ab' ILIKE 'a%' = TRUE", "=(ILIKE(v,v),v)"),
        (
            "SELECT NOT 'Ab' ILIKE 'a%' AND TRUE",
            "AND(NOT(ILIKE(v,v)),v)",
        ),
        (
            "SELECT NOT ('Ab' ILIKE 'a%' AND TRUE)",
            "NOT((AND(ILIKE(v,v),v)))",
        ),
        ("SELECT 'Ab' NOT ILIKE 'a%'", "NOT ILIKE(v,v)"),
        ("SELECT NOT 'Ab' NOT ILIKE 'a%'", "NOT(NOT ILIKE(v,v))"),
        (
            "SELECT 'Ab' ILIKE 'a%' IS TRUE OR FALSE",
            "OR(ISfalsetrue(ILIKE(v,v)),v)",
        ),
        ("SELECT ('Ab' ILIKE 'a%') = TRUE", "=((ILIKE(v,v)),v)"),
        ("SELECT 'Ab' ILIKE ('a%')", "ILIKE(v,(v))"),
        (
            "SELECT CAST(NULL AS CHAR) ILIKE 'a%' = TRUE",
            "=(ILIKE(v,v),v)",
        ),
    ];
    for mode in [
        parser::mysql::SQLMode(0),
        parser::mysql::ModeHighNotPrecedence,
    ] {
        let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        pg.SetSQLMode(mode);
        for (sql, expected) in cases {
            let stmt = pg
                .ParseOneStmt(sql, "", "")
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
            let select = stmt
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert_eq!(
                shape(select.Fields.Fields[0].Expr.as_ref().unwrap()),
                expected,
                "{sql}, mode={mode:?}"
            );
        }
        let mut mysql = parser::Parser::with_dialect(SqlDialect::MySQL);
        mysql.SetSQLMode(mode);
        let stmt = mysql
            .ParseOneStmt("SELECT NOT 'Ab' LIKE 'a%'", "", "")
            .unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(
            shape(select.Fields.Fields[0].Expr.as_ref().unwrap()),
            if mode == parser::mysql::SQLMode(0) {
                "NOT(LIKE(v,v))"
            } else {
                "LIKE(!(v),v)"
            }
        );
    }
    let sql = "/*中*/ SELECT $2 ILIKE 'a%' = TRUE";
    let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(sql, "", "")
        .unwrap();
    let select = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let ExprKind::Binary { L, .. } = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind else {
        panic!("comparison lost")
    };
    let ExprKind::Like { Expr, Pattern, .. } = &L.Kind else {
        panic!("predicate lost")
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(2));
    assert_eq!(Expr.node_text.OriginalText(), b"$2");
    assert_eq!(
        Expr.node_text.OriginTextPosition(),
        sql.find("$2").unwrap() as i32
    );
    assert!(matches!(&Pattern.Kind, ExprKind::Value(v) if v == "a%"));
    assert_eq!(L.node_text.OriginalText(), b"$2 ILIKE 'a%'");
    assert_eq!(L.OriginTextPosition, sql.find("$2").unwrap() as i32);
    let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    let invalid = "/*中*/ SELECT NOT 'Ab' ILIKE (";
    assert!(pg.ParseOneStmt(invalid, "", "").is_err());
    assert_eq!(
        pg.PostgreSQLEofErrorPosition(),
        Some(invalid.chars().count() + 1)
    );
    assert!(
        pg.ParseOneStmt("SELECT 'NOT ILIKE $2' /* AND TRUE */", "", "")
            .is_ok()
    );
    assert_eq!(pg.PostgreSQLEofErrorPosition(), None);
    println!("pg_dialect_03_behavior: 9 PG shapes x 2 modes, 2 MySQL mode baselines");
}

#[test]
fn pg_dialect_04_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    fn insert(sql: &str, dialect: SqlDialect) -> parser_ast::InsertStmt {
        let node = parser::Parser::with_dialect(dialect)
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        *node
            .into_any()
            .downcast::<parser_ast::InsertStmt>()
            .ok()
            .expect("INSERT AST")
    }
    let sql = "/*中*/ INSERT INTO public.t(id) VALUES ($2) RETURNING id, id+1 AS \"Next_ID\", t.*, $1 AS \"P\"";
    let stmt = insert(sql, SqlDialect::PostgreSQL);
    assert_eq!(stmt.Returning.len(), 4);
    assert!(matches!(&stmt.Returning[0].Expr.as_ref().unwrap().Kind,
        ExprKind::Column(c) if c.Name.O == "id"));
    assert!(matches!(&stmt.Returning[1].Expr.as_ref().unwrap().Kind,
        ExprKind::Binary { Op, L, R }
            if Op == "+" && matches!(&L.Kind, ExprKind::Column(c) if c.Name.O == "id")
            && matches!(&R.Kind, ExprKind::Value(v) if v.text() == "1")));
    assert_eq!(stmt.Returning[1].AsName.O, "Next_ID");
    let wildcard = stmt.Returning[2].WildCard.as_ref().unwrap();
    assert_eq!(wildcard.Table.O, "t");
    assert!(wildcard.Schema.O.is_empty());
    assert!(stmt.Returning[2].Expr.is_none());
    for (expr, index, marker) in [
        (&stmt.Lists[0][0], 2, "$2"),
        (stmt.Returning[3].Expr.as_ref().unwrap(), 1, "$1"),
    ] {
        assert_eq!(expr.PostgreSQLParameterIndex(), Some(index));
        assert!(matches!(expr.Kind, ExprKind::ParamMarker { Offset }
            if Offset == sql.find(marker).unwrap()));
        assert_eq!(expr.OriginTextPosition as usize, sql.find(marker).unwrap());
    }
    for (field, original) in
        stmt.Returning
            .iter()
            .zip(["id", "id+1 AS \"Next_ID\"", "t.*", "$1 AS \"P\""])
    {
        assert_eq!(
            field.OriginalText, original,
            "projection source lost: {field:?}"
        );
        assert_eq!(
            &sql[field.Offset..field.Offset + field.OriginalText.len()],
            original
        );
        // Byte offsets remain losslessly convertible to original character spans.
        assert!(sql.is_char_boundary(field.Offset));
        assert_eq!(sql[..field.Offset].chars().count() + 1, {
            let list_start = sql.find("RETURNING ").unwrap() + "RETURNING ".len();
            let expected_start = list_start + sql[list_start..].find(original).unwrap();
            sql[..expected_start].chars().count() + 1
        });
    }
    let plain = insert(
        "INSERT INTO public.t(id) VALUES ($1)",
        SqlDialect::PostgreSQL,
    );
    assert!(plain.Returning.is_empty());
    let protected = insert(
        "INSERT INTO public.t(id) VALUES ('RETURNING') /* RETURNING $2 */",
        SqlDialect::PostgreSQL,
    );
    assert!(protected.Returning.is_empty());
    assert!(matches!(&protected.Lists[0][0].Kind, ExprKind::Value(v) if v.text() == "RETURNING"));
    let null = insert(
        "INSERT INTO public.t(id) VALUES (NULL) RETURNING *",
        SqlDialect::PostgreSQL,
    );
    assert!(
        matches!(&null.Lists[0][0].Kind, ExprKind::Value(v) if v.Datum == parser_ast::ValueDatum::Null)
    );
    assert_eq!(null.Returning.len(), 1);
    assert_eq!(null.Returning[0].OriginalText, "*");
    assert!(
        null.Returning[0]
            .WildCard
            .as_ref()
            .unwrap()
            .Table
            .O
            .is_empty()
    );
    let bare = insert(
        "INSERT INTO public.t(id) VALUES (1) RETURNING id label",
        SqlDialect::PostgreSQL,
    );
    assert_eq!(bare.Returning[0].AsName.O, "label");
    for sql in [
        "INSERT INTO public.t(id) VALUES (1) RETURNING",
        "/*中*/ INSERT INTO public.t(id) VALUES (1) RETURNING id,",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err());
        assert_eq!(
            parser.PostgreSQLEofErrorPosition(),
            Some(sql.chars().count() + 1)
        );
        assert!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
        assert_eq!(parser.PostgreSQLEofErrorPosition(), None);
    }
    // Freeze the pre-existing shared MySQL RETURNING extension, rather than
    // silently deleting it to imitate the official MySQL grammar.
    let mysql = insert(
        "INSERT INTO t(id) VALUES (?) RETURNING id, t.*",
        SqlDialect::MySQL,
    );
    assert_eq!(mysql.Returning.len(), 2);
    assert_eq!(mysql.Returning[1].WildCard.as_ref().unwrap().Table.O, "t");
    println!(
        "pg_dialect_04_behavior: 7 PG inputs, 1 MySQL extension baseline; projection tree/source/parameter assertions"
    );
}
