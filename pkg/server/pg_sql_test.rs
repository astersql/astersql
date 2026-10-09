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
