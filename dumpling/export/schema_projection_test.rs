// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.

use crate::schema_projection::*;
use schema_parser::ast;

fn project(sql: &str, selected: &[&str]) -> crate::Result<ProjectedTableSchema> {
    let schema = build_projected_table_schema(
        &mut schema_parser::New(),
        sql,
        &selected
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>(),
    )?;
    let restored = restore_projected_schema(&schema.create_table)?;
    let reparsed = parse_table_schema(&mut schema_parser::New(), &restored)
        .unwrap_or_else(|error| panic!("restored schema must parse: {restored}: {}", error.msg));
    assert_eq!(
        reparsed
            .create_table
            .Cols
            .iter()
            .map(|column| &column.Name.Name.L)
            .collect::<Vec<_>>(),
        schema
            .create_table
            .Cols
            .iter()
            .map(|column| &column.Name.Name.L)
            .collect::<Vec<_>>()
    );
    Ok(schema)
}

fn parsed(sql: &str) -> ProjectedTableSchema {
    parse_table_schema(&mut schema_parser::New(), sql).unwrap()
}
fn error(sql: &str, selected: &[&str]) -> String {
    match project(sql, selected) {
        Ok(_) => panic!("expected projection failure: {sql}"),
        Err(error) => error.msg,
    }
}

#[test]
fn projected_schema_generated_dependencies_follow_declaration_order() {
    let schema = project("CREATE TABLE t (a INT,b INT,c INT GENERATED ALWAYS AS (a+b) VIRTUAL,d INT GENERATED ALWAYS AS (a*2) STORED,e INT GENERATED ALWAYS AS (d+1) VIRTUAL,KEY idx_c(c),KEY idx_e(e),KEY idx_ab(a,b),CONSTRAINT chk_b CHECK(b>0)) ENGINE=InnoDB", &["A"]).unwrap();
    assert_eq!(
        schema
            .create_table
            .Cols
            .iter()
            .map(|column| column.Name.Name.O.as_str())
            .collect::<Vec<_>>(),
        ["a", "d", "e"]
    );
    assert_eq!(
        schema
            .create_table
            .Constraints
            .iter()
            .map(|constraint| constraint.Name.as_str())
            .collect::<Vec<_>>(),
        ["idx_e"]
    );
    assert!(
        schema.create_table.Cols[1]
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::Generated && option.Stored)
    );
    assert!(!schema.retained_columns.contains("c"));
}

#[test]
fn projected_schema_partition_and_subpartition_dependencies_are_checked() {
    for sql in [
        "CREATE TABLE t(a INT,b INT) PARTITION BY HASH(a) PARTITIONS 4",
        "CREATE TABLE t(a INT,b INT) PARTITION BY KEY(a) PARTITIONS 4",
        "CREATE TABLE t(a INT,b INT,PRIMARY KEY(a,b)) PARTITION BY RANGE(a) SUBPARTITION BY HASH(b) SUBPARTITIONS 2 (PARTITION p0 VALUES LESS THAN(100),PARTITION pmax VALUES LESS THAN MAXVALUE)",
    ] {
        let selected = if sql.contains("SUBPARTITION") {
            vec!["a"]
        } else {
            vec!["b"]
        };
        assert_eq!(
            error(sql, &selected),
            "partition definition references a removed column"
        );
        let original = parsed(sql);
        let projected = project(sql, &["a", "b"]).unwrap();
        assert_eq!(
            projected.create_table.Partition,
            original.create_table.Partition
        );
    }
    let sql = "CREATE TABLE t(id INT,tenant_id INT,secret INT,PRIMARY KEY(id,tenant_id)) PARTITION BY KEY() PARTITIONS 2";
    assert_eq!(
        error(sql, &["id"]),
        "PARTITION BY KEY() is not supported with column filtering"
    );
    assert!(parsed(sql).create_table.Partition.is_some());
}

#[test]
fn projected_schema_retains_vector_index_and_filters_expression_indexes() {
    let sql = "CREATE TABLE t(id INT PRIMARY KEY,embedding VECTOR(3),secret INT,VECTOR INDEX idx_embedding((VEC_COSINE_DISTANCE(embedding))),KEY idx_secret((secret+1)))";
    let schema = project(sql, &["id", "embedding"]).unwrap();
    assert_eq!(
        schema
            .create_table
            .Constraints
            .iter()
            .map(|constraint| constraint.Name.as_str())
            .collect::<Vec<_>>(),
        ["idx_embedding"]
    );
    assert_eq!(
        schema.create_table.Constraints[0].Tp,
        ast::ConstraintType::Vector
    );
}

#[test]
fn projected_schema_ttl_dependency_and_retained_table_options() {
    let sql = "CREATE TABLE t(id INT PRIMARY KEY,created_at DATETIME,secret INT) TTL = created_at + INTERVAL 1 DAY";
    assert_eq!(
        error(sql, &["id"]),
        "TTL definition references removed column `created_at`"
    );
    let before = parsed(sql);
    let after = project(sql, &["id", "created_at"]).unwrap();
    assert_eq!(after.create_table.Options, before.create_table.Options);
}

#[test]
fn projected_schema_default_and_on_update_dependencies_are_not_silently_removed() {
    assert_eq!(
        error("CREATE TABLE t(a INT,b INT DEFAULT (a))", &["b"]),
        "column `b` expression references a removed column"
    );
    let mut schema = parsed("CREATE TABLE t(a INT,b INT DEFAULT(a))");
    // ON UPDATE expressions use the same dependency rule; exercise its AST even
    // where MySQL's SQL grammar restricts this option to current timestamps.
    schema.create_table.Cols[1].Options[0].Tp = ast::ColumnOptionType::OnUpdate;
    let failure = project_table_schema(schema, &["b".into()]).err().unwrap();
    assert_eq!(
        failure.msg,
        "column `b` expression references a removed column"
    );
    let sql = "CREATE TABLE t(id INT PRIMARY KEY,token VARCHAR(32) DEFAULT(UUID()),secret INT)";
    let original = parsed(sql);
    let projected = project(sql, &["id", "token"]).unwrap();
    assert_eq!(
        projected.create_table.Cols[1].Options,
        original.create_table.Cols[1].Options
    );
}

#[test]
fn projected_schema_string_literals_preserve_backslashes_in_ast() {
    let sql = r"CREATE TABLE t(id INT PRIMARY KEY,path VARCHAR(32) DEFAULT 'C:\\new' COMMENT 'C:\\new',secret INT)";
    let original = parsed(sql);
    let projected = project(sql, &["id", "path"]).unwrap();
    assert_eq!(
        projected.create_table.Cols[1].Options,
        original.create_table.Cols[1].Options
    );
    assert_eq!(projected.create_table.Cols.len(), 2);
}

#[test]
fn projected_schema_primary_keys_and_auto_columns_follow_go_validation() {
    let schema = project("CREATE TABLE t(tenant_id INT,id INT,name VARCHAR(32) DEFAULT NULL,PRIMARY KEY(tenant_id,id))", &["id","name"]).unwrap();
    assert!(schema.create_table.Constraints.is_empty());
    assert_eq!(
        error(
            "CREATE TABLE t(tenant_id INT,id BIGINT AUTO_INCREMENT,PRIMARY KEY(tenant_id,id))",
            &["id"]
        ),
        "auto_increment column `id` must be defined as a key"
    );
    assert_eq!(
        error(
            "CREATE TABLE t(id BIGINT AUTO_RANDOM(3),tenant_id BIGINT,PRIMARY KEY(id,tenant_id) CLUSTERED)",
            &["id"]
        ),
        "auto_random is only supported on the tables with clustered primary key"
    );
    for sql in [
        "CREATE TABLE t(id BIGINT AUTO_INCREMENT PRIMARY KEY,secret INT)",
        "CREATE TABLE t(id BIGINT AUTO_RANDOM PRIMARY KEY,secret INT)",
    ] {
        assert_eq!(project(sql, &["id"]).unwrap().create_table.Cols.len(), 1);
    }
}

#[test]
fn projected_schema_inline_checks_and_partial_index_conditions_are_filtered() {
    let sql = "CREATE TABLE t(a INT CHECK(a>0),b INT, CHECK(b>0),KEY ia(a))";
    let original = parsed(sql);
    let projected = project(sql, &["a"]).unwrap();
    assert_eq!(
        projected.create_table.Cols[0].Options,
        original.create_table.Cols[0].Options
    );
    assert_eq!(projected.create_table.Constraints.len(), 1);
    let mut partial = parsed("CREATE TABLE t(a INT,b INT,KEY ia(a),CHECK(b>0))");
    let condition = partial.create_table.Constraints[1].Expr.clone();
    partial.create_table.Constraints[0].Option = Some(ast::IndexOption {
        Condition: condition,
        ..Default::default()
    });
    assert!(
        project_table_schema(partial, &["a".into()])
            .unwrap()
            .create_table
            .Constraints
            .is_empty()
    );
}

fn foreign_keys(
    child_sql: &str,
    child_columns: &[&str],
    parent_sql: &str,
    parent_columns: &[&str],
) -> crate::Result<()> {
    let child = project(child_sql, child_columns)?;
    let parent = project(parent_sql, parent_columns)?;
    let mut schemas = ProjectedTableSchemas::new();
    schemas.insert(("test".into(), "parent".into()), parent);
    validate_foreign_key_parents("test", &child, &schemas)
}

#[test]
fn projected_schema_foreign_keys_reject_removed_parent_columns_and_supporting_indexes() {
    let child = "CREATE TABLE child(id INT,parent_secret INT,CONSTRAINT fk_secret FOREIGN KEY(parent_secret) REFERENCES parent(secret))";
    assert_eq!(
        foreign_keys(
            child,
            &["id", "parent_secret"],
            "CREATE TABLE parent(id INT PRIMARY KEY,secret INT,KEY(secret))",
            &["id"]
        )
        .unwrap_err()
        .msg,
        "foreign key references removed column `test`.`parent`.`secret`"
    );
    let child = "CREATE TABLE child(id INT PRIMARY KEY,parent_id INT,FOREIGN KEY(parent_id) REFERENCES parent(id))";
    assert!(
        foreign_keys(
            child,
            &["id", "parent_id"],
            "CREATE TABLE parent(id INT,secret INT,UNIQUE KEY(id,secret))",
            &["id"]
        )
        .unwrap_err()
        .msg
        .contains("referenced columns are not indexed")
    );
    foreign_keys(
        child,
        &["id", "parent_id"],
        "CREATE TABLE parent(id INT,tenant_id INT,secret INT,KEY(id,tenant_id))",
        &["id", "tenant_id"],
    )
    .unwrap();
}

#[test]
fn projected_schema_foreign_keys_accept_generated_parent_and_preserve_actions() {
    let child = "CREATE TABLE child(id INT,parent_generated INT,CONSTRAINT fk_generated FOREIGN KEY(parent_generated) REFERENCES parent(`generated`))";
    foreign_keys(
        child,
        &["id", "parent_generated"],
        "CREATE TABLE parent(id INT,`generated` INT GENERATED ALWAYS AS(id+1) STORED,KEY(`generated`))",
        &["id"],
    )
    .unwrap();
    let sql = "CREATE TABLE child(id INT PRIMARY KEY,parent_id INT,FOREIGN KEY(parent_id) REFERENCES parent(id) ON DELETE CASCADE ON UPDATE SET NULL)";
    let original = parsed(sql);
    let schema = project(sql, &["id", "parent_id"]).unwrap();
    assert_eq!(
        schema.create_table.Constraints,
        original.create_table.Constraints
    );
    foreign_keys(
        sql,
        &["id", "parent_id"],
        "CREATE TABLE parent(id INT PRIMARY KEY)",
        &["id"],
    )
    .unwrap();
}

#[test]
fn projected_schema_inline_foreign_key_and_unique_parent_are_checked() {
    let child = "CREATE TABLE child(parent_name VARCHAR(32) REFERENCES parent(name))";
    foreign_keys(
        child,
        &["parent_name"],
        "CREATE TABLE parent(name VARCHAR(32) UNIQUE KEY,secret INT)",
        &["name"],
    )
    .unwrap();
    assert!(
        foreign_keys(
            child,
            &["parent_name"],
            "CREATE TABLE parent(name VARCHAR(32),secret INT)",
            &["secret"]
        )
        .unwrap_err()
        .msg
        .contains("removed column")
    );
}

#[test]
fn projected_schema_foreign_keys_require_full_column_prefix_and_regular_index() {
    let child = "CREATE TABLE child(id INT PRIMARY KEY,parent_name VARCHAR(32),FOREIGN KEY(parent_name) REFERENCES parent(name))";
    for parent in [
        "CREATE TABLE parent(name VARCHAR(32),KEY(name(8)))",
        "CREATE TABLE parent(name VARCHAR(32),FULLTEXT INDEX(name))",
    ] {
        assert!(
            foreign_keys(child, &["id", "parent_name"], parent, &["name"])
                .unwrap_err()
                .msg
                .contains("referenced columns are not indexed")
        );
    }
}

#[test]
fn projected_schema_lookup_prefers_exact_names_and_rejects_ambiguous_folded_names() {
    let mut schemas = ProjectedTableSchemas::new();
    schemas.insert(
        ("test".into(), "Orders".into()),
        project("CREATE TABLE Orders(id INT PRIMARY KEY)", &["id"]).unwrap(),
    );
    schemas.insert(
        ("test".into(), "orders".into()),
        project(
            "CREATE TABLE orders(other_id INT PRIMARY KEY)",
            &["other_id"],
        )
        .unwrap(),
    );
    assert!(
        lookup_schema(&schemas, "test", "Orders")
            .unwrap()
            .unwrap()
            .retained_columns
            .contains("id")
    );
    assert!(
        lookup_schema(&schemas, "test", "orders")
            .unwrap()
            .unwrap()
            .retained_columns
            .contains("other_id")
    );
    assert!(
        lookup_schema(&schemas, "test", "ORDERS")
            .err()
            .unwrap()
            .msg
            .contains("ambiguous under case-insensitive matching")
    );
    assert!(
        lookup_schema(&schemas, "outside", "orders")
            .unwrap()
            .is_none()
    );
}

#[test]
fn projected_schema_self_foreign_key_case_insensitive_and_outside_dump_behavior() {
    let sql = "CREATE TABLE t(id INT,parent_secret INT,secret INT,FOREIGN KEY(parent_secret) REFERENCES T(secret))";
    let schema = project(sql, &["id", "parent_secret"]).unwrap();
    let mut schemas = ProjectedTableSchemas::new();
    schemas.insert(("test".into(), "t".into()), schema);
    assert_eq!(
        validate_foreign_key_parents(
            "test",
            schemas.get(&("test".into(), "t".into())).unwrap(),
            &schemas
        )
        .unwrap_err()
        .msg,
        "foreign key references removed column `test`.`T`.`secret`"
    );
    let outside =
        parsed("CREATE TABLE child(id INT,FOREIGN KEY(id) REFERENCES external_parent(id))");
    validate_foreign_key_parents("test", &outside, &ProjectedTableSchemas::new()).unwrap();
}

#[test]
fn projected_schema_parse_rejects_other_statements_and_keeps_unfiltered_generated_columns() {
    assert!(
        parse_table_schema(&mut schema_parser::New(), "SELECT 1")
            .err()
            .unwrap()
            .msg
            .contains("expected CREATE TABLE")
    );
    assert!(
        parse_table_schema(&mut schema_parser::New(), "CREATE TABLE (")
            .err()
            .unwrap()
            .msg
            .contains("failed to parse CREATE TABLE")
    );
    let schema = parsed("CREATE TABLE t(a INT,b INT GENERATED ALWAYS AS(a+1) VIRTUAL)");
    assert_eq!(schema.create_table.Cols.len(), 2);
    assert!(schema.retained_columns.contains("b"));
}

#[test]
fn projected_schema_lookup_uses_unicode_simple_folding() {
    let mut schemas = ProjectedTableSchemas::new();
    for name in ["Σ", "S", "K", "µ", "ß", "I"] {
        schemas.insert(
            ("test".into(), name.into()),
            parsed("CREATE TABLE t(id INT PRIMARY KEY)"),
        );
    }
    for (name, matched) in [
        ("ς", true),
        ("σ", true),
        ("ſ", true),
        ("K", true),
        ("Μ", true),
        ("SS", false),
        ("ı", false),
        ("İ", false),
    ] {
        assert_eq!(
            lookup_schema(&schemas, "TEST", name).unwrap().is_some(),
            matched,
            "{name}"
        );
    }
}

#[test]
fn projected_schema_restores_original_go_positive_outputs() {
    for (sql, columns, expected) in [
        (
            "CREATE TABLE t(a INT,b INT) PARTITION BY HASH(a) PARTITIONS 4",
            vec!["a"],
            "PARTITION BY HASH (`a`) PARTITIONS 4",
        ),
        (
            "CREATE TABLE t(a INT,b INT) PARTITION BY KEY(a) PARTITIONS 4",
            vec!["a"],
            "PARTITION BY KEY (`a`) PARTITIONS 4",
        ),
        (
            "CREATE TABLE t(id INT PRIMARY KEY,created_at DATETIME,secret INT) TTL=created_at+INTERVAL 1 DAY",
            vec!["id", "created_at"],
            "/*T![ttl] TTL = `created_at` + INTERVAL 1 DAY */",
        ),
        (
            "CREATE TABLE t(id INT PRIMARY KEY,token VARCHAR(32) DEFAULT(UUID()),secret INT)",
            vec!["id", "token"],
            "`token` VARCHAR(32) DEFAULT (UUID())",
        ),
        (
            "CREATE TABLE child(id INT PRIMARY KEY,parent_id INT,FOREIGN KEY(parent_id) REFERENCES parent(id) ON DELETE CASCADE ON UPDATE SET NULL)",
            vec!["id", "parent_id"],
            "ON DELETE CASCADE ON UPDATE SET NULL",
        ),
        (
            "CREATE TABLE child(parent_name VARCHAR(32) REFERENCES parent(name))",
            vec!["parent_name"],
            "REFERENCES `parent`(`name`)",
        ),
    ] {
        let schema = project(sql, &columns).unwrap();
        let restored = restore_projected_schema(&schema.create_table).unwrap();
        assert!(
            restored.contains(expected),
            "{restored} does not contain {expected}"
        );
    }
}

#[test]
fn projected_schema_restoration_preserves_typed_literals_and_special_comments() {
    let sql = r"CREATE TABLE t(id BIGINT AUTO_RANDOM(3) PRIMARY KEY CLUSTERED,path VARCHAR(32) DEFAULT 'C:\\new' COMMENT 'C:\\new',numeric_text VARCHAR(32) DEFAULT '123',null_text VARCHAR(32) DEFAULT 'NULL',secret INT) AUTO_ID_CACHE=8";
    let schema = project(sql, &["id", "path", "numeric_text", "null_text"]).unwrap();
    let restored = restore_projected_schema(&schema.create_table).unwrap();
    assert!(
        restored.contains(r"DEFAULT _UTF8MB4'C:\\new'"),
        "{restored}"
    );
    assert!(restored.contains(r"COMMENT 'C:\\new'"), "{restored}");
    assert!(restored.contains("DEFAULT _UTF8MB4'123'"), "{restored}");
    assert!(restored.contains("DEFAULT _UTF8MB4'NULL'"), "{restored}");
    assert!(
        restored.contains("/*T![auto_rand] AUTO_RANDOM(3) */"),
        "{restored}"
    );
    assert!(
        restored.contains("/*T![clustered_index] CLUSTERED */"),
        "{restored}"
    );
    assert!(
        restored.contains("/*T![auto_id_cache] AUTO_ID_CACHE = 8 */"),
        "{restored}"
    );
    assert!(!restored.contains("`secret`"));
}

#[test]
fn projected_schema_sql_mode_is_formatted_and_applied() {
    let mut params = std::collections::HashMap::new();
    params.insert("sql_mode".into(), "ansi ".into());
    let mut parser = new_schema_parser(&params).unwrap();
    let schema = build_projected_table_schema(
        &mut parser,
        r#"CREATE TABLE "t"("id" INT PRIMARY KEY,"secret" INT)"#,
        &["id".into()],
    )
    .unwrap();
    let restored = restore_projected_schema(&schema.create_table).unwrap();
    assert!(restored.contains("`id`"));
    assert!(!restored.contains("`secret`"));
    assert!(
        new_schema_parser(&std::collections::HashMap::from([(
            "sql_mode".into(),
            "bad mode".into()
        )]))
        .err()
        .unwrap()
        .msg
        .contains("failed to parse session sql_mode")
    );
}

#[test]
fn projected_schema_enum_and_set_elements_preserve_backslashes() {
    for tp in ["ENUM", "SET"] {
        let sql =
            format!(r"CREATE TABLE t(id INT PRIMARY KEY,path {tp}('C:\\new','123'),secret INT)");
        let schema = project(&sql, &["id", "path"]).unwrap();
        let restored = restore_projected_schema(&schema.create_table).unwrap();
        let reparsed = parsed(&restored);
        assert_eq!(
            schema.create_table.Cols[1].Tp.GetElems(),
            reparsed.create_table.Cols[1].Tp.GetElems(),
            "{restored}"
        );
    }
}

#[test]
fn projected_schema_placement_policy_preserves_identifier() {
    let mut schema = project(
        "CREATE TABLE t(id INT PRIMARY KEY,secret INT) PLACEMENT POLICY = `default`",
        &["id"],
    )
    .unwrap();
    let policy = schema
        .create_table
        .Options
        .iter_mut()
        .find(|o| o.Tp == ast::TableOptionType::Policy)
        .unwrap();
    // Go PlacementOption.Restore uses the policy name regardless of UintValue.
    policy.UintValue = 1;
    let restored = restore_projected_schema(&schema.create_table).unwrap();
    assert!(
        restored.contains("PLACEMENT POLICY = `default`"),
        "{restored}"
    );
    assert_eq!(
        parsed(&restored).create_table.Options[0].StrValue,
        "default"
    );
}
