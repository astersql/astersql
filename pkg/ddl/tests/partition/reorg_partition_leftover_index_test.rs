// Copyright 2026 AsterSQL.

use astersql_ddl::job_worker::{DurableJobSession, JobLease, JobWorker, WorkerType};
use astersql_ddl::table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor};
use astersql_meta_model::group_2::JobArgsCompat;
use astersql_meta_model::{
    SchemaState as S,
    group_3::{Job, JobState, JobVersion},
};
use astersql_testkit::{TestKit, mockstore::CreateAnalyzeStatsStore};
use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

struct Lease(u64, Arc<AtomicU64>);
impl JobLease for Lease {
    fn is_owner(&self) -> bool {
        self.1.load(Ordering::Acquire) == self.0
    }
    fn is_cancelled(&self) -> bool {
        false
    }
    fn owner_epoch(&self) -> u64 {
        self.0
    }
}
#[derive(Default)]
struct Barrier(Vec<i64>);
impl DdlSchemaBarrier for Barrier {
    fn recover(&mut self, job: &Job, lease: &dyn JobLease) -> Result<(), String> {
        self.wait(job, job.last_schema_version, lease)
    }
    fn wait(&mut self, _: &Job, v: i64, _: &dyn JobLease) -> Result<(), String> {
        if v > 0 {
            self.0.push(v);
        }
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
        barrier: Barrier::default(),
        policy: Policy,
        sequence: Arc::new(AtomicI64::new(0)),
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn index_entries(domain: &astersql_domain::Domain, table: i64, index: i64) -> usize {
    domain.storage_handle().with_storage(|storage| {
        let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
        let (start, end) = astersql_tablecodec::GetTableIndexKeyRange(table, index);
        let mut it = snapshot
            .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
            .unwrap();
        let mut count = 0;
        while it.Valid() {
            count += 1;
            it.Next().unwrap();
        }
        it.Close();
        count
    })
}
/// Start from persisted in-flight metadata, cancel through the normal executor,
/// discard the owner at every boundary, and reload the SQL job on the next owner.
#[test]
fn reorg_partition_leftover_index_cancel_owner_resume() {
    use astersql_domain_infosync as infosync;
    infosync::GlobalInfoSyncerInit(
        "partition-rollback".into(),
        Arc::new(|| 1),
        None,
        None,
        None,
        infosync::Codec::default(),
        false,
        None,
    )
    .unwrap();
    use astersql_meta_model::group_2::GetFinishedTablePartitionArgs;
    use astersql_meta_model::{ACTION_ALTER_TABLE_PARTITIONING, ACTION_REORGANIZE_PARTITION};
    for action in [ACTION_REORGANIZE_PARTITION, ACTION_ALTER_TABLE_PARTITIONING] {
        for state in [
            S::DeleteOnly,
            S::WriteOnly,
            S::WriteReorganization,
            S::DeleteReorganization,
        ] {
            for version in [JobVersion::V1, JobVersion::V2] {
                let store = CreateAnalyzeStatsStore();
                let domain = store.domain();
                let mut tk = TestKit::new(store);
                tk.MustExec("create table leftovers(a int, b int, key idx(b) global) partition by range(a) (partition p0 values less than (10), partition p1 values less than (maxvalue))", vec![]);
                tk.MustExec("insert into leftovers values(1,10),(11,20)", vec![]);
                let mut table = (*domain.table_by_name("test", "leftovers").unwrap()).clone();
                let db = domain
                    .info_schema()
                    .AllSchemas()
                    .into_iter()
                    .find(|s| s.name.lower == "test")
                    .unwrap()
                    .id;
                let keyspace = domain
                    .storage_handle()
                    .with_storage(|s| s.DDLKeyspaceID())
                    .unwrap();
                let codec = if keyspace == u32::MAX {
                    infosync::label::tikv::NewCodecV1()
                } else {
                    infosync::label::tikv::NewCodecV2(keyspace).unwrap()
                };
                let adding_label = infosync::label::NewRuleID(
                    codec,
                    "test".into(),
                    "leftovers".into(),
                    "new_p0".into(),
                );
                infosync::PutLabelRule(Some(&infosync::label::Rule {
                    ID: adding_label.clone(),
                    ..Default::default()
                }))
                .unwrap();
                let original_id = table.Indices[0].ID;
                assert_eq!(
                    index_entries(&domain, table.ID, original_id),
                    2,
                    "real global entries must exist before cancellation"
                );
                let copy_id = table.MaxIndexID + 1;
                let mut copy = table.Indices[0].clone();
                copy.ID = copy_id;
                copy.State = if state == S::DeleteReorganization {
                    S::Public
                } else {
                    state
                };
                table.Indices.push(copy);
                table.MaxIndexID = copy_id;
                let pi = table.Partition.as_mut().unwrap();
                let original = pi.Definitions.clone();
                let mut added = original[0].clone();
                added.ID += 100000;
                added.Name = astersql_parser_ast::NewCIStr("new_p0");
                pi.AddingDefinitions = vec![added.clone()];
                pi.DroppingDefinitions = vec![original[0].clone()];
                pi.OriginalPartitionIDsOrder = original.iter().map(|d| d.ID).collect();
                pi.DDLState = state;
                pi.DDLAction = action;
                pi.DDLType = pi.Type;
                pi.DDLExpr = pi.Expr.clone();
                pi.DDLColumns = pi.Columns.clone();
                pi.DDLChangedIndex.insert(original_id, false);
                pi.DDLChangedIndex.insert(copy_id, true);
                if state == S::DeleteReorganization {
                    pi.Definitions[0] = added;
                    table.Indices[0].State = S::WriteOnly;
                    if action == ACTION_ALTER_TABLE_PARTITIONING {
                        pi.DroppingDefinitions = original.clone();
                    }
                }
                let expected_ids: Vec<_> = original.iter().map(|d| d.ID).collect();
                let table_id = table.ID;
                let pool = astersql_session::runtime::system_session::SystemSessionPool::new(
                    domain.clone(),
                );
                let mut session = pool.acquire().unwrap();
                session.begin().unwrap();
                session
                    .with_execution_context(Box::new(move |context| {
                        context.with_transaction(&mut |txn| {
                            astersql_meta::TransactionMutator::new(txn)
                                .update_table(db, &mut table)?;
                            Ok(vec![])
                        })
                    }))
                    .unwrap();
                session.commit().unwrap();
                let mut job = Job {
                    id: table_id + 200000,
                    schema_id: db,
                    table_id,
                    schema_name: "test".into(),
                    table_name: "leftovers".into(),
                    tp: action,
                    state: JobState::Cancelling,
                    schema_state: state,
                    version,
                    ..Default::default()
                };
                let part_info = astersql_meta_model::PartitionInfo {
                    Type: if action == ACTION_ALTER_TABLE_PARTITIONING {
                        astersql_meta_model::ast::PartitionTypeRange
                    } else {
                        astersql_meta_model::ast::PartitionTypeNone
                    },
                    NewTableID: if action == ACTION_ALTER_TABLE_PARTITIONING {
                        table_id + 300000
                    } else {
                        0
                    },
                    Expr: "a".into(),
                    ..Default::default()
                };
                job.raw_args = serde_json::to_vec(&if version == JobVersion::V1 {
                    serde_json::json!([[], part_info])
                } else {
                    serde_json::json!({"part_names":[], "part_info":part_info})
                })
                .unwrap();
                let encoded = job.encode(false).unwrap();
                session.query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},1,'{}','{}',X'{}',{},0)",job.id,db,table_id,hex(&encoded),action)).unwrap();
                let mut versions = Vec::new();
                let owner = Arc::new(AtomicU64::new(1));
                let steps = if state == S::DeleteOnly { 2 } else { 3 };
                for step in 0..steps {
                    // Fresh owner and job; no in-memory phase marker survives.
                    let rows = session
                        .query(format!(
                            "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id={}",
                            job.id
                        ))
                        .unwrap();
                    let expected = rows[0][0].as_bytes().to_vec();
                    job = astersql_meta::decode_go_history_job(&expected).unwrap();
                    let mut executor = executor();
                    let epoch = step as u64 + 1;
                    owner.store(epoch, Ordering::Release);
                    if step > 0 {
                        let mut stale_job =
                            astersql_meta::decode_go_history_job(&expected).unwrap();
                        let rejected = JobWorker::new(WorkerType::General)
                            .transit_persisted_job_step(
                                &mut session,
                                &Lease(epoch - 1, owner.clone()),
                                &mut executor,
                                &mut stale_job,
                                &expected,
                            );
                        assert_eq!(rejected.unwrap_err(), "not DDL owner");
                    }
                    let mut worker = JobWorker::new(WorkerType::General);
                    let v = worker
                        .transit_persisted_job_step(
                            &mut session,
                            &Lease(epoch, owner.clone()),
                            &mut executor,
                            &mut job,
                            &expected,
                        )
                        .unwrap();
                    if step > 0 {
                        assert!(
                            executor.barrier.0.contains(versions.last().unwrap()),
                            "new owner must recover the last committed schema version"
                        );
                    }
                    assert!(job.error_count <= 1, "{:?}", job.error);
                    assert!(v > 0, "{:?}", job.error);
                    versions.push(v);
                    domain.reload().unwrap();
                    let current = domain.table_by_name("test", "leftovers").unwrap();
                    assert_eq!(
                        current
                            .Partition
                            .as_ref()
                            .unwrap()
                            .Definitions
                            .iter()
                            .map(|d| d.ID)
                            .collect::<Vec<_>>(),
                        expected_ids
                    );
                    if step + 1 < steps {
                        let copy = current
                            .Indices
                            .iter()
                            .find(|i| i.ID == copy_id)
                            .expect("copy must remain until stop-write version is committed");
                        let expected_state = if step == 0 && state != S::DeleteOnly {
                            S::WriteOnly
                        } else {
                            S::DeleteOnly
                        };
                        assert_eq!(copy.State, expected_state);
                        assert_eq!(job.state, JobState::Rollingback);
                    } else {
                        assert_eq!(job.state, JobState::RollbackDone);
                        assert_eq!(current.Indices.len(), 1);
                        assert_eq!(current.Indices[0].ID, original_id);
                        assert_eq!(current.Indices[0].State, S::Public);
                        let args = GetFinishedTablePartitionArgs(&mut job).unwrap();
                        if action == ACTION_ALTER_TABLE_PARTITIONING {
                            assert!(args.OldPhysicalTblIDs.contains(&(table_id + 300000)));
                        }
                        assert_eq!(args.OldGlobalIndexes.len(), 1);
                        assert_eq!(args.OldGlobalIndexes[0].IndexID, copy_id);
                    }
                    let before = index_entries(&domain, table_id, copy_id);
                    tk.MustExec(
                        &format!("insert into leftovers values({},30)", step + 3),
                        vec![],
                    );
                    let after = index_entries(&domain, table_id, copy_id);
                    assert_eq!(
                        after - before,
                        usize::from(step == 0 && state != S::DeleteOnly),
                        "only write-only copies may take new mutations"
                    );
                }
                assert!(versions.windows(2).all(|v| v[0] < v[1]));
                assert!(infosync::GetLabelRules(&[adding_label]).unwrap().is_empty());
                assert_eq!(index_entries(&domain, table_id, original_id), 2 + steps);
                tk.MustExec("admin check table leftovers", vec![]);
                tk.MustExec("insert into leftovers values(2,10)", vec![]);
                assert_eq!(index_entries(&domain, table_id, original_id), 3 + steps);
                assert_eq!(
                    tk.MustQuery("select a,b from leftovers order by a", vec![])
                        .Rows()
                        .len(),
                    3 + steps
                );
                // Terminal handling consumes the persisted finished args and
                // registers physical/index garbage before moving to history.
                let rows = session
                    .query(format!(
                        "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id={}",
                        job.id
                    ))
                    .unwrap();
                let expected = rows[0][0].as_bytes().to_vec();
                job = astersql_meta::decode_go_history_job(&expected).unwrap();
                owner.store(steps as u64 + 1, Ordering::Release);
                JobWorker::new(WorkerType::General)
                    .transit_persisted_job_step(
                        &mut session,
                        &Lease(steps as u64 + 1, owner.clone()),
                        &mut executor(),
                        &mut job,
                        &expected,
                    )
                    .unwrap();
                assert!(
                    session
                        .query(format!(
                            "SELECT job_meta FROM mysql.tidb_ddl_job WHERE job_id={}",
                            job.id
                        ))
                        .unwrap()
                        .is_empty()
                );
                let gc = session
                    .query(format!(
                        "SELECT count(*) FROM mysql.gc_delete_range WHERE job_id={}",
                        job.id
                    ))
                    .unwrap();
                assert!(gc[0][0].parse::<usize>().unwrap() >= 2);
                // ADMIN CHECK must inspect real scalar global entries, not just metadata.
                domain.storage_handle().with_storage(|storage| {
                    let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
                    let (start, end) =
                        astersql_tablecodec::GetTableIndexKeyRange(table_id, original_id);
                    let mut it = snapshot
                        .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
                        .unwrap();
                    assert!(it.Valid());
                    let key = it.Key();
                    it.Close();
                    let mut txn = storage.Begin(&[]).unwrap();
                    txn.Delete(key).unwrap();
                    txn.Commit(&astersql_kv::Context::default()).unwrap();
                });
                tk.MustContainErrMsg("admin check table leftovers", "index entry is missing");
                drop(session);
                pool.close();
            }
        }
    }
}

#[test]
fn hidden_column_model_preserves_copy_flags_and_compacts_after_last_consumer() {
    use astersql_meta_model::{ColumnInfo, IndexColumn, IndexInfo, TableInfo};
    use astersql_parser_mysql::r#type::MultipleKeyFlag;
    let mut hidden = ColumnInfo::default();
    hidden.ID = 1;
    hidden.Name = astersql_parser_ast::NewCIStr("hidden");
    hidden.Hidden = true;
    hidden.AddFlag(MultipleKeyFlag);
    let mut visible = ColumnInfo::default();
    visible.ID = 2;
    visible.Offset = 1;
    visible.Name = astersql_parser_ast::NewCIStr("visible");
    let old = IndexInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("expr"),
        Columns: vec![IndexColumn {
            Name: hidden.Name.clone(),
            Offset: 0,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut copy = old.clone();
    copy.ID = 2;
    let remaining = IndexInfo {
        ID: 3,
        Name: astersql_parser_ast::NewCIStr("other"),
        Columns: vec![IndexColumn {
            Name: visible.Name.clone(),
            Offset: 1,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut table = TableInfo {
        Columns: vec![hidden, visible],
        Indices: vec![old.clone(), copy.clone(), remaining],
        ..Default::default()
    };
    astersql_ddl::index::drop_model_index_column_flag(&mut table, &old);
    assert_ne!(table.Columns[0].GetFlag() & MultipleKeyFlag, 0);
    astersql_ddl::index::remove_model_dependent_hidden_columns(&mut table, &old);
    assert_eq!(table.Columns.len(), 2);
    table.Indices.retain(|i| i.ID != old.ID);
    astersql_ddl::index::drop_model_index_column_flag(&mut table, &copy);
    assert_eq!(table.Columns[0].GetFlag() & MultipleKeyFlag, 0);
    astersql_ddl::index::remove_model_dependent_hidden_columns(&mut table, &copy);
    table.Indices.retain(|i| i.ID != copy.ID);
    assert_eq!(table.Columns.len(), 1);
    assert_eq!(table.Columns[0].Offset, 0);
    assert_eq!(table.Indices[0].Columns[0].Offset, 0);
}

#[test]
fn reorg_partition_leftover_index_global_unique_dml_regression() {
    let store = CreateAnalyzeStatsStore();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table global_unique(a int, b int, unique key idx(b) global) partition by range(a) (partition p0 values less than (10), partition pmax values less than (maxvalue))", vec![]);
    tk.MustExec("insert into global_unique values(1,10),(11,20)", vec![]);
    tk.MustContainErrMsg("insert into global_unique values(12,10)", "Duplicate entry");
    tk.MustExec("update global_unique set b=30 where a=1", vec![]);
    tk.MustExec("admin check table global_unique", vec![]);
    tk.MustExec("delete from global_unique where a=11", vec![]);
    tk.MustExec("admin check table global_unique", vec![]);
    tk.MustQuery("select a,b from global_unique order by a", vec![])
        .Check(vec![vec!["1", "30"]]);
    tk.MustExec("alter table global_unique drop index idx", vec![]);
    tk.MustExec("insert into global_unique values(12,30)", vec![]);
    tk.MustExec("admin check table global_unique", vec![]);
}
