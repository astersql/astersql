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
// 注册表与 mock store 路径测试覆盖互斥时间旅行变量、Scope 注册/注销、
// rate limit 与 MAX_EXECUTION_TIME 等。
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
        "Variable 'sv_global_0408' is a GLOBAL variable and should be set with SET GLOBAL"
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

/// 没有 RU v2 运行时证据时，上一条查询信息仍须显式暴露零值。
// 对应 Go LastQueryInfo：statementRUTotal 由真实 RU v2 证据决定，mock store 不应伪造消耗。
#[test]
fn last_query_info_exposes_zero_without_ru_v2_evidence() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec(
        "create table last_query_info_ru_v2(id int primary key, v int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into last_query_info_ru_v2 values (1, 2), (2, 3), (3, 4)",
        Vec::new(),
    );
    tk.MustQuery(
        "select * from last_query_info_ru_v2 where id > 0 order by id",
        Vec::new(),
    )
    .Check(Rows(&["1 2", "2 3", "3 4"]));

    let info = tk
        .MustQuery("select @@tidb_last_query_info", Vec::new())
        .Rows()[0][0]
        .clone();
    let info: serde_json::Value = serde_json::from_str(&info).expect("valid last query info JSON");
    assert_eq!(info["ru_v2_consumption"].as_f64(), Some(0.0));
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
