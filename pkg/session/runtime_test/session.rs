// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
//
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

#[test]
fn setting_tiflash_cop_updates_planner_session_vars() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session.WithSessionVars(|variables| assert!(variables.IsTiFlashCopBanned()));
    session
        .execute("set @@session.tidb_allow_tiflash_cop=ON")
        .expect("enable TiFlash Cop");
    session.WithSessionVars(|variables| assert!(!variables.IsTiFlashCopBanned()));
    session
        .execute("set @@session.tidb_allow_tiflash_cop=OFF")
        .expect("disable TiFlash Cop");
    session.WithSessionVars(|variables| assert!(variables.IsTiFlashCopBanned()));
}

#[test]
fn plan_replayer_dump_sql_writes_a_downloadable_zip() {
    let root = std::env::temp_dir().join(format!(
        "astersql-session-plan-replayer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create ext storage root");
    let context = astersql_planner_extstore::Context::background();
    let storage = astersql_planner_extstore::NewExtStorage(
        &context,
        &format!("file://{}", root.display()),
        "",
    )
    .expect("create ext storage");
    astersql_planner_extstore::SetGlobalExtStorageForTest(Some(Arc::clone(&storage)));

    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create table plan_replayer_sql (a int, b int comment 'xx;xxx')")
        .expect("create source table");
    let mut result = session
        .execute("plan replayer dump explain select * from plan_replayer_sql")
        .expect("dump plan replayer");
    let row = result
        .last_mut()
        .expect("result set")
        .next_row()
        .expect("read result")
        .expect("file token row");
    assert_eq!(row[0], "File token");
    let bytes = storage
        .ReadFile(&context, &format!("replayer/{}", row[1]))
        .expect("read generated zip");
    let archive = astersql_domain::plan_replayer_dump::decode_replay_archive(&bytes)
        .expect("decode generated zip");
    assert!(
        archive
            .files
            .contains_key("schema/test.plan_replayer_sql.schema.txt")
    );
    assert!(
        archive
            .files
            .contains_key("stats/test.plan_replayer_sql.json")
    );
    assert_eq!(
        archive.files.get("sql/sql0.sql").map(Vec::as_slice),
        Some(b"select * from plan_replayer_sql".as_slice())
    );

    let local_zip = root.join("round-trip.zip");
    std::fs::write(&local_zip, &bytes).expect("write local replay zip");
    session
        .execute("drop table plan_replayer_sql")
        .expect("drop source table");
    session
        .execute(&format!("plan replayer load \"{}\"", local_zip.display()))
        .expect("load plan replayer");
    let mut shown = session
        .execute("show create table plan_replayer_sql")
        .expect("loaded table exists");
    let create_sql = shown[0]
        .next_row()
        .expect("read show create")
        .expect("show create row")[1]
        .clone();
    assert!(create_sql.contains("plan_replayer_sql"));
    let mut redump = session
        .execute("plan replayer dump explain select * from plan_replayer_sql")
        .expect("redump loaded table");
    let redump_token = redump
        .last_mut()
        .expect("redump result")
        .next_row()
        .expect("read redump result")
        .expect("redump token")[1]
        .clone();
    let redump_bytes = storage
        .ReadFile(&context, &format!("replayer/{redump_token}"))
        .expect("read redumped zip");
    let redump_archive = astersql_domain::plan_replayer_dump::decode_replay_archive(&redump_bytes)
        .expect("decode redumped zip");
    let redumped_schema = std::str::from_utf8(
        redump_archive
            .files
            .get("schema/test.plan_replayer_sql.schema.txt")
            .expect("redumped schema"),
    )
    .expect("redumped schema UTF-8");
    assert!(redumped_schema.contains("xx;xxx"));

    astersql_planner_extstore::SetGlobalExtStorageForTest(None);
    storage.Close();
    std::fs::remove_dir_all(root).expect("remove ext storage root");
}

#[test]
fn transaction_schema_check_ignores_temporary_tables_but_rejects_changed_normal_tables() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    let ddl_session = ConcreteSession::new(domain);
    for connection in [&session, &ddl_session] {
        connection
            .execute("use test")
            .expect("select test database");
    }
    session
        .execute("create table normal_table (id int primary key, c int)")
        .expect("create normal table");
    session
        .execute("create table unrelated_table (id int primary key, c int)")
        .expect("create unrelated table");
    session
        .execute(
            "create global temporary table temp_table (id int primary key, c int) \
             on commit delete rows",
        )
        .expect("create global temporary table");

    session.execute("begin").expect("begin temporary-table txn");
    ddl_session
        .execute("alter table temp_table modify column c tinyint")
        .expect("alter temporary table concurrently");
    session
        .execute("insert into temp_table values (1, 1)")
        .expect("write temporary table through stale schema");
    session
        .execute("commit")
        .expect("temporary-table schema changes do not invalidate transactions");

    session.execute("begin").expect("begin unrelated-DDL txn");
    ddl_session
        .execute("alter table unrelated_table modify column c bigint")
        .expect("alter unrelated normal table concurrently");
    session
        .execute("insert into normal_table values (1, 1)")
        .expect("stage write unaffected by unrelated DDL");
    session
        .execute("commit")
        .expect("unrelated normal-table DDL must not invalidate the transaction");
    session
        .execute("delete from normal_table")
        .expect("reset normal table");

    session.execute("begin").expect("begin normal-table txn");
    ddl_session
        .execute("alter table normal_table modify column c bigint")
        .expect("alter normal table concurrently");
    session
        .execute("insert into normal_table values (1, 1)")
        .expect("stage normal-table write through stale schema");
    let error = match session.execute("commit") {
        Ok(_) => panic!("normal-table schema changes must invalidate transactions"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .starts_with("[domain:8028]Information schema is changed"),
        "{error}"
    );
    let mut rows = session
        .execute("select * from normal_table")
        .expect("query normal table after rejected commit");
    assert_eq!(rows[0].Next().expect("read normal table"), None);
}

#[test]
fn stale_read_only_transaction_does_not_require_noop_functions() {
    let session = concrete_session();

    let error = match session.execute("start transaction read only") {
        Ok(_) => panic!("plain READ ONLY must remain a gated MySQL noop feature"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("tidb_enable_noop_functions is enabled")
    );

    session
        .execute("start transaction read only as of timestamp 1")
        .expect("AS OF READ ONLY is a TiDB stale-read feature, not a noop");
    session.execute("commit").expect("commit stale transaction");
}

#[test]
/// 统计同步加载：正常完成与超时路径。
fn session_stats_waiter_uses_real_sync_load_queue_for_completion_and_timeout() {
    let runtime = runtime();
    let store = runtime.NewMockStore().expect("create canonical mock store");
    let domain = runtime
        .BootstrapSession(Arc::clone(&store))
        .expect("bootstrap canonical domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("runtime domain")
        .domain();
    let stats_handle = domain.stats_handle();
    stats_handle
        .lock()
        .expect("domain stats handle lock")
        .cache_mut()
        .put(astersql_statistics_handle::TableStats {
            physical_id: 41,
            initialized: true,
            stats_version: 2,
            columns: [(
                7,
                astersql_statistics_handle::ColumnStats {
                    analyzed_or_synthesized: true,
                    stats_version: 2,
                    ndv: 3,
                    version: 11,
                    loaded_or_evicted: false,
                    field_type: 3,
                    buckets: vec![astersql_statistics_handle::Bucket {
                        count: 3,
                        lower: vec![1],
                        upper: vec![3],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });

    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables.StmtCtx.StatsLoad.Timeout = std::time::Duration::from_secs(1);
    variables
        .StmtCtx
        .StatsLoad
        .NeededItems
        .lock()
        .expect("stats-load item lock")
        .push(astersql_sessionctx_stmtctx::cache_value(
            astersql_meta_model::StatsLoadItem {
                TableItemID: astersql_meta_model::TableItemID {
                    TableID: 41,
                    ID: 7,
                    IsIndex: false,
                    IsSyncLoadFailed: false,
                },
                FullLoad: true,
            },
        ));
    let waiter = SessionStatsSyncLoadAdapter::new(Arc::clone(&stats_handle));
    astersql_planner_core_base::StatsLoadWaiter::SyncWaitStatsLoad(&waiter, &variables)
        .expect("wait for real synchronous statistics queue");

    let loaded = stats_handle
        .lock()
        .expect("domain stats handle lock")
        .stats_meta(41)
        .and_then(|table| table.columns.get(&7))
        .expect("loaded column")
        .loaded_or_evicted;
    assert!(
        loaded,
        "sync-load worker must publish a fully loaded column"
    );

    stats_handle
        .lock()
        .expect("domain stats handle lock")
        .cache_mut()
        .get_mut(41)
        .expect("cached table")
        .columns
        .get_mut(&7)
        .expect("cached column")
        .loaded_or_evicted = false;
    let timeout_variables = astersql_sessionctx_variable::session::SessionVars::default();
    timeout_variables
        .StmtCtx
        .StatsLoad
        .NeededItems
        .lock()
        .expect("stats-load timeout item lock")
        .push(astersql_sessionctx_stmtctx::cache_value(
            astersql_meta_model::StatsLoadItem {
                TableItemID: astersql_meta_model::TableItemID {
                    TableID: 41,
                    ID: 7,
                    IsIndex: false,
                    IsSyncLoadFailed: false,
                },
                FullLoad: true,
            },
        ));
    let timeout_waiter = SessionStatsSyncLoadAdapter::new(stats_handle);
    let error = astersql_planner_core_base::StatsLoadWaiter::SyncWaitStatsLoad(
        &timeout_waiter,
        &timeout_variables,
    )
    .expect_err("zero-timeout real queue must report timeout");
    assert!(error.contains("timeout"), "{error}");
}

#[test]
fn concrete_session_initializes_statement_stats_sync_timeout() {
    let runtime = runtime();
    let store = runtime.NewMockStore().expect("create canonical mock store");
    let runtime_domain = runtime
        .BootstrapSession(Arc::clone(&store))
        .expect("bootstrap canonical domain");
    let domain = runtime_domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("runtime domain")
        .domain();
    let session = ConcreteSession::new(Arc::clone(domain));

    session.WithSessionVars(|variables| {
        assert_eq!(
            variables.StmtCtx.StatsLoad.Timeout,
            std::time::Duration::from_millis(
                astersql_sessionctx_vardef::DefTiDBStatsLoadSyncWait as u64,
            )
        );
    });
}

#[test]
/// 解析、预编译、提交、回滚与结果集读写闭环。
fn concrete_runtime_executes_parse_prepare_commit_rollback_and_record_set() {
    let runtime = runtime();
    let store = runtime.NewMockStore().expect("create canonical mock store");
    let domain = runtime
        .BootstrapSession(Arc::clone(&store))
        .expect("bootstrap canonical domain");
    assert!(domain.as_any().downcast_ref::<RuntimeDomain>().is_some());
    let session = runtime
        .CreateSession4Test(store)
        .expect("create concrete session");

    session.Execute("BEGIN").expect("begin transaction");
    let insert = session
        .PrepareStmt("INSERT INTO aster_session_kv(k, v) VALUES (?, ?)")
        .expect("prepare insert through parser");
    assert!(
        session
            .ExecutePreparedStmt(insert, &["alpha".into(), "one".into()])
            .expect("execute prepared insert")
            .is_none()
    );
    session
        .Execute("COMMIT")
        .expect("commit canonical transaction");

    let mut result = session
        .Execute("SELECT v FROM aster_session_kv WHERE k = 'alpha'")
        .expect("execute parsed select")
        .remove(0);
    assert_eq!(result.Columns(), &["v"]);
    assert_eq!(result.Next().expect("read row"), Some(vec!["one".into()]));
    assert_eq!(result.Next().expect("read eof"), None);
    result.Close().expect("close record set");
    assert!(
        result.Next().is_err(),
        "closed record set must reject reads"
    );

    session
        .Execute("BEGIN")
        .expect("begin rollback transaction");
    session
        .Execute("INSERT INTO aster_session_kv(k, v) VALUES ('beta', 'two')")
        .expect("write rollback candidate");
    session
        .Execute("ROLLBACK")
        .expect("rollback canonical transaction");
    let mut rolled_back = session
        .Execute("SELECT v FROM aster_session_kv WHERE k = 'beta'")
        .expect("query rolled back key")
        .remove(0);
    assert_eq!(rolled_back.Next().expect("read rolled back result"), None);
}

#[test]
fn canonical_factory_shares_domain_and_store_but_isolates_session_state() {
    let _production_constructor: fn(
        astersql_store::TikvStore,
    ) -> SessionResult<CanonicalSessionFactory> = CanonicalSessionFactory::from_tikv_store;
    let factory = CanonicalSessionFactory::from_storage_for_test(
        new_storage().expect("create canonical transactional mock store"),
    )
    .expect("initialize canonical Domain/session factory");
    let first = factory.create_session();
    let second = factory.create_session();

    assert!(Arc::ptr_eq(first.domain(), factory.domain()));
    assert!(Arc::ptr_eq(second.domain(), factory.domain()));

    first
        .execute("create database listener_shared")
        .expect("create database through first session");
    first
        .execute("use listener_shared")
        .expect("select database in first session");
    first
        .execute("create table t (id bigint primary key, payload varchar(32))")
        .expect("create table through shared Domain");
    first
        .execute("insert into t values (1, 'same-store')")
        .expect("write through canonical shared store");

    assert!(
        second.execute("select payload from t where id=1").is_err(),
        "the second session must keep its independent current database"
    );
    second
        .execute("use listener_shared")
        .expect("select shared database independently");
    let mut rows = second
        .execute("select payload from t where id=1")
        .expect("read first session write through shared Domain/store");
    assert_eq!(
        rows[0].Next().expect("read shared row"),
        Some(vec!["same-store".to_owned()])
    );

    drop(first);
    let mut rows = second
        .execute("select count(*) from t")
        .expect("remaining session survives another session closing");
    assert_eq!(
        rows[0].Next().expect("read shared row count"),
        Some(vec!["1".to_owned()])
    );
}

#[test]
fn production_record_set_and_connection_state_do_not_depend_on_test_traits() {
    let mut session = concrete_session();
    session
        .configure_connection(73, 0x10200, 46)
        .expect("configure production connection metadata");
    assert_eq!(session.connection_id(), 73);
    assert_eq!(
        SplitSQLStatements("select ';' as marker; select 2").expect("split SQL"),
        vec!["select ';' as marker", "select 2"]
    );

    let mut record_sets = session
        .execute("select 1 as production_value")
        .expect("execute concrete production query");
    let mut record_set = record_sets.remove(0);
    assert_eq!(record_set.columns(), &["production_value".to_owned()]);
    assert_eq!(
        record_set.next_row().expect("consume production row"),
        Some(vec!["1".to_owned()])
    );
    assert_eq!(record_set.next_row().expect("consume end of rows"), None);
    record_set.close().expect("close production record set");

    let state = session.protocol_state();
    assert_eq!(state.current_database, "test");
    assert_eq!(state.warning_count, 0);
    assert_eq!(state.status & 0x0002, 0x0002);
}

#[test]
fn concrete_set_global_default_uses_the_registered_sysvar_default() {
    let session = concrete_session();
    for variable in [
        "tidb_restricted_read_only",
        "tidb_super_read_only",
        "read_only",
    ] {
        session
            .execute(&format!("set global {variable}=default"))
            .unwrap_or_else(|error| panic!("SET GLOBAL {variable}=DEFAULT failed: {error}"));
    }
}

#[test]
fn concrete_session_answers_connector_j_connection_variables() {
    let session = concrete_session();
    session
        .execute("SET character_set_results = NULL")
        .expect("Connector/J must be able to disable result charset conversion");
    let mut result = session
        .execute(
            "SELECT @@session.auto_increment_increment AS auto_increment_increment, \
             @@character_set_client AS character_set_client, \
             @@character_set_connection AS character_set_connection, \
             @@character_set_results AS character_set_results, \
             @@character_set_server AS character_set_server, \
             @@collation_server AS collation_server, \
             @@collation_connection AS collation_connection, \
             @@init_connect AS init_connect, \
             @@interactive_timeout AS interactive_timeout, \
             @@license AS license, \
             @@lower_case_table_names AS lower_case_table_names, \
             @@max_allowed_packet AS max_allowed_packet, \
             @@net_write_timeout AS net_write_timeout, \
             @@performance_schema AS performance_schema, \
             @@sql_mode AS sql_mode, \
             @@system_time_zone AS system_time_zone, \
             @@time_zone AS time_zone, \
             @@transaction_isolation AS transaction_isolation, \
             @@wait_timeout AS wait_timeout",
        )
        .expect("Connector/J connection-variable probe should succeed")
        .remove(0);
    assert_eq!(
        result.Next().expect("read Connector/J variables"),
        Some(vec![
            "1".to_owned(),
            "utf8mb4".to_owned(),
            "utf8mb4".to_owned(),
            String::new(),
            "utf8mb4".to_owned(),
            "utf8mb4_bin".to_owned(),
            "utf8mb4_bin".to_owned(),
            String::new(),
            "28800".to_owned(),
            "Apache License 2.0".to_owned(),
            "2".to_owned(),
            astersql_sessionctx_vardef::DefMaxAllowedPacket.to_string(),
            "60".to_owned(),
            "OFF".to_owned(),
            astersql_parser_mysql::r#const::DefaultSQLMode.to_owned(),
            "CST".to_owned(),
            "SYSTEM".to_owned(),
            "REPEATABLE-READ".to_owned(),
            "28800".to_owned(),
        ])
    );
}

#[test]
/// 仅带 WRITE_SLOW_LOG hint 的语句强制写慢日志。
fn forced_slow_log_is_emitted_by_the_concrete_session_only_for_the_hinted_statement() {
    let session = concrete_session();
    let logger = Logger::memory(LogLevel::Warn);
    session
        .ExecuteWithSlowLogLogger(
            "INSERT INTO aster_session_kv(k, v) VALUES ('alpha', 'one')",
            &logger,
        )
        .expect("seed session KV");
    session
        .ExecuteWithSlowLogLogger("SELECT v FROM aster_session_kv WHERE k = 'alpha'", &logger)
        .expect("ordinary select");
    assert!(
        logger.entries().is_empty(),
        "ordinary SQL must not be forced"
    );

    let sql = "SELECT /*+ WRITE_SLOW_LOG */ v FROM aster_session_kv WHERE k = 'alpha'";
    session
        .ExecuteWithSlowLogLogger(sql, &logger)
        .expect("hinted select");
    let entries = logger.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].message.contains(sql));
    assert!(entries[0].message.contains("Succ: true"));
}

#[test]
/// 多语句 SQL 中仅匹配到 binding 的语句写慢日志。
fn multi_statement_binding_matches_each_statement_text_and_logs_only_its_match() {
    let session = concrete_session();
    let logger = Logger::memory(LogLevel::Warn);
    session
        .execute("INSERT INTO aster_session_kv(k, v) VALUES ('alpha', 'one')")
        .expect("seed session KV");

    let select_sql = "SELECT v FROM aster_session_kv WHERE k = 'alpha'";
    session.AddSessionBinding(astersql_bindinfo::Binding {
        OriginalSQL: select_sql.to_owned(),
        Db: "test".to_owned(),
        BindSQL: "SELECT /*+ WRITE_SLOW_LOG */ v FROM aster_session_kv WHERE k = 'alpha'"
            .to_owned(),
        Status: astersql_bindinfo::StatusEnabled.to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        TableNames: vec![astersql_bindinfo::TableName {
            Schema: "test".to_owned(),
            Name: "aster_session_kv".to_owned(),
            Alias: String::new(),
        }],
        ..Default::default()
    });

    let insert_sql = "INSERT INTO aster_session_kv(k, v) VALUES ('beta', 'two')";
    session
        .ExecuteWithSlowLogLogger(&format!("{select_sql}; {insert_sql}"), &logger)
        .expect("execute multi-statement SQL");
    let entries = logger.entries();
    assert_eq!(
        entries.len(),
        1,
        "only the binding-matched statement is forced"
    );
    assert!(entries[0].message.contains(select_sql));
    assert!(!entries[0].message.contains(insert_sql));
    session.WithSessionVars(|variables| {
        assert_eq!(
            variables
                .GetHintSystemVar(astersql_sessionctx_vardef::TiDBFoundInBinding)
                .expect("binding state"),
            astersql_sessionctx_vardef::Off,
            "the final unmatched statement resets the lifecycle flag"
        );
    });
}

/// 构造带前缀索引的 t_multi 表 InfoSchema。
struct StaticSessionManager {
    processes: HashMap<u64, Arc<astersql_session_sessmgr::ProcessInfo>>,
}

impl astersql_session_sessmgr::InfoSchemaCoordinator for StaticSessionManager {
    fn StoreInternalSession(&self, _: astersql_session_sessmgr::InternalSession) {}
    fn DeleteInternalSession(&self, _: &astersql_session_sessmgr::InternalSession) {}
    fn ContainsInternalSession(&self, _: &astersql_session_sessmgr::InternalSession) -> bool {
        false
    }
    fn InternalSessionCount(&self) -> isize {
        0
    }
    fn CheckOldRunningTxn(
        &self,
        _: &mut HashMap<i64, Arc<astersql_session_sessmgr::mdldef::JobMDL>>,
    ) {
    }
    fn KillNonFlashbackClusterConn(&self) {}
}

impl astersql_session_sessmgr::Manager for StaticSessionManager {
    fn ShowProcessList(&self) -> HashMap<u64, Arc<astersql_session_sessmgr::ProcessInfo>> {
        self.processes.clone()
    }
    fn ShowTxnList(&self) -> Vec<Arc<astersql_session_sessmgr::txninfo::TxnInfo>> {
        Vec::new()
    }
    fn GetProcessInfo(&self, id: u64) -> Option<Arc<astersql_session_sessmgr::ProcessInfo>> {
        self.processes.get(&id).cloned()
    }
    fn Kill(&self, _: u64, _: bool, _: bool, _: bool) {}
    fn KillAllConnections(&self) {}
    fn UpdateTLSConfig(&self, _: Option<Arc<rustls::ServerConfig>>) {}
    fn ServerID(&self) -> u64 {
        0
    }
    fn GetInternalSessionStartTSList(&self) -> Vec<u64> {
        Vec::new()
    }
    fn GetConAttrs(
        &self,
        _: &astersql_session_sessmgr::auth::UserIdentity,
    ) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
    fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
}

#[test]
fn show_processlist_uses_session_manager_rows_and_full_flag() {
    let (_domain, mut session) =
        crate::runtime::CreateAnalyzeSession().expect("create processlist runtime");
    let sql = "x".repeat(101);
    let root_process = astersql_session_sessmgr::ProcessInfo {
        Time: SystemTime::now(),
        User: "root".to_owned(),
        Host: "127.0.0.1".to_owned(),
        DB: "test".to_owned(),
        Info: sql.clone(),
        Port: "4000".to_owned(),
        ID: 42,
        State: astersql_parser_mysql::r#const::ServerStatusAutocommit,
        Command: astersql_parser_mysql::r#const::ComQuery,
        ..Default::default()
    };
    let alice_process = astersql_session_sessmgr::ProcessInfo {
        Time: SystemTime::now(),
        User: "alice".to_owned(),
        ID: 43,
        Command: astersql_parser_mysql::r#const::ComSleep,
        ..Default::default()
    };
    let manager: Arc<dyn astersql_session_sessmgr::Manager> = Arc::new(StaticSessionManager {
        processes: HashMap::from([(42, Arc::new(root_process)), (43, Arc::new(alice_process))]),
    });
    session.SetSessionManager(Arc::downgrade(&manager));
    session.SetAuthenticatedUser("root".to_owned(), false);

    let mut short_result = session
        .execute("show processlist")
        .expect("execute SHOW PROCESSLIST")
        .remove(0);
    assert_eq!(
        short_result.Columns(),
        [
            "Id", "User", "Host", "db", "Command", "Time", "State", "Info"
        ]
    );
    let short = short_result
        .Next()
        .expect("read process row")
        .expect("one process");
    assert_eq!(
        &short[..5],
        ["42", "root", "127.0.0.1:4000", "test", "Query"]
    );
    assert_eq!(short[6], "autocommit");
    assert_eq!(short[7].chars().count(), 100);
    let result_fields = short_result.result_fields();
    let id_type = &result_fields[0]
        .as_ref()
        .expect("SHOW PROCESSLIST Id metadata")
        .column;
    assert_eq!(
        id_type.GetType(),
        astersql_parser_mysql::r#type::TypeLonglong
    );
    assert_eq!(id_type.GetFlen(), 20);
    assert_eq!(id_type.GetDecimal(), 0);
    let time_type = &result_fields[5]
        .as_ref()
        .expect("SHOW PROCESSLIST Time metadata")
        .column;
    assert_eq!(time_type.GetType(), astersql_parser_mysql::r#type::TypeLong);
    assert_eq!(time_type.GetFlen(), 11);
    assert_eq!(time_type.GetDecimal(), 0);
    assert!(
        short_result
            .Next()
            .expect("read filtered process row")
            .is_none(),
        "without PROCESS privilege Go exposes only the login user's sessions"
    );

    session.SetAuthenticatedUser("root".to_owned(), true);
    let mut full = session
        .execute("show full processlist")
        .expect("execute SHOW FULL PROCESSLIST")
        .remove(0);
    let mut full_rows = Vec::new();
    while let Some(row) = full.Next().expect("read full process row") {
        full_rows.push(row);
    }
    assert_eq!(full_rows.len(), 2);
    let root = full_rows
        .iter()
        .find(|row| row[0] == "42")
        .expect("root process row");
    assert_eq!(root[7], sql);
}

#[test]
fn show_collation_and_mysql_user_catalog_match_go_metadata() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("create metadata runtime");

    let mut collations = session
        .execute("show collation like 'utf8mb4_bin'")
        .expect("execute SHOW COLLATION")
        .remove(0);
    assert_eq!(
        collations.columns(),
        [
            "Collation",
            "Charset",
            "Id",
            "Default",
            "Compiled",
            "Sortlen",
            "Pad_attribute",
        ]
    );
    assert_eq!(
        collations.next_row().expect("read utf8mb4_bin collation"),
        Some(vec![
            "utf8mb4_bin".to_owned(),
            "utf8mb4".to_owned(),
            "46".to_owned(),
            "Yes".to_owned(),
            "Yes".to_owned(),
            "1".to_owned(),
            "PAD SPACE".to_owned(),
        ])
    );
    let id = &collations.result_fields()[2]
        .as_ref()
        .expect("SHOW COLLATION Id metadata")
        .column;
    assert_eq!(id.GetType(), astersql_parser_mysql::r#type::TypeLonglong);
    assert_eq!(id.GetDecimal(), 0);
    assert!(astersql_parser_mysql::r#type::HasUnsignedFlag(id.GetFlag()));

    session
        .execute("create user 'reporter'@'localhost' identified by 'secret'")
        .expect("persist CREATE USER");
    let mut users = session
        .execute(
            "select User, Host, plugin, authentication_string from mysql.user \
             where User in ('root', 'reporter') order by User, Host",
        )
        .expect("enumerate mysql.user")
        .remove(0);
    let mut user_rows = Vec::new();
    while let Some(row) = users.next_row().expect("read mysql.user row") {
        user_rows.push(row);
    }
    let reporter = user_rows
        .iter()
        .find(|row| row[0] == "reporter")
        .expect("created user must be visible through mysql.user");
    assert_eq!(reporter[1], "localhost");
    assert_eq!(reporter[2], "mysql_native_password");
    assert!(reporter[3].starts_with('*'));

    session
        .execute("drop user 'reporter'@'localhost'")
        .expect("persist DROP USER");
    users = session
        .execute("select User from mysql.user where User='reporter'")
        .expect("query dropped user")
        .remove(0);
    assert!(
        users
            .next_row()
            .expect("read dropped user result")
            .is_none()
    );
}

#[test]
fn sem_restricted_sql_uses_authenticated_dynamic_privileges_and_preserves_import_path() {
    struct SemCleanup(Option<Box<dyn Fn()>>);
    impl Drop for SemCleanup {
        fn drop(&mut self) {
            if let Some(cleanup) = self.0.take() {
                cleanup();
            }
        }
    }

    let sem_config: serde_json::Value =
        serde_json::from_str(astersql_util_sem_compat::compatibleSEMV2Config)
            .expect("parse compatible SEM v2 config");
    for restriction in sem_config["restricted_variables"]
        .as_array()
        .expect("restricted variables")
    {
        let name = restriction["name"]
            .as_str()
            .expect("restricted variable name");
        if astersql_sessionctx_variable::GetSysVar(name).is_none() {
            astersql_sessionctx_variable::RegisterSysVar(astersql_sessionctx_variable::SysVar {
                Name: name.to_owned(),
                Value: "default".to_owned(),
                Scope: if restriction["value"].as_str().unwrap_or_default().is_empty() {
                    astersql_sessionctx_vardef::ScopeGlobal
                } else {
                    astersql_sessionctx_vardef::ScopeNone
                },
                ..Default::default()
            });
        }
    }
    let _cleanup = SemCleanup(Some(astersql_util_sem_compat::SwitchToSEMForTest(
        astersql_util_sem_compat::V2,
    )));
    let (domain, root) =
        crate::runtime::CreateAnalyzeSession().expect("create SEM integration runtime");
    root.execute("create table test.t (id int)")
        .expect("create import target");
    root.execute("CREATE USER nobodyuser, semuser")
        .expect("create SEM users");
    root.execute("GRANT ALL PRIVILEGES ON *.* TO nobodyuser")
        .expect("grant baseline privileges");
    root.execute("GRANT ALL PRIVILEGES ON *.* TO semuser")
        .expect("grant baseline privileges");
    root.execute("GRANT RESTRICTED_SQL_ADMIN ON *.* TO semuser")
        .expect("grant restricted SQL privilege");
    root.execute(
        "CREATE RESOURCE GROUP rg RU_PER_SEC=1000 \
         QUERY_LIMIT=(EXEC_ELAPSED='50ms' ACTION=KILL)",
    )
    .expect("create resource group");

    let mut nobody = ConcreteSession::new(Arc::clone(&domain));
    nobody
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "nobodyuser".to_owned(),
            hostname: "localhost".to_owned(),
            ..Default::default()
        })
        .expect("authenticate nobodyuser");
    let error = nobody
        .execute("ALTER RESOURCE GROUP rg RU_PER_SEC=500")
        .err()
        .expect("restricted SQL must be denied");
    assert!(
        error
            .to_string()
            .contains("is not supported when security enhanced mode is enabled")
    );

    let import_sql = "IMPORT INTO test.t FROM \
                      's3://bucket?EXTERNAL-ID=allowed'";
    if astersql_config_kerneltype::IsNextGen() {
        assert!(
            nobody
                .execute(import_sql)
                .err()
                .expect("NextGen rejects explicit external ID")
                .to_string()
                .contains("IMPORT INTO with explicit external ID")
        );
    } else {
        assert!(
            nobody
                .execute(import_sql)
                .err()
                .expect("the Go test stops after building the import plan")
                .to_string()
                .contains("import object storage is not configured")
        );
        assert_eq!(
            nobody.LastImportPlanPathForTest().as_deref(),
            Some("s3://bucket?EXTERNAL-ID=allowed")
        );
    }

    let mut sem_user = ConcreteSession::new(domain);
    sem_user
        .AuthenticateUserForTest(&astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: "semuser".to_owned(),
            hostname: "localhost".to_owned(),
            ..Default::default()
        })
        .expect("authenticate semuser");
    sem_user
        .execute("ALTER RESOURCE GROUP rg RU_PER_SEC=500")
        .expect("RESTRICTED_SQL_ADMIN permits restricted SQL");
    if astersql_config_kerneltype::IsNextGen() {
        assert!(
            sem_user
                .execute(import_sql)
                .err()
                .expect("dynamic privilege does not bypass NextGen external ID isolation")
                .to_string()
                .contains("IMPORT INTO with explicit external ID")
        );
    } else {
        assert!(
            sem_user
                .execute(import_sql)
                .err()
                .expect("the Go test stops after building the import plan")
                .to_string()
                .contains("import object storage is not configured")
        );
        assert_eq!(
            sem_user.LastImportPlanPathForTest().as_deref(),
            Some("s3://bucket?EXTERNAL-ID=allowed")
        );
    }

    assert_eq!(
        crate::runtime::PrepareImportPathForKernelForTest(
            "s3://bucket?EXTERNAL-ID=allowed",
            false,
        )
        .expect("classic path bypasses tenant external ID rewriting"),
        "s3://bucket?EXTERNAL-ID=allowed"
    );
    assert!(
        crate::runtime::ValidateImportPathForKernelForTest(
            "s3://bucket?EXTERNAL-ID=allowed",
            true,
        )
        .expect_err("NextGen always rejects explicit external ID")
        .to_string()
        .contains("IMPORT INTO with explicit external ID")
    );
    let prepared = crate::runtime::PrepareImportPathForKernelForTest(
        "s3://bucket?access_key=ak&secret_access_key=sk",
        true,
    )
    .expect("NextGen accepts explicit credentials");
    assert!(prepared.contains("external-id="));
}
