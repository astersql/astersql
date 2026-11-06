// Copyright 2026 AsterSQL.
use super::*;

#[test]
fn cat14_19_table_acl_provenance_atomicity_and_lifecycle() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_role_command(&PgRoleCommand::Create {
        name: "cat14_19_model_role".into(),
        can_login: false,
    })
    .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Create("cat14_19_model".into()))
        .unwrap();
    let physical = pg
        .pg_bind_relation(Some("cat14_19_model"), "probe", true)
        .unwrap();
    pg.execute(&format!("CREATE TABLE \"{physical}\"(id integer)"))
        .unwrap();
    let model = || domain.table_by_name("test", &physical).unwrap();
    let original = (*model()).clone();
    let roles = pg.pg_role_facts().unwrap();
    assert_eq!(
        pg_table::pg_table_acl_text(&original, &roles).unwrap(),
        None
    );
    let command = PgSchemaCommand::TablePrivileges {
        sequence: false,
        columns: Vec::new(),
        schema: Some("cat14_19_model".into()),
        table: "probe".into(),
        grantee: "cat14_19_model_role".into(),
        public: false,
        privileges: "TRIGGER".into(),
        grant: true,
        grant_option: true,
        cascade: false,
    };
    pg.pg_schema_privilege_command(&command).unwrap();
    assert!(
        pg_table::pg_table_acl_text(&model(), &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_19_model_role=t*/root")
    );
    assert!(
        pg.pg_database_drop_role("cat14_19_model_role", false)
            .is_err()
    );
    let granted = (*model()).clone();
    for corruption in ["missing", "grantor", "options", "owner", "default"] {
        domain
            .ddl_update_postgresql_acl("test", original.ID, |m| {
                *m = original.clone();
                match corruption {
                    "missing" => m.PostgreSQLAcl = None,
                    "grantor" => m.PostgreSQLAcl.as_mut().unwrap().grants[0].grantor = 0,
                    "options" => {
                        let item = &mut m.PostgreSQLAcl.as_mut().unwrap().grants[0];
                        item.privileges = 2;
                        item.grant_options = 4;
                    }
                    "owner" => m.PostgreSQLOwner = None,
                    _ => m.PostgreSQLAcl.as_mut().unwrap().grants.clear(),
                }
                Ok(())
            })
            .unwrap();
        domain.force_full_reload_for_test().unwrap();
        let damaged = serde_json::to_vec(&*model()).unwrap();
        assert_eq!(
            pg_table::pg_table_acl_text(&model(), &roles)
                .unwrap_err()
                .pg_function_error()
                .unwrap()
                .state,
            "0A000",
            "{corruption}"
        );
        let error = pg.pg_schema_privilege_command(&command).unwrap_err();
        assert_eq!(
            error.pg_function_error().unwrap().state,
            "0A000",
            "{corruption}: {error}"
        );
        assert_eq!(
            serde_json::to_vec(&*model()).unwrap(),
            damaged,
            "failed ACL edit must not publish partial metadata"
        );
    }
    domain
        .ddl_update_postgresql_acl("test", original.ID, |m| {
            *m = granted;
            Ok(())
        })
        .unwrap();
    domain.force_full_reload_for_test().unwrap();
    assert!(
        pg_table::pg_table_acl_text(&model(), &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_19_model_role=t*/root")
    );
    pg.execute(&format!("DROP TABLE \"{physical}\"")).unwrap();
    pg.pg_database_drop_role("cat14_19_model_role", false)
        .unwrap();
    native
        .execute("CREATE TABLE legacy_acl(id integer)")
        .unwrap();
    let legacy = domain.table_by_name("test", "legacy_acl").unwrap();
    assert!(legacy.PostgreSQLAcl.is_none());
    assert_eq!(
        pg_table::pg_table_acl_text(&legacy, &roles)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    native.execute("DROP TABLE legacy_acl").unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Drop("cat14_19_model".into()))
        .unwrap();
    pg.pg_role_command(&PgRoleCommand::Drop {
        name: "cat14_19_model_role".into(),
    })
    .unwrap();
    domain.close();
}

#[test]
fn cat14_20_column_acl_provenance_atomicity_and_lifecycle() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_role_command(&PgRoleCommand::Create {
        name: "cat14_20_model_role".into(),
        can_login: false,
    })
    .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Create("cat14_20_model".into()))
        .unwrap();
    let physical = pg
        .pg_bind_relation(Some("cat14_20_model"), "probe", true)
        .unwrap();
    pg.execute(&format!(
        "CREATE TABLE \"{physical}\"(id integer,secret text)"
    ))
    .unwrap();
    let model = || domain.table_by_name("test", &physical).unwrap();
    let original = (*model()).clone();
    let column = original.Columns[0].ID;
    let roles = pg.pg_role_facts().unwrap();
    assert_eq!(
        pg_table::pg_column_acl_text(&original, column, &roles).unwrap(),
        None
    );
    let command = PgSchemaCommand::TablePrivileges {
        sequence: false,
        columns: vec!["id".into()],
        schema: Some("cat14_20_model".into()),
        table: "probe".into(),
        grantee: "cat14_20_model_role".into(),
        public: false,
        privileges: "SELECT".into(),
        grant: true,
        grant_option: true,
        cascade: false,
    };
    pg.pg_schema_privilege_command(&command).unwrap();
    assert!(
        pg_table::pg_column_acl_text(&model(), column, &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_20_model_role=r*/root")
    );
    assert!(
        pg.pg_database_drop_role("cat14_20_model_role", false)
            .is_err()
    );
    let granted = (*model()).clone();
    for corruption in [
        "missing", "grantor", "options", "owner", "default", "illegal",
    ] {
        domain
            .ddl_update_postgresql_acl("test", original.ID, |m| {
                *m = granted.clone();
                match corruption {
                    "missing" => m.PostgreSQLColumnAcls = None,
                    "owner" => m.PostgreSQLOwner = None,
                    _ => {
                        let acl = m
                            .PostgreSQLColumnAcls
                            .as_mut()
                            .unwrap()
                            .get_mut(&column)
                            .unwrap();
                        match corruption {
                            "grantor" => acl.grants[0].grantor = 0,
                            "options" => acl.grants[0].grant_options = 4,
                            "default" => acl.default_acl = true,
                            _ => acl.grants[0].privileges = 8,
                        }
                    }
                }
                Ok(())
            })
            .unwrap();
        let before = serde_json::to_vec(&*model()).unwrap();
        assert_eq!(
            pg_table::pg_column_acl_text(&model(), column, &roles)
                .unwrap_err()
                .pg_function_error()
                .unwrap()
                .state,
            "0A000",
            "{corruption}"
        );
        assert_eq!(
            pg.pg_schema_privilege_command(&command)
                .unwrap_err()
                .pg_function_error()
                .unwrap()
                .state,
            "0A000",
            "{corruption}"
        );
        assert_eq!(
            serde_json::to_vec(&*model()).unwrap(),
            before,
            "{corruption}"
        );
    }
    domain
        .ddl_update_postgresql_acl("test", original.ID, |m| {
            *m = original.clone();
            m.PostgreSQLColumnAcls.as_mut().unwrap().insert(
                column,
                astersql_meta_model::PostgreSQLRelationAcl {
                    default_acl: false,
                    grants: Vec::new(),
                },
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        pg_table::pg_column_acl_text(&model(), column, &roles).unwrap(),
        Some("{}".into())
    );
    domain.reload().unwrap();
    assert_eq!(
        pg_table::pg_column_acl_text(&model(), column, &roles).unwrap(),
        Some("{}".into())
    );
    // Owner transfer rewrites both column grantor and grantee without changing relation/column IDs.
    domain
        .ddl_update_postgresql_acl("test", original.ID, |m| {
            *m = granted.clone();
            Ok(())
        })
        .unwrap();
    let owner = roles
        .iter()
        .find(|r| r.name == "cat14_20_model_role")
        .unwrap()
        .oid;
    domain
        .ddl_set_postgresql_owner("test", original.ID, owner)
        .unwrap();
    assert_eq!(model().ID, original.ID);
    assert_eq!(model().Columns[0].ID, column);
    assert!(
        pg_table::pg_column_acl_text(&model(), column, &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_20_model_role=r*/cat14_20_model_role")
    );
    pg.execute(&format!("ALTER TABLE \"{physical}\" DROP COLUMN id"))
        .unwrap();
    assert!(
        !model()
            .PostgreSQLColumnAcls
            .as_ref()
            .unwrap()
            .contains_key(&column)
    );
    pg.execute(&format!("DROP TABLE \"{physical}\"")).unwrap();
    pg.pg_database_drop_role("cat14_20_model_role", false)
        .unwrap();
    domain.close();
}

#[test]
fn cat14_21_sequence_acl_provenance_atomicity_and_lifecycle() {
    let (domain, native) = CreateAnalyzeSession().unwrap();
    native.pg_register_bootstrap_role().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), ast::SqlDialect::PostgreSQL);
    pg.pg_role_command(&PgRoleCommand::Create {
        name: "cat14_21_model_role".into(),
        can_login: false,
    })
    .unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Create("cat14_21_model".into()))
        .unwrap();
    let physical = pg
        .pg_bind_relation(Some("cat14_21_model"), "probe", true)
        .unwrap();
    pg.execute(&format!("CREATE SEQUENCE \"{physical}\""))
        .unwrap();
    let model = || domain.table_by_name("test", &physical).unwrap();
    let original = (*model()).clone();
    let roles = pg.pg_role_facts().unwrap();
    assert_eq!(
        pg_table::pg_table_acl_text(&original, &roles).unwrap(),
        None
    );
    let command = PgSchemaCommand::TablePrivileges {
        sequence: true,
        columns: Vec::new(),
        schema: Some("cat14_21_model".into()),
        table: "probe".into(),
        grantee: "cat14_21_model_role".into(),
        public: false,
        privileges: "USAGE".into(),
        grant: true,
        grant_option: true,
        cascade: false,
    };
    pg.pg_schema_privilege_command(&command).unwrap();
    assert!(
        pg_table::pg_table_acl_text(&model(), &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_21_model_role=U*/root")
    );
    assert!(
        pg.pg_database_drop_role("cat14_21_model_role", false)
            .is_err()
    );
    let granted = (*model()).clone();
    for corruption in [
        "missing", "grantor", "options", "owner", "default", "illegal",
    ] {
        domain
            .ddl_update_postgresql_acl("test", original.ID, |m| {
                *m = original.clone();
                match corruption {
                    "missing" => m.PostgreSQLAcl = None,
                    "grantor" => m.PostgreSQLAcl.as_mut().unwrap().grants[0].grantor = 0,
                    "options" => {
                        let item = &mut m.PostgreSQLAcl.as_mut().unwrap().grants[0];
                        item.privileges = 2;
                        item.grant_options = 4;
                    }
                    "owner" => m.PostgreSQLOwner = None,
                    "illegal" => m.PostgreSQLAcl.as_mut().unwrap().grants[0].privileges = 8,
                    _ => m.PostgreSQLAcl.as_mut().unwrap().grants.clear(),
                }
                Ok(())
            })
            .unwrap();
        domain.force_full_reload_for_test().unwrap();
        let damaged = serde_json::to_vec(&*model()).unwrap();
        assert_eq!(
            pg_table::pg_table_acl_text(&model(), &roles)
                .unwrap_err()
                .pg_function_error()
                .unwrap()
                .state,
            "0A000",
            "{corruption}"
        );
        let error = pg.pg_schema_privilege_command(&command).unwrap_err();
        assert_eq!(
            error.pg_function_error().unwrap().state,
            "0A000",
            "{corruption}: {error}"
        );
        assert_eq!(
            serde_json::to_vec(&*model()).unwrap(),
            damaged,
            "failed ACL edit must not publish partial metadata"
        );
    }
    domain
        .ddl_update_postgresql_acl("test", original.ID, |m| {
            *m = granted;
            Ok(())
        })
        .unwrap();
    domain.force_full_reload_for_test().unwrap();
    assert!(
        pg_table::pg_table_acl_text(&model(), &roles)
            .unwrap()
            .unwrap()
            .contains("cat14_21_model_role=U*/root")
    );
    pg.execute(&format!("DROP SEQUENCE \"{physical}\""))
        .unwrap();
    pg.pg_database_drop_role("cat14_21_model_role", false)
        .unwrap();
    native
        .execute("CREATE SEQUENCE legacy_sequence_acl")
        .unwrap();
    let legacy = domain.table_by_name("test", "legacy_sequence_acl").unwrap();
    assert!(legacy.PostgreSQLAcl.is_none());
    assert_eq!(
        pg_table::pg_table_acl_text(&legacy, &roles)
            .unwrap_err()
            .pg_function_error()
            .unwrap()
            .state,
        "0A000"
    );
    native.execute("DROP SEQUENCE legacy_sequence_acl").unwrap();
    pg.pg_schema_command(&PgSchemaCommand::Drop("cat14_21_model".into()))
        .unwrap();
    pg.pg_role_command(&PgRoleCommand::Drop {
        name: "cat14_21_model_role".into(),
    })
    .unwrap();
    domain.close();
}
