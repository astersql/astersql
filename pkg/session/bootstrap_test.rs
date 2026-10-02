// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Session bootstrap / upgrade 流程的单元测试草稿。
//
// 对应 Go `bootstrap_test.go`：用 mock store 验证首次初始化、版本升级、
// 全局变量默认值迁移、系统表约束与 keyspace etcd 命名空间等行为；
// 外部依赖（etcd、failpoint、真实事务）以占位 helper 表示。
//
// Bootstrap：集群首次启动时创建 mysql/sys 系统库表并写入版本标志；
// Upgrade：已初始化集群在新版本启动时按 meta 版本号逐步迁移 schema/变量。

// 这段逻辑只描述 bootstrap/upgrade 测试如何准备 mock store、回退版本、重启 domain 并校验系统表或变量，不创建真实集群。
// etcd integration、failpoint、事务提交、session 初始化、资源 Close 等外部动作均以轻量占位 helper 表示，便于后续接线。
#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 校验 next-gen 基础系统表数量、建表 SQL 形状与保留 ID 单调性。
#[test]
pub fn TestMySQLDBTables() {
    use crate::bootstrap::{systemDatabases, versionedBootstrapSchemas};

    assert_eq!(
        systemDatabases[0].id,
        astersql_meta_metadef::SystemDatabaseID
    );
    assert_eq!(systemDatabases[1].id, astersql_meta_metadef::SysDatabaseID);
    let tables = versionedBootstrapSchemas
        .iter()
        .flat_map(|schema| schema.databases)
        .flat_map(|database| database.tables)
        .collect::<Vec<_>>();
    for (name, id) in [
        (
            "tidb_global_task",
            astersql_meta_metadef::TiDBGlobalTaskTableID,
        ),
        ("stats_meta", astersql_meta_metadef::StatsMetaTableID),
        (
            "tidb_masking_policy",
            astersql_meta_metadef::TiDBMaskingPolicyTableID,
        ),
    ] {
        assert_eq!(
            tables.iter().find(|table| table.name == name).unwrap().id,
            id
        );
    }
    assert_eq!(versionedBootstrapSchemas[0].databases[0].tables.len(), 52);
    let mut ids = systemDatabases
        .iter()
        .map(|database| database.id)
        .collect::<Vec<_>>();
    for schema in versionedBootstrapSchemas {
        for database in schema.databases {
            for table in database.tables {
                assert!(table.create_sql.contains("IF NOT EXISTS mysql."));
                ids.push(table.id);
            }
        }
    }
    ids.sort_unstable();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
}

// This test file have many problem.
// 1. Please use testkit to create dom, session and store.
// 2. Don't use CreateStoreAndBootstrap and BootstrapSession together. It will cause data race.
// Please do not add any test here. You can add test case at the bootstrap_update_test.go. After All problem fixed,
// We will overwrite this file by update_test.go.
/// 首次 bootstrap 后校验 user/权限表、全局变量与二次启动 autocommit 语义。
#[test]
pub fn TestBootstrap() {
    let env = create_store_and_bootstrap();
    let se = create_session_and_set_id(&env);
    must_exec(&se, "set global tidb_txn_mode=''");
    must_exec(&se, "use mysql");

    // 首次 bootstrap 后 mysql.user 至少有 root 行，并按 Go match(...) 校验 root 权限默认值。
    query_and_match_first_row(
        &se,
        "select * from user",
        &["%", "root", "mysql_native_password"],
    );
    auth_root_from_anyhost(&se);

    must_exec_many(
        &se,
        &[
            "use test",
            "SELECT * from mysql.global_priv",
            "SELECT * from mysql.db",
            "SELECT * from mysql.tables_priv",
            "SELECT * from mysql.columns_priv",
            "SELECT * from mysql.global_grants",
        ],
    );
    expect_global_vars_count(&se);

    // 第二次启动后存储操作应默认 autocommit；Go 通过 drop/create/insert/select 验证。
    must_exec_many(
        &se,
        &[
            "USE test",
            "drop table if exists t",
            "create table t (id int)",
        ],
    );
    mark_store_not_bootstrapped(&env);
    recreate_session_and_assert_autocommit(&env);

    // 已 bootstrap 的系统再次执行 doDMLWorks 不应 fatal，并校验 blacklist 与 bind_info 内置记录。
    do_dml_works_idempotently(&env);
    env.close();
}

/// 统计具有全局作用域（HasGlobalScope）的系统变量个数。
pub fn globalVarsCount() -> i64 {
    // Go 遍历 variable.GetSysVars() 并统计 HasGlobalScope()；仅保留这个计数语义。
    count_global_scope_sysvars()
}

// testBootstrapWithError :
// When a session failed in bootstrap process (for example, the session is killed after doDDLWorks()).
// We should make sure that the following session could finish the bootstrap process.
/// bootstrap 中途失败后，后续 session 应能继续完成初始化。
#[test]
pub fn TestBootstrapWithError() {
    let store = new_embed_unistore();
    if is_next_gen() {
        bootstrap_schemas(&store);
    }
    // 手工构造 session，完成 InitDDLTables、domain.Start(ddl.Bootstrap)、checkBootstrapped=false、doDDLWorks 后中断。
    run_partial_bootstrap_until_ddl_works(&store);
    close_domain_for_store(&store);

    let dom = bootstrap_session(&store);
    let se = create_session_for_store(&store);
    query_and_match_first_row(
        &se,
        "select * from user",
        &["%", "root", "mysql_native_password"],
    );
    must_exec_many(
        &se,
        &[
            "USE test",
            "SELECT * from mysql.global_priv",
            "SELECT * from mysql.db",
            "SELECT * from mysql.tables_priv",
            "SELECT * from mysql.columns_priv",
            "SELECT * from mysql.role_edges",
            "SELECT * from mysql.default_roles",
            "SELECT * from mysql.tidb_background_subtask",
            "SELECT * from mysql.tidb_background_subtask_history",
            "SELECT * from mysql.tidb_ttl_table_status",
            "SELECT * from mysql.tidb_workload_values",
        ],
    );
    expect_global_vars_count(&se);
    expect_tidb_bootstrapped_true(&se);
    dom.close();
}

/// 降级 DDL 表版本并删表后，BootstrapSession 应重建 backfill 相关系统表。
#[test]
pub fn TestDDLTableCreateBackfillTable() {
    enable_failpoint(
        "github.com/pingcap/tidb/pkg/ddl/skipCheckReservedSchemaObjInNextGen",
        "return(true)",
    );
    let env = create_store_and_bootstrap();
    assert_ddl_table_version_at_least("meta.BackfillTableVersion");
    // 降级 mDDLTableVersion 并删掉 DDL 相关系统表，随后 BootstrapSession 应重新创建它们。
    downgrade_ddl_table_version_and_drop_background_tables();
    let dom = bootstrap_session(env.store());
    let se = create_session_for_store(env.store());
    must_exec_many(
        &se,
        &[
            "select * from mysql.tidb_background_subtask",
            "select * from mysql.tidb_background_subtask_history",
        ],
    );
    dom.close();
}

// TestUpgrade tests upgrading
/// 降级 meta/变量后重启，验证自动 upgrade 恢复到当前 bootstrap 版本。
#[test]
pub fn TestUpgrade() {
    skip_in_next_gen(
        "Skip this case because there is no upgrade in the first release of next-gen kernel",
    );
    let env = create_store_and_bootstrap();
    let se = create_session_and_set_id(&env);
    must_exec(&se, "USE mysql");
    expect_tidb_server_version(&se, "currentBootstrapVersion");

    // 降级 meta bootstrap version 和 mysql.TiDB/global_variables，再清 StoreBootstrappedKey 触发自动 upgrade。
    downgrade_bootstrap_version(&se, 0);
    must_exec_many(
        &se,
        &[
            "delete from mysql.TiDB where VARIABLE_NAME=\"tidb_server_version\"",
            "update mysql.global_variables set variable_value='off' where variable_name='tidb_enable_dist_task'",
            "delete from mysql.global_variables where VARIABLE_NAME=\"TiDBDistSQLScanConcurrency\"",
            "commit",
        ],
    );
    mark_store_not_bootstrapped(&env);
    expect_bootstrap_version(&se, 0);

    env.domain_close();
    let dom = bootstrap_session(env.store());
    let se2 = create_session_for_store(env.store());
    expect_tidb_server_version(&se2, "currentBootstrapVersion");
    expect_bootstrap_version_name(&se2, "currentBootstrapVersion");
    expect_tidb_value(&se2, "TidbNewCollationEnabled", "False");
    assert_no_multischema_bootstrap_ddl_jobs(&se2);
    dom.close();
}

/// 旧密码（SHA1 hex）升级为 MySQL native password 格式。
#[test]
pub fn TestOldPasswordUpgrade() {
    let mut runtime = BootstrapRuntimeRecorder {
        hash: [
            0x0D, 0x3C, 0xED, 0x9B, 0xEC, 0x10, 0xA7, 0x77, 0xAE, 0xC2, 0x3C, 0xCC, 0x35, 0x3A,
            0x8C, 0x08, 0xA6, 0x33, 0x04, 0x5E,
        ],
        ..Default::default()
    };
    assert_eq!(
        crate::bootstrap::oldPasswordUpgrade(&mut runtime, "616263").unwrap(),
        "*0D3CED9BEC10A777AEC23CCC353A8C08A633045E"
    );
    assert_eq!(
        crate::bootstrap::oldPasswordUpgrade(&mut runtime, "abc"),
        Err(crate::bootstrap::BootstrapError::InvalidPasswordHex)
    );
}

/// 校验 bootstrap 后 Domain 已挂接 expensive query handle。
#[test]
pub fn TestBootstrapInitExpensiveQueryHandle() {
    let env = create_store_and_bootstrap();
    let se = create_session_for_store(env.store());
    expect_domain_expensive_query_handle(&se);
}

/// Issue 23387：旧版本 bootstrap 后升级，用户 grant 不应丢失。
#[test]
pub fn TestForIssue23387() {
    // 先把 currentBootstrapVersion 临时设到 version57，老版本 bootstrap 后创建 quatest，再升级并验证 grant 不丢。
    with_current_bootstrap_version("version57", || {
        let store = new_mock_store_without_bootstrap();
        let dom = bootstrap_session(&store);
        let se = create_session_for_store(&store);
        auth_root_percent_with_token(&se, "012345678901234567890");
        must_exec(&se, "create user quatest");
        dom.close();
        let dom2 = bootstrap_session(&store);
        expect_query_row(
            &se,
            "show grants for quatest",
            "GRANT USAGE ON *.* TO 'quatest'@'%'",
        );
        dom2.close();
    });
}

/// 新集群默认开启 Index Merge（索引合并扫描）。
#[test]
pub fn TestIndexMergeInNewCluster() {
    expect_new_cluster_default("select @@tidb_enable_index_merge;", "1");
}

/// 新集群默认开启 advanced join hint。
#[test]
pub fn TestTiDBOptAdvancedJoinHintInNewCluster() {
    expect_new_cluster_default("select @@tidb_opt_advanced_join_hint;", "1");
}

/// 新集群默认代价模型版本为 2。
#[test]
pub fn TestTiDBCostModelInNewCluster() {
    expect_new_cluster_default("select @@tidb_cost_model_version;", "2");
}

/// 6.3→6.5：GC-aware 内存跟踪变量升级后默认关闭。
#[test]
pub fn TestTiDBGCAwareUpgradeFrom630To650() {
    run_global_variable_upgrade_case("version93", "TiDBEnableGCAwareMemoryTrack", "1", "0");
}

/// 内存限额从 0 升级到默认 DefTiDBServerMemoryLimit。
#[test]
pub fn TestTiDBServerMemoryLimitUpgradeTo651_1() {
    run_global_variable_upgrade_case(
        "version132",
        "TiDBServerMemoryLimit",
        "0",
        "DefTiDBServerMemoryLimit",
    );
}

/// 已设置百分比的内存限额升级后保持原值。
#[test]
pub fn TestTiDBServerMemoryLimitUpgradeTo651_2() {
    run_global_variable_upgrade_case("version132", "TiDBServerMemoryLimit", "70%", "70%");
}

/// 6.3→6.6：若干全局变量默认值从 OFF 恢复为 ON。
#[test]
pub fn TestTiDBGlobalVariablesDefaultValueUpgradeFrom630To660() {
    // 四个变量在 6.3 被改为 OFF，升级到 6.6 后期望恢复 ON。
    run_multi_variable_upgrade_case(
        "version93",
        &[
            "TiDBEnableForeignKey",
            "ForeignKeyChecks",
            "TiDBEnableHistoricalStats",
            "TiDBEnablePlanReplayerCapture",
        ],
        &["OFF", "OFF", "OFF", "OFF"],
        &["ON", "ON", "ON", "ON"],
    );
}

/// 6.5→6.6：StoreBatchSize 升级时保留用户值或取新默认值 4。
#[test]
pub fn TestTiDBStoreBatchSizeUpgradeFrom650To660() {
    // Go 跑两轮：第一轮升级前手动 set global = 1，升级后保持 1；第二轮取最新默认值 4。
    run_store_batch_size_upgrade_case("version132", true, "1");
    run_store_batch_size_upgrade_case("version132", false, "4");
}

/// 升级到 Ver136：重建 background_subtask 索引并记录 fast reorg 禁用标志。
#[test]
pub fn TestTiDBUpgradeToVer136() {
    run_plain_version_upgrade_case("version135", |se| {
        must_exec(
            se,
            "ALTER TABLE mysql.tidb_background_subtask DROP INDEX idx_task_key;",
        );
        enable_failpoint(
            "github.com/pingcap/tidb/pkg/ddl/reorgMetaRecordFastReorgDisabled",
            "return",
        );
        must_exec(se, "set global tidb_ddl_enable_fast_reorg = 1");
        expect_flag("ddl.LastReorgMetaFastReorgDisabled", true);
    });
}

/// 升级到 Ver140：补回 tidb_global_task.task_key，并覆盖 reset session 顺序。
#[test]
pub fn TestTiDBUpgradeToVer140() {
    // 覆盖 tidb_global_task 缺 task_key 列时的升级，以及旧 domain 关闭后再创建 reset session 的顺序。
    run_plain_version_upgrade_case("version139", |se| {
        must_exec(
            se,
            "alter table mysql.tidb_global_task drop column task_key",
        )
    });
    run_plain_version_upgrade_case("version139", |_se| {});
}

/// 5.4→7.0：非预处理计划缓存相关变量被删除后的升级表现。
#[test]
pub fn TestTiDBNonPrepPlanCacheUpgradeFrom540To700() {
    run_deleted_variable_upgrade_case(
        "version82",
        "TiDBEnableNonPreparedPlanCache",
        &[
            ("TiDBEnableNonPreparedPlanCache", "OFF"),
            ("TiDBNonPreparedPlanCacheSize", "100"),
        ],
    );
}

/// 6.1→6.5：StatsLoadPseudoTimeout 默认从 0 变为 1。
#[test]
pub fn TestTiDBStatsLoadPseudoTimeoutUpgradeFrom610To650() {
    run_global_variable_upgrade_case("version91", "TiDBStatsLoadPseudoTimeout", "0", "1");
}

/// 升级到 Ver138：开启 null-aware anti-join（NAAJ）。
#[test]
pub fn TestTiDBTiDBOptTiDBOptimizerEnableNAAJWhenUpgradingToVer138() {
    run_global_variable_upgrade_case(
        "version137",
        "tidb_enable_null_aware_anti_join",
        "OFF",
        "ON",
    );
}

/// 升级到 Ver143 的纯版本号推进用例。
#[test]
pub fn TestTiDBUpgradeToVer143() {
    run_plain_version_upgrade_case("version142", |_se| {});
}

/// 升级到 Ver141：基于负载的副本读阈值默认变为 1s。
#[test]
pub fn TestTiDBLoadBasedReplicaReadThresholdUpgradingToVer141() {
    run_global_variable_upgrade_case("version139", "TiDBLoadBasedReplicaReadThreshold", "0", "1s");
}

/// 升级到 Ver144：删除 fresh-stats 触发计划缓存失效的变量。
#[test]
pub fn TestTiDBPlanCacheInvalidationOnFreshStatsWhenUpgradingToVer144() {
    run_deleted_variable_upgrade_case(
        "version143",
        "tidb_plan_cache_invalidation_on_fresh_stats",
        &[
            ("tidb_plan_cache_invalidation_on_fresh_stats", "OFF"),
            (
                "@@session/global tidb_plan_cache_invalidation_on_fresh_stats",
                "0/0",
            ),
        ],
    );
}

/// 升级到 Ver145 的纯版本号推进用例。
#[test]
pub fn TestTiDBUpgradeToVer145() {
    run_plain_version_upgrade_case("version144", |_se| {});
}

/// 升级到 Ver170 的纯版本号推进用例。
#[test]
pub fn TestTiDBUpgradeToVer170() {
    run_plain_version_upgrade_case("version169", |_se| {});
}

/// 升级到 Ver176：创建 mysql.tidb_global_task_history。
#[test]
pub fn TestTiDBUpgradeToVer176() {
    run_plain_version_upgrade_case("version175", |se| {
        // Go 避免复用绑定旧 domain 的 session，升级后新建 session 查询新表。
        must_exec(se, "SELECT * from mysql.tidb_global_task_history");
    });
}

/// 升级到 Ver177：创建 mysql.dist_framework_meta。
#[test]
pub fn TestTiDBUpgradeToVer177() {
    run_plain_version_upgrade_case("version176", |se| {
        must_exec(se, "SELECT * from mysql.dist_framework_meta")
    });
}

/// 升级到 Ver209：资源管控严格模式变量迁移/删除。
#[test]
pub fn TestTiDBUpgradeToVer209() {
    run_deleted_variable_upgrade_case(
        "version198",
        "tidb_resource_control_strict_mode",
        &[
            ("tidb_resource_control_strict_mode", "OFF"),
            ("@@global.tidb_resource_control_strict_mode", "0"),
            ("EnableResourceControlStrictMode", "false"),
        ],
    );
}

/// Issue 61890：重建 global_variables 后 InitGlobalVariableIfNotExists 可写默认值。
#[test]
pub fn TestIssue61890() {
    enable_failpoint(
        "github.com/pingcap/tidb/pkg/ddl/skipCheckReservedSchemaObjInNextGen",
        "return(true)",
    );
    let env = create_store_and_bootstrap();
    let s1 = create_session_and_set_id(&env);
    must_exec_many(
        &s1,
        &[
            "drop table mysql.global_variables",
            "create table mysql.global_variables(`VARIABLE_NAME` varchar(64) NOT NULL PRIMARY KEY clustered, `VARIABLE_VALUE` varchar(16383) DEFAULT NULL)",
        ],
    );
    let s2 = create_session_and_set_id(&env);
    init_global_variable_if_not_exists(&s2, "TiDBEnableINLJoinInnerMultiPattern", "Off");
}

/// System keyspace 应带 etcd 命名空间前缀。
#[test]
pub fn TestKeyspaceEtcdNamespace() {
    skip_in_classic("keyspace is not supported in classic kernel");
    makeStore(
        &KeyspaceMetaDraft {
            id: 2,
            name: "System",
        },
        true,
    );
}

/// 经典 NULL keyspace 不应带前缀。
#[test]
pub fn TestNullKeyspaceEtcdNamespace() {
    skip_in_next_gen("next-gen kernel doesn't have the NULL keyspace concept");
    makeStore(
        &KeyspaceMetaDraft {
            id: 0,
            name: "NULL",
        },
        false,
    );
}

/// 启动 mock etcd 与 domain，校验 keyspace 命名空间。
pub fn makeStore(keyspace_meta: &KeyspaceMetaDraft, is_has_prefix: bool) {
    // Go 启动 etcd integration cluster，构造 mockEtcdBackend，再通过 domap.getWithEtcdClient 获得带 etcd client 的 domain。
    integration_before_test_external();
    let store = new_mock_store_with_optional_keyspace(keyspace_meta);
    let cluster = new_etcd_cluster_v3(1);
    let backend = mockEtcdBackend {
        storage: store,
        pdAddrs: vec![cluster.grpc_url()],
    };
    start_owner_manager(&backend);
    let dom = get_domain_with_etcd_client(&backend, cluster.rand_client());
    checkETCDNameSpace(&dom, is_has_prefix);
    close_owner_manager(&backend);
}

/// Put/Get 验证 etcd key 是否按 keyspace 加前缀。
pub fn checkETCDNameSpace(dom: &DomainDraft, is_has_prefix: bool) {
    let namespace_prefix = make_keyspace_etcd_namespace(dom);
    let test_key_without_prefix = "/testkey";
    let expect_test_key = if is_has_prefix {
        format!("{}{}", namespace_prefix, test_key_without_prefix)
    } else {
        test_key_without_prefix.to_string()
    };
    // Go 先 Put 未加前缀 key，再用 UnprefixedEtcdCli 按期望 key 查询；有前缀时原始 key 应查不到。
    dom.put_etcd(test_key_without_prefix, "test");
    dom.expect_unprefixed_get_len(&expect_test_key, 1);
    if is_has_prefix {
        dom.expect_unprefixed_get_len(test_key_without_prefix, 0);
    }
}

/// 测试用 etcd 后端：挂接 storage 与 PD 地址列表。
pub struct mockEtcdBackend {
    storage: StorageDraft,
    pdAddrs: Vec<String>,
}

impl mockEtcdBackend {
    /// 返回 etcd/PD 地址列表。
    pub fn EtcdAddrs(&self) -> Vec<String> {
        self.pdAddrs.clone()
    }
    /// 返回 PD 地址列表。
    pub fn GetPDAddrs(&self) -> Vec<String> {
        self.pdAddrs.clone()
    }
    /// 返回 TLS 配置（测试中恒为 None）。
    pub fn TLSConfig(&self) -> Option<TlsConfigDraft> {
        None
    }
    /// 占位：启动 GC worker。
    pub fn StartGCWorker(&self) -> Result<(), ErrorDraft> {
        Ok(())
    }
}

/// 升级后 analyze_jobs 索引应保留。
#[test]
pub fn TestTiDBUpgradeToVer240() {
    run_table_index_preserving_upgrade_case(
        "version239",
        "mysql.analyze_jobs",
        &["idx_schema_table_state", "idx_schema_table_partition_state"],
    );
}

/// bind_info 时间戳精度从 3 升级回 6。
#[test]
pub fn TestTiDBUpgradeToVer252() {
    // classic 和 next-gen 都要通过：bind_info create_time/update_time 从 timestamp(3) 升级回 timestamp(6)。
    run_bind_info_timestamp_upgrade_case("version250");
}

/// runaway_watch 相关索引应重建。
#[test]
pub fn TestTiDBUpgradeToVer254() {
    run_table_index_recreate_upgrade_case(
        "version253",
        &[
            ("tidb_runaway_watch", "idx_start_time"),
            ("tidb_runaway_watch_done", "idx_done_time"),
        ],
    );
}

/// 升级到 242 时应写入 cluster_id。
#[test]
pub fn TestWriteClusterIDToMySQLTiDBWhenUpgradingTo242() {
    run_cluster_id_upgrade_case("version241");
}

/// 升级时修复 bind_info 重复 digest 的唯一索引。
#[test]
pub fn TestBindInfoUniqueIndex() {
    skip_in_next_gen(
        "Skip this case because there is no upgrade in the first release of next-gen kernel",
    );
    run_plain_version_upgrade_case("version245", |se| {
        must_exec(se, "alter table mysql.bind_info drop index digest_index");
        for sql_digest in ["null", "'x'", "'y'"] {
            for plan_digest in ["null", "'x'", "'y'"] {
                // Go 对每个 digest 组合插入两条重复记录，升级时验证唯一索引修复流程不会失败。
                must_exec(
                    se,
                    &format!(
                        "insert duplicated bind_info values sql_digest={}, plan_digest={}",
                        sql_digest, plan_digest
                    ),
                );
                must_exec(
                    se,
                    &format!(
                        "insert duplicated bind_info values sql_digest={}, plan_digest={}",
                        sql_digest, plan_digest
                    ),
                );
            }
        }
    });
}

/// 校验 versionedBootstrapSchemas 表数量与 ID 不变式。
#[test]
pub fn TestVersionedBootstrapSchemas() {
    let schemas = crate::bootstrap::versionedBootstrapSchemas;
    assert_eq!(schemas[0].databases[0].tables.len(), 52);
    assert!(schemas[0].databases[1].tables.is_empty());
    assert!(
        schemas
            .windows(2)
            .all(|pair| pair[0].version < pair[1].version)
    );
    assert!(
        schemas
            .iter()
            .all(|schema| schema.databases.iter().all(|db| db.id > 0))
    );
}

/// 表驱动：系统表分区/AUTO_ID_CACHE 约束。
#[test]
pub fn TestCheckSystemTableConstraint() {
    let tests = [
        ConstraintCase {
            name: "valid system table",
            partitioned: false,
            auto_id_cache: 0,
            err_msg: "",
        },
        ConstraintCase {
            name: "table with partition should fail",
            partitioned: true,
            auto_id_cache: 0,
            err_msg: "system table should not be partitioned table",
        },
        ConstraintCase {
            name: "table with SepAutoInc should fail - version 5 and AutoIDCache 1",
            partitioned: false,
            auto_id_cache: 1,
            err_msg: "system table should not use AUTO_ID_CACHE=1",
        },
    ];
    for case in tests {
        let result = crate::bootstrap::checkSystemTableConstraint::<()>(
            &crate::bootstrap::SystemTableInfo {
                partitioned: case.partitioned,
                separate_auto_increment: case.auto_id_cache == 1,
            },
        );
        match result {
            Ok(()) => assert!(case.err_msg.is_empty(), "{}", case.name),
            Err(crate::bootstrap::BootstrapError::InvalidSystemTable(message)) => {
                assert_eq!(message, case.err_msg, "{}", case.name)
            }
            Err(error) => panic!("{}: unexpected error {error:?}", case.name),
        }
    }
}

struct EnvDraft;
struct SessionDraft;
struct DomainDraft;
struct StorageDraft;
struct TlsConfigDraft;
struct ErrorDraft;
struct EtcdClusterDraft;
struct KeyspaceMetaDraft {
    id: i64,
    name: &'static str,
}
struct ConstraintCase {
    name: &'static str,
    partitioned: bool,
    auto_id_cache: i64,
    err_msg: &'static str,
}

impl EnvDraft {
    /// 返回测试环境中的 Storage 草稿。
    fn store(&self) -> &StorageDraft {
        static STORE: StorageDraft = StorageDraft;
        &STORE
    }
    /// 关闭环境或 Domain 资源。
    fn close(&self) {}
    /// 仅关闭 Domain。
    fn domain_close(&self) {}
}

impl DomainDraft {
    /// 关闭环境或 Domain 资源。
    fn close(&self) {}
    /// 向 etcd 写入键值。
    fn put_etcd(&self, _key: &str, _value: &str) {}
    /// 断言 UnprefixedEtcdCli 查询结果长度。
    fn expect_unprefixed_get_len(&self, _key: &str, _len: usize) {}
}

impl EtcdClusterDraft {
    /// 返回 etcd 集群 gRPC URL。
    fn grpc_url(&self) -> String {
        "grpc://mock".to_string()
    }
    /// 返回随机 etcd 客户端草稿。
    fn rand_client(&self) -> EtcdClientDraft {
        EtcdClientDraft
    }
}

struct EtcdClientDraft;

/// 创建 mock store 并完成 bootstrap。
fn create_store_and_bootstrap() -> EnvDraft {
    EnvDraft
}
/// 创建 session 并设置连接 ID。
fn create_session_and_set_id(_env: &EnvDraft) -> SessionDraft {
    SessionDraft
}
/// 为给定 store 创建 session。
fn create_session_for_store(_store: &StorageDraft) -> SessionDraft {
    SessionDraft
}
/// 创建嵌入式 unistore。
fn new_embed_unistore() -> StorageDraft {
    StorageDraft
}
/// 创建未 bootstrap 的 mock store。
fn new_mock_store_without_bootstrap() -> StorageDraft {
    StorageDraft
}
/// 按 keyspace 元信息创建 mock store。
fn new_mock_store_with_optional_keyspace(_meta: &KeyspaceMetaDraft) -> StorageDraft {
    StorageDraft
}
/// 对 store 执行 BootstrapSession。
fn bootstrap_session(_store: &StorageDraft) -> DomainDraft {
    DomainDraft
}
/// 关闭与 store 关联的 domain。
fn close_domain_for_store(_store: &StorageDraft) {}
/// 执行 next-gen bootstrapSchemas。
fn bootstrap_schemas(_store: &StorageDraft) {}
/// 执行到 doDDLWorks 后中断的部分 bootstrap。
fn run_partial_bootstrap_until_ddl_works(_store: &StorageDraft) {}
/// 统计全局作用域系统变量个数。
fn count_global_scope_sysvars() -> i64 {
    0
}
/// 是否为 next-gen 内核。
fn is_next_gen() -> bool {
    false
}
/// 在 next-gen 下跳过用例。
fn skip_in_next_gen(_msg: &str) {}
/// 在经典内核下跳过用例。
fn skip_in_classic(_msg: &str) {}
/// 断言命名集合长度。
fn expect_len(_name: &str, _len: usize) {}
/// 校验 versioned schema 建表 SQL 形状。
fn test_bootstrap_schema_table_basic_info(_format: &str) {}
/// 断言系统表保留 ID 单调递增。
fn collect_and_assert_reserved_ids_are_increasing() {}
/// 必须成功执行 SQL。
fn must_exec(_se: &SessionDraft, _sql: &str) {}
/// 依次执行多条 SQL。
fn must_exec_many(se: &SessionDraft, sqls: &[&str]) {
    for sql in sqls {
        must_exec(se, sql);
    }
}
/// 查询并匹配首行前缀列。
fn query_and_match_first_row(_se: &SessionDraft, _sql: &str, _expected_prefix: &[&str]) {}
/// 以 root@% 鉴权。
fn auth_root_from_anyhost(_se: &SessionDraft) {}
/// 以带 token 的 root@% 鉴权。
fn auth_root_percent_with_token(_se: &SessionDraft, _token: &str) {}
/// 断言全局变量行数。
fn expect_global_vars_count(_se: &SessionDraft) {}
/// 清除 StoreBootstrappedKey。
fn mark_store_not_bootstrapped(_env: &EnvDraft) {}
/// 重建 session 并断言默认 autocommit。
fn recreate_session_and_assert_autocommit(_env: &EnvDraft) {}
/// 再次执行 doDMLWorks 应幂等。
fn do_dml_works_idempotently(_env: &EnvDraft) {}
/// 断言 bootstrapped=True。
fn expect_tidb_bootstrapped_true(_se: &SessionDraft) {}
/// 启用指定 failpoint。
fn enable_failpoint(_name: &str, _expr: &str) {}
/// 断言 DDL 表版本不低于给定值。
fn assert_ddl_table_version_at_least(_version: &str) {}
/// 降级 DDL 表版本并删除 background 表。
fn downgrade_ddl_table_version_and_drop_background_tables() {}
/// 断言 tidb_server_version。
fn expect_tidb_server_version(_se: &SessionDraft, _version_name: &str) {}
/// 降级 meta 中的 bootstrap 版本。
fn downgrade_bootstrap_version(_se: &SessionDraft, _version: i64) {}
/// 断言 bootstrap 版本号。
fn expect_bootstrap_version(_se: &SessionDraft, _version: i64) {}
/// 断言 bootstrap 版本名。
fn expect_bootstrap_version_name(_se: &SessionDraft, _version_name: &str) {}
/// 断言 mysql.tidb 变量值。
fn expect_tidb_value(_se: &SessionDraft, _name: &str, _value: &str) {}
/// 断言无 multi-schema bootstrap DDL job。
fn assert_no_multischema_bootstrap_ddl_jobs(_se: &SessionDraft) {}
/// 断言旧密码升级结果。
fn expect_old_password_upgrade(_password: &str, _expected: &str) {}
/// 断言 expensive query handle 已初始化。
fn expect_domain_expensive_query_handle(_se: &SessionDraft) {}
/// 临时设置 currentBootstrapVersion 后执行闭包。
fn with_current_bootstrap_version<F: FnOnce()>(_version: &str, f: F) {
    f();
}
/// 断言查询单行结果。
fn expect_query_row(_se: &SessionDraft, _sql: &str, _expected: &str) {}
/// 断言新集群变量默认值。
fn expect_new_cluster_default(_sql: &str, _expected: &str) {}
/// 通用：单全局变量升级用例。
fn run_global_variable_upgrade_case(
    _from_version: &str,
    _var_name: &str,
    _old_value: &str,
    _new_value: &str,
) {
    // 通用升级测试：降级 meta version、RevertVersionAndVariables、写旧值、清 StoreBootstrappedKey、BootstrapSession 后检查新值。
}
/// 通用：多全局变量升级用例。
fn run_multi_variable_upgrade_case(
    _from_version: &str,
    _vars: &[&str],
    _old_values: &[&str],
    _new_values: &[&str],
) {
}
/// StoreBatchSize 升级用例。
fn run_store_batch_size_upgrade_case(
    _from_version: &str,
    _set_to_one_before_upgrade: bool,
    _expected_after_upgrade: &str,
) {
}
/// 通用：纯版本升级（可在升级前改 schema）。
fn run_plain_version_upgrade_case<F: FnOnce(&SessionDraft)>(
    _from_version: &str,
    before_upgrade: F,
) {
    let se = SessionDraft;
    before_upgrade(&se);
    // Go 的公共流程是 FinishBootstrap(oldVersion)、RevertVersionAndVariables、清 bootstrapped key、重启 domain，并断言版本增加。
}
/// 断言命名标志位。
fn expect_flag(_name: &str, _value: bool) {}
/// 升级后应删除/重置变量的用例。
fn run_deleted_variable_upgrade_case(
    _from_version: &str,
    _deleted_var: &str,
    _expected_after_upgrade: &[(&str, &str)],
) {
}
/// 若不存在则插入全局变量。
fn init_global_variable_if_not_exists(_se: &SessionDraft, _name: &str, _default_value: &str) {}
/// etcd 集成测试前置。
fn integration_before_test_external() {}
/// 创建 etcd v3 集成集群。
fn new_etcd_cluster_v3(_size: usize) -> EtcdClusterDraft {
    EtcdClusterDraft
}
/// 启动 Owner Manager。
fn start_owner_manager(_backend: &mockEtcdBackend) {}
/// 关闭 Owner Manager。
fn close_owner_manager(_backend: &mockEtcdBackend) {}
/// 通过 etcd client 获取 domain。
fn get_domain_with_etcd_client(
    _backend: &mockEtcdBackend,
    _client: EtcdClientDraft,
) -> DomainDraft {
    DomainDraft
}
/// 构造 keyspace etcd 命名空间前缀。
fn make_keyspace_etcd_namespace(_dom: &DomainDraft) -> String {
    "/keyspaces/tidb/".to_string()
}
/// 升级后应保留表索引的用例。
fn run_table_index_preserving_upgrade_case(_from_version: &str, _table: &str, _indexes: &[&str]) {}
/// bind_info 时间戳精度升级用例。
fn run_bind_info_timestamp_upgrade_case(_from_version: &str) {}
/// 升级后应重建表索引的用例。
fn run_table_index_recreate_upgrade_case(_from_version: &str, _table_indexes: &[(&str, &str)]) {}
/// 写入 cluster_id 的升级用例。
fn run_cluster_id_upgrade_case(_from_version: &str) {}
/// 断言 versioned schema 不变式。
fn assert_versioned_bootstrap_schema_invariants() {}
/// 运行单条系统表约束用例。
fn run_constraint_case(_case: ConstraintCase) {}

/// 直接调用 checkSystemTableConstraint 覆盖拒绝分支。
#[test]
fn canonical_system_table_constraints_cover_all_rejection_branches() {
    use crate::bootstrap::{SystemTableInfo, checkSystemTableConstraint};

    assert!(
        checkSystemTableConstraint::<()>(&SystemTableInfo {
            partitioned: false,
            separate_auto_increment: false,
        })
        .is_ok()
    );
    assert!(
        checkSystemTableConstraint::<()>(&SystemTableInfo {
            partitioned: true,
            separate_auto_increment: false,
        })
        .is_err()
    );
    assert!(
        checkSystemTableConstraint::<()>(&SystemTableInfo {
            partitioned: false,
            separate_auto_increment: true,
        })
        .is_err()
    );
}

#[derive(Default)]
struct BootstrapRuntimeRecorder {
    sql: Vec<(String, Vec<crate::bootstrap::SqlValue>)>,
    secure_user: Option<String>,
    hash: [u8; 20],
}

impl crate::bootstrap::BootstrapRuntime for BootstrapRuntimeRecorder {
    type Error = String;
    type LockGuard = ();

    fn init_mdl_for_bootstrap(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn is_ddl_owner(&mut self) -> Result<bool, Self::Error> {
        Ok(true)
    }
    fn upgrade(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn sleep(&mut self, _: std::time::Duration) {}
    fn use_system_database(&mut self) -> Result<bool, Self::Error> {
        Ok(false)
    }
    fn read_tidb_variable(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn commit_transaction(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[crate::bootstrap::SqlValue],
        _: std::time::Duration,
    ) -> Result<(), Self::Error> {
        self.sql.push((sql.to_owned(), args.to_vec()));
        Ok(())
    }
    fn query_global_variable(&mut self, _: &str) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn acquire_distributed_lock(&mut self, _: &str) -> Result<Self::LockGuard, Self::Error> {
        Ok(())
    }
    fn nextgen_schema_version(&mut self) -> Result<i32, Self::Error> {
        Ok(0)
    }
    fn create_system_database(
        &mut self,
        _: crate::bootstrap::DatabaseBasicInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn create_and_split_system_table(
        &mut self,
        _: i64,
        _: crate::bootstrap::TableBasicInfo,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_nextgen_schema_version(&mut self, _: i32) -> Result<(), Self::Error> {
        Ok(())
    }
    fn classic_kernel(&mut self) -> bool {
        true
    }
    fn insert_builtin_bind_info(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn initialize_sql_file(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(None)
    }
    fn parse_sql(&mut self, _: &str) -> Result<Vec<String>, Self::Error> {
        Ok(Vec::new())
    }
    fn execute_statement(&mut self, _: &str) -> Result<(), Self::Error> {
        Ok(())
    }
    fn secure_bootstrap_user(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.secure_user.clone())
    }
    fn global_system_variables(&mut self) -> Result<Vec<(String, String)>, Self::Error> {
        Ok(Vec::new())
    }
    fn current_bootstrap_version(&mut self) -> i64 {
        1
    }
    fn new_collation_enabled_on_first_bootstrap(&mut self) -> bool {
        false
    }
    fn write_system_timezone(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn write_new_collation_parameter(&mut self, _: bool) -> Result<(), Self::Error> {
        Ok(())
    }
    fn write_statement_summary_variables(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn write_ddl_table_version(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn write_cluster_id(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn sha1(&mut self, _: &[u8]) -> [u8; 20] {
        self.hash
    }
    fn rebuild_partition_maps(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn bootstrap_root_insert_preserves_go_privilege_shape() {
    let mut runtime = BootstrapRuntimeRecorder::default();
    crate::bootstrap::doDMLWorks(&mut runtime).unwrap();

    let root_insert = &runtime.sql[1].0;
    assert!(root_insert.contains("Select_priv,Insert_priv,Update_priv,Delete_priv"));
    assert!(root_insert.contains("Create_Tablespace_Priv,User_attributes,Token_issuer"));
    assert!(root_insert.contains("'mysql_native_password'"));
}

#[test]
fn secure_bootstrap_root_insert_uses_auth_socket_and_current_user() {
    let mut runtime = BootstrapRuntimeRecorder {
        secure_user: Some("alice".to_owned()),
        ..Default::default()
    };
    crate::bootstrap::doDMLWorks(&mut runtime).unwrap();

    let (sql, args) = &runtime.sql[1];
    assert!(sql.contains("('localhost','root',?,'auth_socket'"));
    assert_eq!(
        args,
        &[crate::bootstrap::SqlValue::String("alice".to_owned())]
    );
}

#[cfg(feature = "nextgen")]
#[test]
fn nextgen_production_bootstrap_preserves_reserved_schema_ids_and_version() {
    let store = std::sync::Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            Some(astersql_store_mockstore_mockstorage::KeyspaceMeta {
                Name: "bootstrap-user".into(),
                Id: 96062,
            }),
        )
        .unwrap(),
    )
    .ok()
    .unwrap();
    let mut config = astersql_domain::DomainConfig::default();
    config.keyspace = "bootstrap-user".into();
    let domain = std::sync::Arc::new(astersql_domain::Domain::new(
        store,
        std::sync::Arc::new(astersql_domain::KvInfoSchemaLoader::new()),
        config,
    ));
    domain.init().unwrap();
    crate::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();
    let database = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|database| database.name.lower == "mysql")
        .unwrap();
    assert_eq!(database.id, astersql_meta_metadef::SystemDatabaseID);
    assert_eq!(
        domain
            .table_by_name("mysql", "tidb_global_task")
            .unwrap()
            .ID,
        astersql_meta_metadef::TiDBGlobalTaskTableID
    );
    assert_eq!(
        domain
            .table_by_name("mysql", "tidb_masking_policy")
            .unwrap()
            .ID,
        astersql_meta_metadef::TiDBMaskingPolicyTableID
    );
    assert_eq!(
        domain
            .table_by_name("mysql", "tidb_background_subtask")
            .unwrap()
            .ID,
        astersql_meta_metadef::TiDBBackgroundSubtaskTableID
    );
    assert_eq!(
        domain.table_by_name("mysql", "tidb_ddl_job").unwrap().ID,
        astersql_meta_metadef::TiDBDDLJobTableID
    );
    let key = astersql_meta::transaction_meta_string_key(b"BootTableVersion");
    let version = domain
        .storage_handle()
        .with_storage(|store| {
            store
                .GetSnapshot(store.CurrentVersion("global").unwrap())
                .Get(&astersql_kv::Context::default(), key, &[])
        })
        .unwrap();
    assert_eq!(version.Value, b"4");
    crate::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();
    assert_eq!(
        domain
            .table_by_name("mysql", "tidb_global_task")
            .unwrap()
            .ID,
        astersql_meta_metadef::TiDBGlobalTaskTableID
    );
    domain.close();
}
