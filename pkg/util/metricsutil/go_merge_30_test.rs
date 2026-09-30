// Copyright 2026 AsterSQL.

use astersql_sessionctx_variable::session::SessionVars;

#[test]
fn go_merge_30_db_labels_follow_config_and_deduplicate_tables() {
    use astersql_sessionctx_stmtctx::TableEntry;

    let restore = astersql_config::restore_func();
    let vars = SessionVars::new();
    vars.SetCurrentDB("DatabaseA");
    astersql_config::update_global(|config| config.status.record_db_label = false);
    assert_eq!(crate::GetDBNames(Some(&vars)), vec![""]);
    astersql_config::update_global(|config| config.status.record_db_label = true);
    assert_eq!(crate::GetDBNames(None), vec![""]);
    assert_eq!(crate::GetDBNames(Some(&vars)), vec!["databasea"]);
    vars.StmtCtx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "db_b".into(),
            Table: "t1".into(),
        },
        TableEntry {
            DB: "db_a".into(),
            Table: "t2".into(),
        },
        TableEntry {
            DB: "db_b".into(),
            Table: "t3".into(),
        },
    ]);
    assert_eq!(crate::GetDBNames(Some(&vars)), vec!["db_a", "db_b"]);
    restore();
}
