// Copyright 2026 AsterSQL.
use super::normal_ddl_fixture::{Fixture, hash, hex};
use astersql_ddl::job_worker::{
    DurableJobExecutor, DurableJobSession, DurableJobStep, JobLease, JobWorker, WorkerType,
};
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
struct Lease;
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        true
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
// Drive the initial action through the production worker transaction while the
// complete normal dispatcher stays unavailable until later build/rollback stages.
struct Initial;
impl DurableJobExecutor for Initial {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &Job) -> Result<bool, String> {
        Ok(true)
    }
    fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
    fn wait_synced(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
    fn step(
        &mut self,
        s: &mut dyn DurableJobSession,
        j: &mut Job,
    ) -> Result<DurableJobStep, String> {
        let mut current = Job::decode(&j.encode(false).unwrap()).unwrap();
        let output = s.with_execution_context(Box::new(move |ctx| {
            let mut stage = None;
            ctx.with_transaction(&mut |txn| {
                stage = Some(txn.StageStatement().map_err(|e| e.to_string())?);
                current.real_start_ts = txn.StartTS();
                Ok(vec![])
            })?;
            if matches!(current.state, JobState::Queueing | JobState::Running) {
                current.state = JobState::Running;
            }
            match astersql_ddl::persistent_actions::step(ctx, &mut current) {
                Ok(v) => {
                    current.last_schema_version = v;
                    ctx.with_transaction(&mut |txn| {
                        txn.ReleaseStatement(stage.unwrap())
                            .map_err(|e| e.to_string())?;
                        Ok(vec![])
                    })?;
                }
                Err(e) => {
                    ctx.with_transaction(&mut |txn| {
                        txn.CleanupStatement(stage.unwrap())
                            .map_err(|e| e.to_string())?;
                        Ok(vec![])
                    })?;
                    current.error = Some(e);
                    current.error_count += 1;
                    current.last_schema_version = 0;
                }
            }
            current.encode(false).map_err(|e| e.to_string())
        }))?;
        *j = Job::decode(&output).map_err(|e| e.to_string())?;
        Ok(DurableJobStep {
            schema_version: j.last_schema_version,
            update_raw_args: false,
            removed: false,
        })
    }
}
fn seed(f: &Fixture, t: &TableInfo) {
    let mut tx = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    tx.Set(
        hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", t.ID).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(t).unwrap(),
    )
    .unwrap();
    tx.Commit(&astersql_kv::Context::default()).unwrap();
}
fn setup(v: JobVersion) -> (Fixture, TableInfo, Job) {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (17,'retained row')")
        .unwrap();
    let mut base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut log = base.clone();
    log.ID += 6000;
    log.Name = astersql_meta_model::ast::NewCIStr("$mlog$normal_ddl_target");
    log.PKIsHandle = false;
    log.Indices.clear();
    for column in &mut log.Columns {
        column.FieldType.DelFlag(
            astersql_parser_mysql::r#type::PriKeyFlag
                | astersql_parser_mysql::r#type::AutoIncrementFlag,
        );
    }
    for (name, tp) in [
        (
            astersql_meta_model::MaterializedViewLogDMLTypeColumnName,
            astersql_parser_mysql::r#type::TypeVarchar,
        ),
        (
            astersql_meta_model::MaterializedViewLogOldNewColumnName,
            astersql_parser_mysql::r#type::TypeTiny,
        ),
    ] {
        let mut column = base.Columns[0].clone();
        column.ID = log.Columns.len() as i64 + 1;
        column.Offset = log.Columns.len() as isize;
        column.Name = astersql_meta_model::ast::NewCIStr(name);
        column.FieldType = astersql_parser_types::NewFieldType(tp);
        column
            .FieldType
            .SetFlag(astersql_parser_mysql::r#type::NotNullFlag);
        column
            .FieldType
            .SetFlen(if tp == astersql_parser_mysql::r#type::TypeVarchar {
                1
            } else {
                4
            });
        log.Columns.push(column);
    }
    log.MaterializedViewLog = Some(astersql_meta_model::MaterializedViewLogInfo {
        BaseTableID: base.ID,
        Columns: base.Columns.iter().map(|c| c.Name.clone()).collect(),
        DependentMViewIDs: vec![7001],
        ..Default::default()
    });
    base.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
        MLogID: log.ID,
        MViewIDs: vec![7001],
    });
    seed(&f, &base);
    seed(&f, &log);
    let mut view = base.clone();
    view.ID += 8000;
    view.Name = astersql_meta_model::ast::NewCIStr("normal_mview");
    view.State = SchemaState::None;
    view.MaterializedViewBase = None;
    view.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo {
        BaseTableIDs: vec![base.ID],
        InitBuildState: astersql_meta_model::MViewInitBuildBuilding,
        SQLContent: "SELECT id,payload FROM test.normal_ddl_target".into(),
        RefreshMethod: "FAST".into(),
        RefreshNext: "DATE_ADD(NOW(), INTERVAL 1 HOUR)".into(),
        ..Default::default()
    });
    let mut j = Job {
        id: 99816,
        tp: 86,
        schema_id: f.db,
        table_id: view.ID,
        schema_name: "test".into(),
        table_name: view.Name.L.clone(),
        state: JobState::Queueing,
        version: v,
        reorg_meta: Some(Default::default()),
        ..Default::default()
    };
    j.raw_args = serde_json::to_vec(&if v == JobVersion::V1 {
        serde_json::json!([view, [log.ID]])
    } else {
        serde_json::json!({"table_info":view,"mlog_table_ids":[log.ID]})
    })
    .unwrap();
    let wire = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},1,'{}','{},{},{}',X'{}',86,0)",j.id,f.db,j.table_id,base.ID,log.ID,hex(&wire))).unwrap();
    (f, view, j)
}
fn run(f: &Fixture, j: &mut Job) -> Result<i64, String> {
    let bytes = astersql_meta::encode_go_ddl_job(j, false).unwrap();
    JobWorker::new(WorkerType::General).transit_persisted_job_step(
        &mut f.pool.acquire().unwrap(),
        &Lease,
        &mut Initial,
        j,
        &bytes,
    )
}
fn check(v: JobVersion) {
    let (f, t, mut j) = setup(v);
    run(&f, &mut j).unwrap();
    assert_eq!(
        j.schema_state,
        SchemaState::WriteReorganization,
        "{:?}",
        j.error
    );
    assert_eq!(j.state, JobState::Running);
    assert!(j.last_schema_version > 0);
    assert!(!astersql_ddl::persistent_actions::handler_available(86));
    let actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
    assert_eq!(actual.State, SchemaState::Public);
    assert_eq!(
        serde_json::to_value(&actual.MaterializedView).unwrap(),
        serde_json::to_value(&t.MaterializedView).unwrap()
    );
    let b = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .MaterializedViewBase
        .unwrap();
    assert_eq!(b.MViewIDs, vec![7001, t.ID]);
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let raw = txn
        .Get(
            &astersql_kv::Context::default(),
            astersql_meta::transaction_meta_string_key(
                format!("Diff:{}", j.last_schema_version).as_bytes(),
            ),
            &[],
        )
        .unwrap()
        .Value;
    txn.Rollback().unwrap();
    let d: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(d["type"], 86);
    assert_eq!(d["table_id"], t.ID);
    assert_eq!(d["old_table_id"], 0);
    assert_eq!(
        d["affected_options"],
        serde_json::json!([
            {"schema_id":f.db,"old_schema_id":f.db,"table_id":f.table,"old_table_id":f.table},
            {"schema_id":f.db,"old_schema_id":f.db,"table_id":b.MLogID,"old_table_id":b.MLogID}
        ])
    );
    let mut queued = f.queue(j.id).unwrap();
    assert_eq!(
        astersql_meta_model::group_2::GetCreateMaterializedViewArgs(&mut queued)
            .unwrap()
            .MLogTableIDs,
        vec![b.MLogID]
    );
    let event = f
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id={}",
            j.id
        ))
        .unwrap();
    assert_eq!(event.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&event[0][0]).unwrap();
    assert_eq!(event["type"], 3);
    assert_eq!(event["table_info"], serde_json::to_value(&actual).unwrap());

    assert_eq!(
        f.reader()
            .get_table(f.db, b.MLogID)
            .unwrap()
            .unwrap()
            .MaterializedViewLog
            .unwrap()
            .DependentMViewIDs,
        vec![7001, t.ID]
    );
    assert_eq!(
        f.queue(j.id).unwrap().schema_state,
        SchemaState::WriteReorganization
    );
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    let rows=f.pool.acquire().unwrap().query(format!("SELECT LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS,NEXT_REFRESH_UNIX_SECONDS FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",t.ID)).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0][0].parse::<u64>().unwrap() > j.real_start_ts);
    assert_eq!(&rows[0][1..], &["<nil>".to_string(), "<nil>".to_string()]);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT id,payload FROM test.normal_ddl_target")
            .unwrap(),
        vec![vec!["17".to_string(), "retained row".to_string()]]
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_1_v1() {
    check(JobVersion::V1)
}
#[test]
fn normal_ddl_plan_create_materialized_view_1_v2() {
    check(JobVersion::V2)
}

fn replace(f: &Fixture, j: &mut Job) {
    let bytes = astersql_meta::encode_go_ddl_job(j, false).unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
            hex(&bytes),
            j.id
        ))
        .unwrap();
}
fn rewrite_args(j: &mut Job, t: &TableInfo, logs: &[i64]) {
    j.raw_args = serde_json::to_vec(&if j.version == JobVersion::V1 {
        serde_json::json!([t, logs])
    } else {
        serde_json::json!({"table_info":t,"mlog_table_ids":logs})
    })
    .unwrap();
}
#[test]
fn normal_ddl_plan_create_materialized_view_1_dependency_errors() {
    for v in [JobVersion::V1, JobVersion::V2] {
        for case in 0..19 {
            let (f, mut t, mut j) = setup(v);
            let mut base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
            let log_id = base.MaterializedViewBase.as_ref().unwrap().MLogID;
            let mut log = f.reader().get_table(f.db, log_id).unwrap().unwrap();
            let mut logs = vec![log_id];
            match case {
                0 => t.MaterializedView.as_mut().unwrap().BaseTableIDs = vec![],
                1 => t.MaterializedView.as_mut().unwrap().BaseTableIDs = vec![0],
                2 => t.MaterializedView.as_mut().unwrap().BaseTableIDs = vec![base.ID, base.ID],
                3 => t.MaterializedView.as_mut().unwrap().BaseTableIDs = vec![999999],
                4 => base.MaterializedViewBase = None,
                5 => base.State = SchemaState::WriteOnly,
                6 => base.TempTableType = astersql_meta_model::TempTableLocal,
                7 => log.MaterializedViewLog.as_mut().unwrap().BaseTableID = 900000,
                8 => log.State = SchemaState::WriteOnly,
                9 => logs = vec![0],
                10 => {
                    log.ID += 1;
                    log.MaterializedViewLog.as_mut().unwrap().BaseTableID = 900000;
                    logs = vec![log.ID];
                }
                11 => {
                    let mut collision = t.clone();
                    collision.ID += 1;
                    collision.State = SchemaState::Public;
                    seed(&f, &collision);
                }
                12 => t.MaterializedView = None,
                13 => base.View = Some(Default::default()),
                14 => base.Sequence = Some(Default::default()),
                15 => {
                    base.Partition = Some(astersql_meta_model::PartitionInfo {
                        Enable: true,
                        ..Default::default()
                    })
                }
                16 => log.MaterializedViewLog = None,
                17 => base.MaterializedViewBase.as_mut().unwrap().MLogID = 999999,
                _ => j.schema_id = 999999,
            }
            seed(&f, &base);
            seed(&f, &log);
            rewrite_args(&mut j, &t, &logs);
            replace(&f, &mut j);
            run(&f, &mut j).unwrap();
            assert_eq!(j.state, JobState::Cancelled, "case {case} {:?}", j.error);
            assert!(j.error.is_some());
            assert_eq!(j.last_schema_version, 0);
            assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
            assert_eq!(
                f.reader()
                    .get_table(f.db, f.table)
                    .unwrap()
                    .unwrap()
                    .MaterializedViewBase
                    .as_ref()
                    .map(|b| b.MViewIDs.clone()),
                base.MaterializedViewBase
                    .as_ref()
                    .map(|b| b.MViewIDs.clone())
            );
            assert!(
                f.pool
                    .acquire()
                    .unwrap()
                    .query(format!(
                        "SELECT MVIEW_ID FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                        t.ID
                    ))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_1_prewrite_error_and_unfinished_stage() {
    let (f, t, mut j) = setup(JobVersion::V2);
    f.pool
        .acquire()
        .unwrap()
        .query("DROP TABLE mysql.tidb_mview_refresh_info")
        .unwrap();
    run(&f, &mut j).unwrap();
    assert_eq!(j.state, JobState::Rollingback, "{:?}", j.error);
    assert!(
        j.error
            .as_ref()
            .unwrap()
            .contains("tidb_mview_refresh_info")
    );
    assert_eq!(j.last_schema_version, 0);
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .unwrap()
            .MViewIDs,
        vec![7001]
    );
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    // This task cannot execute rollback or the data build phase.
    run(&f, &mut j).unwrap();
    assert!(j.error.as_ref().unwrap().contains("stage unavailable"));
}
#[test]
fn normal_ddl_plan_create_materialized_view_1_independent_commit_conflict_restart() {
    let (mut f, t, mut j) = setup(JobVersion::V1);
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_mview_refresh_info (MVIEW_ID,LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS,NEXT_REFRESH_UNIX_SECONDS) VALUES ({},123,456,789)",t.ID)).unwrap();
    let original = f.queue(j.id).unwrap().encode(false).unwrap();
    let mut first = f.pool.acquire().unwrap();
    first.begin().unwrap();
    Initial.step(&mut first, &mut j).unwrap();
    // The independent record is visible while worker metadata is uncommitted.
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    let row=f.pool.acquire().unwrap().query(format!("SELECT LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS,NEXT_REFRESH_UNIX_SECONDS FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",t.ID)).unwrap();
    let independent = row[0][0].parse::<u64>().unwrap();
    assert!(independent > j.real_start_ts);
    assert_eq!(&row[0][1..], &["<nil>".to_string(), "789".to_string()]);
    let mut other = f.pool.acquire().unwrap();
    other.begin().unwrap();
    other
        .with_transaction(Box::new(|txn| {
            astersql_meta::TransactionMutator::new(txn).gen_schema_version()?;
            Ok(vec![])
        }))
        .unwrap();
    other.commit().unwrap();
    let error = first.commit().unwrap_err();
    assert!(
        error.to_lowercase().contains("conflict") || error.contains(astersql_kv::TxnRetryableMark),
        "{error}"
    );
    first.rollback();
    drop(first);
    drop(other);
    assert_eq!(f.queue(j.id).unwrap().encode(false).unwrap(), original);
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                t.ID
            ))
            .unwrap()[0][0],
        independent.to_string()
    );
    f.pool.close();
    f.pool = super::system_session::SystemSessionPool::new(f.domain.clone());
    j = f.queue(j.id).unwrap();
    run(&f, &mut j).unwrap();
    assert_eq!(
        j.schema_state,
        SchemaState::WriteReorganization,
        "{:?}",
        j.error
    );
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_some());
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",
                t.ID
            ))
            .unwrap()[0][0]
            .parse::<u64>()
            .unwrap()
            > independent
    );
    // Resume at the durable phase; initial metadata is not duplicated or finalized.
    let version = j.last_schema_version;
    f.domain.reload().unwrap();
    run(&f, &mut j).unwrap();
    assert!(j.error.is_none(), "{:?}", j.error);
    assert!(j.snapshot_ver > 0);
    assert_eq!(
        f.queue(j.id).unwrap().schema_state,
        SchemaState::WriteReorganization
    );
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    assert!(version > 0);
}

#[test]
fn normal_ddl_plan_create_materialized_view_2_import_select() {
    let f = Fixture::new();
    let mut session = f.pool.acquire().unwrap();
    session
        .query("INSERT INTO test.normal_ddl_target VALUES (17,'source row')")
        .unwrap();
    session.query("CREATE TABLE test.normal_mview_import (id BIGINT PRIMARY KEY,payload VARCHAR(255), KEY payload_idx(payload))").unwrap();
    session.query("IMPORT INTO test.normal_mview_import FROM (SELECT id,payload FROM test.normal_ddl_target) WITH disable_precheck, thread=1, disk_quota='50GiB'").unwrap();
    assert_eq!(
        session
            .query("SELECT id,payload FROM test.normal_mview_import")
            .unwrap(),
        vec![vec!["17".to_string(), "source row".to_string()]]
    );
}
fn build_data(v: JobVersion) {
    let (f, t, mut j) = setup(v);
    j.reorg_meta = Some(Default::default());
    replace(&f, &mut j);
    run(&f, &mut j).unwrap();
    assert_eq!(j.schema_state, SchemaState::WriteReorganization);
    f.domain.reload().unwrap();
    run(&f, &mut j).unwrap();
    assert!(j.error.is_none(), "{:?}", j.error);
    assert!(j.snapshot_ver > 0);
    assert_eq!(f.queue(j.id).unwrap().snapshot_ver, j.snapshot_ver);
    assert_eq!(j.state, JobState::Running);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT id,payload FROM test.normal_mview")
            .unwrap(),
        vec![vec!["17".to_string(), "retained row".to_string()]]
    );
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    assert!(
        f.reader()
            .get_table(f.db, t.ID)
            .unwrap()
            .unwrap()
            .MaterializedView
            .unwrap()
            .InitBuildState
            == astersql_meta_model::MViewInitBuildBuilding
    );
    assert!(!astersql_ddl::persistent_actions::handler_available(86));
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_v1() {
    build_data(JobVersion::V1);
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_v2() {
    build_data(JobVersion::V2);
}

#[test]
fn normal_ddl_plan_create_materialized_view_2_snapshot_and_restart() {
    for version in [JobVersion::V1, JobVersion::V2] {
        let (mut f, t, mut j) = setup(version);
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES (18,'before build')")
            .unwrap();
        run(&f, &mut j).unwrap();
        f.domain.reload().unwrap();
        run(&f, &mut j).unwrap();
        assert!(j.error.is_none(), "{:?}", j.error);
        let snapshot = j.snapshot_ver;
        assert!(snapshot > j.real_start_ts);
        assert_eq!(j.get_row_count(), 2);
        assert_eq!(f.queue(j.id).unwrap().get_row_count(), 2);
        run(&f, &mut j).unwrap();
        assert_eq!(j.snapshot_ver, snapshot);
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT COUNT(*) FROM test.normal_mview")
                .unwrap()[0][0],
            "2"
        );
        let maintenance=f.pool.acquire().unwrap().query(format!("SELECT LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",t.ID)).unwrap();
        assert_ne!(maintenance[0][0], snapshot.to_string());
        assert_eq!(maintenance[0][1], "<nil>");
        // As Go does, a new owner without the completed reorg context rejects
        // residual physical rows instead of silently rebuilding live data.
        f.pool.close();
        f.pool = super::system_session::SystemSessionPool::new(f.domain.clone());
        j = f.queue(j.id).unwrap();
        run(&f, &mut j).unwrap();
        assert_eq!(j.state, JobState::Rollingback);
        assert!(j.error.as_ref().unwrap().contains("residual build rows"));
        assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_build_error_and_admin_state() {
    for version in [JobVersion::V1, JobVersion::V2] {
        for case in 0..4 {
            let (f, mut t, mut j) = setup(version);
            if case == 0 {
                t.MaterializedView.as_mut().unwrap().SQLContent =
                    "SELECT missing FROM test.normal_ddl_target".into();
                rewrite_args(
                    &mut j,
                    &t,
                    &[f.reader()
                        .get_table(f.db, f.table)
                        .unwrap()
                        .unwrap()
                        .MaterializedViewBase
                        .unwrap()
                        .MLogID],
                );
                replace(&f, &mut j);
            }
            if case == 1 {
                j.reorg_meta = None;
                replace(&f, &mut j);
            }
            run(&f, &mut j).unwrap();
            f.domain.reload().unwrap();
            if case == 2 {
                j.state = JobState::Cancelling;
                replace(&f, &mut j);
            }
            if case == 3 {
                j.state = JobState::Paused;
                replace(&f, &mut j);
            }
            run(&f, &mut j).unwrap();
            assert_eq!(j.snapshot_ver, 0);
            assert_eq!(
                j.state,
                if case == 3 {
                    JobState::Paused
                } else {
                    JobState::Rollingback
                },
                "case {case} {:?}",
                j.error
            );
            assert_eq!(
                f.pool
                    .acquire()
                    .unwrap()
                    .query("SELECT COUNT(*) FROM test.normal_mview")
                    .unwrap()[0][0],
                "0"
            );
            assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_commit_conflict() {
    let (f, _, mut j) = setup(JobVersion::V2);
    run(&f, &mut j).unwrap();
    f.domain.reload().unwrap();
    let original = f.queue(j.id).unwrap().encode(false).unwrap();
    let mut first = f.pool.acquire().unwrap();
    first.begin().unwrap();
    Initial.step(&mut first, &mut j).unwrap();
    assert!(j.snapshot_ver > 0);
    // The data commit precedes the worker's job checkpoint commit.
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT COUNT(*) FROM test.normal_mview")
            .unwrap()[0][0],
        "1"
    );
    let mut other = f.pool.acquire().unwrap();
    other
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET processing=1 WHERE job_id={}",
            j.id
        ))
        .unwrap();
    first
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
            hex(&astersql_meta::encode_go_ddl_job(&mut j, false).unwrap()),
            j.id
        ))
        .unwrap();
    assert!(first.commit().is_err());
    first.rollback();
    assert_eq!(f.queue(j.id).unwrap().encode(false).unwrap(), original);
    j = f.queue(j.id).unwrap();
    run(&f, &mut j).unwrap();
    assert!(j.error.is_none(), "{:?}", j.error);
    assert!(j.snapshot_ver > 0);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT COUNT(*) FROM test.normal_mview")
            .unwrap()[0][0],
        "1"
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_import_errors() {
    let f = Fixture::new();
    let session = f.pool.acquire().unwrap();
    session
        .query("INSERT INTO test.normal_ddl_target VALUES (17,'source row')")
        .unwrap();
    session
        .query("CREATE TABLE test.normal_mview_import (id BIGINT PRIMARY KEY,payload VARCHAR(255))")
        .unwrap();
    let duplicate=session.query("IMPORT INTO test.normal_mview_import FROM (SELECT id,payload FROM test.normal_ddl_target UNION ALL SELECT id,payload FROM test.normal_ddl_target) WITH disable_precheck").unwrap_err();
    assert!(duplicate.contains("duplicate key"), "{duplicate}");
    assert_eq!(
        session
            .query("SELECT COUNT(*) FROM test.normal_mview_import")
            .unwrap()[0][0],
        "0"
    );
    session.query("IMPORT INTO test.normal_mview_import FROM (SELECT id,payload FROM test.normal_ddl_target)").unwrap();
    let nonempty=session.query("IMPORT INTO test.normal_mview_import FROM (SELECT id,payload FROM test.normal_ddl_target)").unwrap_err();
    assert!(nonempty.contains("not empty"), "{nonempty}");
}

struct EpochLease(u64);
impl JobLease for EpochLease {
    fn owner_epoch(&self) -> u64 {
        self.0
    }
    fn is_owner(&self) -> bool {
        true
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_2_owner_epoch() {
    let (f, _, mut j) = setup(JobVersion::V2);
    for _ in 0..2 {
        let bytes = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
        JobWorker::new(WorkerType::General)
            .transit_persisted_job_step(
                &mut f.pool.acquire().unwrap(),
                &EpochLease(1),
                &mut Initial,
                &mut j,
                &bytes,
            )
            .unwrap();
        f.domain.reload().unwrap();
    }
    assert!(j.snapshot_ver > 0);
    let bytes = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    JobWorker::new(WorkerType::General)
        .transit_persisted_job_step(
            &mut f.pool.acquire().unwrap(),
            &EpochLease(2),
            &mut Initial,
            &mut j,
            &bytes,
        )
        .unwrap();
    assert_eq!(j.state, JobState::Rollingback);
    assert!(j.error.unwrap().contains("residual build rows"));
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT COUNT(*) FROM test.normal_mview")
            .unwrap()[0][0],
        "1"
    );
}

#[test]
fn normal_ddl_plan_create_materialized_view_2_definition_precision() {
    let (f, mut t, mut j) = setup(JobVersion::V2);
    t.MaterializedView.as_mut().unwrap().SQLContent =
        "SELECT id,id/3 AS payload FROM test.normal_ddl_target".into();
    t.MaterializedView
        .as_mut()
        .unwrap()
        .DefinitionDivPrecisionIncrement = 6;
    let log = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .MaterializedViewBase
        .unwrap()
        .MLogID;
    rewrite_args(&mut j, &t, &[log]);
    replace(&f, &mut j);
    run(&f, &mut j).unwrap();
    f.domain.reload().unwrap();
    run(&f, &mut j).unwrap();
    assert!(j.error.is_none(), "{:?}", j.error);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_mview")
            .unwrap()[0][0],
        "5.666667"
    );
}

#[test]
fn normal_ddl_plan_create_materialized_view_3_publication() {
    for v in [JobVersion::V1, JobVersion::V2] {
        let (f, t, mut j) = setup(v);
        run(&f, &mut j).unwrap();
        f.domain.reload().unwrap();
        run(&f, &mut j).unwrap();
        run(&f, &mut j).unwrap();
        assert_eq!(j.state, JobState::Done, "{:?}", j.error);
        assert_eq!(j.schema_state, SchemaState::Public);
        assert!(
            f.reader()
                .get_table(f.db, t.ID)
                .unwrap()
                .unwrap()
                .MaterializedView
                .unwrap()
                .InitBuildState
                == astersql_meta_model::MViewInitBuildReady
        );
        let rows=f.pool.acquire().unwrap().query(format!("SELECT LAST_SUCCESS_READ_TSO,LAST_SUCCESS_REFRESH_END_UNIX_SECONDS,NEXT_REFRESH_UNIX_SECONDS FROM mysql.tidb_mview_refresh_info WHERE MVIEW_ID={}",t.ID)).unwrap();
        assert_eq!(rows[0][0], j.snapshot_ver.to_string());
        assert!(rows[0][1].parse::<i64>().unwrap() > 0);
        assert!(rows[0][2].parse::<i64>().unwrap() > rows[0][1].parse::<i64>().unwrap());
        assert!(astersql_ddl::persistent_actions::handler_available(86));
    }
}

#[test]
fn normal_ddl_plan_create_materialized_view_3_cancel_dispatch_prerequisite() {
    use astersql_ddl::table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor};
    struct Barrier;
    impl DdlSchemaBarrier for Barrier {
        fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
            Ok(())
        }
        fn wait(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
            Ok(())
        }
    }
    struct Policy;
    impl DdlJobPolicy for Policy {
        fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &Job) -> Result<bool, String> {
            Ok(true)
        }
        fn error_limit(&self) -> i64 {
            3
        }
        fn mdl_owner(&self) -> Option<String> {
            None
        }
    }
    let (f, t, mut j) = setup(JobVersion::V2);
    run(&f, &mut j).unwrap();
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_some());
    j.state = JobState::Cancelling;
    replace(&f, &mut j);
    let bytes = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    let mut executor = NormalDdlExecutor {
        barrier: Barrier,
        policy: Policy,
        sequence: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
    };
    JobWorker::new(WorkerType::General)
        .transit_persisted_job_step(
            &mut f.pool.acquire().unwrap(),
            &Lease,
            &mut executor,
            &mut j,
            &bytes,
        )
        .unwrap();
    assert_eq!(
        j.state,
        JobState::Rollingback,
        "normal dispatcher must retain rollback before history; object remains: {}",
        f.reader().get_table(f.db, t.ID).unwrap().is_some()
    );
    assert!(f.queue(j.id).is_some());
}

#[test]
fn import_select_rejects_enabled_ttl_even_with_disable_precheck() {
    let f = Fixture::new();
    let mut session = f.pool.acquire().unwrap();
    session
        .query("CREATE TABLE test.ttl_import_source (id int primary key, created_at datetime)")
        .unwrap();
    session
        .query("INSERT INTO test.ttl_import_source VALUES (17,'2026-10-04 01:02:03')")
        .unwrap();
    session.query("CREATE TABLE test.ttl_import_target (id int primary key, created_at datetime) TTL = `created_at` + INTERVAL 1 DAY").unwrap();
    assert!(
        f.domain
            .table_by_name("test", "ttl_import_target")
            .unwrap()
            .TTLInfo
            .as_ref()
            .unwrap()
            .Enable
    );
    let error = session.query("IMPORT INTO test.ttl_import_target FROM (SELECT * FROM test.ttl_import_source) WITH DISABLE_PRECHECK").unwrap_err();
    assert_eq!(
        error.to_string(),
        "SQL error: [executor:8173]PreCheck failed: target table has TTL enabled, please disable TTL before IMPORT INTO"
    );
    for sql in [
        "IMPORT INTO test.ttl_import_target FROM (SELECT * FROM test.ttl_import_source)",
        "IMPORT INTO test.ttl_import_target FROM '/file.csv'",
        "IMPORT INTO test.ttl_import_target FROM '/file.csv' WITH SPLIT_FILE",
    ] {
        let error = session.query(sql).unwrap_err();
        assert_eq!(
            error.to_string(),
            "SQL error: [executor:8173]PreCheck failed: target table has TTL enabled, please disable TTL before IMPORT INTO"
        );
    }
    assert!(
        session
            .query("SELECT * FROM test.ttl_import_target")
            .unwrap()
            .is_empty()
    );
    // The current SQL runtime lacks ALTER TTL_ENABLE; create the same real
    // target definition with TTL disabled to verify the precheck's OFF branch.
    session.query("DROP TABLE test.ttl_import_target").unwrap();
    session.query("CREATE TABLE test.ttl_import_target (id int primary key, created_at datetime) TTL = `created_at` + INTERVAL 1 DAY TTL_ENABLE='OFF'").unwrap();
    assert!(
        !f.domain
            .table_by_name("test", "ttl_import_target")
            .unwrap()
            .TTLInfo
            .as_ref()
            .unwrap()
            .Enable
    );
    session.query("IMPORT INTO test.ttl_import_target FROM (SELECT * FROM test.ttl_import_source) WITH DISABLE_PRECHECK").unwrap();
    assert_eq!(
        session
            .query("SELECT id FROM test.ttl_import_target")
            .unwrap(),
        vec![vec!["17".to_string()]]
    );
}
