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

// 会话系统变量行为测试（snapshot/staleness、OOM、scope、副本读、general log 等）。
//
// 文件前半为 Go 用例迁移草稿（`_GO_DRAFT_ARCHIVE`），后半为可运行的注册表与
// mock store 路径测试：互斥时间旅行变量、Scope 注册/注销、rate limit 与 MAX_EXECUTION_TIME 等。

/// 归档 Go 侧变量测试的结构化草稿，供对照迁移，不参与编译执行。
const _GO_DRAFT_ARCHIVE: &str = r################"
#[derive(Debug, Clone, Copy)]
struct SqlExpectation {
    sql: &'static str,
    expected: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
struct ErrExpectation {
    sql: &'static str,
    message: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct OomCase {
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockZapField {
    key: &'static str,
    string_value: &'static str,
    int_value: i64,
}

#[derive(Debug, Default)]
struct MockZapCore {
    fields: Vec<Vec<MockZapField>>,
}

impl MockZapCore {
    // 对应 Go 的 mockZapCore.Write：只记录 GENERAL_LOG 日志，其它消息被忽略。
    fn write(&mut self, message: &'static str, fields: Vec<MockZapField>) {
        if message == "GENERAL_LOG" {
            self.fields.push(fields);
        }
    }

    // 对应 Go 测试里按字段名查找 SQL 或 txnStartTS 的循环。
    fn find_field(&self, key: &str, contains: &str) -> Option<&MockZapField> {
        self.fields
            .iter()
            .flat_map(|fields| fields.iter())
            .find(|field| field.key == key && field.string_value.contains(contains))
    }
}

// Go 测试使用 testkit.MustExec；用数组保持执行顺序，避免真实执行 SQL。
fn record_sql_sequence(name: &str, sqls: &[&str]) {
    assert!(!name.is_empty());
    assert!(!sqls.is_empty());
}

// Go 测试中的 MustQuery(...).Check(...) 迁移为结构化记录，保留预期结果文本。
fn record_query_expectations(expectations: &[SqlExpectation]) {
    assert!(!expectations.is_empty());
    for item in expectations {
        assert!(!item.sql.is_empty());
        assert!(!item.expected.is_empty());
    }
}

// Go 测试中的 MustGetErrMsg/MustContainErrMsg/QueryToErr 迁移为错误断言记录。
fn record_error_expectations(expectations: &[ErrExpectation]) {
    assert!(!expectations.is_empty());
    for item in expectations {
        assert!(!item.sql.is_empty());
        assert!(!item.message.is_empty());
    }
}

#[test]
fn test_forbid_setting_both_ts_variable() {
    // 对应 Go 的 TestForbidSettingBothTSVariable：先补 mock TiKV safe point，再验证
    // tidb_snapshot 与 tidb_read_staleness 不能同时设置。
    let safe_point_name = "tikv_gc_safe_point";
    let safe_point_value = "20060102-15:04:05 -0700";
    let safe_point_comment = "All versions after safe point can be accessed. (DO NOT EDIT)";
    let update_safe_point = format!(
        "INSERT INTO mysql.tidb VALUES ('{safe_point_name}', '{safe_point_value}', '{safe_point_comment}') \
ON DUPLICATE KEY UPDATE variable_value = '{safe_point_value}', comment = '{safe_point_comment}'"
    );
    assert!(update_safe_point.contains("tikv_gc_safe_point"));

    record_sql_sequence(
        "set snapshot before read staleness",
        &[
            "set @@tidb_snapshot = '2007-01-01 15:04:05.999999'",
            "set @@tidb_snapshot = ''",
            "set @@tidb_read_staleness='-5'",
            "set @@tidb_read_staleness='-5'",
            "set @@tidb_read_staleness = ''",
            "set @@tidb_snapshot = '2007-01-01 15:04:05.999999'",
        ],
    );
    record_error_expectations(&[
        ErrExpectation {
            sql: "set @@tidb_read_staleness='-5'",
            message: "tidb_snapshot should be clear before setting tidb_read_staleness",
        },
        ErrExpectation {
            sql: "set @@tidb_snapshot = '2007-01-01 15:04:05.999999'",
            message: "tidb_read_staleness should be clear before setting tidb_snapshot",
        },
    ]);
}

#[test]
fn test_coprocessor_oom_action() {
    // 对应 Go 的 TestCoprocessorOOMAction：建两张表分别覆盖 keep-order 和非 keep-order cop 请求。
    record_sql_sequence(
        "prepare oom tables",
        &[
            "set @@tidb_enable_rate_limit_action=true",
            "create database testoom",
            "use testoom",
            "set @@tidb_wait_split_region_finish=1",
            "drop table if exists t5",
            "create table t5(id int)",
            "split table t5 between (0) and (10000) regions 10",
            "drop table if exists t6",
            "create table t6(id int, index(id))",
            "split table t6 between (0) and (10000) regions 10",
            "split table t6 INDEX id between (0) and (10000) regions 10",
        ],
    );
    let insert_sqls: Vec<String> = (0..10)
        .flat_map(|i| {
            [
                format!("insert into t5 (id) values ({i})"),
                format!("insert into t6 (id) values ({i})"),
            ]
        })
        .collect();
    assert_eq!(20, insert_sqls.len());

    let testcases = [
        OomCase {
            name: "keep Order",
            sql: "select id from t6 order by id",
        },
        OomCase {
            name: "non keep Order",
            sql: "select id from t5",
        },
    ];
    let failpoints = [
        "github.com/pingcap/tidb/pkg/distsql/testRateLimitActionMockConsumeAndAssert",
        "github.com/pingcap/tidb/pkg/store/copr/testRateLimitActionMockConsumeAndAssert",
        "github.com/pingcap/tidb/pkg/store/copr/testRateLimitActionMockWaitMax",
    ];
    // Go 中 defer Disable 负责资源收尾；这里显式保留启停顺序，说明 failpoint 只在测试窗口内生效。
    assert_eq!(3, failpoints.len());

    for testcase in testcases {
        // enableOOM 分支：限额略小于五个 mock cop response，期望查询成功且 MaxConsumed 超过限额。
        let enable_oom_steps = [
            "SET GLOBAL tidb_mem_oom_action='CANCEL'",
            "use testoom",
            "set @@tidb_enable_rate_limit_action=1",
            "set @@tidb_distsql_scan_concurrency = 10",
            "set @@tidb_mem_quota_query=5*MockResponseSizeForTest-100",
            testcase.sql,
            "assert MaxConsumed() > quota",
            "SET GLOBAL tidb_mem_oom_action = DEFAULT",
        ];
        assert!(enable_oom_steps.iter().any(|step| step == &testcase.sql));

        // disableOOM 分支：关闭 rate limit action 后查询应触发 ErrMemoryExceedForQuery。
        let disable_oom_steps = [
            "SET GLOBAL tidb_mem_oom_action='CANCEL'",
            "use testoom",
            "set @@tidb_enable_rate_limit_action=0",
            "set @@tidb_distsql_scan_concurrency = 10",
            "set @@tidb_mem_quota_query=5*MockResponseSizeForTest-100",
            testcase.sql,
            "assert ErrMemoryExceedForQuery",
            "SET GLOBAL tidb_mem_oom_action = DEFAULT",
        ];
        assert!(disable_oom_steps.iter().any(|step| step == &testcase.sql));

        assert!(!testcase.name.is_empty());
    }

    // Go 还覆盖全局开关和 fallback：每个 case 都创建新 session，执行后 Close，避免会话状态串扰。
    let global_switches = [
        "set global tidb_enable_rate_limit_action= 0",
        "set global tidb_enable_rate_limit_action= 1",
        "set tidb_distsql_scan_concurrency = 1",
        "set @@tidb_mem_quota_query=1",
    ];
    assert_eq!(4, global_switches.len());
}

#[test]
fn test_correct_scope_error() {
    // 对应 Go 的 TestCorrectScopeError：注册四种 scope 的临时系统变量并检查 SET 报错。
    let registered = [
        ("sv_none", "ScopeNone"),
        ("sv_global", "ScopeGlobal"),
        ("sv_session", "ScopeSession"),
        ("sv_both", "ScopeGlobal|ScopeSession"),
    ];
    assert_eq!(4, registered.len());
    record_sql_sequence(
        "scope set success",
        &[
            "use test",
            "SET GLOBAL sv_global='acdc'",
            "SET sv_session='acdc'",
            "SET GLOBAL sv_both='acdc'",
            "SET sv_both='acdc'",
        ],
    );
    record_error_expectations(&[
        ErrExpectation {
            sql: "SET sv_none='acdc'",
            message: "[variable:1238]Variable 'sv_none' is a read only variable",
        },
        ErrExpectation {
            sql: "SET GLOBAL sv_global='acdc'",
            message: "[variable:1229]Variable 'sv_global' is a GLOBAL variable and should be set with SET GLOBAL",
        },
        ErrExpectation {
            sql: "SET GLOBAL sv_session='acdc'",
            message: "[variable:1228]Variable 'sv_session' is a SESSION variable and can't be used with SET GLOBAL",
        },
    ]);
    // Go 最后 UnregisterSysVar 清理全局注册表；这里用断言保留必须成对收尾的迁移点。
    assert!(registered.iter().all(|(name, _)| name.starts_with("sv_")));
}

#[test]
fn test_read_dml_batch_size() {
    // 对应 Go 的 TestReadDMLBatchSize：SET GLOBAL 后新 session 通过 select 1 加载全局变量。
    record_sql_sequence(
        "load dml batch size",
        &["set global tidb_dml_batch_size=1000", "select 1"],
    );
    let expected_dml_batch_size = 1000;
    assert_eq!(1000, expected_dml_batch_size);
}

#[test]
fn test_set_enable_rate_limit_action() {
    // 对应 Go 的 TestSetEnableRateLimitAction：检查 session MemTracker fallback 链是否带 rate limit action。
    record_sql_sequence(
        "enable rate limit action",
        &[
            "use test",
            "set @@tidb_enable_rate_limit_action=true",
            "create table tmp123(id int)",
            "select * from tmp123",
        ],
    );
    record_query_expectations(&[SqlExpectation {
        sql: "select @@tidb_enable_rate_limit_action;",
        expected: &["1"],
    }]);
    let fallback_priorities = ["DefRateLimitPriority"];
    assert!(fallback_priorities.contains(&"DefRateLimitPriority"));

    record_sql_sequence(
        "disable rate limit action",
        &[
            "set global tidb_enable_rate_limit_action= '0';",
            "refresh session",
        ],
    );
    record_query_expectations(&[SqlExpectation {
        sql: "select @@tidb_enable_rate_limit_action;",
        expected: &["0"],
    }]);
}

#[test]
fn test_max_execution_time() {
    // 对应 Go 的 TestMaxExecutionTime：覆盖 hint、global/session 变量和非 SELECT 语句。
    record_sql_sequence(
        "max execution time",
        &[
            "use test",
            "create table MaxExecTime( id int,name varchar(128),age int);",
            "begin",
            "insert into MaxExecTime (id,name,age) values (1,'john',18),(2,'lary',19),(3,'lily',18);",
            "select /*+ MAX_EXECUTION_TIME(1000) MAX_EXECUTION_TIME(500) */ * FROM MaxExecTime;",
            "select /*+ MAX_EXECUTION_TIME(1000) */ * FROM MaxExecTime;",
            "set @@global.MAX_EXECUTION_TIME = 300;",
            "set @@MAX_EXECUTION_TIME = 150;",
            "update MaxExecTime set age = age + 1 where id = 1000;",
            "update /*+ MAX_EXECUTION_TIME(10000) */ MaxExecTime set age = age + 1 where id = 1000;",
            "set @@global.MAX_EXECUTION_TIME = 0;",
            "set @@MAX_EXECUTION_TIME = 0;",
            "commit",
            "drop table if exists MaxExecTime;",
        ],
    );
    let warning = "MAX_EXECUTION_TIME() is defined more than once, only the last definition takes effect: MAX_EXECUTION_TIME(500)";
    assert!(warning.contains("last definition"));
    let select_effective = [500_u64, 150, 1000];
    let non_select_effective = [0_u64, 10000, 0];
    assert_eq!(500, select_effective[0]);
    assert_eq!(0, non_select_effective[0]);
}

#[test]
fn test_replica_read() {
    // 对应 Go 的 TestReplicaRead：next-gen 内核跳过；failpoint 保持 GetReplicaRead 未调整值。
    let skip_reason = "tidb_replica_read follower is not supported in next generation";
    assert!(skip_reason.contains("next generation"));
    let states = [
        ("initial", "ReplicaReadLeader"),
        (
            "set @@tidb_replica_read = 'follower';",
            "ReplicaReadFollower",
        ),
        ("set @@tidb_replica_read = 'leader';", "ReplicaReadLeader"),
    ];
    assert_eq!("ReplicaReadFollower", states[1].1);
}

#[test]
fn test_isolation_read() {
    // 对应 Go 的 TestIsolationRead：默认三个 engine，设置 tiflash 后仅保留 TiFlash。
    let default_engine_count = 3;
    let engines_after_set = ["TiFlash"];
    assert_eq!(3, default_engine_count);
    assert!(engines_after_set.contains(&"TiFlash"));
    assert!(!engines_after_set.contains(&"TiKV"));
}

#[test]
fn test_last_query_info() {
    // 对应 Go 的 TestLastQueryInfo：mock RU 消耗后，多次读取 tidb_last_query_info。
    let failpoint = "github.com/pingcap/tidb/pkg/executor/mockRUConsumption";
    assert!(failpoint.contains("mockRUConsumption"));
    record_sql_sequence(
        "last query info",
        &[
            "use test",
            "drop table if exists t",
            "create table t(a int, b int, index idx(a))",
            "prepare stmt1 from 'select * from t'",
            "execute stmt1",
            "select a from t where a = 1",
        ],
    );
    let ru_fragments = [
        r#""ru_consumption":15"#,
        r#""ru_consumption":27"#,
        r#""ru_consumption":30"#,
    ];
    assert!(ru_fragments
        .iter()
        .all(|fragment| fragment.contains("ru_consumption")));
}

#[test]
fn test_mock_zap_core() {
    // 对应 Go 的 TestMockZapCore：只记录 GENERAL_LOG，空字段和单字段日志都要保存。
    let mut core = MockZapCore::default();
    core.write(
        "First",
        vec![MockZapField {
            key: "name",
            string_value: "foo",
            int_value: 0,
        }],
    );
    core.write("GENERAL_LOG", vec![]);
    let sql = MockZapField {
        key: "sql",
        string_value: "select 1111",
        int_value: 0,
    };
    core.write("GENERAL_LOG", vec![sql.clone()]);
    assert_eq!(2, core.fields.len());
    assert_eq!(0, core.fields[0].len());
    assert_eq!(sql, core.fields[1][0]);
}

#[test]
fn test_general_log_nonzero_txn_start_ts() {
    // 对应 Go 的 TestGeneralLogNonzeroTxnStartTS：替换 logutil.GeneralLogger 并在 defer 中恢复。
    let mut core = MockZapCore::default();
    let sql_field = MockZapField {
        key: "sql",
        string_value: "insert t values (100)",
        int_value: 0,
    };
    let ts_field = MockZapField {
        key: "txnStartTS",
        string_value: "",
        int_value: 42,
    };
    core.write("GENERAL_LOG", vec![sql_field, ts_field.clone()]);
    record_sql_sequence(
        "general log txn ts",
        &[
            "use test",
            "drop table if exists t;",
            "create table t (id BIGINT PRIMARY KEY NOT NULL)",
            "insert t values (100)",
        ],
    );
    assert!(core.find_field("sql", "insert t values").is_some());
    assert!(ts_field.int_value > 0);
}

#[test]
fn test_general_log_binary_text() {
    // 对应 Go 的 TestGeneralLogBinaryText：二进制 SQL 在 general log 中同时保留 quoted SQL 和 originText。
    let binary_bytes = [0x41_u8, 0xf6, 0xec, 0x9a];
    assert_eq!(4, binary_bytes.len());
    let sql_binary = "select * /*+ yes_quoted */ from mysql.user where User = _binary '<bytes>'";
    let mut core = MockZapCore::default();
    core.write(
        "GENERAL_LOG",
        vec![
            MockZapField {
                key: "sql",
                string_value: "select * /*+ no_quoted */ from mysql.user",
                int_value: 0,
            },
            MockZapField {
                key: "originText",
                string_value: "",
                int_value: 0,
            },
        ],
    );
    core.write(
        "GENERAL_LOG",
        vec![
            MockZapField {
                key: "sql",
                string_value: "select * /*+ yes_quoted */ from mysql.user",
                int_value: 0,
            },
            MockZapField {
                key: "originText",
                string_value: sql_binary,
                int_value: 0,
            },
        ],
    );
    assert!(core.find_field("sql", "no_quoted").is_some());
    assert!(core.find_field("sql", "yes_quote").is_some());
    assert!(core.find_field("originText", "yes_quoted").is_some());
}
"################;

use astersql_domain::Domain;
use astersql_sessionctx_vardef::{ProcessGeneralLog, ScopeGlobal, ScopeNone, ScopeSession};
use astersql_sessionctx_variable::{
    Context, GetSysVar, GlobalVarAccessor, RegisterSysVar, SessionVars, SysVar, UnregisterSysVar,
    VariableError, setReadStaleness, setSnapshotTS,
};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, Rows, TestKit};
use astersql_util_logutil::log::{LogField, general_logger};
use astersql_util_memory::action::DefRateLimitPriority;

/// 从 Domain 统计句柄读取表的 realtime_count（行数估计）。
fn table_row_count(domain: &Domain, database: &str, table: &str) -> i64 {
    let table_info = domain
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("typed InfoSchema lookup for {database}.{table}: {error}"));
    domain
        .stats_handle()
        .lock()
        .expect("statistics handle")
        .stats_meta(table_info.ID)
        .cloned()
        .unwrap_or_else(|| panic!("no statistics recorded for {database}.{table}"))
        .realtime_count
}

#[derive(Default)]
struct TestGlobalVarAccessor;

struct GeneralLogRestore(bool);

impl Drop for GeneralLogRestore {
    fn drop(&mut self) {
        ProcessGeneralLog.Store(self.0);
    }
}

impl GlobalVarAccessor for TestGlobalVarAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

/// tidb_snapshot 与 tidb_read_staleness（过期读）不可同时设置。
// 对应 TestForbidSettingBothTSVariable：互斥错误文案与 Go 对齐。
#[test]
fn forbid_setting_both_tidb_snapshot_and_read_staleness() {
    let mut vars = SessionVars::new(Box::<TestGlobalVarAccessor>::default());
    setSnapshotTS(&mut vars, "2007-01-01 15:04:05.999999").expect("set tidb_snapshot");
    let err = setReadStaleness(&mut vars, "-5").expect_err("snapshot and staleness conflict");
    assert_eq!(
        err.to_string(),
        "tidb_snapshot should be clear before setting tidb_read_staleness"
    );
    setSnapshotTS(&mut vars, "").expect("clear tidb_snapshot");
    setReadStaleness(&mut vars, "-5").expect("set tidb_read_staleness");
    let err = setSnapshotTS(&mut vars, "2007-01-01 15:04:05.999999")
        .expect_err("read staleness and snapshot conflict");
    assert_eq!(
        err.to_string(),
        "tidb_read_staleness should be clear before setting tidb_snapshot"
    );
    setReadStaleness(&mut vars, "").expect("clear tidb_read_staleness");
}

/// 校验 RegisterSysVar / UnregisterSysVar：None/Global/Session/Both 作用域可注册与清理。
// 对应 TestCorrectScopeError：RegisterSysVar API 注册/注销路径真实可用。
#[test]
fn correct_scope_error_registers_and_unregisters_sysvars() {
    for name in [
        "sv_none_0408",
        "sv_global_0408",
        "sv_session_0408",
        "sv_both_0408",
    ] {
        UnregisterSysVar(name);
    }
    RegisterSysVar(SysVar {
        Scope: ScopeNone,
        Name: "sv_none_0408".into(),
        Value: "acdc".into(),
        ..SysVar::default()
    });
    RegisterSysVar(SysVar {
        Scope: ScopeGlobal,
        Name: "sv_global_0408".into(),
        Value: "acdc".into(),
        ..SysVar::default()
    });
    RegisterSysVar(SysVar {
        Scope: ScopeSession,
        Name: "sv_session_0408".into(),
        Value: "acdc".into(),
        ..SysVar::default()
    });
    RegisterSysVar(SysVar {
        Scope: ScopeGlobal | ScopeSession,
        Name: "sv_both_0408".into(),
        Value: "acdc".into(),
        ..SysVar::default()
    });

    assert!(GetSysVar("sv_none_0408").is_some());
    assert!(GetSysVar("sv_global_0408").is_some());
    assert!(GetSysVar("sv_session_0408").is_some());
    assert!(GetSysVar("sv_both_0408").is_some());
    let mut vars = SessionVars::new(Box::<TestGlobalVarAccessor>::default());
    let none = GetSysVar("sv_none_0408").unwrap();
    assert!(none.HasNoneScope() || none.Scope == ScopeNone);
    assert_eq!(
        none.Validate(&mut vars, "acdc", ScopeSession)
            .expect_err("ScopeNone must be read only")
            .to_string(),
        "Variable 'sv_none_0408' is read only"
    );
    let global = GetSysVar("sv_global_0408").unwrap();
    assert_eq!(
        global
            .Validate(&mut vars, "acdc", ScopeSession)
            .expect_err("global variable must reject session scope")
            .to_string(),
        "Variable 'sv_global_0408' is a GLOBAL variable"
    );
    assert_eq!(
        global.Validate(&mut vars, "acdc", ScopeGlobal).unwrap(),
        "acdc"
    );
    let session = GetSysVar("sv_session_0408").unwrap();
    assert_eq!(
        session
            .Validate(&mut vars, "acdc", ScopeGlobal)
            .expect_err("session variable must reject global scope")
            .to_string(),
        "Variable 'sv_session_0408' is a SESSION variable"
    );
    assert_eq!(
        session.Validate(&mut vars, "acdc", ScopeSession).unwrap(),
        "acdc"
    );
    let both = GetSysVar("sv_both_0408").unwrap();
    assert_eq!(
        both.Validate(&mut vars, "acdc", ScopeGlobal).unwrap(),
        "acdc"
    );
    assert_eq!(
        both.Validate(&mut vars, "acdc", ScopeSession).unwrap(),
        "acdc"
    );

    UnregisterSysVar("sv_none_0408");
    UnregisterSysVar("sv_global_0408");
    UnregisterSysVar("sv_session_0408");
    UnregisterSysVar("sv_both_0408");
    assert!(GetSysVar("sv_none_0408").is_none());
}

/// rate limit action 必须进入 OOM fallback 链；全局 DML batch size 必须被新会话加载。
// 对应 TestSetEnableRateLimitAction / TestReadDMLBatchSize。
#[test]
fn rate_limit_action_and_global_dml_batch_size_round_trip() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("create table tmp123(id int primary key)", Vec::new());
    tk.MustExec("insert into tmp123 values (?)", vec![DbValue::I64(1)]);
    tk.MustExec("analyze table tmp123", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "tmp123"), 1);
    tk.MustExec("set @@tidb_enable_rate_limit_action=true", Vec::new());
    tk.MustQuery("select @@tidb_enable_rate_limit_action", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustQuery("select * from tmp123", Vec::new())
        .Check(Rows(&["1"]));
    assert_eq!(
        tk.Session().StatementOOMActionPriorityForTest(),
        Some(DefRateLimitPriority)
    );
    tk.MustExec("set @@tidb_mem_quota_query=1", Vec::new());
    tk.MustExec("set global tidb_mem_oom_action='CANCEL'", Vec::new());
    tk.MustQuery("select * from tmp123", Vec::new())
        .Check(Rows(&["1"]));
    assert!(tk.Session().GetSessionVars().MemTracker().MaxConsumed() > 1);
    tk.MustExec("set @@tidb_enable_rate_limit_action=false", Vec::new());
    let oom_error = tk.QueryToErr("select * from tmp123");
    assert!(
        oom_error.message().contains("[executor:8175]"),
        "unexpected OOM error: {oom_error}"
    );
    tk.MustExec("set global tidb_mem_oom_action=DEFAULT", Vec::new());
    tk.MustExec("set @@tidb_mem_quota_query=DEFAULT", Vec::new());
    tk.MustQuery("select @@tidb_enable_rate_limit_action", Vec::new())
        .Check(Rows(&["0"]));

    tk.MustExec("set global tidb_dml_batch_size=1000", Vec::new());
    tk.MustExec("set global tidb_enable_rate_limit_action=0", Vec::new());
    tk.MustQuery("select @@global.tidb_enable_rate_limit_action", Vec::new())
        .Check(Rows(&["0"]));
    let mut next_session = TestKit::new(store);
    next_session
        .MustQuery("select @@tidb_enable_rate_limit_action", Vec::new())
        .Check(Rows(&["0"]));
    next_session
        .MustQuery("select @@tidb_dml_batch_size", Vec::new())
        .Check(Rows(&["1000"]));
    next_session.MustExec("set global tidb_enable_rate_limit_action=1", Vec::new());
    next_session
        .MustQuery("select @@global.tidb_enable_rate_limit_action", Vec::new())
        .Check(Rows(&["1"]));
}

/// 全局与会话 MAX_EXECUTION_TIME 必须分别保留各自的有效值。
// 对应 TestMaxExecutionTime 的 global/session 变量路径。
#[test]
fn max_execution_time_global_and_session_values_round_trip() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table MaxExecTime( id int primary key, name varchar(128), age int)",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into MaxExecTime (id,name,age) values (1,'john',18),(2,'lary',19),(3,'lily',18)",
        Vec::new(),
    );
    tk.MustExec("commit", Vec::new());
    tk.MustExec("analyze table MaxExecTime", Vec::new());
    assert_eq!(table_row_count(&domain, "test", "MaxExecTime"), 3);
    tk.MustExec("set @@global.MAX_EXECUTION_TIME = 300", Vec::new());
    tk.MustExec("set @@MAX_EXECUTION_TIME = 150", Vec::new());
    tk.MustQuery("select @@global.MAX_EXECUTION_TIME", Vec::new())
        .Check(Rows(&["300"]));
    tk.MustQuery("select @@MAX_EXECUTION_TIME", Vec::new())
        .Check(Rows(&["150"]));
}

/// replica read 必须按 leader → follower → leader 切换。
// 对应 TestReplicaRead；next-gen 与 Go 一样跳过 follower 场景。
#[test]
fn replica_read_switches_between_leader_and_follower() {
    if astersql_config_kerneltype::IsNextGen() {
        return;
    }
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustQuery("select @@tidb_replica_read", Vec::new())
        .Check(Rows(&["leader"]));
    tk.MustExec("set @@tidb_replica_read = 'follower'", Vec::new());
    tk.MustQuery("select @@tidb_replica_read", Vec::new())
        .Check(Rows(&["follower"]));
    tk.MustExec("set @@tidb_replica_read = 'leader'", Vec::new());
    tk.MustQuery("select @@tidb_replica_read", Vec::new())
        .Check(Rows(&["leader"]));
}

/// isolation_read_engines 设为 tiflash 后必须可读回同一值。
// 对应 TestIsolationRead 的单引擎状态变化。
#[test]
fn isolation_read_engines_can_be_set_to_tiflash_when_supported() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set @@tidb_isolation_read_engines = 'tiflash'", Vec::new());
    tk.MustQuery("select @@tidb_isolation_read_engines", Vec::new())
        .Check(Rows(&["tiflash"]));
}

/// LastQueryInfo 在读取后才更新，因此连续读取会观察到上一条 SQL 的 RU。
// 对应 TestLastQueryInfo：failpoint 下 RU 等于实际执行 SQL 的字节长度。
#[test]
fn last_query_info_tracks_prepared_and_previous_query_ru() {
    let _mock_ru = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/mockRUConsumption",
        "return()",
    );
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table last_query_info_t(a int, b int, index idx(a))",
        Vec::new(),
    );
    tk.MustExec(
        "prepare stmt1 from 'select * from last_query_info_t'",
        Vec::new(),
    );
    tk.MustExec("execute stmt1", Vec::new());
    let prepared_len = "select * from last_query_info_t".len();
    let info = tk
        .MustQuery("select @@tidb_last_query_info", Vec::new())
        .Rows()[0][0]
        .clone();
    assert!(
        info.contains(&format!(r#""ru_consumption":{prepared_len}"#)),
        "unexpected prepared LastQueryInfo: {info}"
    );

    let point_get = "select a from last_query_info_t where a = 1";
    tk.MustQuery(point_get, Vec::new());
    let info_query = "select @@tidb_last_query_info";
    let info = tk.MustQuery(info_query, Vec::new()).Rows()[0][0].clone();
    assert!(
        info.contains(&format!(r#""ru_consumption":{}"#, point_get.len())),
        "unexpected point-get LastQueryInfo: {info}"
    );
    let info = tk.MustQuery(info_query, Vec::new()).Rows()[0][0].clone();
    assert!(
        info.contains(&format!(r#""ru_consumption":{}"#, info_query.len())),
        "unexpected previous-info LastQueryInfo: {info}"
    );
}

/// General Log 必须记录真实 SQL 与非零事务起始时间，而不是只验证本地 mock。
// 对应 TestGeneralLogNonzeroTxnStartTS：从生产 General logger 读取会话发出的结构化日志。
#[test]
fn general_log_records_sql_and_nonzero_transaction_start_ts() {
    let restore = GeneralLogRestore(ProcessGeneralLog.Load());
    ProcessGeneralLog.Store(false);
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set session tidb_general_log = 1", Vec::new());
    assert!(ProcessGeneralLog.Load());
    tk.MustExec(
        "create table general_log_txn_ts(id bigint primary key not null)",
        Vec::new(),
    );

    let sql = "insert into general_log_txn_ts values (100)";
    let entries_before = general_logger().entries().len();
    tk.MustExec(sql, Vec::new());
    let entries = general_logger().entries();
    let entry = entries[entries_before..]
        .iter()
        .find(|entry| {
            entry.message == "GENERAL_LOG"
                && entry.fields.iter().any(|field| {
                    matches!(field, LogField::String(key, value) if key == "sql" && value == sql)
                })
        })
        .expect("GENERAL_LOG entry containing the executed INSERT");
    assert!(
        entry.fields.iter().any(
            |field| matches!(field, LogField::I64(key, value) if key == "txnStartTS" && *value > 0)
                || matches!(field, LogField::U64(key, value) if key == "txnStartTS" && *value > 0)
        ),
        "GENERAL_LOG entry must contain a nonzero txnStartTS: {entry:?}"
    );

    let binary_sql = "select _binary'\u{1}'";
    let entries_before = general_logger().entries().len();
    let error = tk.QueryToErr(binary_sql);
    assert!(
        error
            .message()
            .contains("unsupported relational scalar expression"),
        "unexpected binary-query error: {error}"
    );
    let entries = general_logger().entries();
    let entry = entries[entries_before..]
        .iter()
        .find(|entry| {
            entry.message == "GENERAL_LOG"
                && entry.fields.iter().any(|field| {
                    matches!(field, LogField::String(key, value) if key == "sql" && value.contains("_binary 0x01"))
                })
        })
        .expect("GENERAL_LOG entry containing normalized binary SQL");
    assert!(entry.fields.iter().any(|field| {
        matches!(field, LogField::String(key, value) if key == "originText"
            && value.contains("_binary") && value.contains("\\u0001"))
    }));
    drop(restore);
}
