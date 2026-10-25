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
    for sql in [
        "WITH x AS (SELECT 1) SELECT id FROM \"Foo\"",
        "CREATE TEMPORARY TABLE \"Foo\"(id int4)",
    ] {
        assert_eq!(rewrite(sql, "pg_name_exact", true).unwrap_err().0, "0A000");
    }
    println!("pg_dialect_15_behavior: exact identities, six reads, MySQL isolation, exact drop");
}
