// Copyright 2026 AsterSQL.

#[test]
fn postgres_create_table_mapping() {
    let sql = r#"CREATE TABLE public."CaseSensitiveTable" (
        "small" smallint,
        "regular" integer,
        "large" bigint,
        "single" real,
        "double" double precision,
        "amount" numeric(12, 3) DEFAULT 1.250,
        "ratio" decimal(8, 2),
        "enabled" boolean NOT NULL DEFAULT true,
        "fixed" char(4),
        "varying" varchar(24) DEFAULT 'integer bytea',
        "body" text,
        "payload" bytea,
        "day" date,
        "clock" time(3),
        "stamp" timestamp(3) DEFAULT CURRENT_TIMESTAMP(3)
    )"#;
    let adapted = crate::pg_sql::adapt(sql).unwrap();
    for expected in [
        "`small` SMALLINT",
        "`regular` INT",
        "`large` BIGINT",
        "`single` FLOAT",
        "`double` DOUBLE",
        "`amount` DECIMAL(12, 3)",
        "`ratio` DECIMAL(8, 2)",
        "`enabled` BOOLEAN",
        "`fixed` CHAR(4)",
        "`varying` VARCHAR(24) DEFAULT 'integer bytea'",
        "`body` TEXT",
        "`payload` BLOB",
        "`day` DATE",
        "`clock` TIME(3)",
        "`stamp` TIMESTAMP(3)",
    ] {
        assert!(adapted.contains(expected), "missing {expected}: {adapted}");
    }
    assert!(adapted.starts_with("CREATE TABLE public.`CaseSensitiveTable`"));
}

#[test]
fn postgres_create_table_rejects_unsupported_identity_types() {
    for ty in ["serial", "bigserial"] {
        let error =
            crate::pg_sql::adapt(&format!("CREATE TABLE public.t (id {ty})")).expect_err(ty);
        assert_eq!(error.0, "0A000");
    }
    assert_eq!(
        crate::pg_sql::adapt("CREATE TABLE public.t (id integer")
            .unwrap_err()
            .0,
        "42601"
    );
    assert_eq!(
        crate::pg_sql::adapt("CREATE TABLE public.t (id integer /* open)")
            .unwrap_err()
            .0,
        "42601"
    );
}

#[test]
fn postgres_create_table_preserves_literals_and_comments() {
    let sql = "CREATE TABLE public.t (note varchar(20) DEFAULT 'serial bytea', /* integer */ payload bytea)";
    let adapted = crate::pg_sql::adapt(sql).unwrap();
    assert!(adapted.contains("DEFAULT 'serial bytea'"));
    assert!(adapted.contains("/* integer */"));
    assert!(adapted.contains("payload BLOB"));
}

#[test]
fn postgres_alter_column_mapping() {
    let cases = [
        (
            "ALTER TABLE test.t ADD COLUMN amount numeric(12, 3) DEFAULT 1.25",
            "ALTER TABLE test.t ADD COLUMN amount DECIMAL(12, 3) DEFAULT 1.25",
        ),
        (
            "ALTER TABLE test.t RENAME COLUMN amount TO total",
            "ALTER TABLE test.t RENAME COLUMN amount TO total",
        ),
        (
            "ALTER TABLE test.t ALTER COLUMN total TYPE varchar(40)",
            "ALTER TABLE test.t MODIFY COLUMN total VARCHAR(40)",
        ),
        (
            "ALTER TABLE test.t ALTER COLUMN total SET DEFAULT 'ready'",
            "ALTER TABLE test.t ALTER COLUMN total SET DEFAULT 'ready'",
        ),
        (
            "ALTER TABLE test.t ALTER COLUMN total DROP DEFAULT",
            "ALTER TABLE test.t ALTER COLUMN total DROP DEFAULT",
        ),
        (
            "ALTER TABLE test.t DROP COLUMN IF EXISTS total",
            "ALTER TABLE test.t DROP COLUMN IF EXISTS total",
        ),
    ];
    for (sql, expected) in cases {
        assert_eq!(crate::pg_sql::adapt(sql).unwrap(), expected, "{sql}");
    }

    for sql in [
        "ALTER TABLE test.t ALTER COLUMN total TYPE bigint USING total::bigint",
        "ALTER TABLE test.t DROP COLUMN total CASCADE",
        "ALTER TABLE test.t ADD COLUMN a integer, ADD COLUMN b integer",
    ] {
        assert_eq!(crate::pg_sql::adapt(sql).unwrap_err().0, "0A000", "{sql}");
    }
}

#[test]
fn pg_scalar_20_float_column_aliases() {
    assert_eq!(
        crate::pg_sql::adapt("CREATE TABLE t (f FLOAT4, d FLOAT8)").unwrap(),
        "CREATE TABLE t (f FLOAT, d DOUBLE)"
    );
}

#[test]
fn pg09_12_values_source() {
    let sql = "SELECT rank() OVER(ORDER BY x) FROM (VALUES ($1),(NULL::int4)) t(x)";
    let (source, offsets) = crate::pg_sql::values_parameter_source(sql).unwrap();
    assert_eq!(
        source,
        "SELECT rank() OVER(ORDER BY x) FROM (VALUES ROW ($1),ROW (NULL::int4)) t(x)"
    );
    assert_eq!(
        offsets,
        vec![sql.find("($1").unwrap(), sql.find("(NULL").unwrap()]
    );
    assert_eq!(crate::pg_sql::adapt(sql).unwrap(), source);
    assert_eq!(
        crate::pg_sql::adapt("SELECT x FROM (VALUES(1),(2)) t(x)").unwrap(),
        "SELECT x FROM (VALUES ROW(1),ROW (2)) t(x)"
    );
    for sql in [
        "INSERT INTO t VALUES ($1),($2)",
        "SELECT 'VALUES ($1),($2)' AS s",
    ] {
        assert_eq!(
            crate::pg_sql::values_parameter_source(sql).unwrap(),
            (sql.to_owned(), vec![])
        );
    }
}

#[test]
fn pg10_28_old_new_capability_gate_is_lexical_and_syntax_checked() {
    for sql in [
        "UPDATE t SET v=99 RETURNING WITH (OLD AS o, NEW AS n) o.v,n.v",
        "DELETE FROM t RETURNING /* comment */ WITH (OLD AS \"before\") \"before\".*",
        "INSERT INTO t VALUES(1) RETURNING WITH (NEW AS n) n.*",
    ] {
        assert_eq!(
            crate::pg_sql::check_returning_aliases(sql).unwrap_err().0,
            "0A000"
        );
        assert_eq!(
            crate::pg_result::prepare_command(sql).err().unwrap().0,
            "0A000"
        );
    }
    for sql in [
        "SELECT 'RETURNING WITH (OLD AS o) o.v'",
        "UPDATE t SET v=99 RETURNING WITH (OLD o) o.v",
        "UPDATE t SET v=99 RETURNING WITH (OLD AS o, OLD AS p) o.v",
        "UPDATE t SET v=99 RETURNING WITH (OLD AS o)",
        "UPDATE t SET RETURNING WITH (OLD AS o) o.v",
    ] {
        assert!(crate::pg_sql::check_returning_aliases(sql).is_ok(), "{sql}");
    }
}

#[test]
fn query_statement_text_preserves_quoted_semicolons() {
    for source in [
        "SELECT ';' AS marker",
        "; /* ; comment */ SELECT ';' AS marker;;",
        "-- ; comment\n ; SELECT ';' AS marker; -- trailing ;",
    ] {
        assert_eq!(
            crate::pg_sql::query_statement_text(source).unwrap(),
            "SELECT ';' AS marker"
        );
    }
    assert_eq!(
        crate::pg_sql::query_statement_text("; /* ; */ ;").unwrap(),
        ""
    );
}

#[test]
fn pg13_011_user_type_creation_gate() {
    for sql in [
        "CREATE DOMAIN public.positive AS integer CHECK (VALUE>0)",
        "/* before */ CREATE /* kind */ DOMAIN public.d AS integer",
        "CREATE TYPE public.pair AS (id integer, label text)",
        "CREATE TYPE public.mood AS ENUM ('good','bad')",
    ] {
        assert_eq!(crate::pg_sql::adapt(sql).unwrap_err().0, "0A000", "{sql}");
    }
    for sql in [
        "CREATE TABLE public.type(id integer)",
        "SELECT 'CREATE TYPE'",
        "SELECT 1 AS domain",
    ] {
        assert!(crate::pg_sql::adapt(sql).is_ok(), "{sql}");
    }
}

#[test]
fn pg13_023_default_literal_escapes() {
    assert_eq!(
        super::pg_sql::adapt("CREATE TABLE t (v text DEFAULT 'a\\b')").unwrap(),
        "CREATE TABLE t (v TEXT DEFAULT 'a\\\\b')"
    );
    assert_eq!(
        super::pg_sql::adapt("CREATE TABLE t (v text DEFAULT E'a\\\\b')").unwrap(),
        "CREATE TABLE t (v TEXT DEFAULT 'a\\\\b')"
    );
}

#[test]
fn pg13_034_interval_ddl_adaptation() {
    for declaration in [
        "interval",
        "interval(3)",
        "interval day to second(2)",
        "interval year to month",
    ] {
        let sql = format!("CREATE TABLE t(v {declaration})");
        let adapted = crate::pg_sql::adapt(&sql).unwrap();
        assert!(
            adapted.contains(&declaration.replacen("interval", "INTERVAL", 1)),
            "{adapted}"
        );
    }
}
