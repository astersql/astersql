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
    for sql in [
        "SELECT \"Foo\" FROM public.t",
        "SELECT foo FROM public.\"Foo\"",
    ] {
        assert_eq!(rewrite(sql, "db", true).unwrap_err().0, "0A000");
    }
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
