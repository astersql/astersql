// Copyright 2026 AsterSQL.

// parser 生成状态机与语义动作的补充单元测试。
//
// 验证严格 DOUBLE 检查、生成列选项错误、FULLTEXT 修饰符、
// 集合运算 ORDER BY/LIMIT 归属，以及 yyParse 对核心 SQL 族的 AST 构造。

use super::*;

/// 严格 DOUBLE 精度检查开关与 Go 行为一致。
#[test]
fn parser_3_strict_double_type_check_matches_go() {
    let mut parser = Parser::default();
    assert!(
        parser
            .Parse("CREATE TABLE t (c DOUBLE(10))", "", "")
            .is_err()
    );
    parser.SetStrictDoubleTypeCheck(false);
    assert!(
        parser
            .Parse("CREATE TABLE t (c DOUBLE(10))", "", "")
            .is_ok()
    );
    parser.SetStrictDoubleTypeCheck(true);
    assert!(
        parser
            .Parse("CREATE TABLE t (c DOUBLE(10, 2))", "", "")
            .is_ok()
    );
}

/// 生成列非法选项错误保持 ddl 类与文案。
#[test]
fn parser_3_generated_column_option_errors_keep_ddl_class() {
    for (sql, option) in [
        (
            "create table t(a int, b int as (a + 1) default 10)",
            "DEFAULT",
        ),
        (
            "create table t(a int, b int as (a + 1) on update now())",
            "ON UPDATE",
        ),
        (
            "create table t(a int, b int as (a + 1) auto_increment)",
            "AUTO_INCREMENT",
        ),
    ] {
        let error = match Parser::default().Parse(sql, "", "") {
            Err(error) => error,
            Ok(_) => panic!("{sql}: expected generated-column option error"),
        };
        assert_eq!(
            error.to_string(),
            format!("[ddl:1221]Incorrect usage of {option} and generated column"),
            "{sql}"
        );
    }
}

/// 不支持的字符集 introducer 报错保留名称与 ddl 类。
#[test]
fn parser_3_unsupported_character_introducer_keeps_name_and_ddl_class() {
    for charset in ["gbk", "ujis"] {
        let sql = format!("select hex(_{charset} '3F')");
        let error = match Parser::default().ParseOneStmt(&sql, "", "") {
            Err(error) => error,
            Ok(_) => panic!("{sql}: expected unsupported introducer error"),
        };
        assert_eq!(
            error.to_string(),
            format!("[ddl:1115]Unsupported character introducer: '{charset}'"),
            "{sql}"
        );
    }
}

/// FULLTEXT AGAINST 修饰符位值与 Go 一致。
#[test]
fn parser_3_fulltext_modifiers_keep_go_bit_values() {
    for (suffix, expected) in [
        ("", 0),
        (" IN NATURAL LANGUAGE MODE", 0),
        (" IN NATURAL LANGUAGE MODE WITH QUERY EXPANSION", 16),
        (" IN BOOLEAN MODE", 1),
        (" WITH QUERY EXPANSION", 16),
    ] {
        let sql = format!("SELECT * FROM t WHERE MATCH(title, body) AGAINST('search'{suffix})");
        let statement = Parser::default().ParseOneStmt(&sql, "", "").unwrap();
        let select = statement
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let parser_ast::ExprKind::MatchAgainst { Modifier, .. } =
            &select.Where.as_ref().unwrap().Kind
        else {
            panic!("{sql}: expected MATCH AGAINST expression")
        };
        assert_eq!(*Modifier, expected, "{sql}");
    }
}

/// UNION 外层拥有 ORDER BY/LIMIT，内层为空。
#[test]
fn parser_3_set_operator_owns_outer_order_by_and_limit() {
    let statement = Parser::default()
        .ParseOneStmt(
            "SELECT 2 AS a UNION SELECT 1 AS b ORDER BY a LIMIT 1",
            "",
            "",
        )
        .unwrap();
    let set = statement
        .as_any()
        .downcast_ref::<parser_ast::SetOprStmt>()
        .unwrap();
    assert_eq!(set.OrderBy.len(), 1);
    assert!(set.Limit.is_some());
    let last = set.select_list.selects[1]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(last.OrderBy.is_empty());
    assert!(last.Limit.is_none());
}

/// SELECT INTO OUTFILE 选项字段对齐 Go。
#[test]
fn parser_3_select_into_outfile_keeps_go_option() {
    let statement = Parser::default()
        .ParseOneStmt("SELECT * FROM t INTO OUTFILE '/tmp/t.txt'", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let into = select.SelectIntoOpt.as_ref().expect("SELECT INTO option");
    assert_eq!(into.Tp, parser_ast::SelectIntoType::Outfile);
    assert_eq!(into.FileName, "/tmp/t.txt");
}

#[derive(Default)]
/// 用预置 token 序列驱动 yyParse 的简易词法器。
struct TokenLexer {
    tokens: Vec<isize>,
    cursor: usize,
    errors: Vec<ParserError>,
}

impl TokenLexer {
    fn new(tokens: Vec<isize>) -> Self {
        Self {
            tokens,
            ..Self::default()
        }
    }
}

impl yyLexer for TokenLexer {
    fn Lex(&mut self, _lval: &mut yySymType) -> isize {
        let token = self.tokens.get(self.cursor).copied().unwrap_or(0);
        self.cursor += 1;
        token
    }

    fn Errorf(&self, message_format: &str, _args: &[&dyn std::any::Any]) -> ParserError {
        ParserError(message_format.to_owned())
    }

    fn AppendError(&mut self, err: ParserError) {
        self.errors.push(err);
    }

    fn AppendWarn(&mut self, _err: ParserError) {}

    fn Errors(&self) -> (Vec<ParserError>, Vec<ParserError>) {
        (self.errors.clone(), Vec::new())
    }
}

#[derive(Default)]
/// 带 Reduced 回调的扩展词法器，用于验证 yyLexerEx。
struct ExtendedTokenLexer {
    inner: TokenLexer,
    reductions: usize,
}

impl yyLexer for ExtendedTokenLexer {
    fn Lex(&mut self, lval: &mut yySymType) -> isize {
        self.inner.Lex(lval)
    }
    fn Errorf(&self, format: &str, args: &[&dyn std::any::Any]) -> ParserError {
        self.inner.Errorf(format, args)
    }
    fn AppendError(&mut self, err: ParserError) {
        self.inner.AppendError(err);
    }
    fn AppendWarn(&mut self, err: ParserError) {
        self.inner.AppendWarn(err);
    }
    fn Errors(&self) -> (Vec<ParserError>, Vec<ParserError>) {
        self.inner.Errors()
    }
    fn as_extended(&mut self) -> Option<&mut dyn yyLexerEx> {
        Some(self)
    }
}

impl yyLexerEx for ExtendedTokenLexer {
    fn Reduced(&mut self, _rule: isize, _state: isize, _lval: &mut yySymType) -> bool {
        self.reductions += 1;
        true
    }
}

/// 纯 Rust 生成的稀疏动作行保留显式列与空洞。
#[test]
fn parser_3_generated_sparse_rows_preserve_explicit_columns() {
    let (state, row, missing_column) = GENERATED_MAIN_PARSE_TABLE
        .iter()
        .enumerate()
        .find_map(|(state, row)| {
            row.windows(2).find_map(|entries| {
                (entries[1].0 > entries[0].0 + 1).then_some((state, *row, entries[0].0 + 1))
            })
        })
        .expect("generated main table should contain a sparse row");

    assert_eq!(generated_main_action(state, row[0].0), row[0].1);
    assert_eq!(generated_main_action(state, missing_column), 0);
}

/// yySymName 与生成翻译表一致。
#[test]
fn parser_3_symbol_names_match_generated_translation_table() {
    assert_eq!(yySymName(account), "account");
    assert_eq!(yySymName(123_456_789), "123456789");
}

/// 非正 token 由 yylex1 折叠为 EOF。
#[test]
fn parser_3_lexer_folds_non_positive_tokens_to_eof() {
    let mut lexer = TokenLexer::new(vec![account, 0, -9]);
    let mut semantic_value = yySymType::default();
    assert_eq!(yylex1(&mut lexer, &mut semantic_value), account);
    assert_eq!(yylex1(&mut lexer, &mut semantic_value), yyEOFCode);
    assert_eq!(yylex1(&mut lexer, &mut semantic_value), yyEOFCode);
}

/// 空输入可被生成状态机接受。
#[test]
fn parser_3_generated_machine_accepts_empty_input() {
    let mut lexer = TokenLexer::new(vec![0]);
    let mut parser_state = Parser::default();
    let result = yyParse(&mut lexer, &mut parser_state);
    assert_eq!(result, 0);
    assert!(lexer.errors.is_empty());
}

/// 规约时调用 yyLexerEx::Reduced 钩子。
#[test]
fn parser_3_invokes_go_reduced_hook() {
    let mut lexer = ExtendedTokenLexer {
        inner: TokenLexer::new(vec![0]),
        ..Default::default()
    };
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), -1);
    assert_eq!(lexer.reductions, 1);
}

/// BEGIN 语句可被状态机接受。
#[test]
fn parser_3_generated_machine_accepts_begin_statement() {
    let mut lexer = TokenLexer::new(vec![token::begin, 59, 0]);
    let mut parser_state = Parser::default();
    assert_eq!(
        yyParse(&mut lexer, &mut parser_state),
        0,
        "{:?}",
        lexer.Errors().1
    );
    assert!(lexer.errors.is_empty());
}

/// 核心 SQL 族可解析通过。
#[test]
fn parser_3_generated_machine_accepts_core_go_sql_families() {
    for sql in [
        "SELECT 1",
        "INSERT INTO t(a) VALUES (1)",
        "UPDATE t SET a = 2 WHERE a = 1",
        "DELETE FROM t WHERE a = 1",
        "CREATE TABLE t(a INT)",
        "ALTER TABLE t ADD COLUMN b INT",
        "DROP TABLE t",
        "SHOW TABLES",
        "SET @a = 1",
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "EXPLAIN SELECT 1",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(lexer.Errors().1.is_empty(), "{sql}: {:?}", lexer.Errors().1);
    }
}

/// 未知 token 导致解析失败。
#[test]
fn parser_3_generated_machine_rejects_unknown_token() {
    let mut lexer = TokenLexer::new(vec![123_456_789, 0]);
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 1);
    assert_eq!(lexer.errors.len(), 1);
    assert!(lexer.errors[0].to_string().is_empty());
}

/// 默认语义值不分配 item。
#[test]
fn parser_3_empty_semantic_values_do_not_allocate_items() {
    assert!(yySymType::default().item.is_none());
}

/// 全部文法语义动作均由稳定 RuleId 的命名 Rust action 注册。
#[test]
fn parser_3_marks_every_generated_semantic_action_as_implemented() {
    let required_rules = GENERATED_MAIN_ACTION_REQUIRED
        .iter()
        .zip(RULE_IDS_BY_REDUCTION)
        .filter_map(|(required, rule)| required.then_some(*rule))
        .collect::<Vec<_>>();

    assert_eq!(required_rules.len(), 2153);
    let missing = required_rules
        .into_iter()
        .filter(|rule_id| !parser_actions::has_semantic_action(*rule_id))
        .map(RuleId::as_str)
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "named Rust semantic actions missing for RuleId: {missing:?}"
    );
}

/// 长 INSERT 复用 Parser.cache 栈存储。
#[test]
fn parser_3_reuses_stack_storage_for_long_insert() {
    // Mirrors Go TestInsertStatementMemoryAllocation's 1001-row shape. The
    // first parse may grow Parser.cache; the second must reuse that allocation.
    let sql = format!("insert t values (1){}", ",(1)".repeat(1000));
    let mut parser_state = Parser::default();

    let mut first_lexer = Scanner::default();
    first_lexer.reset(sql.clone());
    assert_eq!(yyParse(&mut first_lexer, &mut parser_state), 0);
    assert!(first_lexer.Errors().1.is_empty());
    let cached_pointer = parser_state.cache.as_ptr();
    let cached_capacity = parser_state.cache.capacity();
    assert!(cached_capacity >= 200);

    let mut second_lexer = Scanner::default();
    second_lexer.reset(sql);
    assert_eq!(yyParse(&mut second_lexer, &mut parser_state), 0);
    assert!(second_lexer.Errors().1.is_empty());
    assert_eq!(parser_state.cache.as_ptr(), cached_pointer);
    assert_eq!(parser_state.cache.capacity(), cached_capacity);
}

/// 事务与 USE 语义动作构造正确 AST。
#[test]
fn parser_3_executes_transaction_and_use_semantic_actions() {
    let cases: &[(&str, fn(&dyn parser_ast::Node))] = &[
        ("BEGIN PESSIMISTIC", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::BeginStmt>()
                .unwrap();
            assert_eq!(statement.Mode, parser_ast::Pessimistic);
        }),
        ("START TRANSACTION WITH CAUSAL CONSISTENCY ONLY", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::BeginStmt>()
                .unwrap();
            assert!(statement.CausalConsistencyOnly);
        }),
        ("START TRANSACTION READ ONLY", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::BeginStmt>()
                .unwrap();
            assert!(statement.ReadOnly);
        }),
        ("COMMIT AND CHAIN", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::CommitStmt>()
                .unwrap();
            assert_eq!(statement.CompletionType, parser_ast::CompletionTypeChain);
        }),
        ("ROLLBACK RELEASE", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::RollbackStmt>()
                .unwrap();
            assert_eq!(statement.CompletionType, parser_ast::CompletionTypeRelease);
        }),
        ("ROLLBACK TO SAVEPOINT s1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::RollbackStmt>()
                .unwrap();
            assert_eq!(statement.SavepointName, "s1");
        }),
        ("USE test_db", |node| {
            let statement = node.as_any().downcast_ref::<parser_ast::UseStmt>().unwrap();
            assert_eq!(statement.DBName, "test_db");
        }),
        ("BINLOG 'abc'", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::BinlogStmt>()
                .unwrap();
            assert_eq!(statement.Str, "abc");
        }),
        ("DEALLOCATE PREPARE stmt1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::DeallocateStmt>()
                .unwrap();
            assert_eq!(statement.Name, "stmt1");
        }),
        ("PREPARE stmt1 FROM 'SELECT 1'", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::PrepareStmt>()
                .unwrap();
            assert_eq!(statement.Name, "stmt1");
            assert_eq!(statement.SQLText, "SELECT 1");
            assert_eq!(statement.SQLVar, None);
        }),
        ("PREPARE stmt1 FROM @SQL", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::PrepareStmt>()
                .unwrap();
            assert_eq!(statement.Name, "stmt1");
            assert_eq!(statement.SQLText, "");
            assert_eq!(statement.SQLVar.as_deref(), Some("SQL"));
        }),
        ("EXECUTE stmt1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::ExecuteStmt>()
                .unwrap();
            assert_eq!(statement.Name, "stmt1");
            assert!(statement.UsingVars.is_empty());
        }),
        ("SAVEPOINT s1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::SavepointStmt>()
                .unwrap();
            assert_eq!(statement.Name, "s1");
        }),
        ("RELEASE SAVEPOINT s1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::ReleaseSavepointStmt>()
                .unwrap();
            assert_eq!(statement.Name, "s1");
        }),
        ("HELP 'contents'", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::HelpStmt>()
                .unwrap();
            assert_eq!(statement.Topic, "contents");
        }),
        ("SHUTDOWN", |node| {
            assert!(node.as_any().is::<parser_ast::ShutdownStmt>())
        }),
        ("RESTART", |node| {
            assert!(node.as_any().is::<parser_ast::RestartStmt>())
        }),
        ("DROP STATISTICS stats1", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::DropStatisticsStmt>()
                .unwrap();
            assert_eq!(statement.StatsName, "stats1");
        }),
        ("UNLOCK TABLES", |node| {
            assert!(node.as_any().is::<parser_ast::UnlockTablesStmt>())
        }),
        ("SET PASSWORD = 'secret'", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::SetPwdStmt>()
                .unwrap();
            assert_eq!(statement.Password, "secret");
        }),
        ("SET SESSION_STATES 'x'", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::SetSessionStatesStmt>()
                .unwrap();
            assert_eq!(statement.SessionStates, "x");
        }),
    ];

    for (sql, check) in cases {
        let mut lexer = Scanner::default();
        lexer.reset((*sql).to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(lexer.Errors().1.is_empty(), "{sql}: {:?}", lexer.Errors().1);
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        check(parser_state.result[0].as_ref());
    }
}

/// 简单 ADMIN 语句语义动作正确。
#[test]
fn parser_3_executes_simple_admin_semantic_actions() {
    for (sql, expected_type) in [
        ("ADMIN SHOW DDL", parser_ast::AdminStmtType::ShowDdl),
        (
            "ADMIN SHOW DDL JOBS",
            parser_ast::AdminStmtType::ShowDdlJobs,
        ),
        (
            "ADMIN CREATE WORKLOAD SNAPSHOT",
            parser_ast::AdminStmtType::WorkloadRepoCreate,
        ),
        (
            "ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST",
            parser_ast::AdminStmtType::ReloadExprPushdownBlacklist,
        ),
        (
            "ADMIN RELOAD OPT_RULE_BLACKLIST",
            parser_ast::AdminStmtType::ReloadOptRuleBlacklist,
        ),
        (
            "ADMIN FLUSH BINDINGS",
            parser_ast::AdminStmtType::FlushBindings,
        ),
        (
            "ADMIN CAPTURE BINDINGS",
            parser_ast::AdminStmtType::CaptureBindings,
        ),
        (
            "ADMIN EVOLVE BINDINGS",
            parser_ast::AdminStmtType::EvolveBindings,
        ),
        (
            "ADMIN RELOAD BINDINGS",
            parser_ast::AdminStmtType::ReloadBindings,
        ),
        (
            "ADMIN RELOAD CLUSTER BINDINGS",
            parser_ast::AdminStmtType::ReloadClusterBindings,
        ),
        (
            "ADMIN RELOAD STATISTICS",
            parser_ast::AdminStmtType::ReloadStatistics,
        ),
        (
            "ADMIN SHOW BDR ROLE",
            parser_ast::AdminStmtType::ShowBdrRole,
        ),
        (
            "ADMIN UNSET BDR ROLE",
            parser_ast::AdminStmtType::UnsetBdrRole,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::AdminStmt>()
            .unwrap();
        assert_eq!(statement.statement_type, expected_type, "{sql}");
    }

    for (sql, expected_type, expected_ids) in [
        (
            "ADMIN CANCEL DDL JOBS 1, 2",
            parser_ast::AdminStmtType::CancelDdlJobs,
            vec![1, 2],
        ),
        (
            "ADMIN PAUSE DDL JOBS 3",
            parser_ast::AdminStmtType::PauseDdlJobs,
            vec![3],
        ),
        (
            "ADMIN RESUME DDL JOBS 4, 5",
            parser_ast::AdminStmtType::ResumeDdlJobs,
            vec![4, 5],
        ),
        (
            "ADMIN SHOW DDL JOB QUERIES 6, 7",
            parser_ast::AdminStmtType::ShowDdlJobQueries,
            vec![6, 7],
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::AdminStmt>()
            .unwrap();
        assert_eq!(statement.statement_type, expected_type, "{sql}");
        assert_eq!(statement.job_ids, expected_ids, "{sql}");
    }

    for (sql, expected_type, expected_tables, expected_index) in [
        (
            "ADMIN SHOW t1 NEXT_ROW_ID",
            parser_ast::AdminStmtType::ShowNextRowId,
            vec!["t1"],
            "",
        ),
        (
            "ADMIN CHECK TABLE t1, db.t2",
            parser_ast::AdminStmtType::CheckTable,
            vec!["t1", "t2"],
            "",
        ),
        (
            "ADMIN CHECK INDEX t1 idx1",
            parser_ast::AdminStmtType::CheckIndex,
            vec!["t1"],
            "idx1",
        ),
        (
            "ADMIN RECOVER INDEX t1 idx1",
            parser_ast::AdminStmtType::RecoverIndex,
            vec!["t1"],
            "idx1",
        ),
        (
            "ADMIN CLEANUP INDEX t1 idx1",
            parser_ast::AdminStmtType::CleanupIndex,
            vec!["t1"],
            "idx1",
        ),
        (
            "ADMIN CHECKSUM TABLE t1, t2",
            parser_ast::AdminStmtType::ChecksumTable,
            vec!["t1", "t2"],
            "",
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::AdminStmt>()
            .unwrap();
        assert_eq!(statement.statement_type, expected_type, "{sql}");
        assert_eq!(
            statement
                .tables
                .iter()
                .map(|table| table.Name.O.as_str())
                .collect::<Vec<_>>(),
            expected_tables,
            "{sql}",
        );
        assert_eq!(statement.index, expected_index, "{sql}");
    }

    let sql = "ADMIN CANCEL DDL JOBS 18446744073709551612";
    let mut lexer = Scanner::default();
    lexer.reset(sql.to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 1);
    assert!(
        lexer
            .Errors()
            .1
            .iter()
            .any(|error| error.to_string().contains("out of range"))
    );
}

/// 基础 SELECT 直接构造 AST，无需兼容转换。
#[test]
fn parser_3_builds_basic_select_without_compat_converter() {
    for sql in [
        "SELECT 1",
        "SELECT DISTINCT 1",
        "SELECT a, 'x' AS b WHERE a = 1",
        "SELECT a FROM db.t AS x WHERE a = 1",
        "SELECT * FROM t USE INDEX (idx_a)",
        "SELECT * FROM a LEFT JOIN b ON a.id = b.id",
        "SELECT * FROM a, b",
        "SELECT a FROM t ORDER BY a DESC LIMIT 2 OFFSET 1",
        "SELECT (a + 1) * 2, ~a, ? FROM t",
        "SELECT CASE a WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END FROM t",
        "SELECT CAST(a AS CHAR(10) BINARY), CAST(a AS DATE), CAST(a AS YEAR)",
        "SELECT CAST(a AS DATETIME(3)), CAST(a AS DECIMAL(10,2)), CAST(a AS TIME(2))",
        "SELECT CAST(a AS SIGNED), CAST(a AS UNSIGNED), CAST(a AS JSON)",
        "SELECT CAST(a AS DOUBLE), CAST(a AS FLOAT(24)), CAST(a AS REAL)",
        "SELECT db.custom_fn(1, a) FROM t",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(lexer.Errors().1.is_empty(), "{sql}: {:?}", lexer.Errors().1);
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert!(!statement.Fields.Fields.is_empty(), "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("SELECT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let expression = statement.Fields.Fields[0].Expr.as_ref().unwrap();
    assert!(matches!(
        &expression.Kind,
        parser_ast::ExprKind::Value(value)
            if value.Datum == parser_ast::ValueDatum::Int64(1)
    ));
    assert_eq!(expression.OriginTextPosition, 7);

    let mut lexer = Scanner::default();
    lexer.reset("SELECT DISTINCT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(statement.Distinct);

    let mut lexer = Scanner::default();
    lexer.reset("SELECT a FROM db.t AS x".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let left = statement
        .From
        .as_ref()
        .unwrap()
        .TableRefs
        .Left
        .as_deref()
        .unwrap();
    let parser_ast::ResultSetNode::TableSource(source) = left else {
        panic!("table source")
    };
    assert_eq!(source.Source.Schema.O, "db");
    assert_eq!(source.Source.Name.O, "t");
    assert_eq!(source.AsName.O, "x");

    let mut lexer = Scanner::default();
    lexer.reset("SELECT * FROM a LEFT JOIN b ON a.id = b.id".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let join = &statement.From.as_ref().unwrap().TableRefs;
    assert_eq!(join.Tp, parser_ast::JoinType::LeftJoin);
    assert!(join.On.is_some());

    let mut lexer = Scanner::default();
    lexer.reset("SELECT a FROM t ORDER BY a DESC LIMIT 2 OFFSET 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(statement.OrderBy[0].Desc);
    assert_eq!(
        statement.Limit.as_ref().unwrap().Count,
        Some(parser_ast::ExprNode::Value("2".to_owned()))
    );
    assert_eq!(
        statement.Limit.as_ref().unwrap().Offset,
        Some(parser_ast::ExprNode::Value("1".to_owned()))
    );

    let mut lexer = Scanner::default();
    lexer.reset("SELECT 1 UNION ALL SELECT 2 EXCEPT SELECT 3".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SetOprStmt>()
        .unwrap();
    assert_eq!(statement.select_list.selects.len(), 3);
    assert_eq!(
        statement.select_list.operators,
        vec![
            None,
            Some(parser_ast::SetOprType::UnionAll),
            Some(parser_ast::SetOprType::Except),
        ]
    );

    let mut lexer = Scanner::default();
    lexer.reset("SELECT 1 UNION SELECT 2 ORDER BY 1 LIMIT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SetOprStmt>()
        .unwrap();
    assert_eq!(statement.OrderBy.len(), 1);
    assert!(statement.Limit.is_some());

    let mut lexer = Scanner::default();
    lexer.reset(
        "WITH RECURSIVE cte(a) AS (SELECT 1), cte2 AS (SELECT 2) SELECT a FROM cte".to_owned(),
    );
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    let with_clause = statement.With.as_ref().unwrap().borrow();
    assert!(with_clause.IsRecursive);
    assert_eq!(with_clause.CTEs.len(), 2);
    assert!(with_clause.CTEs.iter().all(|cte| cte.IsRecursive));
    assert_eq!(with_clause.CTEs[0].Name.O, "cte");
    assert_eq!(with_clause.CTEs[0].ColNameList[0].O, "a");

    for sql in [
        "SELECT 1 UNION (SELECT 2 ORDER BY 1 LIMIT 1)",
        "(SELECT 1 UNION SELECT 2) UNION ALL SELECT 3",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        assert!(
            parser_state.result[0]
                .as_any()
                .is::<parser_ast::SetOprStmt>(),
            "{sql}"
        );
    }

    for (sql, priority, small, big, buffer, found, straight) in [
        (
            "SELECT HIGH_PRIORITY 1",
            2,
            false,
            false,
            false,
            false,
            false,
        ),
        (
            "SELECT SQL_SMALL_RESULT 1",
            0,
            true,
            false,
            false,
            false,
            false,
        ),
        (
            "SELECT SQL_BIG_RESULT 1",
            0,
            false,
            true,
            false,
            false,
            false,
        ),
        (
            "SELECT SQL_BUFFER_RESULT 1",
            0,
            false,
            false,
            true,
            false,
            false,
        ),
        (
            "SELECT SQL_CALC_FOUND_ROWS 1",
            0,
            false,
            false,
            false,
            true,
            false,
        ),
        (
            "SELECT STRAIGHT_JOIN 1",
            0,
            false,
            false,
            false,
            false,
            true,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        let select = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(select.SelectStmtOpts.Priority, priority, "{sql}");
        assert_eq!(select.SelectStmtOpts.SQLSmallResult, small, "{sql}");
        assert_eq!(select.SelectStmtOpts.SQLBigResult, big, "{sql}");
        assert_eq!(select.SelectStmtOpts.SQLBufferResult, buffer, "{sql}");
        assert_eq!(select.SelectStmtOpts.CalcFoundRows, found, "{sql}");
        assert_eq!(select.SelectStmtOpts.StraightJoin, straight, "{sql}");
    }

    for (sql, lock_type, wait) in [
        (
            "SELECT * FROM t FOR UPDATE OF t WAIT 3",
            parser_ast::SelectLockType::ForUpdateWaitN,
            3,
        ),
        (
            "SELECT * FROM t FOR SHARE SKIP LOCKED",
            parser_ast::SelectLockType::ForShareSkipLocked,
            0,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        let select = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        let lock = select.lock_info.as_ref().unwrap();
        assert_eq!(lock.LockType, lock_type, "{sql}");
        assert_eq!(lock.WaitSec, wait, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("SELECT * FROM t TABLESAMPLE SYSTEM (10 PERCENT) REPEATABLE (7)".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let select = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
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
    let sample = source.TableSample.as_ref().unwrap();
    assert_eq!(sample.SampleMethod, parser_ast::SampleMethodType::System);
    assert_eq!(
        sample.SampleClauseUnit,
        parser_ast::SampleClauseUnitType::Percent
    );
    assert!(sample.RepeatableSeed.is_some());

    for (sql, kind, rows) in [
        (
            "TABLE db.t ORDER BY a LIMIT 2",
            parser_ast::SelectStmtKind::Table,
            0,
        ),
        (
            "VALUES ROW(1, 2), ROW(3, 4) ORDER BY 1 LIMIT 1",
            parser_ast::SelectStmtKind::Values,
            2,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let select = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert_eq!(select.Kind, kind, "{sql}");
        assert_eq!(select.Lists.len(), rows, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.EnableWindowFunc(true);
    lexer.reset("SELECT ROW_NUMBER() OVER win FROM t WINDOW win AS (PARTITION BY b ORDER BY a ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(
        yyParse(&mut lexer, &mut parser_state),
        0,
        "{:?}",
        lexer.Errors().1
    );
    assert!(parser_state.allStatementsSemanticallyComplete);
    let select = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.WindowSpecs.len(), 1);
    assert_eq!(select.WindowSpecs[0].Name.O, "win");
    assert!(select.WindowSpecs[0].Frame.is_some());
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::WindowFunction { .. }
    ));
}

/// ORDER BY/GROUP BY 的整数序号必须保留字面值，供执行器解析投影位置。
#[test]
fn parser_3_keeps_positional_by_item_literals() {
    let statement = Parser::default()
        .ParseOneStmt("SELECT 1, SUM(a) FROM t GROUP BY 1 ORDER BY 2", "", "")
        .unwrap();
    let select = statement
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();

    for (item, expected) in [(&select.GroupBy[0], 1), (&select.OrderBy[0], 2)] {
        assert!(matches!(
            &item.Expr.Kind,
            parser_ast::ExprKind::Value(value)
                if value.Datum == parser_ast::ValueDatum::Int64(expected)
        ));
    }
}

/// 多变量 SET 必须分别保留每个右值，避免后续 EXECUTE 绑定错误参数。
#[test]
fn parser_3_keeps_each_set_assignment_value() {
    let statement = Parser::default()
        .ParseOneStmt("SET @a = 1, @b = 2", "", "")
        .unwrap();
    let set = statement
        .as_any()
        .downcast_ref::<parser_ast::SetStmt>()
        .unwrap();
    assert_eq!(set.Variables.len(), 2);
    for (variable, expected) in set.Variables.iter().zip([1, 2]) {
        assert!(matches!(
            &variable.Value.Kind,
            parser_ast::ExprKind::Value(value)
                if value.Datum == parser_ast::ValueDatum::Int64(expected)
        ));
    }
}

/// 内置函数与聚合表达式节点对齐 Go。
#[test]
fn parser_3_builds_go_builtin_and_aggregate_expression_nodes() {
    let mut lexer = Scanner::default();
    lexer.reset("SELECT DATE '2024-01-01', MOD(7, 3), TRIM(BOTH 'x' FROM a), GET_FORMAT(DATE, 'USA') FROM t".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(
        yyParse(&mut lexer, &mut parser_state),
        0,
        "{:?}",
        lexer.Errors().1
    );
    assert!(parser_state.allStatementsSemanticallyComplete);
    let select = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert_eq!(select.Fields.Fields.len(), 4);
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::Function { .. }
    ));
    assert!(
        matches!(select.Fields.Fields[1].Expr.as_ref().unwrap().Kind, parser_ast::ExprKind::Binary { ref Op, .. } if Op == "%")
    );
    let parser_ast::ExprKind::Function {
        Args: trim_args, ..
    } = &select.Fields.Fields[2].Expr.as_ref().unwrap().Kind
    else {
        panic!("trim")
    };
    assert!(matches!(
        trim_args[2].Kind,
        parser_ast::ExprKind::TrimDirection(parser_ast::TrimDirectionType::Both)
    ));
    let parser_ast::ExprKind::Function {
        Args: format_args, ..
    } = &select.Fields.Fields[3].Expr.as_ref().unwrap().Kind
    else {
        panic!("get_format")
    };
    assert!(matches!(
        format_args[0].Kind,
        parser_ast::ExprKind::GetFormatSelector(parser_ast::GetFormatSelectorType::Date)
    ));

    let mut lexer = Scanner::default();
    lexer.EnableWindowFunc(true);
    lexer.reset("SELECT COUNT(DISTINCT a), SUM(a) OVER (PARTITION BY b) FROM t".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(
        yyParse(&mut lexer, &mut parser_state),
        0,
        "{:?}",
        lexer.Errors().1
    );
    assert!(parser_state.allStatementsSemanticallyComplete);
    let select = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SelectStmt>()
        .unwrap();
    assert!(matches!(
        select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::AggregateFunction { Distinct: true, .. }
    ));
    assert!(matches!(
        select.Fields.Fields[1].Expr.as_ref().unwrap().Kind,
        parser_ast::ExprKind::WindowFunction { .. }
    ));
}

/// INSERT VALUES 直接构造 AST。
#[test]
fn parser_3_builds_insert_values_without_compat_converter() {
    for (sql, ignored, rows) in [
        (
            "INSERT INTO db.t(a, b) VALUES (1, 2), (3, DEFAULT)",
            false,
            2,
        ),
        ("INSERT IGNORE INTO t VALUES (1)", true, 1),
        ("INSERT INTO t SET a = 1, b = 2", false, 1),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(lexer.Errors().1.is_empty(), "{sql}: {:?}", lexer.Errors().1);
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::InsertStmt>()
            .unwrap();
        assert_eq!(statement.IgnoreErr, ignored, "{sql}");
        assert_eq!(statement.Lists.len(), rows, "{sql}");
        assert!(statement.Table.is_some(), "{sql}");
    }
}

/// UPDATE/DELETE 直接构造 AST。
#[test]
fn parser_3_builds_update_delete_without_compat_converter() {
    let mut lexer = Scanner::default();
    lexer.reset("UPDATE db.t SET a = a + 1 WHERE id = 9 ORDER BY id DESC LIMIT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let update = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::UpdateStmt>()
        .unwrap();
    assert_eq!(update.List.len(), 1);
    assert!(update.Where.is_some());
    assert!(update.Order[0].Desc);
    assert!(update.Limit.is_some());

    let mut lexer = Scanner::default();
    lexer.reset("DELETE FROM db.t WHERE id = 9 ORDER BY id DESC LIMIT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let delete = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::DeleteStmt>()
        .unwrap();
    assert!(delete.Where.is_some());
    assert!(delete.Order[0].Desc);
    assert!(delete.Limit.is_some());
}

/// 库级 DDL 直接构造 AST。
#[test]
fn parser_3_builds_basic_database_ddl_without_compat_converter() {
    let cases: &[(&str, fn(&dyn parser_ast::Node))] = &[
        (
            "CREATE DATABASE IF NOT EXISTS app CHARACTER SET utf8mb4",
            |node| {
                let statement = node
                    .as_any()
                    .downcast_ref::<parser_ast::CreateDatabaseStmt>()
                    .unwrap();
                assert!(statement.IfNotExists);
                assert_eq!(statement.Name, "app");
                assert_eq!(statement.Options.len(), 1);
                assert_eq!(
                    statement.Options[0].Tp,
                    parser_ast::DatabaseOptionType::Charset
                );
                assert_eq!(statement.Options[0].Value, "utf8mb4");
            },
        ),
        ("DROP DATABASE IF EXISTS app", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::DropDatabaseStmt>()
                .unwrap();
            assert!(statement.IfExists);
            assert_eq!(statement.Name, "app");
        }),
        ("DROP TABLE IF EXISTS db.t, t2", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::DropTableStmt>()
                .unwrap();
            assert!(statement.IfExists);
            assert_eq!(statement.Tables.len(), 2);
        }),
        ("TRUNCATE TABLE db.t", |node| {
            let statement = node
                .as_any()
                .downcast_ref::<parser_ast::TruncateTableStmt>()
                .unwrap();
            assert_eq!(statement.Table.Schema.O, "db");
        }),
        (
            "CREATE TABLE IF NOT EXISTS db.t(a INT NOT NULL, b BIGINT DEFAULT 1)",
            |node| {
                let statement = node
                    .as_any()
                    .downcast_ref::<parser_ast::CreateTableStmt>()
                    .unwrap();
                assert!(statement.IfNotExists);
                assert_eq!(statement.Table.Schema.O, "db");
                assert_eq!(statement.Cols.len(), 2);
                assert_eq!(
                    statement.Cols[0].Tp.GetType(),
                    parser_mysql::r#type::TypeLong
                );
                assert_eq!(
                    statement.Cols[0].Options[0].Tp,
                    parser_ast::ColumnOptionType::NotNull
                );
            },
        ),
        (
            "CREATE TABLE t(a BIGINT UNSIGNED, b VARCHAR(20), c DECIMAL(8,2))",
            |node| {
                let statement = node
                    .as_any()
                    .downcast_ref::<parser_ast::CreateTableStmt>()
                    .unwrap();
                assert_eq!(statement.Cols.len(), 3);
                assert_ne!(
                    statement.Cols[0].Tp.GetFlag() & parser_mysql::r#type::UnsignedFlag,
                    0
                );
                assert_eq!(
                    statement.Cols[1].Tp.GetType(),
                    parser_mysql::r#type::TypeVarchar
                );
                assert_eq!(statement.Cols[1].Tp.GetFlen(), 20);
                assert_eq!(
                    statement.Cols[2].Tp.GetType(),
                    parser_mysql::r#type::TypeNewDecimal
                );
                assert_eq!(statement.Cols[2].Tp.GetDecimal(), 2);
            },
        ),
        (
            "CREATE TABLE t(a INT) CHARSET=utf8mb4 AUTO_INCREMENT=7 TTL_ENABLE='ON'",
            |node| {
                let statement = node
                    .as_any()
                    .downcast_ref::<parser_ast::CreateTableStmt>()
                    .unwrap();
                assert_eq!(statement.Options.len(), 3);
                assert_eq!(
                    statement.Options[0].Tp,
                    parser_ast::TableOptionType::Charset
                );
                assert_eq!(statement.Options[1].UintValue, 7);
                assert!(statement.Options[2].BoolValue);
            },
        ),
    ];
    for (sql, check) in cases {
        let mut lexer = Scanner::default();
        lexer.reset((*sql).to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        check(parser_state.result[0].as_ref());
    }
}

/// SET 语句直接构造 AST。
#[test]
fn parser_3_builds_set_statements_without_compat_converter() {
    for (sql, check) in [
        ("SET a = 1, GLOBAL b = ON", 2usize),
        ("SET @user_value = a + 1", 1),
        ("SET NAMES utf8mb4 COLLATE utf8mb4_bin", 1),
        (
            "SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY",
            2,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SetStmt>()
            .unwrap();
        assert_eq!(statement.Variables.len(), check, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("SET @@GLOBAL.autocommit = 0".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::SetStmt>()
        .unwrap();
    assert_eq!(statement.Variables[0].Name, "autocommit");
    assert!(statement.Variables[0].IsGlobal);
    assert!(statement.Variables[0].IsSystem);
}

/// 账户选项节点对齐 Go。
#[test]
fn parser_3_builds_go_account_option_nodes() {
    let mut lexer = Scanner::default();
    lexer.reset("CREATE USER IF NOT EXISTS 'u'@'%' IDENTIFIED BY 'p' REQUIRE SSL WITH MAX_USER_CONNECTIONS 5 PASSWORD EXPIRE NEVER ACCOUNT LOCK".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(
        yyParse(&mut lexer, &mut parser_state),
        0,
        "{:?}",
        lexer.Errors().1
    );
    assert!(parser_state.allStatementsSemanticallyComplete);
    let statement = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateUserStmt>()
        .unwrap();
    assert!(statement.IfNotExists);
    assert_eq!(statement.Specs.len(), 1);
    assert!(statement.Specs[0].AuthOpt.as_ref().unwrap().ByAuthString);
    assert_eq!(
        statement.AuthTokenOrTLSOptions[0].Type,
        parser_ast::AuthTokenOrTLSOptionType::Ssl
    );
    assert_eq!(
        statement.ResourceOptions[0].Type,
        parser_ast::ResourceOptionType::MaxUserConnections
    );
    assert_eq!(statement.PasswordOrLockOptions.len(), 2);
}

/// SHOW 语句直接构造 AST。
#[test]
fn parser_3_builds_show_statements_without_compat_converter() {
    let cases = [
        ("SHOW FULL TABLES FROM db", parser_ast::ShowStmtType::Tables),
        (
            "SHOW CREATE TABLE db.t",
            parser_ast::ShowStmtType::CreateTable,
        ),
        ("SHOW GLOBAL VARIABLES", parser_ast::ShowStmtType::Variables),
        ("SHOW COUNT(*) WARNINGS", parser_ast::ShowStmtType::Warnings),
        ("SHOW INDEX FROM db.t", parser_ast::ShowStmtType::Index),
        ("SHOW PROCESSLIST", parser_ast::ShowStmtType::ProcessList),
    ];
    for (sql, expected_type) in cases {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::ShowStmt>()
            .unwrap();
        assert_eq!(statement.Tp, expected_type, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("SHOW COLLATION LIKE 'utf8mb4%'".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let like = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::ShowStmt>()
        .unwrap();
    assert!(like.Pattern.is_some());
    assert!(like.Where.is_none());

    lexer.reset("SHOW COLLATION WHERE Charset = 'utf8mb4'".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let filtered = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::ShowStmt>()
        .unwrap();
    assert!(filtered.Pattern.is_none());
    assert!(filtered.Where.is_some());
}

/// EXPLAIN 直接构造 AST。
#[test]
fn parser_3_builds_explain_without_compat_converter() {
    for (sql, analyze, format) in [
        ("EXPLAIN SELECT 1", false, "row"),
        ("EXPLAIN ANALYZE SELECT * FROM t", true, "row"),
        ("EXPLAIN FORMAT = JSON SELECT 1", false, "JSON"),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::ExplainStmt>()
            .unwrap();
        assert_eq!(statement.analyze, analyze, "{sql}");
        assert_eq!(statement.Format, format, "{sql}");
        assert!(statement.stmt.is_some(), "{sql}");
    }
}

/// 常见 ALTER TABLE 直接构造 AST。
#[test]
fn parser_3_builds_common_alter_table_without_compat_converter() {
    let cases = [
        (
            "ALTER TABLE db.t ADD COLUMN a INT FIRST",
            parser_ast::AlterTableType::AddColumns,
        ),
        (
            "ALTER TABLE t MODIFY COLUMN a BIGINT AFTER b",
            parser_ast::AlterTableType::ModifyColumn,
        ),
        (
            "ALTER TABLE t CHANGE COLUMN a c VARCHAR(20)",
            parser_ast::AlterTableType::ChangeColumn,
        ),
        (
            "ALTER TABLE t DROP INDEX IF EXISTS idx",
            parser_ast::AlterTableType::DropIndex,
        ),
        (
            "ALTER TABLE t RENAME COLUMN a TO b",
            parser_ast::AlterTableType::RenameColumn,
        ),
        (
            "ALTER TABLE t RENAME TO db.t2",
            parser_ast::AlterTableType::RenameTable,
        ),
        (
            "ALTER TABLE t ALGORITHM = INPLACE",
            parser_ast::AlterTableType::Algorithm,
        ),
        (
            "ALTER TABLE t LOCK = SHARED",
            parser_ast::AlterTableType::Lock,
        ),
    ];
    for (sql, expected_type) in cases {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::AlterTableStmt>()
            .unwrap();
        assert_eq!(statement.Specs.len(), 1, "{sql}");
        assert_eq!(statement.Specs[0].Tp, expected_type, "{sql}");
    }
}

/// 补充用例：parser_3_builds_index_ddl_without_compat_converter。
#[test]
fn parser_3_builds_index_ddl_without_compat_converter() {
    let mut lexer = Scanner::default();
    lexer.reset(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx ON db.t(a(10) DESC, (a + 1)) LOCK = SHARED"
            .to_owned(),
    );
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let create = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateIndexStmt>()
        .unwrap();
    assert!(create.IfNotExists);
    assert_eq!(create.KeyType, parser_ast::IndexKeyType::Unique);
    assert_eq!(create.IndexPartSpecifications.len(), 2);
    assert_eq!(create.IndexPartSpecifications[0].Length, 10);
    assert!(create.IndexPartSpecifications[0].Desc);
    assert_eq!(
        create.LockAlg.as_ref().unwrap().LockTp,
        parser_ast::LockType::Shared
    );

    let mut lexer = Scanner::default();
    lexer.reset("DROP INDEX IF EXISTS idx ON db.t ALGORITHM = INPLACE".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let drop = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::DropIndexStmt>()
        .unwrap();
    assert!(drop.IfExists);
    assert_eq!(drop.IndexName, "idx");
    assert_eq!(
        drop.LockAlg.as_ref().unwrap().AlgorithmTp,
        parser_ast::AlgorithmType::Inplace
    );
}

/// 补充用例：parser_3_builds_rename_table_without_compat_converter。
#[test]
fn parser_3_builds_rename_table_without_compat_converter() {
    let mut lexer = Scanner::default();
    lexer.reset("RENAME TABLE db.old TO db.new, x TO y".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let rename = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::RenameTableStmt>()
        .expect("native rename table AST");
    assert_eq!(rename.TableToTables.len(), 2);
    assert_eq!(rename.TableToTables[0].OldTable.Schema.O, "db");
    assert_eq!(rename.TableToTables[0].OldTable.Name.O, "old");
    assert_eq!(rename.TableToTables[0].NewTable.Name.O, "new");
    assert_eq!(rename.TableToTables[1].OldTable.Name.O, "x");
    assert_eq!(rename.TableToTables[1].NewTable.Name.O, "y");
}

/// 补充用例：parser_3_builds_analyze_and_compact_without_compat_converter。
#[test]
fn parser_3_builds_analyze_and_compact_without_compat_converter() {
    let mut lexer = Scanner::default();
    lexer.reset(
        "ANALYZE TABLE db.t PARTITION p0, p1 COLUMNS a, b WITH 16 BUCKETS 8 TOPN".to_owned(),
    );
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(
        parser_state.allStatementsSemanticallyComplete,
        "ANALYZE path must be fully native"
    );
    let analyze = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::AnalyzeTableStmt>()
        .unwrap();
    assert_eq!(analyze.TableNames[0].Schema.O, "db");
    assert_eq!(
        analyze
            .PartitionNames
            .iter()
            .map(|name| name.O.as_str())
            .collect::<Vec<_>>(),
        ["p0", "p1"]
    );
    assert_eq!(analyze.ColumnChoice, parser_ast::ColumnChoice::List);
    assert_eq!(
        analyze
            .ColumnNames
            .iter()
            .map(|name| name.O.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(analyze.AnalyzeOpts.len(), 2);
    assert_eq!(
        analyze.AnalyzeOpts[0].Type,
        parser_ast::AnalyzeOptionType::NumBuckets
    );

    let mut lexer = Scanner::default();
    lexer.reset("ALTER TABLE db.t COMPACT PARTITION p0, p1 TIFLASH REPLICA".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(
        parser_state.allStatementsSemanticallyComplete,
        "COMPACT path must be fully native"
    );
    let compact = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::CompactTableStmt>()
        .unwrap();
    assert_eq!(compact.Table.Name.O, "t");
    assert_eq!(compact.PartitionNames.len(), 2);
    assert_eq!(compact.ReplicaKind, parser_ast::CompactReplicaKind::TiFlash);
}

/// 补充用例：parser_3_builds_flush_and_table_locks_without_compat_converter。
#[test]
fn parser_3_builds_flush_and_table_locks_without_compat_converter() {
    for (sql, expected_type) in [
        ("FLUSH PRIVILEGES", parser_ast::FlushStmtType::Privileges),
        ("FLUSH BINARY LOGS", parser_ast::FlushStmtType::Logs),
        (
            "FLUSH TABLES db.t, x WITH READ LOCK",
            parser_ast::FlushStmtType::Tables,
        ),
        (
            "FLUSH TIDB PLUGINS audit, auth",
            parser_ast::FlushStmtType::TiDBPlugin,
        ),
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let flush = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::FlushStmt>()
            .unwrap();
        assert_eq!(flush.Tp, expected_type, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("LOCK TABLES db.t READ LOCAL, x WRITE".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let locks = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::LockTablesStmt>()
        .unwrap();
    assert_eq!(locks.TableLocks.len(), 2);
    assert_eq!(
        locks.TableLocks[0].Type,
        parser_ast::TableLockType::ReadLocal
    );
    assert_eq!(locks.TableLocks[1].Type, parser_ast::TableLockType::Write);
}

/// 补充用例：parser_3_builds_go_predicate_expressions_without_compat_converter。
#[test]
fn parser_3_builds_go_predicate_expressions_without_compat_converter() {
    for sql in [
        "SELECT * FROM t WHERE a IS NOT NULL",
        "SELECT * FROM t WHERE a NOT IN (1, 2, 3)",
        "SELECT * FROM t WHERE a BETWEEN 1 AND 9",
        "SELECT * FROM t WHERE a LIKE 'x%' ESCAPE '!'",
        "SELECT * FROM t WHERE a REGEXP '^x'",
        "SELECT * FROM t WHERE a IS TRUE",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let select = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::SelectStmt>()
            .unwrap();
        assert!(select.Where.is_some(), "{sql}");
    }
}

/// 补充用例：parser_3_builds_trace_and_backup_without_raw_fallback。
#[test]
fn parser_3_builds_trace_and_backup_without_raw_fallback() {
    let mut lexer = Scanner::default();
    lexer.reset("TRACE FORMAT = 'json' SELECT 1".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let trace = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::TraceStmt>()
        .unwrap();
    assert_eq!(trace.Format, "json");
    assert!(trace.Stmt.as_any().is::<parser_ast::SelectStmt>());

    let mut lexer = Scanner::default();
    lexer.reset("BACKUP DATABASE a TO 'local:///tmp/archive01/'".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let backup = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::BRIEStmt>()
        .unwrap();
    assert_eq!(backup.Kind, parser_ast::BRIEKind::Backup);
    assert_eq!(backup.Schemas, ["a"]);
    assert_eq!(backup.Storage, "local:///tmp/archive01/");
}

/// 补充用例：parser_3_builds_misc_and_stats_statements_like_go。
#[test]
fn parser_3_builds_misc_and_stats_statements_like_go() {
    for sql in [
        "DO 1, a + 2",
        "KILL TIDB QUERY 42",
        "LOAD STATS '/tmp/stats.json'",
        "LOCK STATS db.t",
        "UNLOCK STATS db.t",
        "OPTIMIZE NO_WRITE_TO_BINLOG TABLE db.t, x",
        "DROP STATS db.t",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("KILL TIDB QUERY 42".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    let kill = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::KillStmt>()
        .unwrap();
    assert!(kill.Query);
    assert!(kill.TiDBExtension);
    assert_eq!(kill.ConnectionID, 42);

    let mut lexer = Scanner::default();
    lexer.reset("CREATE STATISTICS IF NOT EXISTS s (DEPENDENCY) ON db.t(a, b)".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let stats = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateStatisticsStmt>()
        .unwrap();
    assert!(stats.IfNotExists);
    assert_eq!(stats.StatsName, "s");
    assert_eq!(stats.StatsType, 1);
    assert_eq!(stats.Columns.len(), 2);

    for sql in [
        "RECOVER TABLE BY JOB 12",
        "RECOVER TABLE db.t 3",
        "FLASHBACK CLUSTER TO TIMESTAMP '2024-01-02 03:04:05'",
        "FLASHBACK TABLE db.t TO t2",
        "FLASHBACK DATABASE db TO db2",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
    }

    for sql in [
        "ALTER DATABASE db CHARACTER SET utf8mb4 COLLATE utf8mb4_bin",
        "ALTER DATABASE CHARACTER SET utf8mb4",
        "DISTRIBUTE TABLE db.t PARTITION (p0, p1) RULE = 'leader-scatter' ENGINE = 'tikv' TIMEOUT = '30m'",
        "CANCEL DISTRIBUTION JOB 17",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
    }

    for sql in [
        "CREATE SEQUENCE IF NOT EXISTS db.s INCREMENT BY 2 START WITH -3 MINVALUE -10 MAXVALUE 100 CACHE 20 CYCLE",
        "ALTER SEQUENCE IF EXISTS db.s INCREMENT BY 4 NOCACHE",
        "ALTER SEQUENCE db.s RESTART WITH -8",
        "DROP SEQUENCE IF EXISTS db.s, s2",
        "CANCEL IMPORT JOB 19",
        "RENAME USER 'old'@'LOCALHOST' TO 'new'@'Example.COM', plain TO other",
        "DROP USER IF EXISTS 'old'@'LOCALHOST', plain",
        "DROP ROLE IF EXISTS 'reader'@'LOCALHOST', writer",
        "DROP PROCEDURE IF EXISTS db.proc",
        "DROP PLACEMENT POLICY IF EXISTS p1",
        "DROP RESOURCE GROUP IF EXISTS rg1",
        "QUERY WATCH REMOVE 42",
        "QUERY WATCH REMOVE RESOURCE GROUP rg1",
        "RECOMMEND INDEX SHOW OPTION",
        "RECOMMEND INDEX APPLY 7",
        "RECOMMEND INDEX IGNORE 8",
        "ALTER INSTANCE RELOAD TLS",
        "ALTER INSTANCE RELOAD TLS NO ROLLBACK ON ERROR",
        "REFRESH STATS *.*, app.*, app.t, lone FULL CLUSTER",
        "REFRESH STATS app.t LITE",
        "PLAN REPLAYER LOAD 'dump.zip'",
        "PLAN REPLAYER CAPTURE 'sql-digest' 'plan-digest'",
        "PLAN REPLAYER CAPTURE REMOVE 'sql-digest' 'plan-digest'",
        "PLAN REPLAYER DUMP EXPLAIN 'slow.log'",
        "PLAN REPLAYER DUMP EXPLAIN ANALYZE 'slow.log'",
        "PLAN REPLAYER DUMP WITH STATS AS OF TIMESTAMP '2024-01-01' EXPLAIN SELECT 1",
        "TRAFFIC CAPTURE TO '/tmp/capture' DURATION='1h30m' ENCRYPTION_METHOD='aes256-ctr' COMPRESS=true",
        "TRAFFIC REPLAY FROM '/tmp/capture' USER='u1' PASSWORD='secret' SPEED=1.5 READ_ONLY=true",
        "SHOW TRAFFIC JOBS",
        "CANCEL TRAFFIC JOBS",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
    }

    let mut lexer = Scanner::default();
    lexer.reset("CALL db.proc(1, a + 2)".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    assert!(parser_state.result[0].as_any().is::<parser_ast::CallStmt>());

    let mut lexer = Scanner::default();
    lexer.reset("REPLACE INTO db.t(a) VALUES (1), (2)".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let replace = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::InsertStmt>()
        .unwrap();
    assert!(replace.IsReplace);
    assert_eq!(replace.Lists.len(), 2);

    let mut lexer = Scanner::default();
    lexer.reset("TRAFFIC CAPTURE TO '/tmp/capture' DURATION='not-a-duration'".to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 1);
    assert!(
        lexer
            .Errors()
            .1
            .iter()
            .any(|error| error.to_string().contains("DURATION"))
    );
}

/// 补充用例：parser_3_builds_all_brie_statement_kinds_without_fallback。
#[test]
fn parser_3_builds_all_brie_statement_kinds_without_fallback() {
    let cases = [
        (
            "BACKUP LOGS TO 's3://bucket/a'",
            parser_ast::BRIEKind::StreamStart,
        ),
        ("STOP BACKUP LOGS", parser_ast::BRIEKind::StreamStop),
        ("PAUSE BACKUP LOGS", parser_ast::BRIEKind::StreamPause),
        ("RESUME BACKUP LOGS", parser_ast::BRIEKind::StreamResume),
        (
            "PURGE BACKUP LOGS FROM 's3://bucket/a'",
            parser_ast::BRIEKind::StreamPurge,
        ),
        (
            "SHOW BACKUP LOGS STATUS",
            parser_ast::BRIEKind::StreamStatus,
        ),
        (
            "SHOW BACKUP LOGS METADATA FROM 's3://bucket/a'",
            parser_ast::BRIEKind::StreamMetaData,
        ),
        ("SHOW BR JOB 7", parser_ast::BRIEKind::ShowJob),
        ("SHOW BR JOB QUERY 8", parser_ast::BRIEKind::ShowQuery),
        ("CANCEL BR JOB 9", parser_ast::BRIEKind::CancelJob),
        (
            "SHOW BACKUP METADATA FROM 's3://bucket/a'",
            parser_ast::BRIEKind::ShowBackupMeta,
        ),
        (
            "RESTORE DATABASE a FROM 's3://bucket/a'",
            parser_ast::BRIEKind::Restore,
        ),
        (
            "RESTORE POINT FROM 's3://bucket/a'",
            parser_ast::BRIEKind::RestorePIT,
        ),
    ];
    for (sql, kind) in cases {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::BRIEStmt>()
            .unwrap();
        assert_eq!(statement.Kind, kind, "{sql}");
    }

    for sql in [
        "BACKUP TABLE a TO 'noop://' CHECKSUM_CONCURRENCY 4 COMPRESSION_LEVEL 4 IGNORE_STATS 1 COMPRESSION_TYPE 'lz4'",
        "RESTORE TABLE g FROM 'noop://' CONCURRENCY 40 CHECKSUM 0 ONLINE 1",
        "BACKUP LOGS TO 'noop://' START_TS = '20220304'",
        "RESTORE POINT FROM 'noop://log' FULL_BACKUP_STORAGE = 'noop://full' RESTORED_TS = '20230123'",
        "BACKUP DATABASE a TO 'noop://' SNAPSHOT = 2 DAY AGO",
    ] {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(yyParse(&mut lexer, &mut parser_state), 0, "{sql}");
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        let statement = parser_state.result[0]
            .as_any()
            .downcast_ref::<parser_ast::BRIEStmt>()
            .unwrap();
        assert!(!statement.Options.is_empty(), "{sql}");
    }
}

/// 补充用例：parser_3_builds_create_view_like_go。
#[test]
fn parser_3_builds_create_view_like_go() {
    let sql = "CREATE OR REPLACE ALGORITHM = MERGE SQL SECURITY INVOKER VIEW db.v (a) AS SELECT 1 AS a WITH LOCAL CHECK OPTION";
    let mut lexer = Scanner::default();
    lexer.reset(sql.to_owned());
    let mut parser_state = Parser::default();
    assert_eq!(yyParse(&mut lexer, &mut parser_state), 0);
    assert!(parser_state.allStatementsSemanticallyComplete);
    let view = parser_state.result[0]
        .as_any()
        .downcast_ref::<parser_ast::CreateViewStmt>()
        .unwrap();
    assert!(view.OrReplace);
    assert_eq!(view.Algorithm, parser_ast::ViewAlgorithm::Merge);
    assert_eq!(view.Security, parser_ast::ViewSecurity::Invoker);
    assert_eq!(view.ViewName.Schema.O, "db");
    assert_eq!(view.Cols[0].O, "a");
    assert_eq!(view.CheckOption, parser_ast::ViewCheckOption::Local);
    assert!(view.Select.as_any().is::<parser_ast::SelectStmt>());
}

/// 补充用例：parser_3_builds_grant_load_and_import_nodes_like_go。
#[test]
fn parser_3_builds_grant_load_and_import_nodes_like_go() {
    let cases: [(&str, fn(&dyn parser_ast::Node)); 4] = [
        ("GRANT SELECT ON db.t TO 'u'@'%'", |node| {
            assert!(node.as_any().is::<parser_ast::GrantStmt>())
        }),
        ("REVOKE SELECT ON db.t FROM 'u'@'%'", |node| {
            assert!(node.as_any().is::<parser_ast::RevokeStmt>())
        }),
        ("LOAD DATA INFILE '/tmp/a.csv' INTO TABLE t", |node| {
            assert!(node.as_any().is::<parser_ast::LoadDataStmt>())
        }),
        ("IMPORT INTO t FROM '/tmp/a.csv'", |node| {
            assert!(node.as_any().is::<parser_ast::ImportIntoStmt>())
        }),
    ];
    for (sql, check) in cases {
        let mut lexer = Scanner::default();
        lexer.reset(sql.to_owned());
        let mut parser_state = Parser::default();
        assert_eq!(
            yyParse(&mut lexer, &mut parser_state),
            0,
            "{sql}: {:?}",
            lexer.Errors().1
        );
        assert!(parser_state.allStatementsSemanticallyComplete, "{sql}");
        assert_eq!(parser_state.result.len(), 1, "{sql}");
        check(parser_state.result[0].as_ref());
    }
}

/// LOAD DATA 的 IGNORE LINES 数量从可选语义值写入 AST。
#[test]
fn parser_3_load_data_preserves_ignore_lines() {
    let statement = Parser::default()
        .ParseOneStmt(
            "LOAD DATA LOCAL INFILE '/tmp/a.csv' INTO TABLE t IGNORE 11 LINES",
            "",
            "",
        )
        .unwrap();
    let load = statement
        .as_any()
        .downcast_ref::<parser_ast::LoadDataStmt>()
        .unwrap();
    assert_eq!(load.IgnoreLines, Some(11));
}

/// 补充用例：parser_3_rejects_go_invalid_semantic_cases。
#[test]
fn parser_3_rejects_go_invalid_semantic_cases() {
    for sql in [
        "REPLACE INTO t VALUES (1,2) AS new;",
        "CREATE TABLE foo (name CHAR(50) COLLATE ascii_bin COLLATE latin1_bin)",
        "CREATE TABLE t1 (a INT) PARTITION BY RANGE (a)",
        "CREATE TABLE t1 (a INT) PARTITION BY HASH (a) (PARTITION x VALUES LESS THAN (10))",
        "DROP INDEX idx ON t LOCK = lock_type",
        "CREATE TABLE t (a TIMESTAMP, b TIMESTAMP AS (a) NOT NULL ON UPDATE CURRENT_TIMESTAMP)",
        r#"SELECT "abc_" LIKE "abc\\_" ESCAPE '||'"#,
    ] {
        assert!(New().Parse(sql, "", "").is_err(), "{sql}");
    }
}
