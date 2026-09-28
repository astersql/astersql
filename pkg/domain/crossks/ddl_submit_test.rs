// Copyright 2026 AsterSQL.

use std::sync::Arc;

use super::ddl_submit::{
    AlterTableModeTarget, DdlBackend, DdlClient, Error, HistoryJobState, SessionVariables,
    TableMode,
};

#[derive(Default)]
struct BuildBackend;

impl DdlBackend for BuildBackend {
    fn resolve_database(&self, _: i64) -> Result<Option<String>, Error> {
        Ok(Some("TÉST".into()))
    }

    fn resolve_table(&self, _: i64, _: i64) -> Result<Option<(String, TableMode)>, Error> {
        Ok(Some(("T_Ä".into(), TableMode::Normal)))
    }

    fn session_variables(&self) -> Result<SessionVariables, Error> {
        Ok(SessionVariables {
            cdc_write_source: 17,
            sql_mode: 23,
        })
    }

    fn refresh_server_state(&self) -> Result<(), Error> {
        unreachable!("build-only test")
    }

    fn submit(&self, _: &mut super::ddl_submit::AlterTableModeJob) -> Result<(), Error> {
        unreachable!("build-only test")
    }

    fn notify_owner(&self) -> Result<(), Error> {
        unreachable!("build-only test")
    }

    fn history_job(&self, _: i64) -> Result<Option<HistoryJobState>, Error> {
        unreachable!("build-only test")
    }
}

#[test]
fn resolver_compares_names_using_go_cistr_lowercase_semantics() {
    let client = DdlClient::new(Arc::new(BuildBackend));
    let resolved = client
        .resolve_alter_table_mode_target(AlterTableModeTarget {
            schema_id: 11,
            schema_name: "tést".into(),
            table_id: 22,
            table_name: "t_ä".into(),
            current_mode: TableMode::Restore,
            target_mode: TableMode::Import,
        })
        .expect("Go CIStr.L comparison is Unicode lowercase-aware");

    assert_eq!(resolved.current_mode, TableMode::Normal);
}

#[test]
fn normal_can_transition_to_restore_like_go_table_mode() {
    let client = DdlClient::new(Arc::new(BuildBackend));
    let job = client
        .build_alter_table_mode_job(&AlterTableModeTarget {
            schema_id: 11,
            schema_name: "TeSt".into(),
            table_id: 22,
            table_name: "T_Mode".into(),
            current_mode: TableMode::Normal,
            target_mode: TableMode::Restore,
        })
        .expect("Normal -> Restore is allowed by Go TableMode.CanTransitionTo")
        .expect("a mode change must create a job");

    assert_eq!(job.schema_name, "test");
    assert_eq!(job.table_name, "t_mode");
    assert_eq!(job.cdc_write_source, 17);
    assert_eq!(job.sql_mode, 23);
}
