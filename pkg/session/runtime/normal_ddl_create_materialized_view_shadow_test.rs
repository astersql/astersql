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
    t.Name = astersql_meta_model::ast::NewCIStr("__mv_shadow_created");
    t.State = SchemaState::None;
    t.MaterializedView = None;
    t.MaterializedViewShadow = Some(astersql_meta_model::MaterializedViewShadowInfo {
        SourceMViewID: f.table,
    });
    t
}
fn insert(f: &Fixture, t: &TableInfo, v: JobVersion) -> Job {
    insert_check(f, t, v, true)
}
fn insert_check(f: &Fixture, t: &TableInfo, v: JobVersion, check: bool) -> Job {
    let mut j = Job {
        id: 99812,
        tp: 93,
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
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',93,0)",j.id,f.db,t.ID,hex(&wire))).unwrap();
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
    seed_source(&f);
    let t = table(&f);
    let j = insert(&f, &t, v);
    run(&f);
    let result = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
    assert_eq!(result.State, SchemaState::Public);
    assert_eq!(
        result
            .MaterializedViewShadow
            .as_ref()
            .unwrap()
            .SourceMViewID,
        f.table
    );
    assert_eq!(
        serde_json::to_value(&result.Columns).unwrap(),
        serde_json::to_value(&t.Columns).unwrap()
    );
    assert!(result.UpdateTS > 0);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    let mut queued = f.queue(j.id).unwrap();
    let args = astersql_meta_model::group_2::GetCreateTableArgs(&mut queued).unwrap();
    let persisted_args: TableInfo =
        serde_json::from_value(serde_json::to_value(args.TableInfo.unwrap()).unwrap()).unwrap();
    assert_eq!(persisted_args.State, SchemaState::Public);
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    assert_diff(&f, &f.queue(j.id).unwrap(), 0);
    assert_no_notification(&f);
    run(&f);
    assert!(f.queue(j.id).is_none());
    let mut h = f.reader().get_history_ddl_job(j.id).unwrap().unwrap();
    assert_eq!(h.state, JobState::Synced);
    assert_eq!(h.cdc_write_source, 41);
    let args = astersql_meta_model::group_2::GetCreateTableArgs(&mut h).unwrap();
    let history_args: TableInfo =
        serde_json::from_value(serde_json::to_value(args.TableInfo.unwrap()).unwrap()).unwrap();
    assert_eq!(history_args.State, SchemaState::Public);
    assert_eq!(
        history_args.MaterializedViewShadow.unwrap().SourceMViewID,
        f.table
    );
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
fn normal_ddl_plan_create_materialized_view_shadow_v1() {
    create(JobVersion::V1)
}
#[test]
fn normal_ddl_plan_create_materialized_view_shadow_v2() {
    create(JobVersion::V2)
}

fn seed_source(f: &Fixture) {
    let mut source = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    source.MaterializedView = Some(astersql_meta_model::MaterializedViewInfo {
        BaseTableIDs: vec![f.table + 100],
        SQLContent: "select id, payload from source".into(),
        RefreshMethod: "COMPLETE".into(),
        ..Default::default()
    });
    seed(f, &source);
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
fn assert_no_notification(f: &Fixture) {
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99812")
            .unwrap()
            .is_empty()
    );
}
fn assert_diff(f: &Fixture, j: &Job, old: i64) {
    assert!(j.last_schema_version > 0);
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
    let diff: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(diff["type"], 93);
    assert_eq!(diff["schema_id"], f.db);
    assert_eq!(diff["table_id"], j.table_id);
    assert_eq!(diff["old_table_id"], old);
}
#[test]
fn normal_ddl_plan_create_materialized_view_shadow_rejects_invalid_metadata() {
    for v in [JobVersion::V1, JobVersion::V2] {
        for case in 0..8 {
            let f = Fixture::new();
            seed_source(&f);
            let mut t = table(&f);
            match case {
                0 => t.MaterializedViewShadow = None,
                1 => t.MaterializedViewShadow.as_mut().unwrap().SourceMViewID = 0,
                2 => t.MaterializedView = Some(Default::default()),
                3 => t.MaterializedViewLog = Some(Default::default()),
                4 => t.View = Some(Default::default()),
                5 => t.Sequence = Some(Default::default()),
                _ => (),
            }
            let version = f.reader().get_schema_version_with_non_empty_diff().unwrap();
            let mut j = insert(&f, &t, v);
            if case >= 6 {
                j.raw_args = serde_json::to_vec(&if case == 6 {
                    if v == JobVersion::V1 {
                        serde_json::json!([null, true])
                    } else {
                        serde_json::json!({"table_info":null,"fk_check":true})
                    }
                } else {
                    serde_json::json!("wrong create arguments")
                })
                .unwrap();
                let wire = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
                f.pool
                    .acquire()
                    .unwrap()
                    .query(format!(
                        "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
                        hex(&wire),
                        j.id
                    ))
                    .unwrap();
            }
            run(&f);
            let h = durable(&f, j.id);
            assert_eq!(h.state, JobState::Cancelled, "case {case}: {:?}", h.error);
            assert!(h.error.is_some());
            if case <= 6 {
                assert!(
                    h.error.as_ref().unwrap().contains("8204"),
                    "case {case}: {:?}",
                    h.error
                );
            }
            assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
            assert_eq!(
                f.reader().get_schema_version_with_non_empty_diff().unwrap(),
                version
            );
            assert_no_notification(&f);
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_shadow_validates_source() {
    for case in 0..5 {
        let f = Fixture::new();
        seed_source(&f);
        let mut source = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let mut t = table(&f);
        let code = match case {
            0 => {
                t.MaterializedViewShadow.as_mut().unwrap().SourceMViewID += 100;
                "1146"
            }
            1 => {
                source.MaterializedView = None;
                seed(&f, &source);
                "1347"
            }
            2 => {
                source.State = SchemaState::DeleteOnly;
                seed(&f, &source);
                "8210"
            }
            3 => {
                source.State = SchemaState::WriteOnly;
                seed(&f, &source);
                "8210"
            }
            _ => "1049",
        };
        let version = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        let mut j = insert(&f, &t, JobVersion::V2);
        if case == 4 {
            j.schema_id += 10000;
            let wire = astersql_meta::encode_go_ddl_job(&mut j, false).unwrap();
            f.pool
                .acquire()
                .unwrap()
                .query(format!(
                    "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
                    hex(&wire),
                    j.id
                ))
                .unwrap();
        }
        run(&f);
        let h = durable(&f, j.id);
        assert_eq!(h.state, JobState::Cancelled);
        assert!(
            h.error.as_ref().unwrap().contains(code),
            "case {case}: {:?}",
            h.error
        );
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
        assert_eq!(
            f.reader().get_schema_version_with_non_empty_diff().unwrap(),
            version
        );
        assert_no_notification(&f);
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_shadow_foreign_keys_single_public_step() {
    for v in [JobVersion::V1, JobVersion::V2] {
        for check in [true, false] {
            let f = Fixture::new();
            seed_source(&f);
            let mut t = table(&f);
            t.MaxForeignKeyID = 6;
            t.ForeignKeys = vec![astersql_meta_model::FKInfo {
                Name: astersql_meta_model::ast::NewCIStr("shadow_fk"),
                RefSchema: astersql_meta_model::ast::NewCIStr("test"),
                RefTable: astersql_meta_model::ast::NewCIStr(if check {
                    "normal_ddl_target"
                } else {
                    "absent_parent"
                }),
                Cols: vec![astersql_meta_model::ast::NewCIStr("id")],
                RefCols: vec![astersql_meta_model::ast::NewCIStr("id")],
                Version: 1,
                ..Default::default()
            }];
            let j = insert_check(&f, &t, v, check);
            run(&f);
            let current = durable(&f, j.id);
            assert_eq!(current.state, JobState::Done, "{:?}", current.error);
            let actual = f.reader().get_table(f.db, t.ID).unwrap().unwrap();
            assert_eq!(actual.State, SchemaState::Public);
            assert_eq!(actual.ForeignKeys[0].State, SchemaState::Public);
            assert_eq!(actual.ForeignKeys[0].ID, 7);
            assert_diff(&f, &current, t.ID);
            assert_no_notification(&f);
            run(&f);
            assert_eq!(durable(&f, j.id).state, JobState::Synced);
        }
    }
}
#[test]
fn normal_ddl_plan_create_materialized_view_shadow_commit_conflict_and_restart() {
    use astersql_ddl::job_worker::DurableJobExecutor;
    let mut f = Fixture::new();
    seed_source(&f);
    let mut t = table(&f);
    t.Version = 5;
    t.AutoIDCache = 100;
    t.AutoIncID = 111;
    t.Columns[0]
        .FieldType
        .AddFlag(astersql_meta_model::mysql::AutoIncrementFlag);
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
    assert!(counter(&f, t.ID, if t.SepAutoInc() { "IID" } else { "TID" }) >= 110);
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
    f.pool.close();
    f.pool = super::system_session::SystemSessionPool::new(f.domain.clone());
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Done);
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Synced);
}

struct TtlBoundary {
    calls: Arc<std::sync::Mutex<Vec<(i64, bool)>>>,
    fail: bool,
}
impl astersql_extworkload::Manager for TtlBoundary {
    fn Close(&mut self) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn Role(&self) -> String {
        astersql_config::RoleMaster.into()
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
        if self.fail {
            Err(std::io::Error::other("controller unavailable").into())
        } else {
            Ok(())
        }
    }
    fn DeleteTTLTableInfo(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: i64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
    }
    fn RecycleTTLTask(
        &mut self,
        _: &astersql_extworkload::context::Context,
        _: u64,
    ) -> Result<(), astersql_extworkload::ManagerError> {
        Ok(())
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
fn normal_ddl_plan_create_materialized_view_shadow_does_not_register_ttl() {
    let f = Fixture::new();
    seed_source(&f);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    f.domain
        .set_external_workload_manager(Some(Box::new(TtlBoundary {
            calls: calls.clone(),
            fail: true,
        })));
    let mut t = table(&f);
    t.TTLInfo = Some(astersql_meta_model::TTLInfo {
        ColumnName: astersql_meta_model::ast::NewCIStr("id"),
        IntervalExprStr: "1".into(),
        Enable: true,
        ..Default::default()
    });
    let j = insert(&f, &t, JobVersion::V2);
    run(&f);
    assert_eq!(durable(&f, j.id).state, JobState::Done);
    assert!(calls.lock().unwrap().is_empty());
    assert_no_notification(&f);
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
fn normal_ddl_plan_create_materialized_view_shadow_pd_placement_and_tiflash_requests() {
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
        seed_source(&f);
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
            assert_no_notification(&f);
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
fn normal_ddl_plan_create_materialized_view_shadow_affinity_requests_failure_and_range_encoding() {
    for level in ["table", "partition"] {
        for fail in [false, true] {
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            astersql_domain_affinity::init_manager(Some(Arc::new(AffinityBoundary {
                requests: requests.clone(),
                fail,
            })));
            let f = Fixture::new();
            seed_source(&f);
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
fn normal_ddl_plan_create_materialized_view_shadow_create_errors_atomic() {
    for case in 0..4 {
        let f = Fixture::new();
        seed_source(&f);
        let mut t = table(&f);
        let code = match case {
            0 => {
                t.Name = astersql_meta_model::ast::NewCIStr("normal_ddl_target");
                "1050"
            }
            1 => {
                t.Columns[0].State = SchemaState::None;
                "8046"
            }
            2 => {
                t.PlacementPolicyRef = Some(astersql_meta_model::PolicyRefInfo {
                    ID: 9999,
                    Name: astersql_meta_model::ast::NewCIStr("absent"),
                });
                "8249"
            }
            _ => {
                t.ForeignKeys = vec![astersql_meta_model::FKInfo {
                    Name: astersql_meta_model::ast::NewCIStr("absent_fk"),
                    RefSchema: astersql_meta_model::ast::NewCIStr("test"),
                    RefTable: astersql_meta_model::ast::NewCIStr("absent"),
                    Cols: vec![astersql_meta_model::ast::NewCIStr("id")],
                    RefCols: vec![astersql_meta_model::ast::NewCIStr("id")],
                    Version: 1,
                    ..Default::default()
                }];
                "1146"
            }
        };
        let source =
            serde_json::to_value(f.reader().get_table(f.db, f.table).unwrap().unwrap()).unwrap();
        let version = f.reader().get_schema_version_with_non_empty_diff().unwrap();
        let j = insert(&f, &t, JobVersion::V2);
        run(&f);
        let h = durable(&f, j.id);
        assert_eq!(h.state, JobState::Cancelled);
        assert!(h.error.as_ref().unwrap().contains(code), "{:?}", h.error);
        assert!(f.reader().get_table(f.db, t.ID).unwrap().is_none());
        assert_eq!(
            f.reader().get_schema_version_with_non_empty_diff().unwrap(),
            version
        );
        assert_eq!(
            serde_json::to_value(f.reader().get_table(f.db, f.table).unwrap().unwrap()).unwrap(),
            source
        );
        assert_no_notification(&f);
    }
}

fn schema_barrier_owner() -> (
    Arc<tokio::runtime::Runtime>,
    Arc<dyn astersql_owner::manager::Manager>,
    astersql_owner::manager::Context,
) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    let cancel = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancel.clone(),
        "schema-barrier-owner",
        None,
        format!("/ddl/schema-barrier/{}", std::process::id()),
    );
    runtime.block_on(owner.CampaignOwner(&[])).unwrap();
    (runtime, owner, cancel)
}

#[test]
fn normal_ddl_plan_create_materialized_view_shadow_public_barrier_restart() {
    use astersql_ddl_schemaver::{Context, MemoryEtcdClient, NewEtcdSyncer};
    let mut f = Fixture::new();
    seed_source(&f);
    let t = table(&f);
    let j = insert(&f, &t, JobVersion::V2);
    let _restore = MdlRestore(
        astersql_sessionctx_vardef::IsMDLEnabled(),
        astersql_ddl_schemaver::IsMDLEnabled(),
    );
    astersql_sessionctx_vardef::SetEnableMDL(false);
    astersql_ddl_schemaver::SetMDLEnabled(false);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'barrier existing row')")
        .unwrap();
    let client = Arc::new(MemoryEtcdClient::default());
    let first = NewEtcdSyncer(client.clone(), "barrier-first");
    let second = NewEtcdSyncer(client.clone(), "barrier-second");
    second.Init(Context::Background()).unwrap();
    let schema = super::session_factory::prepare_normal_schema_runtime(
        &f.domain,
        first.clone(),
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    let lease = Lease;
    let mut e = NormalDdlExecutor {
        barrier: schema.schema_barrier(
            &f.domain,
            owner.clone(),
            cancel.clone(),
            "schema-barrier-owner".into(),
            Some(client.clone()),
        ),
        policy: Policy,
        sequence: Arc::new(AtomicI64::new(0)),
    };
    let parent = e.barrier.context.clone();
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_millis(100));

    assert!(
        JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex)
        )
        .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
        .is_err(),
        "unsynchronized follower must hold the committed step"
    );
    let committed = f.queue(j.id).unwrap();
    assert_eq!(committed.state, JobState::Done);
    assert!(committed.last_schema_version > 0);
    assert_eq!(
        f.reader().get_table(f.db, t.ID).unwrap().unwrap().State,
        SchemaState::Public
    );
    assert_no_notification(&f);
    assert!(f.reader().get_history_ddl_job(j.id).unwrap().is_none());
    f.pool.close();
    f.pool = super::system_session::SystemSessionPool::new(f.domain.clone());
    // A fresh scheduler and executor must recover the committed version before
    // a terminal step can delete the queue or add history.
    owner_runtime.block_on(owner.Close());
    let (owner_runtime, owner, cancel) = schema_barrier_owner();
    e.barrier = schema.schema_barrier(
        &f.domain,
        owner.clone(),
        cancel,
        "schema-barrier-owner".into(),
        Some(client.clone()),
    );
    let parent = e.barrier.context.clone();
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_millis(100));
    assert!(
        JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex)
        )
        .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
        .is_err()
    );
    assert!(f.queue(j.id).is_some());
    e.barrier.context = parent.WithTimeout(std::time::Duration::from_secs(2));
    first
        .UpdateSelfVersion(Context::Background(), 0, committed.last_schema_version)
        .unwrap();
    second
        .UpdateSelfVersion(Context::Background(), 0, committed.last_schema_version)
        .unwrap();
    assert_eq!(
        JobScheduler::new(
            JobWorker::new(WorkerType::General),
            JobWorker::new(WorkerType::AddIndex)
        )
        .schedule_persisted(&mut f.pool.acquire().unwrap(), &lease, &mut e, 0)
        .unwrap(),
        1
    );
    assert!(f.queue(j.id).is_none());
    assert_eq!(
        f.reader().get_history_ddl_job(j.id).unwrap().unwrap().state,
        JobState::Synced
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=1")
            .unwrap()[0][0],
        "barrier existing row"
    );
    schema.close().unwrap();
    owner_runtime.block_on(owner.Close());
}

struct MdlRestore(bool, bool);
impl Drop for MdlRestore {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::SetEnableMDL(self.0);
        astersql_ddl_schemaver::SetMDLEnabled(self.1);
    }
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
fn normal_ddl_plan_create_materialized_view_shadow_extra_row_and_random_bases() {
    for random in [false, true] {
        let f = Fixture::new();
        seed_source(&f);
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
