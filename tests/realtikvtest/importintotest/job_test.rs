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

//! Go job_test.go parity: observe persisted jobs at the same scheduler,
//! subtask and cancellation boundaries, including the complete SHOW rows.
use astersql_tests_realtikvtest_importintotest::harness::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

fn fmap() -> &'static std::collections::HashMap<String, usize> {
    plannercore::ImportIntoFieldMap()
}
fn field<'a>(row: &'a [String], name: &str) -> &'a str {
    &row[fmap()[name]]
}
fn auth_as(s: &MockGCSSuite, user: &str, host: &str) {
    let mut session = s.tk.Session();
    session
        .Auth(&auth::UserIdentity {
            Username: user.into(),
            Hostname: host.into(),
        })
        .unwrap();
    s.tk.set_session(session);
}
fn object(s: &MockGCSSuite, bucket: &str, name: &str, content: &str) {
    s.server.CreateObject(fakestorage::Object {
        ObjectAttrs: fakestorage::ObjectAttrs {
            BucketName: bucket.into(),
            Name: name.into(),
        },
        Content: content.as_bytes().to_vec(),
    });
}
fn source(bucket: &str, name: &str) -> String {
    format!("gs://{bucket}/{name}?endpoint={}", gcs_endpoint())
}
fn show(s: &MockGCSSuite, id: i64) -> Vec<String> {
    let rows = s.tk.MustQuery(&format!("show import job {id}")).Rows();
    assert_eq!(rows.len(), 1);
    rows[0].clone()
}
fn wait_status(s: &MockGCSSuite, id: i64, status: &str) {
    s.Eventually(
        || field(&show(s, id), "Status") == status,
        max_wait_time() * 3,
        Duration::from_millis(10),
    );
}
fn job(
    s: &MockGCSSuite,
    id: i64,
    db: &str,
    table: &str,
    bucket: &str,
    file: &str,
    user: &str,
) -> importer::JobInfo {
    importer::JobInfo {
        ID: id,
        TableSchema: db.into(),
        TableName: table.into(),
        TableID: session_api::GetDomain(&s.store)
            .unwrap()
            .MustGetTableID(&s.t, db, table),
        CreatedBy: user.into(),
        Parameters: importer::ImportParameters {
            FileLocation: source(bucket, file),
            Format: importer::DataFormatCSV.into(),
            ..Default::default()
        },
        SourceFileSize: 3,
        Status: "pending".into(),
        Step: String::new(),
        Summary: None,
        ErrorMessage: String::new(),
        GroupKey: String::new(),
    }
}
fn compare_job_info_without_time(s: &MockGCSSuite, j: &importer::JobInfo, row: &[String]) {
    assert_eq!(field(row, "JobID"), j.ID.to_string());
    let mut expected = url::Url::parse(&j.Parameters.FileLocation).unwrap();
    let mut actual = url::Url::parse(field(row, "DataSource")).unwrap();
    let query = |u: &url::Url| {
        let mut result: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in u.query_pairs() {
            result
                .entry(k.into_owned())
                .or_default()
                .push(v.into_owned());
        }
        result
    };
    assert_eq!(query(&expected), query(&actual));
    expected.set_query(None);
    actual.set_query(None);
    assert_eq!(expected, actual);
    assert_eq!(
        field(row, "TargetTable"),
        utils_br::EncloseDBAndTable(&j.TableSchema, &j.TableName)
    );
    assert_eq!(field(row, "TableID"), j.TableID.to_string());
    assert_eq!(field(row, "Phase"), j.Step);
    assert_eq!(field(row, "Status"), j.Status);
    assert_eq!(
        field(row, "SourceFileSize"),
        units::BytesSize(j.SourceFileSize as f64)
    );
    assert_eq!(
        field(row, "ImportedRows"),
        j.Summary
            .as_ref()
            .map(|s| s.ImportedRows.to_string())
            .unwrap_or_else(|| "<nil>".into())
    );
    s.Regexp(&j.ErrorMessage, field(row, "ResultMessage"));
    assert_eq!(field(row, "CreatedBy"), j.CreatedBy);
}

// Go channels/Once: bounded reach wait, idempotent release and cleanup release.
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, bool)>,
    wake: Condvar,
}
impl Gate {
    fn enter(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.wake.notify_all();
        while !state.1 {
            state = self.wake.wait(state).unwrap();
        }
    }
    fn reached(&self) {
        let state = self.state.lock().unwrap();
        let (state, timeout) = self
            .wake
            .wait_timeout_while(state, max_wait_time(), |s| !s.0)
            .unwrap();
        assert!(
            state.0 && !timeout.timed_out(),
            "timeout waiting for scheduler boundary"
        );
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.wake.notify_all();
    }
}
fn gate(s: &MockGCSSuite, path: &str, job_id: Option<i64>) -> Arc<Gate> {
    let gate = Arc::new(Gate::default());
    let callback = gate.clone();
    testfailpoint::EnableCall(&s.t, path, move |ctx| {
        let matches = match (&job_id, ctx) {
            (None, _) => true,
            (Some(id), FailCtx::JobID(got)) => *id == got,
            (Some(id), FailCtx::Task(t)) => t.lock().unwrap().Key == importinto::TaskKey(*id),
            _ => false,
        };
        if matches {
            callback.enter();
        }
    });
    let release = gate.clone();
    s.t.Cleanup(move || release.release());
    gate
}
fn block_scheduler(s: &MockGCSSuite) -> Arc<Gate> {
    let gate = gate(
        s,
        "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/beforeGetSchedulableTasks",
        None,
    );
    gate.reached();
    gate
}
fn restore_auth(s: &MockGCSSuite) {
    let tk = s.tk.clone();
    s.t.Cleanup(move || {
        let mut session = tk.Session();
        session
            .Auth(&auth::UserIdentity {
                Username: "root".into(),
                Hostname: "localhost".into(),
            })
            .unwrap();
        tk.set_session(session);
    });
}
fn users(s: &MockGCSSuite, db: &str, prefix: &str) {
    restore_auth(s);
    for i in [1, 2] {
        s.tk.MustExec(&format!("DROP USER IF EXISTS '{prefix}{i}'@'localhost'"));
        s.tk.MustExec(&format!("CREATE USER '{prefix}{i}'@'localhost'"));
        s.tk.MustExec(&format!(
            "GRANT SELECT,UPDATE,INSERT,DELETE,ALTER on {db}.* to '{prefix}{i}'@'localhost'"
        ));
    }
}
fn nextgen(s: &MockGCSSuite) {
    let prior = kerneltype::IsNextGen();
    s.t.Cleanup(move || kerneltype::set_next_gen(prior));
    kerneltype::set_next_gen(true);
}

#[test]
fn test_show_job() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.tk.MustExec("delete from mysql.tidb_import_jobs");
    s.prepare_and_use_db("test_show_job");
    for t in ["t1", "t2", "t3"] {
        s.tk.MustExec(&format!("CREATE TABLE {t} (i INT PRIMARY KEY)"));
    }
    object(&s, "test-show-job", "t.csv", "1\n2");
    users(&s, "test_show_job", "test_show_job");
    s.ErrorIs(
        &s.tk.QueryToErr("show import job 9999999999").unwrap_err(),
        exeerrors::ErrLoadDataJobNotFound,
    );
    for path in [
        "github.com/pingcap/tidb/pkg/executor/importer/setLastImportJobID",
        "github.com/pingcap/tidb/pkg/dxf/framework/storage/testSetLastTaskID",
        "github.com/pingcap/tidb/pkg/parser/ast/forceRedactURL",
    ] {
        testfailpoint::Enable(&s.t, path, "return(true)");
    }
    auth_as(&s, "test_show_job1", "localhost");
    let result1=s.tk.MustQuery(&format!("import into t1 FROM 'gs://test-show-job/t.csv?access-key=aaaaaa&secret-access-key=bbbbbb&endpoint={}'",gcs_endpoint())).Rows();
    assert_eq!(result1.len(), 1);
    s.tk.MustQuery("select * from t1")
        .Check(&testkit::Rows(&["1", "2"]));
    let id1 = importer::TestLastImportJobID.load(Ordering::SeqCst);
    assert_eq!(result1, vec![show(&s, id1)]);
    let mut expected = job(
        &s,
        id1,
        "test_show_job",
        "t1",
        "test-show-job",
        "t.csv",
        "test_show_job1@localhost",
    );
    expected.Parameters.FileLocation = format!(
        "gs://test-show-job/t.csv?access-key=xxxxxx&secret-access-key=xxxxxx&endpoint={}",
        gcs_endpoint()
    );
    expected.Status = "finished".into();
    expected.Summary = Some(importer::Summary { ImportedRows: 2 });
    compare_job_info_without_time(&s, &expected, &result1[0]);
    auth_as(&s, "test_show_job2", "localhost");
    let result2 =
        s.tk.MustQuery(&format!(
            "import into t2 FROM '{}'",
            source("test-show-job", "t.csv")
        ))
        .Rows();
    assert_eq!(result2.len(), 1);
    s.tk.MustQuery("select * from t2")
        .Check(&testkit::Rows(&["1", "2"]));
    let id2 = importer::TestLastImportJobID.load(Ordering::SeqCst);
    assert_eq!(result2, vec![show(&s, id2)]);
    expected = job(
        &s,
        id2,
        "test_show_job",
        "t2",
        "test-show-job",
        "t.csv",
        "test_show_job2@localhost",
    );
    expected.Status = "finished".into();
    expected.Summary = Some(importer::Summary { ImportedRows: 2 });
    compare_job_info_without_time(&s, &expected, &result2[0]);
    assert_eq!(s.tk.MustQuery("show import jobs").Rows(), result2);
    let check_jobs = |rows: Vec<Vec<String>>| {
        assert!(rows.len() >= 2);
        let mut matched = 0;
        for row in rows {
            for expected in [&result1[0], &result2[0]] {
                if row[0] == expected[0] {
                    assert_eq!(&row, expected);
                    matched += 1;
                }
            }
        }
        assert_eq!(matched, 2);
    };
    auth_as(&s, "root", "localhost");
    check_jobs(s.tk.MustQuery("show import jobs").Rows());
    assert_eq!(vec![show(&s, id2)], result2);
    compare_job_info_without_time(&s, &expected, &show(&s, id2));
    s.tk.MustExec("GRANT SUPER on *.* to 'test_show_job2'@'localhost'");
    auth_as(&s, "test_show_job2", "localhost");
    check_jobs(s.tk.MustQuery("show import jobs").Rows());
    let counter = Arc::new(AtomicI32::new(0));
    let count = counter.clone();
    let tk = testkit::NewTestKit(&s.t, s.store.clone());
    let ctx = s.t.clone();
    let mut running = job(
        &s,
        0,
        "test_show_job",
        "t3",
        "test-show-job",
        "t*.csv",
        "test_show_job2@localhost",
    );
    running.SourceFileSize = 6;
    running.Status = "running".into();
    running.Step = "importing".into();
    running.Parameters.FileLocation = format!(
        "gs://test-show-job/t*.csv?access-key=xxxxxx&secret-access-key=xxxxxx&endpoint={}",
        gcs_endpoint()
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let snapshots = observed.clone();
    testfailpoint::EnableCall(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/syncAfterSubtaskFinish",
        move |_| {
            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
            let rows = tk
                .MustQuery(&format!(
                    "show import job {}",
                    importer::TestLastImportJobID.load(Ordering::SeqCst)
                ))
                .Rows();
            assert_eq!(rows.len(), 1);
            snapshots.lock().unwrap().push(rows[0].clone());
            if n == 1 {
                let procs = tk.MustQuery("show full processlist").Rows();
                let mut got = false;
                for row in procs {
                    if row[1] == "test_show_job2" && row[7].contains("IMPORT INTO") {
                        assert!(
                            row[7].contains("access-key=xxxxxx")
                                && row[7].contains("secret-access-key=xxxxxx")
                        );
                        assert!(!row[7].contains("aaaaaa") && !row[7].contains("bbbbbb"));
                        got = true;
                    }
                }
                assert!(got);
            }
            if n == 2 {
                failpoint::Disable(
                    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/syncAfterSubtaskFinish",
                )
                .unwrap();
            }
            assert!(!ctx.Failed());
        },
    );
    object(&s, "test-show-job", "t2.csv", "3\n4");
    s.tk.MustQuery(&format!("import into t3 FROM 'gs://test-show-job/t*.csv?access-key=aaaaaa&secret-access-key=bbbbbb&endpoint={}' with thread=1, __max_engine_size='1'",gcs_endpoint()));
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    running.ID = importer::TestLastImportJobID.load(Ordering::SeqCst);
    for (i, row) in observed.lock().unwrap().iter().enumerate() {
        running.Summary = Some(importer::Summary {
            ImportedRows: (i as i64 + 1) * 2,
        });
        compare_job_info_without_time(&s, &running, row);
    }
    s.tk.MustQuery("select * from t3")
        .Sort()
        .Check(&testkit::Rows(&["1", "2", "3", "4"]));
    s.tear_down();
}

#[test]
fn test_show_detached_job() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("show_detached_job");
    for t in ["t1", "t2", "t3"] {
        s.tk.MustExec(&format!("CREATE TABLE {t} (i INT PRIMARY KEY)"));
    }
    object(&s, "test-show-detached-job", "t.csv", "1\n2");
    object(&s, "test-show-detached-job", "t2.csv", "1\n1");
    auth_as(&s, "root", "localhost");
    for (table, file, phase, error) in [
        ("t1", "t.csv", "", ""),
        (
            "t2",
            "t2.csv",
            importer::JobStepValidating,
            r"\[Lighting:Restore:ErrChecksumMismatch]checksum mismatched remote vs local.*",
        ),
        (
            "t3",
            "t.csv",
            importer::JobStepImporting,
            "occur an error when sort chunk.*",
        ),
    ] {
        if table == "t3" {
            testfailpoint::Enable(
                &s.t,
                "github.com/pingcap/tidb/pkg/dxf/importinto/errorWhenSortChunk",
                "return(true)",
            );
        }
        let rows =
            s.tk.MustQuery(&format!(
                "import into {table} FROM '{}' with detached",
                source("test-show-detached-job", file)
            ))
            .Rows();
        assert_eq!(rows.len(), 1);
        let id = rows[0][0].parse().unwrap();
        let mut expected = job(
            &s,
            id,
            "show_detached_job",
            table,
            "test-show-detached-job",
            file,
            "root@%",
        );
        compare_job_info_without_time(&s, &expected, &rows[0]);
        expected.Status = if error.is_empty() {
            "finished"
        } else {
            "failed"
        }
        .into();
        wait_status(&s, id, &expected.Status);
        expected.Step = phase.into();
        expected.ErrorMessage = error.into();
        if error.is_empty() {
            expected.Summary = Some(importer::Summary { ImportedRows: 2 });
            s.tk.MustQuery("select * from t1")
                .Check(&testkit::Rows(&["1", "2"]));
        }
        compare_job_info_without_time(&s, &expected, &show(&s, id));
    }
    s.tear_down();
}

#[test]
fn test_show_import_job_timing_around_prepare() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    nextgen(&s);
    s.prepare_and_use_db("show_job_prepare_timing");
    s.tk.MustExec("CREATE TABLE t (i INT PRIMARY KEY)");
    object(&s, "show-job-prepare-timing-source", "t.csv", "1\n2");
    object(&s, "show-job-prepare-timing-sort", "seed", "seed");
    let scheduler = block_scheduler(&s);
    let rows =
        s.tk.MustQuery(&format!(
            "IMPORT INTO t FROM '{}' WITH DETACHED, cloud_storage_uri='{}'",
            source("show-job-prepare-timing-source", "t.csv"),
            source("show-job-prepare-timing-sort", "path")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id = rows[0][0].parse().unwrap();
    assert_eq!(vec![show(&s, id)], rows);
    assert_eq!(field(&rows[0], "SourceFileSize"), "N/A");
    let before = gate(
        &s,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeJobStarted",
        Some(id),
    );
    let prepared = gate(
        &s,
        "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/afterTaskPrepared",
        Some(id),
    );
    scheduler.release();
    before.reached();
    assert_eq!(vec![show(&s, id)], rows);
    before.release();
    prepared.reached();
    let current = show(&s, id);
    assert_eq!(field(&current, "JobID"), id.to_string());
    assert_eq!(field(&current, "Status"), "running");
    assert_eq!(field(&current, "Phase"), importer::JobStepPreparing);
    assert_eq!(field(&current, "CreatedBy"), field(&rows[0], "CreatedBy"));
    assert_eq!(field(&current, "SourceFileSize"), "3B");
    prepared.release();
    wait_status(&s, id, "finished");
    let mut expected = job(
        &s,
        id,
        "show_job_prepare_timing",
        "t",
        "show-job-prepare-timing-source",
        "t.csv",
        field(&rows[0], "CreatedBy"),
    );
    expected.Status = "finished".into();
    expected.Summary = Some(importer::Summary { ImportedRows: 2 });
    compare_job_info_without_time(&s, &expected, &show(&s, id));
    s.tk.MustQuery("SELECT * FROM t ORDER BY i")
        .Check(&testkit::Rows(&["1", "2"]));
    s.tear_down();
}

#[test]
fn test_precheck_failure_on_prepare_is_not_retried() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    nextgen(&s);
    s.prepare_and_use_db("prepare_precheck_no_retry");
    s.tk.MustExec("CREATE TABLE t (i INT PRIMARY KEY)");
    object(&s, "prepare-precheck-no-retry-source", "t.csv", "");
    object(&s, "prepare-precheck-no-retry-sort", "seed", "seed");
    let rows =
        s.tk.MustQuery(&format!(
            "IMPORT INTO t FROM '{}' WITH DETACHED, cloud_storage_uri='{}'",
            source("prepare-precheck-no-retry-source", "t.csv"),
            source("prepare-precheck-no-retry-sort", "path")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id = rows[0][0].parse().unwrap();
    s.Eventually(
        || {
            let row = show(&s, id);
            field(&row, "Status") == "failed" && field(&row, "Phase") == importer::JobStepPreparing
        },
        max_wait_time(),
        Duration::from_millis(10),
    );
    s.Regexp("the file is empty", field(&show(&s, id), "ResultMessage"));
    s.Eventually(
        || s.get_task_by_job_id((), id).State == proto::TaskStateReverted,
        max_wait_time(),
        Duration::from_millis(10),
    );
    s.tear_down();
}

#[test]
fn test_detached_job_without_prepare_mode_still_succeeds() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    nextgen(&s);
    let db = format!(
        "detached_job_without_prepare_mode_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    s.prepare_and_use_db(&db);
    let tk = s.tk.clone();
    s.t.Cleanup(move || tk.MustExec(&format!("drop database if exists {db}")));
    s.tk.MustExec("CREATE TABLE t (i INT PRIMARY KEY)");
    object(
        &s,
        "detached-job-without-prepare-mode-source",
        "t.csv",
        "1\n2",
    );
    object(&s, "detached-job-without-prepare-mode-sort", "seed", "seed");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/mockDisableAsyncPrepare",
        "return",
    );
    let scheduler = block_scheduler(&s);
    assert!(!importinto::ShouldUseAsyncPrepare(&importer::Plan {
        CloudStorageURI: "gs://mock-bucket/mock-path".into(),
        ..Default::default()
    }));
    let rows =
        s.tk.MustQuery(&format!(
            "IMPORT INTO t FROM '{}' WITH DETACHED, cloud_storage_uri='{}'",
            source("detached-job-without-prepare-mode-source", "t.csv"),
            source("detached-job-without-prepare-mode-sort", "path")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id = rows[0][0].parse().unwrap();
    let key = importinto::TaskKey(id);
    s.tk.MustQuery(&format!("select json_extract(extra_params, '$.prepare_mode') is null from mysql.tidb_global_task where task_key = '{key}'")).Check(&testkit::Rows(&["1"]));
    scheduler.release();
    wait_status(&s, id, "finished");
    assert_eq!(field(&show(&s, id), "Phase"), "");
    s.tk.MustQuery("SELECT * FROM t ORDER BY i")
        .Check(&testkit::Rows(&["1", "2"]));
    s.tear_down();
}

#[test]
fn test_cancel_job() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("test_cancel_job");
    for table in ["t1", "t2"] {
        s.tk.MustExec(&format!("CREATE TABLE {table} (i INT PRIMARY KEY)"));
    }
    object(&s, "test_cancel_job", "t.csv", "1\n2");
    users(&s, "test_cancel_job", "test_cancel_job");
    s.ErrorIs(
        &s.tk.ExecToErr("cancel import job 9999999999").unwrap_err(),
        exeerrors::ErrLoadDataJobNotFound,
    );
    let started = gate(
        &s,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncAfterJobStarted",
        None,
    );
    auth_as(&s, "test_cancel_job1", "localhost");
    let rows =
        s.tk.MustQuery(&format!(
            "import into t1 FROM '{}' with detached",
            source("test_cancel_job", "t.csv")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id = rows[0][0].parse().unwrap();
    started.reached();
    s.tk.MustExec(&format!("cancel import job {id}"));
    started.release();
    let mut expected = job(
        &s,
        id,
        "test_cancel_job",
        "t1",
        "test_cancel_job",
        "t.csv",
        "test_cancel_job1@localhost",
    );
    expected.Status = "cancelled".into();
    expected.Step = importer::JobStepImporting.into();
    expected.ErrorMessage = "cancelled by user".into();
    compare_job_info_without_time(&s, &expected, &show(&s, id));
    s.Eventually(
        || s.get_task_by_job_id((), id).State == proto::TaskStateReverted,
        max_wait_time(),
        Duration::from_millis(10),
    );
    s.ErrorIs(
        &s.tk
            .ExecToErr(&format!("cancel import job {id}"))
            .unwrap_err(),
        exeerrors::ErrLoadDataInvalidOperation,
    );
    auth_as(&s, "test_cancel_job2", "localhost");
    s.ErrorIs(
        &s.tk
            .ExecToErr(&format!("cancel import job {id}"))
            .unwrap_err(),
        plannererrors::ErrSpecificAccessDenied,
    );
    auth_as(&s, "root", "localhost");
    s.ErrorIs(
        &s.tk
            .ExecToErr(&format!("cancel import job {id}"))
            .unwrap_err(),
        exeerrors::ErrLoadDataInvalidOperation,
    );
    failpoint::Disable("github.com/pingcap/tidb/pkg/dxf/importinto/syncAfterJobStarted").unwrap();
    // Cancel a second user's job at the post-process boundary from a root session.
    let post = gate(
        &s,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforePostProcess",
        None,
    );
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/waitCtxDone",
        "return(true)",
    );
    auth_as(&s, "test_cancel_job2", "localhost");
    let rows =
        s.tk.MustQuery(&format!(
            "import into t2 FROM '{}' with detached",
            source("test_cancel_job", "t.csv")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id2 = rows[0][0].parse().unwrap();
    post.reached();
    let tk2 = testkit::NewTestKit(&s.t, s.store.clone());
    let cancel = tk2.clone();
    std::thread::spawn(move || cancel.MustExec(&format!("cancel import job {id2}")))
        .join()
        .unwrap();
    post.release();
    assert_eq!(
        s.get_task_by_job_id((), id2).State,
        proto::TaskStateReverted
    );
    expected = job(
        &s,
        id2,
        "test_cancel_job",
        "t2",
        "test_cancel_job",
        "t.csv",
        "test_cancel_job2@localhost",
    );
    expected.Status = "cancelled".into();
    expected.Step = importer::JobStepValidating.into();
    expected.ErrorMessage = "cancelled by user".into();
    compare_job_info_without_time(&s, &expected, &show(&s, id2));
    let manager = storage::GetTaskManager().unwrap();
    s.Eventually(
        || {
            let task = manager
                .GetTaskByKeyWithHistory((), &importinto::TaskKey(id2))
                .unwrap();
            let subtasks = manager
                .GetSubtasksWithHistory((), task.ID, proto::ImportStepPostProcess)
                .unwrap();
            assert_eq!(subtasks.len(), 1);
            task.State == proto::TaskStateReverted
                && subtasks
                    .iter()
                    .any(|s| s.State == proto::SubtaskStateCanceled)
        },
        max_wait_time(),
        Duration::from_millis(10),
    );
    failpoint::Disable("github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforePostProcess").unwrap();
    failpoint::Disable("github.com/pingcap/tidb/pkg/executor/importer/waitCtxDone").unwrap();
    let pending = gate(
        &s,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeJobStarted",
        None,
    );
    auth_as(&s, "root", "localhost");
    s.tk.MustExec("truncate table t2");
    auth_as(&s, "test_cancel_job2", "localhost");
    let rows =
        s.tk.MustQuery(&format!(
            "import into t2 FROM '{}' with detached",
            source("test_cancel_job", "t.csv")
        ))
        .Rows();
    assert_eq!(rows.len(), 1);
    let id3 = rows[0][0].parse().unwrap();
    pending.reached();
    std::thread::spawn(move || tk2.MustExec(&format!("cancel import job {id3}")))
        .join()
        .unwrap();
    pending.release();
    assert_eq!(
        s.get_task_by_job_id((), id3).State,
        proto::TaskStateReverted
    );
    expected = job(
        &s,
        id3,
        "test_cancel_job",
        "t2",
        "test_cancel_job",
        "t.csv",
        "test_cancel_job2@localhost",
    );
    expected.Status = "cancelled".into();
    expected.Step = importer::JobStepImporting.into();
    expected.ErrorMessage = "cancelled by user".into();
    compare_job_info_without_time(&s, &expected, &show(&s, id3));
    s.tear_down();
}

#[test]
fn test_job_fail_when_dispatch_subtask() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("fail_job_after_import");
    s.tk.MustExec("CREATE TABLE t1 (i INT PRIMARY KEY)");
    object(&s, "fail_job_after_import", "t.csv", "1\n2");
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/failWhenDispatchPostProcessSubtask",
        "return(true)",
    );
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/setLastImportJobID",
        "return(true)",
    );
    auth_as(&s, "root", "localhost");
    let error =
        s.tk.QueryToErr(&format!(
            "import into t1 FROM '{}'",
            source("fail_job_after_import", "t.csv")
        ))
        .unwrap_err();
    s.ErrorContains(&error, "injected error after ImportStepImport");
    let id = importer::TestLastImportJobID.load(Ordering::SeqCst);
    let mut expected = job(
        &s,
        id,
        "fail_job_after_import",
        "t1",
        "fail_job_after_import",
        "t.csv",
        "root@%",
    );
    expected.Status = "failed".into();
    expected.Step = importer::JobStepValidating.into();
    expected.ErrorMessage = "injected error after ImportStepImport".into();
    compare_job_info_without_time(&s, &expected, &show(&s, id));
    s.tear_down();
}

#[test]
fn test_kill_before_finish() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.cleanup_sys_tables();
    s.tk.MustExec("DROP DATABASE IF EXISTS kill_job");
    s.tk.MustExec("CREATE DATABASE kill_job");
    s.tk.MustExec("CREATE TABLE kill_job.t (a INT, b INT, c int)");
    object(&s, "test-load", "t-1.tsv", "1,11,111");
    let cancellation = Arc::new(Mutex::new(None::<Arc<Mutex<Option<bool>>>>));
    let capture = cancellation.clone();
    testfailpoint::EnableCall(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/cancellableCtx",
        move |ctx| {
            if let FailCtx::CancelFlag(flag) = ctx {
                *capture.lock().unwrap() = Some(flag);
            } else {
                panic!("missing cancellation context");
            }
        },
    );
    testfailpoint::EnableCall(
        &s.t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/syncBeforeSortChunk",
        move |_| {
            *cancellation
                .lock()
                .unwrap()
                .as_ref()
                .expect("context initialized before sort")
                .lock()
                .unwrap() = Some(true);
        },
    );
    testfailpoint::Enable(
        &s.t,
        "github.com/pingcap/tidb/pkg/executor/importer/setLastImportJobID",
        "return(true)",
    );
    let tk = s.tk.clone();
    let error = std::thread::spawn(move || {
        tk.QueryToErr(&format!(
            "IMPORT INTO kill_job.t FROM '{}'",
            source("test-load", "t-*.tsv")
        ))
        .unwrap_err()
    })
    .join()
    .unwrap();
    assert_eq!(error, "context canceled");
    let id = importer::TestLastImportJobID.load(Ordering::SeqCst);
    assert_eq!(field(&show(&s, id), "Status"), "cancelled");
    let manager = storage::GetTaskManager().unwrap();
    s.Eventually(
        || {
            manager
                .GetTaskByKeyWithHistory((), &importinto::TaskKey(id))
                .unwrap()
                .State
                == proto::TaskStateReverted
        },
        max_wait_time(),
        Duration::from_millis(10),
    );
    s.tear_down();
}

#[test]
fn test_job_comparison_rejects_wrong_source_and_error() {
    let _serial = serial_guard();
    reset_engine();
    let s = MockGCSSuite::setup();
    s.prepare_and_use_db("job_comparison");
    s.tk.MustExec("create table t (i int primary key)");
    object(&s, "job-comparison", "t.csv", "1\n2");
    let rows =
        s.tk.MustQuery(&format!(
            "import into t from '{}'",
            source("job-comparison", "t.csv")
        ))
        .Rows();
    let id = rows[0][0].parse().unwrap();
    let mut expected = job(
        &s,
        id,
        "job_comparison",
        "t",
        "job-comparison",
        "t.csv",
        "root@%",
    );
    expected.Status = "finished".into();
    expected.Summary = Some(importer::Summary { ImportedRows: 2 });
    compare_job_info_without_time(&s, &expected, &rows[0]);
    let mut wrong = rows[0].clone();
    wrong[fmap()["DataSource"]] = source("wrong-bucket", "t.csv");
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compare_job_info_without_time(&s, &expected, &wrong)
        }))
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || s.Regexp("^expected error$", "the file is empty")
        ))
        .is_err()
    );
    // Query order is irrelevant; duplicate values retain their ordering as in url.Values.
    expected.Parameters.FileLocation.push_str("&x=one&x=two");
    let mut reordered = rows[0].clone();
    reordered[fmap()["DataSource"]] = format!(
        "gs://job-comparison/t.csv?x=one&x=two&endpoint={}",
        gcs_endpoint()
    );
    compare_job_info_without_time(&s, &expected, &reordered);
    s.tear_down();
}
