// Copyright 2026 AsterSQL.
use super::normal_ddl_fixture::{Fixture, hex};
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
    let mut t = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    t.ID += 5000;
    t.Name = astersql_meta_model::ast::NewCIStr("normal_ddl_created");
    t.State = SchemaState::None;
    t
}
fn insert(f: &Fixture, t: &TableInfo, v: JobVersion) -> Job {
    insert_check(f, t, v, true)
}
fn insert_check(f: &Fixture, t: &TableInfo, v: JobVersion, check: bool) -> Job {
    let mut j = Job {
        id: 99812,
        tp: 3,
        schema_id: f.db,
        table_id: t.ID,
        schema_name: "test".into(),
        table_name: t.Name.L.clone(),
        state: JobState::Queueing,
        version: v,
        cdc_write_source: 41,
        ..Default::default()
    };
    let info = serde_json::to_value(t).unwrap();
    j.raw_args = serde_json::to_vec(&if v == JobVersion::V1 {
        serde_json::json!([info, check])
    } else {
        serde_json::json!({"table_info":info,"fk_check":check})
    })
    .unwrap();
    let wire = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
    let mut decoded = astersql_meta::decode_go_history_job(&wire).unwrap();
    let args = astersql_meta_model::group_2::GetCreateTableArgs(&mut decoded).unwrap();
    let full: TableInfo =
        serde_json::from_value(serde_json::to_value(args.TableInfo.unwrap()).unwrap()).unwrap();
    assert_eq!(full.Columns.len(), 2);
    assert_eq!(args.FKCheck, check);
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',3,0)",j.id,f.db,t.ID,hex(&wire))).unwrap();
    j
}
fn durable(f: &Fixture, id: i64) -> Job {
    f.queue(id)
        .or_else(|| f.reader().get_history_ddl_job(id).unwrap())
        .unwrap()
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
    let result = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
    assert_eq!(result.State, SchemaState::Public);
    assert_eq!(
        serde_json::to_value(&result.Columns).unwrap(),
        serde_json::to_value(&t.Columns).unwrap()
    );
    assert!(result.UpdateTS > 0);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    run(&f);
    assert!(f.queue(j.id).is_none());
    let h = f.reader().get_history_ddl_job(j.id).unwrap().unwrap();
    assert_eq!(h.state, JobState::Synced);
    assert_eq!(h.cdc_write_source, 41);
    assert_eq!(h.binlog_info.unwrap().table_info.unwrap().ID, t.ID);
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
fn normal_ddl_plan_create_table_v1() {
    create(JobVersion::V1)
}
#[test]
fn normal_ddl_plan_create_table_v2() {
    create(JobVersion::V2)
}
fn partition(t: &mut TableInfo, tp: astersql_meta_model::ast::PartitionType) {
    t.Partition = Some(astersql_meta_model::PartitionInfo {
        Enable: true,
        Type: tp,
        Expr: "id".into(),
        Num: 2,
        Definitions: vec![
            astersql_meta_model::PartitionDefinition {
                ID: t.ID + 1,
                Name: astersql_meta_model::ast::NewCIStr("p0"),
                LessThan: vec!["10".into()],
                InValues: vec![vec!["1".into()], vec!["NULL".into()]],
                ..Default::default()
            },
            astersql_meta_model::PartitionDefinition {
                ID: t.ID + 2,
                Name: astersql_meta_model::ast::NewCIStr("p1"),
                LessThan: vec!["MAXVALUE".into()],
                InValues: vec![vec!["DEFAULT".into()]],
                ..Default::default()
            },
        ],
        ..Default::default()
    });
}
#[test]
fn normal_ddl_plan_create_table_partition_not_cancelled() {
    use astersql_meta_model::ast::model::*;
    for tp in [
        PartitionTypeHash,
        PartitionTypeRange,
        PartitionTypeKey,
        PartitionTypeList,
    ] {
        let f = Fixture::new();
        let mut t = table(&f);
        partition(&mut t, tp);
        if tp == PartitionTypeKey {
            let p = t.Partition.as_mut().unwrap();
            p.Expr.clear();
            p.Columns = vec![astersql_meta_model::ast::NewCIStr("id")];
        }
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let queued = durable(&f, j.id);
        assert_eq!(queued.state, JobState::Done, "{tp:?}: {:?}", queued.error);
        let actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
        assert_eq!(actual.Partition.unwrap().Definitions.len(), 2);
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Synced);
    }
}
fn fk(t: &mut TableInfo, parent: &str) {
    t.ForeignKeys = vec![astersql_meta_model::FKInfo {
        Name: astersql_meta_model::ast::NewCIStr("fk_parent"),
        RefSchema: astersql_meta_model::ast::NewCIStr("test"),
        RefTable: astersql_meta_model::ast::NewCIStr(parent),
        RefCols: vec![astersql_meta_model::ast::NewCIStr("id")],
        Cols: vec![astersql_meta_model::ast::NewCIStr("id")],
        Version: 1,
        ..Default::default()
    }];
}
#[test]
fn normal_ddl_plan_create_table_foreign_key_restart() {
    for v in [JobVersion::V1, JobVersion::V2] {
        let f = Fixture::new();
        let mut t = table(&f);
        fk(&mut t, "normal_ddl_target");
        let j = insert(&f, &t, v);
        run(&f);
        assert_eq!(durable(&f, j.id).schema_state, SchemaState::DeleteOnly);
        let initial = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
        assert_eq!(initial.State, SchemaState::DeleteOnly);
        assert_eq!(initial.ForeignKeys[0].ID, 1);
        assert_eq!(initial.ForeignKeys[0].State, SchemaState::Public);
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Done);
        let actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
        assert_eq!(actual.State, SchemaState::Public);
        assert_eq!(actual.ForeignKeys[0].ID, 1);
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Synced);
    }
}
#[test]
fn normal_ddl_plan_create_table_foreign_key_missing_checked() {
    let f = Fixture::new();
    let mut t = table(&f);
    fk(&mut t, "absent_parent");
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let h = durable(&f, j.id);
    assert_eq!(h.state, JobState::Cancelled);
    assert!(h.error.unwrap().contains("1146"));
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
}
#[test]
fn normal_ddl_plan_create_table_foreign_key_missing_unchecked() {
    let f = Fixture::new();
    let mut t = table(&f);
    fk(&mut t, "absent_parent");
    let j = insert_check(&f, &t, JobVersion::V2, false);
    run(&f);
    assert_eq!(durable(&f, j.id).schema_state, SchemaState::DeleteOnly);
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Done);
}
#[test]
fn normal_ddl_plan_create_table_duplicate_name_atomic() {
    let f = Fixture::new();
    let mut t = table(&f);
    t.Name = astersql_meta_model::ast::NewCIStr("normal_ddl_target");
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let h = durable(&f, j.id);
    assert_eq!(h.state, JobState::Cancelled);
    assert!(h.error.unwrap().contains("1050"));
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert!(f.reader().get_table(f.db, f.table).unwrap().is_some());
}

fn counter(f: &Fixture, t: i64, prefix: &str) -> i64 {
    let txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let key = super::normal_ddl_fixture::hash(
        format!("DB:{}", f.db).as_bytes(),
        format!("{prefix}:{t}").as_bytes(),
    );
    let v = txn.Get(&astersql_kv::Context::default(), key, &[]).unwrap();
    std::str::from_utf8(&v.Value).unwrap().parse().unwrap()
}
#[test]
fn normal_ddl_plan_create_table_auto_id_independent_notification_retry() {
    for cache in [1, 100] {
        let f = Fixture::new();
        let mut t = table(&f);
        t.Version = 5;
        t.Columns[0]
            .FieldType
            .AddFlag(astersql_meta_model::mysql::AutoIncrementFlag);
        t.AutoIDCache = cache;
        t.AutoIncID = 111;
        let j = insert(&f, &t, JobVersion::V2);
        f.pool.acquire().unwrap().query("INSERT INTO mysql.tidb_ddl_notifier (ddl_job_id,sub_job_id,schema_change,processed_by_flag) VALUES (99812,-1,'{}',0)").unwrap();
        let error = JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex),
        )
        .schedule_persisted(&mut f.pool.acquire().unwrap(), &Lease, &mut executor(), 0)
        .unwrap_err();
        assert!(error.contains("Duplicate entry"), "{error}");
        let failed = durable(&f, j.id);
        assert_eq!(failed.state, JobState::Queueing, "{:?}", failed.error);
        assert_eq!(failed.error_count, 0);
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
        assert!(counter(&f, t.ID, if t.SepAutoInc() { "IID" } else { "TID" }) >= 110);
        f.pool
            .acquire()
            .unwrap()
            .query("DELETE FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99812")
            .unwrap();
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Done);
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_some());
        let rows = f
            .pool
            .acquire()
            .unwrap()
            .query("SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99812")
            .unwrap();
        assert_eq!(rows.len(), 1);
        let event: serde_json::Value = serde_json::from_str(&rows[0][0]).unwrap();
        assert_eq!(event["type"], 3);
        assert_eq!(event["table_info"]["id"], t.ID);
    }
}
#[test]
fn normal_ddl_plan_create_table_commit_conflict_and_restart() {
    use astersql_ddl::job_worker::DurableJobExecutor;
    let f = Fixture::new();
    let t = table(&f);
    let mut j = insert(&f, &t, JobVersion::V2);
    let original = f.queue(j.id).unwrap().encode(false).unwrap();
    let mut first = f.pool.acquire().unwrap();
    first.begin().unwrap();
    executor().step(&mut first, &mut j).unwrap();
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    let mut other = f.pool.acquire().unwrap();
    other.begin().unwrap();
    other
        .with_transaction(Box::new(|txn| {
            astersql_meta::TransactionMutator::new(txn).gen_schema_version()?;
            Ok(Vec::new())
        }))
        .unwrap();
    other.commit().unwrap();
    let error = first.commit().unwrap_err();
    assert!(
        error.to_lowercase().contains("conflict") || error.contains(astersql_kv::TxnRetryableMark),
        "{error}"
    );
    first.rollback();
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    assert_eq!(f.queue(j.id).unwrap().encode(false).unwrap(), original);
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99812")
            .unwrap()
            .is_empty()
    );
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Done);
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Synced);
}

#[derive(Default)]
struct TtlBoundary {
    calls: Arc<std::sync::Mutex<Vec<(i64, bool)>>>,
    fail: bool,
    fail_at: Option<usize>,
    fail_delete: bool,
    role: String,
    recycled: Arc<std::sync::Mutex<Vec<u64>>>,
    fail_recycle: bool,
}
impl astersql_extworkload::Manager for TtlBoundary {
    fn Close(&mut self) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn Role(&self) -> String {
        if self.role.is_empty() {
            astersql_config::RoleMaster.into()
        } else {
            self.role.clone()
        }
    }
    fn Meta(&self) -> Option<&astersql_extworkload::keyspacepb::KeyspaceMeta> {
        None
    }
    fn InitializeGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn AbortGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RegisterGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RecycleGCV2(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn UpdateGCLifeTime(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: std::time::Duration,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RegisterTTLTask(
        &mut self,
        _: &astersql_extworkload::context::Context,
        id: i64,
        enabled: bool,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.calls.lock().unwrap().push((id, enabled));
        if self.fail || self.fail_at == Some(self.calls.lock().unwrap().len()) {
            Err(std::io::Error::other("controller unavailable").into())
        } else {
            Ok(())
        }
    }
    fn DeleteTTLTableInfo(
        &mut self,
        _: &astersql_extworkload::context::Context,
        id: i64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.calls.lock().unwrap().push((-id, false));
        if self.fail_delete {
            Err("delete controller unavailable".into())
        } else {
            Ok(())
        }
    }
    fn RecycleTTLTask(
        &mut self,
        _: &astersql_extworkload::context::Context,
        create_time: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        self.recycled.lock().unwrap().push(create_time);
        if self.fail_recycle {
            Err("recycle controller unavailable".into())
        } else {
            Ok(())
        }
    }
    fn UpdateTTLJobEnable(
        &mut self,
        _: &astersql_extworkload::context::Context,
        enabled: bool,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        let _ = enabled;
        Ok(())
    }
    fn RegisterAutoAnalyze(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RecycleAutoAnalyze(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
}

#[test]
fn normal_ddl_plan_create_table_ttl_controller_success_failure_disabled() {
    for (enabled, fail, role) in [
        (true, false, "master"),
        (true, true, "master"),
        (false, true, "master"),
        (true, false, "ttl"),
        (true, true, "ttl"),
    ] {
        let f = Fixture::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                calls: calls.clone(),
                fail,
                role: role.into(),
                ..Default::default()
            })));
        let mut t = table(&f);
        t.TTLInfo = Some(astersql_meta_model::TTLInfo {
            ColumnName: astersql_meta_model::ast::NewCIStr("id"),
            IntervalExprStr: "1".into(),
            Enable: enabled,
            ..Default::default()
        });
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        if enabled && fail {
            let h = durable(&f, j.id);
            assert_eq!(h.state, JobState::Cancelled);
            assert!(h.error.unwrap().contains("controller unavailable"));
            assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
            assert!(
                f.pool
                    .acquire()
                    .unwrap()
                    .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99812")
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(durable(&f, j.id).state, JobState::Done);
            assert!(f.reader().get_table(f.db, t.ID).unwrap().is_some());
        }
        let actual = calls.lock().unwrap();
        if enabled {
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].0, t.ID);
        } else {
            assert!(actual.is_empty());
        }
    }
}

fn meta_value(f: &Fixture, key: astersql_kv::Key) -> Vec<u8> {
    let mut t = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let v = t
        .Get(&astersql_kv::Context::default(), key, &[])
        .unwrap()
        .Value;
    t.Rollback().unwrap();
    v
}
fn assert_diff(f: &Fixture, j: &Job, old: i64) {
    let v = j.last_schema_version;
    assert!(v > 0);
    let raw = meta_value(
        f,
        astersql_meta::transaction_meta_string_key(format!("Diff:{v}").as_bytes()),
    );
    let diff: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(diff["type"], 3);
    assert_eq!(diff["schema_id"], f.db);
    assert_eq!(diff["table_id"], j.table_id);
    assert_eq!(diff["old_table_id"], old);
}
fn seed(f: &Fixture, t: &TableInfo) {
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    txn.Set(
        super::normal_ddl_fixture::hash(
            format!("DB:{}", f.db).as_bytes(),
            format!("Table:{}", t.ID).as_bytes(),
        ),
        astersql_meta_model::EncodeTableInfo(t).unwrap(),
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
}
#[test]
fn normal_ddl_plan_create_table_restore_public_and_legacy_write_only() {
    for state in [SchemaState::Public, SchemaState::WriteOnly] {
        let f = Fixture::new();
        let mut t = table(&f);
        fk(&mut t, "normal_ddl_created");
        t.State = state;
        t.MaxForeignKeyID = 7;
        if state == SchemaState::WriteOnly {
            t.ForeignKeys[0].ID = 7;
            t.ForeignKeys[0].State = SchemaState::Public;
            seed(&f, &t);
        }
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        if state == SchemaState::Public {
            let current = durable(&f, j.id);
            assert_eq!(current.schema_state, SchemaState::DeleteOnly);
            assert_diff(&f, &current, 0);
            assert_eq!(
                f.reader()
                    .get_table(f.db, t.ID)
                    .unwrap()
                    .unwrap()
                    .ForeignKeys[0]
                    .ID,
                8
            );
            run(&f);
        }
        let current = durable(&f, j.id);
        assert_eq!(current.state, JobState::Done);
        assert_diff(&f, &current, t.ID);
        assert_eq!(
            f.reader().get_table(f.db, t.ID).unwrap().unwrap().State,
            SchemaState::Public
        );
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Synced);
    }
}
#[test]
fn normal_ddl_plan_create_table_validation_errors_atomic() {
    for case in 0..10 {
        let f = Fixture::new();
        let mut t = table(&f);
        let code = match case {
            0 => {
                t.Columns[0].State = SchemaState::None;
                "8046"
            }
            1 => {
                t.PlacementPolicyRef = Some(astersql_meta_model::PolicyRefInfo {
                    ID: 9999,
                    Name: astersql_meta_model::ast::NewCIStr("absent"),
                });
                "8249"
            }
            2 => {
                fk(&mut t, "normal_ddl_target");
                t.ForeignKeys[0].RefCols[0] = astersql_meta_model::ast::NewCIStr("absent");
                "3734"
            }
            3 => {
                fk(&mut t, "normal_ddl_target");
                t.ForeignKeys[0].RefSchema = astersql_meta_model::ast::NewCIStr("absent_db");
                "1049"
            }
            4 => {
                fk(&mut t, "normal_ddl_target");
                t.Columns[0]
                    .FieldType
                    .AddFlag(astersql_meta_model::mysql::UnsignedFlag);
                "3780"
            }
            5 => {
                fk(&mut t, "normal_ddl_target");
                t.TempTableType = astersql_meta_model::TempTableGlobal;
                "1215"
            }
            6 => {
                fk(&mut t, "normal_ddl_target");
                partition(&mut t, astersql_meta_model::ast::model::PartitionTypeHash);
                "1506"
            }
            7 => {
                fk(&mut t, "normal_ddl_target");
                t.ForeignKeys[0].Cols[0] = astersql_meta_model::ast::NewCIStr("absent");
                "1072"
            }
            8 => {
                fk(&mut t, "normal_ddl_created");
                t.PKIsHandle = false;
                t.Indices.clear();
                "1822"
            }
            _ => {
                fk(&mut t, "normal_ddl_created");
                t.Columns[0].GeneratedExprString = "payload".into();
                t.Columns[0].GeneratedStored = false;
                "3733"
            }
        };
        let version = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let h = durable(&f, j.id);
        assert_eq!(h.state, JobState::Cancelled);
        assert!(h.error.unwrap().contains(code));
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
        assert_eq!(
            f.reader().get_schema_version_with_non_empty_diff().unwrap(),
            version
        );
    }
}
#[test]
fn normal_ddl_plan_create_table_partial_index_expression() {
    let f = Fixture::new();
    let mut t = table(&f);
    t.Indices = vec![astersql_meta_model::IndexInfo {
        ID: 3,
        Name: astersql_meta_model::ast::NewCIStr("partial"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: astersql_meta_model::ast::NewCIStr("id"),
            Offset: 0,
            Length: -1,
            ..Default::default()
        }],
        ConditionExprString: "id > 0".into(),
        ..Default::default()
    }];
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let current = durable(&f, j.id);
    assert_eq!(current.state, JobState::Done, "{:?}", current.error);
    assert_diff(&f, &current, 0);
    assert_eq!(
        f.reader().get_table(f.db, t.ID).unwrap().unwrap().Indices[0].ConditionExprString,
        "id > 0"
    );
}

struct PdBoundary {
    calls: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    fail: bool,
}
impl astersql_domain_infosync::PdHttpClient for PdBoundary {
    fn set_placement_rule_bundles(
        &self,
        b: &[astersql_ddl_placement::Bundle],
        partial: bool,
    ) -> astersql_domain_infosync::Result<()> {
        assert!(partial);
        self.calls
            .lock()
            .unwrap()
            .push(serde_json::to_value(b).unwrap());
        if self.fail {
            Err(astersql_domain_infosync::Error::External(
                "PD unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
}
#[test]
fn normal_ddl_plan_create_table_pd_placement_and_tiflash_requests() {
    let original = astersql_domain_infosync::getGlobalInfoSyncer().ok();
    for fail in [false, true] {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = Arc::new(PdBoundary {
            calls: calls.clone(),
            fail,
        });
        let _sync = astersql_domain_infosync::GlobalInfoSyncerInit(
            "task12".into(),
            Arc::new(|| 1),
            None,
            None,
            Some(client),
            astersql_domain_infosync::Codec::default(),
            false,
            None,
        )
        .unwrap();
        let tiflash = astersql_domain_infosync::NewMockTiFlash();
        astersql_domain_infosync::SetMockTiFlash(tiflash.clone()).unwrap();
        let f = Fixture::new();
        let mut t = table(&f);
        partition(&mut t, astersql_meta_model::ast::model::PartitionTypeHash);
        let p = t.Partition.as_mut().unwrap();
        p.AddingDefinitions = vec![astersql_meta_model::PartitionDefinition {
            ID: t.ID + 3,
            Name: astersql_meta_model::ast::NewCIStr("p2"),
            ..Default::default()
        }];
        t.TiFlashReplica = Some(astersql_meta_model::TiFlashReplicaInfo {
            Count: 1,
            LocationLabels: vec!["zone".into()],
            ..Default::default()
        });
        let policy = astersql_meta_model::group_3::PolicyInfo {
            ID: 77,
            Name: astersql_meta_model::ast::NewCIStr("task12_policy"),
            State: SchemaState::Public,
            PlacementSettings: astersql_meta_model::group_3::PlacementSettings {
                Followers: 2,
                ..Default::default()
            },
        };
        let mut bytes = vec![0];
        bytes.extend(serde_json::to_vec(&policy).unwrap());
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        txn.Set(
            super::normal_ddl_fixture::hash(b"Policies", b"Policy:77"),
            bytes,
        )
        .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        t.PlacementPolicyRef = Some(astersql_meta_model::PolicyRefInfo {
            ID: 77,
            Name: policy.Name,
        });
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let current = durable(&f, j.id);
        if fail {
            assert_eq!(current.state, JobState::Cancelled);
            assert!(current.error.unwrap().contains("failed to notify PD"));
            assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
            assert_eq!(calls.lock().unwrap().len(), 4);
        } else {
            assert_eq!(current.state, JobState::Done, "{:?}", current.error);
            assert_diff(&f, &current, 0);
            assert_eq!(calls.lock().unwrap().len(), 1);
        }
        let requests = calls.lock().unwrap();
        assert!(!requests[0].as_array().unwrap().is_empty());
        assert!(
            requests[0][0]["rules"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| !r["start_key"].as_str().unwrap().is_empty())
        );
        let rules = tiflash.GlobalTiFlashPlacementRules.read().unwrap();
        assert_eq!(rules.len(), 3);
        for id in [t.ID + 1, t.ID + 2, t.ID + 3] {
            let r = &rules[&format!("table-{id}-r")];
            assert_eq!(r.Count, 1);
            assert_eq!(r.LocationLabels, ["zone"]);
        }
        assert!(tiflash.SyncStatus.read().unwrap()[&(t.ID + 3)].Accel);
    }
    if let Some(original) = original {
        astersql_domain_infosync::setGlobalInfoSyncer(original);
    }
}

struct AffinityBoundary {
    requests: Arc<
        std::sync::Mutex<
            Vec<
                std::collections::HashMap<
                    String,
                    Vec<astersql_domain_affinity::AffinityGroupKeyRange>,
                >,
            >,
        >,
    >,
    fail: bool,
}
impl astersql_domain_affinity::PdClient for AffinityBoundary {
    fn create_affinity_groups(
        &self,
        _: &dyn astersql_domain_affinity::Context,
        g: &std::collections::HashMap<String, Vec<astersql_domain_affinity::AffinityGroupKeyRange>>,
        skip: bool,
    ) -> Result<
        std::collections::HashMap<String, astersql_domain_affinity::AffinityGroupState>,
        astersql_domain_affinity::AffinityError,
    > {
        assert!(skip);
        self.requests.lock().unwrap().push(g.clone());
        if self.fail {
            Err(astersql_domain_affinity::AffinityError::http_status(
                503,
                "PD unavailable",
            ))
        } else {
            Ok(g.iter()
                .map(|(id, r)| {
                    (
                        id.clone(),
                        astersql_domain_affinity::AffinityGroupState::new(id, r.len()),
                    )
                })
                .collect())
        }
    }
    fn batch_delete_affinity_groups(
        &self,
        _: &dyn astersql_domain_affinity::Context,
        _: &[String],
        _: bool,
    ) -> Result<(), astersql_domain_affinity::AffinityError> {
        Ok(())
    }
    fn get_affinity_groups(
        &self,
        _: &dyn astersql_domain_affinity::Context,
        _: &[String],
    ) -> Result<
        std::collections::HashMap<String, astersql_domain_affinity::AffinityGroupState>,
        astersql_domain_affinity::AffinityError,
    > {
        Ok(Default::default())
    }
    fn get_all_affinity_groups(
        &self,
        _: &dyn astersql_domain_affinity::Context,
    ) -> Result<
        std::collections::HashMap<String, astersql_domain_affinity::AffinityGroupState>,
        astersql_domain_affinity::AffinityError,
    > {
        Ok(Default::default())
    }
}
#[test]
fn normal_ddl_plan_create_table_affinity_requests_failure_and_range_encoding() {
    for level in ["table", "partition"] {
        for fail in [false, true] {
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            astersql_domain_affinity::init_manager(Some(Arc::new(AffinityBoundary {
                requests: requests.clone(),
                fail,
            })));
            let f = Fixture::new();
            let mut t = table(&f);
            if level == "partition" {
                partition(&mut t, astersql_meta_model::ast::model::PartitionTypeHash);
            }
            t.Affinity = Some(astersql_meta_model::TableAffinityInfo {
                Level: level.into(),
            });
            let j = insert(&f, &t, JobVersion::V2);
            run(&f);
            let current = durable(&f, j.id);
            if fail {
                assert_eq!(current.state, JobState::Cancelled);
                assert!(
                    current
                        .error
                        .unwrap()
                        .contains("failed to create table affinity")
                );
                assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
            } else {
                assert_eq!(current.state, JobState::Done, "{:?}", current.error);
            }
            let calls = requests.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let ids = if level == "table" {
                vec![t.ID]
            } else {
                vec![t.ID + 1, t.ID + 2]
            };
            assert_eq!(calls[0].len(), ids.len());
            for id in ids {
                let group = if level == "table" {
                    format!("_tidb_t_{id}")
                } else {
                    format!("_tidb_pt_{}_p{id}", t.ID)
                };
                let r = &calls[0][&group][0];
                let start = astersql_tablecodec::GenTablePrefix(id);
                let end = astersql_tablecodec::GenTablePrefix(id + 1);
                assert_eq!(
                    r.start_key,
                    astersql_util_codec::EncodeBytes(Vec::new(), &start.0)
                );
                assert_eq!(
                    r.end_key,
                    astersql_util_codec::EncodeBytes(Vec::new(), &end.0)
                );
            }
        }
    }
    astersql_domain_affinity::init_manager(None);
}

#[test]
fn normal_ddl_plan_create_table_partition_columns_pruning_and_legacy_bounds() {
    use astersql_meta_model::ast::model::*;
    for tp in [PartitionTypeRange, PartitionTypeList] {
        let f = Fixture::new();
        let mut t = table(&f);
        partition(&mut t, tp);
        let p = t.Partition.as_mut().unwrap();
        p.Expr.clear();
        p.Columns = vec![astersql_meta_model::ast::NewCIStr("id")];
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Done);
        let mut actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
        let loaded =
            astersql_table_tables::tables::table_from_meta_for_validation(&mut actual).unwrap();
        let expr = loaded.partition_expression.as_ref().unwrap();
        assert_eq!(expr.ColumnOffset, [0]);
        if tp == PartitionTypeRange {
            assert!(expr.ForRangeColumnsPruning.as_ref().unwrap().LessThan[1][0].is_none());
            assert_eq!(expr.UpperBounds.len(), 2);
        } else {
            let prune = expr.ForListPruning.as_ref().unwrap();
            assert_eq!(prune.DefaultPartitionIdx, 1);
            assert_eq!(prune.ColPrunes.len(), 1);
            assert!(prune.ColPrunes[0].HasDefault());
            assert_eq!(prune.ColPrunes[0].ValueMap.len(), 2);
        }
    }
    let f = Fixture::new();
    let mut t = table(&f);
    partition(&mut t, PartitionTypeRange);
    t.Partition.as_mut().unwrap().Definitions[0].LessThan = vec!["10+1".into()];
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Done);
    let mut actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
    let loaded =
        astersql_table_tables::tables::table_from_meta_for_validation(&mut actual).unwrap();
    let prune = loaded
        .partition_expression
        .as_ref()
        .unwrap()
        .ForRangePruning
        .as_ref()
        .unwrap();
    assert_eq!(prune.LessThan, [11, 0]);
    assert!(prune.MaxValue);
}
#[test]
fn normal_ddl_plan_create_table_reverse_foreign_key_and_constraint_names() {
    for duplicate in [false, true] {
        let f = Fixture::new();
        let mut old = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let mut t = table(&f);
        if duplicate {
            let constraint = astersql_meta_model::ConstraintInfo {
                ID: 1,
                Name: astersql_meta_model::ast::NewCIStr("same_constraint"),
                State: SchemaState::Public,
                ConstraintCols: vec![astersql_meta_model::ast::NewCIStr("id")],
                ExprString: "id>0".into(),
                Enforced: true,
                ..Default::default()
            };
            old.Constraints = vec![constraint.clone()];
            t.Constraints = vec![constraint];
        } else {
            fk(&mut old, "normal_ddl_created");
            old.ForeignKeys[0].State = SchemaState::Public;
            old.ForeignKeys[0].RefCols[0] = astersql_meta_model::ast::NewCIStr("absent");
        }
        seed(&f, &old);
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let h = durable(&f, j.id);
        assert_eq!(h.state, JobState::Cancelled);
        assert!(
            h.error
                .unwrap()
                .contains(if duplicate { "3822" } else { "3734" })
        );
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
    }
}

#[test]
fn normal_ddl_plan_create_table_invalid_fk_state_retries() {
    let f = Fixture::new();
    let mut t = table(&f);
    fk(&mut t, "normal_ddl_target");
    t.State = SchemaState::WriteReorganization;
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    let queued = f.queue(j.id).unwrap();
    assert_eq!(queued.state, JobState::Running);
    assert_eq!(queued.error_count, 1);
    assert!(queued.error.unwrap().contains("8204"));
    assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
}

#[test]
fn normal_ddl_plan_create_table_extra_row_and_random_bases() {
    for random in [false, true] {
        let f = Fixture::new();
        let mut t = table(&f);
        if random {
            t.Columns[0]
                .FieldType
                .SetType(astersql_meta_model::mysql::TypeLonglong);
            t.AutoRandomBits = 5;
            t.AutoRandID = 301;
        } else {
            t.PKIsHandle = false;
            t.AutoIncIDExtra = 201;
        }
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Done);
        assert!(
            counter(&f, t.ID, if random { "TARID" } else { "TID" })
                >= if random { 300 } else { 200 }
        );
        run(&f);
        assert_eq!(durable(&f, j.id).state, JobState::Synced);
    }
}
#[test]
fn normal_ddl_create_table_columnar_gate() {
    struct Restore(astersql_config::Config);
    impl Drop for Restore {
        fn drop(&mut self) {
            astersql_config::store_global_config(self.0.clone());
        }
    }
    let _restore = Restore(astersql_config::get_global_config().as_ref().clone());
    for (value, count, done) in [
        ("OFF", 1, false),
        ("unexpected", 1, false),
        ("ON", 1, true),
        ("1", 1, true),
        ("OFF", 0, true),
    ] {
        let f = Fixture::new();
        astersql_config::update_global(|c| c.cse.columnar_store_type = "columnar".into());
        f.domain
            .set_global_system_variable("tidb_columnar_storage_enabled", value);
        let mut t = table(&f);
        t.TiFlashReplica = Some(astersql_meta_model::TiFlashReplicaInfo {
            Count: count,
            ..Default::default()
        });
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let result = durable(&f, j.id);
        assert_eq!(
            result.state,
            if done {
                JobState::Done
            } else {
                JobState::Cancelled
            },
            "{value}: {:?}",
            result.error
        );
        assert_eq!(f.reader().get_table(f.db, t.ID).unwrap().is_some(), done);
    }
}

#[test]
fn normal_ddl_storage_class_worker_persists_original_attribute_v1_and_v2() {
    for version in [JobVersion::V1, JobVersion::V2] {
        let f = Fixture::new();
        f.pool
            .acquire()
            .unwrap()
            .query("INSERT INTO test.normal_ddl_target VALUES(9,'storage class row')")
            .unwrap();
        let original = r#"{ "storage_class" : "IA" }"#;
        let args = if version == JobVersion::V1 {
            serde_json::json!([original])
        } else {
            serde_json::json!({"engine_attribute":original})
        };
        let mut job = Job {
            id: 990001,
            tp: 74,
            version,
            schema_id: f.db,
            table_id: f.table,
            schema_name: "test".into(),
            table_name: "normal_ddl_target".into(),
            raw_args: serde_json::to_vec(&args).unwrap(),
            ..Default::default()
        };
        let wire = astersql_meta::encode_go_ddl_job(&mut job, false).unwrap();
        f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',74,0)",job.id,f.db,f.table,hex(&wire))).unwrap();
        run(&f);
        let done = durable(&f, job.id);
        assert_eq!(done.state, JobState::Done);
        let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        assert_eq!(table.EngineAttribute, original);
        assert_eq!(table.StorageClassTier, "IA");
        assert!(done.binlog_info.as_ref().unwrap().schema_version > 0);
        if astersql_config_kerneltype::IsNextGen() {
            let history = f.pool.acquire().unwrap().query(format!(
                "SELECT table_schema,table_name,table_id,direction,state,schema_version,start_ts,physical_targets FROM mysql.tidb_storage_class_transition_history WHERE table_id={}",
                f.table
            )).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(
                &history[0][..5],
                &[
                    "test",
                    "normal_ddl_target",
                    &f.table.to_string(),
                    "TO_IA",
                    "RUNNING"
                ]
            );
            assert_eq!(
                history[0][5],
                done.binlog_info
                    .as_ref()
                    .unwrap()
                    .schema_version
                    .to_string()
            );
            assert_ne!(history[0][6], "0");
            let targets: Vec<astersql_ddl::storage_class_transition::StorageClassTransitionTarget> =
                serde_json::from_str(&history[0][7]).unwrap();
            assert_eq!(
                targets
                    .iter()
                    .map(|target| target.physical_id)
                    .collect::<Vec<_>>(),
                vec![f.table]
            );
        }
        let rows = f
            .pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=9")
            .unwrap();
        assert!(!rows.is_empty());
    }
}
#[test]
fn normal_ddl_storage_class_worker_cancels_invalid_settings_without_metadata_change() {
    let f = Fixture::new();
    let mut job = Job {
        id: 990002,
        tp: 74,
        version: JobVersion::V2,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: "normal_ddl_target".into(),
        raw_args: serde_json::to_vec(
            &serde_json::json!({"engine_attribute":r#"{"storage_class":"COLD"}"#}),
        )
        .unwrap(),
        ..Default::default()
    };
    let wire = astersql_meta::encode_go_ddl_job(&mut job, false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',74,0)",job.id,f.db,f.table,hex(&wire))).unwrap();
    run(&f);
    assert_eq!(durable(&f, job.id).state, JobState::Cancelled);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .EngineAttribute,
        ""
    );
}

#[test]
fn normal_ddl_storage_class_sql_show_information_schema_and_partition_updates() {
    struct RestoreDistTask(bool);
    impl Drop for RestoreDistTask {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::EnableDistTask.Store(self.0);
        }
    }
    let _restore_dist_task = RestoreDistTask(astersql_sessionctx_vardef::EnableDistTask.Load());
    if astersql_config_kerneltype::IsNextGen() {
        astersql_sessionctx_vardef::EnableDistTask.Store(false);
    }
    let f = Fixture::new();
    let mut session = f.pool.acquire().unwrap();
    session
        .query("CREATE TABLE test.storage_class_sql(id int) STORAGE_CLASS='ia'")
        .unwrap();
    let show = session
        .query("SHOW CREATE TABLE test.storage_class_sql")
        .unwrap();
    assert!(show[0][1].contains("ENGINE=InnoDB STORAGE_CLASS='IA' DEFAULT CHARSET="));
    let rows=session.query("SELECT TIDB_STORAGE_CLASS FROM information_schema.tables WHERE table_schema='test' AND table_name='storage_class_sql'").unwrap();
    assert_eq!(rows, vec![vec!["IA"]]);
    use astersql_domain::domain::{DdlService, StartMode};
    let cancellation = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancellation.clone(),
        "storage-class-worker",
        None,
        format!("/storage-class-worker/{}", f.db),
    );
    let service = Arc::new(super::normal_ddl_service::NormalDdlService::new(
        owner,
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        cancellation,
        f.pool.clone(),
        Arc::new(super::normal_ddl_service::DomainSchemaLoader(
            Arc::downgrade(&f.domain),
        )),
        Arc::new(|| Ok(Box::new(executor()))),
        Arc::new(|_| Err("unused table mode".into())),
        Arc::new(|| {}),
        true,
    ));
    struct StopService(Arc<super::normal_ddl_service::NormalDdlService>);
    impl Drop for StopService {
        fn drop(&mut self) {
            let _ = self.0.stop();
        }
    }
    f.domain.set_ddl(service.clone());
    service.start(StartMode::Normal).unwrap();
    let _stop = StopService(service);
    session
        .query("ALTER TABLE test.storage_class_sql STORAGE_CLASS='STANDARD'")
        .unwrap();
    if astersql_config_kerneltype::IsNextGen() {
        let show = session.query("SHOW STORAGE_CLASS TRANSITIONS").unwrap();
        assert_eq!(show.len(), 1);
        assert_eq!(
            &show[0][..6],
            &[
                "test",
                "storage_class_sql",
                &show[0][2],
                "",
                "",
                "TO_STANDARD"
            ]
        );
        let info = session.query("SELECT table_schema,table_name,direction FROM information_schema.tikv_storage_class_transitions WHERE table_name='storage_class_sql'").unwrap();
        assert_eq!(info, vec![vec!["test", "storage_class_sql", "TO_STANDARD"]]);
    }
    assert!(
        session
            .query("SHOW CREATE TABLE test.storage_class_sql")
            .unwrap()[0][1]
            .contains("STORAGE_CLASS='STANDARD'")
    );
    assert!(session.query(r#"ALTER TABLE test.storage_class_sql ENGINE_ATTRIBUTE='{"storage_class":"IA"}', STORAGE_CLASS='STANDARD'"#).is_err());
    session
        .query("ALTER TABLE test.storage_class_sql STORAGE_CLASS='IA', STORAGE_CLASS='STANDARD'")
        .unwrap();
    assert!(
        session
            .query("SHOW CREATE TABLE test.storage_class_sql")
            .unwrap()[0][1]
            .contains("STORAGE_CLASS='STANDARD'")
    );
    assert!(
        session
            .query("ALTER TABLE test.storage_class_sql STORAGE_CLASS='IA', STORAGE_CLASS='COLD'")
            .is_err()
    );
    assert!(
        session
            .query("SHOW CREATE TABLE test.storage_class_sql")
            .unwrap()[0][1]
            .contains("STORAGE_CLASS='STANDARD'")
    );
    session.query(r#"CREATE TABLE test.storage_class_parts(id int) ENGINE_ATTRIBUTE='{"storage_class":{"tier":"IA","less_than":"300"}}' PARTITION BY RANGE(id) (PARTITION p0 VALUES LESS THAN(100),PARTITION p1 VALUES LESS THAN(200))"#).unwrap();
    session.query("ALTER TABLE test.storage_class_parts ADD PARTITION(PARTITION p2 VALUES LESS THAN(100+200))").unwrap();
    let rows=session.query("SELECT partition_name,TIDB_STORAGE_CLASS,partition_description FROM information_schema.partitions WHERE table_schema='test' AND table_name='storage_class_parts' ORDER BY partition_name").unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["p0", "IA", "100"],
            vec!["p1", "IA", "200"],
            vec!["p2", "IA", "300"]
        ]
    );
    session.query("ALTER TABLE test.storage_class_parts REORGANIZE PARTITION p2 INTO(PARTITION p2 VALUES LESS THAN(200+50),PARTITION p3 VALUES LESS THAN(300))").unwrap();
    let rows=session.query("SELECT partition_name,TIDB_STORAGE_CLASS,partition_description FROM information_schema.partitions WHERE table_schema='test' AND table_name='storage_class_parts' ORDER BY partition_name").unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["p0", "IA", "100"],
            vec!["p1", "IA", "200"],
            vec!["p2", "IA", "250"],
            vec!["p3", "IA", "300"]
        ]
    );
    session
        .query("ALTER TABLE test.storage_class_parts REMOVE PARTITIONING")
        .unwrap();
}

#[test]
fn normal_ddl_storage_class_worker_keeps_multi_schema_non_revertible_boundary() {
    let f = Fixture::new();
    let mut job = Job {
        id: 990003,
        tp: 74,
        version: JobVersion::V2,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: "normal_ddl_target".into(),
        raw_args: serde_json::to_vec(
            &serde_json::json!({"engine_attribute":r#"{"storage_class":"IA"}"#}),
        )
        .unwrap(),
        multi_schema_info: Some(astersql_meta_model::group_3::MultiSchemaInfo {
            revertible: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let wire = astersql_meta::encode_go_ddl_job(&mut job, false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',74,0)",job.id,f.db,f.table,hex(&wire))).unwrap();
    run(&f);
    assert!(
        !durable(&f, job.id)
            .multi_schema_info
            .as_ref()
            .unwrap()
            .revertible
    );
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .EngineAttribute,
        ""
    );
    run(&f);
    assert_eq!(durable(&f, job.id).state, JobState::Done);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .StorageClassTier,
        "IA"
    );
}

fn insert_ttl_action(f: &Fixture, action: u8, args: serde_json::Value) -> Job {
    let mut j = Job {
        id: 99812,
        tp: action,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: "normal_ddl_target".into(),
        state: JobState::Queueing,
        version: JobVersion::V2,
        ..Default::default()
    };
    j.raw_args = serde_json::to_vec(&args).unwrap();
    let encoded = hex(&j.encode(false).unwrap());
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',{},0)",j.id,f.db,f.table,encoded,action)).unwrap();
    j
}

#[test]
fn normal_ddl_batch_ttl_registration_compensates_in_reverse() {
    for fail in [false, true] {
        let f = Fixture::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                calls: calls.clone(),
                fail_at: fail.then_some(3),
                fail_delete: fail,
                ..Default::default()
            })));
        let mut tables = Vec::new();
        for (i, enabled) in [true, false, true, true].into_iter().enumerate() {
            let mut t = table(&f);
            t.ID += i as i64;
            t.Name = astersql_meta_model::ast::NewCIStr(format!("batch_{i}"));
            t.TTLInfo = Some(astersql_meta_model::TTLInfo {
                Enable: enabled,
                ..Default::default()
            });
            tables.push(t);
        }
        let args = serde_json::json!({"tables":tables.iter().map(|t|serde_json::json!({"table_info":t,"fk_check":true})).collect::<Vec<_>>()});
        let j = insert_ttl_action(&f, 60, args);
        run(&f);
        assert_eq!(
            durable(&f, j.id).state,
            if fail {
                JobState::Cancelled
            } else {
                JobState::Done
            }
        );
        for t in &tables {
            assert_eq!(f.reader().get_table(f.db, t.ID).unwrap().is_some(), !fail);
        }
        let actual = calls
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let mut expected = vec![tables[0].ID, tables[2].ID, tables[3].ID];
        if fail {
            expected.extend([-tables[2].ID, -tables[0].ID]);
        }
        assert_eq!(actual, expected);
        if !fail {
            assert_eq!(
                durable(&f, j.id)
                    .binlog_info
                    .unwrap()
                    .multiple_table_infos
                    .len(),
                4
            );
        }
    }
}

#[test]
fn normal_ddl_alter_ttl_syncs_and_rolls_back_controller_failure() {
    for (action, enabled, fail) in [
        (65, true, false),
        (65, false, false),
        (65, true, true),
        (67, false, false),
        (67, false, true),
    ] {
        let f = Fixture::new();
        let mut t = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        t.TTLInfo = Some(astersql_meta_model::TTLInfo {
            Enable: true,
            JobInterval: "24h".into(),
            ..Default::default()
        });
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        astersql_meta::TransactionMutator::new(txn.as_mut())
            .update_table(f.db, &mut t)
            .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                calls: calls.clone(),
                fail,
                fail_delete: fail,
                ..Default::default()
            })));
        let j = insert_ttl_action(&f, action, serde_json::json!({"ttl_enable":enabled}));
        run(&f);
        assert_eq!(
            durable(&f, j.id).state,
            if fail {
                JobState::Cancelled
            } else {
                JobState::Done
            }
        );
        let after = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        if fail {
            assert_eq!(
                serde_json::to_value(after.TTLInfo).unwrap(),
                serde_json::to_value(&t.TTLInfo).unwrap()
            );
        } else if action == 67 {
            assert!(after.TTLInfo.is_none());
        } else {
            assert_eq!(after.TTLInfo.unwrap().Enable, enabled);
        }
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(
            calls.lock().unwrap()[0].0,
            if enabled && action == 65 { t.ID } else { -t.ID }
        );
    }
}

#[test]
fn external_ttl_completed_batch_recycles_original_max_create_time_only_from_worker() {
    use super::{
        ConcreteSession, ttl_metadata::collect_physical_ttl_tables, ttl_runtime::run_ttl_tick,
        ttl_worker_session::TtlWorkerSqlSession,
    };
    use astersql_ttl_ttlworker::{persistent::PersistentJobStore, session::WorkerSession};
    for (role, fail_recycle) in [("ttl", false), ("ttl", true), ("master", false)] {
        let f = Fixture::new();
        let setup = f.pool.acquire().unwrap();
        for name in ["recycle_a", "recycle_b"] {
            setup.query(format!("CREATE TABLE test.{name} (id INT PRIMARY KEY,created_at DATETIME) TTL = created_at + INTERVAL 1 DAY")).unwrap();
            setup.query(format!("INSERT INTO test.{name} VALUES (1,'1970-01-01 00:00:01'),(2,'1970-01-04 00:00:00')")).unwrap();
        }
        let tables = collect_physical_ttl_tables(f.domain.info_schema().as_ref(), 200_000).unwrap();
        let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(f.domain.clone()));
        for (i, table) in tables.iter().enumerate() {
            assert!(
                PersistentJobStore::start_job(
                    &mut sql,
                    table,
                    "dead-owner",
                    &format!("recycle-job-{i}"),
                    200_000 + i as u64 * 123,
                    None
                )
                .unwrap()
            );
        }
        let recycled = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                role: role.into(),
                recycled: recycled.clone(),
                fail_recycle,
                ..Default::default()
            })));
        let result = run_ttl_tick(&f.domain, "new-owner", 201_000, || false).unwrap();
        assert_eq!(result.resumed, 2);
        assert_eq!(result.finished, 2);
        assert_eq!(
            *recycled.lock().unwrap(),
            if role == "ttl" { vec![200_123] } else { vec![] }
        );
        run_ttl_tick(&f.domain, "new-owner", 201_001, || false).unwrap();
        assert_eq!(recycled.lock().unwrap().len(), usize::from(role == "ttl"));
        assert_eq!(
            sql.execute("SELECT id FROM test.recycle_a ORDER BY id", &[])
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn sql_alter_ttl_syncs_controller_and_preserves_unspecified_options() {
    let f = Fixture::new();
    let sql = f.pool.acquire().unwrap();
    sql.query("CREATE TABLE test.sql_ttl (id INT PRIMARY KEY,created_at DATETIME) TTL = created_at + INTERVAL 1 DAY TTL_JOB_INTERVAL='24h'").unwrap();
    let id = f.domain.table_by_name("test", "sql_ttl").unwrap().ID;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    f.domain
        .set_external_workload_manager(Some(Box::new(TtlBoundary {
            calls: calls.clone(),
            ..Default::default()
        })));
    sql.query("ALTER TABLE test.sql_ttl TTL_ENABLE='OFF'")
        .unwrap();
    sql.query("ALTER TABLE test.sql_ttl TTL = created_at + INTERVAL 2 DAY")
        .unwrap();
    let ttl = f
        .domain
        .table_by_name("test", "sql_ttl")
        .unwrap()
        .TTLInfo
        .clone()
        .unwrap();
    assert!(!ttl.Enable);
    assert_eq!(ttl.JobInterval, "24h");
    assert_eq!(ttl.IntervalExprStr, "2");
    sql.query("ALTER TABLE test.sql_ttl TTL_ENABLE='ON'")
        .unwrap();
    sql.query("ALTER TABLE test.sql_ttl REMOVE TTL").unwrap();
    assert!(
        f.domain
            .table_by_name("test", "sql_ttl")
            .unwrap()
            .TTLInfo
            .is_none()
    );
    assert!(
        sql.query("ALTER TABLE test.sql_ttl TTL_ENABLE='ON'")
            .is_err()
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![-id, -id, id, -id]
    );
}

struct TtlElectionTransport {
    election: Arc<dyn astersql_owner::Manager>,
    streams: std::sync::Mutex<Vec<std::sync::mpsc::Sender<Vec<u8>>>>,
}
impl super::ttl_runtime::TtlWatchTransport for TtlElectionTransport {
    fn ttl_owner(&self, _: &str) -> Option<Arc<dyn astersql_owner::Manager>> {
        Some(self.election.clone())
    }
    fn watch(
        &self,
        _: super::ttl_runtime::TtlWatchKind,
        _: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<std::sync::mpsc::Receiver<Vec<u8>>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.streams.lock().unwrap().push(tx);
        Ok(rx)
    }
    fn take_command(&self, _: &str) -> Result<bool, String> {
        Ok(true)
    }
    fn response_command(
        &self,
        _: &str,
        _: Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn external_ttl_worker_campaigns_separate_owner_and_closes_it_with_domain() {
    let f = Fixture::new();
    f.domain
        .set_external_workload_manager(Some(Box::new(TtlBoundary {
            role: "ttl".into(),
            ..Default::default()
        })));
    let key = format!("/ttl-election-test/{:p}", Arc::as_ptr(&f.domain));
    let election =
        astersql_owner::NewMockManager(astersql_owner::Context::new(), "ttl-owner", None, &key);
    let transport = Arc::new(TtlElectionTransport {
        election: election.clone(),
        streams: Default::default(),
    });
    assert!(
        super::ttl_runtime::start_domain_ttl_job_manager_with_interval(
            &f.domain,
            Some(transport),
            std::time::Duration::from_millis(10)
        )
        .unwrap()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !election.IsOwner() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        election.IsOwner(),
        "dedicated TTL manager must campaign its own owner"
    );
    f.domain.close();
    assert!(
        !election.IsOwner(),
        "closing Domain must release TTL election"
    );
}

#[test]
fn external_ttl_incomplete_job_prevents_recycling_other_finished_job() {
    use super::{
        ConcreteSession, ttl_metadata::collect_physical_ttl_tables, ttl_runtime::run_ttl_event,
        ttl_worker_session::TtlWorkerSqlSession,
    };
    use astersql_ttl_ttlworker::persistent::PersistentJobStore;
    let f = Fixture::new();
    let setup = f.pool.acquire().unwrap();
    for name in ["incomplete_a", "incomplete_b"] {
        setup.query(format!("CREATE TABLE test.{name} (id INT PRIMARY KEY,created_at DATETIME) TTL = created_at + INTERVAL 1 DAY")).unwrap();
        setup
            .query(format!(
                "INSERT INTO test.{name} VALUES (1,'1970-01-01 00:00:01')"
            ))
            .unwrap();
    }
    let tables = collect_physical_ttl_tables(f.domain.info_schema().as_ref(), 200_000).unwrap();
    let mut sql = TtlWorkerSqlSession::new(ConcreteSession::new(f.domain.clone()));
    for (i, t) in tables.iter().enumerate() {
        assert!(
            PersistentJobStore::start_job(
                &mut sql,
                t,
                "dead-owner",
                &format!("incomplete-job-{i}"),
                200_000 + i as u64,
                None
            )
            .unwrap()
        );
    }
    let recycled = Arc::new(std::sync::Mutex::new(Vec::new()));
    f.domain
        .set_external_workload_manager(Some(Box::new(TtlBoundary {
            role: "ttl".into(),
            recycled: recycled.clone(),
            ..Default::default()
        })));
    let checks = std::cell::Cell::new(0);
    let a = &tables[0];
    let b = &tables[1];
    assert!(
        run_ttl_event(
            &f.domain,
            "new-owner",
            201_000,
            a.table_id,
            a.physical_id,
            "incomplete-job-0",
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 3
            }
        )
        .is_err()
    );
    assert_eq!(
        run_ttl_event(
            &f.domain,
            "new-owner",
            201_001,
            b.table_id,
            b.physical_id,
            "incomplete-job-1",
            || false
        )
        .unwrap()
        .finished,
        1
    );
    assert!(recycled.lock().unwrap().is_empty());
    assert_eq!(
        run_ttl_event(
            &f.domain,
            "next-owner",
            201_300,
            a.table_id,
            a.physical_id,
            "incomplete-job-0",
            || false
        )
        .unwrap()
        .finished,
        1
    );
    assert_eq!(*recycled.lock().unwrap(), vec![200_000]);
}

fn seed_ttl_on_target(f: &Fixture, enabled: bool) -> TableInfo {
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.TTLInfo = Some(astersql_meta_model::TTLInfo {
        Enable: enabled,
        ..Default::default()
    });
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    astersql_meta::TransactionMutator::new(txn.as_mut())
        .update_table(f.db, &mut table)
        .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    table
}

#[test]
fn normal_ddl_drop_ttl_deletes_before_transition_and_cancels_controller_error() {
    for fail in [false, true] {
        let f = Fixture::new();
        let table = seed_ttl_on_target(&f, true);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                calls: calls.clone(),
                fail_delete: fail,
                ..Default::default()
            })));
        let j = insert_ttl_action(&f, 4, serde_json::json!({"fk_check":true,"identifiers":[]}));
        run(&f);
        if fail {
            assert_eq!(durable(&f, j.id).state, JobState::Cancelled);
            assert_eq!(
                f.reader().get_table(f.db, table.ID).unwrap().unwrap().State,
                SchemaState::Public
            );
        } else {
            run(&f);
            run(&f);
            assert!(f.reader().get_table(f.db, table.ID).unwrap().is_none());
        }
        assert_eq!(*calls.lock().unwrap(), vec![(-table.ID, false)]);
    }
}

#[test]
fn normal_ddl_truncate_ttl_compensates_and_preserves_original_error() {
    struct RestoreInfoSyncer(Option<Arc<astersql_domain_infosync::InfoSyncer>>);
    impl Drop for RestoreInfoSyncer {
        fn drop(&mut self) {
            if let Some(original) = self.0.take() {
                astersql_domain_infosync::setGlobalInfoSyncer(original);
            }
        }
    }
    let _restore = RestoreInfoSyncer(astersql_domain_infosync::getGlobalInfoSyncer().ok());
    let _labels = astersql_domain_infosync::GlobalInfoSyncerInit(
        "task121-truncate".into(),
        Arc::new(|| 1),
        None,
        None,
        None,
        astersql_domain_infosync::Codec::default(),
        false,
        None,
    )
    .unwrap();
    for (enabled, fail_delete, fail_register, fail_restore) in [
        (true, false, false, false),
        (false, false, false, false),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, true, true),
    ] {
        let f = Fixture::new();
        let old = seed_ttl_on_target(&f, enabled);
        let new_id = old.ID + 9000;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                calls: calls.clone(),
                fail_delete,
                fail_at: fail_register.then_some(2),
                fail: fail_restore,
                ..Default::default()
            })));
        let log = astersql_util_logutil::log::background_logger();
        let j = insert_ttl_action(
            &f,
            11,
            serde_json::json!({"new_table_id":new_id,"fk_check":true}),
        );
        run(&f);
        if fail_delete || fail_register {
            assert_eq!(durable(&f, j.id).state, JobState::Cancelled);
            assert!(
                durable(&f, j.id)
                    .error
                    .unwrap()
                    .contains("controller unavailable")
            );
            assert!(f.reader().get_table(f.db, old.ID).unwrap().is_some());
            assert!(f.reader().get_table(f.db, new_id).unwrap().is_none());
        } else {
            assert_eq!(
                durable(&f, j.id).state,
                JobState::Done,
                "{:?}",
                durable(&f, j.id).error
            );
            assert!(f.reader().get_table(f.db, old.ID).unwrap().is_none());
            assert!(f.reader().get_table(f.db, new_id).unwrap().is_some());
        }
        let mut expected = vec![-old.ID];
        if enabled && !fail_delete {
            expected.push(new_id);
            if fail_register {
                expected.push(old.ID);
            }
        }
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            expected
        );
        if fail_restore {
            assert!(log.entries().iter().any(|e| {
                e.message
                    .contains("truncate_ttl_restore_old_registration_failed")
                    && e.message.contains(&format!("oldTableID={}", old.ID))
                    && e.message.contains(&format!("newTableID={new_id}"))
            }));
        }
    }
}

#[test]
fn external_ttl_two_managers_elect_one_owner_and_release_on_close() {
    let first = Fixture::new();
    let second = Fixture::new();
    let key = format!("/ttl-two-election-test/{:p}", Arc::as_ptr(&first.domain));
    let a = astersql_owner::NewMockManager(astersql_owner::Context::new(), "ttl-a", None, &key);
    let b = astersql_owner::NewMockManager(astersql_owner::Context::new(), "ttl-b", None, &key);
    for (f, election) in [(&first, a.clone()), (&second, b.clone())] {
        f.domain
            .set_external_workload_manager(Some(Box::new(TtlBoundary {
                role: "ttl".into(),
                ..Default::default()
            })));
        let transport = Arc::new(TtlElectionTransport {
            election,
            streams: Default::default(),
        });
        assert!(
            super::ttl_runtime::start_domain_ttl_job_manager_with_interval(
                &f.domain,
                Some(transport),
                std::time::Duration::from_millis(10)
            )
            .unwrap()
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !a.IsOwner() && !b.IsOwner() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_ne!(a.IsOwner(), b.IsOwner());
    let (leader, follower, successor) = if a.IsOwner() {
        (&first, &second, b.clone())
    } else {
        (&second, &first, a.clone())
    };
    leader.domain.close();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !successor.IsOwner() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(successor.IsOwner());
    follower.domain.close();
    assert!(!successor.IsOwner());
}
