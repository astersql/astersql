// Copyright 2026 AsterSQL.
use super::normal_ddl_fixture::{Fixture, hex};
use astersql_ddl::{
    job_scheduler::JobScheduler,
    job_worker::{DurableJobSession, JobLease, JobWorker, WorkerType},
    table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor},
};
use astersql_meta_model::{
    SchemaState,
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
#[test]
fn masking_policy_drop_table_persistent_state_transitions() {
    let f = Fixture::new();
    let j = insert_drop(&f);
    let column = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns[1]
        .ID;
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_masking_policy (policy_id,policy_name,db_name,table_name,table_id,column_name,column_id,expression,status,masking_type,restrict_on,created_at,updated_at,created_by) VALUES (9606,'p','test','normal_ddl_target',{},'payload',{},'MASK_FULL(payload)','ENABLED','MASK_FULL','NONE','2026-01-01 00:00:00','2026-01-01 00:00:00','root')",f.table,column)).unwrap();
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().schema_state, SchemaState::WriteOnly);
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().State,
        SchemaState::WriteOnly
    );
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().schema_state, SchemaState::DeleteOnly);
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    assert!(f.reader().get_table(f.db, f.table).unwrap().is_none());
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap()
            .is_empty()
    );
}

fn insert_drop(f: &Fixture) -> Job {
    let mut j = Job {
        id: 99606,
        tp: 4,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".to_string(),
        table_name: "normal_ddl_target".to_string(),
        state: JobState::Queueing,
        schema_state: SchemaState::Public,
        version: JobVersion::V2,
        raw_args: b"{}".to_vec(),
        ..Default::default()
    };
    let wire = j.encode(false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',4,0)", j.id,f.db,f.table,hex(&wire))).unwrap();
    j
}

#[test]
fn masking_policy_drop_table_missing_policy_table_rolls_back_metadata() {
    let f = Fixture::new();
    let j = insert_drop(&f);
    run(&f);
    run(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.masking_backup")
        .unwrap();
    run(&f);
    for _ in 0..4 {
        run(&f);
    }
    let failed = f.queue(j.id).unwrap();
    assert_eq!(failed.state, JobState::Running);
    assert_eq!(failed.schema_state, SchemaState::DeleteOnly);
    assert!(
        failed.error.as_ref().unwrap().contains("1146"),
        "{:?}",
        failed.error
    );
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().State,
        SchemaState::DeleteOnly
    );
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id=99606")
            .unwrap()
            .is_empty()
    );
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE mysql.masking_backup TO mysql.tidb_masking_policy")
        .unwrap();
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
}
#[test]
fn masking_policy_drop_table_malformed_policy_prevents_partial_cleanup() {
    let f = Fixture::new();
    let j = insert_drop(&f);
    let col = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns[1]
        .ID;
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_masking_policy (policy_id,policy_name,db_name,table_name,table_id,column_name,column_id,expression,status,masking_type,restrict_on,created_at,updated_at,created_by) VALUES (9606,'p','test','normal_ddl_target',{},'payload',{},'MASK_FULL(payload)','bad','MASK_FULL','NONE','2026-01-01 00:00:00','2026-01-01 00:00:00','root')",f.table,col)).unwrap();
    run(&f);
    run(&f);
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().schema_state, SchemaState::DeleteOnly);
    assert!(
        f.queue(j.id)
            .unwrap()
            .error
            .unwrap()
            .contains("unknown masking policy status: bad")
    );
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().State,
        SchemaState::DeleteOnly
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec!["9606".to_string()]]
    );
}

#[test]
fn masking_policy_truncate_persistent_updates_policy_id() {
    let _resources = LabelResources::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let original_columns = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns
        .iter()
        .map(|c| c.ID)
        .collect::<Vec<_>>();
    let new_id = f.table + 6000;
    let mut j = Job {
        id: 99607,
        tp: 11,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".to_string(),
        table_name: "normal_ddl_target".to_string(),
        state: JobState::Queueing,
        version: JobVersion::V2,
        raw_args: serde_json::to_vec(&serde_json::json!({"new_table_id":new_id,"fk_check":false}))
            .unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut j);
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    assert!(f.reader().get_table(f.db, f.table).unwrap().is_none());
    let new_table = f.reader().get_table(f.db, new_id).unwrap().unwrap();
    assert_eq!(new_table.ID, new_id);
    assert_eq!(
        new_table.Columns.iter().map(|c| c.ID).collect::<Vec<_>>(),
        original_columns
    );
    assert_eq!(f.pool.acquire().unwrap().query("SELECT table_id, column_name, created_at FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap(),vec![vec![new_id.to_string(),"payload".to_string(),"2026-01-01 00:00:00.000000".to_string()]]);
}
fn insert_job(f: &Fixture, j: &mut Job) {
    let wire = j.encode(false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',{},0)",j.id,j.schema_id,j.table_id,hex(&wire),j.tp)).unwrap();
}
#[test]
fn masking_policy_rename_persistent_two_phase_finish() {
    let _resources = LabelResources::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let mut j=Job {
        id:99608,tp:14,schema_id:f.db,table_id:f.table,
        schema_name:"test".to_string(),table_name:"normal_ddl_target".to_string(),
        state:JobState::Queueing,version:JobVersion::V2,
        raw_args:serde_json::to_vec(&serde_json::json!({"old_schema_id":f.db,"old_schema_name":{"O":"test","L":"test"},"new_table_name":{"O":"renamed_target","L":"renamed_target"}})).unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut j);
    run(&f);
    let current = f.queue(j.id).unwrap_or_else(|| {
        panic!(
            "rename history: {:?}",
            f.reader()
                .get_history_ddl_job(j.id)
                .unwrap()
                .map(|j| j.error)
        )
    });
    assert_eq!(current.schema_state, SchemaState::Public);
    assert_eq!(f.pool.acquire().unwrap().query("SELECT db_name,table_name,column_name FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap(),vec![vec!["test".to_string(),"renamed_target".to_string(),"payload".to_string()]]);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Running);
    assert_eq!(
        f.reader().get_table(f.db, f.table).unwrap().unwrap().Name.L,
        "renamed_target"
    );
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
}

struct LabelResources(Option<Arc<astersql_domain_infosync::InfoSyncer>>);
impl LabelResources {
    fn new() -> Self {
        let old = astersql_domain_infosync::getGlobalInfoSyncer().ok();
        astersql_domain_infosync::GlobalInfoSyncerInit(
            "go_commit_96dc7adec0".to_string(),
            Arc::new(|| 1),
            None,
            None,
            None,
            astersql_domain_infosync::Codec::default(),
            false,
            None,
        )
        .unwrap();
        Self(old)
    }
}
impl Drop for LabelResources {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            astersql_domain_infosync::setGlobalInfoSyncer(old);
        }
    }
}

#[test]
fn masking_policy_upgrade_189_254_279_recreates_missing_masking_table_after_restart() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    for version in [189, 254, 279] {
        let f = Fixture::new();
        let handle = f.domain.storage_handle();
        let policy = f
            .domain
            .table_by_name("mysql", "tidb_masking_policy")
            .unwrap();
        f.pool.acquire().unwrap().query(format!("UPDATE mysql.tidb SET variable_value='{version}' WHERE variable_name='tidb_server_version'")).unwrap();
        let mut txn = handle.with_storage(|s| s.Begin(&[])).unwrap();
        txn.Set(
            astersql_meta::transaction_meta_string_key(b"BootstrapKey"),
            version.to_string().into_bytes(),
        )
        .unwrap();
        astersql_meta::TransactionMutator::new(txn.as_mut())
            .drop_table_only(policy.DBID, policy.ID)
            .unwrap();
        assert!(
            astersql_meta::TransactionMutator::new(txn.as_mut())
                .get_table(policy.DBID, policy.ID)
                .unwrap()
                .is_none()
        );
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        f.pool.close();
        f.domain.close();
        let mut config = astersql_domain::DomainConfig::default();
        config.schema_lease = std::time::Duration::ZERO;
        config.stats_lease = std::time::Duration::ZERO;
        let restarted = Arc::new(astersql_domain::Domain::new_with_storage_handle(
            handle,
            Arc::new(astersql_domain::canonical_domain::KvInfoSchemaLoader::new()),
            config,
        ));
        restarted.init().unwrap();
        assert!(
            restarted
                .table_by_name("mysql", "tidb_masking_policy")
                .is_err()
        );
        let session = super::BootstrapCanonicalDomain(restarted.clone()).unwrap();
        let mut rows = session
            .execute(
                "SELECT variable_value FROM mysql.tidb WHERE variable_name='tidb_server_version'",
            )
            .unwrap();
        assert_eq!(
            rows[0].next_row().unwrap().unwrap(),
            vec![unsafe { crate::upgrade_def::currentBootstrapVersion }.to_string()]
        );
        let table = restarted
            .table_by_name("mysql", "tidb_masking_policy")
            .unwrap();
        assert!(!astersql_meta_metadef::IsReservedID(policy.ID));
        assert!(!astersql_meta_metadef::IsReservedID(table.ID));
        assert_ne!(policy.ID, table.ID);
        let expected = [
            "policy_id",
            "policy_name",
            "db_name",
            "table_name",
            "table_id",
            "column_name",
            "column_id",
            "expression",
            "status",
            "masking_type",
            "restrict_on",
            "created_at",
            "updated_at",
            "created_by",
        ];
        assert_eq!(
            table
                .Columns
                .iter()
                .map(|c| c.Name.L.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let mut columns = session.execute("SELECT column_name, LOWER(column_type), is_nullable FROM information_schema.columns WHERE table_schema='mysql' AND table_name='tidb_masking_policy' ORDER BY ordinal_position").unwrap();
        let mut actual_columns = Vec::new();
        while let Some(row) = columns[0].next_row().unwrap() {
            actual_columns.push(row.join(" "));
        }
        assert_eq!(
            actual_columns,
            [
                "policy_id bigint(64) NO",
                "policy_name varchar(64) NO",
                "db_name varchar(64) NO",
                "table_name varchar(64) NO",
                "table_id bigint(64) NO",
                "column_name varchar(64) NO",
                "column_id bigint(64) NO",
                "expression text NO",
                "status varchar(16) NO",
                "masking_type varchar(32) NO",
                "restrict_on varchar(256) NO",
                "created_at datetime(6) NO",
                "updated_at datetime(6) NO",
                "created_by varchar(288) NO",
            ]
        );
        assert!(table.PKIsHandle);
        let mut indexes = session.execute("SELECT index_name, non_unique, seq_in_index, column_name FROM information_schema.statistics WHERE table_schema='mysql' AND table_name='tidb_masking_policy' ORDER BY index_name, seq_in_index").unwrap();
        let mut actual = Vec::new();
        while let Some(row) = indexes[0].next_row().unwrap() {
            actual.push(row);
        }
        let expected = [
            ["PRIMARY", "0", "1", "policy_id"],
            ["uk_table_column", "0", "1", "table_id"],
            ["uk_table_column", "0", "2", "column_id"],
            ["uk_table_policy", "0", "1", "table_id"],
            ["uk_table_policy", "0", "2", "policy_name"],
        ]
        .map(|row| row.map(str::to_string).to_vec())
        .to_vec();
        assert_eq!(actual, expected);
        restarted.close();
    }
}

fn seed_policy(f: &Fixture, id: i64) {
    let col = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns[1]
        .ID;
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_masking_policy (policy_id,policy_name,db_name,table_name,table_id,column_name,column_id,expression,status,masking_type,restrict_on,created_at,updated_at,created_by) VALUES ({id},'p','test','normal_ddl_target',{},'payload',{},'MASK_FULL(payload)','ENABLED','MASK_FULL','NONE','2026-01-01 00:00:00','2026-01-01 00:00:00','root')",f.table,col)).unwrap();
}

#[test]
fn masking_policy_drop_column_persistent_four_phases_and_policy_cleanup() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let col = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns[1]
        .clone();
    let mut j = Job {
        id: 99609,
        tp: 6,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".to_string(),
        table_name: "normal_ddl_target".to_string(),
        state: JobState::Queueing,
        schema_state: SchemaState::Public,
        version: JobVersion::V2,
        raw_args: serde_json::to_vec(&serde_json::json!({"column_info":col})).unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut j);
    for state in [
        SchemaState::WriteOnly,
        SchemaState::DeleteOnly,
        SchemaState::DeleteReorganization,
        SchemaState::None,
    ] {
        run(&f);
        let stored = f.queue(j.id).unwrap_or_else(|| {
            let rows = f
                .pool
                .acquire()
                .unwrap()
                .query(format!(
                    "SELECT job_meta FROM mysql.tidb_ddl_history WHERE job_id={}",
                    j.id
                ))
                .unwrap();
            panic!(
                "job missing at {state:?}: {:?}",
                rows.first()
                    .map(|r| astersql_meta::decode_go_history_job(r[0].as_bytes())
                        .unwrap()
                        .error)
            );
        });
        assert_eq!(stored.schema_state, state);
    }
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Columns
            .len(),
        1
    );
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn masking_policy_drop_column_missing_policy_table_preserves_reorganization() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let col = f
        .reader()
        .get_table(f.db, f.table)
        .unwrap()
        .unwrap()
        .Columns[1]
        .clone();
    let mut j = Job {
        id: 99610,
        tp: 6,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: "normal_ddl_target".into(),
        state: JobState::Queueing,
        schema_state: SchemaState::Public,
        version: JobVersion::V2,
        raw_args: serde_json::to_vec(&serde_json::json!({"column_info": col})).unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut j);
    for _ in 0..3 {
        run(&f);
    }
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.masking_backup")
        .unwrap();
    run(&f);
    let stored = f.queue(j.id).unwrap();
    assert_eq!(stored.state, JobState::Running);
    assert_eq!(stored.schema_state, SchemaState::DeleteReorganization);
    assert!(stored.error.as_ref().unwrap().contains("1146"));
    let t = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(t.Columns.len(), 2);
    assert_eq!(t.Columns[1].State, SchemaState::DeleteReorganization);
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE mysql.masking_backup TO mysql.tidb_masking_policy")
        .unwrap();
    run(&f);
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
}

#[test]
fn masking_policy_drop_column_v1_preserves_unrelated_policy() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let unrelated = table.Columns[0].ID;
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_masking_policy (policy_id,policy_name,db_name,table_name,table_id,column_name,column_id,expression,status,masking_type,restrict_on,created_at,updated_at,created_by) VALUES (9605,'unrelated','test','normal_ddl_target',{},'id',{},'MASK_FULL(id)','bad','MASK_FULL','NONE','2026-01-01 00:00:00','2026-01-01 00:00:00','root')", f.table, unrelated)).unwrap();
    let mut j = Job {
        id: 99611,
        tp: 6,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: "normal_ddl_target".into(),
        state: JobState::Queueing,
        schema_state: SchemaState::Public,
        version: JobVersion::V1,
        raw_args: serde_json::to_vec(&serde_json::json!([table.Columns[1].Name, false])).unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut j);
    for _ in 0..4 {
        run(&f);
    }
    assert_eq!(f.queue(j.id).unwrap().state, JobState::Done);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy ORDER BY policy_id")
            .unwrap(),
        vec![vec!["9605".to_string()]]
    );
}

fn insert_modify(f: &Fixture, missing: bool) -> Job {
    seed_policy(f, 9606);
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut column = table.Columns[1].clone();
    column.Name = astersql_meta_model::ast::NewCIStr("renamed_payload");
    column.SetFlen(80);
    let mut job=Job { id:99612,tp:12,schema_id:f.db,table_id:f.table,
        schema_name:"test".into(),table_name:"normal_ddl_target".into(),state:JobState::Queueing,
        schema_state:SchemaState::Public,version:JobVersion::V2,
        raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":1})).unwrap(),..Default::default() };
    let wire = job.encode(false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},0,'{}','{}',X'{}',12,0)",job.id,f.db,f.table,hex(&wire))).unwrap();
    if missing {
        f.pool
            .acquire()
            .unwrap()
            .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.masking_backup")
            .unwrap();
    }
    job
}
#[test]
fn modify_column_persistent_syncs_nonempty_masking_policy() {
    let f = Fixture::new();
    let job = insert_modify(&f, false);
    run(&f);
    let updated = f.queue(job.id).unwrap();
    assert_eq!(updated.state, JobState::Done, "{:?}", updated.error);
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Columns[1]
            .Name
            .L,
        "renamed_payload"
    );
    let rows=f.pool.acquire().unwrap().query("SELECT column_name,expression,created_at FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap();
    assert_eq!(rows[0][0], "renamed_payload");
    assert_eq!(rows[0][1], "MASK_FULL(`renamed_payload`)");
    assert_eq!(rows[0][2], "2026-01-01 00:00:00.000000");
}
#[test]
fn modify_column_persistent_missing_policy_table_rolls_back() {
    let f = Fixture::new();
    let job = insert_modify(&f, true);
    run(&f);
    let updated = f.queue(job.id).unwrap();
    assert_eq!(updated.state, JobState::Rollingback, "{:?}", updated.error);
    assert!(updated.error.unwrap().contains("1146"));
    assert_eq!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Columns[1]
            .Name
            .L,
        "payload"
    );
}

#[test]
fn modify_column_persistent_reorg_final_syncs_new_column_id() {
    for missing in [false, true] {
        let f = Fixture::new();
        seed_policy(&f, 9606);
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let old = table.Columns[1].clone();
        let mut target = old.clone();
        target.ID = table.MaxColumnID + 1;
        target.Name = astersql_meta_model::ast::NewCIStr("renamed_payload");
        target.State = SchemaState::Public;
        table.MaxColumnID = target.ID;
        table.Columns[1].State = SchemaState::DeleteOnly;
        table.Columns[1].Name = astersql_meta_model::ast::NewCIStr("_Tombstone$_payload");
        target.Offset = 1;
        table.Columns.insert(1, target.clone());
        table.Columns[2].Offset = 2;
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        astersql_meta::TransactionMutator::new(txn.as_mut())
            .update_table(f.db, &mut table)
            .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        f.domain.reload().unwrap();
        let mut job=Job{id:99613,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Running,schema_state:SchemaState::Public,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":target,"old_column_id":old.ID,"old_column_name":old.Name,"changing_column":target,"modify_column_type":4})).unwrap(),..Default::default()};
        let wire = job.encode(false).unwrap();
        f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},1,'{}','{}',X'{}',12,0)",job.id,f.db,f.table,hex(&wire))).unwrap();
        if missing {
            f.pool
                .acquire()
                .unwrap()
                .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.masking_backup")
                .unwrap();
        }
        run(&f);
        let updated = f.queue(job.id).unwrap();
        if missing {
            assert_eq!(updated.state, JobState::Running);
            assert!(updated.error.unwrap().contains("1146"));
            assert_eq!(
                f.reader()
                    .get_table(f.db, f.table)
                    .unwrap()
                    .unwrap()
                    .Columns
                    .len(),
                3
            );
        } else {
            assert_eq!(updated.state, JobState::Done, "{:?}", updated.error);
            assert_eq!(
                f.reader()
                    .get_table(f.db, f.table)
                    .unwrap()
                    .unwrap()
                    .Columns
                    .len(),
                2
            );
            let rows=f.pool.acquire().unwrap().query("SELECT column_id,column_name,expression FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap();
            assert_eq!(
                rows,
                vec![vec![
                    target.ID.to_string(),
                    "renamed_payload".into(),
                    "MASK_FULL(`renamed_payload`)".into()
                ]]
            );
        }
    }
}

struct DomainBarrier(std::sync::Weak<super::Domain>);
impl DdlSchemaBarrier for DomainBarrier {
    fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
        self.0
            .upgrade()
            .ok_or("Domain closed")?
            .reload()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    fn wait(&mut self, _: &Job, version: i64, _: &dyn JobLease) -> Result<(), String> {
        let published = self
            .0
            .upgrade()
            .ok_or("Domain closed")?
            .reload()
            .map_err(|error| error.to_string())?;
        if published < version {
            return Err("Domain did not observe DDL schema version".into());
        }
        Ok(())
    }
}
fn install_service(f: &Fixture) -> Arc<super::normal_ddl_service::NormalDdlService> {
    use astersql_domain::domain::{DdlService, StartMode};
    let cancellation = astersql_owner::manager::Context::new();
    let owner = astersql_owner::mock::NewMockManager(
        cancellation.clone(),
        "masking-worker",
        None,
        format!("/masking-worker/{}", f.db),
    );
    let domain = Arc::downgrade(&f.domain);
    let service = Arc::new(super::normal_ddl_service::NormalDdlService::new(
        owner,
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        cancellation,
        f.pool.clone(),
        Arc::new(super::normal_ddl_service::DomainSchemaLoader(
            Arc::downgrade(&f.domain),
        )),
        Arc::new(move || {
            Ok(Box::new(NormalDdlExecutor {
                barrier: DomainBarrier(domain.clone()),
                policy: Policy,
                sequence: Arc::new(AtomicI64::new(0)),
            }))
        }),
        Arc::new(|_| Err("unused table mode".into())),
        Arc::new(|| {}),
        true,
    ));
    f.domain.set_ddl(service.clone());
    service.start(StartMode::Normal).unwrap();
    service
}
#[test]
fn masking_policy_sql_change_column_uses_persistent_worker() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query(
            "ALTER TABLE test.normal_ddl_target CHANGE COLUMN payload renamed_payload VARCHAR(80) FIRST",
        )
        .unwrap();
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT column_name,expression FROM mysql.tidb_masking_policy WHERE policy_id=9606")
        .unwrap();
    assert_eq!(
        rows,
        vec![vec![
            "renamed_payload".to_string(),
            "MASK_FULL(`renamed_payload`)".to_string()
        ]]
    );
    assert_eq!(
        f.domain
            .table_by_name("test", "normal_ddl_target")
            .unwrap()
            .Columns[0]
            .Name
            .L,
        "renamed_payload"
    );
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    assert!(histories.iter().any(|row| {
        astersql_meta::decode_go_history_job(row[0].as_bytes())
            .is_ok_and(|j| j.tp == 12 && j.state == JobState::Synced)
    }));
}

#[test]
fn masking_policy_sql_table_actions_use_persistent_worker() {
    let _resources = LabelResources::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("TRUNCATE TABLE test.normal_ddl_target")
        .unwrap();
    let new = f
        .domain
        .table_by_name("test", "normal_ddl_target")
        .unwrap()
        .ID;
    assert_ne!(new, f.table);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT table_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec![new.to_string()]]
    );
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE test.normal_ddl_target TO test.renamed_target")
        .unwrap();
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT table_name FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec!["renamed_target".to_string()]]
    );
    f.pool
        .acquire()
        .unwrap()
        .query("ALTER TABLE test.renamed_target DROP COLUMN payload")
        .unwrap();
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap()
            .is_empty()
    );
    f.pool
        .acquire()
        .unwrap()
        .query("DROP TABLE test.renamed_target")
        .unwrap();
    assert!(f.domain.table_by_name("test", "renamed_target").is_err());
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let types: Vec<_> = histories
        .iter()
        .map(|row| {
            astersql_meta::decode_go_history_job(row[0].as_bytes())
                .unwrap()
                .tp
        })
        .collect();
    for action in [4, 6, 11, 14] {
        assert!(
            types.contains(&action),
            "missing history action {action}: {types:?}"
        );
    }
}

#[test]
fn masking_policy_sql_modify_reorg_keeps_nonempty_rows_and_changes_column_id() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30'),(2,'45.60')")
        .unwrap();
    let old = f
        .domain
        .table_by_name("test", "normal_ddl_target")
        .unwrap()
        .Columns[1]
        .ID;
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
        .unwrap();
    let table = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    let new = table
        .Columns
        .iter()
        .find(|c| c.Name.L == "payload")
        .unwrap()
        .ID;
    assert_ne!(new, old);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT id,payload FROM test.normal_ddl_target ORDER BY id")
            .unwrap(),
        vec![
            vec!["1".to_string(), "12.30".to_string()],
            vec!["2".to_string(), "45.60".to_string()]
        ]
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT column_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec![new.to_string()]]
    );
}

#[test]
fn modify_column_persistent_reorg_advances_real_rows() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30')")
        .unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut column = table.Columns[1].clone();
    column.SetType(246);
    column.SetFlen(10);
    column.SetDecimal(2);
    column.SetCharset("binary".to_string());
    column.SetCollate("binary".to_string());
    let mut job=Job{id:99614,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":4})).unwrap(),..Default::default()};
    insert_job(&f, &mut job);
    for _ in 0..12 {
        run(&f);
        let current = f.queue(job.id).unwrap();
        assert!(
            current.error.is_none(),
            "state {:?}/{:?}: {:?}",
            current.state,
            current.schema_state,
            current.error
        );
        if current.state == JobState::Done {
            return;
        }
    }
    panic!("modify-column did not finish");
}

#[test]
fn modify_column_check_rejects_null_and_clears_temporary_flags() {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,NULL)")
        .unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut column = table.Columns[1].clone();
    column.SetFlag(column.GetFlag() | astersql_parser_mysql::r#type::NotNullFlag);
    let mut job=Job{id:99615,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":0})).unwrap(),..Default::default()};
    insert_job(&f, &mut job);
    run(&f);
    assert!(f.queue(job.id).unwrap().error.is_none());
    assert_ne!(
        f.reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Columns[1]
            .GetFlag()
            & astersql_parser_mysql::r#type::PreventNullInsertFlag,
        0
    );
    run(&f);
    assert!(f.queue(job.id).unwrap().error.unwrap().contains("1138"));
    run(&f);
    assert_eq!(f.queue(job.id).unwrap().state, JobState::RollbackDone);
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(
        table.Columns[1].GetFlag() & astersql_parser_mysql::r#type::PreventNullInsertFlag,
        0
    );
    assert_eq!(f.queue(job.id).unwrap().schema_state, SchemaState::None);
}
#[test]
fn modify_column_reorg_rollback_removes_changing_column() {
    let f = Fixture::new();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut column = table.Columns[1].clone();
    column.SetType(246);
    column.SetFlen(10);
    column.SetDecimal(2);
    let mut job=Job{id:99616,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":4})).unwrap(),..Default::default()};
    insert_job(&f, &mut job);
    run(&f);
    run(&f);
    let mut rolling = f.queue(job.id).unwrap();
    rolling.state = JobState::Rollingback;
    f.pool
        .acquire()
        .unwrap()
        .query(format!(
            "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
            hex(&rolling.encode(false).unwrap()),
            job.id
        ))
        .unwrap();
    run(&f);
    let restored = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    assert_eq!(restored.Columns.len(), 2);
    assert_eq!(restored.Columns[1].Name.L, "payload");
    assert_eq!(f.queue(job.id).unwrap().schema_state, SchemaState::None);
}

#[test]
fn masking_policy_operations_require_system_table_with_failpoint() {
    let _resources = LabelResources::new();
    for action in [4, 6, 11, 12, 14] {
        let f = Fixture::new();
        seed_policy(&f, 9606);
        let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let args = match action {
            6 => serde_json::json!({"column_info":table.Columns[1],"ignore_existence_err":false}),
            11 => serde_json::json!({"new_table_id":f.table+1000,"fk_check":false}),
            12 => {
                serde_json::json!({"column":table.Columns[1],"old_column_name":table.Columns[1].Name,"modify_column_type":1})
            }
            14 => {
                serde_json::json!({"old_schema_id":f.db,"old_schema_name":astersql_meta_model::ast::NewCIStr("test"),"new_table_name":astersql_meta_model::ast::NewCIStr("renamed_target")})
            }
            _ => serde_json::json!({}),
        };
        let mut job = Job {
            id: 99700 + action as i64,
            tp: action,
            schema_id: f.db,
            table_id: f.table,
            schema_name: "test".into(),
            table_name: table.Name.L.clone(),
            state: JobState::Queueing,
            schema_state: if action == 4 {
                SchemaState::Public
            } else {
                SchemaState::None
            },
            version: JobVersion::V2,
            raw_args: serde_json::to_vec(&args).unwrap(),
            ..Default::default()
        };
        insert_job(&f, &mut job);
        for _ in 0..match action {
            4 => 2,
            6 => 3,
            _ => 0,
        } {
            run(&f);
        }
        let _guard = astersql_testkit_testfailpoint::enable(
            "github.com/pingcap/tidb/pkg/ddl/mockMissingMaskingPolicySysTable",
            "return(true)",
        );
        run(&f);
        let failed = f
            .queue(job.id)
            .or_else(|| f.reader().get_history_ddl_job(job.id).unwrap())
            .unwrap();
        assert!(
            failed
                .error
                .as_ref()
                .is_some_and(|error| error.contains("[schema:1146]")),
            "action {action}: {:?}",
            failed.error
        );
        assert_ne!(failed.state, JobState::Done);
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT policy_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
                .unwrap(),
            vec![vec!["9606".to_string()]]
        );
    }
}
#[test]
fn masking_policy_sql_chain_rename_is_one_durable_job() {
    let _resources = LabelResources::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let _service = install_service(&f);
    f.pool.acquire().unwrap().query("RENAME TABLE test.normal_ddl_target TO test.intermediate_target, test.intermediate_target TO test.final_target").unwrap();
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT table_name FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec!["final_target".to_string()]]
    );
    assert!(f.domain.table_by_name("test", "normal_ddl_target").is_err());
    assert_eq!(
        f.domain.table_by_name("test", "final_target").unwrap().ID,
        f.table
    );
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    assert_eq!(
        histories
            .iter()
            .filter(
                |row| astersql_meta::decode_go_history_job(row[0].as_bytes())
                    .is_ok_and(|j| j.tp == 47 && j.state == JobState::Synced)
            )
            .count(),
        1
    );
}

#[test]
fn masking_policy_sql_partition_truncate_assigns_physical_ids() {
    let _resources = LabelResources::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Partition = Some(astersql_meta_model::PartitionInfo {
        Enable: true,
        Type: astersql_meta_model::ast::PartitionType::Range,
        Expr: "id".into(),
        Num: 2,
        Definitions: vec![
            astersql_meta_model::PartitionDefinition {
                ID: f.table + 100,
                Name: astersql_meta_model::ast::NewCIStr("p0"),
                LessThan: vec!["10".into()],
                ..Default::default()
            },
            astersql_meta_model::PartitionDefinition {
                ID: f.table + 101,
                Name: astersql_meta_model::ast::NewCIStr("p1"),
                LessThan: vec!["MAXVALUE".into()],
                ..Default::default()
            },
        ],
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
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'first'),(20,'second')")
        .unwrap();
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("TRUNCATE TABLE test.normal_ddl_target")
        .unwrap();
    let new = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    assert_ne!(new.ID, f.table);
    assert!(new.Partition.as_ref().unwrap().Definitions.iter().all(|d| {
        !table
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .any(|old| old.ID == d.ID)
    }));
    assert!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT * FROM test.normal_ddl_target")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT table_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
            .unwrap(),
        vec![vec![new.ID.to_string()]]
    );
}
#[test]
fn masking_policy_sql_modify_reorg_rebuilds_secondary_index() {
    modify_column_secondary_index_analyze(false);
}

#[test]
fn masking_policy_sql_modify_reorg_analyzes_changing_index() {
    modify_column_secondary_index_analyze(true);
}

fn modify_column_secondary_index_analyze(analyze: bool) {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
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
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30'),(2,'45.60')")
        .unwrap();
    let _service = install_service(&f);
    let lease = f.pool.acquire().unwrap();
    if analyze {
        lease
            .query("SET SESSION tidb_stats_update_during_ddl=ON")
            .unwrap();
    }
    lease
        .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
        .unwrap();
    drop(lease);
    let new = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    assert_eq!(new.Indices.len(), 1);
    assert_eq!(new.Indices[0].Name.L, "payload_idx");
    assert_ne!(new.Indices[0].ID, table.Indices[0].ID);
    f.pool
        .acquire()
        .unwrap()
        .query("ADMIN CHECK TABLE test.normal_ddl_target")
        .unwrap();
    assert_eq!(f.pool.acquire().unwrap().query("SELECT payload FROM test.normal_ddl_target FORCE INDEX(payload_idx) WHERE payload=12.30").unwrap(),vec![vec!["12.30".to_string()]]);
    if analyze {
        let histograms = f.pool.acquire().unwrap().query(format!("SELECT hist_id FROM mysql.stats_histograms WHERE table_id={} AND is_index=1 AND hist_id={}", f.table, new.Indices[0].ID)).unwrap();
        assert_eq!(
            histograms,
            vec![vec![new.Indices[0].ID.to_string()]],
            "changing index must be actually analyzed; all={:?}; jobs={:?}",
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT table_id,is_index,hist_id FROM mysql.stats_histograms"),
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT state,job_info FROM mysql.analyze_jobs")
        );
        let jobs = f.pool.acquire().unwrap().query("SELECT state FROM mysql.analyze_jobs WHERE table_schema='test' AND table_name='normal_ddl_target'").unwrap();
        assert!(jobs.iter().any(|row| row[0] == "finished"));
    }
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let done = histories
        .iter()
        .filter_map(|row| astersql_meta::decode_go_history_job(row[0].as_bytes()).ok())
        .find(|job| job.tp == 12)
        .unwrap();
    assert_eq!(
        done.reorg_meta.as_ref().unwrap().AnalyzeState,
        if analyze {
            astersql_meta_model::group_3::AnalyzeStateDone
        } else {
            astersql_meta_model::group_3::AnalyzeStateSkipped
        }
    );
    let args: serde_json::Value = serde_json::from_slice(&done.raw_args).unwrap();
    assert_eq!(
        args["new_index_ids"],
        serde_json::json!([new.Indices[0].ID])
    );
    assert_eq!(
        args["index_ids"],
        serde_json::json!([
            table.Indices[0].ID,
            new.Indices[0].ID | astersql_tablecodec::TempIndexPrefix
        ])
    );
}

#[test]
fn modify_column_write_reorganization_dual_writes_changing_index() {
    let f = Fixture::new();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
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
    f.domain.reload().unwrap();
    let mut column = table.Columns[1].clone();
    column.SetType(246);
    column.SetFlen(10);
    column.SetDecimal(2);
    let mut job=Job{id:99617,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":4})).unwrap(),..Default::default()};
    insert_job(&f, &mut job);
    run(&f);
    run(&f);
    run(&f);
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30')")
        .unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let changing = table.Indices.iter().find(|i| i.IsChanging()).unwrap();
    let snapshot = f
        .domain
        .storage_handle()
        .with_storage(|s| {
            let v = s.CurrentVersion("global")?;
            Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
        })
        .unwrap();
    let prefix = astersql_tablecodec::GenTableIndexPrefix(f.table).0;
    let mut index_prefix = prefix;
    index_prefix.extend_from_slice(&astersql_tablecodec::codec::EncodeInt(
        Vec::new(),
        changing.ID,
    ));
    let mut iter = snapshot
        .Iter(
            astersql_kv::Key(index_prefix.clone()),
            Some(astersql_kv::Key(index_prefix).PrefixNext()),
        )
        .unwrap();
    assert!(iter.Valid(), "changing index wasn't dual-written");
    iter.Close();
}

#[test]
fn modify_column_policy_validation_preserves_metadata_and_policy() {
    for (generated, kind, expression, expected) in [
        (true, 253, "MASK_FULL(payload)", "generated column"),
        (false, 245, "MASK_FULL(payload)", "unsupported column type"),
        (false, 253, "MASK_FULL(", "line 1 column"),
    ] {
        let f = Fixture::new();
        seed_policy(&f, 9606);
        let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "UPDATE mysql.tidb_masking_policy SET expression='{}' WHERE policy_id=9606",
                expression
            ))
            .unwrap();
        let mut column = table.Columns[1].clone();
        column.Name = astersql_meta_model::ast::NewCIStr("renamed_payload");
        column.SetType(kind);
        if generated {
            column.GeneratedExprString = "id + 1".into();
        }
        let mut job=Job{id:99618,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":1})).unwrap(),..Default::default()};
        insert_job(&f, &mut job);
        run(&f);
        let failed = f.queue(job.id).unwrap();
        assert_eq!(failed.state, JobState::Rollingback);
        let error = failed.error.unwrap();
        assert!(
            error.to_lowercase().contains(expected),
            "expected {expected}: {error}"
        );
        assert_eq!(
            f.reader()
                .get_table(f.db, f.table)
                .unwrap()
                .unwrap()
                .Columns[1]
                .Name
                .L,
            "payload"
        );
        assert_eq!(f.pool.acquire().unwrap().query("SELECT column_name,expression FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap(),vec![vec!["payload".to_string(),expression.to_string()]]);
    }
}
#[test]
fn modify_column_v1_policy_expression_and_normalized_fields() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    f.pool.acquire().unwrap().query("UPDATE mysql.tidb_masking_policy SET status='enable',restrict_on='CTAS,INSERT_INTO_SELECT,CTAS',expression='CONCAT(payload, \"payload\")' WHERE policy_id=9606").unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let mut column = table.Columns[1].clone();
    column.Name = astersql_meta_model::ast::NewCIStr("renamed_payload");
    let mut job = Job {
        id: 99619,
        tp: 12,
        schema_id: f.db,
        table_id: f.table,
        schema_name: "test".into(),
        table_name: table.Name.L.clone(),
        state: JobState::Queueing,
        version: JobVersion::V1,
        raw_args: serde_json::to_vec(&serde_json::json!([
            column,
            table.Columns[1].Name,
            null,
            1,
            0,
            null,
            null,
            null,
            0
        ]))
        .unwrap(),
        ..Default::default()
    };
    insert_job(&f, &mut job);
    run(&f);
    assert_eq!(
        f.queue(job.id)
            .or_else(|| f.reader().get_history_ddl_job(job.id).unwrap())
            .unwrap()
            .state,
        JobState::Done,
        "{:?}",
        f.reader()
            .get_history_ddl_job(job.id)
            .unwrap()
            .map(|j| j.error)
    );
    assert_eq!(f.pool.acquire().unwrap().query("SELECT status,restrict_on,expression,created_by FROM mysql.tidb_masking_policy WHERE policy_id=9606").unwrap(),vec![vec!["ENABLED".to_string(),"INSERT_INTO_SELECT,CTAS".to_string(),"CONCAT(`renamed_payload`,_UTF8MB4'payload')".to_string(),"root".to_string()]]);
    assert_eq!(f.queue(job.id).unwrap().raw_args, b"[[],[],[]]");
}

#[test]
fn masking_policy_sql_rename_column_uses_persistent_modify() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("ALTER TABLE test.normal_ddl_target RENAME COLUMN payload TO renamed_payload")
        .unwrap();
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query(
                "SELECT column_name,expression FROM mysql.tidb_masking_policy WHERE policy_id=9606"
            )
            .unwrap(),
        vec![vec![
            "renamed_payload".to_string(),
            "MASK_FULL(`renamed_payload`)".to_string()
        ]]
    );
}

#[test]
fn modify_column_v1_finished_null_slices_register_empty_delete_range() {
    let f = Fixture::new();
    let mut job = Job {
        id: 99620,
        tp: 12,
        schema_id: f.db,
        table_id: f.table,
        state: JobState::Done,
        version: JobVersion::V1,
        raw_args: b"[null,null,null]".to_vec(),
        ..Default::default()
    };
    assert_eq!(
        f.pool.acquire().unwrap().register_delete_ranges(&mut job),
        Ok(())
    );
}

#[test]
fn masking_policy_sql_varchar_to_char_precheck_preserves_value_semantics() {
    for trailing in [false, true] {
        let f = Fixture::new();
        seed_policy(&f, 9606);
        let old = f
            .reader()
            .get_table(f.db, f.table)
            .unwrap()
            .unwrap()
            .Columns[1]
            .ID;
        let _service = install_service(&f);
        let mut session = f.pool.acquire().unwrap();
        session
            .query("SET SESSION sql_mode='STRICT_ALL_TABLES'")
            .unwrap();
        session
            .query(if trailing {
                "INSERT INTO test.normal_ddl_target VALUES (1,'value ')"
            } else {
                "INSERT INTO test.normal_ddl_target VALUES (1,'value')"
            })
            .unwrap();
        session
            .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload CHAR(20)")
            .unwrap();
        assert_eq!(
            session
                .query("SELECT payload FROM test.normal_ddl_target")
                .unwrap(),
            vec![vec!["value".to_string()]]
        );
        let new = f
            .domain
            .table_by_name("test", "normal_ddl_target")
            .unwrap()
            .Columns[1]
            .ID;
        assert_eq!(new != old, trailing);
        assert_eq!(
            session
                .query("SELECT column_id FROM mysql.tidb_masking_policy WHERE policy_id=9606")
                .unwrap(),
            vec![vec![new.to_string()]]
        );
    }
}

#[test]
fn masking_policy_sql_modify_preserves_reorg_session_snapshot() {
    struct RestoreSpeed(i64);
    impl Drop for RestoreSpeed {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::DDLReorgMaxWriteSpeed.Store(self.0);
        }
    }
    let _speed = RestoreSpeed(astersql_sessionctx_vardef::DDLReorgMaxWriteSpeed.Load());
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let _service = install_service(&f);
    let mut session = f.pool.acquire().unwrap();
    session
        .query("SET GLOBAL tidb_ddl_reorg_max_write_speed='1.5MiB'")
        .unwrap();
    assert_eq!(
        session
            .query("SELECT @@global.tidb_ddl_reorg_max_write_speed")
            .unwrap(),
        vec![vec!["1572864".to_owned()]]
    );
    session
        .query("SET GLOBAL tidb_ddl_reorg_max_write_speed='0X_1.8p+0MiB'")
        .unwrap();
    assert_eq!(
        session
            .query("SELECT @@global.tidb_ddl_reorg_max_write_speed")
            .unwrap(),
        vec![vec!["1572864".to_owned()]]
    );
    session.query("SET SESSION time_zone='+08:00'").unwrap();
    session
        .query("SET SESSION tidb_ddl_reorg_batch_size=128")
        .unwrap();
    session
        .query("SET SESSION tidb_ddl_reorg_worker_cnt=3")
        .unwrap();
    session
        .query(
            "ALTER TABLE test.normal_ddl_target CHANGE COLUMN payload renamed_payload VARCHAR(80)",
        )
        .unwrap();
    let histories = session
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let job = histories
        .iter()
        .filter_map(|row| astersql_meta::decode_go_history_job(row[0].as_bytes()).ok())
        .find(|job| job.tp == 12)
        .unwrap();
    let meta = job
        .reorg_meta
        .expect("ordinary MODIFY must retain Go NewDDLReorgMeta snapshot");
    assert_eq!(
        meta.BatchSize.load(std::sync::atomic::Ordering::SeqCst),
        128
    );
    assert_eq!(
        meta.Concurrency.load(std::sync::atomic::Ordering::SeqCst),
        3
    );
    assert_eq!(meta.GetMaxWriteSpeed(), 1572864);
    assert_eq!(meta.Location.as_ref().unwrap().offset, 8 * 3600);
    assert_eq!(meta.SQLMode, job.sql_mode);
}

#[test]
fn modify_column_fast_reorg_merges_concurrent_mutations() {
    let f = Fixture::new();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
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
    f.domain.reload().unwrap();
    let mut column = table.Columns[1].clone();
    column.SetType(246);
    column.SetFlen(10);
    column.SetDecimal(2);
    let mut job=Job{id:99617,tp:12,schema_id:f.db,table_id:f.table,schema_name:"test".into(),table_name:table.Name.L.clone(),state:JobState::Queueing,version:JobVersion::V2,raw_args:serde_json::to_vec(&serde_json::json!({"column":column,"old_column_name":table.Columns[1].Name,"modify_column_type":4})).unwrap(),..Default::default()};
    job.reorg_meta = Some(astersql_meta_model::group_3::DDLReorgMeta {
        IsFastReorg: true,
        ..Default::default()
    });
    insert_job(&f, &mut job);
    run(&f);
    run(&f);
    run(&f);
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30')")
        .unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let changing = table.Indices.iter().find(|i| i.IsChanging()).unwrap();
    let snapshot = f
        .domain
        .storage_handle()
        .with_storage(|s| {
            let v = s.CurrentVersion("global")?;
            Ok::<_, astersql_kv::Error>(s.GetSnapshot(v))
        })
        .unwrap();
    let prefix = astersql_tablecodec::GenTableIndexPrefix(f.table).0;
    let mut index_prefix = prefix;
    index_prefix.extend_from_slice(&astersql_tablecodec::codec::EncodeInt(
        Vec::new(),
        changing.ID | astersql_tablecodec::TempIndexPrefix,
    ));
    let mut iter = snapshot
        .Iter(
            astersql_kv::Key(index_prefix.clone()),
            Some(astersql_kv::Key(index_prefix).PrefixNext()),
        )
        .unwrap();
    assert!(iter.Valid(), "changing index wasn't dual-written");
    iter.Close();
    f.pool
        .acquire()
        .unwrap()
        .query("UPDATE test.normal_ddl_target SET payload='45.60' WHERE id=1")
        .unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (2,'78.90')")
        .unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("DELETE FROM test.normal_ddl_target WHERE id=2")
        .unwrap();
    let mut observed = std::collections::BTreeSet::new();
    for _ in 0..40 {
        run(&f);
        let current = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        for index in &current.Indices {
            if index.IsChanging() {
                observed.insert(index.BackfillState.0);
            }
        }
        if f.queue(job.id)
            .is_some_and(|job| job.state == JobState::Done)
        {
            break;
        }
    }
    assert_eq!(f.queue(job.id).unwrap().state, JobState::Done);
    assert!(observed.contains(&astersql_meta_model::BackfillStateReadyToMerge.0));
    assert!(observed.contains(&astersql_meta_model::BackfillStateMerging.0));
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("ADMIN CHECK TABLE test.normal_ddl_target")
        .unwrap();
    assert_eq!(f.pool.acquire().unwrap().query("SELECT payload FROM test.normal_ddl_target FORCE INDEX(payload_idx) WHERE payload=45.60").unwrap(),vec![vec!["45.60".to_owned()]]);
}

#[test]
fn masking_policy_sql_modify_ingest_rebuilds_secondary_index() {
    let _ingest = IngestEnvironment::new();
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
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
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30'),(2,'45.60')")
        .unwrap();
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
        .unwrap();
    let new = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    assert_eq!(new.Indices.len(), 1);
    assert_eq!(new.Indices[0].Name.L, "payload_idx");
    assert_ne!(new.Indices[0].ID, table.Indices[0].ID);
    f.pool
        .acquire()
        .unwrap()
        .query("ADMIN CHECK TABLE test.normal_ddl_target")
        .unwrap();
    assert_eq!(f.pool.acquire().unwrap().query("SELECT payload FROM test.normal_ddl_target FORCE INDEX(payload_idx) WHERE payload=12.30").unwrap(),vec![vec!["12.30".to_string()]]);
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let done = histories
        .iter()
        .filter_map(|row| astersql_meta::decode_go_history_job(row[0].as_bytes()).ok())
        .find(|job| job.tp == 12)
        .unwrap();
    let meta = done.reorg_meta.as_ref().unwrap();
    assert!(meta.IsFastReorg);
    assert_eq!(
        meta.ReorgTp,
        astersql_meta_model::group_3::ReorgType::ReorgTypeIngest
    );
    let args: serde_json::Value = serde_json::from_slice(&done.raw_args).unwrap();
    assert_eq!(
        args["new_index_ids"],
        serde_json::json!([new.Indices[0].ID])
    );
    assert_eq!(
        args["index_ids"],
        serde_json::json!([
            table.Indices[0].ID,
            new.Indices[0].ID | astersql_tablecodec::TempIndexPrefix
        ])
    );
}

struct IngestEnvironment {
    old: Option<std::path::PathBuf>,
    path: std::path::PathBuf,
}
impl IngestEnvironment {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("astersql-ddl-ingest-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let old = astersql_ddl::index::replace_global_lightning_env_for_test(Some(path.clone()));
        Self { old, path }
    }
}
impl Drop for IngestEnvironment {
    fn drop(&mut self) {
        astersql_ddl::index::replace_global_lightning_env_for_test(self.old.take());
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn masking_policy_sql_modify_index_reorg_preserves_column_and_row_ids() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Columns[1].SetType(254);
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let mut meta = astersql_meta::TransactionMutator::new(txn.as_mut());
    meta.update_table(f.db, &mut table).unwrap();
    let version = meta.gen_schema_version().unwrap();
    meta.set_table_schema_diff(
        &Job {
            tp: 12,
            schema_id: f.db,
            table_id: f.table,
            ..Default::default()
        },
        version,
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30'),(2,'45.60')")
        .unwrap();
    let _service = install_service(&f);
    f.pool
        .acquire()
        .unwrap()
        .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload VARCHAR(40)")
        .unwrap();
    let new = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    assert_eq!(new.Columns[1].ID, table.Columns[1].ID);
    assert_eq!(new.Indices.len(), 1);
    assert_eq!(new.Indices[0].Name.L, "payload_idx");
    assert_ne!(new.Indices[0].ID, table.Indices[0].ID);
    f.pool
        .acquire()
        .unwrap()
        .query("ADMIN CHECK TABLE test.normal_ddl_target")
        .unwrap();
    assert_eq!(f.pool.acquire().unwrap().query("SELECT payload FROM test.normal_ddl_target FORCE INDEX(payload_idx) WHERE payload='12.30'").unwrap(),vec![vec!["12.30".to_string()]]);
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let done = histories
        .iter()
        .filter_map(|row| astersql_meta::decode_go_history_job(row[0].as_bytes()).ok())
        .find(|job| job.tp == 12)
        .unwrap();
    assert!(done.reorg_meta.as_ref().unwrap().IsFastReorg);
    assert_eq!(
        done.reorg_meta.as_ref().unwrap().ReorgTp,
        astersql_meta_model::group_3::ReorgType::ReorgTypeTxnMerge
    );
    let args: serde_json::Value = serde_json::from_slice(&done.raw_args).unwrap();
    assert_eq!(args["new_index_ids"], serde_json::json!([]));
    assert_eq!(
        args["index_ids"],
        serde_json::json!([
            table.Indices[0].ID,
            new.Indices[0].ID | astersql_tablecodec::TempIndexPrefix
        ])
    );
}

#[test]
fn masking_policy_sql_modify_dist_reorg_executes_persistent_subtasks() {
    modify_dist_reorg(false, false, false, false, false, false);
}

#[test]
fn masking_policy_sql_modify_dist_reorg_backfills_all_partitions() {
    modify_dist_reorg(true, false, false, false, false, false);
}

#[test]
fn masking_policy_sql_modify_dist_reorg_pauses_and_resumes() {
    modify_dist_reorg(false, true, false, false, false, false);
}

struct BackfillRelease(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
impl BackfillRelease {
    fn release(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
impl Drop for BackfillRelease {
    fn drop(&mut self) {
        self.release();
    }
}

#[test]
fn masking_policy_sql_modify_dist_reorg_uses_named_node_scope() {
    modify_dist_reorg(false, false, true, false, false, false);
}

struct RestoreNodeScope(astersql_config::Config, String);
impl Drop for RestoreNodeScope {
    fn drop(&mut self) {
        astersql_config::store_global_config(self.0.clone());
        astersql_sessionctx_vardef::ServiceScope.Store(self.1.clone());
    }
}

#[test]
fn masking_policy_sql_modify_dist_reorg_counts_rows_once_for_multiple_indexes() {
    modify_dist_reorg(false, false, false, true, false, false);
}

#[test]
fn masking_policy_sql_modify_dist_reorg_tunes_actual_worker_lifetimes() {
    modify_dist_reorg(false, false, false, false, true, false);
}

fn modify_dist_reorg(
    partitioned: bool,
    pause: bool,
    named_scope: bool,
    multiple_indexes: bool,
    tune: bool,
    cloud: bool,
) {
    let _cloud = CloudSortConfiguration::new(cloud);
    let _dist = DistTaskConfiguration(astersql_sessionctx_vardef::EnableDistTask.Load());
    let _ingest = IngestEnvironment::new();
    let _scope = RestoreNodeScope(
        (*astersql_config::get_global_config()).clone(),
        astersql_sessionctx_vardef::ServiceScope.Load(),
    );
    let f = Fixture::new();
    if named_scope {
        f.pool
            .acquire()
            .unwrap()
            .query("SET GLOBAL tidb_service_scope='BACKGROUND'")
            .unwrap();
    }

    seed_policy(&f, 9606);
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    if partitioned {
        table.Partition = Some(astersql_meta_model::PartitionInfo {
            Enable: true,
            Type: astersql_meta_model::ast::PartitionType::Range,
            Expr: "id".into(),
            Num: 2,
            Definitions: vec![
                astersql_meta_model::PartitionDefinition {
                    ID: f.table + 100,
                    Name: astersql_meta_model::ast::NewCIStr("p0"),
                    LessThan: vec!["10".into()],
                    ..Default::default()
                },
                astersql_meta_model::PartitionDefinition {
                    ID: f.table + 101,
                    Name: astersql_meta_model::ast::NewCIStr("p1"),
                    LessThan: vec!["MAXVALUE".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }
    table.MaxIndexID += 1;
    table.Indices.push(astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID,
        Name: astersql_meta_model::ast::NewCIStr("payload_idx"),
        State: SchemaState::Public,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: table.Columns[1].Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    });
    if multiple_indexes {
        table.MaxIndexID += 1;
        let mut second = table.Indices[0].clone();
        second.ID = table.MaxIndexID;
        second.Name = astersql_meta_model::ast::NewCIStr("payload_idx2");
        table.Indices.push(second);
    }
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|s| s.Begin(&[]))
        .unwrap();
    let mut meta = astersql_meta::TransactionMutator::new(txn.as_mut());
    meta.update_table(f.db, &mut table).unwrap();
    let version = meta.gen_schema_version().unwrap();
    meta.set_table_schema_diff(
        &Job {
            tp: 12,
            schema_id: f.db,
            table_id: f.table,
            ..Default::default()
        },
        version,
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    f.domain.reload().unwrap();
    if partitioned {
        let ids: Vec<_> = table
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .map(|definition| definition.ID)
            .collect();
        let handle = f.domain.stats_handle();
        for id in &ids {
            astersql_statistics_handle::AnalyzeStatsStorage::register_table_stats(
                &mut *handle.lock().unwrap(),
                *id,
            )
            .unwrap();
        }
        f.domain.persist_stats_meta(&ids).unwrap();
    }
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30'),(12,'45.60')")
        .unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("SET GLOBAL tidb_enable_dist_task=ON")
        .unwrap();
    let _service = install_service(&f);
    if tune {
        use astersql_dxf_framework_taskexecutor::StepExecutor;
        let gate = BackfillRelease(Arc::new((
            std::sync::Mutex::new(false),
            std::sync::Condvar::new(),
        )));
        let callback_gate = gate.0.clone();
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (entered, ready) = std::sync::mpsc::channel();
        let _hook = astersql_testkit_testfailpoint::enable_value_call(
            "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
            move |job| {
                if !first.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    return;
                }
                let job = astersql_meta_model::group_3::Job::decode(job.as_bytes()).unwrap();
                entered.send(job.id).unwrap();
                let release = callback_gate.0.lock().unwrap();
                let _ = callback_gate
                    .1
                    .wait_timeout_while(release, std::time::Duration::from_secs(20), |released| {
                        !*released
                    })
                    .unwrap();
            },
        );
        let pool = f.pool.clone();
        let alter = std::thread::spawn(move || {
            pool.acquire()
                .unwrap()
                .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
        });
        let job_id = ready
            .recv_timeout(std::time::Duration::from_secs(15))
            .expect("real scanner must run before resource tuning");
        let step = super::modify_column_dist_backfill::read_index_for_test(job_id)
            .expect("actual DXF read executor");
        let context = astersql_dxf_framework_taskexecutor::Context::Background();
        let tuned = (|| {
            step.ResourceModified(
                &context,
                &astersql_dxf_framework_taskexecutor::StepResource {
                    CPU: 8,
                    Memory: 512 * 1024 * 1024,
                },
            )?;
            let before = step.closed_pipeline_workers_for_test();
            step.ResourceModified(
                &context,
                &astersql_dxf_framework_taskexecutor::StepResource {
                    CPU: 2,
                    Memory: 128 * 1024 * 1024,
                },
            )?;
            let after = step.closed_pipeline_workers_for_test();
            assert_eq!(
                (after.0 - before.0, after.1 - before.1),
                (3, 3),
                "Tune(wait=true) must close actual idle scan and ingest workers"
            );
            Ok::<(), astersql_dxf_framework_taskexecutor::ExecutorError>(())
        })();
        gate.release();
        alter.join().unwrap().unwrap();
        tuned.unwrap();
    } else if pause {
        let gate = BackfillRelease(Arc::new((
            std::sync::Mutex::new(false),
            std::sync::Condvar::new(),
        )));
        let callback_gate = gate.0.clone();
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (entered, ready) = std::sync::mpsc::channel();
        let _hook = astersql_testkit_testfailpoint::enable_value_call(
            "github.com/pingcap/tidb/pkg/ddl/beforeGetUserTableForBackfillStep",
            move |_| {
                if !first.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    return;
                }
                entered.send(()).unwrap();
                let release = callback_gate.0.lock().unwrap();
                let _ = callback_gate
                    .1
                    .wait_timeout_while(release, std::time::Duration::from_secs(15), |released| {
                        !*released
                    })
                    .unwrap();
            },
        );
        let pool = f.pool.clone();
        let alter = std::thread::spawn(move || {
            pool.acquire()
                .unwrap()
                .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
        });
        ready
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("background backfill executor must reach the real Go failpoint");
        let manager = super::ConcreteSession::new(f.domain.clone())
            .ImportTaskManager()
            .unwrap();
        let keys = f.pool.acquire().unwrap().query("SELECT task_key FROM mysql.tidb_global_task WHERE type='backfill' AND state='running'").unwrap();
        assert_eq!(keys.len(), 1);
        let key = keys[0][0].clone();
        assert!(manager.PauseTask((), key.clone()).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let task = manager.GetTaskByKey((), key.clone()).unwrap();
            if task.State == astersql_dxf_framework_storage::proto::TaskStatePaused {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "DXF pause did not persist: {}",
                task.State
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let subtasks = f
            .pool
            .acquire()
            .unwrap()
            .query("SELECT state FROM mysql.tidb_background_subtask")
            .unwrap();
        assert_eq!(subtasks, vec![vec!["paused".to_owned()]]);
        gate.release();
        assert!(manager.ResumeTask((), key).unwrap());
        alter.join().unwrap().unwrap();
    } else {
        f.pool
            .acquire()
            .unwrap()
            .query("ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DECIMAL(10,2)")
            .unwrap_or_else(|failure| {
                panic!(
                    "{failure}; history={:?}",
                    f.pool
                        .acquire()
                        .unwrap()
                        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
                )
            });
    }
    let new = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    assert_eq!(new.Indices.len(), if multiple_indexes { 2 } else { 1 });
    assert_eq!(new.Indices[0].Name.L, "payload_idx");
    assert_ne!(new.Indices[0].ID, table.Indices[0].ID);
    f.pool
        .acquire()
        .unwrap()
        .query("ADMIN CHECK TABLE test.normal_ddl_target")
        .unwrap();
    assert_eq!(f.pool.acquire().unwrap().query("SELECT payload FROM test.normal_ddl_target FORCE INDEX(payload_idx) WHERE payload=12.30").unwrap(),vec![vec!["12.30".to_string()]], "rows={:?}; subtasks={:?}", f.pool.acquire().unwrap().query("SELECT id,payload FROM test.normal_ddl_target"), f.pool.acquire().unwrap().query("SELECT meta,checkpoint FROM mysql.tidb_background_subtask"));
    let histories = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    let done = histories
        .iter()
        .filter_map(|row| astersql_meta::decode_go_history_job(row[0].as_bytes()).ok())
        .find(|job| job.tp == 12)
        .unwrap();
    let meta = done.reorg_meta.as_ref().unwrap();
    assert!(meta.IsFastReorg);
    assert_eq!(
        meta.ReorgTp,
        astersql_meta_model::group_3::ReorgType::ReorgTypeIngest
    );
    assert!(meta.IsDistReorg);
    assert_eq!(meta.UseCloudStorage, cloud);
    if cloud {
        let task = tasks_for_cloud(&f, done.id);
        let configured = astersql_sessionctx_vardef::CloudStorageURI.Load();
        let cluster_id = f
            .domain
            .storage_handle()
            .with_storage(|store| store.GetClusterID());
        let expected =
            astersql_ddl::index::resolve_cloud_storage_uri(&configured, false, || Some(cluster_id));
        assert_eq!(
            task["cloud_storage_uri"], expected,
            "cloud job must keep its actual URI in durable DXF metadata"
        );
        let steps = f
            .pool
            .acquire()
            .unwrap()
            .query("SELECT step FROM mysql.tidb_background_subtask ORDER BY step")
            .unwrap();
        assert!(
            steps.contains(&vec!["3".into()]),
            "cloud read output must be imported by the distinct write-and-ingest step: {steps:?}"
        );
        if astersql_testkit_testfailpoint::is_active(
            "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        ) {
            assert!(
                steps.contains(&vec!["2".into()]),
                "forced cloud merge must run a durable step: {steps:?}"
            );
        }
        assert!(
            task["summary"]["index_kv_size"]
                .as_u64()
                .is_some_and(|bytes| bytes > 0)
        );
    }
    if named_scope {
        assert_eq!(meta.TargetScope, "background");
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT role FROM mysql.dist_framework_meta")
                .unwrap(),
            vec![vec!["background".to_owned()]]
        );
    }

    let tasks = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT type,state,meta FROM mysql.tidb_global_task")
        .unwrap();
    assert!(tasks.iter().any(|row| {
        row[0] == "backfill"
            && row[1] == "succeed"
            && serde_json::from_str::<serde_json::Value>(&row[2])
                .is_ok_and(|value| value["job"]["id"] == done.id)
    }));
    let keys = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT task_key FROM mysql.tidb_global_task WHERE type='backfill'")
        .unwrap();
    assert!(keys.contains(&vec![format!("ddl/backfill/{}", done.id)]));
    assert!(keys.contains(&vec![format!("ddl/backfill/{}/merge", done.id)]));
    let merge_ranges = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT meta FROM mysql.tidb_background_subtask WHERE step=4")
        .unwrap();
    for row in merge_ranges {
        use base64::Engine;
        let meta: serde_json::Value = serde_json::from_str(&row[0]).unwrap();
        let start = base64::engine::general_purpose::STANDARD
            .decode(
                meta["row_start"]
                    .as_str()
                    .or(meta["start-key"].as_str())
                    .unwrap(),
            )
            .unwrap();
        let id = astersql_tablecodec::DecodeIndexID(astersql_tablecodec::kv::Key(start)).unwrap();
        assert!(
            new.Indices
                .iter()
                .any(|index| id == astersql_tablecodec::TempIndexPrefix | index.ID),
            "merge scans only a target temporary index: {id}"
        );
    }
    let subtasks = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT state,checkpoint FROM mysql.tidb_background_subtask")
        .unwrap();
    assert_eq!(
        subtasks.len(),
        (1 + new.Indices.len()) * if partitioned { 2 } else { 1 }
            + if cloud { new.Indices.len() } else { 0 }
            + if cloud
                && astersql_testkit_testfailpoint::is_active(
                    "github.com/pingcap/tidb/pkg/ddl/forceMergeSort"
                )
            {
                new.Indices.len()
            } else {
                0
            },
        "one real range per physical table per read/merge step"
    );
    assert!(
        subtasks
            .iter()
            .all(|row| row[0] == "succeed" && !row[1].is_empty())
    );
    let progress = f
        .pool
        .acquire()
        .unwrap()
        .query("SELECT checkpoint FROM mysql.tidb_background_subtask WHERE step=1")
        .unwrap();
    let rows: i64 = progress
        .iter()
        .map(|row| {
            serde_json::from_str::<serde_json::Value>(&row[0]).unwrap()["row_count"]
                .as_i64()
                .unwrap()
        })
        .sum();
    assert_eq!(rows, 2, "row progress counts each source row once");
    let args: serde_json::Value = serde_json::from_slice(&done.raw_args).unwrap();
    assert_eq!(
        args["new_index_ids"],
        serde_json::json!(new.Indices.iter().map(|index| index.ID).collect::<Vec<_>>())
    );
    let gc: Vec<_> = table
        .Indices
        .iter()
        .map(|index| index.ID)
        .chain(
            new.Indices
                .iter()
                .map(|index| index.ID | astersql_tablecodec::TempIndexPrefix),
        )
        .collect();
    assert_eq!(args["index_ids"], serde_json::json!(gc));
}

struct DistTaskConfiguration(bool);
impl Drop for DistTaskConfiguration {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::EnableDistTask.Store(self.0);
    }
}

#[test]
fn modify_column_temporary_unique_delete_removes_orphaned_handle() {
    let f = Fixture::new();
    f.pool
        .acquire()
        .unwrap()
        .query("CREATE UNIQUE INDEX payload_idx ON test.normal_ddl_target(payload)")
        .unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'12.30')")
        .unwrap();
    let table = f.domain.table_by_name("test", "normal_ddl_target").unwrap();
    let index = table
        .Indices
        .iter()
        .find(|index| index.Name.L == "payload_idx")
        .unwrap();
    let snapshot = f
        .domain
        .storage_handle()
        .with_storage(|store| {
            let version = store.CurrentVersion("global")?;
            Ok::<_, astersql_kv::Error>(store.GetSnapshot(version))
        })
        .unwrap();
    let prefix = astersql_tablecodec::EncodeTableIndexPrefix(f.table, index.ID).0;
    let mut values = snapshot
        .Iter(
            astersql_kv::Key(prefix.clone()),
            Some(astersql_kv::Key(prefix).PrefixNext()),
        )
        .unwrap();
    assert!(values.Valid());
    let original = values.Key();
    values.Close();
    let mut temporary = original.0.clone();
    astersql_tablecodec::IndexKey2TempIndexKey(&mut temporary);
    let deletion = astersql_tablecodec::TempIndexValueElem {
        Value: Vec::new(),
        Handle: Box::new(astersql_tablecodec::kv::IntHandle(2)),
        KeyVer: astersql_tablecodec::TempIndexKeyTypeBackfill,
        Delete: true,
        Distinct: true,
        Global: false,
    }
    .Encode(None);
    let row = astersql_tablecodec::EncodeRowKeyWithHandle(
        f.table,
        Box::new(astersql_tablecodec::kv::IntHandle(1)),
    );
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    txn.Delete(astersql_kv::Key(row.0)).unwrap();
    txn.Set(astersql_kv::Key(temporary.clone()), deletion)
        .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    let mut session = super::ConcreteSession::new(f.domain.clone());
    session.execute("BEGIN").unwrap();
    super::modify_column_backfill::merge(
        &mut session,
        astersql_ddl::backfilling::IndexBackfillBatch {
            schema_id: f.db,
            table_id: f.table,
            index_ids: vec![index.ID],
            task: astersql_ddl::backfilling::ReorgBackfillTask {
                physical_table_id: f.table,
                start_key: temporary.clone(),
                end_key: astersql_kv::Key(temporary.clone()).PrefixNext().0,
                ..Default::default()
            },
            batch_size: 256,
            resource_group: String::new(),
            sql_mode: 0,
        },
    )
    .unwrap();
    session.execute("COMMIT").unwrap();
    let snapshot = f
        .domain
        .storage_handle()
        .with_storage(|store| {
            let version = store.CurrentVersion("global")?;
            Ok::<_, astersql_kv::Error>(store.GetSnapshot(version))
        })
        .unwrap();
    let result = snapshot.Get(&astersql_kv::Context::default(), original, &[]);
    assert!(
        result.as_ref().is_err_and(astersql_kv::IsErrNotFound),
        "orphaned unique index was retained: {result:?}"
    );
}

#[test]
fn masking_policy_sql_modify_timestamp_uses_captured_timezone() {
    modify_timestamp_timezone(false, "+05:45", "2026-01-02 08:49:05");
}

#[test]
fn masking_policy_sql_modify_datetime_to_timestamp_converts_local_calendar() {
    modify_timestamp_timezone(true, "+05:45", "2026-01-01 21:19:05");
}

#[test]
fn masking_policy_sql_modify_timestamp_uses_named_timezone() {
    modify_timestamp_timezone(false, "America/New_York", "2026-01-01 22:04:05");
}

fn modify_timestamp_timezone(reverse: bool, timezone: &str, expected: &str) {
    let f = Fixture::new();
    let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    table.Columns[1].SetType(if reverse {
        astersql_parser_mysql::r#type::TypeDatetime
    } else {
        astersql_parser_mysql::r#type::TypeTimestamp
    });
    table.Columns[1].SetFlen(19);
    table.Columns[1].SetDecimal(0);
    table.Columns[1].SetCharset("binary".into());
    table.Columns[1].SetCollate("binary".into());
    let mut txn = f
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .unwrap();
    let mut meta = astersql_meta::TransactionMutator::new(txn.as_mut());
    meta.update_table(f.db, &mut table).unwrap();
    let version = meta.gen_schema_version().unwrap();
    meta.set_table_schema_diff(
        &Job {
            tp: 12,
            schema_id: f.db,
            table_id: f.table,
            ..Default::default()
        },
        version,
    )
    .unwrap();
    txn.Commit(&astersql_kv::Context::default()).unwrap();
    f.domain.reload().unwrap();
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (1,'2026-01-02 03:04:05')")
        .unwrap();
    let _service = install_service(&f);
    let session = f.pool.acquire().unwrap();
    session
        .query(format!("SET SESSION time_zone='{timezone}'"))
        .unwrap();
    session
        .query(if reverse {
            "ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload TIMESTAMP NULL"
        } else {
            "ALTER TABLE test.normal_ddl_target MODIFY COLUMN payload DATETIME"
        })
        .unwrap();
    session.query("SET SESSION time_zone='+00:00'").unwrap();
    assert_eq!(
        session
            .query("SELECT payload FROM test.normal_ddl_target")
            .unwrap(),
        vec![vec![expected.to_owned()]]
    );
}

#[test]
fn masking_policy_modify_dxf_row_count_queries_active_and_history_summaries() {
    let f = Fixture::new();
    let manager = super::ConcreteSession::new(f.domain.clone())
        .ImportTaskManager()
        .unwrap();
    assert_eq!(manager.GetSubtaskRowCount((), 960600, 1).unwrap(), 0);
    f.pool.acquire().unwrap().query("INSERT INTO mysql.tidb_background_subtask (task_key,step,state,checkpoint,summary) VALUES (960600,1,'succeed','{}','{\"row_count\":2}')").unwrap();
    f.pool.acquire().unwrap().query("INSERT INTO mysql.tidb_background_subtask_history (task_key,step,state,checkpoint,summary) VALUES (960600,1,'succeed','{}','{\"row_count\":3}')").unwrap();
    assert_eq!(manager.GetSubtaskRowCount((), 960600, 1).unwrap(), 5);
    assert_eq!(manager.GetSubtaskRowCount((), 960600, 4).unwrap(), 0);
}

#[test]
fn masking_policy_serving_factory_installs_durable_modify_column_path() {
    serving_factory_modify(false);
}

#[test]
fn masking_policy_serving_factory_backfills_named_keyspace() {
    serving_factory_modify(true);
}

fn serving_factory_modify(named: bool) {
    let _dist = DistTaskConfiguration(astersql_sessionctx_vardef::EnableDistTask.Load());
    if named {
        astersql_sessionctx_vardef::EnableDistTask.Store(true);
    }
    let _ingest = named.then(IngestEnvironment::new);
    let store = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            named.then(|| astersql_store_mockstore_mockstorage::KeyspaceMeta {
                Name: "factory-user".into(),
                Id: 9606,
            }),
        )
        .unwrap(),
    )
    .ok()
    .unwrap();
    let factory = super::CanonicalSessionFactory::from_storage_for_test(store).unwrap();
    assert!(
        factory
            .domain()
            .ddl()
            .is_some_and(|ddl| ddl.supports_persistent_actions()),
        "the serving factory must assemble the actual durable DDL service"
    );
    let session = factory.create_session();
    if named {
        session
            .execute("SET GLOBAL tidb_enable_dist_task=ON")
            .unwrap();
    }
    session
        .execute(
            "CREATE TABLE test.factory_modify_target (id INT PRIMARY KEY, payload VARCHAR(40))",
        )
        .unwrap();
    if named {
        session
            .execute("CREATE INDEX payload_idx ON test.factory_modify_target(payload)")
            .unwrap();
    }
    if named {
        let table = factory
            .domain()
            .table_by_name("test", "factory_modify_target")
            .unwrap();
        let db = factory
            .domain()
            .info_schema()
            .AllSchemas()
            .into_iter()
            .find(|db| db.name.lower == "test")
            .unwrap()
            .id;
        let mut txn = factory
            .domain()
            .storage_handle()
            .with_storage(|store| store.Begin(&[]))
            .unwrap();
        let persisted = astersql_meta::TransactionMutator::new(txn.as_mut())
            .get_table(db, table.ID)
            .unwrap()
            .unwrap();
        txn.Rollback().unwrap();
        assert!(
            persisted.MaxIndexID >= persisted.Indices[0].ID,
            "CREATE INDEX must persist the table-local index ID high-water mark"
        );
        assert_eq!(
            persisted.Indices.len(),
            1,
            "CREATE INDEX persists the source metadata consumed by MODIFY"
        );
    }
    session
        .execute("INSERT INTO test.factory_modify_target VALUES (1,'12.30')")
        .unwrap();
    let alter_domain = factory.domain().clone();
    let (send, receive) = std::sync::mpsc::channel();
    let alter = std::thread::spawn(move || {
        let alter_session = super::ConcreteSession::new(alter_domain);
        let result = alter_session
            .execute("ALTER TABLE test.factory_modify_target MODIFY COLUMN payload DECIMAL(10,2)");
        send.send(result.map(|_| ())).ok();
    });
    let outcome = receive.recv_timeout(std::time::Duration::from_secs(30));
    if outcome.is_err() {
        let tasks = session
            .execute("SELECT state,error FROM mysql.tidb_global_task")
            .map(|sets| sets.into_iter().map(|set| set.rows).collect::<Vec<_>>());
        factory.domain().close();
        alter.join().unwrap();
        panic!("MODIFY did not complete; tasks={tasks:?}");
    }
    alter.join().unwrap();
    outcome.unwrap().unwrap();
    let rows = session
        .execute("SELECT payload FROM test.factory_modify_target")
        .unwrap();
    assert_eq!(rows[0].rows, vec![vec!["12.30".to_owned()]]);
    let history = session
        .execute("SELECT job_meta FROM mysql.tidb_ddl_history")
        .unwrap();
    assert!(
        history[0]
            .rows
            .iter()
            .filter_map(|row| Job::decode(row[0].as_bytes()).ok())
            .any(|job| job.tp == 12 && job.state == JobState::Synced)
    );
    if named {
        let modified = history[0]
            .rows
            .iter()
            .filter_map(|row| Job::decode(row[0].as_bytes()).ok())
            .find(|job| job.tp == 12)
            .unwrap();
        assert_eq!(
            modified.reorg_meta.as_ref().unwrap().ReorgTp,
            astersql_meta_model::group_3::ReorgType::ReorgTypeIngest,
            "history={}",
            history[0]
                .rows
                .iter()
                .find(|row| row[0].contains("modify"))
                .map(|row| row[0].as_str())
                .unwrap_or("")
        );
        assert!(
            history[0]
                .rows
                .iter()
                .filter_map(|row| Job::decode(row[0].as_bytes()).ok())
                .any(|job| job.tp == 12
                    && job.reorg_meta.as_ref().is_some_and(|meta| meta.IsDistReorg))
        );
        assert!(
            !session
                .execute("SELECT task_key FROM mysql.tidb_global_task WHERE type='backfill'")
                .unwrap()[0]
                .rows
                .is_empty(),
            "history={:?}; tasks={:?}; subtasks={:?}",
            history[0].rows,
            session
                .execute("SELECT type,state,task_key FROM mysql.tidb_global_task")
                .map(|sets| sets.into_iter().map(|set| set.rows).collect::<Vec<_>>()),
            session
                .execute("SELECT state,meta FROM mysql.tidb_background_subtask")
                .map(|sets| sets.into_iter().map(|set| set.rows).collect::<Vec<_>>())
        );
    }
    factory.domain().close();
}

#[test]
fn masking_policy_dxf_foreign_runtime_checks_store_and_releases_holders() {
    use super::modify_column_dist_backfill::TaskRuntimeBinding;
    use super::session_factory::{KeyspaceSessionFactory, TargetSessionStore, TargetTransport};
    use astersql_dxf_framework_taskexecutor::{Task, TaskBase, TaskRuntime};
    let source = bootstrapped_keyspace_domain("SYSTEM", 96060);
    let target = bootstrapped_keyspace_domain("runtime-user", 96061);
    let storage = target.storage_handle();
    let server = Arc::new(astersql_domain_serverinfo::MemoryEtcdClient::default());
    let schema = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    Arc::new(KeyspaceSessionFactory::with_openers(
        Arc::new(move |keyspace| {
            Ok(TargetSessionStore::new(
                storage.clone(),
                keyspace.into(),
                false,
            ))
        }),
        Arc::new(move |_| {
            Ok(TargetTransport {
                server: server.clone(),
                schema: schema.clone(),
            })
        }),
    ))
    .install_on_domain(&source, "SYSTEM".into())
    .unwrap();
    let manager = source.cross_ks_manager().unwrap();
    let mut task = Task {
        TaskBase: TaskBase {
            ID: 960600,
            Keyspace: "runtime-user".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let binding = TaskRuntimeBinding::acquire(source.clone(), &task).unwrap();
    binding.CheckTaskKeyspace("runtime-user").unwrap();
    assert!(binding.CheckTaskKeyspace("other").is_err());
    assert!(manager.has_active_holder("runtime-user", "DXF/executor/960600"));
    assert!(TaskRuntimeBinding::acquire(source.clone(), &task).is_err());
    assert_eq!(manager.active_holder_len("runtime-user"), Some(1));
    drop(binding);
    assert_eq!(manager.active_holder_len("runtime-user"), Some(0));
    task.TaskBase.Keyspace = "wrong-store".into();
    assert!(TaskRuntimeBinding::acquire(source.clone(), &task).is_err());
    assert_eq!(manager.active_holder_len("wrong-store"), Some(0));
    task.TaskBase.Keyspace = "runtime-user".into();
    let binding = TaskRuntimeBinding::acquire(source.clone(), &task).unwrap();
    binding.Release();
    binding.Release();
    assert_eq!(manager.active_holder_len("runtime-user"), Some(0));
    assert!(binding.CheckTaskKeyspace("runtime-user").is_err());
    drop(binding);
    let binding = TaskRuntimeBinding::acquire(source.clone(), &task).unwrap();
    manager.close();
    assert!(binding.CheckTaskKeyspace("runtime-user").is_err());
    drop(binding);
    source.close();
    target.close();
}

fn bootstrapped_keyspace_domain(name: &str, id: u32) -> Arc<astersql_domain::Domain> {
    let store = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            Some(astersql_store_mockstore_mockstorage::KeyspaceMeta {
                Name: name.into(),
                Id: id,
            }),
        )
        .unwrap(),
    )
    .ok()
    .unwrap();
    let mut config = astersql_domain::DomainConfig::default();
    config.keyspace = name.into();
    let domain = Arc::new(astersql_domain::Domain::new(
        store,
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        config,
    ));
    domain.init().unwrap();
    domain
        .ddl_create_database_with_id("mysql", true, Some(astersql_meta_metadef::SystemDatabaseID))
        .unwrap();
    super::BootstrapCanonicalDomain(domain.clone()).unwrap();
    domain
}

#[cfg(feature = "nextgen")]
#[test]
fn masking_policy_nextgen_system_executes_user_keyspace_modify() {
    use super::session_factory::{KeyspaceSessionFactory, TargetSessionStore, TargetTransport};
    let _scope = RestoreNodeScope(
        (*astersql_config::get_global_config()).clone(),
        astersql_sessionctx_vardef::ServiceScope.Load(),
    );
    let _dist = DistTaskConfiguration(astersql_sessionctx_vardef::EnableDistTask.Load());
    let _ingest = IngestEnvironment::new();
    let system = bootstrapped_keyspace_domain("SYSTEM", 96063);
    let user = bootstrapped_keyspace_domain("dxf-user", 96064);
    fn connect(
        source: &Arc<astersql_domain::Domain>,
        target: &Arc<astersql_domain::Domain>,
        current: &str,
        server: Arc<astersql_domain_serverinfo::MemoryEtcdClient>,
        schema: Arc<astersql_ddl_schemaver::MemoryEtcdClient>,
    ) {
        let storage = target.storage_handle();
        Arc::new(KeyspaceSessionFactory::with_openers(
            Arc::new(move |name| Ok(TargetSessionStore::new(storage.clone(), name.into(), false))),
            Arc::new(move |_| {
                Ok(TargetTransport {
                    server: server.clone(),
                    schema: schema.clone(),
                })
            }),
        ))
        .install_on_domain(source, current.into())
        .unwrap();
    }
    let user_server = Arc::new(astersql_domain_serverinfo::MemoryEtcdClient::default());
    let user_schema = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    let system_server = Arc::new(astersql_domain_serverinfo::MemoryEtcdClient::default());
    let system_schema = Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default());
    system
        .install_server_info_syncer("nextgen-system-node".to_owned(), system_server.clone(), &[])
        .unwrap();
    user.install_server_info_syncer("nextgen-user-owner".to_owned(), user_server.clone(), &[])
        .unwrap();
    connect(&system, &user, "SYSTEM", user_server, user_schema.clone());
    connect(&user, &system, "dxf-user", system_server, system_schema);
    let session = super::ConcreteSession::new(user.clone());
    session
        .execute("SET GLOBAL tidb_enable_dist_task=ON")
        .unwrap();
    session
        .execute("CREATE TABLE test.nextgen_modify (id INT PRIMARY KEY, payload VARCHAR(40))")
        .unwrap();
    session
        .execute("CREATE INDEX payload_idx ON test.nextgen_modify(payload)")
        .unwrap();
    session
        .execute("INSERT INTO test.nextgen_modify VALUES (1,'12.30'),(2,'45.60')")
        .unwrap();
    astersql_sessionctx_vardef::ServiceScope.Store("dxf_service");
    let worker = super::modify_column_dist_backfill::NodeService::start(
        &mut super::ConcreteSession::new(system.clone()),
    )
    .unwrap();
    let local_nodes = super::ConcreteSession::new(system.clone())
        .ImportTaskManager()
        .unwrap()
        .GetAllNodes(())
        .unwrap();
    assert_eq!(local_nodes.len(), 1, "SYSTEM executor registration");
    let _user_worker = super::modify_column_dist_backfill::NodeService::start(
        &mut super::ConcreteSession::new(user.clone()),
    )
    .unwrap();
    let remote_nodes = astersql_dxf_framework_storage::GetDXFSvcTaskMgr()
        .unwrap()
        .GetAllNodes(())
        .unwrap();
    assert_eq!(
        remote_nodes.len(),
        1,
        "SYSTEM task access must observe executor registration: local={local_nodes:?}"
    );
    let cancel = astersql_owner::Context::new();
    let owner = astersql_owner::NewMockManager(
        cancel.clone(),
        "nextgen-user-owner",
        None,
        "/nextgen-user-owner",
    );
    super::session_factory::install_serving_ddl_runtime(
        &user,
        owner,
        Arc::new(tokio::runtime::Runtime::new().unwrap()),
        cancel,
        user_schema.clone(),
        "nextgen-user-owner",
        std::time::Duration::from_millis(50),
    )
    .unwrap();
    let alter_domain = user.clone();
    let (send, receive) = std::sync::mpsc::channel();
    let alter = std::thread::spawn(move || {
        let session = super::ConcreteSession::new(alter_domain);
        let result =
            session.execute("ALTER TABLE test.nextgen_modify MODIFY COLUMN payload DECIMAL(10,2)");
        send.send(result.map(|_| ())).ok();
    });
    let outcome = receive.recv_timeout(std::time::Duration::from_secs(40));
    if outcome.is_err() {
        let tasks = super::ConcreteSession::new(system.clone())
            .execute("SELECT state,error FROM mysql.tidb_global_task")
            .map(|sets| sets.into_iter().map(|set| set.rows).collect::<Vec<_>>());
        let queue = super::system_session::SystemSessionPool::new(user.clone())
            .acquire()
            .unwrap()
            .query("SELECT job_id,job_meta FROM mysql.tidb_ddl_job")
            .map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        let job = astersql_meta_model::group_3::Job::decode(row[1].as_bytes());
                        match job {
                            Ok(job) => format!(
                                "id={} state={:?} schema={:?} last={} stage={:?}",
                                job.id,
                                job.state,
                                job.schema_state,
                                job.last_schema_version,
                                job.reorg_meta.as_ref().map(|meta| meta.Stage)
                            ),
                            Err(e) => format!(
                                "{e:?}; raw={}",
                                row[1].chars().take(180).collect::<String>()
                            ),
                        }
                    })
                    .collect::<Vec<_>>()
            });
        let versions = astersql_ddl_schemaver::EtcdClient::Get(
            user_schema.as_ref(),
            &astersql_ddl_schemaver::Context::Background(),
            "/tidb/ddl/",
            true,
        )
        .map(|response| {
            response
                .Kvs
                .into_iter()
                .map(|kv| (kv.Key, kv.Value))
                .collect::<Vec<_>>()
        });
        user.close();
        worker.stop();
        system.close();
        alter.join().unwrap();
        panic!(
            "NextGen MODIFY did not complete: tasks={tasks:?}; queue={queue:?}; versions={versions:?}"
        );
    }
    alter.join().unwrap();
    outcome.unwrap().unwrap();
    session
        .execute("ADMIN CHECK TABLE test.nextgen_modify")
        .unwrap();
    assert_eq!(session.execute("SELECT payload FROM test.nextgen_modify FORCE INDEX(payload_idx) WHERE payload=12.30").unwrap()[0].rows,vec![vec!["12.30".to_owned()]]);
    let sys_session = super::ConcreteSession::new(system.clone());
    let tasks = sys_session
        .execute("SELECT task_key,keyspace,state FROM mysql.tidb_global_task WHERE type='backfill'")
        .unwrap();
    assert_eq!(tasks[0].rows.len(), 2);
    assert!(
        tasks[0]
            .rows
            .iter()
            .all(|row| row[0].starts_with("dxf-user/ddl/backfill/")
                && row[1] == "dxf-user"
                && row[2] == "succeed")
    );
    assert!(
        session
            .execute("SELECT task_key FROM mysql.tidb_global_task")
            .unwrap()[0]
            .rows
            .is_empty()
    );
    assert_eq!(
        system
            .cross_ks_manager()
            .unwrap()
            .active_holder_len("dxf-user"),
        Some(0)
    );
    let nodes = sys_session
        .execute("SELECT role FROM mysql.dist_framework_meta")
        .unwrap();
    assert_eq!(nodes[0].rows, vec![vec!["dxf_service".to_owned()]]);
    user.close();
    worker.stop();
    system.close();
}

#[test]
fn masking_policy_sql_modify_cloud_reorg_uses_durable_object_stages() {
    modify_dist_reorg(false, false, false, false, false, true);
}
#[test]
fn masking_policy_sql_modify_cloud_reorg_runs_forced_merge_stage() {
    let _merge = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/forceMergeSort",
        "return()",
    );
    modify_dist_reorg(false, false, false, false, false, true);
}
fn tasks_for_cloud(f: &Fixture, job: i64) -> serde_json::Value {
    let rows = f
        .pool
        .acquire()
        .unwrap()
        .query(format!(
            "SELECT meta FROM mysql.tidb_global_task WHERE task_key='ddl/backfill/{job}'"
        ))
        .unwrap();
    assert_eq!(rows.len(), 1);
    serde_json::from_str(&rows[0][0]).unwrap()
}
struct CloudSortConfiguration {
    previous: String,
    directory: Option<std::path::PathBuf>,
}
impl CloudSortConfiguration {
    fn new(enabled: bool) -> Self {
        let previous = astersql_sessionctx_vardef::CloudStorageURI.Load();
        let directory = enabled.then(|| {
            std::env::temp_dir().join(format!("task6-native-cloud-{}", uuid::Uuid::new_v4()))
        });
        if let Some(path) = &directory {
            std::fs::create_dir(path).unwrap();
            astersql_sessionctx_vardef::CloudStorageURI
                .Store(format!("local://{}", path.display()));
        } else {
            astersql_sessionctx_vardef::CloudStorageURI.Store("");
        }
        Self {
            previous,
            directory,
        }
    }
}
impl Drop for CloudSortConfiguration {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::CloudStorageURI.Store(self.previous.clone());
        if let Some(path) = &self.directory {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}

#[test]
fn masking_policy_restart_preserves_renamed_system_table() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    for concurrent_upgrade in [false, true] {
        let f = Fixture::new();
        seed_policy(&f, 9606);
        f.pool
            .acquire()
            .unwrap()
            .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.tidb_masking_policy_bak")
            .unwrap();
        let backup = f
            .domain
            .table_by_name("mysql", "tidb_masking_policy_bak")
            .unwrap();
        let current = unsafe { crate::upgrade_def::currentBootstrapVersion };
        if concurrent_upgrade {
            f.pool.acquire().unwrap().query("UPDATE mysql.tidb SET variable_value='189' WHERE variable_name='tidb_server_version'").unwrap();
        }
        let lock = super::session::acquire_bootstrap_upgrade_lock(&f.domain, || {
            f.pool.acquire().unwrap().query(format!("UPDATE mysql.tidb SET variable_value='{current}' WHERE variable_name='tidb_server_version'")).unwrap();
            Ok(())
        }).unwrap();
        assert_eq!(lock.is_some(), concurrent_upgrade);
        super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
        assert!(
            f.domain
                .table_by_name("mysql", "tidb_masking_policy")
                .is_err()
        );
        assert_eq!(
            f.domain
                .table_by_name("mysql", "tidb_masking_policy_bak")
                .unwrap()
                .ID,
            backup.ID
        );
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT policy_id FROM mysql.tidb_masking_policy_bak")
                .unwrap(),
            vec![vec!["9606".to_string()]]
        );
    }
}

#[test]
fn masking_policy_dependent_table_initialization_is_versioned_and_idempotent() {
    let f = Fixture::new();
    seed_policy(&f, 9606);
    let original = f
        .domain
        .table_by_name("mysql", "tidb_masking_policy")
        .unwrap();
    let handle = f.domain.storage_handle();
    let read_id = || {
        let txn = handle.with_storage(|s| s.Begin(&[])).unwrap();
        txn.Get(
            &astersql_kv::Context::default(),
            astersql_meta::transaction_meta_string_key(b"NextGlobalID"),
            &[],
        )
        .unwrap()
        .Value
    };
    let before = read_id();
    for version in [Some(189), Some(254), Some(189)] {
        super::session::init_bootstrap_dependent_tables(&f.domain, version).unwrap();
    }
    assert_eq!(read_id(), before);
    assert_eq!(
        f.domain
            .table_by_name("mysql", "tidb_masking_policy")
            .unwrap()
            .ID,
        original.ID
    );
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT policy_id FROM mysql.tidb_masking_policy")
            .unwrap(),
        vec![vec!["9606".to_string()]]
    );
    f.pool
        .acquire()
        .unwrap()
        .query("RENAME TABLE mysql.tidb_masking_policy TO mysql.tidb_masking_policy_bak")
        .unwrap();
    for version in [None, Some(0), Some(280), Some(281)] {
        super::session::init_bootstrap_dependent_tables(&f.domain, version).unwrap();
        assert!(
            f.domain
                .table_by_name("mysql", "tidb_masking_policy")
                .is_err()
        );
        assert_eq!(read_id(), before);
    }
    super::session::init_bootstrap_dependent_tables(&f.domain, Some(189)).unwrap();
    if astersql_config_kerneltype::IsNextGen() {
        assert!(
            f.domain
                .table_by_name("mysql", "tidb_masking_policy")
                .is_err()
        );
    } else {
        let created = f
            .domain
            .table_by_name("mysql", "tidb_masking_policy")
            .unwrap();
        assert!(!astersql_meta_metadef::IsReservedID(created.ID));
        assert_ne!(created.ID, original.ID);
        assert_eq!(created.Columns.len(), 14);
        let after = read_id();
        super::session::init_bootstrap_dependent_tables(&f.domain, Some(189)).unwrap();
        assert_eq!(read_id(), after);
    }
}

#[test]
fn bootstrap_reserved_versions_preserve_compatibility_variables() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    for (version, expected) in [(278, "0.8"), (280, "0.8"), (281, "0")] {
        let f = Fixture::new();
        let mut sql = f.pool.acquire().unwrap();
        sql.query(format!("UPDATE mysql.tidb SET variable_value='{version}' WHERE variable_name='tidb_server_version'")).unwrap();
        sql.query("DELETE FROM mysql.global_variables WHERE variable_name='tidb_default_string_match_selectivity'").unwrap();
        super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
        // The historical value must be materialized before bootstrap's current
        // sysvar default, while a cluster already at 281 keeps that default.
        assert_eq!(sql.query("SELECT variable_value FROM mysql.global_variables WHERE variable_name='tidb_default_string_match_selectivity'").unwrap(), vec![vec![expected.to_string()]]);
        sql.query("UPDATE mysql.global_variables SET variable_value='0.6' WHERE variable_name='tidb_default_string_match_selectivity'").unwrap();
        sql.query(format!("UPDATE mysql.tidb SET variable_value='{version}' WHERE variable_name='tidb_server_version'")).unwrap();
        super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
        assert_eq!(sql.query("SELECT variable_value FROM mysql.global_variables WHERE variable_name='tidb_default_string_match_selectivity'").unwrap(), vec![vec!["0.6".to_string()]]);
    }
}

#[test]
fn bootstrap_binding_digest_refresh_runs_until_version282() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    for version in [281, 282] {
        let f = Fixture::new();
        let mut sql = f.pool.acquire().unwrap();
        sql.query(format!("UPDATE mysql.tidb SET variable_value='{version}' WHERE variable_name='tidb_server_version'")).unwrap();
        sql.query("INSERT INTO mysql.bind_info (original_sql,bind_sql,default_db,status,create_time,update_time,charset,collation,source,sql_digest,plan_digest) VALUES ('old','select * from test.normal_ddl_target where ((id = 1))','test','enabled','2026-01-01 00:00:00','2026-01-01 00:00:00','utf8mb4','utf8mb4_bin','manual','old-digest','plan-digest')").unwrap();
        super::BootstrapCanonicalDomain(f.domain.clone()).unwrap();
        let rows = sql
            .query("SELECT sql_digest,plan_digest FROM mysql.bind_info WHERE source='manual'")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "plan-digest");
        if version < 282 {
            assert_ne!(rows[0][0], "old-digest");
            assert!(!rows[0][0].is_empty());
        } else {
            assert_eq!(rows[0][0], "old-digest");
        }
    }
}
