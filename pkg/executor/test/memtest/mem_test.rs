// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 对应 Go `mem_test.go`：所有断言均通过真实 TestKit/ConcreteSession SQL 路径，
// 不手工模拟 tracker cleanup、系统变量或内存仲裁指标。

#![allow(non_snake_case)]

use astersql_testkit::{Rows, TestKit};
use astersql_util_memory::global_arbitrator::{
    CleanupGlobalMemArbitratorForTest, GlobalMemArbitrator, SetupGlobalMemArbitratorForTest,
    WorkMode,
};
use astersql_util_memory::tracker::ServerMemoryLimit;

const MAX_SERVER_LIMIT: i64 = 1_000_000_000_000_000;
const ERR_ARBITRATOR_MODE: &str = "tidb_mem_arbitrator_mode: disable; standard; priority;";
const ERR_SOFT_LIMIT: &str = "tidb_mem_arbitrator_soft_limit: 0 (default); (0, 1.0] float-rate * server-limit; (1, server-limit] integer bytes; auto;";
const ERR_WAIT_AVERSE: &str = "tidb_mem_arbitrator_wait_averse: 0 (disable); 1 (enable); nolimit;";
const ERR_QUERY_RESERVED: &str =
    "tidb_mem_arbitrator_query_reserved: 0 (default); (1, server-limit] integer bytes;";

struct GlobalArbitratorCleanup {
    previous_server_limit: u64,
}

impl Drop for GlobalArbitratorCleanup {
    fn drop(&mut self) {
        CleanupGlobalMemArbitratorForTest();
        ServerMemoryLimit.Store(self.previous_server_limit);
    }
}

fn new_testkit() -> TestKit {
    TestKit::new(astersql_testkit::mockstore::CreateMockStoreAndDomain().0)
}

#[test]
fn TestInsertUpdateTrackerOnCleanUp() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (id int)", Vec::new());

    let origin = tk.Session().GetSessionVars().MemTracker().BytesConsumed();
    tk.MustExec("insert t (id) values (1)", Vec::new());
    tk.MustExec("insert t (id) values (2)", Vec::new());
    tk.MustExec("insert t (id) values (3)", Vec::new());
    assert_eq!(
        tk.Session().GetSessionVars().MemTracker().BytesConsumed(),
        origin
    );

    let origin = tk.Session().GetSessionVars().MemTracker().BytesConsumed();
    tk.MustExec("update t set id = 4 where id = 1", Vec::new());
    tk.MustExec("update t set id = 5 where id = 2", Vec::new());
    tk.MustExec("update t set id = 6 where id = 3", Vec::new());
    assert_eq!(
        tk.Session().GetSessionVars().MemTracker().BytesConsumed(),
        origin
    );
}

#[test]
fn TestGlobalMemArbitrator() {
    let directory = tempfile::tempdir().expect("create temp directory");
    let previous_server_limit = ServerMemoryLimit.Load();
    SetupGlobalMemArbitratorForTest(directory.path().display().to_string());
    let _cleanup = GlobalArbitratorCleanup {
        previous_server_limit,
    };

    let mut tk = new_testkit();
    assert!(
        tk.ExecToErr("set @@tidb_mem_arbitrator_mode = standard")
            .message()
            .contains("GLOBAL")
    );
    assert_eq!(
        tk.ExecToErr("set global tidb_mem_arbitrator_mode = 1")
            .message(),
        ERR_ARBITRATOR_MODE
    );
    assert!(GlobalMemArbitrator().is_none());

    tk.MustExec("set global tidb_mem_arbitrator_mode = standard", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_mode", Vec::new())
        .Check(Rows(&["standard"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .WorkMode(),
        WorkMode::Standard
    );

    tk.MustExec("set global tidb_mem_arbitrator_mode = priority", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_mode", Vec::new())
        .Check(Rows(&["priority"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("priority arbitrator")
            .WorkMode(),
        WorkMode::Priority
    );

    tk.MustExec("set global tidb_mem_arbitrator_mode = default", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_mode", Vec::new())
        .Check(Rows(&["disable"]));
    assert!(GlobalMemArbitrator().is_none());

    tk.MustExec(
        &format!("set global tidb_server_memory_limit={MAX_SERVER_LIMIT}"),
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_server_memory_limit", Vec::new())
        .Check(Rows(&[&MAX_SERVER_LIMIT.to_string()]));
    assert_eq!(ServerMemoryLimit.Load(), MAX_SERVER_LIMIT as u64);
    assert!(GlobalMemArbitrator().is_none());

    assert_eq!(
        tk.ExecToErr("set global tidb_mem_arbitrator_soft_limit=-1")
            .message(),
        ERR_SOFT_LIMIT
    );
    tk.MustExec(
        "set global tidb_mem_arbitrator_soft_limit = 12345678",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["12345678"]));
    assert!(GlobalMemArbitrator().is_none());

    tk.MustExec("set global tidb_mem_arbitrator_mode = standard", Vec::new());
    {
        let arbitrator = GlobalMemArbitrator().expect("standard arbitrator");
        assert_eq!(arbitrator.Limit(), MAX_SERVER_LIMIT);
        assert_eq!(arbitrator.SoftLimit(), 12_345_678);
    }
    assert!(
        tk.ExecToErr("set @@tidb_mem_arbitrator_soft_limit = 0")
            .message()
            .contains("GLOBAL")
    );

    tk.MustExec(
        "set global tidb_mem_arbitrator_soft_limit = default",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["0"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        (MAX_SERVER_LIMIT as f64 * 0.95) as i64
    );

    tk.MustExec("set global tidb_mem_arbitrator_soft_limit = 1", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["1"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        MAX_SERVER_LIMIT
    );

    tk.MustExec(
        "set global tidb_mem_arbitrator_soft_limit = 0.5",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["0.5"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        MAX_SERVER_LIMIT / 2
    );

    let over_limit = MAX_SERVER_LIMIT * 10;
    tk.MustExec(
        &format!("set global tidb_mem_arbitrator_soft_limit={over_limit}"),
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&[&over_limit.to_string()]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        MAX_SERVER_LIMIT
    );

    tk.MustExec(
        "set global tidb_mem_arbitrator_soft_limit = 100",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["100"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        100
    );

    tk.MustExec(
        "set global tidb_mem_arbitrator_soft_limit = auto",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_soft_limit", Vec::new())
        .Check(Rows(&["auto"]));
    assert_eq!(
        GlobalMemArbitrator()
            .expect("standard arbitrator")
            .SoftLimit(),
        (MAX_SERVER_LIMIT as f64 * 0.95) as i64
    );

    assert_eq!(
        tk.ExecToErr("set tidb_mem_arbitrator_wait_averse=anonymous")
            .message(),
        ERR_WAIT_AVERSE
    );
    tk.MustExec("set tidb_mem_arbitrator_wait_averse = 1", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_wait_averse", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("set tidb_mem_arbitrator_wait_averse = default", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_wait_averse", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec("set tidb_mem_arbitrator_wait_averse = nolimit", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_wait_averse", Vec::new())
        .Check(Rows(&["nolimit"]));
    assert!(
        tk.ExecToErr("set global tidb_mem_arbitrator_wait_averse=0")
            .message()
            .contains("SESSION")
    );
    tk.MustExec("set tidb_mem_arbitrator_wait_averse=0", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_wait_averse", Vec::new())
        .Check(Rows(&["0"]));

    assert!(
        tk.ExecToErr("set global tidb_mem_arbitrator_query_reserved = 0")
            .message()
            .contains("SESSION")
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_query_reserved", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec(
        "set tidb_mem_arbitrator_query_reserved = default",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_query_reserved", Vec::new())
        .Check(Rows(&["0"]));
    assert_eq!(
        tk.ExecToErr("set tidb_mem_arbitrator_query_reserved = 9223372036854775808")
            .message(),
        ERR_QUERY_RESERVED
    );
    assert_eq!(
        tk.ExecToErr("set tidb_mem_arbitrator_query_reserved = 1")
            .message(),
        ERR_QUERY_RESERVED
    );
    tk.MustExec("set tidb_mem_arbitrator_query_reserved = 2", Vec::new());
    tk.MustQuery("select @@tidb_mem_arbitrator_query_reserved", Vec::new())
        .Check(Rows(&["2"]));
    tk.MustExec(
        "set tidb_mem_arbitrator_query_reserved = 100000",
        Vec::new(),
    );
    tk.MustQuery("select @@tidb_mem_arbitrator_query_reserved", Vec::new())
        .Check(Rows(&["100000"]));
    tk.MustQuery(
        "select /*+ set_var(tidb_mem_arbitrator_query_reserved=1234) */ \
         @@tidb_mem_arbitrator_query_reserved",
        Vec::new(),
    )
    .Check(Rows(&["1234"]));
    tk.MustExec(
        "set tidb_mem_arbitrator_query_reserved = default",
        Vec::new(),
    );

    tk.MustExec("set global tidb_enable_resource_control=on", Vec::new());
    tk.MustExec(
        "create resource group rg1 RU_PER_SEC=111 priority=LOW",
        Vec::new(),
    );
    tk.MustExec(
        "create resource group rg2 RU_PER_SEC=222 priority=HIGH",
        Vec::new(),
    );
    tk.MustExec("create resource group rg3 RU_PER_SEC=333", Vec::new());
    tk.MustQuery(
        "select NAME,RU_PER_SEC,PRIORITY from information_schema.resource_groups \
         where name='rg2'",
        Vec::new(),
    )
    .Check(Rows(&["rg2 222 HIGH"]));
    tk.MustQuery(
        "select NAME,RU_PER_SEC,PRIORITY from information_schema.resource_groups \
         where name='rg3'",
        Vec::new(),
    )
    .Check(Rows(&["rg3 333 MEDIUM"]));
    tk.MustQuery(
        "select NAME,RU_PER_SEC,PRIORITY from information_schema.resource_groups \
         where name='rg1'",
        Vec::new(),
    )
    .Check(Rows(&["rg1 111 LOW"]));

    tk.MustExec("use test; create table t (a int)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());

    let query = |group: &str, reserved: i64| {
        format!(
            "select /*+ resource_group({group}) \
             set_var(tidb_mem_arbitrator_query_reserved={reserved}) */ * from t"
        )
    };
    for group in ["rg1", "rg2", "rg3"] {
        tk.MustQuery(&query(group, 100_000), Vec::new())
            .Check(Rows(&["1"]));
    }
    {
        let metrics = GlobalMemArbitrator()
            .expect("standard arbitrator")
            .ExecMetrics();
        assert_eq!(metrics.Task.Succ, 3);
        assert_eq!(metrics.Task.Fail, 0);
        assert_eq!(metrics.Task.SuccByPriority, [0, 0, 0]);
    }

    tk.MustExec("set global tidb_mem_arbitrator_mode = priority", Vec::new());
    for (group, expected_success, expected_by_priority) in [
        ("rg1", 4, [1, 0, 0]),
        ("rg2", 5, [1, 0, 1]),
        ("rg3", 6, [1, 1, 1]),
    ] {
        tk.MustQuery(&query(group, 100_000), Vec::new())
            .Check(Rows(&["1"]));
        let metrics = GlobalMemArbitrator()
            .expect("priority arbitrator")
            .ExecMetrics();
        assert_eq!(metrics.Task.Succ, expected_success);
        assert_eq!(metrics.Task.Fail, 0);
        assert_eq!(metrics.Task.SuccByPriority, expected_by_priority);
    }

    let mut expected_task_fail = 0;
    let mut expected_wait_averse = 0;
    tk.MustExec("set tidb_mem_arbitrator_wait_averse=1", Vec::new());
    for group in ["rg1", "rg2", "rg3"] {
        let error = tk.QueryToErr(&query(group, MAX_SERVER_LIMIT));
        assert!(
            error.message().contains(
                "[executor:8180]Query execution was stopped by the global memory arbitrator \
                 [reason=CANCEL(out-of-quota & wait-averse)] [conn="
            ),
            "{error}"
        );
        expected_task_fail += 1;
        expected_wait_averse += 1;
        let metrics = GlobalMemArbitrator()
            .expect("priority arbitrator")
            .ExecMetrics();
        assert_eq!(metrics.Task.Succ, 6);
        assert_eq!(metrics.Task.Fail, expected_task_fail);
        assert_eq!(metrics.Task.SuccByPriority, [1, 1, 1]);
        assert_eq!(metrics.Cancel.WaitAverse, expected_wait_averse);
    }

    tk.MustExec("set global tidb_mem_arbitrator_mode = standard", Vec::new());
    let mut expected_standard_cancel = 0;
    for group in ["rg1", "rg2", "rg3"] {
        let error = tk.QueryToErr(&query(group, MAX_SERVER_LIMIT));
        assert!(
            error.message().contains(
                "[executor:8180]Query execution was stopped by the global memory arbitrator \
                 [reason=CANCEL(out-of-quota & standard-mode)] [conn="
            ),
            "{error}"
        );
        expected_task_fail += 1;
        expected_standard_cancel += 1;
        let metrics = GlobalMemArbitrator()
            .expect("standard arbitrator")
            .ExecMetrics();
        assert_eq!(metrics.Task.Succ, 6);
        assert_eq!(metrics.Task.Fail, expected_task_fail);
        assert_eq!(metrics.Task.SuccByPriority, [1, 1, 1]);
        assert_eq!(metrics.Cancel.WaitAverse, expected_wait_averse);
        assert_eq!(metrics.Cancel.StandardMode, expected_standard_cancel);
    }
}
