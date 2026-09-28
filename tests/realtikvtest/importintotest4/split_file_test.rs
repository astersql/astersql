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

// Exercise SQL execution, production CSV region planning and KV-backed indexes.
use astersql_lightning_mydump::test_support::MemoryStorage;
use astersql_session::runtime::{BootstrapCanonicalDomain, ConcreteSession};
use astersql_session::testutil::TestRecordSet;
use std::sync::Arc;

fn query(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut sets = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    assert_eq!(
        sets.len(),
        1,
        "IMPORT/query must return one record set: {sql}"
    );
    let mut set = sets.pop().unwrap();
    let mut rows = Vec::new();
    while let Some(row) = set.Next().unwrap() {
        rows.push(row);
    }
    rows
}

struct RegionSize<'a>(&'a ConcreteSession, i64);
impl<'a> RegionSize<'a> {
    fn set(session: &'a ConcreteSession, value: i64) -> Self {
        Self(session, session.SetImportRegionSize(value).unwrap())
    }
}
impl Drop for RegionSize<'_> {
    fn drop(&mut self) {
        self.0.SetImportRegionSize(self.1).unwrap();
    }
}

struct TestDatabase<'a>(&'a ConcreteSession, String);
impl Drop for TestDatabase<'_> {
    fn drop(&mut self) {
        let _ = self
            .0
            .execute(&format!("drop database if exists {}", self.1));
    }
}

#[test]
fn test_split_file() {
    let store = astersql_store_driver::TiKVDriver::default()
        .Open(&format!(
            "tikv://{}?disableGC=true",
            std::env::var("REAL_TIKV_PD").unwrap_or_else(|_| "127.0.0.1:12379".into())
        ))
        .expect("TestSplitFile requires real TiKV, like Go mockGCSSuite");
    let domain = Arc::new(astersql_domain::Domain::new(
        store,
        Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        astersql_domain::DomainConfig::default(),
    ));
    domain.init().unwrap();
    let session = BootstrapCanonicalDomain(domain.clone()).unwrap();
    check_split_file(&session, true);
    domain.close();
}

#[test]
fn split_file_with_canonical_kv() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    check_split_file(&session, false);
    domain.close();
}

fn check_split_file(session: &ConcreteSession, real_tikv: bool) {
    let storage = Arc::new(MemoryStorage::default());
    let source = "gs://split-file/1.csv?endpoint=http://127.0.0.1:4443";
    let content: String = (0..500).map(|j| format!("{j},test-{j}\n")).collect();
    let mut all_data: Vec<String> = (0..500).map(|j| format!("{j} test-{j}")).collect();
    all_data.sort();
    storage.insert(source, content.as_bytes());
    session.SetImportFileStorage(storage.clone());
    let database = format!(
        "split_file_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    session
        .execute(&format!("create database {database}"))
        .unwrap();
    let _database = TestDatabase(session, database.clone());
    session.execute(&format!("use {database}")).unwrap();
    session
        .execute("create table t (a bigint primary key, b varchar(100))")
        .unwrap();
    let _region_size = RegionSize::set(&session, content.len().div_ceil(3) as i64);
    let result = query(
        &session,
        &format!(
            "import into t from '{source}' with split_file, lines_terminated_by='\\n', __max_engine_size='1'"
        ),
    );
    assert_eq!(result.len(), 1);
    let physical = session.LastImportSSTStats();
    assert!(physical.keys >= 500);
    if real_tikv {
        assert!(
            physical.ingest_rpcs >= 3,
            "IMPORT must use real MultiIngest: {physical:?}"
        );
        assert!(
            physical.write_rpcs >= physical.ingest_rpcs,
            "each ingested SST must have been written to the TiKV store: {physical:?}"
        );
    }
    let job_id: i64 = result[0][0].parse().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    let persisted = manager
        .GetTaskByKeyWithHistory((), astersql_dxf_importinto::TaskKey(job_id))
        .unwrap();
    let persisted_subtasks = manager
        .GetSubtasksWithHistory(
            (),
            persisted.ID,
            astersql_dxf_framework_proto::ImportStepImport,
        )
        .unwrap()
        .unwrap();
    assert_eq!(persisted_subtasks.len(), 3);
    let metadata = astersql_dxf_importinto::TaskMeta::Unmarshal(&persisted.Meta).unwrap();
    assert_eq!(metadata.JobID, job_id);
    assert_eq!(metadata.Plan.Path, source);
    assert!(metadata.Plan.SplitFile);
    assert_eq!(metadata.Summary.ImportedRows, 500);
    let mut chunks = metadata.ChunkMap.values().flatten().collect::<Vec<_>>();
    chunks.sort_by_key(|chunk| chunk.Offset);
    assert_eq!(chunks.first().unwrap().Offset, 0);
    assert_eq!(chunks.last().unwrap().EndOffset, content.len() as i64);
    assert!(
        chunks
            .windows(2)
            .all(|pair| pair[0].EndOffset == pair[1].Offset)
    );
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks.first().unwrap().PrevRowIDMax, 0);
    for chunk in &chunks {
        assert_eq!(
            chunk.RowIDMax - chunk.PrevRowIDMax,
            (chunk.EndOffset - chunk.Offset) / 2,
            "row-ID estimates must use the two target columns, as in Go"
        );
    }
    for subtask in &persisted_subtasks {
        let step: serde_json::Value = serde_json::from_slice(&subtask.Meta).unwrap();
        assert_eq!(step["ID"].as_i64(), Some(i64::from(subtask.Ordinal - 1)));
        assert!(!step["Chunks"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        persisted.State,
        astersql_dxf_framework_storage::proto::TaskStateSucceed
    );
    // The active rows must have been moved, not copied into a session cache.
    assert!(manager.GetTaskByID((), persisted.ID).is_err());
    let task = session
        .ImportFileTaskByKeyWithHistory(&astersql_dxf_importinto::TaskKey(job_id))
        .unwrap();
    assert_eq!(task.state, "succeed");
    assert_eq!(
        task.subtasks
            .iter()
            .filter(|subtask| subtask.step == astersql_dxf_framework_proto::ImportStepImport)
            .count(),
        3
    );
    assert_eq!(
        task.subtasks.iter().map(|task| task.rows).sum::<usize>(),
        500
    );
    assert!(task.subtasks.iter().all(|task| task.state == "succeed"));
    let mut actual: Vec<String> = query(&session, "select * from t")
        .iter()
        .map(|row| row.join(" "))
        .collect();
    actual.sort();
    assert_eq!(actual, all_data);

    session.execute("truncate table t").unwrap();
    query(
        &session,
        &format!(
            "import into t from '{source}' with split_file, lines_terminated_by='\\n', skip_rows=1, __max_engine_size='1'"
        ),
    );
    let mut actual: Vec<String> = query(&session, "select * from t")
        .iter()
        .map(|row| row.join(" "))
        .collect();
    actual.sort();
    assert_eq!(actual, all_data[1..]);
    session.execute("truncate table t").unwrap();
    query(
        session,
        &format!(
            "import into t from '{source}' with split_file, lines_terminated_by='\\n', skip_rows=2, __max_engine_size='1'"
        ),
    );
    let mut two_skipped: Vec<_> = query(session, "select * from t")
        .iter()
        .map(|row| row.join(" "))
        .collect();
    two_skipped.sort();
    assert_eq!(two_skipped, all_data[2..]);

    session
        .execute("create table t2 (a int primary key nonclustered, b varchar(100))")
        .unwrap();
    let source = "gs://split-file/2.csv?endpoint=http://127.0.0.1:4443";
    storage.insert(source, b"1,2\r\n3,4\r\n5,6\r\n7,8\r\n9,10\r\n");
    session.SetImportRegionSize(9).unwrap();
    query(
        &session,
        &format!("import into t2 from '{source}' with split_file, lines_terminated_by='\\r\\n'"),
    );
    assert_eq!(
        query(&session, "select * from t2 order by a"),
        vec![
            vec!["1", "2"],
            vec!["3", "4"],
            vec!["5", "6"],
            vec!["7", "8"],
            vec!["9", "10"]
        ]
    );
    session.execute("admin check table t2").unwrap();
    // Go SplitLargeCSV aligns the 9/18-byte split points to CRLF ends at
    // 15/20, and estimates each chunk's row-ID range as bytes / columns.
    // The three chunks therefore start at row IDs 1, 8 and 10.
    assert_eq!(
        query(session, "select _tidb_rowid, a from t2 order by a"),
        vec![
            vec!["1", "1"],
            vec!["2", "3"],
            vec!["3", "5"],
            vec!["8", "7"],
            vec!["10", "9"]
        ]
    );
    assert!(
        session
            .execute("insert into t2 values(1, 'duplicate')")
            .is_err(),
        "nonclustered primary index must reject duplicates with a new allocated handle"
    );
    // Storage errors and uniqueness violations must propagate through SQL;
    // neither path may manufacture a successful task or overwrite table rows.
    assert!(
        session
            .execute("import into t2 from 'gs://split-file/missing.csv' with split_file")
            .is_err()
    );
    assert!(
        session
            .execute(&format!(
                "import into t2 from '{source}' with split_file, lines_terminated_by='\\r\\n'"
            ))
            .is_err()
    );
    let failed_job = query(
        session,
        &format!(
            "select id from mysql.tidb_import_jobs where table_schema='{database}' and status='failed' order by id desc limit 1"
        ),
    );
    assert_eq!(failed_job.len(), 1);
    let failed = session
        .ImportFileTaskByKeyWithHistory(&astersql_dxf_importinto::TaskKey(
            failed_job[0][0].parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(
        failed
            .subtasks
            .iter()
            .any(|subtask| subtask.state == "failed")
    );
    session.execute("admin check table t2").unwrap();
    drop(_region_size);
    assert_eq!(
        session.SetImportRegionSize(96 * 1024 * 1024).unwrap(),
        96 * 1024 * 1024
    );
}

#[test]
fn task_history_uses_independent_sql_sessions_and_real_transactions() {
    use astersql_dxf_framework_storage as dxf;
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let manager = session.ImportTaskManager().unwrap();
    dxf::SetNodeResource(dxf::proto::NewNodeResource(1, 0, 0));
    manager
        .InitMeta((), "test-executor".into(), "".into())
        .unwrap();
    let id = manager
        .CreateTask(
            (),
            "persistent-import".into(),
            dxf::proto::ImportInto,
            "".into(),
            1,
            "".into(),
            1,
            dxf::proto::ExtraParams::default(),
            b"{}".to_vec(),
        )
        .unwrap();
    drop(manager);
    let other = ConcreteSession::new(domain.clone());
    let manager = other.ImportTaskManager().unwrap();
    let task = manager
        .GetTaskByKeyWithHistory((), "persistent-import".into())
        .unwrap();
    assert_eq!(task.ID, id);
    assert_eq!(task.State, dxf::proto::TaskStatePending);
    assert_eq!(task.Meta, b"{}");
    let failure = manager.WithNewTxn((), |se| {
        dxf::sqlexec::ExecSQL(
            (),
            se.GetSQLExecutor(),
            "delete from mysql.tidb_global_task where id=%?",
            vec![id.into()],
        )?;
        Err(dxf::Error::new("abort transaction"))
    });
    assert!(failure.is_err());
    assert_eq!(manager.GetTaskByIDWithHistory((), id).unwrap().ID, id);
    manager
        .SwitchTaskStep(
            (),
            task,
            dxf::proto::TaskStateRunning,
            1,
            vec![dxf::proto::Subtask {
                SubtaskBase: dxf::proto::SubtaskBase {
                    TaskID: id,
                    Step: 1,
                    Type: dxf::proto::ImportInto,
                    ExecID: "test-executor".into(),
                    Ordinal: 1,
                    Concurrency: 1,
                    ..Default::default()
                },
                Meta: b"{}".to_vec(),
                ..Default::default()
            }],
        )
        .unwrap();
    let subtask = manager
        .GetSubtasksWithHistory((), id, 1)
        .unwrap()
        .unwrap()
        .remove(0);
    assert!(
        manager
            .StartSubtask((), subtask.ID, "wrong-owner".into())
            .is_err()
    );
    manager
        .StartSubtask((), subtask.ID, "test-executor".into())
        .unwrap();
    manager
        .FinishSubtask((), "test-executor".into(), subtask.ID, b"{}".to_vec())
        .unwrap();
    manager.SucceedTask((), id).unwrap();
    manager
        .TransferTasks2History((), vec![manager.GetTaskByID((), id).unwrap()])
        .unwrap();
    drop(manager);
    let manager = session.ImportTaskManager().unwrap();
    assert!(manager.GetTaskByID((), id).is_err());
    assert_eq!(manager.GetTaskByIDWithHistory((), id).unwrap().Meta, b"{}");
    assert_eq!(
        manager.GetTaskByIDWithHistory((), id).unwrap().State,
        dxf::proto::TaskStateSucceed
    );
    assert_eq!(
        manager.GetSubtasksWithHistory((), id, 1).unwrap().unwrap()[0].State,
        dxf::proto::SubtaskStateSucceed
    );
    domain.close();
}
