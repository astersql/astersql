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
