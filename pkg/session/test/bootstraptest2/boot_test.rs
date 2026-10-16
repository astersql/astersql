// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Bootstrap 高版本升级回归测试（Rust 元数据层断言）。
//
// Rust 测试在元数据 mutator 上验证
// DDL 表版本可读性、bootstrap 版本单调递增，以及 dist task 保留表 ID。
use astersql_meta::kv::Transaction;
use astersql_meta::{DDLTableVersion, new_mutator};
use astersql_meta_metadef::{
    CreateGlobalVariablesTable, CreateTiDBBackgroundSubtaskHistoryTable, CreateTiDBGlobalTaskTable,
    CreateTiDBRunawayQueriesTable, ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound,
    TiDBGlobalTaskHistoryTableID, TiDBGlobalTaskTableID,
};
use astersql_session::bootstrap::CreateBootstrapUpgradeTestSession;
use astersql_session::runtime::{BootstrapCanonicalDomain, CreateAnalyzeSession};
use astersql_session::testutil::{MustExec, MustExecToRecodeSet, TestRecordSet};

/// 对应 Go `TestWriteDDLTableVersionToMySQLTiDBWhenUpgradingTo178` 的真实存储回归：
/// 在同一个 Domain 中回退版本并删除 ddl_table_version 后，重新 bootstrap 必须执行升级，
/// 而不是把已初始化的系统表当作首次 bootstrap 再次写入。
#[test]
fn upgrade_from_177_rewrites_ddl_table_version_in_the_same_store() {
    let (domain, old_session) = CreateAnalyzeSession().expect("create bootstrapped store");
    let mut ddl_table_version = MustExecToRecodeSet(
        &old_session,
        "select variable_value from mysql.tidb where variable_name='ddl_table_version'",
        &[],
    );
    let expected_ddl_table_version = ddl_table_version
        .Next()
        .expect("read ddl_table_version")
        .expect("ddl_table_version row")[0]
        .clone();
    ddl_table_version.Close().expect("close result set");

    // Rust 的当前 DML 运行时要求先选择数据库；状态变化与 Go 的限定表名 SQL 相同。
    MustExec(&old_session, "use mysql", &[]);
    MustExec(
        &old_session,
        "update tidb set variable_value='177' where variable_name='tidb_server_version'",
        &[],
    );
    MustExec(
        &old_session,
        "delete from tidb where variable_name='ddl_table_version'",
        &[],
    );

    let upgraded_session = BootstrapCanonicalDomain(domain)
        .expect("bootstrap must upgrade an already initialized store from version 177");
    let mut upgraded_ddl_version = MustExecToRecodeSet(
        &upgraded_session,
        "select variable_value from mysql.tidb where variable_name='ddl_table_version'",
        &[],
    );
    assert_eq!(
        upgraded_ddl_version.Next().expect("read ddl version row"),
        Some(vec![expected_ddl_table_version])
    );
    assert_eq!(
        upgraded_ddl_version.Next().expect("read end of ddl rows"),
        None
    );
    upgraded_ddl_version.Close().expect("close ddl result set");

    let mut upgraded_bootstrap_version = MustExecToRecodeSet(
        &upgraded_session,
        "select variable_value from mysql.tidb where variable_name='tidb_server_version'",
        &[],
    );
    assert_eq!(
        upgraded_bootstrap_version
            .Next()
            .expect("read bootstrap version row"),
        Some(vec![{
            let current_version = unsafe { astersql_session::upgrade_def::currentBootstrapVersion };
            current_version.to_string()
        }])
    );
    assert_eq!(
        upgraded_bootstrap_version
            .Next()
            .expect("read end of bootstrap rows"),
        None
    );
    upgraded_bootstrap_version
        .Close()
        .expect("close bootstrap result set");
}

/// 对应 Go `TestTiDBUpgradeToVer211`：旧集群缺失 `summary` 列时，升级必须补回该列，
/// 并让聚合查询通过真实 SQL/结果集边界返回一行。
#[test]
fn upgrade_from_210_restores_background_subtask_history_summary_column() {
    let (domain, old_session) = CreateAnalyzeSession().expect("create bootstrapped store");
    MustExec(&old_session, "use mysql", &[]);
    MustExec(
        &old_session,
        "update tidb set variable_value='210' where variable_name='tidb_server_version'",
        &[],
    );
    MustExec(
        &old_session,
        "alter table tidb_background_subtask_history drop column summary",
        &[],
    );

    let upgraded_session = BootstrapCanonicalDomain(domain)
        .expect("bootstrap must upgrade an already initialized store from version 210");
    let mut rows = MustExecToRecodeSet(
        &upgraded_session,
        "select count(summary) from mysql.tidb_background_subtask_history",
        &[],
    );
    assert_eq!(
        rows.Next().expect("read summary count"),
        Some(vec!["0".to_owned()])
    );
    assert_eq!(rows.Next().expect("read end of rows"), None);
    rows.Close().expect("close result set");
}

/// 对应 Go `TestTiDBUpgradeToVer179`：从 178 重新 bootstrap 后，版本前进且
/// `mysql.global_variables.variable_value` 保持升级后的 16383 字符容量。
#[test]
fn upgrade_from_178_keeps_global_variable_value_capacity() {
    let (domain, old_session) = CreateAnalyzeSession().expect("create bootstrapped store");
    MustExec(&old_session, "use mysql", &[]);
    MustExec(
        &old_session,
        "update tidb set variable_value='178' where variable_name='tidb_server_version'",
        &[],
    );

    let upgraded_session = BootstrapCanonicalDomain(domain.clone())
        .expect("bootstrap must upgrade an already initialized store from version 178");
    let mut version = MustExecToRecodeSet(
        &upgraded_session,
        "select variable_value from mysql.tidb where variable_name='tidb_server_version'",
        &[],
    );
    assert_eq!(
        version.Next().expect("read bootstrap version"),
        Some(vec![{
            let current_version = unsafe { astersql_session::upgrade_def::currentBootstrapVersion };
            current_version.to_string()
        }])
    );
    version.Close().expect("close bootstrap version result");

    let (_, table) = domain
        .stats_table("mysql", "global_variables")
        .expect("mysql.global_variables must exist after upgrade");
    let value_column = table
        .Columns
        .iter()
        .find(|column| column.Name.L == "variable_value")
        .expect("VARIABLE_VALUE column must exist");
    assert_eq!(value_column.GetFlen(), 16_383);
}

fn upgrade_from_178_with_dist_task_injection(inject_sql: &str) {
    let (domain, old_session) = CreateBootstrapUpgradeTestSession();
    MustExec(
        &old_session,
        "update mysql.tidb set variable_value='178' where variable_name='tidb_server_version'",
        &[],
    );
    MustExec(&old_session, inject_sql, &[]);

    BootstrapCanonicalDomain(domain)
        .unwrap_or_else(|error| panic!("dist-task state must not make upgrade fatal: {error}"));
}

/// 对应 Go `TestTiDBUpgradeWithDistTaskEnable`：开关的两个值都不能让升级 fatal。
#[test]
fn upgrade_from_178_is_nonfatal_for_both_dist_task_switch_values() {
    let cases = ["1", "0"].map(|value| {
        std::thread::spawn(move || {
            upgrade_from_178_with_dist_task_injection(&format!(
                "set global tidb_enable_dist_task = {value}"
            ));
        })
    });
    for case in cases {
        case.join().expect("dist-task switch upgrade case");
    }
}

/// 对应 Go `TestTiDBUpgradeWithDistTaskRunning`：六种已有任务状态都不能让升级 fatal。
#[test]
fn upgrade_from_178_is_nonfatal_for_every_dist_task_state() {
    let cases = [
        "running", "succeed", "failed", "reverted", "paused", "other",
    ]
    .map(|state| {
        std::thread::spawn(move || {
            upgrade_from_178_with_dist_task_injection(&format!(
                "insert into mysql.tidb_global_task (id, task_key, type, state) \
                 values (1, 'aaa', 'aaa', '{state}')"
            ));
        })
    });
    for case in cases {
        case.join().expect("dist-task state upgrade case");
    }
}

/// 对应 Go `TestTiDBUpgradeToVer212`：从 198 升级后，runaway queries 的新增列
/// 必须能通过真实 SQL 解析、规划和执行链路查询。
#[test]
fn upgrade_from_198_exposes_runaway_query_columns() {
    let (domain, old_session) = CreateAnalyzeSession().expect("create bootstrapped store");
    MustExec(&old_session, "use mysql", &[]);
    MustExec(
        &old_session,
        "update tidb set variable_value='198' where variable_name='tidb_server_version'",
        &[],
    );

    let upgraded_session = BootstrapCanonicalDomain(domain)
        .expect("bootstrap must upgrade an already initialized store from version 198");
    let mut rows = MustExecToRecodeSet(
        &upgraded_session,
        "select sample_sql, start_time, plan_digest from mysql.tidb_runaway_queries",
        &[],
    );
    assert_eq!(rows.Next().expect("query runaway rows"), None);
    rows.Close().expect("close runaway result set");
}

/// 对应 TestWriteDDLTableVersionToMySQLTiDBWhenUpgradingTo178：回退到旧 bootstrap 版本后，
/// 元数据层的 ddl_table_version 仍应可被读回，供升级逻辑补写 mysql.tidb。
// 对应 TestWriteDDLTableVersionToMySQLTiDBWhenUpgradingTo178：回退到旧 bootstrap 版本后，
// 元数据层的 ddl_table_version 仍应可被读回，供升级逻辑补写 mysql.tidb。
#[test]
fn finish_bootstrap_regression_keeps_ddl_table_version_readable_across_the_same_mutator() {
    let mut mutator = new_mutator(Transaction::default(), Vec::new());
    assert_eq!(
        mutator.get_ddl_table_version().unwrap(),
        DDLTableVersion::Init as i32
    );

    mutator
        .set_ddl_table_version(DDLTableVersion::DdlNotifier)
        .unwrap();
    let ddl_table_ver_before_regression = mutator.get_ddl_table_version().unwrap();

    let ver177: i64 = 177;
    mutator.finish_bootstrap(ver177).unwrap();
    assert_eq!(mutator.get_bootstrap_version().unwrap(), ver177);
    // 回退 bootstrap 版本不应改写已经写入的 ddl_table_version。
    assert_eq!(
        mutator.get_ddl_table_version().unwrap(),
        ddl_table_ver_before_regression
    );

    let current_version: i64 = 179;
    mutator.finish_bootstrap(current_version).unwrap();
    assert_eq!(mutator.get_bootstrap_version().unwrap(), current_version);
}

/// 对应 TestTiDBUpgradeToVer179/TestTiDBUpgradeToVer211/TestTiDBUpgradeToVer212：升级总是把
/// bootstrap 版本单调前移，旧版本号写回后应当能被覆盖为更高的当前版本。
// 对应 TestTiDBUpgradeToVer179/TestTiDBUpgradeToVer211/TestTiDBUpgradeToVer212：升级总是把
// bootstrap 版本单调前移，旧版本号写回后应当能被覆盖为更高的当前版本。
#[test]
fn bootstrap_version_upgrade_is_monotonically_increasing() {
    let mut mutator = new_mutator(Transaction::default(), Vec::new());

    for legacy_version in [178_i64, 198, 210] {
        mutator.finish_bootstrap(legacy_version).unwrap();
        assert_eq!(mutator.get_bootstrap_version().unwrap(), legacy_version);

        let upgraded_version = legacy_version + 1;
        mutator.finish_bootstrap(upgraded_version).unwrap();
        assert!(mutator.get_bootstrap_version().unwrap() > legacy_version);
    }
}

/// 对应 TestTiDBUpgradeToVer179：旧集群升级后 global_variables 的值列必须保留
/// `varchar(16383)`，否则升级后的全局变量会被截断。
#[test]
fn upgrade_to_ver179_keeps_global_variable_value_capacity() {
    assert!(
        CreateGlobalVariablesTable
            .to_ascii_lowercase()
            .contains("variable_value varchar(16383)"),
        "mysql.global_variables.variable_value must match the upgraded Go schema",
    );
}

/// 对应 TestTiDBUpgradeWithDistTaskRunning 依赖的 mysql.tidb_global_task /
/// mysql.tidb_global_task_history：两张表的保留 ID 必须落在保留区间内且互不相同。
// 对应 TestTiDBUpgradeWithDistTaskRunning 依赖的 mysql.tidb_global_task /
// mysql.tidb_global_task_history：两张表的保留 ID 必须落在保留区间内且互不相同。
#[test]
fn dist_task_reserved_table_ids_are_distinct_and_within_reserved_bounds() {
    for id in [TiDBGlobalTaskTableID, TiDBGlobalTaskHistoryTableID] {
        assert!(id > ReservedGlobalIDLowerBound);
        assert!(id <= ReservedGlobalIDUpperBound);
    }
    assert_ne!(TiDBGlobalTaskTableID, TiDBGlobalTaskHistoryTableID);

    // Go 注入 running/succeed/failed/reverted/paused/other 六种任务状态后均可完成升级；
    // 状态列必须是自由文本而不是将这些运行时状态收窄为固定枚举。
    let task_ddl = CreateTiDBGlobalTaskTable.to_ascii_lowercase();
    assert!(task_ddl.contains("state varchar"));
    assert!(!task_ddl.contains("state enum("));
}

/// 对应 TestTiDBUpgradeToVer211：升级会恢复历史子任务表的 summary 列。
#[test]
fn upgrade_to_ver211_restores_background_subtask_history_summary_column() {
    assert!(
        CreateTiDBBackgroundSubtaskHistoryTable
            .to_ascii_lowercase()
            .contains("summary"),
        "upgraded tidb_background_subtask_history must expose summary",
    );
}

/// 对应 TestTiDBUpgradeToVer212：runaway query 表的新增列必须可被查询。
#[test]
fn upgrade_to_ver212_exposes_runaway_query_columns() {
    let ddl = CreateTiDBRunawayQueriesTable.to_ascii_lowercase();
    for column in ["sample_sql", "start_time", "plan_digest"] {
        assert!(
            ddl.contains(column),
            "missing upgraded runaway column {column}"
        );
    }
}
