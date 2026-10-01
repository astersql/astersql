// Copyright 2026 AsterSQL.
use super::super::normal_ddl_fixture::{Fixture, hex};
use astersql_ddl::{
    job_scheduler::JobScheduler,
    job_worker::{DurableJobSession, JobLease, JobWorker, WorkerType},
    table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor},
};
use astersql_meta_model::{
    SchemaState, TableInfo,
    group_3::{Job, JobState, JobVersion},
};
use std::sync::{Arc, atomic::AtomicI64};
struct Lease;
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        true
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
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
fn executor() -> NormalDdlExecutor<Barrier, Policy> {
    NormalDdlExecutor {
        barrier: Barrier,
        policy: Policy,
        sequence: Arc::new(AtomicI64::new(0)),
    }
}
fn run(f: &Fixture) {
    assert_eq!(
        JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex)
        )
        .schedule_persisted(&mut f.pool.acquire().unwrap(), &Lease, &mut executor(), 0),
        Ok(1)
    );
}
fn table(f: &Fixture) -> TableInfo {
    let base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut columns = Vec::new();
    for c in &base.Columns {
        let mut tp = c.FieldType.clone();
        tp.DelFlag(
            astersql_parser_mysql::r#type::PriKeyFlag
                | astersql_parser_mysql::r#type::UniqueKeyFlag
                | astersql_parser_mysql::r#type::MultipleKeyFlag
                | astersql_parser_mysql::r#type::AutoIncrementFlag
                | astersql_parser_mysql::r#type::OnUpdateNowFlag,
        );
        columns.push(super::super::ast::ColumnDef {
            Name: super::super::ast::ColumnName {
                Name: c.Name.clone(),
                ..Default::default()
            },
            Tp: tp,
            Options: vec![],
        });
    }
    for (name, ty, len) in [
        (
            astersql_meta_model::MaterializedViewLogDMLTypeColumnName,
            astersql_parser_mysql::r#type::TypeVarchar,
            1,
        ),
        (
            astersql_meta_model::MaterializedViewLogOldNewColumnName,
            astersql_parser_mysql::r#type::TypeTiny,
            4,
        ),
    ] {
        let mut tp = astersql_parser_types::NewFieldType(ty);
        tp.SetFlen(len);
        tp.SetFlag(astersql_parser_mysql::r#type::NotNullFlag);
        columns.push(super::super::ast::ColumnDef {
            Name: super::super::ast::ColumnName {
                Name: super::super::ast::NewCIStr(name),
                ..Default::default()
            },
            Tp: tp,
            Options: vec![],
        });
    }
    let create = super::super::ast::CreateTableStmt {
        Table: super::super::ast::TableName {
            Schema: super::super::ast::NewCIStr("test"),
            Name: super::super::ast::NewCIStr("$mlog$normal_ddl_target"),
            ..Default::default()
        },
        Cols: columns,
        ..Default::default()
    };
    let ctx = astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
    let mut t = astersql_ddl::BuildTableInfoFromAST(&ctx, &create).unwrap();
    t.ID = base.ID + 6000;
    t.State = SchemaState::None;
    t.MaterializedViewLog = Some(astersql_meta_model::MaterializedViewLogInfo {
        BaseTableID: f.table,
        Columns: base.Columns.iter().map(|c| c.Name.clone()).collect(),
        DependentMViewIDs: vec![7001, 7002],
        LogAccumulationAlertRows: Some(1234),
        ..Default::default()
    });
    t
}
fn insert(f: &Fixture, t: &TableInfo, v: JobVersion) -> Job {
    let mut j = Job {
        id: 99814,
        tp: 85,
        schema_id: f.db,
        table_id: t.ID,
        schema_name: "test".into(),
        table_name: t.Name.L.clone(),
        state: JobState::Queueing,
        version: v,
        snapshot_ver: 98765,
        cdc_write_source: 41,
        ..Default::default()
    };
    j.raw_args = serde_json::to_vec(&if v == JobVersion::V1 {
        serde_json::json!([t])
    } else {
        serde_json::json!({"table_info":t})
    })
    .unwrap();
    let wire = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    let mut decoded = astersql_meta::decode_go_history_job(&wire).unwrap();
    assert!(
        astersql_meta_model::group_2::GetCreateMaterializedViewLogArgs(&mut decoded)
            .unwrap()
            .TableInfo
            .is_some()
    );
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{},{}',X'{}',85,0)",j.id,f.db,t.ID,f.table,hex(&wire))).unwrap();
    j
}
fn create(v: JobVersion) {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (17,'retained row')")
        .unwrap();
    let t = table(&f);
    let j = insert(&f, &t, v);
    run(&f);
    let actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
    assert_eq!(actual.State, SchemaState::Public);
    assert_eq!(
        serde_json::to_value(&actual.MaterializedViewLog).unwrap(),
        serde_json::to_value(&t.MaterializedViewLog).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&actual.Columns).unwrap(),
        serde_json::to_value(&t.Columns).unwrap()
    );
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .unwrap()
            .MLogID,
        t.ID
    );
    let queued = f.queue(j.id).unwrap();
    assert_eq!(queued.state, JobState::Done);
    assert!(queued.real_start_ts > 0);
    assert_eq!(queued.snapshot_ver, 98765);
    assert_eq!(actual.UpdateTS, queued.real_start_ts);
    assert_eq!(
        queued
            .binlog_info
            .as_ref()
            .unwrap()
            .multiple_table_infos
            .len(),
        2
    );
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "SELECT MLOG_ID FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
            t.ID
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99814")
            .unwrap()
            .len(),
        1
    );
    run(&f);
    assert!(f.queue(j.id).is_none());
    assert_eq!(
        f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
        JobState::Synced
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=17")
            .unwrap()[0][0],
        "retained row"
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_v1() {
    create(JobVersion::V1)
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_v2() {
    create(JobVersion::V2)
}

fn seed(f: &Fixture, t: &TableInfo) {
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        super::super::normal_ddl_fixture::hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", t.ID).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(t).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
}
fn durable(f: &Fixture, id: i64) -> Job {
    f.queue(id)
        .or_else(|| f.reader().get_history_ddl_job(id).unwrap())
        .unwrap()
}
fn assert_absent(f: &Fixture, id: i64) {
    assert!(f.reader().get_table(f.db, id).unwrap().is_none());
    assert!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .as_ref()
            .is_none_or(|b| b.MLogID != id)
    );
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT MLOG_ID FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={id}"
            ))
            .unwrap()
            .is_empty()
    );
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99814")
            .unwrap()
            .is_empty()
    );
}
fn diff(f: &Fixture, j: &Job) -> serde_json::Value {
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
    serde_json::from_slice(&raw).unwrap()
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_metadata_and_args() {
    for version in [JobVersion::V1, JobVersion::V2] {
        let f = Fixture::new();
        let mut base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        base.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: 0,
            MViewIDs: vec![51, 52],
        });
        seed(&f, &base);
        let t = table(&f);
        let j = insert(&f, &t, version);
        run(&f);
        let mut result = f.queue(j.id).unwrap();
        assert_eq!(
            f.reader()
                .get_table(f.db, f.table)
                .unwrap()
                .unwrap()
                .MaterializedViewBase
                .unwrap()
                .MViewIDs,
            vec![51, 52]
        );
        let d = diff(&f, &result);
        assert_eq!(d["type"], 85);
        assert_eq!(d["table_id"], t.ID);
        assert_eq!(d["old_table_id"], 0);
        assert_eq!(
            d["affected_options"],
            serde_json::json!([{"schema_id":f.db,"old_schema_id":f.db,"table_id":f.table,"old_table_id":f.table}])
        );
        let args =
            astersql_meta_model::group_2::GetCreateMaterializedViewLogArgs(&mut result).unwrap();
        assert_eq!(
            serde_json::to_value(args.TableInfo.unwrap()).unwrap()["state"],
            serde_json::to_value(SchemaState::Public).unwrap()
        );
        assert_eq!(
            result.binlog_info.unwrap().multiple_table_infos[0].ID,
            f.table
        );
        run(&f);
        let mut history = f.reader().get_history_ddl_job(j.id).unwrap().unwrap();
        assert_eq!(history.snapshot_ver, 98765);
        assert_eq!(history.cdc_write_source, 41);
        assert_eq!(
            serde_json::to_value(
                astersql_meta_model::group_2::GetCreateMaterializedViewLogArgs(&mut history)
                    .unwrap()
                    .TableInfo
                    .unwrap()
            )
            .unwrap()["state"],
            serde_json::to_value(SchemaState::Public).unwrap()
        );
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_rejects_invalid_metadata() {
    for v in [JobVersion::V1, JobVersion::V2] {
        for case in 0..12 {
            let f = Fixture::new();
            let mut t = table(&f);
            let mut base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
            let expected = match case {
                0 => {
                    t.MaterializedViewLog = None;
                    "8204"
                }
                1 => {
                    t.MaterializedViewLog.as_mut().unwrap().BaseTableID = 0;
                    "8204"
                }
                2 => {
                    t.MaterializedViewLog.as_mut().unwrap().BaseTableID = 900000;
                    "1146"
                }
                3 => {
                    base.View = Some(Default::default());
                    "BASE TABLE"
                }
                4 => {
                    base.Sequence = Some(Default::default());
                    "BASE TABLE"
                }
                5 => {
                    base.TempTableType = astersql_meta_model::TempTableGlobal;
                    "BASE TABLE"
                }
                6 => {
                    base.MaterializedView = Some(Default::default());
                    "BASE TABLE"
                }
                7 => {
                    base.MaterializedViewShadow = Some(Default::default());
                    "BASE TABLE"
                }
                8 => {
                    base.MaterializedViewLog = Some(Default::default());
                    "BASE TABLE"
                }
                9 => {
                    base.State = SchemaState::WriteOnly;
                    "not in public"
                }
                10 => {
                    base.Partition = Some(astersql_meta_model::PartitionInfo {
                        Enable: true,
                        ..Default::default()
                    });
                    "partition table"
                }
                _ => {
                    base.MaterializedViewBase =
                        Some(astersql_meta_model::MaterializedViewBaseInfo {
                            MLogID: 123,
                            ..Default::default()
                        });
                    "1050"
                }
            };
            seed(&f, &base);
            let j = insert(&f, &t, v);
            run(&f);
            let result = durable(&f, j.id);
            assert_eq!(
                result.state,
                JobState::Cancelled,
                "case {case}: {:?}",
                result.error
            );
            assert!(
                result.error.as_ref().unwrap().contains(expected),
                "case {case}: {:?}",
                result.error
            );
            assert_absent(&f, t.ID);
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_schedule_matrix() {
    for (start, next, expected) in [
        ("", "", None),
        (
            "",
            "CAST('2039-01-01 00:00:00' AS DATETIME)",
            Some(2177452800_i64),
        ),
        (
            "NOW(6)",
            "CAST('2039-01-01 00:00:00' AS DATETIME)",
            Some(2177452800_i64),
        ),
        (
            "CAST('2038-01-01 01:02:03' AS DATETIME)",
            "CAST('2039-01-01 00:00:00' AS DATETIME)",
            Some(2145920523),
        ),
        (
            "CAST('2001-01-01' AS DATETIME)",
            "CAST('2039-01-01 00:00:00' AS DATETIME)",
            Some(2177452800_i64),
        ),
        (
            "CAST(NULL AS DATETIME)",
            "CAST('2039-01-01' AS DATETIME)",
            None,
        ),
        ("", "CAST(NULL AS DATETIME)", None),
        ("CAST('2001-01-01' AS DATETIME)", "", Some(978307200)),
    ] {
        let f = Fixture::new();
        let mut t = table(&f);
        let m = t.MaterializedViewLog.as_mut().unwrap();
        m.PurgeStartWith = start.into();
        m.PurgeNext = next.into();
        f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_mlog_purge_info (MLOG_ID,NEXT_PURGE_UNIX_SECONDS,LAST_PURGED_TSO) VALUES ({},123,456)",t.ID)).unwrap();
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        assert_eq!(
            durable(&f, j.id).state,
            JobState::Done,
            "{:?}",
            durable(&f, j.id).error
        );
        let row=f.pool.acquire().unwrap().query(format!("SELECT NEXT_PURGE_UNIX_SECONDS,LAST_PURGED_TSO FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",t.ID)).unwrap();
        assert_eq!(
            row[0][0],
            expected.map_or("<nil>".into(), |n| n.to_string()),
            "{start} / {next}"
        );
        assert_eq!(row[0][1], "456");
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_purge_table_missing_rolls_back() {
    let f = Fixture::new();
    let t = table(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("DROP TABLE mysql.tidb_mlog_purge_info")
        .unwrap();
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let result = f.queue(j.id).unwrap();
    assert_eq!(result.state, JobState::Rollingback, "{:?}", result.error);
    assert!(result.error.unwrap().contains("8204"));
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .is_none()
    );
    run(&f);
    let result = f.queue(j.id).unwrap();
    assert_eq!(result.state, JobState::RollbackDone);
    let d = diff(&f, &result);
    assert_eq!(d["table_id"], 0);
    assert_eq!(d["old_table_id"], t.ID);
    run(&f);
    assert_eq!(
        f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
        JobState::RollbackDone
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_schedule_error_has_no_orphans() {
    let f = Fixture::new();
    let mut t = table(&f);
    t.MaterializedViewLog.as_mut().unwrap().PurgeNext = "1".into();
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let result = f.queue(j.id).unwrap();
    assert_eq!(result.state, JobState::Running);
    assert_eq!(result.error_count, 1);
    assert!(
        result.error.as_ref().unwrap().contains("expected DATE"),
        "{:?}",
        result.error
    );
    assert_absent(&f, t.ID);
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_commit_conflict_and_restart() {
    use astersql_ddl::job_worker::DurableJobExecutor;
    let mut f = Fixture::new();
    let t = table(&f);
    let mut j = insert(&f, &t, JobVersion::V2);
    let original = f.queue(j.id).unwrap().encode(false).unwrap();
    let mut first = f.pool.acquire().unwrap();
    first.begin().unwrap();
    executor().step(&mut first, &mut j).unwrap();
    assert_absent(&f, t.ID);
    let mut other = f.pool.acquire().unwrap();
    other.begin().unwrap();
    other
        .with_transaction(Box::new(|txn| {
            astersql_meta::TransactionMutator::new(txn).gen_schema_version()?;
            Ok(Vec::new())
        }))
        .unwrap();
    other.commit().unwrap();
    let err = first.commit().unwrap_err();
    assert!(
        err.to_lowercase().contains("conflict") || err.contains(astersql_kv::TxnRetryableMark),
        "{err}"
    );
    first.rollback();
    assert_absent(&f, t.ID);
    assert_eq!(f.queue(j.id).unwrap().encode(false).unwrap(), original);
    f.pool.close();
    f.pool = super::SystemSessionPool::new(f.domain.clone());
    run(&f);
    assert_eq!(
        durable(&f, j.id).state,
        JobState::Done,
        "{:?}",
        durable(&f, j.id).error
    );
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Synced);
}

fn replace_job(f: &Fixture, j: &mut Job) {
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
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_bad_wire_and_schema() {
    for version in [JobVersion::V1, JobVersion::V2] {
        for case in 0..4 {
            let f = Fixture::new();
            let t = table(&f);
            let mut j = insert(&f, &t, version);
            match case {
                0 => j.raw_args = b"false".to_vec(),
                1 => {
                    j.raw_args = if version == JobVersion::V1 {
                        b"[null]".to_vec()
                    } else {
                        b"{\"table_info\":null}".to_vec()
                    }
                }
                2 => j.schema_id = 900000,
                _ => j.schema_name = "MySQL".into(),
            };
            replace_job(&f, &mut j);
            run(&f);
            let result = durable(&f, j.id);
            assert_eq!(
                result.state,
                JobState::Cancelled,
                "case {case} {:?}",
                result.error
            );
            if case == 2 {
                assert!(result.error.unwrap().contains("1049"));
            } else if case == 3 {
                assert!(result.error.unwrap().contains("BASE TABLE"));
            }
            assert_absent(&f, t.ID);
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_name_collision() {
    let f = Fixture::new();
    let mut existing = table(&f);
    existing.ID += 1;
    existing.State = SchemaState::Public;
    seed(&f, &existing);
    let t = table(&f);
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let result = durable(&f, j.id);
    assert_eq!(result.state, JobState::Cancelled);
    assert!(result.error.unwrap().contains("1050"));
    assert_absent(&f, t.ID);
    assert!(f.reader().get_table(f.db, existing.ID).unwrap().is_some());
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_notifier_failure_is_atomic() {
    let f = Fixture::new();
    let t = table(&f);
    f.pool.acquire().unwrap().query("INSERT INTO mysql.tidb_ddl_notifier (ddl_job_id,sub_job_id,schema_change,processed_by_flag) VALUES (99814,-1,X'7b7d',0)").unwrap();
    let j = insert(&f, &t, JobVersion::V2);
    let error = JobScheduler::new(
        JobWorker::new(WorkerType::General),
        JobWorker::new(WorkerType::AddIndex),
    )
    .schedule_persisted(&mut f.pool.acquire().unwrap(), &Lease, &mut executor(), 0)
    .unwrap_err();
    assert!(error.contains("Duplicate"), "{error}");
    let result = f.queue(j.id).unwrap();
    assert_eq!(result.state, JobState::Queueing);
    assert_eq!(result.error_count, 0);
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .is_none()
    );
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "SELECT MLOG_ID FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
                t.ID
            ))
            .unwrap()
            .is_empty()
    );
    f.pool
        .acquire()
        .unwrap()
        .query("DELETE FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99814")
        .unwrap();
    run(&f);
    assert_eq!(
        durable(&f, j.id).state,
        JobState::Done,
        "{:?}",
        durable(&f, j.id).error
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_rollback_existing_metadata() {
    for bound in [true, false] {
        let f = Fixture::new();
        let mut t = table(&f);
        t.State = SchemaState::Public;
        seed(&f, &t);
        let mut base = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        base.MaterializedViewBase = Some(astersql_meta_model::MaterializedViewBaseInfo {
            MLogID: if bound { t.ID } else { 123 },
            MViewIDs: vec![51, 52],
        });
        seed(&f, &base);
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        for key in ["TID", "IID", "TARID"] {
            txn.Set(
                super::super::normal_ddl_fixture::hash(
                    format!("DB:{}", f.db).as_bytes(),
                    format!("{key}:{}", t.ID).as_bytes(),
                ),
                b"100".to_vec(),
            )
            .unwrap();
        }
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "INSERT INTO mysql.tidb_mlog_purge_info (MLOG_ID) VALUES ({})",
                t.ID
            ))
            .unwrap();
        let mut j = insert(&f, &t, JobVersion::V2);
        j.state = JobState::Rollingback;
        replace_job(&f, &mut j);
        run(&f);
        assert_eq!(f.queue(j.id).unwrap().state, JobState::RollbackDone);
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
        let b = f
            .reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .MaterializedViewBase
            .unwrap();
        assert_eq!(b.MLogID, if bound { 0 } else { 123 });
        assert_eq!(b.MViewIDs, vec![51, 52]);
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        for key in ["TID", "IID", "TARID"] {
            assert!(
                txn.Get(
                    &astersql_kv::Context::default(),
                    super::super::normal_ddl_fixture::hash(
                        format!("DB:{}", f.db).as_bytes(),
                        format!("{key}:{}", t.ID).as_bytes()
                    ),
                    &[]
                )
                .is_err()
            );
        }
        txn.Rollback().unwrap();
        let d = diff(&f, &f.queue(j.id).unwrap());
        assert_eq!(d["table_id"], 0);
        assert_eq!(d["old_table_id"], t.ID);
        assert_eq!(d["affected_options"][0]["table_id"], f.table);
        run(&f);
        assert_eq!(
            f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
            JobState::RollbackDone
        );
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_utc_sqlmode_and_statement_vars() {
    let f = Fixture::new();
    let mut t = table(&f);
    let info = t.MaterializedViewLog.as_mut().unwrap();
    info.PurgeNext = "CAST(\"2030-01-02 10:00:00\" AS DATETIME)".into();
    info.PurgeScheduleSQLMode = astersql_parser_mysql::r#const::SQLMode(
        astersql_parser_mysql::r#const::ModeStrictAllTables.0
            | astersql_parser_mysql::r#const::ModeNoBackslashEscapes.0,
    );
    let mut j = insert(&f, &t, JobVersion::V2);
    let mut session = f.pool.acquire().unwrap();
    session.query("SET time_zone='+08:00'").unwrap();
    session.query("SET sql_mode='ANSI_QUOTES'").unwrap();
    let timezone_before = session
        .concrete()
        .call(|s| Ok(format!("{:?}", *s.time_zone.borrow())))
        .unwrap();
    assert!(timezone_before.contains("+08"));
    let expected = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    JobWorker::new(WorkerType::General)
        .transit_persisted_job_step(&mut session, &Lease, &mut executor(), &mut j, &expected)
        .unwrap();
    let timezone_after = session
        .concrete()
        .call(|s| Ok(format!("{:?}", *s.time_zone.borrow())))
        .unwrap();
    assert_eq!(timezone_after, timezone_before);
    assert!(session.query("SELECT @@sql_mode").unwrap()[0][0].contains("ANSI_QUOTES"));
    assert_eq!(
        session
            .query(format!(
                "SELECT NEXT_PURGE_UNIX_SECONDS FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
                t.ID
            ))
            .unwrap()[0][0],
        "1893578400"
    );
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_1_date_add_now() {
    let f = Fixture::new();
    let mut t = table(&f);
    t.MaterializedViewLog.as_mut().unwrap().PurgeNext =
        "DATE_ADD(NOW(6), INTERVAL 40 MINUTE)".into();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    assert_eq!(
        durable(&f, j.id).state,
        JobState::Done,
        "{:?}",
        durable(&f, j.id).error
    );
    let next: f64 = f
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "SELECT NEXT_PURGE_UNIX_SECONDS FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
            t.ID
        ))
        .unwrap()[0][0]
        .parse()
        .unwrap();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(next >= (before + 2400) as f64 && next <= (after + 2400) as f64);
}

#[test]
fn normal_ddl_plan_create_materialized_view_log_2_maintenance_dml_retry() {
    for v in [JobVersion::V1, JobVersion::V2] {
        let f = Fixture::new();
        let mut t = table(&f);
        t.MaterializedViewLog.as_mut().unwrap().PurgeNext = "CAST('2039-01-01' AS DATETIME)".into();
        // Keep the system table present but make its maintenance write invalid.
        // This exercises a real SQL error rather than the missing-table rollback branch.
        let s = f.pool.acquire().unwrap();
        s.query("DROP TABLE mysql.tidb_mlog_purge_info").unwrap();
        s.query("CREATE TABLE mysql.tidb_mlog_purge_info (MLOG_ID TINYINT PRIMARY KEY, NEXT_PURGE_UNIX_SECONDS BIGINT)")
            .unwrap();
        let j = insert(&f, &t, v);
        run(&f);
        let failed = f.queue(j.id).unwrap();
        assert_eq!(failed.state, JobState::Running);
        assert_eq!(failed.error_count, 1);
        assert!(failed.error.is_some());
        assert_absent(&f, t.ID);
        assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
        s.query("DROP TABLE mysql.tidb_mlog_purge_info").unwrap();
        s.query("CREATE TABLE mysql.tidb_mlog_purge_info (MLOG_ID BIGINT PRIMARY KEY, NEXT_PURGE_UNIX_SECONDS BIGINT, LAST_PURGED_TSO BIGINT)").unwrap();
        run(&f);
        assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
        let rows = s
            .query(format!(
                "SELECT NEXT_PURGE_UNIX_SECONDS FROM mysql.tidb_mlog_purge_info WHERE MLOG_ID={}",
                t.ID
            ))
            .unwrap();
        assert_eq!(rows, vec![vec!["2177452800".to_string()]]);
        run(&f);
        assert_eq!(
            f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
            JobState::Synced
        );
    }
}

struct RecoverBarrier {
    fail: bool,
    seen: Vec<i64>,
}
impl DdlSchemaBarrier for RecoverBarrier {
    fn recover(&mut self, job: &Job, lease: &dyn JobLease) -> Result<(), String> {
        self.wait(job, job.last_schema_version, lease)
    }
    fn wait(&mut self, _: &Job, version: i64, _: &dyn JobLease) -> Result<(), String> {
        if version > 0 {
            self.seen.push(version);
            if self.fail {
                return Err("schema/MDL synchronization unavailable".into());
            }
        }
        Ok(())
    }
}
struct MdlPolicy;
impl DdlJobPolicy for MdlPolicy {
    fn runnable(&mut self, _: &mut dyn DurableJobSession, _: &Job) -> Result<bool, String> {
        Ok(true)
    }
    fn error_limit(&self) -> i64 {
        3
    }
    fn mdl_owner(&self) -> Option<String> {
        Some("mlog-owner".into())
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_log_2_barrier_recovery_and_payload() {
    for v in [JobVersion::V1, JobVersion::V2] {
        let mut f = Fixture::new();
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES (17,'retained row')")
            .unwrap();
        let t = table(&f);
        let j = insert(&f, &t, v);
        let mut e = NormalDdlExecutor {
            barrier: RecoverBarrier {
                fail: true,
                seen: vec![],
            },
            policy: MdlPolicy,
            sequence: Arc::new(AtomicI64::new(0)),
        };
        let mut sched = JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex),
        );
        let err = sched
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &Lease, &mut e, 0)
            .unwrap_err();
        assert!(err.contains("synchronization unavailable"), "{err}");
        let done = f.queue(j.id).unwrap();
        assert_eq!(done.state, JobState::Done);
        assert_eq!(e.barrier.seen, vec![done.last_schema_version]);
        assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
        let s = f.pool.acquire().unwrap();
        let mdl = s
            .query("SELECT version,table_ids,owner_id FROM mysql.tidb_mdl_info WHERE job_id=99814")
            .unwrap();
        assert_eq!(mdl.len(), 1);
        assert_eq!(mdl[0][0], done.last_schema_version.to_string());
        assert!(mdl[0][1].split(',').any(|id| id == t.ID.to_string()));
        assert!(mdl[0][1].split(',').any(|id| id == f.table.to_string()));
        assert_eq!(mdl[0][2], "mlog-owner");
        let rows=s.query("SELECT sub_job_id,schema_change,processed_by_flag FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99814").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "-1");
        assert_eq!(rows[0][2], "0");
        let event: serde_json::Value = serde_json::from_str(&rows[0][1]).unwrap();
        assert_eq!(event["type"], 3);
        assert_eq!(
            event["table_info"],
            serde_json::to_value(f.reader().get_table(f.db, t.ID).unwrap().unwrap()).unwrap()
        );
        drop(s);
        f.pool.close();
        f.pool = super::SystemSessionPool::new(f.domain.clone());
        let mut recovered = NormalDdlExecutor {
            barrier: RecoverBarrier {
                fail: false,
                seen: vec![],
            },
            policy: MdlPolicy,
            sequence: Arc::new(AtomicI64::new(0)),
        };
        let mut sched = JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex),
        );
        sched
            .schedule_persisted(&mut f.pool.acquire().unwrap(), &Lease, &mut recovered, 0)
            .unwrap();
        assert_eq!(recovered.barrier.seen, vec![done.last_schema_version]);
        assert!(f.queue(j.id).is_none());
        assert_eq!(
            f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
            JobState::Synced
        );
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99814")
                .unwrap(),
            vec![vec![rows[0][1].clone()]]
        );
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT payload FROM test.normal_ddl_target WHERE id=17")
                .unwrap()[0][0],
            "retained row"
        );
    }
}
