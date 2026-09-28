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

// Admin Pause 测试包入口与跨文件契约测试。

use std::collections::BTreeSet;
use std::time::Duration;

use astersql_ddl::ddl::{AdminCommandOperator, Ddl, Job, JobCommand, JobState};

use crate::ddl_data_generation::{
    ADMIN_PAUSE_TEST_TABLE, ADMIN_PAUSE_TEST_TABLE_WITH_VEC, AGE_MAX, SqlExecutor, TestTableUser,
    generate_tbl_user, generate_tbl_user_parition, generate_tbl_user_with_vec,
};
use crate::ddl_stmt_cases::{
    AutoIncrsedID, StmtCase, column_ddl_stmt_case, index_ddl_stmt_case, place_rul_ddl_stmt_case,
    schema_ddl_stmt_case, simple_run_stmt, table_ddl_stmt, table_partition_ddl_stmt_case,
};

/// Go `TestMain` shortens DDL retry sleeps to one microsecond. The Rust DDL
/// model has no process-wide retry-sleep knob, so retain the exact harness
/// contract here for the test paths that need to apply it locally.
const DDL_ERROR_WAIT: Duration = Duration::from_micros(1);

/// Apply the executable process-level setup performed by Go `TestMain`.
fn configure_test_main() {
    astersql_testkit_testsetup::SetupForCommonTest();
    astersql_config::update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
    });
}

#[derive(Default)]
struct RecordingExecutor {
    calls: Vec<String>,
    fail_on: Option<String>,
}

impl SqlExecutor for RecordingExecutor {
    fn must_exec(&mut self, sql: &str) -> Result<(), String> {
        self.calls.push(sql.to_owned());
        if self.fail_on.as_deref() == Some(sql) {
            Err(format!("recorded execution failed: {sql}"))
        } else {
            Ok(())
        }
    }

    fn exec(&mut self, sql: &str) -> Result<(), String> {
        self.calls.push(sql.to_owned());
        Ok(())
    }
}

fn all_cases(ai: &mut AutoIncrsedID) -> Vec<StmtCase> {
    let mut cases = Vec::new();
    cases.extend(schema_ddl_stmt_case(ai));
    cases.extend(table_ddl_stmt(ai));
    cases.extend(index_ddl_stmt_case(ai));
    cases.extend(column_ddl_stmt_case(ai));
    cases.extend(table_partition_ddl_stmt_case(ai));
    cases.extend(place_rul_ddl_stmt_case(ai));
    cases
}

fn running_ddl(id: i64, query: &str) -> Ddl {
    let mut ddl = Ddl::new("adminpause-matrix", Vec::new());
    ddl.started = true;
    ddl.submit_job(Job::new(id, 1, id, query)).unwrap();
    ddl
}

#[test]
fn test_main_contract_and_case_matrix_match_go() {
    // Go TestMain sets these values before running the package.
    assert_eq!(crate::DB_TEST_LEASE_MILLIS, 600);
    assert_eq!(crate::LOGGER, "logutil.DDLLogger()");

    let mut ai = AutoIncrsedID { idx: 0 };
    let cases = all_cases(&mut ai);
    assert_eq!(cases.len(), 85);
    let ids: BTreeSet<_> = cases.iter().map(|case| case.global_id).collect();
    assert_eq!(ids.len(), cases.len());
    assert_eq!(
        ids.iter().copied().collect::<Vec<_>>(),
        (0..85).collect::<Vec<_>>()
    );
    assert_eq!(cases.iter().filter(|case| case.is_job_pausable).count(), 42);
}

#[test]
fn prepare_domain_returns_shared_real_testkits() {
    let mut prepared = crate::prepare_domain(&());
    prepared
        .stmt_kit
        .MustExec("create table adminpause_prepare_probe(a int)", Vec::new());
    prepared.admin_command_kit.MustExec("use test", Vec::new());
    prepared
        .admin_command_kit
        .MustQuery(
            "select count(*) from information_schema.tables where table_schema = 'test' and table_name = 'adminpause_prepare_probe'",
            Vec::new(),
        )
        .Check(astersql_testkit::Rows(&["1"]));
    prepared
        .stmt_kit
        .MustExec("drop table adminpause_prepare_probe", Vec::new());
    assert!(std::sync::Arc::strong_count(&prepared.store) >= 3);
    assert!(!prepared.domain.is_closed());
}

#[test]
fn data_generation_restores_go_random_bounds_and_sql_side_effects() {
    let mut user = TestTableUser::default();
    user.generate_attributes(7).unwrap();
    assert!(user.tenant.len() <= 126);
    assert!(user.name.len() <= 126);
    assert!(user.province.len() <= 31);
    assert!(user.city.len() <= 31);
    assert_eq!(user.phone.len(), 14);
    assert!((0..AGE_MAX).contains(&user.age));
    assert_ne!(user.created_time, "time.Now()");
    assert_ne!(user.updated_time, "time.Now()");
    assert_eq!(user.vec, "[0,0,0]");

    let mut executor = RecordingExecutor::default();
    generate_tbl_user(&mut executor, 2).unwrap();
    assert_eq!(executor.calls.len(), 2);
    assert_eq!(executor.calls[0], crate::ADMIN_PAUSE_TEST_TABLE_STMT);
    assert!(executor.calls[1].starts_with("INSERT INTO t_user("));

    executor.calls.clear();
    generate_tbl_user_with_vec(&mut executor, 0).unwrap();
    assert_eq!(
        executor.calls,
        vec![crate::ADMIN_PAUSE_TEST_TABLE_STMT_WITH_VEC]
    );

    executor.calls.clear();
    generate_tbl_user_with_vec(&mut executor, 1).unwrap();
    assert_eq!(executor.calls.len(), 3);
    assert!(executor.calls[1].contains("set tiflash replica 3"));
    assert!(executor.calls[2].contains("vec) VALUES"));

    executor.calls.clear();
    generate_tbl_user_parition(&mut executor).unwrap();
    assert_eq!(
        executor.calls,
        vec![crate::ADMIN_PAUSE_TEST_PARTITION_TABLE_STMT]
    );
    assert_eq!(ADMIN_PAUSE_TEST_TABLE, "t_user");
    assert_eq!(ADMIN_PAUSE_TEST_TABLE_WITH_VEC, "t_user_vec");
}

#[test]
fn simple_run_stmt_executes_prepare_target_and_best_effort_rollback_in_order() {
    let case = StmtCase {
        global_id: 1,
        stmt: "alter table t add column c int".to_owned(),
        schema_state: "StateNone",
        is_job_pausable: true,
        pre_condition_stmts: vec!["create table t(a int)".to_owned()],
        rollback_stmts: vec!["drop table t".to_owned()],
    };
    let mut executor = RecordingExecutor::default();
    simple_run_stmt(&case, &mut executor).unwrap();
    assert_eq!(
        executor.calls,
        vec![
            "create table t(a int)",
            "alter table t add column c int",
            "drop table t",
        ]
    );
}

#[test]
fn every_go_case_supports_pause_resume_and_pause_cancel_paths() {
    let mut ai = AutoIncrsedID { idx: 0 };
    for case in all_cases(&mut ai) {
        let id = i64::from(case.global_id) + 1;
        let mut ddl = running_ddl(id, &case.stmt);
        if case.is_job_pausable {
            assert_eq!(
                ddl.process_jobs(&[id], JobCommand::Pause, AdminCommandOperator::User),
                vec![Ok(())]
            );
            assert_eq!(ddl.jobs[&id].state, JobState::Paused);
            assert_eq!(
                ddl.process_jobs(&[id], JobCommand::Resume, AdminCommandOperator::User),
                vec![Ok(())]
            );
            assert_eq!(ddl.jobs[&id].state, JobState::Running);

            let mut cancel_ddl = running_ddl(id, &case.stmt);
            cancel_ddl.process_jobs(&[id], JobCommand::Pause, AdminCommandOperator::User);
            assert_eq!(
                cancel_ddl.process_jobs(&[id], JobCommand::Cancel, AdminCommandOperator::User),
                vec![Ok(())]
            );
            assert_eq!(cancel_ddl.jobs[&id].state, JobState::Cancelling);
        } else {
            assert_eq!(ddl.jobs[&id].state, JobState::Running);
            // A non-pausable Go case executes normally and never issues admin pause.
            assert_eq!(ddl.jobs[&id].paused_by, None);
        }
    }
}

#[test]
fn test_main_applies_go_process_configuration() {
    let restore_config = astersql_config::restore_func();

    configure_test_main();

    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert_eq!(DDL_ERROR_WAIT, Duration::from_micros(1));

    restore_config();
}
