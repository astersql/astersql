// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL util 单元测试：目录非空、系统库判定、作业暂停状态机。

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::{
    AdminCommandOperator, CancellationToken, DdlUtilError, DeleteKeysWithPrefixFromEtcd,
    EtcdClient, FolderNotEmpty, HasSysDB, InvolvingSchemaInfo, Job, JobState, PauseRunningJob,
    PutKVToEtcd, PutKVToEtcdMono, SchemaState,
};

/// Go 的 `for i := range retryCnt` 在 retryCnt 为零时不执行操作，并返回 nil。
#[test]
fn zero_etcd_retries_match_go_noop_success() {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let client = EtcdClient::default();

    assert_eq!(
        Ok(()),
        DeleteKeysWithPrefixFromEtcd("/ddl/", &client, 0, Duration::ZERO)
    );
    assert_eq!(
        Ok(()),
        PutKVToEtcdMono(
            &cancellation,
            &client,
            0,
            "/ddl/key",
            "value",
            Duration::ZERO,
        )
    );
    assert_eq!(
        Ok(()),
        PutKVToEtcd(
            &cancellation,
            &client,
            0,
            "/ddl/key",
            "value",
            Duration::ZERO,
        )
    );
}

/// 空目录/不存在路径为 false；含文件后为 true。
// test_folder_not_empty 对应 Go 的 TestFolderNotEmpty。
#[test]
fn test_folder_not_empty() {
    let tmp: PathBuf = std::env::temp_dir().join(format!(
        "astersql-ddl-util-folder-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&tmp).unwrap();
    let tmp_str = tmp.to_str().unwrap();
    assert!(!FolderNotEmpty(tmp_str));
    assert!(!FolderNotEmpty(tmp.join("not-exist").to_str().unwrap()));

    let file_path = tmp.join("test-file");
    let f = fs::File::create(&file_path).unwrap();
    drop(f);
    assert!(FolderNotEmpty(tmp_str));
    fs::remove_dir_all(&tmp).ok();
}

/// 用户库返回 false；含 mysql/sys 等系统库返回 true。
// test_has_sys_db 对应 Go 的 TestHasSysDB。
#[test]
fn test_has_sys_db() {
    let tests = [
        (
            "user database",
            Job {
                involving_schemas: vec![InvolvingSchemaInfo {
                    database: "test".to_string(),
                }],
                ..Default::default()
            },
            false,
        ),
        (
            "system database",
            Job {
                involving_schemas: vec![InvolvingSchemaInfo {
                    database: "mysql".to_string(),
                }],
                ..Default::default()
            },
            true,
        ),
        (
            "mixed databases",
            Job {
                involving_schemas: vec![
                    InvolvingSchemaInfo {
                        database: "test".to_string(),
                    },
                    InvolvingSchemaInfo {
                        database: "sys".to_string(),
                    },
                ],
                ..Default::default()
            },
            true,
        ),
    ];

    for (name, job, want) in tests {
        assert_eq!(want, HasSysDB(&job), "{name}");
    }
}

/// 覆盖排队可暂停、已暂停报错、Done 不可暂停等分支。
// test_pause_running_job 对应 Go 的 TestPauseRunningJob。
#[test]
fn test_pause_running_job() {
    /// 单条暂停用例期望。
    struct Case {
        /// 用例名。
        name: &'static str,
        /// 输入作业。
        job: Job,
        /// 操作者。
        by_who: AdminCommandOperator,
        /// 期望错误；`None` 表示成功。
        err_equal: Option<DdlUtilError>,
        /// 期望作业状态。
        want_state: JobState,
        /// 期望记录的操作者。
        want_admin_op: AdminCommandOperator,
    }

    let tests = [
        Case {
            name: "queueing job becomes pausing",
            job: Job {
                id: 101,
                state: JobState::Queueing,
                ..Default::default()
            },
            by_who: AdminCommandOperator::User,
            err_equal: None,
            want_state: JobState::Pausing,
            want_admin_op: AdminCommandOperator::User,
        },
        Case {
            name: "pausing job returns paused error",
            job: Job {
                id: 102,
                state: JobState::Pausing,
                admin_operator: AdminCommandOperator::System,
                ..Default::default()
            },
            by_who: AdminCommandOperator::User,
            err_equal: Some(DdlUtilError::PausedJob(102)),
            want_state: JobState::Pausing,
            want_admin_op: AdminCommandOperator::System,
        },
        Case {
            name: "paused job returns paused error",
            job: Job {
                id: 103,
                state: JobState::Paused,
                admin_operator: AdminCommandOperator::System,
                ..Default::default()
            },
            by_who: AdminCommandOperator::User,
            err_equal: Some(DdlUtilError::PausedJob(103)),
            want_state: JobState::Paused,
            want_admin_op: AdminCommandOperator::System,
        },
        Case {
            name: "non pausable job returns cannot pause error",
            job: Job {
                id: 104,
                state: JobState::Done,
                schema_state: SchemaState::Public,
                admin_operator: AdminCommandOperator::System,
                ..Default::default()
            },
            by_who: AdminCommandOperator::User,
            err_equal: Some(DdlUtilError::CannotPauseJob {
                job_id: 104,
                reason: format!(
                    "state [{:?}] or schema state [{:?}]",
                    JobState::Done,
                    SchemaState::Public
                ),
            }),
            want_state: JobState::Done,
            want_admin_op: AdminCommandOperator::System,
        },
    ];

    // 逐用例调用 PauseRunningJob，核对错误、状态与 admin_operator。
    for mut tt in tests {
        let err = PauseRunningJob(&mut tt.job, tt.by_who);
        match tt.err_equal {
            None => assert!(err.is_ok(), "{}: {err:?}", tt.name),
            Some(expected) => {
                let actual = err.expect_err(tt.name);
                assert_eq!(expected, actual, "{}", tt.name);
            }
        }
        assert_eq!(tt.want_state, tt.job.state, "{}", tt.name);
        assert_eq!(tt.want_admin_op, tt.job.admin_operator, "{}", tt.name);
    }
}
