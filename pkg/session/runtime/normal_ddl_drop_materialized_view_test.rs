// Copyright 2026 AsterSQL.

use super::normal_ddl_create_materialized_view_test::{run, seed, setup};
use super::normal_ddl_fixture::hex;
use astersql_meta_model::{
    SchemaState,
    group_2::{
        DropTableArgs, JobArgs,
        ast::{Ident, NewCIStr},
    },
    group_3::{
        ACTION_DROP_MATERIALIZED_VIEW, ACTION_DROP_MATERIALIZED_VIEW_LOG, Job, JobState, JobVersion,
    },
};

fn drop_job(version: JobVersion, action: u8, schema: i64, table: i64, name: &str) -> Job {
    let mut job = Job {
        id: 99817 + i64::from(action),
        tp: action,
        schema_id: schema,
        table_id: table,
        schema_name: "test".into(),
        table_name: name.into(),
        state: JobState::Queueing,
        version,
        ..Default::default()
    };
    let args = DropTableArgs {
        Identifiers: vec![Ident {
            Schema: NewCIStr("test"),
            Name: NewCIStr(name),
        }],
        ..Default::default()
    };
    job.raw_args = serde_json::to_vec(&if version == JobVersion::V1 {
        serde_json::Value::Array(args.getArgsV1(&job))
    } else {
        serde_json::to_value(args).unwrap()
    })
    .unwrap();
    job
}

fn install_job(fixture: &super::normal_ddl_fixture::Fixture, job: &mut Job) {
    let wire = astersql_meta::encode_go_ddl_job(job, false).unwrap();
    fixture.pool.acquire().unwrap().query(format!(
        "INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',{},0)",
        job.id,
        job.schema_id,
        job.table_id,
        hex(&wire),
        job.tp
    )).unwrap();
}

fn drop_view(version: JobVersion) {
    let (fixture, mut view, create) = setup(version);
    view.State = SchemaState::Public;
    seed(&fixture, &view);
    fixture
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "INSERT INTO mysql.tidb_mview_refresh_info (MVIEW_ID) VALUES ({})",
            view.ID
        ))
        .unwrap();
    fixture
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "INSERT INTO mysql.tidb_mview_refresh_alert (MVIEW_ID) VALUES ({})",
            view.ID
        ))
        .unwrap();

    let mut job = drop_job(
        version,
        ACTION_DROP_MATERIALIZED_VIEW,
        create.schema_id,
        view.ID,
        &view.Name.L,
    );
    install_job(&fixture, &mut job);
    for expected in [
        SchemaState::WriteOnly,
        SchemaState::DeleteOnly,
        SchemaState::None,
    ] {
        run(&fixture, &mut job).unwrap();
        assert_eq!(job.schema_state, expected, "{:?}", job.error);
    }
    assert_eq!(job.state, JobState::Done);
    assert!(astersql_ddl::persistent_actions::handler_available(
        ACTION_DROP_MATERIALIZED_VIEW
    ));
    assert!(
        fixture
            .reader()
            .get_table(create.schema_id, view.ID)
            .unwrap()
            .is_none()
    );

    let base = fixture
        .reader()
        .get_table(create.schema_id, fixture.table)
        .unwrap()
        .unwrap();
    let base_info = base.MaterializedViewBase.unwrap();
    assert_eq!(base_info.MViewIDs, vec![7001]);
    let log = fixture
        .reader()
        .get_table(create.schema_id, base_info.MLogID)
        .unwrap()
        .unwrap();
    assert_eq!(
        log.MaterializedViewLog.unwrap().DependentMViewIDs,
        vec![7001]
    );
    assert!(
        fixture
            .pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT MVIEW_ID FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                view.ID
            ))
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT MVIEW_ID FROM mysql.tidb_mview_refresh_alert WHERE MVIEW_ID={}",
                view.ID
            ))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn drop_materialized_view_v1_cleans_dependencies_and_maintenance_state() {
    drop_view(JobVersion::V1);
}

#[test]
fn drop_materialized_view_v2_cleans_dependencies_and_maintenance_state() {
    drop_view(JobVersion::V2);
}

#[test]
fn drop_materialized_view_log_rechecks_dependents_in_worker() {
    let (fixture, _view, create) = setup(JobVersion::V2);
    let base = fixture
        .reader()
        .get_table(create.schema_id, fixture.table)
        .unwrap()
        .unwrap();
    let log_id = base.MaterializedViewBase.unwrap().MLogID;
    let log = fixture
        .reader()
        .get_table(create.schema_id, log_id)
        .unwrap()
        .unwrap();
    let mut job = drop_job(
        JobVersion::V2,
        ACTION_DROP_MATERIALIZED_VIEW_LOG,
        create.schema_id,
        log_id,
        &log.Name.L,
    );
    install_job(&fixture, &mut job);
    run(&fixture, &mut job).unwrap();
    let error = job.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("dependent materialized views exist"),
        "{error}"
    );
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(
        fixture
            .reader()
            .get_table(create.schema_id, log_id)
            .unwrap()
            .unwrap()
            .State,
        SchemaState::Public
    );
}

#[test]
fn drop_materialized_view_log_cleans_base_and_purge_state() {
    let (fixture, _view, create) = setup(JobVersion::V2);
    let mut base = fixture
        .reader()
        .get_table(create.schema_id, fixture.table)
        .unwrap()
        .unwrap();
    let log_id = base.MaterializedViewBase.as_ref().unwrap().MLogID;
    base.MaterializedViewBase.as_mut().unwrap().MViewIDs.clear();
    seed(&fixture, &base);
    let mut log = fixture
        .reader()
        .get_table(create.schema_id, log_id)
        .unwrap()
        .unwrap();
    log.MaterializedViewLog
        .as_mut()
        .unwrap()
        .DependentMViewIDs
        .clear();
    seed(&fixture, &log);
    fixture
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "INSERT INTO mysql.tidb_mlog_purge_info (MLOG_ID) VALUES ({log_id})"
        ))
        .unwrap();
    let mut job = drop_job(
        JobVersion::V2,
        ACTION_DROP_MATERIALIZED_VIEW_LOG,
        create.schema_id,
        log_id,
        &log.Name.L,
    );
    install_job(&fixture, &mut job);
    for expected in [
        SchemaState::WriteOnly,
        SchemaState::DeleteOnly,
        SchemaState::None,
    ] {
        run(&fixture, &mut job).unwrap();
        assert_eq!(job.schema_state, expected, "{:?}", job.error);
    }
    assert_eq!(job.state, JobState::Done);
    assert!(
        fixture
            .reader()
            .get_table(create.schema_id, log_id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .reader()
            .get_table(create.schema_id, fixture.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .is_none()
    );
    assert!(
        fixture
            .pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT MLOG_ID FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={log_id}"
            ))
            .unwrap()
            .is_empty()
    );
}
