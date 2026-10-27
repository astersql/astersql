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
        "SELECT CAST(1 AS int2)",
        "SELECT 1::text",
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

#[test]
fn pg_dialect_05_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    fn update(sql: &str, dialect: SqlDialect) -> parser_ast::UpdateStmt {
        let node = parser::Parser::with_dialect(dialect)
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        *node
            .into_any()
            .downcast::<parser_ast::UpdateStmt>()
            .ok()
            .expect("UPDATE AST")
    }

    let sql = "/*中*/ UPDATE public.t AS a SET id=$1 WHERE id=1 RETURNING a.id AS \"ID\", a.*";
    let stmt = update(sql, SqlDialect::PostgreSQL);
    assert_eq!(stmt.List.len(), 1);
    assert_eq!(stmt.List[0].Column.Name.O, "id");
    assert_eq!(stmt.List[0].Expr.PostgreSQLParameterIndex(), Some(1));
    assert_eq!(stmt.Returning.len(), 2);
    assert!(matches!(&stmt.Returning[0].Expr.as_ref().unwrap().Kind,
        ExprKind::Column(column) if column.Table.O == "a" && column.Name.O == "id"));
    assert_eq!(stmt.Returning[0].AsName.O, "ID");
    assert_eq!(stmt.Returning[0].OriginalText, "a.id AS \"ID\"");
    assert_eq!(stmt.Returning[1].OriginalText, "a.*");
    for field in &stmt.Returning {
        assert_eq!(
            &sql[field.Offset..field.Offset + field.OriginalText.len()],
            field.OriginalText
        );
        assert!(sql.is_char_boundary(field.Offset));
    }
    let zero_rows = update(
        "UPDATE public.t AS a SET id=$1 WHERE FALSE RETURNING a.id",
        SqlDialect::PostgreSQL,
    );
    assert!(matches!(
        zero_rows.Where.as_ref().unwrap().Kind,
        ExprKind::Value(_)
    ));
    assert_eq!(zero_rows.Returning[0].OriginalText, "a.id");

    // PostgreSQL UPDATE FROM retains both the FROM source and the ordered projection.
    let from_sql = "UPDATE public.t AS a SET id=$1 FROM public.s AS b WHERE a.id=b.id RETURNING b.id AS source_id, a.*";
    let from = update(from_sql, SqlDialect::PostgreSQL);
    assert!(matches!(
        from.From.as_ref().unwrap().TableRefs.Left.as_deref(),
        Some(parser_ast::ResultSetNode::TableSource(source)) if source.AsName.O == "b"
    ));
    assert!(matches!(
        from.TableRefs.as_ref().unwrap().TableRefs.Left.as_deref(),
        Some(parser_ast::ResultSetNode::TableSource(source)) if source.AsName.O == "a"
    ));
    assert!(matches!(
        from.Where.as_ref().unwrap().Kind,
        ExprKind::Binary { .. }
    ));
    assert_eq!(from.Returning.len(), 2);
    assert_eq!(from.Returning[0].AsName.O, "source_id");
    assert_eq!(from.Returning[0].OriginalText, "b.id AS source_id");
    assert_eq!(from.Returning[1].OriginalText, "a.*");
    let mut mysql = parser::Parser::with_dialect(SqlDialect::MySQL);
    assert!(mysql.ParseOneStmt(from_sql, "", "").is_err());

    for invalid in [
        "UPDATE public.t SET id=1 RETURNING",
        "/*中*/ UPDATE public.t SET id=1 RETURNING id,",
        "UPDATE public.t SET id=1 RETURNING id WHERE id=1",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(invalid, "", "").is_err(), "{invalid}");
        assert!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }

    let misplaced = "/*中*/ UPDATE public.t SET id=1 RETURNING id WHERE id=1";
    let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    assert!(parser.ParseOneStmt(misplaced, "", "").is_err());
    assert_eq!(
        parser.PostgreSQLSyntaxErrorPosition(),
        Some(
            misplaced[..misplaced.find("WHERE").unwrap()]
                .chars()
                .count()
                + 1
        )
    );

    // Preserve the pre-existing shared MySQL UPDATE RETURNING extension.
    let mysql_update = update("UPDATE t SET id=1 RETURNING id", SqlDialect::MySQL);
    assert_eq!(mysql_update.Returning.len(), 1);
    assert_eq!(mysql_update.Returning[0].OriginalText, "id");
    println!(
        "pg_dialect_05_behavior: base, zero-row, UPDATE FROM, malformed clause, spans, aliases, parameters and MySQL baseline"
    );
}

#[test]
fn pg_dialect_06_behavior() {
    use parser_ast::{ExprKind, SqlDialect};

    fn delete(sql: &str, dialect: SqlDialect) -> parser_ast::DeleteStmt {
        let node = parser::Parser::with_dialect(dialect)
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        *node
            .into_any()
            .downcast::<parser_ast::DeleteStmt>()
            .ok()
            .expect("DELETE AST")
    }

    let sql = "/*中*/ DELETE FROM public.t AS a WHERE id=$1 RETURNING a.id, a.*";
    let statement = delete(sql, SqlDialect::PostgreSQL);
    assert_eq!(statement.Returning.len(), 2);
    assert!(matches!(
        &statement.Returning[0].Expr.as_ref().unwrap().Kind,
        ExprKind::Column(column) if column.Table.O == "a" && column.Name.O == "id"
    ));
    assert_eq!(
        statement.Returning[1].WildCard.as_ref().unwrap().Table.O,
        "a"
    );
    let parameter = statement.Where.as_ref().unwrap();
    let ExprKind::Binary { Op, L, R } = &parameter.Kind else {
        panic!("DELETE predicate should be an equality")
    };
    assert_eq!(Op, "=");
    assert!(matches!(&L.Kind, ExprKind::Column(column) if column.Name.O == "id"));
    assert!(matches!(R.Kind, ExprKind::ParamMarker { Offset }
        if Offset == sql.find("$1").unwrap()));
    assert_eq!(R.PostgreSQLParameterIndex(), Some(1));
    assert_eq!(R.OriginTextPosition as usize, sql.find("$1").unwrap());
    for (field, original) in statement.Returning.iter().zip(["a.id", "a.*"]) {
        assert_eq!(field.OriginalText, original);
        assert_eq!(
            &sql[field.Offset..field.Offset + field.OriginalText.len()],
            original
        );
        assert!(sql.is_char_boundary(field.Offset));
    }

    let using_sql = "DELETE FROM public.t AS a USING public.s AS b WHERE a.id=b.id RETURNING a.id, b.id AS source_id";
    let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    let using = pg
        .ParseOneStmt(using_sql, "", "")
        .unwrap_or_else(|error| panic!("PostgreSQL DELETE USING: {error}"));
    let using = *using
        .into_any()
        .downcast::<parser_ast::DeleteStmt>()
        .ok()
        .expect("DELETE USING AST");
    let target = using.TableRefs.as_ref().unwrap().TableRefs.Left.as_deref();
    let Some(parser_ast::ResultSetNode::TableSource(target)) = target else {
        panic!("DELETE target must remain separate from USING sources");
    };
    assert_eq!(target.Source.Name.O, "t");
    assert_eq!(target.Source.Schema.O, "public");
    assert_eq!(target.AsName.O, "a");
    let source = using.Using.as_ref().unwrap().TableRefs.Left.as_deref();
    let Some(parser_ast::ResultSetNode::TableSource(source)) = source else {
        panic!("DELETE USING source must be retained independently");
    };
    assert_eq!(source.Source.Name.O, "s");
    assert_eq!(source.Source.Schema.O, "public");
    assert_eq!(source.AsName.O, "b");
    assert_eq!(using.UsingOffset, Some(using_sql.find("USING").unwrap()));
    assert!(matches!(
        &using.Where.as_ref().unwrap().Kind,
        ExprKind::Binary { L, R, .. }
            if matches!(&L.Kind, ExprKind::Column(column) if column.Table.O == "a" && column.Name.O == "id")
                && matches!(&R.Kind, ExprKind::Column(column) if column.Table.O == "b" && column.Name.O == "id")
    ));
    assert_eq!(using.Returning.len(), 2);
    assert!(matches!(
        &using.Returning[1].Expr.as_ref().unwrap().Kind,
        ExprKind::Column(column) if column.Table.O == "b" && column.Name.O == "id"
    ));
    assert_eq!(using.Returning[1].AsName.O, "source_id");
    assert!(
        parser::Parser::with_dialect(SqlDialect::MySQL)
            .ParseOneStmt(using_sql, "", "")
            .is_err()
    );

    let unaliased_using = delete(
        "DELETE FROM public.t USING public.s WHERE public.t.id=public.s.id RETURNING public.t.id, public.s.id AS source_id",
        SqlDialect::PostgreSQL,
    );
    assert!(unaliased_using.Using.is_some());
    assert!(unaliased_using.Returning.len() == 2);

    let null_projection = delete(
        "DELETE FROM public.t WHERE id IS NULL RETURNING *",
        SqlDialect::PostgreSQL,
    );
    assert_eq!(null_projection.Returning.len(), 1);
    assert!(null_projection.Returning[0].WildCard.is_some());

    let protected = delete(
        "DELETE FROM public.t WHERE id=1 /* RETURNING $2 */",
        SqlDialect::PostgreSQL,
    );
    assert!(protected.Returning.is_empty());

    for (invalid, expected_position) in [
        (
            "DELETE FROM public.t WHERE id=1 RETURNING",
            "DELETE FROM public.t WHERE id=1 RETURNING".chars().count() + 1,
        ),
        (
            "DELETE FROM public.t RETURNING id WHERE id=1",
            "DELETE FROM public.t RETURNING id ".chars().count() + 1,
        ),
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(invalid, "", "").is_err(), "{invalid}");
        assert_eq!(
            parser.PostgreSQLSyntaxErrorPosition(),
            Some(expected_position)
        );
        assert!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }

    // Preserve the pre-existing MySQL DELETE RETURNING extension.
    let mysql = delete("DELETE FROM t WHERE id=1 RETURNING t.*", SqlDialect::MySQL);
    assert_eq!(mysql.Returning.len(), 1);
    assert_eq!(mysql.Returning[0].WildCard.as_ref().unwrap().Table.O, "t");
    let mysql_using = delete("DELETE FROM t USING s WHERE t.id=s.id", SqlDialect::MySQL);
    assert!(mysql_using.IsMultiTable);
    assert!(mysql_using.Using.is_none());
    assert_eq!(mysql_using.Tables.len(), 1);
    assert_eq!(mysql_using.Tables[0].Name.O, "t");
    println!(
        "pg_dialect_06_behavior: projection/span, parameter, USING, NULL, malformed SQL, lexical guard and MySQL baseline"
    );
}

#[test]
fn pg_dialect_07_behavior() {
    use parser_ast::{
        ExprKind, Node, PostgreSQLConflictAction, PostgreSQLConflictTarget, SqlDialect,
    };

    fn insert(sql: &str, dialect: SqlDialect) -> parser_ast::InsertStmt {
        let statement = parser::Parser::with_dialect(dialect)
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        *statement
            .into_any()
            .downcast::<parser_ast::InsertStmt>()
            .ok()
            .expect("INSERT AST")
    }

    let mysql = insert(
        "INSERT INTO t(conflict, nothing) VALUES (1, 2) ON DUPLICATE KEY UPDATE conflict=VALUES(conflict)",
        SqlDialect::MySQL,
    );
    assert_eq!(mysql.OnDuplicate.len(), 1);
    assert!(mysql.PostgreSQLConflict.is_none());

    let omitted_sql =
        "/*中*/ INSERT INTO public.t(id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING id";
    let omitted = insert(omitted_sql, SqlDialect::PostgreSQL);
    let clause = omitted
        .PostgreSQLConflict
        .as_ref()
        .expect("conflict clause");
    assert_eq!(clause.Target, PostgreSQLConflictTarget::Any);
    assert_eq!(clause.Action, PostgreSQLConflictAction::DoNothing);
    assert!(clause.Columns.is_empty());
    assert!(clause.Constraint.is_none());
    assert!(clause.Predicate.is_none());
    let start = omitted_sql.find("ON CONFLICT").unwrap();
    let end = omitted_sql.find(" RETURNING").unwrap();
    assert_eq!(clause.SourceSpan, (start, end));
    assert_eq!(clause.OriginalText, &omitted_sql[start..end]);
    assert_eq!(omitted.Lists[0][0].PostgreSQLParameterIndex(), Some(1));
    assert_eq!(omitted.Returning.len(), 1);

    let inference_sql =
        "INSERT INTO public.t(id) VALUES (2) ON CONFLICT (id) WHERE id > 0 DO NOTHING";
    let inference = insert(inference_sql, SqlDialect::PostgreSQL);
    let clause = inference
        .PostgreSQLConflict
        .as_ref()
        .expect("conflict clause");
    assert_eq!(clause.Target, PostgreSQLConflictTarget::Inference);
    assert_eq!(clause.Columns.len(), 1);
    assert_eq!(clause.Columns[0].Name.O, "id");
    assert_eq!(clause.Columns[0].PostgreSQLParts[0].Original, "id");
    let predicate = clause.Predicate.as_ref().expect("inference predicate");
    assert!(matches!(&predicate.Kind, ExprKind::Binary { Op, L, R }
        if Op == ">"
            && matches!(&L.Kind, ExprKind::Column(column) if column.Name.O == "id")
            && matches!(&R.Kind, ExprKind::Value(value) if value.text() == "0")));
    let start = inference_sql.find("ON CONFLICT").unwrap();
    let end = inference_sql.find("DO NOTHING").unwrap() + "DO NOTHING".len();
    assert_eq!(clause.SourceSpan, (start, end));
    assert_eq!(clause.OriginalText, &inference_sql[start..end]);

    struct ConflictWalk {
        columns: usize,
        predicates: usize,
    }
    impl parser_ast::Visitor for ConflictWalk {
        fn enter(&mut self, node: &dyn parser_ast::Node) -> bool {
            if let Some(expression) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                self.predicates += usize::from(matches!(expression.Kind, ExprKind::Binary { .. }));
            }
            false
        }
        fn leave(&mut self, _: &dyn parser_ast::Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, _: &parser_ast::ColumnName) -> bool {
            self.columns += 1;
            false
        }
    }
    let mut walk = ConflictWalk {
        columns: 0,
        predicates: 0,
    };
    inference.accept(&mut walk);
    assert!(
        walk.columns >= 2,
        "INSERT and conflict target columns must be visited"
    );
    assert_eq!(
        walk.predicates, 1,
        "conflict target predicate must be visited"
    );

    let constraint_sql =
        "INSERT INTO public.t(id) VALUES (3) ON CONFLICT ON CONSTRAINT \"t_pkey\" DO NOTHING";
    let constraint = insert(constraint_sql, SqlDialect::PostgreSQL);
    let clause = constraint
        .PostgreSQLConflict
        .as_ref()
        .expect("conflict clause");
    assert_eq!(clause.Target, PostgreSQLConflictTarget::Constraint);
    let name = clause.Constraint.as_ref().expect("constraint identifier");
    assert_eq!(name.Original, "\"t_pkey\"");
    assert_eq!(name.Decoded, "t_pkey");
    assert!(name.Quoted);
    let start = constraint_sql.find("ON CONFLICT").unwrap();
    let end = constraint_sql.find("DO NOTHING").unwrap() + "DO NOTHING".len();
    assert_eq!(clause.SourceSpan, (start, end));
    assert_eq!(clause.OriginalText, &constraint_sql[start..end]);

    for (sql, position) in [
        (
            "/*中*/ INSERT INTO public.t(id) VALUES (4) ON CONFLICT",
            "/*中*/ INSERT INTO public.t(id) VALUES (4) ON CONFLICT"
                .chars()
                .count()
                + 1,
        ),
        (
            "INSERT INTO public.t(id) VALUES (5) ON CONFLICT () DO NOTHING",
            "INSERT INTO public.t(id) VALUES (5) ON CONFLICT () DO NOTHING"
                .find(") DO")
                .unwrap()
                + 1,
        ),
        (
            "INSERT INTO public.t(id) VALUES (6) ON CONFLICT (id) DO",
            "INSERT INTO public.t(id) VALUES (6) ON CONFLICT (id) DO"
                .chars()
                .count()
                + 1,
        ),
    ] {
        let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(pg.ParseOneStmt(sql, "", "").is_err(), "{sql}");
        assert_eq!(pg.PostgreSQLSyntaxErrorPosition(), Some(position), "{sql}");
        assert!(pg.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
        assert_eq!(pg.PostgreSQLSyntaxErrorPosition(), None);
    }

    for (sql, keyword) in [
        (
            "INSERT INTO public.t(id) VALUES (7) ON \"CONFLICT\" DO NOTHING",
            "\"CONFLICT\"",
        ),
        (
            "INSERT INTO public.t(id) VALUES (8) ON CONFLICT DO \"NOTHING\"",
            "\"NOTHING\"",
        ),
    ] {
        let mut pg = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(
            pg.ParseOneStmt(sql, "", "").is_err(),
            "quoted identifier must not be treated as a grammar keyword: {sql}"
        );
        assert_eq!(
            pg.PostgreSQLSyntaxErrorPosition(),
            Some(sql.find(keyword).unwrap() + 1),
            "quoted grammar keyword error position: {sql}"
        );
    }
    let protected = insert(
        "INSERT INTO public.t(id) VALUES ('ON CONFLICT DO NOTHING') /* ON CONFLICT DO NOTHING */",
        SqlDialect::PostgreSQL,
    );
    assert!(protected.PostgreSQLConflict.is_none());
    assert!(matches!(&protected.Lists[0][0].Kind, ExprKind::Value(value)
        if value.text() == "ON CONFLICT DO NOTHING"));

    println!(
        "pg_dialect_07_behavior: three targets, predicate visitor/source spans, syntax positions, quoted keywords and MySQL duplicate baseline"
    );
}

#[test]
fn pg_dialect_08_behavior() {
    use parser_ast::{Node, PostgreSQLConflictAction, PostgreSQLConflictTarget, SqlDialect};

    let sql = "/*中*/ INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT (id) WHERE id > 0 DO UPDATE SET v=excluded.v, id=$1 WHERE t.v IS NOT NULL RETURNING id";
    let statement = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    let insert = statement
        .into_any()
        .downcast::<parser_ast::InsertStmt>()
        .ok()
        .expect("INSERT AST");
    let conflict = insert.PostgreSQLConflict.as_ref().expect("conflict clause");
    assert_eq!(conflict.Target, PostgreSQLConflictTarget::Inference);
    assert_eq!(conflict.Action, PostgreSQLConflictAction::DoUpdate);
    let target_predicate = conflict.Predicate.as_ref().expect("inference predicate");
    assert!(matches!(
        &target_predicate.Kind,
        parser_ast::ExprKind::Binary { Op, .. } if Op == ">"
    ));
    let target_start = sql.find("WHERE id").expect("target WHERE start");
    let target_end = sql.find(" DO UPDATE").expect("target WHERE end");
    assert_eq!(
        conflict.TargetPredicateSpan,
        Some((target_start, target_end))
    );
    assert_eq!(conflict.UpdateAssignments.len(), 2);
    assert_eq!(conflict.UpdateAssignments[0].Column.Name.O, "v");
    assert!(matches!(
        &conflict.UpdateAssignments[0].Expr.Kind,
        parser_ast::ExprKind::Column(column)
            if column.Table.O.eq_ignore_ascii_case("excluded") && column.Name.O == "v"
    ));
    assert_eq!(conflict.UpdateAssignments[1].Column.Name.O, "id");
    assert_eq!(
        conflict.UpdateAssignments[1]
            .Expr
            .PostgreSQLParameterIndex(),
        Some(1)
    );
    let update_predicate = conflict
        .UpdatePredicate
        .as_ref()
        .expect("DO UPDATE predicate");
    assert!(matches!(
        &update_predicate.Kind,
        parser_ast::ExprKind::IsNull { Expr, Not: true }
            if matches!(&Expr.Kind, parser_ast::ExprKind::Column(column)
                if column.Table.O == "t" && column.Name.O == "v")
    ));
    let update_start = sql.find("WHERE t.v").expect("update WHERE start");
    let update_end = sql.find(" RETURNING").expect("update WHERE end");
    assert_eq!(
        conflict.UpdatePredicateSpan,
        Some((update_start, update_end))
    );
    let start = sql.find("ON CONFLICT").expect("conflict start");
    let end = sql.find(" RETURNING").expect("conflict end");
    assert_eq!(conflict.SourceSpan, (start, end));
    assert_eq!(conflict.OriginalText, &sql[start..end]);
    assert_eq!(insert.Lists[0][0].PostgreSQLParameterIndex(), Some(1));
    assert_eq!(insert.Lists[0][1].PostgreSQLParameterIndex(), Some(2));
    assert_eq!(insert.Returning.len(), 1);

    struct ConflictWalk {
        inference_predicates: usize,
        update_predicates: usize,
        assignment_columns: Vec<String>,
    }
    impl parser_ast::Visitor for ConflictWalk {
        fn enter(&mut self, node: &dyn parser_ast::Node) -> bool {
            if let Some(expression) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                self.inference_predicates += usize::from(matches!(
                    &expression.Kind,
                    parser_ast::ExprKind::Binary { Op, .. } if Op == ">"
                ));
                self.update_predicates += usize::from(matches!(
                    &expression.Kind,
                    parser_ast::ExprKind::IsNull { Not: true, .. }
                ));
            }
            false
        }
        fn leave(&mut self, _: &dyn parser_ast::Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, column: &parser_ast::ColumnName) -> bool {
            if column.Name.O == "v" || column.Name.O == "id" {
                self.assignment_columns.push(column.Name.O.clone());
            }
            false
        }
    }
    let mut walk = ConflictWalk {
        inference_predicates: 0,
        update_predicates: 0,
        assignment_columns: Vec::new(),
    };
    insert.accept(&mut walk);
    assert_eq!(walk.inference_predicates, 1);
    assert_eq!(walk.update_predicates, 1);
    assert!(
        walk.assignment_columns
            .windows(2)
            .any(|pair| pair == ["v", "id"])
    );

    let constraint_sql = "INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT ON CONSTRAINT \"t_pkey\" DO UPDATE SET v=excluded.v WHERE t.v IS NULL RETURNING id";
    let constraint_stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(constraint_sql, "", "")
        .unwrap()
        .into_any()
        .downcast::<parser_ast::InsertStmt>()
        .ok()
        .expect("constraint target INSERT AST");
    let constraint = constraint_stmt.PostgreSQLConflict.as_ref().unwrap();
    assert_eq!(constraint.Target, PostgreSQLConflictTarget::Constraint);
    let name = constraint.Constraint.as_ref().unwrap();
    assert_eq!(name.Original, "\"t_pkey\"");
    assert_eq!(name.Decoded, "t_pkey");
    assert!(name.Quoted);
    assert_eq!(constraint.Action, PostgreSQLConflictAction::DoUpdate);
    assert!(constraint.Predicate.is_none());
    assert!(constraint.TargetPredicateSpan.is_none());
    assert_eq!(constraint.UpdateAssignments.len(), 1);
    assert!(constraint.UpdatePredicate.is_some());
    assert!(constraint.UpdatePredicateSpan.is_some());

    for (sql, token) in [
        (
            "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT DO UPDATE SET v=excluded.v RETURNING id",
            "UPDATE",
        ),
        (
            "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT (id) DO UPDATE WHERE v IS NULL RETURNING id",
            "WHERE",
        ),
        (
            "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT (id) DO UPDATE SET WHERE v IS NULL RETURNING id",
            "WHERE",
        ),
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err(), "{sql}");
        assert_eq!(
            parser.PostgreSQLSyntaxErrorPosition(),
            Some(sql.find(token).unwrap() + 1),
            "PG syntax position for {sql}"
        );
    }
    let quoted_update = "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT (id) DO \"UPDATE\" SET v=excluded.v";
    let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    assert!(parser.ParseOneStmt(quoted_update, "", "").is_err());
}

#[test]
fn pg_dialect_09_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    fn expression(sql: &str) -> parser_ast::ExprNode {
        let result = parser::Parser::with_dialect(SqlDialect::PostgreSQL).ParseOneStmt(sql, "", "");
        assert!(
            result.is_ok(),
            "ARRAY syntax must reach shared AST: {sql}: {}",
            result.err().unwrap()
        );
        let stmt = result
            .unwrap()
            .into_any()
            .downcast::<parser_ast::SelectStmt>()
            .ok()
            .unwrap();
        stmt.Fields.Fields[0].Expr.clone().unwrap()
    }
    for (sql, expected) in [
        ("SELECT ARRAY[1,2]", vec!["1", "2"]),
        ("SELECT ARRAY[]", vec![]),
        ("SELECT ARRAY[NULL,1]", vec!["NULL", "1"]),
        ("SELECT ARRAY[$2,$1,$2]", vec!["$2", "$1", "$2"]),
        (
            "/*中*/ SELECT ARRAY[1 + 2, '中,文', $2]",
            vec!["1 + 2", "'中,文'", "$2"],
        ),
    ] {
        let expr = expression(sql);
        let ExprKind::Array {
            Elements,
            ElementSpans,
            Brackets,
            Commas,
        } = &expr.Kind
        else {
            panic!("independent ARRAY node required");
        };
        assert_eq!(Elements.len(), expected.len());
        assert_eq!(Commas.len(), expected.len().saturating_sub(1));
        assert_eq!(&sql[Brackets.0..Brackets.0 + 1], "[");
        assert_eq!(&sql[Brackets.1..Brackets.1 + 1], "]");
        assert_eq!(
            expr.node_text.Text(),
            &sql[sql.find("ARRAY").unwrap()..Brackets.1 + 1]
        );
        assert_eq!(expr.OriginTextPosition as usize, sql.find("ARRAY").unwrap());
        for ((element, &(start, end)), expected) in Elements.iter().zip(ElementSpans).zip(expected)
        {
            assert_eq!(&sql[start..end], expected);
            assert!(sql.is_char_boundary(start) && sql.is_char_boundary(end));
            assert_eq!(element.OriginTextPosition as usize, start);
            if expected.starts_with('$') {
                assert_eq!(
                    element.PostgreSQLParameterIndex(),
                    Some(expected[1..].parse().unwrap())
                );
                assert!(
                    matches!(element.Kind, ExprKind::ParamMarker { Offset } if Offset == start)
                );
            }
        }
        for &comma in Commas {
            assert_eq!(&sql[comma..comma + 1], ",");
        }
        if sql.contains("NULL") {
            assert!(
                matches!(&Elements[0].Kind, ExprKind::Value(v) if v.Datum == parser_ast::ValueDatum::Null)
            );
        }
        struct Parameters(Vec<usize>);
        impl parser_ast::Visitor for Parameters {
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
        let mut parameters = Parameters(vec![]);
        parser_ast::Node::accept(&expr, &mut parameters);
        if sql == "SELECT ARRAY[$2,$1,$2]" {
            assert_eq!(parameters.0, [2, 1, 2]);
        }
    }
    for sql in [
        "SELECT ARRAY[1,2",
        "/*中*/ SELECT ARRAY[1,2",
        "SELECT ARRAY[,1]",
        "SELECT ARRAY[1,]",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err());
        if !sql.ends_with(']') {
            assert_eq!(
                parser.PostgreSQLEofErrorPosition(),
                Some(sql.chars().count() + 1)
            );
        }
        assert!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }
    let protected = expression("SELECT 'ARRAY[1,2]' /* ARRAY[] $2 */");
    assert!(matches!(&protected.Kind, ExprKind::Value(v) if v.text() == "ARRAY[1,2]"));
    for sql in [
        "SELECT ARRAY[1,2]",
        "SELECT ARRAY[]",
        "SELECT ARRAY[NULL,1]",
        "SELECT ARRAY[$2,$1,$2]",
    ] {
        assert!(
            parser::Parser::with_dialect(SqlDialect::MySQL)
                .ParseOneStmt(sql, "", "")
                .is_err()
        );
    }
    println!(
        "pg_dialect_09_behavior: 10 unique PG inputs; source/tree/parameter/visitor and 4 MySQL isolation checks"
    );
}

#[test]
fn pg_dialect_10_behavior() {
    use parser_ast::{ExprKind, SqlDialect};

    fn expression(sql: &str) -> parser_ast::ExprNode {
        let result = parser::Parser::with_dialect(SqlDialect::PostgreSQL).ParseOneStmt(sql, "", "");
        assert!(
            result.is_ok(),
            "nested ARRAY syntax must reach the shared AST: {sql}: {}",
            result.err().unwrap()
        );
        result
            .unwrap()
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap()
            .Fields
            .Fields[0]
            .Expr
            .clone()
            .unwrap()
    }

    fn array(
        expr: &parser_ast::ExprNode,
    ) -> (
        &[parser_ast::ExprNode],
        &[(usize, usize)],
        (usize, usize),
        &[usize],
    ) {
        let ExprKind::Array {
            Elements,
            ElementSpans,
            Brackets,
            Commas,
        } = &expr.Kind
        else {
            panic!("nested constructor must remain an ARRAY node");
        };
        (Elements, ElementSpans, *Brackets, Commas)
    }

    fn shape(expr: &parser_ast::ExprNode) -> Vec<usize> {
        let (elements, _, _, _) = array(expr);
        let mut result = vec![elements.len()];
        if let Some(first) = elements.first() {
            if matches!(first.Kind, ExprKind::Array { .. }) {
                let child = shape(first);
                assert!(elements.iter().all(|element| {
                    matches!(element.Kind, ExprKind::Array { .. }) && shape(element) == child
                }));
                result.extend(child);
            }
        }
        result
    }

    let assertions = std::cell::Cell::new(0usize);
    macro_rules! check {
        ($condition:expr $(, $args:tt)*) => {{
            assertions.set(assertions.get() + 1);
            assert!($condition $(, $args)*);
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(, $args:tt)*) => {{
            assertions.set(assertions.get() + 1);
            assert_eq!($left, $right $(, $args)*);
        }};
    }

    for (sql, expected_elements) in [
        (
            "/*中*/ SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]",
            ["ARRAY[1,2]", "ARRAY[3,4]"],
        ),
        ("/*中*/ SELECT ARRAY[[1,2],[3,4]]", ["[1,2]", "[3,4]"]),
    ] {
        let expr = expression(sql);
        let (elements, spans, brackets, commas) = array(&expr);
        check_eq!(elements.len(), 2);
        check_eq!(commas.len(), 1);
        check_eq!(shape(&expr), [2, 2]);
        check_eq!(expr.OriginTextPosition as usize, sql.find("ARRAY").unwrap());
        check_eq!(
            expr.node_text.Text(),
            &sql[sql.find("ARRAY").unwrap()..brackets.1 + 1]
        );
        check_eq!(&sql[brackets.0..brackets.0 + 1], "[");
        check_eq!(&sql[brackets.1..brackets.1 + 1], "]");
        check_eq!(&sql[commas[0]..commas[0] + 1], ",");
        for ((element, &(start, end)), expected) in
            elements.iter().zip(spans).zip(expected_elements)
        {
            check_eq!(&sql[start..end], expected);
            check!(sql.is_char_boundary(start) && sql.is_char_boundary(end));
            check_eq!(element.OriginTextPosition as usize, start);
            check_eq!(element.node_text.Text(), expected);
            let (inner, inner_spans, inner_brackets, inner_commas) = array(element);
            check_eq!(inner.len(), 2);
            check_eq!(inner_commas.len(), 1);
            check_eq!(&sql[inner_brackets.0..inner_brackets.0 + 1], "[");
            check_eq!(&sql[inner_brackets.1..inner_brackets.1 + 1], "]");
            let inner_source = expected
                .strip_prefix("ARRAY")
                .unwrap_or(expected)
                .trim_start_matches('[')
                .trim_end_matches(']');
            for ((start, end), value) in inner_spans.iter().zip(inner_source.split(',')) {
                check_eq!(&sql[*start..*end], value);
            }
        }
    }

    let ragged_sql = "SELECT ARRAY[[1,2],[3]]";
    let ragged = expression(ragged_sql);
    let (outer, outer_spans, _, _) = array(&ragged);
    check_eq!(outer.len(), 2);
    check_eq!(shape(&outer[0]), [2]);
    check_eq!(shape(&outer[1]), [1]);
    check_eq!(&ragged_sql[outer_spans[0].0..outer_spans[0].1], "[1,2]");
    check_eq!(&ragged_sql[outer_spans[1].0..outer_spans[1].1], "[3]");
    check!(!ragged_sql.contains("NULL"));

    let parameters_sql = "SELECT ARRAY[ARRAY[$2,$1],ARRAY[$1,$2]]";
    let parameters = expression(parameters_sql);
    struct ParameterIndexes(Vec<usize>);
    impl parser_ast::Visitor for ParameterIndexes {
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
    let mut indexes = ParameterIndexes(vec![]);
    parser_ast::Node::accept(&parameters, &mut indexes);
    check_eq!(indexes.0, [2, 1, 1, 2]);
    let (outer, spans, _, _) = array(&parameters);
    for ((element, &(start, end)), expected) in outer
        .iter()
        .zip(spans)
        .zip(["ARRAY[$2,$1]", "ARRAY[$1,$2]"])
    {
        check_eq!(&parameters_sql[start..end], expected);
        check_eq!(element.node_text.Text(), expected);
    }

    let nulls = expression("SELECT ARRAY[[NULL,1],[2,NULL]]");
    let (rows, _, _, _) = array(&nulls);
    check_eq!(rows.len(), 2);
    for row in rows {
        let (elements, _, _, _) = array(row);
        check_eq!(elements.len(), 2);
    }
    check!(matches!(
        array(&rows[0]).0[0].Kind,
        ExprKind::Value(ref value) if value.Datum == parser_ast::ValueDatum::Null
    ));
    check!(matches!(
        array(&rows[1]).0[1].Kind,
        ExprKind::Value(ref value) if value.Datum == parser_ast::ValueDatum::Null
    ));

    let numeric = expression("SELECT ARRAY[1,2.5]");
    let (elements, _, _, _) = array(&numeric);
    check_eq!(elements.len(), 2);
    check!(
        matches!(elements[0].Kind, ExprKind::Value(ref value) if value.Datum == parser_ast::ValueDatum::Int64(1))
    );
    check!(
        matches!(elements[1].Kind, ExprKind::Value(ref value) if matches!(value.Datum, parser_ast::ValueDatum::Decimal(_)))
    );

    let mixed_types = expression("SELECT ARRAY[1,true]");
    let (elements, spans, _, _) = array(&mixed_types);
    check_eq!(elements.len(), 2);
    check_eq!(
        &mixed_types.node_text.Text()[spans[0].0 - mixed_types.OriginTextPosition as usize
            ..spans[1].1 - mixed_types.OriginTextPosition as usize],
        "1,true"
    );

    for sql in [
        "/*中*/ SELECT ARRAY[[1,2],[3,4]",
        "/*中*/ SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        check!(parser.ParseOneStmt(sql, "", "").is_err());
        check_eq!(
            parser.PostgreSQLEofErrorPosition(),
            Some(sql.chars().count() + 1)
        );
        check!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }

    let protected = expression("SELECT 'ARRAY[[1,2],[3,4]]' /* ARRAY[] $2 */");
    check!(matches!(
        protected.Kind,
        ExprKind::Value(ref value) if value.text() == "ARRAY[[1,2],[3,4]]"
    ));
    for sql in [
        "SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]",
        "SELECT ARRAY[[1,2],[3,4]]",
    ] {
        check!(
            parser::Parser::with_dialect(SqlDialect::MySQL)
                .ParseOneStmt(sql, "", "")
                .is_err(),
            "PostgreSQL nested ARRAY leaked into MySQL: {sql}"
        );
    }
    println!(
        "pg_dialect_10_behavior: 10 unique nested/type/parameter/error inputs; {} tree, source, NULL, recovery and dialect assertions",
        assertions.get()
    );
}

#[test]
fn pg_dialect_11_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    fn expression(sql: &str) -> parser_ast::ExprNode {
        let result = parser::Parser::with_dialect(SqlDialect::PostgreSQL).ParseOneStmt(sql, "", "");
        assert!(
            result.is_ok(),
            "ROW syntax must reach shared AST: {sql}: {}",
            result.err().unwrap()
        );
        result
            .unwrap()
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap()
            .Fields
            .Fields[0]
            .Expr
            .clone()
            .unwrap()
    }
    let assertions = std::cell::Cell::new(0usize);
    macro_rules! check {
        ($($args:tt)*) => {{ assertions.set(assertions.get()+1); assert!($($args)*); }};
    }
    macro_rules! check_eq {
        ($($args:tt)*) => {{ assertions.set(assertions.get()+1); assert_eq!($($args)*); }};
    }
    for (sql, expected) in [
        ("SELECT ROW(1)", vec!["1"]),
        ("SELECT ROW(1,2)", vec!["1", "2"]),
        ("SELECT ROW(1,ROW(2,3))", vec!["1", "ROW(2,3)"]),
        ("SELECT ROW(NULL,$2,$1,$2)", vec!["NULL", "$2", "$1", "$2"]),
        (
            "/*中*/ SELECT ROW(1 + 2, '中,文', $2)",
            vec!["1 + 2", "'中,文'", "$2"],
        ),
    ] {
        let expr = expression(sql);
        let ExprKind::ExplicitRow {
            Fields: fields,
            FieldSpans,
            Parentheses,
            Commas,
        } = &expr.Kind
        else {
            panic!("ROW node required")
        };
        check_eq!(fields.len(), expected.len());
        check_eq!(expr.node_text.Text(), &sql[sql.find("ROW").unwrap()..]);
        check_eq!(expr.OriginTextPosition as usize, sql.find("ROW").unwrap());
        check_eq!(FieldSpans.len(), fields.len());
        check_eq!(Commas.len(), fields.len() - 1);
        check_eq!(&sql[Parentheses.0..Parentheses.0 + 1], "(");
        check_eq!(&sql[Parentheses.1..Parentheses.1 + 1], ")");
        for ((field, &(start, end)), spelling) in fields.iter().zip(FieldSpans).zip(&expected) {
            check_eq!(&sql[start..end], *spelling);
            check!(sql.is_char_boundary(start) && sql.is_char_boundary(end));
            check_eq!(field.OriginTextPosition as usize, start);
            if spelling.starts_with('$') {
                check!(matches!(field.Kind, ExprKind::ParamMarker { Offset } if Offset==start));
            }
        }
        for &comma in Commas {
            check_eq!(&sql[comma..comma + 1], ",");
        }
        struct Parameters(Vec<u16>);
        impl parser_ast::Visitor for Parameters {
            fn enter(&mut self, node: &dyn parser_ast::Node) -> bool {
                if let Some(expr) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                    if let Some(index) = expr.PostgreSQLParameterIndex() {
                        self.0.push(index);
                    }
                }
                false
            }
            fn leave(&mut self, _: &dyn parser_ast::Node) -> bool {
                true
            }
        }
        let mut parameters = Parameters(vec![]);
        parser_ast::Node::accept(&expr, &mut parameters);
        if sql.contains("NULL") {
            check_eq!(parameters.0, [2, 1, 2]);
        }
        for (field, expected) in fields.iter().zip(&expected) {
            if let Some(index) = expected.strip_prefix('$') {
                check_eq!(
                    field.PostgreSQLParameterIndex(),
                    Some(index.parse().unwrap())
                );
            }
        }
        if sql.contains("NULL") {
            check!(
                matches!(&fields[0].Kind, ExprKind::Value(v) if v.Datum == parser_ast::ValueDatum::Null)
            );
        }
        if sql.contains("ROW(2,3)") {
            let ExprKind::ExplicitRow {
                Fields: inner,
                FieldSpans: spans,
                Parentheses: parens,
                Commas: commas,
            } = &fields[1].Kind
            else {
                panic!("nested explicit ROW lost")
            };
            check_eq!(inner.len(), 2);
            check_eq!(&sql[spans[0].0..spans[0].1], "2");
            check_eq!(&sql[spans[1].0..spans[1].1], "3");
            check_eq!(&sql[parens.0..parens.0 + 1], "(");
            check_eq!(&sql[commas[0]..commas[0] + 1], ",");
        }
    }
    for sql in ["SELECT ROW(1,", "/*中*/ SELECT ROW(1,", "SELECT ROW()"] {
        let mut p = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        check!(p.ParseOneStmt(sql, "", "").is_err());
        if sql.ends_with(',') {
            check_eq!(
                p.PostgreSQLEofErrorPosition(),
                Some(sql.chars().count() + 1)
            );
        }
        check!(p.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }
    let protected = expression("SELECT 'ROW(1,2)' /* ROW($2) */");
    check!(matches!(&protected.Kind, ExprKind::Value(v) if v.text()=="ROW(1,2)"));
    let grouped = expression("SELECT (1)");
    check!(matches!(grouped.Kind, ExprKind::Parentheses(_)));
    let mysql = parser::Parser::with_dialect(SqlDialect::MySQL)
        .ParseOneStmt("SELECT ROW(1,2)", "", "")
        .unwrap();
    let mysql = mysql
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap()
        .Fields
        .Fields[0]
        .Expr
        .as_ref()
        .unwrap();
    check!(matches!(&mysql.Kind, ExprKind::Row(fields) if fields.len()==2));
    check!(
        parser::Parser::with_dialect(SqlDialect::MySQL)
            .ParseOneStmt("SELECT ROW(1)", "", "")
            .is_err()
    );
    println!(
        "pg_dialect_11_behavior: assertions={}; 10 unique PG inputs, MySQL multi/single-field baseline",
        assertions.get()
    );
}

#[test]
fn pg_dialect_13_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    let count = std::cell::Cell::new(0usize);
    macro_rules! check { ($($args:tt)*) => {{count.set(count.get()+1); assert!($($args)*);}}; }
    macro_rules! equal { ($($args:tt)*) => {{count.set(count.get()+1); assert_eq!($($args)*);}}; }
    let sql = "/*中*/ SELECT foo, Foo, \"Foo\", \"a\"\"b\", $2 FROM \"public\".\"t\"";
    let statement = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(sql, "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let mut previous = 0;
    for (field, (raw, decoded, lookup, quoted)) in select.Fields.Fields.iter().zip([
        ("foo", "foo", "foo", false),
        ("Foo", "Foo", "foo", false),
        ("\"Foo\"", "Foo", "Foo", true),
        ("\"a\"\"b\"", "a\"b", "a\"b", true),
    ]) {
        let expr = field.Expr.as_ref().unwrap();
        let ExprKind::Column(column) = &expr.Kind else {
            panic!("quoted name must be a column: {expr:?}")
        };
        equal!(
            column.Name.L,
            decoded.to_lowercase(),
            "native CIStr compatibility key"
        );
        equal!(&column.Name.O, decoded);
        equal!(column.PostgreSQLParts.len(), 1);
        let part = &column.PostgreSQLParts[0];
        equal!(&part.Original, raw);
        equal!(&part.Decoded, decoded);
        equal!(&part.Lookup, lookup, "PostgreSQL lookup identity");
        equal!(part.Quoted, quoted);
        equal!(&sql[part.Span.0..part.Span.1], raw);
        check!(part.Span.0 >= previous);
        previous = part.Span.1;
        check!(sql.is_char_boundary(part.Span.0) && sql.is_char_boundary(part.Span.1));
        equal!(expr.OriginTextPosition as usize, part.Span.0);
    }
    equal!(
        select.Fields.Fields[4]
            .Expr
            .as_ref()
            .unwrap()
            .PostgreSQLParameterIndex(),
        Some(2)
    );
    let parser_ast::ResultSetNode::TableSource(source) = select
        .From
        .as_ref()
        .unwrap()
        .TableRefs
        .Left
        .as_deref()
        .unwrap()
    else {
        panic!("table source")
    };
    equal!(source.Source.PostgreSQLParts.len(), 2);
    for (part, raw) in source
        .Source
        .PostgreSQLParts
        .iter()
        .zip(["\"public\"", "\"t\""])
    {
        equal!(&part.Original, raw);
        equal!(&sql[part.Span.0..part.Span.1], raw);
        check!(part.Quoted);
    }
    let qualified = "/*中*/ SELECT public.\"t\".\"a.b\", a.\"x.y\", \"x\\y\"";
    let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(qualified, "", "")
        .unwrap();
    let stmt = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    for (field, expected) in stmt.Fields.Fields.iter().zip([
        vec![("public", false), ("t", true), ("a.b", true)],
        vec![("a", false), ("x.y", true)],
        vec![("x\\y", true)],
    ]) {
        let ExprKind::Column(column) = &field.Expr.as_ref().unwrap().Kind else {
            panic!("column")
        };
        equal!(column.PostgreSQLParts.len(), expected.len());
        for (part, (lookup, quoted)) in column.PostgreSQLParts.iter().zip(expected) {
            equal!(&part.Lookup, lookup);
            equal!(part.Quoted, quoted);
            equal!(&qualified[part.Span.0..part.Span.1], part.Original.as_str());
        }
    }
    for sql in [
        "SELECT \"\"",
        "SELECT \"unterminated",
        "/*中*/ SELECT \"\"",
        "/*中*/ SELECT \"unterminated",
    ] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        check!(parser.ParseOneStmt(sql, "", "").is_err(), "{sql}");
        equal!(
            parser.PostgreSQLSyntaxErrorPosition(),
            Some(sql[..sql.find('"').unwrap()].chars().count() + 1)
        );
    }
    let protected = "SELECT 'Foo \"a\"\"b\"' /* \"Foo\" */ -- \"bad\n";
    let mut lookahead = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    check!(
        lookahead
            .ParseOneStmt("SELECT TO \"broken", "", "")
            .is_err()
    );
    equal!(lookahead.PostgreSQLSyntaxErrorPosition(), Some(8));
    let stmt = parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseOneStmt(protected, "", "")
        .unwrap();
    let stmt = stmt
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    check!(
        matches!(&stmt.Fields.Fields[0].Expr.as_ref().unwrap().Kind,ExprKind::Value(value) if value.as_str()=="Foo \"a\"\"b\"")
    );
    for mode in [parser::mysql::SQLMode(0), parser::mysql::ModeANSIQuotes] {
        let mut parser = parser::New();
        parser.SetSQLMode(mode);
        let stmt = parser.ParseOneStmt("SELECT \"Foo\"", "", "").unwrap();
        let stmt = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = stmt.Fields.Fields[0].Expr.as_ref().unwrap();
        if mode == parser::mysql::ModeANSIQuotes {
            let ExprKind::Column(column) = &expr.Kind else {
                panic!("ANSI_QUOTES column")
            };
            equal!(&column.Name.L, "foo");
            check!(column.PostgreSQLParts.is_empty());
        } else {
            check!(matches!(expr.Kind, ExprKind::Value(_)));
        }
    }
    println!(
        "pg_dialect_13_behavior: assertions={}; 7 primary PG inputs + 1 lookahead guard, MySQL default/ANSI_QUOTES",
        count.get()
    );
}

#[test]
fn pg_dialect_12_behavior() {
    use parser_ast::{ExprKind, SqlDialect};
    let count = std::cell::Cell::new(0usize);
    macro_rules! check {($($arg:tt)*)=>{{count.set(count.get()+1);assert!($($arg)*);}};}
    macro_rules! equal {($($arg:tt)*)=>{{count.set(count.get()+1);assert_eq!($($arg)*);}};}
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
    for (sql, expected) in [
        ("SELECT (1,2)", "(1,2)"),
        ("SELECT (1)", "(1)"),
        ("SELECT (SELECT 1)", "(SELECT 1)"),
    ] {
        let expr = expression(sql);
        equal!(
            parser_ast::sql_restore::restore_expr(&expr).unwrap(),
            expected
        );
        if sql == "SELECT (1)" {
            check!(matches!(expr.Kind, ExprKind::Parentheses(_)));
        }
        if sql == "SELECT (SELECT 1)" {
            check!(matches!(expr.Kind, ExprKind::Subquery { .. }));
        }
    }
    for (sql, spellings) in [
        ("SELECT (1,2)", vec!["1", "2"]),
        ("SELECT (1,(2,3))", vec!["1", "(2,3)"]),
        (
            "/*中*/ SELECT (NULL, $2, $1, $2)",
            vec!["NULL", "$2", "$1", "$2"],
        ),
        (
            "SELECT ('中,文' /* , */, 1 + 2, (SELECT 2))",
            vec!["'中,文'", "1 + 2", "(SELECT 2)"],
        ),
        (
            "SELECT (\"t\".\"id\",(SELECT 2)) FROM public.t t",
            vec!["\"t\".\"id\"", "(SELECT 2)"],
        ),
    ] {
        let expr = expression(sql);
        let ExprKind::ImplicitRow {
            Fields,
            FieldSpans,
            Parentheses,
            Commas,
        } = &expr.Kind
        else {
            panic!("implicit row form lost");
        };
        equal!(Fields.len(), spellings.len());
        equal!(FieldSpans.len(), Fields.len());
        equal!(Commas.len(), Fields.len() - 1);
        equal!(expr.OriginTextPosition as usize, Parentheses.0);
        equal!(&sql[Parentheses.0..Parentheses.0 + 1], "(");
        equal!(&sql[Parentheses.1..Parentheses.1 + 1], ")");
        equal!(
            expr.node_text.Text(),
            &sql[Parentheses.0..Parentheses.1 + 1]
        );
        for ((field, &(start, end)), spelling) in Fields.iter().zip(FieldSpans).zip(spellings) {
            equal!(&sql[start..end], spelling);
            check!(sql.is_char_boundary(start) && sql.is_char_boundary(end));
            equal!(field.OriginTextPosition as usize, start);
            if spelling.starts_with('$') {
                equal!(
                    field.PostgreSQLParameterIndex(),
                    Some(spelling[1..].parse().unwrap())
                );
                check!(matches!(field.Kind,ExprKind::ParamMarker{Offset} if Offset==start));
            }
        }
        for &comma in Commas {
            equal!(&sql[comma..comma + 1], ",");
        }
        if sql.contains("(2,3)") {
            check!(matches!(Fields[1].Kind, ExprKind::ImplicitRow { .. }));
        }
        if sql.contains("NULL") {
            check!(
                matches!(&Fields[0].Kind,ExprKind::Value(v) if v.Datum==parser_ast::ValueDatum::Null)
            );
        }
        struct Parameters(Vec<u16>);
        impl parser_ast::Visitor for Parameters {
            fn enter(&mut self, node: &dyn parser_ast::Node) -> bool {
                if let Some(expr) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                    if let Some(i) = expr.PostgreSQLParameterIndex() {
                        self.0.push(i);
                    }
                }
                false
            }
            fn leave(&mut self, _: &dyn parser_ast::Node) -> bool {
                true
            }
        }
        let mut params = Parameters(vec![]);
        parser_ast::Node::accept(&expr, &mut params);
        equal!(
            params.0,
            if sql.contains("$2") {
                vec![2, 1, 2]
            } else {
                vec![]
            }
        );
    }
    let expr = expression("SELECT ROW(t.id,(SELECT 2)) FROM public.t t");
    let ExprKind::ExplicitRow { Fields, .. } = expr.Kind else {
        panic!("explicit ROW changed");
    };
    check!(matches!(Fields[0].Kind, ExprKind::Column(_)));
    check!(matches!(Fields[1].Kind, ExprKind::Subquery { .. }));
    let protected = expression("SELECT '(1,2)' /* ($2,$1) */");
    check!(matches!(&protected.Kind,ExprKind::Value(v) if v.text()=="(1,2)"));
    for sql in ["SELECT (1,", "/*中*/ SELECT (1,", "SELECT (,1)"] {
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        check!(parser.ParseOneStmt(sql, "", "").is_err());
        if sql.ends_with(',') {
            equal!(
                parser.PostgreSQLEofErrorPosition(),
                Some(sql.chars().count() + 1)
            );
        }
        check!(parser.ParseOneStmt("SELECT 1::int4", "", "").is_ok());
    }
    // A bare relation in parentheses is rejected by PostgreSQL; it must not
    // affect expression row constructors or scalar grouping.
    check!(
        parser::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseOneStmt("SELECT * FROM (public.t) AS t", "", "")
            .is_err()
    );
    let mysql = parser::Parser::with_dialect(SqlDialect::MySQL)
        .ParseOneStmt("SELECT (1,2)", "", "")
        .unwrap();
    check!(
        matches!(&mysql.as_any().downcast_ref::<parser_ast::SelectStmt>().unwrap().Fields.Fields[0].Expr.as_ref().unwrap().Kind,ExprKind::Row(fields) if fields.len()==2)
    );
    println!(
        "pg_dialect_12_behavior: {} assertions, 5 core inputs plus nested/NULL/parameter/source/error/dialect coverage",
        count.get()
    );
}

#[test]
fn pg_dialect_26_behavior() {
    use parser_ast::{ExprKind, Node, SqlDialect, Visitor};
    struct Parameters(Vec<(u16, usize)>);
    impl Visitor for Parameters {
        fn enter(&mut self, node: &dyn Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<parser_ast::ExprNode>() {
                if let Some(index) = expr.PostgreSQLParameterIndex() {
                    let ExprKind::ParamMarker { Offset } = expr.Kind else {
                        panic!("indexed marker tree")
                    };
                    assert_eq!(expr.OriginTextPosition as usize, Offset);
                    self.0.push((index, Offset));
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn Node) -> bool {
            true
        }
    }
    let sqls = [
        "SELECT $2::int4,CAST($1 AS int4),$2::int4",
        "SELECT 'A_b' ILIKE 'a!_b' ESCAPE $2, $1, $2",
        "INSERT INTO public.t(id) VALUES($2) RETURNING $1::int4,$2::int4",
        "UPDATE public.t SET id=$2 WHERE id=$1 RETURNING $2::int4",
        "DELETE FROM public.t WHERE id=$2 RETURNING $1::int4,$2::int4",
        "INSERT INTO public.t(id) VALUES($2) ON CONFLICT(id) WHERE id>$1 DO NOTHING RETURNING $2::int4",
        "INSERT INTO public.t(id) VALUES($2) ON CONFLICT(id) WHERE id>$1 DO UPDATE SET id=$2 WHERE t.id>$1 RETURNING $2::int4",
        "SELECT ARRAY[$2::int4,$1::int4,$2::int4]",
        "SELECT ARRAY[ARRAY[$2::int4,$1::int4],ARRAY[$2::int4,$1::int4]]",
        "SELECT ROW($2::int4,$1::int4,$2::int4)",
        "SELECT ($2::int4,$1::int4,$2::int4)",
        "SELECT ($2::int4),($1::int4),($2::int4)",
        "SELECT ARRAY[$12::int4,$1::int4,$12::int4]",
    ];
    let mut assertions = 0;
    for source in sqls {
        let sql = format!("/*中 $3*/ {source} /* $3 */");
        let mut parser = parser::Parser::with_dialect(SqlDialect::PostgreSQL);
        let stmt = parser.ParseOneStmt(&sql, "", "").unwrap();
        let mut visitor = Parameters(vec![]);
        stmt.accept(&mut visitor);
        let expected = sql
            .match_indices('$')
            .filter_map(|(offset, _)| {
                if offset < "/*中 $3*/ ".len() || offset >= sql.rfind("/*").unwrap() {
                    return None;
                }
                let digits: String = sql[offset + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                Some((digits.parse::<u16>().unwrap(), offset))
            })
            .collect::<Vec<_>>();
        // DML visitors group RETURNING and conflict nodes by ownership;
        // source offsets, rather than traversal order, define occurrence order.
        visitor.0.sort_by_key(|(_, offset)| *offset);
        assert_eq!(visitor.0, expected, "{sql}");
        assertions += visitor.0.len() + 1;
        let protected = "SELECT '$1' /* $3 */";
        let stmt = parser.ParseOneStmt(protected, "", "").unwrap();
        let mut v = Parameters(vec![]);
        stmt.accept(&mut v);
        assert!(v.0.is_empty());
        assertions += 1;
    }
    assert!(
        parser::Parser::with_dialect(SqlDialect::MySQL)
            .ParseOneStmt("SELECT ARRAY[$2,$1,$2]", "", "")
            .is_err()
    );
    println!(
        "pg_dialect_26_behavior: 13 syntax trees; {} identity/span/protection assertions",
        assertions + 1
    );
}

#[test]
fn pg_scalar_01_boolean_ast_identity() {
    let mut pg = parser::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT NULL::boolean",
        "SELECT NULL::bool",
        "SELECT CAST(NULL AS BOOLEAN)",
        "SELECT CAST(NULL AS BOOL)",
    ] {
        let statement = pg.ParseOneStmt(sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expression = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let parser_ast::ExprKind::Cast { Tp, .. } = &expression.Kind else {
            panic!("missing cast");
        };
        assert_ne!(Tp.GetFlag() & parser_mysql::r#type::IsBooleanFlag, 0);
        let restored = parser_ast::sql_restore::restore_expr(expression).unwrap();
        assert_eq!(restored, "CAST(NULL AS BOOLEAN)");
        let again = pg
            .ParseOneStmt(&format!("SELECT {restored}"), "", "")
            .unwrap();
        let select = again
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(
            parser_ast::sql_restore::restore_expr(select.Fields.Fields[0].Expr.as_ref().unwrap())
                .unwrap(),
            restored
        );
        assert!(
            parser::New().ParseOneStmt(sql, "", "").is_err(),
            "PG cast leaked into MySQL: {sql}"
        );
    }
    for dialect in [
        parser_ast::SqlDialect::PostgreSQL,
        parser_ast::SqlDialect::MySQL,
    ] {
        let mut parser = parser::Parser::with_dialect(dialect);
        let statement = parser
            .ParseOneStmt("CREATE TABLE t (a BOOL,b BOOLEAN,c TINYINT)", "", "")
            .unwrap();
        let create = statement
            .as_any()
            .downcast_ref::<parser_ast::CreateTableStmt>()
            .unwrap();
        for (index, column) in create.Cols.iter().enumerate() {
            assert_eq!(column.Tp.GetType(), parser_mysql::r#type::TypeTiny);
            assert_eq!(
                column.Tp.GetFlag() & parser_mysql::r#type::IsBooleanFlag != 0,
                dialect == parser_ast::SqlDialect::PostgreSQL && index < 2
            );
        }
    }
}

#[test]
fn pg_scalar_03_text_cast_target() {
    use parser_ast::{CastFunctionType, ExprKind, SqlDialect};
    for sql in [
        "SELECT TRUE::text",
        "SELECT CAST(FALSE AS text)",
        "SELECT 'yes'::text::boolean",
        "SELECT NULL::text",
    ] {
        let (statements, _) = crate::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
        let select = statements[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert!(matches!(
            select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
            ExprKind::Cast {
                FunctionType: CastFunctionType::Cast,
                ..
            }
        ));
        if !sql.ends_with("boolean") {
            let ExprKind::Cast { Tp, .. } = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
            else {
                unreachable!()
            };
            assert_eq!(Tp.GetType(), parser_mysql::r#type::TypeVarString);
            assert_eq!(Tp.GetCharset(), "utf8mb4");
        }
        assert!(
            crate::Parser::with_dialect(SqlDialect::MySQL)
                .ParseSQL(sql, &[])
                .is_err(),
            "{sql}"
        );
    }
}

#[test]
fn pg_scalar_05_smallint_ast_and_roundtrip() {
    use parser_ast::{CastFunctionType, ExprKind, SqlDialect};
    let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
    for sql in [
        "SELECT 1::smallint",
        "SELECT 1::int2",
        "SELECT CAST(1 AS SMALLINT)",
        "SELECT CAST(1 AS INT2)",
        "SELECT NULL::int2",
    ] {
        let stmt = pg.ParseOneStmt(sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(
            matches!(&expr.Kind,ExprKind::Cast{FunctionType:CastFunctionType::PostgreSQLInt2,Tp,..} if Tp.GetType()==parser_mysql::r#type::TypeShort)
        );
        let restored = parser_ast::sql_restore::restore_expr(expr).unwrap();
        let again = pg
            .ParseOneStmt(&format!("SELECT {restored}"), "", "")
            .unwrap();
        let again = again
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(
            parser_ast::sql_restore::restore_expr(again.Fields.Fields[0].Expr.as_ref().unwrap())
                .unwrap(),
            restored
        );
        assert!(
            crate::Parser::with_dialect(SqlDialect::MySQL)
                .ParseOneStmt(sql, "", "")
                .is_err()
        );
    }
    assert!(
        pg.ParseOneStmt("SELECT CAST(1 AS int2 ARRAY)", "", "")
            .is_err()
    );
}

#[test]
fn pg_finite_numeric_cast_identity() {
    let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT '12.340'::numeric",
        "SELECT CAST(NULL AS numeric)",
        "SELECT '12.340'::decimal",
    ] {
        let stmt = parser.ParseOneStmt(sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(
            matches!(expr.Kind, parser_ast::ExprKind::Cast { FunctionType: parser_ast::CastFunctionType::PostgreSQLNumeric, ref Tp, .. } if Tp.GetDecimal() == -1)
        );
    }
}

#[test]
fn pg_numeric_grammar_mysql_isolation() {
    let mut mysql = parser::New();
    for sql in [
        "SELECT CAST('12.340' AS DECIMAL)",
        "SELECT CAST('12.340' AS NUMERIC)",
        "SELECT CAST('12.340' AS DECIMAL(6,3))",
    ] {
        let stmt = mysql.ParseOneStmt(sql, "", "").unwrap();
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
    assert!(
        mysql
            .ParseOneStmt("SELECT '12.340'::numeric", "", "")
            .is_err()
    );
}

#[test]
fn pg_scalar_09_numeric_modifiers() {
    let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for (target, p, s) in [
        ("numeric(3,1)", 3, 1),
        ("numeric(2,-1)", 2, -1),
        ("numeric(2)", 2, 0),
        ("decimal(2,-2)", 2, -2),
    ] {
        let sql = format!("SELECT '1250'::{target}");
        let stmt = parser.ParseOneStmt(&sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        assert!(
            matches!(&expr.Kind,parser_ast::ExprKind::Cast { Tp,.. } if Tp.GetFlen()==p && Tp.GetDecimal()==s)
        );
        let restored = parser_ast::sql_restore::restore_expr(expr).unwrap();
        assert!(
            restored.ends_with(&format!("NUMERIC({p},{s}))")),
            "{restored}"
        );
    }
}

#[test]
fn pg_scalar_15_negative_scale_declaration() {
    let sql = "CREATE TABLE t (n NUMERIC(2,-3), d DECIMAL(2,-1), p NUMERIC(2), u NUMERIC)";
    let mut pg = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    let statement = pg.ParseOneStmt(sql, "", "").unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<parser_ast::CreateTableStmt>()
        .unwrap();
    assert_eq!(
        (create.Cols[0].Tp.GetFlen(), create.Cols[0].Tp.GetDecimal()),
        (2, -3)
    );
    assert_eq!(
        (create.Cols[1].Tp.GetFlen(), create.Cols[1].Tp.GetDecimal()),
        (2, -1)
    );
    assert_eq!(
        (create.Cols[2].Tp.GetFlen(), create.Cols[2].Tp.GetDecimal()),
        (2, 0)
    );
    assert_eq!(
        (create.Cols[3].Tp.GetFlen(), create.Cols[3].Tp.GetDecimal()),
        (-1, -1)
    );
    let mut mysql = crate::Parser::with_dialect(parser_ast::SqlDialect::MySQL);
    assert!(mysql.ParseOneStmt(sql, "", "").is_err());
}

#[test]
fn pg_scalar_16_float4_cast_type() {
    let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in ["SELECT CAST('1.5' AS FLOAT4)", "SELECT '1.5'::float4"] {
        let stmt = parser.ParseOneStmt(sql, "", "").unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .unwrap();
        let expression = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let crate::ast::ExprKind::Cast {
            Tp, FunctionType, ..
        } = &expression.Kind
        else {
            panic!("cast")
        };
        assert_eq!(Tp.GetType(), parser_mysql::r#type::TypeFloat);
        assert_eq!(
            *FunctionType,
            crate::ast::CastFunctionType::PostgreSQLFloat4
        );
    }
}

#[test]
fn pg_scalar_17_double_parameter_cast_identity() {
    for (dialect, expected) in [
        (
            parser_ast::SqlDialect::PostgreSQL,
            crate::ast::CastFunctionType::PostgreSQLFloat8,
        ),
        (
            parser_ast::SqlDialect::MySQL,
            crate::ast::CastFunctionType::Cast,
        ),
    ] {
        let mut parser = crate::Parser::with_dialect(dialect);
        let stmt = parser
            .ParseOneStmt("SELECT CAST('-0' AS DOUBLE)", "", "")
            .unwrap();
        let select = stmt
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .unwrap();
        let expression = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let crate::ast::ExprKind::Cast {
            Tp, FunctionType, ..
        } = &expression.Kind
        else {
            panic!("cast")
        };
        assert_eq!(Tp.GetType(), parser_mysql::r#type::TypeDouble);
        assert_eq!(*FunctionType, expected);
    }
}

#[test]
fn pg_scalar_18_float_declaration_identity() {
    for sql in ["SELECT NULL::float(1,2)", "SELECT CAST(NULL AS FLOAT(1,2))"] {
        let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
        assert!(parser.ParseOneStmt(sql, "", "").is_err());
    }

    for (ty, expected) in [
        ("float(1)", 4),
        ("float(24)", 4),
        ("float(25)", 5),
        ("float(53)", 5),
        ("float", 5),
        ("real", 4),
        ("double precision", 5),
        ("float4", 4),
        ("float8", 5),
    ] {
        for sql in [
            format!("SELECT NULL::{ty}"),
            format!("SELECT CAST(NULL AS {ty})"),
        ] {
            let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
            let stmt = parser.ParseOneStmt(&sql, "", "").unwrap();
            let select = stmt
                .as_any()
                .downcast_ref::<crate::ast::SelectStmt>()
                .unwrap();
            let crate::ast::ExprKind::Cast {
                Tp, FunctionType, ..
            } = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
            else {
                panic!("cast")
            };
            assert_eq!(Tp.GetType(), expected, "{sql}");
            assert_eq!(
                *FunctionType,
                if expected == 4 {
                    crate::ast::CastFunctionType::PostgreSQLFloat4
                } else {
                    crate::ast::CastFunctionType::PostgreSQLFloat8
                },
                "{sql}"
            );
        }
    }
}

#[test]
fn pg_scalar_21_text_cast_identity() {
    for (name, oid, length) in [
        ("text", 25, -1),
        ("varchar", 1043, -1),
        ("character varying", 1043, -1),
        ("char varying", 1043, -1),
        ("bpchar", 1042, -1),
        ("char", 1042, 1),
    ] {
        for sql in [
            format!("SELECT NULL::{name}"),
            format!("SELECT CAST(NULL AS {name})"),
        ] {
            let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
            let statement = parser.ParseOneStmt(&sql, "", "").unwrap();
            let select = statement
                .as_any()
                .downcast_ref::<crate::ast::SelectStmt>()
                .unwrap();
            let crate::ast::ExprKind::Cast { Tp, .. } =
                &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
            else {
                panic!("cast")
            };
            assert_eq!(Tp.PostgreSQLTextOid(), Some(oid), "{sql}");
            assert_eq!(Tp.GetFlen(), length, "{sql}");
            let mut restored = Vec::new();
            Tp.FormatAsCastType(&mut restored, false).unwrap();
            let restored = String::from_utf8(restored).unwrap();
            let roundtrip = parser
                .ParseOneStmt(&format!("SELECT CAST(NULL AS {restored})"), "", "")
                .unwrap();
            let select = roundtrip
                .as_any()
                .downcast_ref::<crate::ast::SelectStmt>()
                .unwrap();
            let crate::ast::ExprKind::Cast { Tp, .. } =
                &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
            else {
                panic!("cast")
            };
            assert_eq!(Tp.PostgreSQLTextOid(), Some(oid));
            assert_eq!(Tp.GetFlen(), length);
        }
    }
    for sql in [
        "SELECT NULL::varchar",
        "SELECT CAST(NULL AS varchar)",
        "SELECT NULL::bpchar",
    ] {
        assert!(
            crate::Parser::with_dialect(parser_ast::SqlDialect::MySQL)
                .ParseOneStmt(sql, "", "")
                .is_err(),
            "{sql}"
        );
    }
    let statement = crate::Parser::with_dialect(parser_ast::SqlDialect::MySQL)
        .ParseOneStmt("SELECT CAST(NULL AS CHAR)", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<crate::ast::SelectStmt>()
        .unwrap();
    let crate::ast::ExprKind::Cast { Tp, .. } =
        &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
    else {
        panic!("cast")
    };
    assert_eq!(Tp.PostgreSQLTextOid(), None);
    assert_eq!(Tp.GetFlen(), -1);
}

#[test]
fn pg_temporal_date_postfix_cast() {
    use parser_ast::{CastFunctionType, ExprKind, SqlDialect};
    for sql in [
        "SELECT NULL::date",
        "SELECT $1::DATE",
        "SELECT '2000-01-01'::date",
        "SELECT NULL::date::date",
    ] {
        let (statements, _) = crate::Parser::with_dialect(SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
        let select = statements[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let ExprKind::Cast {
            Expr,
            Tp,
            FunctionType,
            ..
        } = &expr.Kind
        else {
            panic!("{sql}")
        };
        assert_eq!(Tp.GetType(), parser_mysql::r#type::TypeDate);
        assert_eq!(*FunctionType, CastFunctionType::Cast);
        assert_eq!(expr.node_text.OriginalText(), &sql.as_bytes()[7..]);
        if sql == "SELECT $1::DATE" {
            assert_eq!(Expr.PostgreSQLParameterIndex(), Some(1));
        }
        assert!(
            crate::Parser::with_dialect(SqlDialect::MySQL)
                .ParseSQL(sql, &[])
                .is_err(),
            "{sql}"
        );
    }
    for sql in ["SELECT NULL::date(2)", "SELECT NULL::date[]"] {
        assert!(
            crate::Parser::with_dialect(SqlDialect::PostgreSQL)
                .ParseSQL(sql, &[])
                .is_err(),
            "{sql}"
        );
    }
    assert!(
        crate::Parser::with_dialect(SqlDialect::MySQL)
            .ParseSQL("SELECT CAST(NULL AS DATE)", &[])
            .is_ok()
    );
}

#[test]
fn pg_temporal_time_precision_grammar() {
    use parser_ast::{ExprKind, SqlDialect};
    let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
    for (target, precision, warning) in [
        ("time", -1, false),
        ("time(0)", 0, false),
        ("time(6)", 6, false),
        ("time(7)", 6, true),
    ] {
        for sql in [
            format!("SELECT $1::{target}"),
            format!("SELECT CAST($1 AS {target})"),
        ] {
            let (statements, warnings) = pg.ParseSQL(&sql, &[]).unwrap();
            assert_eq!(warnings.len(), usize::from(warning), "{sql}");
            let select = statements[0]
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert!(
                matches!(&select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
                ExprKind::Cast {Tp,..} if Tp.GetType() == parser_mysql::r#type::TypeDuration && Tp.GetDecimal() == precision),
                "{sql}"
            );
        }
    }
    for target in ["time(-1)", "time(-0)", "time(+3)"] {
        for sql in [
            format!("SELECT NULL::{target}"),
            format!("SELECT CAST(NULL AS {target})"),
        ] {
            assert!(pg.ParseSQL(&sql, &[]).is_err(), "{sql}");
        }
    }
    let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
    for sql in [
        "SELECT NULL::time(3)",
        "SELECT CAST(NULL AS TIME(-1))",
        "SELECT CAST(NULL AS TIME(+3))",
    ] {
        assert!(mysql.ParseSQL(sql, &[]).is_err());
    }
    let (_, warnings) = mysql.ParseSQL("SELECT CAST(NULL AS TIME(7))", &[]).unwrap();
    assert!(warnings.is_empty());
}

#[test]
fn pg_temporal_ts_precision_grammar() {
    use parser_ast::{ExprKind, SqlDialect};
    let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
    for (target, precision, warning) in [
        ("timestamp", -1, false),
        ("timestamp(0)", 0, false),
        ("timestamp(6)", 6, false),
        ("timestamp(7)", 6, true),
    ] {
        for sql in [
            format!("SELECT $1::{target}"),
            format!("SELECT CAST($1 AS {target})"),
        ] {
            let (statements, warnings) = pg.ParseSQL(&sql, &[]).unwrap();
            assert_eq!(warnings.len(), usize::from(warning), "{sql}");
            let select = statements[0]
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert!(
                matches!(&select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
                ExprKind::Cast {Tp,..} if Tp.GetType() == parser_mysql::r#type::TypeDatetime && Tp.GetDecimal() == precision),
                "{sql}"
            );
        }
    }
    for target in ["timestamp(-1)", "timestamp(-0)", "timestamp(+3)"] {
        for sql in [
            format!("SELECT NULL::{target}"),
            format!("SELECT CAST(NULL AS {target})"),
        ] {
            assert!(pg.ParseSQL(&sql, &[]).is_err(), "{sql}");
        }
    }
    let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
    for sql in [
        "SELECT NULL::timestamp(3)",
        "SELECT CAST(NULL AS TIMESTAMP(-1))",
        "SELECT CAST(NULL AS TIMESTAMP(+3))",
    ] {
        assert!(mysql.ParseSQL(sql, &[]).is_err());
    }
    assert!(
        mysql
            .ParseSQL("SELECT CAST(NULL AS TIMESTAMP(7))", &[])
            .is_err()
    );
}

#[test]
fn pg_temporal_ts_typed_literal() {
    use parser_ast::{ExprKind, SqlDialect};
    for dialect in [SqlDialect::PostgreSQL, SqlDialect::MySQL] {
        let mut parser = crate::Parser::with_dialect(dialect);
        let (statements, _) = parser
            .ParseSQL("SELECT TIMESTAMP '2004-10-19 10:23:54+02'", &[])
            .unwrap();
        let select = statements[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let kind = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind;
        if dialect == SqlDialect::PostgreSQL {
            assert!(
                matches!(kind, ExprKind::Cast { Tp, .. } if Tp.GetType() == parser_mysql::r#type::TypeDatetime && Tp.GetDecimal() == -1)
            );
        } else {
            assert!(matches!(kind, ExprKind::Function { .. }));
        }
    }
}

#[test]
fn pg_timestamptz_cast_identity() {
    use crate::ast::SqlDialect;
    let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
    for (target, precision, warning) in [
        ("timestamptz", -1, false),
        ("timestamptz(0)", 0, false),
        ("timestamptz(-0)", 0, false),
        ("timestamptz(6)", 6, false),
        ("timestamptz(7)", 6, true),
    ] {
        for sql in [
            format!("SELECT $1::{target}"),
            format!("SELECT CAST($1 AS {target})"),
        ] {
            let (nodes, warnings) = pg.ParseSQL(&sql, &[]).unwrap();
            assert_eq!(warnings.len(), usize::from(warning));
            let select = nodes[0]
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert!(
                matches!(&select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
            parser_ast::ExprKind::Cast { Tp, .. } if Tp.GetType() == parser_mysql::r#type::TypePgTimestampTz && Tp.GetDecimal() == precision)
            );
            let mut mysql = crate::Parser::with_dialect(SqlDialect::MySQL);
            assert!(mysql.ParseSQL(&sql, &[]).is_err());
        }
    }

    for target in [
        "timestamptz(-1)",
        "timestamptz(+3)",
        "timestamptz(3,2)",
        "text(3)",
    ] {
        assert!(pg.ParseSQL(&format!("SELECT NULL::{target}"), &[]).is_err());
    }
}

#[test]
fn pg_timestamptz_typed_literal() {
    use crate::ast::SqlDialect;
    for sql in [
        "SELECT TIMESTAMPTZ 'now'",
        "SELECT timestamptz '2000-01-01 08:00:00+08' AS v",
        "SELECT (timestamptz /* literal */ 'infinity')",
    ] {
        let mut pg = crate::Parser::with_dialect(SqlDialect::PostgreSQL);
        let (nodes, warnings) = pg.ParseSQL(sql, &[]).unwrap();
        assert!(warnings.is_empty());
        let select = nodes[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let mut expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        if let parser_ast::ExprKind::Parentheses(inner) = &expr.Kind {
            expr = inner;
        }
        assert!(
            matches!(&expr.Kind, parser_ast::ExprKind::Cast {Tp, Expr, ..}
            if Tp.GetType() == parser_mysql::r#type::TypePgTimestampTz && Tp.GetDecimal() == -1
            && matches!(Expr.Kind, parser_ast::ExprKind::Value(_))),
            "{sql}: {expr:?}"
        );
    }
    // The unreserved identifier and quoted identifier remain column references.
    for dialect in [SqlDialect::MySQL, SqlDialect::PostgreSQL] {
        let mut parser = crate::Parser::with_dialect(dialect);
        for sql in ["SELECT timestamptz", "SELECT \"timestamptz\" 'label'"] {
            let (nodes, _) = parser.ParseSQL(sql, &[]).unwrap();
            let select = nodes[0]
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap();
            assert!(!matches!(
                select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
                parser_ast::ExprKind::Cast { .. }
            ));
        }
    }
}

#[test]
fn pg_interval_cast_and_literal_identity() {
    let mut pg = crate::Parser::with_dialect(crate::ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT $1::interval AS v",
        "SELECT CAST($1 AS interval)",
        "SELECT INTERVAL '1.5 months'",
    ] {
        let (nodes, warnings) = pg.ParseSQL(sql, &[]).unwrap();
        assert!(warnings.is_empty());
        let select = nodes[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert!(
            matches!(&select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,parser_ast::ExprKind::Cast{Tp,..} if Tp.GetType()==parser_mysql::r#type::TypePgInterval)
        );
    }
    let mut mysql = crate::Parser::with_dialect(crate::ast::SqlDialect::MySQL);
    assert!(
        mysql
            .ParseSQL("SELECT CAST('1 month' AS interval)", &[])
            .is_err()
    );
    assert!(
        mysql
            .ParseSQL("SELECT DATE_ADD('2000-01-01', INTERVAL 1 DAY)", &[])
            .is_ok()
    );
}

#[test]
fn pg_interval_declaration_fields_precision() {
    for (sql, mask, precision) in [
        ("SELECT $1::interval", -1, -1),
        ("SELECT $1::interval(3)", 32767, 3),
        ("SELECT $1::interval(3) AS YEAR", 32767, 3),
        ("SELECT CAST($1 AS interval HOUR TO MINUTE)", 3072, -1),
        ("SELECT interval '1 year 2 months' YEAR", 4, -1),
        ("SELECT interval(3) '1.2345 seconds'", 32767, 3),
        ("SELECT $1::interval DAY TO SECOND(2)", 7176, 2),
    ] {
        let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
        let (nodes, warnings) = parser.ParseSQL(sql, &[]).unwrap();
        assert!(warnings.is_empty());
        let select = nodes[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let parser_ast::ExprKind::Cast { Tp, .. } =
            &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
        else {
            panic!("{sql}");
        };
        assert_eq!(
            (Tp.GetType(), Tp.GetFlen(), Tp.GetDecimal()),
            (parser_mysql::r#type::TypePgInterval, mask, precision),
            "{sql}"
        );
    }
    for sql in [
        "SELECT interval '1 second' (3)",
        "SELECT $1::interval YEAR(3)",
        "SELECT $1::interval(3) YEAR",
        "SELECT $1::interval HOUR TO DAY",
        "SELECT $1::interval(-1)",
    ] {
        assert!(
            crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL)
                .ParseSQL(sql, &[])
                .is_err(),
            "{sql}"
        );
    }
    let (_, warnings) = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL)
        .ParseSQL("SELECT NULL::interval(7)", &[])
        .unwrap();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn pg_interval_unreserved_names() {
    let mut parser = crate::Parser::with_dialect(parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT interval FROM interval",
        "SELECT t.interval AS interval FROM interval AS t",
        "SELECT interval.interval FROM interval",
        "CREATE TABLE interval(interval int4)",
    ] {
        parser
            .ParseSQL(sql, &[])
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

#[test]
fn pg05_uuid_sql_parser_identity_and_dialect() {
    for dialect in [
        crate::ast::SqlDialect::PostgreSQL,
        crate::ast::SqlDialect::MySQL,
    ] {
        let mut parser = crate::Parser::with_dialect(dialect);
        assert!(parser.ParseSQL("SELECT uuid FROM t", &[]).is_ok());
        assert!(parser.ParseSQL("SELECT UUID()", &[]).is_ok());
    }
    let mut pg = crate::Parser::with_dialect(crate::ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT $1::uuid",
        "SELECT CAST(NULL AS uuid)",
        "SELECT '00112233445566778899aabbccddeeff'::uuid",
    ] {
        let (nodes, warnings) = pg.ParseSQL(sql, &[]).unwrap();
        assert!(warnings.is_empty());
        let select = nodes[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let parser_ast::ExprKind::Cast { Tp, .. } = &expr.Kind else {
            panic!("missing cast")
        };
        assert_eq!(Tp.GetType(), parser_mysql::r#type::TypePgUuid);
        assert_eq!(Tp.GetFlen(), 16);
        assert_eq!(Tp.GetDecimal(), -1);
        assert!(Tp.PostgreSQLTextOid().is_none());
    }
    for sql in ["SELECT NULL::uuid(3)", "SELECT CAST(NULL AS uuid)"] {
        let mut mysql = crate::Parser::with_dialect(crate::ast::SqlDialect::MySQL);
        assert!(mysql.ParseSQL(sql, &[]).is_err());
    }
    assert!(pg.ParseSQL("SELECT NULL::uuid(3)", &[]).is_err());
}
