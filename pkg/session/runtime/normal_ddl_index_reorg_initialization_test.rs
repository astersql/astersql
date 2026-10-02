// Copyright 2026 AsterSQL.

use super::normal_ddl_fixture::{Fixture, hex};
use astersql_ddl::job_worker::{JobLease, JobWorker, WorkerType};
use astersql_meta_model::group_3::{DDLReorgMeta, Job, JobState, ReorgType};
use std::sync::Mutex;

static CONFIG: Mutex<()> = Mutex::new(());
const DISK_FAULT: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/mockIngestCheckEnvFailed";
struct Lease;
impl JobLease for Lease {
    fn owner_epoch(&self) -> u64 {
        1
    }
    fn is_owner(&self) -> bool {
        true
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
struct Restore(String, Option<std::path::PathBuf>);
impl Drop for Restore {
    fn drop(&mut self) {
        astersql_sessionctx_vardef::CloudStorageURI.Store(self.0.clone());
        fail::remove(DISK_FAULT);
        astersql_ddl::index::replace_global_lightning_env_for_test(self.1.clone());
    }
}

fn setup(f: &Fixture, id: i64, dist: bool) -> (Job, Vec<u8>, astersql_meta_model::IndexInfo) {
    f.pool
        .acquire()
        .unwrap()
        .query("INSERT INTO test.normal_ddl_target VALUES (17,'retained row')")
        .unwrap();
    let table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
    let column = table
        .Columns
        .iter()
        .find(|c| c.Name.L == "payload")
        .unwrap();
    let index = astersql_meta_model::IndexInfo {
        ID: table.MaxIndexID + 1,
        Name: astersql_meta_model::ast::NewCIStr("idx_payload"),
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: column.Name.clone(),
            Offset: column.Offset,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut job = Job {
        id,
        tp: 7,
        schema_id: f.db,
        table_id: f.table,
        state: JobState::Running,
        reorg_meta: Some(DDLReorgMeta {
            IsFastReorg: true,
            IsDistReorg: dist,
            ..Default::default()
        }),
        ..Default::default()
    };
    let bytes = job.encode(false).unwrap();
    f.pool.acquire().unwrap().query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({id},1,'{}','{}',X'{}',7,1)",f.db,f.table,hex(&bytes))).unwrap();
    (job, bytes, index)
}

#[test]
fn persisted_worker_cloud_skips_real_disk_fault() {
    let _serial = CONFIG.lock().unwrap();
    let old_root = astersql_ddl::index::replace_global_lightning_env_for_test(Some(
        std::env::temp_dir().join("aster-d0dfde35b7-ingest"),
    ));
    let _restore = Restore(astersql_sessionctx_vardef::CloudStorageURI.Load(), old_root);
    fail::cfg(DISK_FAULT, "return(true)").unwrap();
    astersql_sessionctx_vardef::CloudStorageURI.Store("s3://bucket");
    let loaded = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = loaded.clone();
    let _loaded_hook = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/afterLoadCloudStorageURI",
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );
    let f = Fixture::new();
    let (mut job, bytes, index) = setup(&f, 89201, true);
    let mut session = f.pool.acquire().unwrap();
    let indexes = JobWorker::new(WorkerType::AddIndex)
        .initialize_persisted_index_reorg(&mut session, &Lease, &mut job, &bytes, vec![index])
        .unwrap();
    assert_eq!(
        session.index_cloud_storage_uri_for_test(job.id).as_deref(),
        Some("s3://bucket/dxf/")
    );
    assert_eq!(loaded.load(std::sync::atomic::Ordering::SeqCst), 1);
    let meta = job.reorg_meta.as_ref().unwrap();
    assert!(meta.UseCloudStorage);
    assert_eq!(meta.ReorgTp, ReorgType::ReorgTypeIngest);
    assert_eq!(
        indexes[0].BackfillState,
        astersql_meta_model::BackfillStateRunning
    );
    let persisted = f.queue(job.id).unwrap();
    let meta = persisted.reorg_meta.as_ref().unwrap();
    assert!(meta.UseCloudStorage);
    assert_eq!(meta.ReorgTp, ReorgType::ReorgTypeIngest);
    assert_eq!(persisted.state, JobState::Running);
    assert_eq!(
        f.pool
            .acquire()
            .unwrap()
            .query("SELECT payload FROM test.normal_ddl_target WHERE id=17")
            .unwrap(),
        vec![vec!["retained row".to_owned()]]
    );
}

#[test]
fn persisted_local_disk_error_preserves_queue() {
    let _serial = CONFIG.lock().unwrap();
    let old_root = astersql_ddl::index::replace_global_lightning_env_for_test(Some(
        std::env::temp_dir().join("aster-d0dfde35b7-ingest"),
    ));
    let _restore = Restore(astersql_sessionctx_vardef::CloudStorageURI.Load(), old_root);
    fail::cfg(DISK_FAULT, "return(true)").unwrap();
    astersql_sessionctx_vardef::CloudStorageURI.Store("s3://bucket");
    let f = Fixture::new();
    let (mut job, bytes, index) = setup(&f, 89202, false);
    let mut session = f.pool.acquire().unwrap();
    let error = JobWorker::new(WorkerType::AddIndex)
        .initialize_persisted_index_reorg(&mut session, &Lease, &mut job, &bytes, vec![index])
        .unwrap_err();
    assert!(error.contains("mock error"), "{error}");
    assert_eq!(
        session.index_cloud_storage_uri_for_test(job.id).as_deref(),
        Some("s3://bucket/dxf/")
    );
    assert_eq!(job.encode(false).unwrap(), bytes);
    assert_eq!(f.queue(job.id).unwrap().encode(false).unwrap(), bytes);
}

#[test]
fn initialization_fences_replacement_owner() {
    let _serial = CONFIG.lock().unwrap();
    let old_root = astersql_ddl::index::replace_global_lightning_env_for_test(Some(
        std::env::temp_dir().join("aster-d0dfde35b7-ingest"),
    ));
    let _restore = Restore(astersql_sessionctx_vardef::CloudStorageURI.Load(), old_root);
    astersql_sessionctx_vardef::CloudStorageURI.Store("s3://bucket");
    struct ChangingLease(std::sync::Arc<std::sync::atomic::AtomicU64>);
    impl JobLease for ChangingLease {
        fn owner_epoch(&self) -> u64 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn is_owner(&self) -> bool {
            true
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    let epoch = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
    let observed = epoch.clone();
    let _hook = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/afterLoadCloudStorageURI",
        move || {
            observed.store(2, std::sync::atomic::Ordering::SeqCst);
        },
    );
    let f = Fixture::new();
    let (mut job, bytes, index) = setup(&f, 89203, true);
    let error = JobWorker::new(WorkerType::AddIndex)
        .initialize_persisted_index_reorg(
            &mut f.pool.acquire().unwrap(),
            &ChangingLease(epoch),
            &mut job,
            &bytes,
            vec![index],
        )
        .unwrap_err();
    assert_eq!(error, "not DDL owner");
    assert_eq!(f.queue(job.id).unwrap().encode(false).unwrap(), bytes);
}

struct Barrier;
impl astersql_ddl::table_mode::DdlSchemaBarrier for Barrier {
    fn recover(&mut self, _: &Job, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self, _: &Job, _: i64, _: &dyn JobLease) -> Result<(), String> {
        Ok(())
    }
}
struct Policy;
impl astersql_ddl::table_mode::DdlJobPolicy for Policy {
    fn runnable(
        &mut self,
        _: &mut dyn astersql_ddl::job_worker::DurableJobSession,
        _: &Job,
    ) -> Result<bool, String> {
        Ok(true)
    }
    fn error_limit(&self) -> i64 {
        3
    }
    fn mdl_owner(&self) -> Option<String> {
        None
    }
}

#[test]
fn normal_dispatch_initializes_and_reports_next_stage() {
    let _serial = CONFIG.lock().unwrap();
    let old_root = astersql_ddl::index::replace_global_lightning_env_for_test(Some(
        std::env::temp_dir().join("aster-d0dfde35b7-ingest"),
    ));
    let _restore = Restore(astersql_sessionctx_vardef::CloudStorageURI.Load(), old_root);
    astersql_sessionctx_vardef::CloudStorageURI.Store("s3://bucket");
    fail::cfg(DISK_FAULT, "return(true)").unwrap();
    for cloud in [true, false] {
        let f = Fixture::new();
        let (mut job, _, index) = setup(&f, 89204, cloud);
        job.version = astersql_meta_model::group_3::JobVersion::V2;
        job.raw_args = serde_json::to_vec(&astersql_meta_model::group_2::ModifyIndexArgs {
            IndexArgs: vec![astersql_meta_model::group_2::IndexArg {
                IndexName: astersql_meta_model::group_2::ast::NewCIStr(&index.Name.O),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        let mut table = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        table.MaxIndexID = index.ID;
        table.Indices.push(index.clone());
        let mut txn = f
            .domain
            .storage_handle()
            .with_storage(|s| s.Begin(&[]))
            .unwrap();
        astersql_meta::TransactionMutator::new(txn.as_mut())
            .update_table(f.db, &mut table)
            .unwrap();
        txn.Commit(&astersql_kv::Context::default()).unwrap();
        let bytes = job.encode(false).unwrap();
        f.pool
            .acquire()
            .unwrap()
            .query(format!(
                "UPDATE mysql.tidb_ddl_job SET job_meta=X'{}' WHERE job_id={}",
                hex(&bytes),
                job.id
            ))
            .unwrap();
        let mut worker = JobWorker::new(WorkerType::AddIndex);
        let mut executor = astersql_ddl::table_mode::NormalDdlExecutor {
            barrier: Barrier,
            policy: Policy,
            sequence: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        };
        worker
            .transit_persisted_job_step(
                &mut f.pool.acquire().unwrap(),
                &Lease,
                &mut executor,
                &mut job,
                &bytes,
            )
            .unwrap();
        let persisted = f
            .queue(job.id)
            .or_else(|| f.reader().get_history_ddl_job(job.id).unwrap())
            .unwrap();
        let meta = persisted.reorg_meta.as_ref().unwrap();
        assert_eq!(meta.UseCloudStorage, cloud);
        assert_eq!(
            persisted.state,
            if cloud {
                JobState::Running
            } else {
                JobState::Cancelled
            }
        );
        assert_eq!(
            persisted.schema_state,
            astersql_meta_model::SchemaState::None
        );
        let actual = f.reader().get_table(f.db, f.table).unwrap().unwrap();
        let actual = actual.Indices.iter().find(|i| i.ID == index.ID).unwrap();
        assert_eq!(actual.State, astersql_meta_model::SchemaState::None);
        if cloud {
            assert_eq!(meta.ReorgTp, ReorgType::ReorgTypeIngest);
            assert_eq!(
                actual.BackfillState,
                astersql_meta_model::BackfillStateRunning
            );
            assert_eq!(persisted.error_count, 0);
            assert!(persisted.error.is_none());
            let bytes = job.encode(false).unwrap();
            worker
                .transit_persisted_job_step(
                    &mut f.pool.acquire().unwrap(),
                    &Lease,
                    &mut executor,
                    &mut job,
                    &bytes,
                )
                .unwrap();
            let next = f.queue(job.id).unwrap();
            assert!(
                next.error
                    .as_deref()
                    .unwrap()
                    .contains("ADD INDEX stage after reorg initialization is not implemented")
            );
            assert_eq!(next.error_count, 1);
            assert_eq!(next.schema_state, astersql_meta_model::SchemaState::None);
            assert_eq!(next.state, JobState::Running);
            assert_eq!(next.reorg_meta.unwrap().ReorgTp, ReorgType::ReorgTypeIngest);
        } else {
            assert!(f.queue(job.id).is_none());
            assert_eq!(meta.ReorgTp, ReorgType::ReorgTypeNone);
            assert_eq!(persisted.error_count, 1);
            assert!(
                persisted
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("[ddl:8256]Check ingest environment failed: mock error")
            );
            assert_eq!(actual.BackfillState, index.BackfillState);
        }
        assert_eq!(
            f.pool
                .acquire()
                .unwrap()
                .query("SELECT payload FROM test.normal_ddl_target WHERE id=17")
                .unwrap(),
            vec![vec!["retained row".to_owned()]]
        );
    }
}
