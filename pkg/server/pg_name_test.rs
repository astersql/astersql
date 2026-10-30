// Copyright 2026 AsterSQL.
use crate::pg_name::rewrite;

#[test]
fn pg_introspection_names_token_scopes() {
    for (sql, expected) in [
        (
            "SELECT 'FROM public.fake' FROM public.t /* JOIN other.x */ AS a",
            "SELECT 'FROM public.fake' FROM `db`.t /* JOIN other.x */ AS a",
        ),
        (
            "SELECT \"a\".\"id\" FROM \"public\".\"t\" AS \"a\"",
            "SELECT \"a\".\"id\" FROM `db`.\"t\" AS \"a\"",
        ),
        (
            "SELECT * FROM t a, public.u b JOIN db.public.v c ON b.id = c.id",
            "SELECT * FROM `db`.t a, `db`.u b JOIN `db`.v c ON b.id = c.id",
        ),
        (
            "WITH t AS (SELECT * FROM public.t) SELECT * FROM t JOIN public.t AS original ON t.id = original.id",
            "WITH t AS (SELECT * FROM `db`.t) SELECT * FROM t JOIN `db`.t AS original ON t.id = original.id",
        ),
        (
            "SELECT * FROM (WITH t AS (SELECT * FROM public.u) SELECT * FROM t) q JOIN t outer_t ON q.id = outer_t.id",
            "SELECT * FROM (WITH t AS (SELECT * FROM `db`.u) SELECT * FROM t) q JOIN `db`.t outer_t ON q.id = outer_t.id",
        ),
        (
            "SELECT extract(year FROM '2020-01-01') FROM t",
            "SELECT extract(year FROM '2020-01-01') FROM `db`.t",
        ),
        (
            "INSERT INTO public.t SELECT * FROM public.u",
            "INSERT INTO `db`.t SELECT * FROM `db`.u",
        ),
        (
            "UPDATE public.t SET note = 'JOIN other.t'",
            "UPDATE `db`.t SET note = 'JOIN other.t'",
        ),
        (
            "DELETE FROM db.public.t WHERE id = 1",
            "DELETE FROM `db`.t WHERE id = 1",
        ),
        (
            "CREATE TABLE IF NOT EXISTS public.t (id INT)",
            "CREATE TABLE IF NOT EXISTS `db`.t (id INT)",
        ),
        (
            "ALTER TABLE public.t RENAME TO public.u",
            "ALTER TABLE `db`.t RENAME TO `db`.u",
        ),
        (
            "DROP TABLE IF EXISTS public.t, public.u",
            "DROP TABLE IF EXISTS `db`.t, `db`.u",
        ),
        ("DROP VIEW public.v", "DROP VIEW `db`.v"),
        ("TRUNCATE TABLE public.t", "TRUNCATE TABLE `db`.t"),
        (
            "SELECT * FROM public.\"a\"\"b\"",
            "SELECT * FROM `db`.\"a\"\"b\"",
        ),
        (
            "SELECT NULL -- FROM other.t\n",
            "SELECT NULL -- FROM other.t\n",
        ),
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap(), expected, "{sql}");
    }
    assert_eq!(
        rewrite("SELECT * FROM public.t", "db", false).unwrap(),
        "SELECT * FROM `db`.t"
    );
    assert_eq!(
        rewrite("WITH t AS (SELECT 1) SELECT * FROM t", "db", false).unwrap(),
        "WITH t AS (SELECT 1) SELECT * FROM t"
    );
}

#[test]
fn pg_introspection_names_rejection_boundaries() {
    for (sql, db, public, state) in [
        ("SELECT * FROM other.public.t", "db", true, "0A000"),
        ("SELECT * FROM other.t", "db", true, "0A000"),
        ("SELECT * FROM db.private.t", "db", true, "0A000"),
        ("SELECT * FROM \"PUBLIC\".t", "db", true, "0A000"),
        ("SELECT * FROM public.t.extra", "db", true, "0A000"),
        ("SELECT * FROM public.\"\"", "db", true, "42601"),
        ("SELECT * FROM public.\"broken", "db", true, "42601"),
        ("SELECT * FROM public.t", "", true, "3D000"),
        ("SELECT * FROM t", "db", false, "42P01"),
        ("SELECT * FROM `other`.t", "db", true, "0A000"),
        ("SELECT $$FROM other.t$$", "db", true, "0A000"),
        (
            "WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t",
            "db",
            true,
            "0A000",
        ),
        ("SELECT * FROM t /* unfinished", "db", true, "42601"),
        ("SELECT * FROM t /* nested /* x */ */", "db", true, "0A000"),
    ] {
        assert_eq!(rewrite(sql, db, public).unwrap_err().0, state, "{sql}");
    }
}

#[test]
fn pg_dialect_13_name_boundary() {
    let sql = "/*中*/ SELECT \"a\"\"b\", 'Foo \"a\"\"b\"', $2 FROM \"public\".\"t\" /* \"Foo\" */";
    assert_eq!(
        rewrite(sql, "db", true).unwrap(),
        "/*中*/ SELECT \"a\"\"b\", 'Foo \"a\"\"b\"', $2 FROM `db`.\"t\" /* \"Foo\" */"
    );
    assert_eq!(
        rewrite("SELECT foo AS \"Foo\" FROM public.t", "db", true).unwrap(),
        "SELECT foo AS \"Foo\" FROM `db`.t"
    );
    assert_eq!(
        rewrite("CREATE TABLE public.\"Foo\" (id int)", "db", true).unwrap(),
        "CREATE TABLE `db`.\"Foo\" (id int)"
    );
    for sql in ["SELECT \"Foo\" FROM public.t"] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "0A000");
    }
}

#[test]
fn pg_dialect_06_delete_using_name_adaptation() {
    let sql = "DELETE FROM public.t AS a USING public.s AS b WHERE a.id=b.id RETURNING a.id, b.id AS source_id";
    assert_eq!(
        rewrite(sql, "db", true).unwrap(),
        "DELETE FROM `db`.t AS a USING `db`.s AS b WHERE a.id=b.id RETURNING a.id, b.id AS source_id"
    );
    let unaliased = "DELETE FROM public.t USING public.s WHERE public.t.id=public.s.id RETURNING public.t.id, public.s.id AS source_id";
    assert_eq!(
        rewrite(unaliased, "db", true).unwrap(),
        "DELETE FROM `db`.t USING `db`.s WHERE public.t.id=public.s.id RETURNING public.t.id, public.s.id AS source_id"
    );
}

#[test]
fn pg_dialect_14_behavior() {
    use astersql_parser_ast as ast;
    let mut assertions = 0;
    for (relation, parts) in [
        ("t", 1),
        ("public.t", 2),
        ("db.public.t", 3),
        ("\"db\".\"public\".\"t\"", 3),
    ] {
        let sql =
            format!("/*中*/ SELECT $2, 'other.public.t' FROM {relation} /* db.public.t.extra */");
        let stmt = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseOneStmt(&sql, "", "")
            .unwrap();
        let select = stmt.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let ast::ResultSetNode::TableSource(source) = select
            .From
            .as_ref()
            .unwrap()
            .TableRefs
            .Left
            .as_deref()
            .unwrap()
        else {
            panic!("relation AST")
        };
        assert_eq!(source.Source.PostgreSQLParts.len(), parts);
        assertions += 1;
        assert_eq!(source.Source.Name.O, "t");
        assertions += 1;
        assert_eq!(
            source.Source.Schema.O,
            if parts > 1 { "public" } else { "" }
        );
        assertions += 1;
        for part in &source.Source.PostgreSQLParts {
            assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
            assertions += 1;
            assert_eq!(part.Quoted, relation.starts_with('"'));
            assertions += 1;
        }
        assert_eq!(
            select.Fields.Fields[0]
                .Expr
                .as_ref()
                .unwrap()
                .PostgreSQLParameterIndex(),
            Some(2)
        );
        assertions += 1;
        let adapted = rewrite(&sql, "db", true).unwrap();
        assert!(
            adapted.contains("'other.public.t'") && adapted.ends_with("/* db.public.t.extra */")
        );
        assertions += 1;
        assert_eq!(
            rewrite(&format!("SELECT id FROM {relation}"), "db", true).unwrap(),
            format!(
                "SELECT id FROM `db`.{}",
                if relation.starts_with('"') {
                    "\"t\""
                } else {
                    "t"
                }
            )
        );
        assertions += 1;
    }
    for (sql, state) in [
        ("SELECT id FROM other.public.t", "0A000"),
        ("SELECT id FROM other.t", "0A000"),
        ("SELECT id FROM public.", "42601"),
        ("SELECT id FROM db.public.t.extra", "42601"),
        ("SELECT id FROM \"DB\".public.t", "0A000"),
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, state, "{sql}");
        assertions += 1;
    }
    assert_eq!(
        rewrite("SELECT id FROM t", "db", false).unwrap_err().0,
        "42P01"
    );
    assertions += 1;
    assert!(rewrite("SELECT id FROM db.public.t", "db", false).is_ok());
    assertions += 1;
    assert!(
        astersql_parser::New()
            .ParseOneStmt("SELECT id FROM db.public.t", "", "")
            .is_err()
    );
    assertions += 1;
    for sql in [
        "SELECT id FROM other.t",
        "SELECT id FROM other.public.t",
        "SELECT id FROM db.public.t JOIN other.public.u ON t.id=u.id",
    ] {
        assert_eq!(
            crate::pg_name::validate_relations(sql, "db").unwrap_err().0,
            "0A000"
        );
        assertions += 1;
    }
    for sql in ["SELECT id FROM db.public.t", "SELECT id FROM db.t"] {
        assert!(crate::pg_name::validate_relations(sql, "db").is_ok());
        assertions += 1;
    }
    println!(
        "pg_dialect_14_behavior: assertions={assertions}, primary_cases=7; quoted/path/MySQL guards separate"
    );
}

#[test]
fn pg_dialect_16_behavior() {
    use astersql_parser_ast as ast;
    let mut assertions = 0;
    for sql in [
        "SELECT a.id FROM public.t AS a",
        "SELECT a.missing FROM public.t AS a",
        "SELECT a.id,b.id FROM public.t AS a JOIN public.t AS b ON a.id=b.id",
        "SELECT \"A\".id FROM public.t AS \"A\"",
        "/*中*/ SELECT a.id,$2,'t.id' FROM public.t a /* t.id */",
    ] {
        let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
        let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let ast::ExprKind::Column(column) = &expr.Kind else {
            panic!("column AST")
        };
        assert_eq!(column.PostgreSQLParts.len(), 2);
        assertions += 1;
        for part in &column.PostgreSQLParts {
            assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
            assertions += 1;
        }
        assert!(rewrite(sql, "db", true).is_ok(), "{sql}");
        assertions += 1;
    }
    for sql in [
        "SELECT t.id FROM public.t AS a",
        "SELECT a.id FROM public.t AS \"A\"",
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "42P01", "{sql}");
        assertions += 1;
    }
    let sql = "/*中*/ SELECT a.id,$2,'t.id' FROM public.t a /* t.id */";
    let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .unwrap();
    let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
    assert_eq!(
        select.Fields.Fields[1]
            .Expr
            .as_ref()
            .unwrap()
            .PostgreSQLParameterIndex(),
        Some(2)
    );
    assertions += 1;
    let adapted = rewrite(sql, "db", true).unwrap();
    assert!(adapted.contains("'t.id'") && adapted.ends_with("/* t.id */"));
    assertions += 1;
    assert_eq!(
        rewrite(
            "SELECT a.id FROM public.t a JOIN public.t AS \"A\" ON true",
            "db",
            true
        )
        .unwrap_err()
        .0,
        "0A000"
    );
    assertions += 1;
    println!(
        "pg_dialect_16_behavior: assertions={assertions}; 5 primary shapes, visibility/case guards"
    );
}

#[test]
fn pg_dialect_18_behavior() {
    use astersql_parser_ast as ast;
    let cases = [
        (
            "WITH t AS (SELECT 1 AS v) SELECT v FROM t",
            "WITH t AS (SELECT 1 AS v) SELECT v FROM t",
            vec!["t"],
        ),
        (
            "WITH t AS (SELECT 1 AS v) SELECT v FROM public.t",
            "WITH t AS (SELECT 1 AS v) SELECT v FROM `db`.t",
            vec!["t"],
        ),
        (
            "WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b",
            "WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b",
            vec!["a", "b"],
        ),
        (
            "WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a",
            "WITH a AS (SELECT v FROM `db`.b), b AS (SELECT 2 AS v) SELECT v FROM a",
            vec!["a", "b"],
        ),
        (
            "WITH t AS (SELECT v FROM t) SELECT v FROM t",
            "WITH t AS (SELECT v FROM `db`.t) SELECT v FROM t",
            vec!["t"],
        ),
        (
            "/*中*/ WITH t AS (SELECT $2::int4 AS v) SELECT v FROM t /* public.t $1 */",
            "/*中*/ WITH t AS (SELECT $2::int4 AS v) SELECT v FROM t /* public.t $1 */",
            vec!["t"],
        ),
    ];
    let mut assertions = 0;
    for (sql, expected, names) in cases {
        let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
        let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let with = select.With.as_ref().unwrap().borrow();
        assert!(!with.IsRecursive);
        assert_eq!(
            with.CTEs
                .iter()
                .map(|cte| cte.Name.O.as_str())
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(rewrite(sql, "db", true).unwrap(), expected);
        assertions += 3;
        for cte in &with.CTEs {
            assert!(cte.Query.as_any().is::<ast::SelectStmt>());
            assertions += 1;
        }
    }
    let sql = "/*中*/ WITH t AS (SELECT $2::int4 AS v) SELECT t.v FROM t /* public.t $1 */";
    let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .unwrap();
    let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
    let with = select.With.as_ref().unwrap().borrow();
    let query = with.CTEs[0]
        .Query
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap();
    let ast::ExprKind::Cast { Expr, .. } = &query.Fields.Fields[0].Expr.as_ref().unwrap().Kind
    else {
        panic!("cast")
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(2));
    let ast::ExprKind::Column(column) = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind else {
        panic!("column")
    };
    for part in &column.PostgreSQLParts {
        assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
        assertions += 1;
    }
    assert!(rewrite(sql, "db", false).is_ok());
    assert_eq!(
        rewrite(
            "WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a",
            "db",
            false
        )
        .unwrap_err()
        .0,
        "42P01"
    );
    assert!(
        astersql_parser::New()
            .ParseOneStmt("WITH t AS (SELECT 1 AS v) SELECT v FROM t", "", "")
            .is_ok()
    );
    assertions += 4;
    println!(
        "pg_dialect_18_behavior: primary_shapes=6 assertions={assertions}; binding values verified separately by libpq"
    );
}

#[test]
fn pg_dialect_24_behavior() {
    use astersql_parser_ast::{CastFunctionType, ExprKind, SelectStmt, SqlDialect};
    let mut pg = astersql_parser::Parser::with_dialect(SqlDialect::PostgreSQL);
    let mut mysql = astersql_parser::Parser::with_dialect(SqlDialect::MySQL);
    let sql = "/*中*/ SELECT $2::int4 AS \"v\", '$1 ::int8' /* $3 */";
    let mut checks = 0;
    // Each instance is reused after errors and Reset while the other stays live.
    for _ in 0..3 {
        assert!(pg.ParseOneStmt("/*中*/ SELECT )", "", "").is_err());
        checks += 1;
        assert_eq!(pg.PostgreSQLSyntaxErrorPosition(), Some(14));
        checks += 1;
        assert_eq!(mysql.PostgreSQLSyntaxErrorPosition(), None);
        checks += 1;
        mysql.Reset();
        assert_eq!(mysql.sql_dialect(), SqlDialect::MySQL);
        checks += 1;
        let stmt = mysql
            .ParseOneStmt("SELECT CAST(1 AS SIGNED)", "", "")
            .unwrap();
        let select = stmt.as_any().downcast_ref::<SelectStmt>().unwrap();
        assert!(matches!(
            select.Fields.Fields[0].Expr.as_ref().unwrap().Kind,
            ExprKind::Cast {
                FunctionType: CastFunctionType::Cast,
                ..
            }
        ));
        checks += 1;
        assert!(mysql.ParseOneStmt("SELECT 1::int4", "", "").is_err());
        checks += 1;
        pg.Reset();
        assert_eq!(pg.sql_dialect(), SqlDialect::PostgreSQL);
        checks += 1;
        let stmt = pg.ParseOneStmt(sql, "", "").unwrap();
        assert_eq!(pg.PostgreSQLSyntaxErrorPosition(), None);
        checks += 1;
        let select = stmt.as_any().downcast_ref::<SelectStmt>().unwrap();
        let expr = select.Fields.Fields[0].Expr.as_ref().unwrap();
        let ExprKind::Cast {
            Expr,
            FunctionType,
            Tp,
            ..
        } = &expr.Kind
        else {
            panic!("PG cast tree: {expr:?}");
        };
        assert_eq!(*FunctionType, CastFunctionType::PostgreSQLInt4);
        assert_eq!(Tp.GetType(), astersql_parser_mysql::r#type::TypeLong);
        assert_eq!(Expr.PostgreSQLParameterIndex(), Some(2));
        assert_eq!(expr.node_text.OriginalText(), b"$2::int4");
        assert_eq!(&sql[expr.OriginTextPosition as usize..][..8], "$2::int4");
        assert_eq!(select.Fields.Fields[0].AsName.O, "v");
        let literal = select.Fields.Fields[1].Expr.as_ref().unwrap();
        assert!(matches!(&literal.Kind, ExprKind::Value(value) if value == "$1 ::int8"));
        assert_eq!(literal.PostgreSQLParameterIndex(), None);
        checks += 8;
    }
    assert_eq!(checks, 48);
    println!("pg_dialect_24_behavior: {checks} semantic assertions; 3 reuse/error/reset cycles");
}

#[test]
fn pg_dialect_15_behavior() {
    use astersql_parser_ast as ast;
    use astersql_session::testutil::TestRecordSet;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let pg = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain.clone(),
        ast::SqlDialect::PostgreSQL,
    );
    pg.execute("CREATE DATABASE pg_name_exact").unwrap();
    pg.execute("USE pg_name_exact").unwrap();
    for sql in [
        "CREATE TABLE foo(id int4)",
        "CREATE TABLE \"Foo\"(id int4)",
        "CREATE TABLE \"a\"\"b\"(id int4)",
        "INSERT INTO foo VALUES (1)",
        "INSERT INTO \"Foo\" VALUES (2)",
        "INSERT INTO \"a\"\"b\" VALUES (3)",
    ] {
        pg.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    pg.execute("BEGIN").unwrap();
    for (sql, expected) in [
        ("SELECT id FROM foo", "1"),
        ("SELECT id FROM Foo", "1"),
        ("SELECT id FROM \"foo\"", "1"),
        ("SELECT id FROM \"Foo\"", "2"),
        ("SELECT id FROM \"a\"\"b\"", "3"),
        ("SELECT a.id FROM \"Foo\" a WHERE a.id=2::int4", "2"),
    ] {
        let parsed = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseOneStmt(sql, "", "")
            .unwrap();
        let select = parsed.as_any().downcast_ref::<ast::SelectStmt>().unwrap();
        let ast::ResultSetNode::TableSource(source) = select
            .From
            .as_ref()
            .unwrap()
            .TableRefs
            .Left
            .as_deref()
            .unwrap()
        else {
            panic!("formal relation AST")
        };
        let part = source.Source.PostgreSQLParts.last().unwrap();
        assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
        assert_eq!(
            part.Lookup,
            if expected == "1" {
                "foo"
            } else if expected == "2" {
                "Foo"
            } else {
                "a\"b"
            }
        );
        let adapted = rewrite(sql, "pg_name_exact", true).unwrap();
        let mut rows = pg.execute(&adapted).unwrap().remove(0);
        assert_eq!(
            rows.Next().unwrap(),
            Some(vec![expected.to_owned()]),
            "{sql}"
        );
        assert_eq!(rows.Next().unwrap(), None);
    }
    pg.execute("COMMIT").unwrap();
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg_name_exact"))
        .unwrap();
    let lower = tables.iter().find(|t| t.name.original == "foo").unwrap();
    let upper = tables.iter().find(|t| t.name.original == "Foo").unwrap();
    assert_ne!(lower.id, upper.id);
    for table in [lower, upper] {
        let model = table.model_meta.as_ref().unwrap();
        let encoded = astersql_meta_model::EncodeTableInfo(model).unwrap();
        let restored = astersql_meta_model::DecodeTableInfo(&encoded).unwrap();
        assert!(restored.ExactName);
        assert_eq!(restored.ID, table.id);
        assert_eq!(restored.Name.O, table.name.original);
    }
    let mysql = astersql_session::runtime::ConcreteSession::new(domain);
    mysql.execute("USE pg_name_exact").unwrap();
    let mut rows = mysql.execute("SELECT id FROM Foo").unwrap().remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["1".to_owned()]));
    pg.execute("DROP TABLE \"Foo\"").unwrap();
    assert!(pg.execute("SELECT id FROM \"Foo\"").is_err());
    let mut rows = pg.execute("SELECT id FROM foo").unwrap().remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["1".to_owned()]));
    pg.execute("CREATE TABLE \"Foo\"(id int4)").unwrap();
    pg.execute("INSERT INTO \"Foo\" VALUES(2)").unwrap();
    pg.execute("DROP TABLE foo").unwrap();
    let mut rows = mysql.execute("SELECT id FROM Foo").unwrap().remove(0);
    assert_eq!(rows.Next().unwrap(), Some(vec!["2".to_owned()]));
    mysql.execute("DROP TABLE Foo").unwrap();
    assert!(pg.execute("SELECT id FROM \"Foo\"").is_err());
    pg.execute("DROP DATABASE pg_name_exact").unwrap();
    assert_eq!(
        rewrite(
            "WITH x AS (SELECT 1) SELECT id FROM \"Foo\"",
            "pg_name_exact",
            true
        )
        .unwrap(),
        "WITH x AS (SELECT 1) SELECT id FROM `pg_name_exact`.\"Foo\""
    );
    assert_eq!(
        rewrite(
            "CREATE TEMPORARY TABLE \"Foo\"(id int4)",
            "pg_name_exact",
            true
        )
        .unwrap_err()
        .0,
        "0A000"
    );
    println!("pg_dialect_15_behavior: exact identities, six reads, MySQL isolation, exact drop");
}

#[test]
fn pg_dialect_17_behavior() {
    use astersql_parser_ast as ast;
    let mut checks = 0;
    for (sql, expected) in [
        (
            "INSERT INTO public.t AS a(id,v) VALUES ($1,$2) ON CONFLICT(id) DO UPDATE SET v=excluded.v WHERE a.v IS NULL RETURNING a.id",
            None,
        ),
        (
            "INSERT INTO public.excluded AS a(id,v) VALUES (1,NULL) ON CONFLICT(id) DO UPDATE SET v=excluded.v RETURNING a.id",
            None,
        ),
        (
            "INSERT INTO public.t AS excluded(id,v) VALUES (1,NULL) ON CONFLICT(id) DO UPDATE SET v=excluded.v",
            Some("42P09"),
        ),
        ("SELECT excluded.v", Some("42P01")),
        (
            "INSERT INTO public.t(id,v) VALUES (1,NULL) ON CONFLICT(id) DO UPDATE SET v=excluded.v RETURNING excluded.v",
            Some("42P01"),
        ),
        (
            "INSERT INTO public.t AS a(id,v) VALUES (1,NULL) ON CONFLICT(id) DO UPDATE SET v=t.v",
            Some("42P01"),
        ),
    ] {
        let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
        if let Some(insert) = nodes[0].as_any().downcast_ref::<ast::InsertStmt>() {
            let clause = insert.PostgreSQLConflict.as_ref().unwrap();
            assert_eq!(
                &sql[clause.SourceSpan.0..clause.SourceSpan.1],
                clause.OriginalText
            );
            assert_eq!(clause.Action, ast::PostgreSQLConflictAction::DoUpdate);
            let ast::ExprKind::Column(column) = &clause.UpdateAssignments[0].Expr.Kind else {
                panic!("qualified RHS")
            };
            assert_eq!(column.PostgreSQLParts.len(), 2);
            for part in &column.PostgreSQLParts {
                assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
                checks += 1;
            }
            checks += 3;
        }
        match expected {
            Some(code) => assert_eq!(rewrite(sql, "db", true).unwrap_err().0, code, "{sql}"),
            None => {
                rewrite(sql, "db", true).unwrap();
            }
        }
        checks += 1;
    }
    let quoted = r#"INSERT INTO public.t AS "A"(id,v) VALUES ($1,$2) ON CONFLICT(id) DO UPDATE SET v=excluded."v" WHERE "A".id=$1 RETURNING "A".id"#;
    let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(quoted, &[])
        .unwrap();
    let insert = nodes[0].as_any().downcast_ref::<ast::InsertStmt>().unwrap();
    let clause = insert.PostgreSQLConflict.as_ref().unwrap();
    let ast::ExprKind::Column(column) = &clause.UpdateAssignments[0].Expr.Kind else {
        panic!("quoted RHS")
    };
    assert!(column.PostgreSQLParts[1].Quoted);
    assert_eq!(column.PostgreSQLParts[1].Original, "\"v\"");
    assert_eq!(column.PostgreSQLParts[1].Lookup, "v");
    assert!(rewrite(quoted, "db", true).is_ok());
    checks += 4;
    let sql = "/*中*/ INSERT INTO public.t AS a(id,v) VALUES ($2,'excluded.v') ON CONFLICT(id) DO UPDATE SET v=excluded.v WHERE a.id=$1 /* excluded.missing */";
    let rewritten = rewrite(sql, "db", true).unwrap();
    assert!(
        rewritten.contains("$2,'excluded.v'")
            && rewritten.contains("a.id=$1")
            && rewritten.ends_with("/* excluded.missing */")
    );
    assert!(
        astersql_parser::New()
            .ParseSQL(
                "INSERT INTO t(id) VALUES (1) ON CONFLICT(id) DO UPDATE SET id=excluded.id",
                &[]
            )
            .is_err()
    );
    checks += 2;
    println!(
        "pg_dialect_17_behavior: {checks} assertions; 6 primary binding shapes plus source/parameter/dialect guards"
    );
}

#[test]
fn pg_dialect_19_behavior() {
    use astersql_parser_ast as ast;
    let sql = "WITH t AS (SELECT 1 AS v) SELECT t.v,q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q";
    let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .unwrap();
    let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
    assert_eq!(select.With.as_ref().unwrap().borrow().CTEs[0].Name.O, "t");
    assert_eq!(select.Fields.Fields.len(), 2);
    assert_eq!(rewrite(sql, "db", false).unwrap(), sql);
    #[derive(Default)]
    struct Blocks(Vec<String>);
    impl ast::Visitor for Blocks {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>()
                && let Some(with) = &select.With
            {
                self.0
                    .extend(with.borrow().CTEs.iter().map(|cte| cte.Name.L.clone()));
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut blocks = Blocks::default();
    ast::Node::accept(select, &mut blocks);
    assert_eq!(blocks.0, vec!["t", "t"]);
    let release = "SELECT t.v,q.v FROM (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q CROSS JOIN t";
    assert_eq!(
        rewrite(release, "db", true).unwrap(),
        "SELECT t.v,q.v FROM (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q CROSS JOIN `db`.t"
    );
    assert_eq!(
        rewrite("SELECT v FROM t", "db", true).unwrap(),
        "SELECT v FROM `db`.t"
    );
    assert_eq!(
        rewrite("SELECT v FROM t", "db", false).unwrap_err().0,
        "42P01"
    );
    let quoted = "WITH \"T\" AS (SELECT 1 AS v), t AS (SELECT 2 AS v) SELECT \"T\".v,t.v FROM \"T\" CROSS JOIN t";
    assert!(
        astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
            .ParseSQL(quoted, &[])
            .is_ok()
    );
    assert!(rewrite(quoted, "db", false).is_ok());
    let (nodes, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(quoted, &[])
        .unwrap();
    let select = nodes[0].as_any().downcast_ref::<ast::SelectStmt>().unwrap();
    assert_eq!(
        select
            .With
            .as_ref()
            .unwrap()
            .borrow()
            .CTEs
            .iter()
            .map(|cte| cte.Name.L.as_str())
            .collect::<Vec<_>>(),
        vec!["T", "t"]
    );
    for (field, expected) in select.Fields.Fields.iter().zip(["T", "t"]) {
        let ast::ExprKind::Column(column) = &field.Expr.as_ref().unwrap().Kind else {
            panic!("column")
        };
        assert_eq!(column.Table.L, expected);
        for part in &column.PostgreSQLParts {
            assert_eq!(&quoted[part.Span.0..part.Span.1], part.Original);
        }
    }
    println!(
        "pg_dialect_19_behavior: assertions=16; actual bindings and lifecycle verified by libpq"
    );
}

#[test]
fn pg_dialect_20_behavior() {
    for sql in [
        "WITH t AS (SELECT 1), t AS (SELECT 2) SELECT * FROM t",
        "WITH t AS (SELECT 1), T AS (SELECT 2) SELECT * FROM t",
        r#"/*中文*/ WITH t AS (SELECT 1), "t" AS (SELECT 2) SELECT * FROM t"#,
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "42712", "{sql}");
    }
    for sql in [
        r#"WITH t AS (SELECT 1 AS v), "T" AS (SELECT 2 AS v) SELECT t.v,"T".v FROM t CROSS JOIN "T""#,
        "WITH t AS (SELECT 1 AS v) SELECT t.v,q.v FROM t CROSS JOIN (WITH t AS (SELECT 2 AS v) SELECT v FROM t) q",
    ] {
        assert_eq!(rewrite(sql, "db", false).unwrap(), sql);
        let (nodes, _) =
            astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
                .ParseSQL(sql, &[])
                .unwrap();
        let select = nodes[0]
            .as_any()
            .downcast_ref::<astersql_parser_ast::SelectStmt>()
            .unwrap();
        let with = select.With.as_ref().unwrap().borrow();
        assert_eq!(with.CTEs[0].Name.L, "t");
        assert!(
            with.CTEs[0]
                .Query
                .as_any()
                .is::<astersql_parser_ast::SelectStmt>()
        );
        if with.CTEs.len() == 2 {
            assert_eq!(with.CTEs[1].Name.L, "T");
        }
        for field in &select.Fields.Fields {
            let astersql_parser_ast::ExprKind::Column(column) = &field.Expr.as_ref().unwrap().Kind
            else {
                panic!("column");
            };
            for part in &column.PostgreSQLParts {
                assert_eq!(&sql[part.Span.0..part.Span.1], part.Original);
            }
        }
    }
    println!(
        "pg_dialect_20_behavior: cases=5; duplicate diagnostics, shared AST identities, query bodies and source spans verified"
    );
}

#[test]
fn pg_dialect_21_behavior() {
    for sql in [
        "WITH RECURSIVE t AS (SELECT 1) SELECT * FROM t",
        "WITH RECURSIVE a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<2) SELECT n FROM r ORDER BY n",
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "0A000");
        let (nodes, _) =
            astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
                .ParseSQL(sql, &[])
                .unwrap();
        let select = nodes[0]
            .as_any()
            .downcast_ref::<astersql_parser_ast::SelectStmt>()
            .unwrap();
        assert!(select.With.as_ref().unwrap().borrow().IsRecursive);
    }
    assert_eq!(
        rewrite(
            "WITH RECURSIVE r(n) AS (SELECT n FROM r UNION ALL SELECT 1) SELECT * FROM r",
            "db",
            true
        )
        .unwrap_err()
        .0,
        "42P19"
    );
    assert_eq!(
        rewrite(
            "WITH RECURSIVE t AS (SELECT 1), t AS (SELECT 2) SELECT * FROM t",
            "db",
            true
        )
        .unwrap_err()
        .0,
        "42712"
    );
    let sql =
        "WITH r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<2) SELECT n FROM r ORDER BY n";
    assert!(rewrite(sql, "db", true).unwrap().contains("FROM `db`.r"));
    for sql in [
        "WITH RECURSIVE r(n) AS (SELECT n FROM public.r UNION ALL SELECT 1) SELECT * FROM r",
        "WITH RECURSIVE r(n) AS (WITH r AS (SELECT 1 AS n) SELECT n FROM r UNION ALL SELECT 1) SELECT * FROM r",
        "WITH RECURSIVE r(n) AS (SELECT n FROM r() UNION ALL SELECT 1) SELECT * FROM r",
        "WITH RECURSIVE r(n) AS (SELECT n FROM \"R\" UNION ALL SELECT 1) SELECT * FROM r",
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "0A000");
    }
    println!(
        "pg_dialect_21_behavior: cases=6 guard_shapes=4; recursive AST, explicit gate, seed diagnostic, duplicate diagnostic and nonrecursive identity"
    );
}

#[test]
fn pg_dialect_22_behavior() {
    use crate::pg_session::SessionQuery;
    for sql in [
        "SET search_path TO missing, public",
        r#"SET search_path TO "$user", public"#,
        "SET search_path TO public, public, pg_catalog",
        r#"SET search_path TO "PUBLIC", public"#,
    ] {
        assert!(
            matches!(SessionQuery::parse(sql), Ok(Some(SessionQuery::Set(_)))),
            "{sql}"
        );
    }
    let sql = r#"/*中*/ SELECT $1::int4 FROM "t" /* public.t $1 */"#;
    assert_eq!(rewrite(sql, "db", false).unwrap_err().0, "42P01");
    let (statements, _) =
        astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
    assert_eq!(statements.len(), 1);
    use astersql_parser_ast as ast;
    let select = statements[0]
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .unwrap();
    let ast::ExprKind::Cast { Expr, .. } = &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
    else {
        panic!("original typed parameter");
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(1));
    struct Tables(Vec<Vec<ast::PostgreSQLIdentifier>>);
    impl ast::Visitor for Tables {
        fn enter(&mut self, _: &dyn ast::Node) -> bool {
            false
        }
        fn enter_table_name(&mut self, table: &ast::TableName) -> bool {
            self.0.push(table.PostgreSQLParts.clone());
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut tables = Tables(Vec::new());
    statements[0].accept(&mut tables);
    assert_eq!(tables.0.len(), 1);
    assert_eq!(tables.0[0].len(), 1);
    let part = &tables.0[0][0];
    assert_eq!(part.Lookup, "t");
    assert!(part.Quoted);
    assert_eq!(&sql[part.Span.0..part.Span.1], r#""t""#);

    assert!(
        rewrite(sql, "db", true)
            .unwrap()
            .contains(r#"$1::int4 FROM `db`."t" /* public.t $1 */"#)
    );
    assert_eq!(
        rewrite("WITH t AS (SELECT 1) SELECT * FROM t", "db", false).unwrap(),
        "WITH t AS (SELECT 1) SELECT * FROM t"
    );

    let meta = crate::pg_catalog::CatalogQuery::parse(
        "SELECT nspname AS visible_schema FROM pg_catalog.pg_namespace",
    )
    .unwrap()
    .unwrap()
    .metadata();
    assert_eq!(meta.catalog_column_origins, vec![Some((2615, 2))]);
    let mut result = crate::conn::QueryResult {
        columns: meta.columns,
        native_types: meta.native_types,
        catalog_column_origins: meta.catalog_column_origins,
        ..Default::default()
    };
    let encoded = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(encoded[0].0, b'T');
    result.catalog_column_origins = vec![Some((99999, 2))];
    assert!(
        crate::pg_result::encode(&result, "SELECT").is_err(),
        "unregistered catalog origins must fail"
    );
    result.catalog_column_origins = vec![Some((2615, 0))];
    assert!(
        crate::pg_result::encode(&result, "SELECT").is_err(),
        "invalid attribute must fail"
    );
    println!(
        "pg_dialect_22_behavior: path_inputs=4 binding_cases=4; positions, original parameters/comments and CTE precedence"
    );
}

#[test]
fn pg_dialect_23_behavior() {
    use astersql_parser::{Parser, ast::SqlDialect};
    let cases = [
        ("SELECT )", 8),
        ("/*中*/ SELECT )", 14),
        ("/*中*/ SELECT v FROM public.t WHERE )", 36),
        ("/*中*/ SELECT '未闭合", 14),
        ("/*中*/ SELECT \"未闭合", 14),
        ("/*中*/ SELECT ARRAY[1,", 22),
        ("/*中*/ SELECT ROW(1,", 20),
        ("/*中*/ SELECT 1 WHERE 'a' ILIKE", 31),
        ("INSERT INTO public.t VALUES (1) RETURNING", 42),
        ("INSERT INTO public.t VALUES (1) ON CONFLICT (v) DO", 51),
    ];
    for (sql, expected) in cases {
        let mut parser = Parser::with_dialect(SqlDialect::PostgreSQL);
        assert!(parser.ParseSQL(sql, &[]).is_err(), "{sql}");
        assert_eq!(
            parser.PostgreSQLSyntaxErrorPosition(),
            Some(expected),
            "{sql}"
        );
    }
    let sql = "/*中*/ SELECT $1::int4, 'public.t ''中'' $2' FROM public.t /* public.t $3 */";
    let (statements, _) = Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .unwrap();
    let select = statements[0]
        .as_any()
        .downcast_ref::<astersql_parser_ast::SelectStmt>()
        .unwrap();
    let astersql_parser_ast::ExprKind::Cast { Expr, .. } =
        &select.Fields.Fields[0].Expr.as_ref().unwrap().Kind
    else {
        panic!("typed parameter tree");
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(1));
    assert_eq!(&sql[Expr.OriginTextPosition as usize..][..2], "$1");
    let rewritten = rewrite(sql, "a_very_long_database_name", true).unwrap();
    assert!(rewritten.contains("'public.t ''中'' $2'"));
    assert!(rewritten.ends_with("/* public.t $3 */"));
    assert!(rewritten.contains("$1::int4"));
    println!("pg_dialect_23_behavior: syntax_cases=10 protected_rewrite=1");
}

#[test]
fn pg_dialect_25_behavior() {
    use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    fn send(s: &mut TcpStream, tag: u8, body: &[u8]) {
        s.write_all(&[tag]).unwrap();
        s.write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        s.write_all(body).unwrap();
    }
    fn read(s: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut h = [0; 5];
        s.read_exact(&mut h).unwrap();
        let mut b = vec![0; u32::from_be_bytes(h[1..].try_into().unwrap()) as usize - 4];
        s.read_exact(&mut b).unwrap();
        (h[0], b)
    }
    fn query(s: &mut TcpStream, sql: &str) {
        send(s, b'Q', &[sql.as_bytes(), b"\0"].concat());
        loop {
            let (tag, b) = read(s);
            assert_ne!(tag, b'E', "{sql}: {b:?}");
            if tag == b'Z' {
                assert_eq!(b, b"I");
                break;
            }
        }
    }
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.reanalysis25(id INT)")
        .unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = crate::pg_conn::PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    s.write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    s.write_all(&body).unwrap();
    while read(&mut s).0 != b'Z' {}
    let sql = "/*中*/ SELECT id,$1::int4,'public.reanalysis25 $2' FROM \"reanalysis25\"";
    let (stmts, _) =
        astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL)
            .ParseSQL(sql, &[])
            .unwrap();
    let select = stmts[0]
        .as_any()
        .downcast_ref::<astersql_parser_ast::SelectStmt>()
        .unwrap();
    let astersql_parser_ast::ExprKind::Cast { Expr, .. } =
        &select.Fields.Fields[1].Expr.as_ref().unwrap().Kind
    else {
        panic!("parameter cast")
    };
    assert_eq!(Expr.PostgreSQLParameterIndex(), Some(1));
    assert!(
        matches!(&select.Fields.Fields[2].Expr.as_ref().unwrap().Kind,astersql_parser_ast::ExprKind::Value(v) if v=="public.reanalysis25 $2")
    );
    let parse = [
        b"same\0".as_slice(),
        sql.as_bytes(),
        b"\0",
        &1u16.to_be_bytes(),
        &23u32.to_be_bytes(),
    ]
    .concat();
    send(&mut s, b'P', &parse);
    assert_eq!(read(&mut s), (b'1', vec![]));
    send(&mut s, b'S', &[]);
    assert_eq!(read(&mut s), (b'Z', b"I".to_vec()));
    query(&mut s, "SET search_path TO ''");
    send(&mut s, b'D', b"Ssame\0");
    send(&mut s, b'S', &[]);
    assert_eq!(
        read(&mut s),
        (
            b't',
            [1u16.to_be_bytes().as_slice(), &23u32.to_be_bytes()].concat()
        )
    );
    let (tag, error) = read(&mut s);
    assert_eq!(
        tag, b'E',
        "Describe must reanalyze original query, got {error:?}"
    );
    assert!(error.windows(6).any(|s| s == b"42P01\0"));
    let expected = format!("P{}\0", sql.chars().position(|c| c == '\"').unwrap() + 1);
    assert!(
        error
            .windows(expected.len())
            .any(|s| s == expected.as_bytes()),
        "{error:?}"
    );
    assert_eq!(read(&mut s), (b'Z', b"I".to_vec()));
    query(&mut s, "SELECT 1::int4");
    query(&mut s, "SET search_path TO public");
    send(&mut s, b'D', b"Ssame\0");
    send(&mut s, b'S', &[]);
    assert_eq!(read(&mut s).0, b't');
    assert_eq!(read(&mut s).0, b'T');
    assert_eq!(read(&mut s), (b'Z', b"I".to_vec()));
    send(&mut s, b'X', &[]);
    drop(s);
    service.close();
    native.execute("DROP TABLE test.reanalysis25").unwrap();
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg_dialect_25_behavior: same Statement Describe invalidation/restoration; original Unicode position, typed parameter/literal, recovery and listener cleanup"
    );
}

#[test]
fn pg08_38_relation_boundaries() {
    for sql in [
        "SELECT NULL::int4 IS DISTINCT FROM NULL::int4 AS v",
        "SELECT NULL::int4 IS NOT DISTINCT /* predicate */ FROM NULL::int4 AS v",
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap(), sql);
    }
    assert_eq!(
        rewrite("SELECT a IS DISTINCT FROM b AS v FROM public.t", "db", true).unwrap(),
        "SELECT a IS DISTINCT FROM b AS v FROM `db`.t"
    );
    assert_eq!(
        rewrite("SELECT 1 AS distinct FROM public.t", "db", true).unwrap(),
        "SELECT 1 AS distinct FROM `db`.t"
    );
}
