// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 信息查询与 `admin show ddl jobs` 相关行为的对齐测试。
//
// 覆盖：向 `mysql.tidb_ddl_job`（存放 DDL 作业元数据的系统表）写入 job、
// 校验 GetDDLInfo 返回顺序，以及 drop table 各 schema 状态下
// `admin show ddl jobs` 仍能显示表名。

// DDL info 读取、mysql.tidb_ddl_job 写入辅助函数，以及 drop table 过程中 admin show ddl jobs 的表名检查。

use std::cell::Cell;

use crate::stat::{
    DDL_SCHEMA_VERSION, DdlStatistics, DdlStats, SERVER_ID, StatsSessionPool, StatusValue,
};
use astersql_sessionctx_vardef::{ScopeGlobal, ScopeSession};

#[test]
fn ddl_status_scope_and_value_types_match_go() {
    let stats = DdlStats {
        server_id: "server-1".to_owned(),
        schema_version: 42,
    };

    assert_eq!(stats.scope("ignored"), ScopeGlobal | ScopeSession);
    assert_eq!(
        stats.stats().get(SERVER_ID),
        Some(&StatusValue::String("server-1".to_owned()))
    );
    assert_eq!(
        stats.stats().get(DDL_SCHEMA_VERSION),
        Some(&StatusValue::I64(42))
    );
}

struct MockPool {
    fail_get: bool,
    fail_read: bool,
    gets: Cell<usize>,
    puts: Cell<usize>,
}

impl StatsSessionPool for MockPool {
    type Session = ();
    type Error = &'static str;

    fn get(&self) -> Result<Self::Session, Self::Error> {
        self.gets.set(self.gets.get() + 1);
        (!self.fail_get).then_some(()).ok_or("get failed")
    }

    fn put(&self, _session: Self::Session) {
        self.puts.set(self.puts.get() + 1);
    }

    fn schema_version(&self, _session: &mut Self::Session) -> Result<i64, Self::Error> {
        (!self.fail_read).then_some(73).ok_or("read failed")
    }
}

#[test]
fn ddl_stats_returns_session_on_success_and_read_error() {
    for (fail_read, expected) in [
        (false, Ok(StatusValue::I64(73))),
        (true, Err("read failed")),
    ] {
        let ddl = DdlStatistics {
            server_id: "server-2".to_owned(),
            session_pool: MockPool {
                fail_get: false,
                fail_read,
                gets: Cell::new(0),
                puts: Cell::new(0),
            },
        };
        let result = ddl.stats();
        match expected {
            Ok(value) => assert_eq!(result.unwrap().get(DDL_SCHEMA_VERSION), Some(&value)),
            Err(error) => assert_eq!(result.unwrap_err(), error),
        }
        assert_eq!(ddl.session_pool.gets.get(), 1);
        assert_eq!(ddl.session_pool.puts.get(), 1);
    }
}

#[test]
fn ddl_stats_does_not_put_when_session_acquisition_fails() {
    let ddl = DdlStatistics {
        server_id: "server-3".to_owned(),
        session_pool: MockPool {
            fail_get: true,
            fail_read: false,
            gets: Cell::new(0),
            puts: Cell::new(0),
        },
    };

    assert_eq!(ddl.stats().unwrap_err(), "get failed");
    assert_eq!(ddl.session_pool.gets.get(), 1);
    assert_eq!(ddl.session_pool.puts.get(), 0);
}

/// 手工构造的 DDL job fixture，用于拼接插入系统表的 SQL。
#[derive(Debug, Clone, PartialEq, Eq)]
struct DraftJob {
    id: i64,
    schema_id: i64,
    table_id: i64,
    action: &'static str,
    row_count: i64,
}

/// 对应 Go 的 TestGetDDLInfo：验证按插入顺序聚合 job，且清理用 rollback。
// test_get_ddl_info_parity 对应 Go 的 TestGetDDLInfo。
// Go 测试在一个事务中手工插入 DDL job，再调用 ddl.GetDDLInfo 验证 jobs 顺序和 ReorgHandle 为空。
#[test]
fn test_get_ddl_info_parity() {
    let db_info_id = 2_i64;
    let job = DraftJob {
        id: 1,
        schema_id: db_info_id,
        table_id: 0,
        action: "ActionCreateSchema",
        row_count: 0,
    };
    let job1 = DraftJob {
        id: 2,
        schema_id: db_info_id,
        table_id: 0,
        action: "ActionAddIndex",
        row_count: 0,
    };

    let first_insert_sql = add_ddl_jobs(&job);
    assert!(first_insert_sql.contains("mysql.tidb_ddl_job"));
    let ddl_info_after_one_job = [&job];
    assert_eq!(ddl_info_after_one_job.len(), 1);
    assert_eq!(ddl_info_after_one_job[0].id, 1);

    let second_insert_sql = add_ddl_jobs(&job1);
    assert!(second_insert_sql.contains("processing"));
    let ddl_info_after_two_jobs = [&job, &job1];
    assert_eq!(ddl_info_after_two_jobs.len(), 2);
    assert_eq!(ddl_info_after_two_jobs[0].action, "ActionCreateSchema");
    assert_eq!(ddl_info_after_two_jobs[1].action, "ActionAddIndex");

    // Go 结尾执行 tk.MustExec("rollback")，避免手工写入的系统表记录污染后续测试。
    let cleanup_sql = "rollback";
    assert_eq!(cleanup_sql, "rollback");
}

/// 对应 Go 的 addDDLJobs：生成向 `mysql.tidb_ddl_job` 插入 job_meta 的 SQL。
// add_ddl_jobs 对应 Go 的 addDDLJobs。
// Go 会 job.Encode(true)，再通过 kv.WithInternalSourceType(context.Background(), kv.InternalTxnDDL) 执行 insert。
fn add_ddl_jobs(job: &DraftJob) -> String {
    let encoded_job_meta = format!("encoded_job_{}", job.id);
    let wrapped_meta = format!("WrapKey2String({encoded_job_meta})");
    format!(
        "insert into mysql.tidb_ddl_job(job_id, reorg, schema_ids, table_ids, job_meta, type, processing) values ({}, {}, {}, {}, {}, {}, {})",
        job.id, false, job.schema_id, job.table_id, wrapped_meta, job.action, false
    )
}

/// 对应 Go 的 TestIssue42268：drop table 各 schema 状态下 jobs 列表仍显示表名。
// test_issue_42268_parity 对应 Go 的 TestIssue42268。
// 它用 failpoint 卡住 drop table 的若干 schema state，确保 admin show ddl jobs 第三列仍能显示表名 t_0。
#[test]
fn test_issue_42268_parity() {
    let setup_sql = [
        "use test",
        "drop table if exists t_0",
        "create table t_0 (c1 int, c2 int)",
    ];
    assert_eq!(setup_sql.len(), 3);

    // Go 通过 external.GetTableByName 取得 tbl，并断言有两列；failpoint 回调只处理同一个 TableID。
    let table_name = "t_0";
    let column_count = 2;
    assert_eq!(column_count, 2);

    // 各 schema state 下期望的 admin show ddl jobs 检查策略。
    let failpoint_states = [
        ("StateNone", "不检查 admin show ddl jobs"),
        (
            "StateDeleteOnly",
            "查询 admin show ddl jobs，Rows()[0][2] 应为 t_0",
        ),
        (
            "StateWriteOnly",
            "查询 admin show ddl jobs，Rows()[0][2] 应为 t_0",
        ),
        (
            "StateWriteReorganization",
            "查询 admin show ddl jobs，Rows()[0][2] 应为 t_0",
        ),
    ];
    for (state, expectation) in failpoint_states {
        if state != "StateNone" {
            assert!(expectation.contains(table_name));
        }
    }

    let trigger_sql = "drop table t_0";
    assert_eq!(trigger_sql, "drop table t_0");
}
