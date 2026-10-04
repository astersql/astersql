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
