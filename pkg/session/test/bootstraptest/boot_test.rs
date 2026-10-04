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

// Bootstrap 系统库建表、默认值与历史升级的对抗性一致性测试。
//
// 确认 `systemDatabases` 中的 `mysql` 库包含核心系统表，且表 ID 单调递增；
// 同时断言版本化 bootstrap schema 列表非空。Bootstrap 指集群首次启动时写入元数据与系统表的过程。

use std::collections::BTreeSet;
use std::sync::Arc;

use astersql_meta_metadef::{
    BootstrapSystemTableDefinitions, CreateTiDBBackgroundSubtaskHistoryTable,
    CreateTiDBBackgroundSubtaskTable, CreateTiDBGlobalTaskHistoryTable, CreateTiDBGlobalTaskTable,
    CreateTiDBMaskingPolicyTable,
};
use astersql_session::bootstrap::{systemDatabases, versionedBootstrapSchemas};
use astersql_testkit::{Database, NewTestKit};

fn new_bootstrap_test_kit() -> astersql_testkit::TestKit {
    let (store, _domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    NewTestKit(store as Arc<dyn Database>)
}

fn new_rebootstrap_test_kit() -> (
    astersql_testkit::TestKit,
    impl FnOnce() -> astersql_testkit::TestKit,
) {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let tk = NewTestKit(store.clone() as Arc<dyn Database>);
    let rebootstrap = move || {
        astersql_session::runtime::BootstrapCanonicalDomain(domain)
            .expect("re-bootstrap canonical domain");
        NewTestKit(store as Arc<dyn Database>)
    };
    (tk, rebootstrap)
}

fn set_bootstrap_version(tk: &mut astersql_testkit::TestKit, version: i64) {
    tk.MustExec(
        &format!(
            "update mysql.tidb set variable_value='{version}' where variable_name='tidb_server_version'"
        ),
        Vec::new(),
    );
    assert_eq!(
        tk.MustQuery(
            "select variable_value from mysql.tidb where variable_name='tidb_server_version'",
            Vec::new(),
        )
        .Rows(),
        vec![vec![version.to_string()]],
        "bootstrap version update must be visible before rebootstrap",
    );
}

fn persistent_global_value(tk: &astersql_testkit::TestKit, name: &str) -> Vec<Vec<String>> {
    tk.MustQuery(
        &format!("select variable_value from mysql.global_variables where variable_name='{name}'"),
        Vec::new(),
    )
    .Rows()
    .clone()
}

fn set_persistent_global(tk: &mut astersql_testkit::TestKit, name: &str, value: &str) {
    tk.MustExec(
        &format!(
            "insert into mysql.global_variables (variable_name, variable_value) values ('{name}', '{value}') on duplicate key update variable_value='{value}'"
        ),
        Vec::new(),
    );
}

/// 校验 `mysql` 系统库含 user / global_variables / tidb 等核心表，且表 ID 有序。
#[test]
fn bootstrap_schema_catalog_contains_mysql_core_tables() {
    let mysql = systemDatabases
        .iter()
        .find(|db| db.name == "mysql")
        .unwrap();
    let names: Vec<_> = mysql.tables.iter().map(|table| table.name).collect();
    assert!(names.contains(&"user"));
    assert!(names.contains(&"global_variables"));
    assert!(names.contains(&"tidb"));
    // 表 ID 必须严格递增，避免元数据分配冲突。
    assert!(mysql.tables.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert!(!versionedBootstrapSchemas.is_empty());
}

/// Bootstrap 目录中的每张持久系统表都必须一一引用 metadef 的完整权威 DDL。
#[test]
fn bootstrap_schema_catalog_uses_every_authoritative_system_table_definition_once() {
    let bootstrap_tables = versionedBootstrapSchemas
        .iter()
        .flat_map(|schema| schema.databases)
        .filter(|database| database.name == "mysql")
        .flat_map(|database| database.tables)
        .collect::<Vec<_>>();
    let actual = bootstrap_tables
        .iter()
        .map(|table| (table.name, table.create_sql))
        .collect::<BTreeSet<_>>();
    let expected = BootstrapSystemTableDefinitions
        .iter()
        .map(|definition| (definition.name, definition.create_sql))
        .collect::<BTreeSet<_>>();

    assert!(
        expected.is_subset(&actual),
        "every authoritative base definition must appear in the versioned bootstrap catalog",
    );
    assert_eq!(
        actual.len(),
        bootstrap_tables.len(),
        "duplicate bootstrap table",
    );
    assert!(
        actual
            .iter()
            .all(|(_, create_sql)| create_sql.contains('(')),
        "bootstrap create_sql must contain complete column definitions",
    );
}

/// 对应 Go `TestWriteDDLTableVersionToMySQLTiDB` 的真实 SQL 断言：首次 bootstrap
/// 必须把权威 DDL 表版本写入 `mysql.tidb`，且值与 metadata 侧版本一致。
#[test]
fn bootstrap_writes_ddl_table_version_to_mysql_tidb() {
    let tk = new_bootstrap_test_kit();
    let rows = tk.MustQuery(
        "select variable_value from mysql.tidb where variable_name='ddl_table_version'",
        Vec::new(),
    );
    assert_eq!(rows.Rows().len(), 1);
    assert_eq!(
        rows.Rows()[0][0],
        (astersql_meta::DDLTableVersion::DdlNotifier as i32).to_string(),
    );
}

fn assert_columns_in_order(sql: &str, columns: &[&str]) {
    let sql = sql.to_ascii_lowercase();
    let mut offset = 0;
    for column in columns {
        let position = sql[offset..]
            .find(&format!("{column} "))
            .unwrap_or_else(|| panic!("missing column {column} in authoritative DDL"));
        offset += position + column.len();
    }
}

/// 对应 Go `TestTiDBHistoryTableConsistent`：成对的历史系统表必须保持相同列目录。
#[test]
fn bootstrap_history_tables_have_matching_column_catalogs() {
    let columns = [
        "id",
        "step",
        "namespace",
        "task_key",
        "ddl_physical_tid",
        "type",
        "exec_id",
        "state",
        "checkpoint",
        "meta",
        "ordinal",
        "error",
        "summary",
    ];
    assert_columns_in_order(CreateTiDBBackgroundSubtaskTable, &columns);
    assert_columns_in_order(CreateTiDBBackgroundSubtaskHistoryTable, &columns);

    let global_task_columns = ["id", "task_key", "type", "state", "state_update_time"];
    assert_columns_in_order(CreateTiDBGlobalTaskTable, &global_task_columns);
    assert_columns_in_order(CreateTiDBGlobalTaskHistoryTable, &global_task_columns);

    let tk = new_bootstrap_test_kit();
    for (active, history) in [
        ("tidb_background_subtask", "tidb_background_subtask_history"),
        ("tidb_global_task", "tidb_global_task_history"),
    ] {
        let query_columns = |table: &str| {
            tk.MustQuery(
                &format!(
                    "select column_name from information_schema.columns where table_schema='mysql' and table_name='{table}' order by ordinal_position"
                ),
                Vec::new(),
            )
            .Rows()
            .clone()
        };
        assert_eq!(query_columns(active), query_columns(history));
    }
}

/// 对应 Go `TestBootstrapMaskingPolicyTable`：masking policy 表的列和唯一索引必须完整。
#[test]
fn bootstrap_masking_policy_table_schema_is_complete() {
    let columns = [
        "policy_id",
        "policy_name",
        "db_name",
        "table_name",
        "table_id",
        "column_name",
        "column_id",
        "expression",
        "status",
        "masking_type",
        "restrict_on",
        "created_at",
        "updated_at",
        "created_by",
    ];
    assert_columns_in_order(CreateTiDBMaskingPolicyTable, &columns);
    for index in [
        "PRIMARY KEY(policy_id)",
        "uk_table_column(table_id, column_id)",
        "uk_table_policy(table_id, policy_name)",
    ] {
        assert!(
            CreateTiDBMaskingPolicyTable
                .to_ascii_lowercase()
                .contains(&index.to_ascii_lowercase()),
            "missing masking policy index {index}"
        );
    }

    let tk = new_bootstrap_test_kit();
    assert_eq!(
        tk.MustQuery(
            "select count(*) from information_schema.tables where table_schema='mysql' and table_name='tidb_masking_policy'",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["1".to_owned()]],
    );
    assert_eq!(
        tk.MustQuery(
            "select column_name, lower(column_type), is_nullable from information_schema.columns where table_schema='mysql' and table_name='tidb_masking_policy' order by ordinal_position",
            Vec::new(),
        )
        .Rows(),
        vec![
            vec!["policy_id".to_owned(), "bigint(64)".to_owned(), "NO".to_owned()],
            vec!["policy_name".to_owned(), "varchar(64)".to_owned(), "NO".to_owned()],
            vec!["db_name".to_owned(), "varchar(64)".to_owned(), "NO".to_owned()],
            vec!["table_name".to_owned(), "varchar(64)".to_owned(), "NO".to_owned()],
            vec!["table_id".to_owned(), "bigint(64)".to_owned(), "NO".to_owned()],
            vec!["column_name".to_owned(), "varchar(64)".to_owned(), "NO".to_owned()],
            vec!["column_id".to_owned(), "bigint(64)".to_owned(), "NO".to_owned()],
            vec!["expression".to_owned(), "text".to_owned(), "NO".to_owned()],
            vec!["status".to_owned(), "varchar(16)".to_owned(), "NO".to_owned()],
            vec!["masking_type".to_owned(), "varchar(32)".to_owned(), "NO".to_owned()],
            vec!["restrict_on".to_owned(), "varchar(256)".to_owned(), "NO".to_owned()],
            vec!["created_at".to_owned(), "datetime(6)".to_owned(), "NO".to_owned()],
            vec!["updated_at".to_owned(), "datetime(6)".to_owned(), "NO".to_owned()],
            vec!["created_by".to_owned(), "varchar(288)".to_owned(), "NO".to_owned()],
        ],
    );
    assert_eq!(
        tk.MustQuery(
            "select index_name, non_unique, seq_in_index, column_name from information_schema.statistics where table_schema='mysql' and table_name='tidb_masking_policy' order by index_name, seq_in_index",
            Vec::new(),
        )
        .Rows(),
        vec![
            vec!["PRIMARY".to_owned(), "0".to_owned(), "1".to_owned(), "policy_id".to_owned()],
            vec!["uk_table_column".to_owned(), "0".to_owned(), "1".to_owned(), "table_id".to_owned()],
            vec!["uk_table_column".to_owned(), "0".to_owned(), "2".to_owned(), "column_id".to_owned()],
            vec!["uk_table_policy".to_owned(), "0".to_owned(), "1".to_owned(), "table_id".to_owned()],
            vec!["uk_table_policy".to_owned(), "0".to_owned(), "2".to_owned(), "policy_name".to_owned()],
        ],
    );
}

/// 对应 Go `TestANSISQLMode` 的关键回归：ANSI 模式下仍可读取 bootstrap 的时区变量。
#[test]
fn bootstrap_system_timezone_query_survives_ansi_sql_mode() {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone() as Arc<dyn Database>);
    tk.MustExec(
        "set @@global.sql_mode='NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION,ANSI'",
        Vec::new(),
    );
    tk.MustExec(
        "delete from mysql.tidb where variable_name='tidb_server_version'",
        Vec::new(),
    );
    drop(tk);
    astersql_session::runtime::BootstrapCanonicalDomain(domain)
        .expect("ANSI global SQL mode must not prevent re-bootstrap");
    let tk = NewTestKit(store as Arc<dyn Database>);
    let rows = tk.MustQuery(
        "select variable_value from mysql.tidb where variable_name='system_tz'",
        Vec::new(),
    );
    assert_eq!(rows.Rows(), vec![vec!["CST".to_owned()]]);
    assert!(tk.MustQuery("select @@global.sql_mode", Vec::new()).Rows()[0][0].contains("ANSI"),);
}

/// 对应 Go `TestStmtSummary`：statement summary 默认值必须写入持久全局变量表，
/// 不能只校验编译期默认常量。
#[test]
fn bootstrap_enables_statement_summary_by_default() {
    let tk = new_bootstrap_test_kit();
    let rows = tk.MustQuery(
        "select variable_value from mysql.global_variables where variable_name='tidb_enable_stmt_summary'",
        Vec::new(),
    );
    assert_eq!(rows.Rows(), vec![vec!["ON".to_owned()]]);
}

/// 对齐 Go `TestDefaultAnalyzeBackgroundOnlyAffectsFreshBootstrap`：首次
/// bootstrap 给 default 组启用 stats；升级不覆盖已有的后台任务配置。
#[test]
fn default_analyze_background_only_affects_fresh_bootstrap() {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone() as Arc<dyn Database>);
    tk.MustExec("set global tidb_enable_resource_control = 'on'", Vec::new());
    let read_background = || {
        domain.storage_handle().with_storage(|storage| {
            let version = storage
                .CurrentVersion("global")
                .expect("read default group metadata version");
            let reader = astersql_meta::SnapshotReader::new(storage.GetSnapshot(version));
            reader
                .get_resource_group(1)
                .expect("read default resource group metadata")
                .and_then(|group| group.ResourceGroupSettings.Background)
                .map(|background| background.JobTypes.clone())
        })
    };
    assert_eq!(
        read_background(),
        Some(vec!["stats".to_owned()]),
        "fresh bootstrap must enable stats background jobs for default",
    );

    if astersql_config_kerneltype::IsNextGen() {
        return;
    }

    tk.MustExec(
        "alter resource group default BACKGROUND=(TASK_TYPES='lightning')",
        Vec::new(),
    );
    let upgrade_from = unsafe { astersql_session::upgrade_def::currentBootstrapVersion } - 1;
    set_bootstrap_version(&mut tk, upgrade_from);
    drop(tk);

    astersql_session::runtime::BootstrapCanonicalDomain(domain.clone())
        .expect("existing store must take the upgrade path");
    assert_eq!(
        read_background(),
        Some(vec!["lightning".to_owned()]),
        "upgrade must preserve the existing background task configuration",
    );
}

/// 对齐 Go `TestResourceGroupBasic`：default 组的后台任务配置在 priority、RU
/// 和 burst 属性变更后仍保存在 ResourceGroups 元数据中。
#[test]
fn default_stats_background_survives_other_resource_group_alters() {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store as Arc<dyn Database>);
    tk.MustExec("set global tidb_enable_resource_control = 'on'", Vec::new());
    let query = "select * from information_schema.resource_groups where name = 'default'";
    let read_background = || {
        domain.storage_handle().with_storage(|storage| {
            let version = storage
                .CurrentVersion("global")
                .expect("read default group metadata version");
            let reader = astersql_meta::SnapshotReader::new(storage.GetSnapshot(version));
            reader
                .get_resource_group(1)
                .expect("read default resource group metadata")
                .and_then(|group| group.ResourceGroupSettings.Background)
                .map(|background| background.JobTypes.clone())
        })
    };
    assert_eq!(read_background(), Some(vec!["stats".to_owned()]));
    let expected_before_alter = "default UNLIMITED MEDIUM UNLIMITED <nil> TASK_TYPES='stats'";
    tk.MustQuery(query, Vec::new())
        .Check(astersql_testkit::Rows(&[expected_before_alter]));

    for (sql, expected) in [
        (
            "alter resource group `default` PRIORITY=LOW",
            "default UNLIMITED LOW UNLIMITED <nil> TASK_TYPES='stats'",
        ),
        (
            "alter resource group `default` ru_per_sec=1000",
            "default 1000 LOW UNLIMITED <nil> TASK_TYPES='stats'",
        ),
        (
            "alter resource group `default` BURSTABLE",
            "default 1000 LOW MODERATED <nil> TASK_TYPES='stats'",
        ),
        (
            "alter resource group `default` BURSTABLE=OFF",
            "default 1000 LOW OFF <nil> TASK_TYPES='stats'",
        ),
        (
            "alter resource group `default` BURSTABLE=MODERATED",
            "default 1000 LOW MODERATED <nil> TASK_TYPES='stats'",
        ),
        (
            "alter resource group `default` BURSTABLE=UNLIMITED",
            "default 1000 LOW UNLIMITED <nil> TASK_TYPES='stats'",
        ),
    ] {
        tk.MustExec(sql, Vec::new());
        tk.MustQuery(query, Vec::new())
            .Check(astersql_testkit::Rows(&[expected]));
        assert_eq!(read_background(), Some(vec!["stats".to_owned()]), "{sql}");
    }
}

/// 对应 Go `TestReferencesPrivilegeOnColumn`：bootstrap 后权限表必须支持列级
/// REFERENCES 与 SELECT/UPDATE/INSERT 权限写入。
#[test]
fn bootstrap_supports_references_privilege_on_columns() {
    let mut tk = new_bootstrap_test_kit();
    tk.MustExec("create user if not exists issue28531", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t1 (a int)", Vec::new());
    tk.MustExec(
        "grant select (a), update (a), insert (a), references (a) on t1 to issue28531",
        Vec::new(),
    );
}

/// 对应 Go `TestTiDBEnablePagingVariable`：全局与会话作用域都暴露同一默认值。
#[test]
fn bootstrap_exposes_paging_default_in_global_and_session_scopes() {
    let tk = new_bootstrap_test_kit();
    for sql in [
        "select @@global.tidb_enable_paging",
        "select @@session.tidb_enable_paging",
    ] {
        assert_eq!(
            tk.MustQuery(sql, Vec::new()).Rows(),
            vec![vec![
                u8::from(astersql_sessionctx_vardef::DefTiDBEnablePaging).to_string()
            ]],
        );
    }
}

/// 对应 Go `TestDDLTableCreateDDLNotifierTable` 的当前版本前置契约：DDL notifier
/// 表与元数据版本必须在首次 bootstrap 后同时可见。
#[test]
fn bootstrap_creates_ddl_notifier_table_at_current_ddl_version() {
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone() as Arc<dyn Database>);
    tk.MustExec(
        "update mysql.tidb set variable_value='3' where variable_name='ddl_table_version'",
        Vec::new(),
    );
    tk.MustExec("drop table mysql.tidb_ddl_notifier", Vec::new());
    drop(tk);
    astersql_session::runtime::BootstrapCanonicalDomain(domain)
        .expect("DDL notifier table must be recreated during bootstrap");
    let tk = NewTestKit(store as Arc<dyn Database>);
    tk.MustQuery("select * from mysql.tidb_ddl_notifier", Vec::new());
    assert_eq!(
        tk.MustQuery(
            "select variable_value from mysql.tidb where variable_name='ddl_table_version'",
            Vec::new(),
        )
        .Rows(),
        vec![vec![
            (astersql_meta::DDLTableVersion::DdlNotifier as i32).to_string()
        ]],
    );
}

/// 对应 Go `TestIssue17979_1` / `TestIssue17979_2`：只有从 59 之前升级时
/// 才写入兼容 OOM action；已处于 59 的集群不得凭空新增该行。
#[test]
fn upgrade_59_writes_oom_compatibility_only_for_older_clusters() {
    let (mut from_58, rebootstrap_from_58) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut from_58, 58);
    from_58.MustExec(
        "delete from mysql.tidb where variable_name='default_oom_action'",
        Vec::new(),
    );
    drop(from_58);
    let from_58 = rebootstrap_from_58();
    assert_eq!(
        from_58
            .MustQuery(
                "select variable_value from mysql.tidb where variable_name='default_oom_action'",
                Vec::new(),
            )
            .Rows(),
        vec![vec!["log".to_owned()]],
    );

    let (mut from_59, rebootstrap_from_59) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut from_59, 59);
    from_59.MustExec(
        "delete from mysql.tidb where variable_name='default_oom_action'",
        Vec::new(),
    );
    drop(from_59);
    let from_59 = rebootstrap_from_59();
    assert!(
        from_59
            .MustQuery(
                "select variable_value from mysql.tidb where variable_name='default_oom_action'",
                Vec::new(),
            )
            .Rows()
            .is_empty(),
    );
}

/// 对应 Go `TestIssue20900_2`：从 52 升级不再写旧的 mysql.tidb 内存配额兼容行，
/// 会话使用当前 1GiB 默认值。
#[test]
fn upgrade_from_52_uses_current_memory_quota_without_legacy_tidb_row() {
    let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut tk, 52);
    tk.MustExec(
        "delete from mysql.tidb where variable_name='default_memory_quota_query'",
        Vec::new(),
    );
    drop(tk);
    let tk = rebootstrap();
    assert!(
        tk.MustQuery(
            "select variable_value from mysql.tidb where variable_name='default_memory_quota_query'",
            Vec::new(),
        )
        .Rows()
        .is_empty(),
    );
    assert_eq!(
        tk.MustQuery("select @@tidb_mem_quota_query", Vec::new())
            .Rows(),
        vec![vec!["1073741824".to_owned()]],
    );
}

/// 对应 Go `TestUpgradeClusteredIndexDefaultValue`：旧 OFF 行被删除后，全局和会话
/// 都回落到当前 ON 默认值。
#[test]
fn upgrade_68_restores_clustered_index_default() {
    let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut tk, 67);
    set_persistent_global(&mut tk, "tidb_enable_clustered_index", "OFF");
    drop(tk);
    let tk = rebootstrap();
    assert_eq!(
        tk.MustQuery(
            "select @@global.tidb_enable_clustered_index, @@session.tidb_enable_clustered_index",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["ON".to_owned(), "ON".to_owned()]],
    );
}

/// 对应 Go 的两个 analyze version 升级用例：缺失值初始化为 2，遗留值 1 也重写为 2。
#[test]
fn analyze_version_upgrades_missing_and_legacy_values_to_two() {
    let (mut missing, rebootstrap_missing) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut missing, 33);
    missing.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_analyze_version'",
        Vec::new(),
    );
    drop(missing);
    let missing = rebootstrap_missing();
    assert_eq!(
        persistent_global_value(&missing, "tidb_analyze_version"),
        vec![vec!["2".to_owned()]],
    );

    let (mut legacy, rebootstrap_legacy) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut legacy, 254);
    set_persistent_global(&mut legacy, "tidb_analyze_version", "1");
    drop(legacy);
    let legacy = rebootstrap_legacy();
    assert_eq!(
        persistent_global_value(&legacy, "tidb_analyze_version"),
        vec![vec!["2".to_owned()]],
    );
    assert_eq!(
        legacy
            .MustQuery("select @@global.tidb_analyze_version", Vec::new())
            .Rows(),
        vec![vec!["2".to_owned()]],
    );
}

/// 对应 Go 的 index merge 升级矩阵：3.0 缺失值初始化为 OFF；4.0 已有 OFF/ON
/// 均必须原样保留。
#[test]
fn index_merge_upgrade_preserves_existing_choice_and_defaults_old_missing_to_off() {
    let (mut missing, rebootstrap_missing) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut missing, 33);
    missing.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_enable_index_merge'",
        Vec::new(),
    );
    drop(missing);
    let missing = rebootstrap_missing();
    assert_eq!(
        persistent_global_value(&missing, "tidb_enable_index_merge"),
        vec![vec!["OFF".to_owned()]],
    );

    for value in ["OFF", "ON"] {
        let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
        set_bootstrap_version(&mut tk, 46);
        set_persistent_global(&mut tk, "tidb_enable_index_merge", value);
        drop(tk);
        let tk = rebootstrap();
        assert_eq!(
            persistent_global_value(&tk, "tidb_enable_index_merge"),
            vec![vec![value.to_owned()]],
        );
    }
}

/// 对应 Go `TestTiDBOptRangeMaxSizeWhenUpgrading`。
#[test]
fn upgrade_97_initializes_opt_range_max_size_to_zero() {
    let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut tk, 94);
    tk.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_opt_range_max_size'",
        Vec::new(),
    );
    drop(tk);
    let tk = rebootstrap();
    assert_eq!(
        persistent_global_value(&tk, "tidb_opt_range_max_size"),
        vec![vec!["0".to_owned()]],
    );
    assert_eq!(
        tk.MustQuery(
            "select @@session.tidb_opt_range_max_size, @@global.tidb_opt_range_max_size",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["0".to_owned(), "0".to_owned()]],
    );
}

/// 对应 Go `TestTiDBOptAdvancedJoinHintWhenUpgrading`。
#[test]
fn upgrade_135_initializes_advanced_join_hint_to_off() {
    let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut tk, 134);
    tk.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_opt_advanced_join_hint'",
        Vec::new(),
    );
    drop(tk);
    let tk = rebootstrap();
    assert_eq!(
        persistent_global_value(&tk, "tidb_opt_advanced_join_hint"),
        vec![vec!["OFF".to_owned()]],
    );
    assert_eq!(
        tk.MustQuery(
            "select @@session.tidb_opt_advanced_join_hint, @@global.tidb_opt_advanced_join_hint",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["0".to_owned(), "0".to_owned()]],
    );
}

/// 对应 Go 的 cost model 升级矩阵：旧缺失值初始化为 1，已有 1/2 均保留。
#[test]
fn cost_model_upgrade_defaults_old_missing_to_one_and_preserves_existing_value() {
    let (mut missing, rebootstrap_missing) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut missing, 33);
    missing.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_cost_model_version'",
        Vec::new(),
    );
    drop(missing);
    let missing = rebootstrap_missing();
    assert_eq!(
        persistent_global_value(&missing, "tidb_cost_model_version"),
        vec![vec!["1".to_owned()]],
    );

    for value in ["1", "2"] {
        let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
        set_bootstrap_version(&mut tk, 91);
        set_persistent_global(&mut tk, "tidb_cost_model_version", value);
        drop(tk);
        let tk = rebootstrap();
        assert_eq!(
            persistent_global_value(&tk, "tidb_cost_model_version"),
            vec![vec![value.to_owned()]],
        );
    }
}

/// 对应 Go `TestIndexJoinMultiPatternByUpgrade650To840`。
#[test]
fn upgrade_215_initializes_inl_join_inner_multi_pattern_to_off() {
    let (mut tk, rebootstrap) = new_rebootstrap_test_kit();
    set_bootstrap_version(&mut tk, 109);
    tk.MustExec(
        "delete from mysql.global_variables where variable_name='tidb_enable_inl_join_inner_multi_pattern'",
        Vec::new(),
    );
    drop(tk);
    let tk = rebootstrap();
    assert_eq!(
        persistent_global_value(&tk, "tidb_enable_inl_join_inner_multi_pattern"),
        vec![vec!["OFF".to_owned()]],
    );
    assert_eq!(
        tk.MustQuery(
            "select @@global.tidb_enable_inl_join_inner_multi_pattern",
            Vec::new(),
        )
        .Rows(),
        vec![vec!["0".to_owned()]],
    );
}

/// Go optimizer boolean sysvars expose integer SQL values in both scopes.
#[test]
fn optimizer_join_boolean_queries_preserve_scope_and_storage() {
    let mut tk = new_bootstrap_test_kit();
    for name in [
        "tidb_opt_advanced_join_hint",
        "tidb_enable_inl_join_inner_multi_pattern",
    ] {
        tk.MustExec(&format!("set @@global.{name} = ON"), Vec::new());
        tk.MustExec(&format!("set @@session.{name} = OFF"), Vec::new());
        assert_eq!(
            tk.MustQuery(
                &format!("select @@session.{name}, @@global.{name}"),
                Vec::new()
            )
            .Rows(),
            vec![vec!["0".to_owned(), "1".to_owned()]],
        );
        assert_eq!(
            persistent_global_value(&tk, name),
            vec![vec!["ON".to_owned()]]
        );
        tk.MustExec(&format!("set @@global.{name} = OFF"), Vec::new());
        tk.MustExec(&format!("set @@session.{name} = ON"), Vec::new());
        assert_eq!(
            tk.MustQuery(
                &format!("select @@session.{name}, @@global.{name}"),
                Vec::new()
            )
            .Rows(),
            vec![vec!["1".to_owned(), "0".to_owned()]],
        );
    }
}
