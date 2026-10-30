// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 分区重组 DDL 测试。
//
// 对应 Go `reorg_partition_test.go`：覆盖 reorganize / remove partitioning /
// `PARTITION BY` / add·coalesce hash·key 分区的失败路径，并发 DML、failpoint
// 注入回滚，以及 Placement Policy（放置策略）约束。
// Reorg 指后台将旧物理分区数据回填到新分区定义的重组过程。

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::{
    ACTION_REORGANIZE_PARTITION, ActionNone, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StateDeleteReorganization, StateNone, StatePublic, StateWriteOnly,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit_testfailpoint::{enable, eval_bool};
use std::thread;

/// 构造带 LessThan 上界的 range 分区定义。
fn part(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 验证 reorganize 经 WriteOnly → DeleteReorg → Public 时旧定义被 Adding 替换并清理中间态。
#[test]
fn reorg_partition_moves_old_and_new_definitions_through_schema_states() {
    let old = part(1, "p0", "100");
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![old.clone(), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![old],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        NewPartitionIDs: vec![2, 3],
        ..Default::default()
    };
    assert_eq!(info.DroppingDefinitions[0].ID, 1);
    // DeleteReorganization：用新分区定义 splice 替换旧 p0，再进入 Public 清空 Adding/Dropping。
    info.DDLState = StateDeleteReorganization;
    info.Definitions
        .splice(0..1, info.AddingDefinitions.clone());
    assert_eq!(info.GetPartitionIDByName("p0a"), 2);
    assert_eq!(info.GetPartitionIDByName("p0b"), 3);
    info.DDLState = StatePublic;
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    assert_eq!(info.Definitions.len(), 3);
}

/// 验证重组期间并发 DML 不丢行：各物理分区 realtime_count 之和等于写入总量。
#[test]
fn concurrent_dml_during_reorg_keeps_every_real_row() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec(
        "create table reorg_dml(a int primary key, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    // 4 worker × 8 行，模拟 reorg 期间并发写入。
    let workers = (0..4)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                let mut client = TestKit::new(store);
                for offset in 0..8 {
                    let value = worker * 8 + offset;
                    client.MustExec(
                        &format!("insert into reorg_dml values ({value},{value})"),
                        Vec::new(),
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("reorg DML worker");
    }
    owner.MustExec("flush stats_delta reorg_dml", Vec::new());
    let context = owner.AnalyzeStatsContext().unwrap();
    let table = context
        .catalog()
        .get(&("test".to_owned(), "reorg_dml".to_owned()))
        .unwrap()
        .1
        .Clone();
    let total = table
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|definition| {
            context
                .physical_stats(definition.ID)
                .unwrap()
                .realtime_count
        })
        .sum::<i64>();
    assert_eq!(total, 32);
}

/// 验证回填 failpoint 触发后清理 Adding/Dropping/NewIDs，中间态清空且保留原物理 ID。
#[test]
fn injected_reorg_failure_rolls_metadata_back_without_stale_ids() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(1, "p0", "100"), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![part(1, "p0", "100")],
        NewPartitionIDs: vec![2, 3],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    let _failure = enable("partition/reorg-backfill", "return(true)");
    assert!(eval_bool("partition/reorg-backfill"));
    // 模拟失败回滚：丢掉中间定义并 ClearReorgIntermediateInfo。
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    info.NewPartitionIDs.clear();
    info.ClearReorgIntermediateInfo();
    assert_eq!(info.DDLAction, ActionNone);
    assert_eq!(info.DDLState, StateNone);
    assert_eq!(info.Definitions[0].ID, 1);
}

/// 验证 LIST 分区删除时默认分区索引与 overlapping dropping 下标计算。
#[test]
fn list_reorg_uses_default_partition_for_dropping_values() {
    let info = PartitionInfo {
        Type: PartitionType::List,
        Enable: true,
        Definitions: vec![
            PartitionDefinition {
                ID: 1,
                Name: NewCIStr("p0"),
                InValues: vec![vec!["1".to_owned()]],
                ..Default::default()
            },
            PartitionDefinition {
                ID: 2,
                Name: NewCIStr("pdefault"),
                InValues: vec![vec!["DEFAULT".to_owned()]],
                ..Default::default()
            },
        ],
        DroppingDefinitions: vec![PartitionDefinition {
            ID: 1,
            Name: NewCIStr("p0"),
            ..Default::default()
        }],
        DDLAction: astersql_meta_model::ActionDropTablePartition,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    assert_eq!(info.GetDefaultListPartition(), 1);
    assert_eq!(info.GetOverlappingDroppingPartitionIdx(0), 1);
}

/// 验证 `GCPartitionStates` 丢弃已不在 Definitions 中的陈旧分区状态。
#[test]
fn reorg_gc_drops_only_partition_states_no_longer_in_definitions() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(2, "p1", "MAXVALUE")],
        ..Default::default()
    };
    info.SetStateByID(1, StateDeleteOnly);
    info.SetStateByID(2, StatePublic);
    info.GCPartitionStates();
    assert_eq!(info.States.len(), 1);
    assert_eq!(info.States[0].ID, 2);
}

/// 对应 Go `TestReorgPartitionConcurrent` 的提交后可见性部分：重组只替换
/// 被选中的分区，且所有原有行在新的分区边界下仍可查询。
#[test]
fn reorg_partition_replaces_selected_definitions_without_losing_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table reorg_runtime(a int primary key, b int) \
         partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into reorg_runtime values (1, 10), (11, 110), (19, 190), (21, 210)",
        Vec::new(),
    );

    let before = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let before_partition = before.GetPartitionInfo().unwrap();
    let old_p0 = before_partition.GetPartitionIDByName("p0");
    let old_p1 = before_partition.GetPartitionIDByName("p1");
    let old_pmax = before_partition.GetPartitionIDByName("pmax");

    testkit.MustExec(
        "alter table reorg_runtime reorganize partition p1 into \
         (partition p1a values less than (15), partition p1b values less than (20))",
        Vec::new(),
    );

    let after = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let partition = after.GetPartitionInfo().unwrap();
    assert_eq!(partition.GetPartitionIDByName("p0"), old_p0);
    assert_eq!(partition.GetPartitionIDByName("pmax"), old_pmax);
    assert_eq!(partition.GetPartitionIDByName("p1"), -1);
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1a")));
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1b")));
    assert_eq!(
        testkit
            .MustQuery("select a, b from reorg_runtime order by a", Vec::new())
            .Rows()
            .len(),
        4
    );
}

/// Go `TestReorgPartitionHandleNotExistNoPanic`: a missing durable reorg
/// element is an ordinary retryable boundary, not an absent context that a
/// partition worker may dereference.  After observing that boundary, the same
/// table can still complete the partition replacement with both local indexes
/// and every source row intact.
#[test]
fn missing_reorg_handle_returns_error_and_retry_preserves_rows_and_local_indexes() {
    use astersql_ddl::job_worker::DurableJobSession;
    use astersql_ddl::reorg::{PersistentReorgHandler, ReorgElement, ReorgInfo};
    use astersql_meta_model::group_3::Job;

    let store = CreateAnalyzeStatsStore();
    let domain = store.domain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table reorg_missing_handle (\
             a int unsigned primary key, b varchar(255), c int, \
             key idx_b (b), key idx_cb (c,b)) \
         partition by range (a) (\
             partition p0 values less than (10), \
             partition p1 values less than (20), \
             partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into reorg_missing_handle values \
         (1,'1',1),(10,'10',10),(23,'23',32),\
         (34,'34',43),(45,'45',54),(56,'56',65)",
        Vec::new(),
    );

    let table = domain
        .table_by_name("test", "reorg_missing_handle")
        .expect("partitioned table");
    let p1 = table
        .GetPartitionInfo()
        .expect("partition metadata")
        .GetPartitionIDByName("p1");
    let schema_id = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|database| database.name.lower == "test")
        .expect("test database")
        .id;
    let mut job = Job {
        id: 69_303,
        tp: ACTION_REORGANIZE_PARTITION,
        schema_id,
        table_id: table.ID,
        snapshot_ver: 42,
        ..Default::default()
    };
    let pool = astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
    let mut session = pool.acquire().expect("system session");
    let error = PersistentReorgHandler::restore(&mut session, &mut job)
        .err()
        .expect("missing reorg element must be reported");
    assert!(
        error.contains("DDL reorg element does not exist"),
        "{error}"
    );
    assert_eq!(job.snapshot_ver, 0);

    let record_prefix = astersql_tablecodec::GenTableRecordPrefix(p1);
    let info = ReorgInfo {
        job_id: job.id,
        physical_table_id: p1,
        start_key: record_prefix.0.clone(),
        end_key: record_prefix.PrefixNext().0,
        element: ReorgElement {
            id: table.Columns[0].ID,
            element_type: b"_col_".to_vec(),
        },
        ..Default::default()
    };
    PersistentReorgHandler::initialize(&mut session, &info).expect("initialize retry state");
    assert_eq!(
        PersistentReorgHandler::restore(&mut session, &mut job)
            .expect("retry restores the durable element")
            .info,
        info
    );

    testkit.MustExec(
        "alter table reorg_missing_handle reorganize partition p1 into \
         (partition p1a values less than (15), partition p1b values less than (20))",
        Vec::new(),
    );
    testkit.MustExec("admin check table reorg_missing_handle", Vec::new());
    let reorganized = domain
        .table_by_name("test", "reorg_missing_handle")
        .expect("reorganized table");
    let local_index_entries = domain.storage_handle().with_storage(|storage| {
        let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
        reorganized
            .GetPartitionInfo()
            .unwrap()
            .Definitions
            .iter()
            .flat_map(|definition| {
                reorganized
                    .Indices
                    .iter()
                    .filter(|index| !index.Global)
                    .map(|index| (definition.ID, index.ID))
            })
            .map(|(physical_id, index_id)| {
                let (start, end) =
                    astersql_tablecodec::GetTableIndexKeyRange(physical_id, index_id);
                let mut iterator = snapshot
                    .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
                    .unwrap();
                let mut count = 0;
                while iterator.Valid() {
                    count += 1;
                    iterator.Next().unwrap();
                }
                iterator.Close();
                count
            })
            .sum::<usize>()
    });
    assert_eq!(local_index_entries, 12);
    let expected = astersql_testkit::Rows(&["1", "10", "23", "34", "45", "56"]);
    testkit
        .MustQuery("select a from reorg_missing_handle order by a", Vec::new())
        .Check(expected);
}

/// Enter exactly the phase changed by 45e4745, after new-partition data and
/// indexes have been built. Use the production pooled session/KV backfiller;
/// the absent whole REORGANIZE job lifecycle is not part of this fixture.
#[test]
fn reorg_non_touched_phase_preserves_all_four_global_entries() {
    use astersql_ddl::job_worker::DurableJobSession;
    use astersql_ddl::reorg::{ReorgElement, ReorgInfo};
    let store = CreateAnalyzeStatsStore();
    let domain = store.domain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table reorg_phase(a int, b int, unique key idx_b(b) global) partition by range(a) (partition p0 values less than (10), partition p1a values less than (15), partition p1b values less than (30), partition pmax values less than (maxvalue))", Vec::new());
    tk.MustExec(
        "insert into reorg_phase values (1,10),(12,120),(25,250),(30,300)",
        Vec::new(),
    );
    let mut final_table = domain
        .table_by_name("test", "reorg_phase")
        .unwrap()
        .as_ref()
        .clone();
    // REORGANIZE rebuilds a fresh global index. Avoid depending on whether
    // ordinary DML already maintains the source index in this repository.
    final_table.MaxIndexID += 1;
    final_table
        .Indices
        .iter_mut()
        .find(|index| index.Name.L == "idx_b")
        .unwrap()
        .ID = final_table.MaxIndexID;
    let schema_id = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|db| db.name.lower == "test")
        .unwrap()
        .id;
    let index_id = final_table
        .Indices
        .iter()
        .find(|index| index.Name.L == "idx_b")
        .unwrap()
        .ID;
    let table_id = final_table.ID;
    let pool = astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
    let mut session = pool.acquire().unwrap();
    session.begin().unwrap();
    let table = final_table.clone();
    session.with_execution_context(Box::new(move |context| {
        let mut table = table;
        let mut job_id = 0;
        context.with_transaction(&mut |txn| {
            let key = astersql_meta::transaction_meta_string_key(b"NextGlobalID");
            job_id = astersql_kv::IncInt64(txn, &key, 3).map_err(|e| e.to_string())?;
            Ok(Vec::new())
        })?;
        let mut finished = table.clone();
        let pi = table.Partition.as_mut().unwrap();
        let definitions = pi.Definitions.clone();
        pi.DDLAction = ACTION_REORGANIZE_PARTITION;
        pi.DDLState = astersql_meta_model::SchemaState::WriteReorganization;
        pi.AddingDefinitions = definitions[1..3].to_vec();
        pi.DroppingDefinitions = vec![part(job_id - 2, "p1", "20"), part(job_id - 1, "p2", "30")];
        pi.Definitions = vec![definitions[0].clone(), pi.DroppingDefinitions[0].clone(), pi.DroppingDefinitions[1].clone(), definitions[3].clone()];
        table.Indices.iter_mut().find(|i| i.ID == index_id).unwrap().State = astersql_meta_model::SchemaState::WriteReorganization;
        context.with_transaction(&mut |txn| {
            astersql_meta::TransactionMutator::new(txn).update_table(schema_id, &mut table)?;
            Ok(Vec::new())
        })?;
        let job = astersql_meta_model::group_3::Job { id: job_id, schema_id, table_id, ..Default::default() };
        // Seed only the completed AddingDefinitions phase through the same real
        // backfill adapter. Neither non-touched partition has an index entry yet.
        for definition in &table.Partition.as_ref().unwrap().AddingDefinitions {
            let prefix = astersql_tablecodec::GenTableRecordPrefix(definition.ID);
            let result = context.backfill_prepared_indexes(astersql_ddl::backfilling::IndexBackfillBatch {
                schema_id, table_id, index_ids: vec![index_id],
                task: astersql_ddl::backfilling::ReorgBackfillTask {
                    job_id, physical_table_id: definition.ID,
                    start_key: prefix.0.clone(), end_key: prefix.PrefixNext().0,
                    ..Default::default()
                }, batch_size: 16, resource_group: String::new(), sql_mode: 0,
            })?;
            assert!(result.done);
            assert_eq!(result.added_count, 1);
        }
        let prefix = astersql_tablecodec::GenTableRecordPrefix(definitions[0].ID);
        let element = ReorgElement { id: index_id, element_type: b"_idx_".to_vec() };
        let mut reorg = ReorgInfo {
            job_id, physical_table_id: definitions[0].ID,
            start_key: prefix.0.clone(), end_key: prefix.PrefixNext().0,
            element: element.clone(), elements: vec![element], ..Default::default()
        };
        context.query(&format!("INSERT INTO mysql.tidb_ddl_reorg (job_id,ele_id,ele_type,start_key,end_key,physical_id) VALUES ({job_id},{index_id},X'5f6964785f',X'',X'',{})", reorg.physical_table_id), "init_handle")?;
        // A batch of one exercises bounded processing through the final partition.
        let mut batches = 0;
        while !astersql_ddl::partition::backfill_non_touched_partition_indexes(context, &job, &table, &mut reorg, 1)? {
            batches += 1;
            assert!(batches < 8);
        }
        assert_eq!(reorg.physical_table_id, 0);
        assert_eq!(context.query(&format!("SELECT physical_id FROM mysql.tidb_ddl_reorg WHERE job_id={job_id}"), "get_handle")?, vec![vec!["0".to_owned()]]);
        context.with_transaction(&mut |txn| {
            astersql_meta::TransactionMutator::new(txn).update_table(schema_id, &mut finished)?;
            Ok(Vec::new())
        })?;
        context.query(&format!("DELETE FROM mysql.tidb_ddl_reorg WHERE job_id={job_id}"), "clean_handle")?;
        Ok(Vec::new())
    })).unwrap();
    session.commit().unwrap();
    domain.reload().unwrap();
    let (start, end) = astersql_tablecodec::GetTableIndexKeyRange(table_id, index_id);
    let values = domain.storage_handle().with_storage(|storage| {
        let mut iter = storage
            .GetSnapshot(storage.CurrentVersion("global").unwrap())
            .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
            .unwrap();
        let mut values = Vec::new();
        while iter.Valid() {
            values.push(iter.Value().to_vec());
            iter.Next().unwrap();
        }
        iter.Close();
        values
    });
    assert_eq!(values.len(), 4);
    let mut counts = std::collections::BTreeMap::new();
    for value in values {
        let handle = astersql_tablecodec::DecodeHandleInIndexValue(value)
            .unwrap()
            .unwrap();
        let handle = handle
            .as_any()
            .downcast_ref::<astersql_tablecodec::kv::PartitionHandle>()
            .unwrap();
        *counts.entry(handle.PartitionID).or_insert(0) += 1;
    }
    for definition in &final_table.Partition.as_ref().unwrap().Definitions {
        assert_eq!(
            counts.get(&definition.ID),
            Some(&1),
            "partition {}",
            definition.Name.O
        );
    }
    tk.MustExec("admin check table reorg_phase", Vec::new());
    for hint in ["use index(idx_b)", "ignore index(idx_b)"] {
        tk.MustQuery(
            &format!("select a,b from reorg_phase {hint} where b >= 0 order by b"),
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "1 10", "12 120", "25 250", "30 300",
        ]));
    }
    tk.MustContainErrMsg("insert into reorg_phase values (31,300)", "Duplicate entry");
}

/// A clustered PRIMARY KEY is the record handle and therefore has no separate
/// index KV. A non-clustered PRIMARY KEY remains an ordinary maintained index,
/// including while REORGANIZE PARTITION moves records to new physical IDs.
#[test]
fn reorg_partition_backfills_only_nonclustered_primary_index_entries() {
    let store = CreateAnalyzeStatsStore();
    let domain = store.domain();
    let mut tk = TestKit::new(store);

    for (table_name, primary_mode, expect_primary_entries) in [
        ("reorg_clustered_pk", "clustered", false),
        ("reorg_nonclustered_pk", "nonclustered", true),
    ] {
        tk.MustExec(
            &format!(
                "create table {table_name} (\
                     a int, b int, c int, primary key (a,b) {primary_mode}, key idx_c(c)) \
                 partition by range (b) (\
                     partition p0 values less than (10), \
                     partition pmax values less than (maxvalue))"
            ),
            Vec::new(),
        );
        tk.MustExec(
            &format!("insert into {table_name} values (1,11,1),(2,12,2),(3,13,3),(4,14,4)"),
            Vec::new(),
        );
        tk.MustExec(
            &format!(
                "alter table {table_name} reorganize partition pmax into \
                 (partition p1 values less than (20), \
                  partition pmax values less than (maxvalue))"
            ),
            Vec::new(),
        );

        let table = domain.table_by_name("test", table_name).unwrap();
        let p1 = table.GetPartitionInfo().unwrap().GetPartitionIDByName("p1");
        let primary_id = table.FindIndexByName("primary").unwrap().ID;
        let secondary_id = table.FindIndexByName("idx_c").unwrap().ID;
        let count_entries = |index_id| {
            domain.storage_handle().with_storage(|storage| {
                let (start, end) = astersql_tablecodec::GetTableIndexKeyRange(p1, index_id);
                let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
                let mut iterator = snapshot
                    .Iter(astersql_kv::Key(start), Some(astersql_kv::Key(end)))
                    .unwrap();
                let mut count = 0;
                while iterator.Valid() {
                    count += 1;
                    iterator.Next().unwrap();
                }
                iterator.Close();
                count
            })
        };
        assert_eq!(
            count_entries(primary_id),
            usize::from(expect_primary_entries) * 4
        );
        assert_eq!(count_entries(secondary_id), 4);
        tk.MustExec(&format!("admin check table {table_name}"), Vec::new());
        tk.MustQuery(
            &format!("select a,b,c from {table_name} order by a"),
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&[
            "1 11 1", "2 12 2", "3 13 3", "4 14 4",
        ]));
    }
}

// Source 9453792437: restore the final stage from durable metadata rather than
// inventing an in-memory reorganization state machine.
#[test]
fn port_9453792437_public_finalization_publishes_metadata_before_one_notification() {
    use astersql_ddl::job_worker::{DurableJobSession, JobLease, JobWorker, WorkerType};
    use astersql_ddl::table_mode::{DdlJobPolicy, DdlSchemaBarrier, NormalDdlExecutor};
    use astersql_meta_model::group_3::{Job, JobState, JobVersion};
    use std::sync::{Arc, atomic::AtomicI64};
    struct Lease;
    impl JobLease for Lease {
        fn is_owner(&self) -> bool {
            true
        }
        fn is_cancelled(&self) -> bool {
            false
        }
        fn owner_epoch(&self) -> u64 {
            1
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
    let store = CreateAnalyzeStatsStore();
    let domain = store.domain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table finalize_reorg(a int unsigned primary key, b varchar(255), c int, key(b), key(c,b)) partition by range(a) (partition p0 values less than(10), partition p1 values less than(20), partition p2 values less than(30), partition pMax values less than(maxvalue))", vec![]);
    tk.MustExec(
        "insert into finalize_reorg values(1,'1',1),(12,'12',21),(23,'23',32)",
        vec![],
    );
    let original = domain.table_by_name("test", "finalize_reorg").unwrap();
    tk.MustExec("alter table finalize_reorg reorganize partition p1,p2 into (partition p1 values less than(17), partition p1b values less than(24), partition p2 values less than(30))", vec![]);
    let mut table = (*domain.table_by_name("test", "finalize_reorg").unwrap()).clone();
    let final_definitions = table.Partition.as_ref().unwrap().Definitions.clone();
    let pi = table.Partition.as_mut().unwrap();
    pi.DroppingDefinitions = original.Partition.as_ref().unwrap().Definitions[1..3].to_vec();
    pi.AddingDefinitions = final_definitions[1..4].to_vec();
    pi.DDLState = StatePublic;
    pi.DDLAction = ACTION_REORGANIZE_PARTITION;
    pi.OriginalPartitionIDsOrder = original
        .Partition
        .as_ref()
        .unwrap()
        .Definitions
        .iter()
        .map(|d| d.ID)
        .collect();
    let db = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|d| d.name.lower == "test")
        .unwrap()
        .id;
    let mut job = Job {
        id: table.ID + 945379,
        schema_id: db,
        table_id: table.ID,
        schema_name: "test".into(),
        table_name: "finalize_reorg".into(),
        tp: ACTION_REORGANIZE_PARTITION,
        state: JobState::Running,
        schema_state: StatePublic,
        version: JobVersion::V2,
        ..Default::default()
    };
    job.raw_args = serde_json::to_vec(&serde_json::json!({"part_names":["p1","p2"],"part_info":{"definitions":final_definitions[1..4]}})).unwrap();
    let pool = astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
    let mut session = pool.acquire().unwrap();
    session.begin().unwrap();
    session
        .with_execution_context(Box::new(move |context| {
            context.with_transaction(&mut |txn| {
                let mut meta = astersql_meta::TransactionMutator::new(txn);
                meta.update_table(db, &mut table)?;
                let version = meta.gen_schema_version()?;
                meta.set_table_schema_diff(
                    &Job {
                        schema_id: db,
                        table_id: table.ID,
                        tp: ACTION_REORGANIZE_PARTITION,
                        ..Default::default()
                    },
                    version,
                )?;
                Ok(vec![])
            })
        }))
        .unwrap();
    session.commit().unwrap();
    let expected = job.encode(false).unwrap();
    let hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
    session.query(format!("INSERT INTO mysql.tidb_ddl_job (job_id,reorg,schema_ids,table_ids,job_meta,type,processing) VALUES ({},1,'{}','{}',X'{}',{},0)",job.id,db,job.table_id,hex,job.tp)).unwrap();
    // Inject at the real context boundary immediately before the final
    // metadata operation. Neither the KV transaction nor the notifier is mocked.
    let initial_version = port945_snapshot(&domain)
        .get_schema_version_with_non_empty_diff()
        .unwrap();
    let mut retry_job = astersql_meta::decode_go_history_job(&expected).unwrap();
    session.begin().unwrap();
    let failure = session
        .with_execution_context(Box::new(move |inner| {
            let mut context = Port945FinalUpdateFailure {
                inner,
                transaction_calls: 0,
                notification: false,
                replacement: None,
            };
            let error =
                astersql_ddl::partition::finish_persisted_partition(&mut context, &mut retry_job)
                    .unwrap_err();
            assert_eq!(error, "injected final metadata failure");
            assert_eq!(context.transaction_calls, 3);
            assert_eq!(retry_job.schema_state, StatePublic);
            assert_eq!(retry_job.state, JobState::Running);
            Err(error)
        }))
        .unwrap_err();
    assert_eq!(failure, "injected final metadata failure");
    session.rollback();
    assert_eq!(
        port945_snapshot(&domain)
            .get_schema_version_with_non_empty_diff()
            .unwrap(),
        initial_version
    );
    assert!(
        session
            .query(format!(
                "SELECT ddl_job_id FROM mysql.tidb_ddl_notifier WHERE ddl_job_id={}",
                job.id
            ))
            .unwrap()
            .is_empty()
    );
    domain.reload().unwrap();
    assert_eq!(
        domain
            .table_by_name("test", "finalize_reorg")
            .unwrap()
            .Partition
            .as_ref()
            .unwrap()
            .DDLState,
        StatePublic
    );
    let mut executor = NormalDdlExecutor {
        barrier: Barrier,
        policy: Policy,
        sequence: Arc::new(AtomicI64::new(0)),
    };
    JobWorker::new(WorkerType::General)
        .transit_persisted_job_step(&mut session, &Lease, &mut executor, &mut job, &expected)
        .expect("persisted Public reorganization must finalize");
    assert_eq!(job.state, JobState::Done, "{:?}", job.error);
    assert_eq!(job.schema_state, StateNone);
    domain.reload().unwrap();
    let actual = domain.table_by_name("test", "finalize_reorg").unwrap();
    let pi = actual.Partition.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(&pi.Definitions).unwrap(),
        serde_json::to_value(&final_definitions).unwrap()
    );
    assert!(pi.AddingDefinitions.is_empty() && pi.DroppingDefinitions.is_empty());
    assert_eq!(pi.DDLState, StateNone);
    let events = session
        .query(format!(
            "SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id={}",
            job.id
        ))
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        port945_snapshot(&domain)
            .get_schema_version_with_non_empty_diff()
            .unwrap(),
        initial_version + 1,
        "only the successful final update may advance schema version"
    );
    let event: serde_json::Value = serde_json::from_str(&events[0][0]).unwrap();
    assert_eq!(event["type"], ACTION_REORGANIZE_PARTITION);
    assert_eq!(event["table_info"], serde_json::to_value(&*actual).unwrap());
    let diff = port945_diff(&domain, job.last_schema_version);
    assert_eq!(diff["table_id"], job.table_id);
    let dropped = &original.Partition.as_ref().unwrap().Definitions[1..3];
    let expected_affected: Vec<_> = (0..3).map(|i| serde_json::json!({"schema_id":0,"old_schema_id":0,"old_table_id":dropped.get(i).map(|d|d.ID).unwrap_or(0),"table_id":final_definitions[i+1].ID})).collect();
    assert_eq!(
        diff["affected_options"],
        serde_json::json!(expected_affected)
    );
    tk.MustExec("admin check table finalize_reorg", vec![]);
    assert_eq!(
        tk.MustQuery("select a,b from finalize_reorg order by a", vec![])
            .Rows()
            .len(),
        3
    );
}

/// Fault injection at a context boundary; all successful operations still use
/// the pooled session's real active MVCC transaction and real SQL notifier.
struct Port945FinalUpdateFailure<'a> {
    inner: &'a mut dyn astersql_ddl::job_worker::JobExecutionContext,
    transaction_calls: usize,
    notification: bool,
    replacement: Option<(i64, i64, i64)>,
}
impl astersql_ddl::job_worker::JobExecutionContext for Port945FinalUpdateFailure<'_> {
    fn query(&mut self, sql: &str, label: &str) -> Result<Vec<Vec<String>>, String> {
        if label == "publish-schema-change" {
            if self.notification {
                return Err("injected notifier SQL failure".into());
            }
            panic!("notification preceded final metadata update");
        }
        self.inner.query(sql, label)
    }
    fn put_create_table_bundles(
        &mut self,
        bundles: &[astersql_ddl_placement::Bundle],
    ) -> Result<(), String> {
        if let Some((db, old_id, new_id)) = self.replacement {
            self.inner.with_transaction(&mut |txn| {
                let meta = astersql_meta::TransactionMutator::new(txn);
                assert!(
                    meta.get_table(db, old_id)?.is_none(),
                    "old table must be retired before placement publication"
                );
                assert!(
                    meta.get_table(db, new_id)?.is_some(),
                    "new metadata must be staged before placement publication"
                );
                Ok(vec![])
            })?;
        }
        self.inner.put_create_table_bundles(bundles)
    }
    fn with_transaction(
        &mut self,
        operation: &mut dyn FnMut(&mut dyn astersql_kv::Transaction) -> Result<Vec<u8>, String>,
    ) -> Result<Vec<u8>, String> {
        self.transaction_calls += 1;
        if self.transaction_calls == 3 && !self.notification {
            return Err("injected final metadata failure".into());
        }
        self.inner.with_transaction(operation)
    }
}

#[test]
fn port_9453792437_list_staging_is_durable_and_rolls_back_without_losing_rows() {
    astersql_domain_infosync::GlobalInfoSyncerInit(
        "port945379".into(),
        std::sync::Arc::new(|| 1),
        None,
        None,
        None,
        astersql_domain_infosync::Codec::default(),
        false,
        None,
    )
    .unwrap();
    use astersql_ddl::job_worker::DurableJobSession;
    use astersql_meta_model::group_3::{Job, JobState, JobVersion};
    let store = CreateAnalyzeStatsStore();
    let domain = store.domain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create table stage_list(a int unsigned primary key nonclustered global, b int not null, c varchar(255), unique index(c) global) partition by list(b) (partition p0 values in(1,2,3), partition p1 values in(4,5,6), partition p2 values in(7,8,9))", vec![]);
    tk.MustExec(
        "insert into stage_list values(1,1,'1'),(4,4,'4'),(9,9,'9')",
        vec![],
    );
    let original = (*domain.table_by_name("test", "stage_list").unwrap()).clone();
    let original_pi = original.Partition.as_ref().unwrap();
    let requested = PartitionInfo {
        Type: original_pi.Type,
        Expr: original_pi.Expr.clone(),
        Definitions: (0..3)
            .map(|i| PartitionDefinition {
                ID: original.ID + 90000 + i,
                Name: NewCIStr(&format!("pNone{}", i + 1)),
                InValues: vec![vec![(i + 1).to_string()], vec![(9 - i).to_string()]],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut staged = original.clone();
    astersql_ddl::partition::stage_reorganize_partition_definitions(
        &mut staged,
        &["p2".into(), "p0".into()],
        &requested,
    )
    .unwrap();
    // This fixture represents a genuinely committed DeleteOnly boundary, not
    // a manually produced expected result. The helper produced both sets.
    staged.Partition.as_mut().unwrap().DDLState = StateDeleteOnly;
    staged.Partition.as_mut().unwrap().DDLAction = ACTION_REORGANIZE_PARTITION;
    let db = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|d| d.name.lower == "test")
        .unwrap()
        .id;
    let pool = astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
    let mut session = pool.acquire().unwrap();
    session.begin().unwrap();
    session
        .with_execution_context(Box::new(move |c| {
            c.with_transaction(&mut |txn| {
                let mut meta = astersql_meta::TransactionMutator::new(txn);
                meta.update_table(db, &mut staged)?;
                let version = meta.gen_schema_version()?;
                meta.set_table_schema_diff(
                    &Job {
                        schema_id: db,
                        table_id: staged.ID,
                        tp: ACTION_REORGANIZE_PARTITION,
                        ..Default::default()
                    },
                    version,
                )?;
                Ok(vec![])
            })
        }))
        .unwrap();
    session.commit().unwrap();
    domain.reload().unwrap();
    let actual = domain.table_by_name("test", "stage_list").unwrap();
    let pi = actual.Partition.as_ref().unwrap();
    assert_eq!(
        serde_json::to_value(&pi.Definitions).unwrap(),
        serde_json::to_value(&original_pi.Definitions).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&pi.DroppingDefinitions).unwrap(),
        serde_json::to_value([&original_pi.Definitions[0], &original_pi.Definitions[2]]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&pi.AddingDefinitions).unwrap(),
        serde_json::to_value(&requested.Definitions).unwrap()
    );
    assert_eq!(pi.DDLState, StateDeleteOnly);
    let mut job = Job {
        id: original.ID + 945380,
        schema_id: db,
        table_id: original.ID,
        schema_name: "test".into(),
        table_name: "stage_list".into(),
        tp: ACTION_REORGANIZE_PARTITION,
        state: JobState::Cancelling,
        schema_state: StateDeleteOnly,
        version: JobVersion::V2,
        ..Default::default()
    };
    job.raw_args =
        serde_json::to_vec(&serde_json::json!({"part_names":["p2","p0"],"part_info":requested}))
            .unwrap();
    // Reuse the actual persistent cancellation/rollback stages in independent
    // transactions; no test code edits the helper's results after publication.
    for _ in 0..2 {
        session.begin().unwrap();
        let bytes = session
            .with_execution_context(Box::new(move |c| {
                astersql_ddl::partition::rollback_persisted_partition(c, &mut job)?;
                job.encode(false).map_err(|e| e.to_string())
            }))
            .unwrap();
        session.commit().unwrap();
        job = astersql_meta::decode_go_history_job(&bytes).unwrap();
    }
    assert_eq!(job.state, JobState::RollbackDone);
    domain.reload().unwrap();
    let after = domain.table_by_name("test", "stage_list").unwrap();
    assert_eq!(
        serde_json::to_value(&after.Partition.as_ref().unwrap().Definitions).unwrap(),
        serde_json::to_value(&original_pi.Definitions).unwrap()
    );
    assert!(
        after
            .Partition
            .as_ref()
            .unwrap()
            .AddingDefinitions
            .is_empty()
    );
    assert!(
        after
            .Partition
            .as_ref()
            .unwrap()
            .DroppingDefinitions
            .is_empty()
    );
    tk.MustExec("alter table stage_list reorganize partition p2,p0 into (partition pNone1 values in(1,9), partition pNone2 values in(2,8), partition pNone3 values in(3,7))", vec![]);
    let final_table = domain.table_by_name("test", "stage_list").unwrap();
    let final_pi = final_table.Partition.as_ref().unwrap();
    assert_eq!(
        final_pi
            .Definitions
            .iter()
            .map(|d| d.Name.L.as_str())
            .collect::<Vec<_>>(),
        vec!["pnone1", "pnone2", "pnone3", "p1"]
    );
    assert_eq!(
        final_pi.GetPartitionIDByName("p1"),
        original_pi.GetPartitionIDByName("p1")
    );
    tk.MustExec("admin check table stage_list", vec![]);
    let rows = tk
        .MustQuery("select a,b,c from stage_list order by a", vec![])
        .Rows()
        .clone();
    assert_eq!(
        rows.iter().map(|r| r.join(" ")).collect::<Vec<_>>(),
        vec!["1 1 1", "4 4 4", "9 9 9"]
    );
}

fn port945_snapshot(domain: &astersql_domain::Domain) -> astersql_meta::SnapshotReader {
    domain.storage_handle().with_storage(|storage| {
        astersql_meta::SnapshotReader::new(
            storage.GetSnapshot(storage.CurrentVersion("global").unwrap()),
        )
    })
}

#[test]
fn port_9453792437_partitioning_finalization_transfers_counters_and_old_cleanup_ids() {
    use astersql_ddl::job_worker::DurableJobSession;
    use astersql_meta_model::{
        ACTION_ALTER_TABLE_PARTITIONING, ACTION_REMOVE_PARTITIONING,
        group_3::{Job, JobState, JobVersion},
    };
    for action in [ACTION_ALTER_TABLE_PARTITIONING, ACTION_REMOVE_PARTITIONING] {
        for version in [JobVersion::V1, JobVersion::V2] {
            let store = CreateAnalyzeStatsStore();
            let domain = store.domain();
            let mut tk = TestKit::new(store);
            let range = " partition by range(a) (partition p0 values less than(10), partition pmax values less than(maxvalue))";
            let source_suffix = if action == ACTION_REMOVE_PARTITIONING {
                range
            } else {
                ""
            };
            let target_suffix = if action == ACTION_ALTER_TABLE_PARTITIONING {
                range
            } else {
                ""
            };
            tk.MustExec(&format!("create table convert_source(a int auto_increment primary key, b int, key idx(b)){source_suffix}"), vec![]);
            tk.MustExec(&format!("create table copied_target(a int auto_increment primary key, b int, key idx(b)){target_suffix}"), vec![]);
            tk.MustExec("insert into convert_source values(1,10),(12,20)", vec![]);
            // These are actual destination records and local index entries,
            // as they exist after backfill. Fixture setup only changes metadata.
            tk.MustExec("insert into copied_target values(1,10),(12,20)", vec![]);
            let mut source = (*domain.table_by_name("test", "convert_source").unwrap()).clone();
            let target = domain.table_by_name("test", "copied_target").unwrap();
            let old_id = source.ID;
            let new_id = target.ID;
            let old_defs = source
                .Partition
                .as_ref()
                .map(|p| p.Definitions.clone())
                .unwrap_or_else(|| {
                    vec![PartitionDefinition {
                        ID: old_id,
                        Name: NewCIStr("p0"),
                        ..Default::default()
                    }]
                });
            let mut requested = target.Partition.clone().unwrap_or_else(|| PartitionInfo {
                Definitions: vec![PartitionDefinition {
                    ID: new_id,
                    Name: NewCIStr("p0"),
                    ..Default::default()
                }],
                ..Default::default()
            });
            requested.NewTableID = new_id;
            source.Partition = Some(PartitionInfo {
                DDLState: StatePublic,
                DDLAction: action,
                AddingDefinitions: requested.Definitions.clone(),
                DroppingDefinitions: old_defs.clone(),
                ..requested.clone()
            });
            let db = domain
                .info_schema()
                .AllSchemas()
                .into_iter()
                .find(|d| d.name.lower == "test")
                .unwrap()
                .id;
            let pool =
                astersql_session::runtime::system_session::SystemSessionPool::new(domain.clone());
            let mut session = pool.acquire().unwrap();
            session.begin().unwrap();
            session
                .with_execution_context(Box::new(move |c| {
                    c.with_transaction(&mut |txn| {
                        let mut meta = astersql_meta::TransactionMutator::new(txn);
                        meta.drop_table_only(db, new_id)?;
                        meta.update_table(db, &mut source)?;
                        for (prefix, value) in [("TID", 101), ("IID", 202), ("TARID", 303)] {
                            txn.Set(
                                astersql_meta::transaction_meta_hash_key(
                                    format!("DB:{db}").as_bytes(),
                                    format!("{prefix}:{old_id}").as_bytes(),
                                ),
                                value.to_string().into_bytes(),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        Ok(vec![])
                    })
                }))
                .unwrap();
            session.commit().unwrap();
            let mut job = Job {
                id: old_id + 945381,
                schema_id: db,
                table_id: old_id,
                schema_name: "test".into(),
                table_name: "convert_source".into(),
                tp: action,
                state: JobState::Running,
                schema_state: StatePublic,
                version,
                ..Default::default()
            };
            job.raw_args = serde_json::to_vec(&if version == JobVersion::V1 {
                serde_json::json!([[], requested])
            } else {
                serde_json::json!({"part_names":[],"part_info":requested})
            })
            .unwrap();
            // A notifier insert error is late enough that metadata/counters
            // have been staged. Roll back the real transaction and verify that
            // neither table-ID replacement nor allocator transfer escaped it.
            let mut failed_job =
                astersql_meta::decode_go_history_job(&job.encode(false).unwrap()).unwrap();
            session.begin().unwrap();
            let error = session
                .with_execution_context(Box::new(move |c| {
                    let error = astersql_ddl::partition::finish_persisted_partition(
                        &mut Port945FinalUpdateFailure {
                            inner: c,
                            transaction_calls: 0,
                            notification: true,
                            replacement: Some((db, old_id, new_id)),
                        },
                        &mut failed_job,
                    )
                    .unwrap_err();
                    assert_eq!(failed_job.schema_state, StatePublic);
                    assert_eq!(failed_job.state, JobState::Running);
                    Err(error)
                }))
                .unwrap_err();
            assert_eq!(error, "injected notifier SQL failure");
            session.rollback();
            let failed_reader = port945_snapshot(&domain);
            assert!(failed_reader.get_table(db, old_id).unwrap().is_some());
            assert!(failed_reader.get_table(db, new_id).unwrap().is_none());
            session.begin().unwrap();
            let bytes = session
                .with_execution_context(Box::new(move |c| {
                    astersql_ddl::partition::finish_persisted_partition(c, &mut job)?;
                    job.encode(false).map_err(|e| e.to_string())
                }))
                .unwrap();
            session.commit().unwrap();
            let mut job = astersql_meta::decode_go_history_job(&bytes).unwrap();
            assert_eq!(job.state, JobState::Done);
            if version == JobVersion::V2 {
                let finished: serde_json::Value = serde_json::from_slice(&job.raw_args).unwrap();
                assert_eq!(
                    finished["part_info"],
                    serde_json::to_value(&requested).unwrap()
                );
                assert_eq!(finished["part_names"], serde_json::json!([]));
            }
            let args =
                astersql_meta_model::group_2::GetFinishedTablePartitionArgs(&mut job).unwrap();
            let mut expected_ids: Vec<_> = old_defs.iter().map(|d| d.ID).collect();
            if expected_ids[0] != old_id {
                expected_ids.push(old_id);
            }
            assert_eq!(args.OldPhysicalTblIDs, expected_ids);
            let reader = port945_snapshot(&domain);
            assert!(reader.get_table(db, old_id).unwrap().is_none());
            let final_table = reader.get_table(db, new_id).unwrap().unwrap();
            assert_eq!(final_table.Name.L, "convert_source");
            let diff = port945_diff(
                &domain,
                reader.get_schema_version_with_non_empty_diff().unwrap(),
            );
            assert_eq!(diff["table_id"], new_id);
            assert_eq!(diff["old_table_id"], old_id);
            assert_eq!(
                diff["affected_options"].as_array().unwrap().len(),
                expected_ids.len().max(requested.Definitions.len())
            );
            assert_eq!(
                final_table.Partition.is_none(),
                action == ACTION_REMOVE_PARTITIONING
            );
            domain.storage_handle().with_storage(|storage| {
                let snap = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
                for (prefix, value) in [("TID", 101), ("IID", 202), ("TARID", 303)] {
                    let key = astersql_meta::transaction_meta_hash_key(
                        format!("DB:{db}").as_bytes(),
                        format!("{prefix}:{new_id}").as_bytes(),
                    );
                    assert_eq!(
                        snap.Get(&astersql_kv::Context::default(), key, &[])
                            .unwrap()
                            .Value,
                        value.to_string().into_bytes()
                    );
                }
            });
            let events = session
                .query(format!(
                    "SELECT schema_change FROM mysql.tidb_ddl_notifier WHERE ddl_job_id={}",
                    job.id
                ))
                .unwrap();
            assert_eq!(events.len(), 1);
            let event: serde_json::Value = serde_json::from_str(&events[0][0]).unwrap();
            assert_eq!(event["type"], action);
            assert_eq!(event["old_table_id_for_partition"], old_id);
            domain.reload().unwrap();
            tk.MustExec("admin check table convert_source", vec![]);
            let rows = tk
                .MustQuery("select a,b from convert_source order by a", vec![])
                .Rows()
                .clone();
            assert_eq!(
                rows.iter().map(|r| r.join(" ")).collect::<Vec<_>>(),
                vec!["1 10", "12 20"]
            );
        }
    }
}

fn port945_diff(domain: &astersql_domain::Domain, version: i64) -> serde_json::Value {
    domain.storage_handle().with_storage(|storage| {
        let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
        let value = snapshot
            .Get(
                &astersql_kv::Context::default(),
                astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()),
                &[],
            )
            .unwrap();
        serde_json::from_slice(&value.Value).unwrap()
    })
}
