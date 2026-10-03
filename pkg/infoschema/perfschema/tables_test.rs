// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// PERFORMANCE_SCHEMA 虚拟表与初始化相关测试。
//
// 对应 `tables_test.go`。完整 Go 用例依赖 session/TestKit/HTTP failpoint；
// 此处校验预定义表名、虚拟 schema 元数据与 init 注册语义。
// Performance Schema：MySQL 兼容的运行时性能统计虚拟库；pprof：性能剖析接口。

// Rust counterpart of pkg/infoschema/perfschema/tables_test.go.
//
// Go TestPerfSchemaTables / TestTiKVProfileCPU / TestSessionConnectAttrs need a
// full session + TestKit + HTTP failpoint stack. This port keeps the
// predefined-table check and exercises the production virtual-schema builder
// for the same table names those SQL tests would query.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::init::{build_performance_schema, set_eval_simple_ast_ready};
use crate::tables::{
    ColumnInfo, Datum, IndexInfo, IsPredefinedTable, PERFORMANCE_SCHEMA_DB_ID, PerfSchemaError,
    ProfileRequestIdentity, RemoteProfileClient, RowSource, ServerInfo, TableMeta,
    VirtualTablePlugin, data_for_remote_profile, register_plugin_table, table_from_meta,
    table_id_map, unregister_plugin_table,
};

/// 预定义表名大小写不敏感；未知表名返回 false。
#[test]
fn test_predefined_tables() {
    assert!(IsPredefinedTable("EVENTS_statements_summary_by_digest"));
    assert!(!IsPredefinedTable("statements"));
}

/// 构建虚拟库后确认核心状态/setup 表存在且可包装。
#[test]
fn test_perf_schema_tables() {
    // Go: use performance_schema; select from global_status / session_status /
    // setup_actors / events_stages_history_long — all return empty rows.
    // Here we verify those tables exist in the built virtual schema metadata.
    // Go 对这些表 SELECT 得空行；此处只断言元数据中存在对应表名。
    let db = build_performance_schema().expect("build performance_schema");
    assert_eq!("PERFORMANCE_SCHEMA", db.name);
    let names: Vec<String> = db
        .tables
        .iter()
        .map(|t| t.name.to_ascii_lowercase())
        .collect();
    for expected in [
        "global_status",
        "session_status",
        "setup_actors",
        "events_stages_history_long",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing performance_schema table {expected}; have {names:?}"
        );
    }
    // Each table can be wrapped as a PerfSchemaTable.
    // 每张表均可包装为 PerfSchemaTable，物理 ID 与元数据一致。
    for table in &db.tables {
        let wrapped = table_from_meta(table).expect("wrap table");
        assert_eq!(table.id, wrapped.physical_id());
        assert!(!wrapped.columns().is_empty() || table.columns.is_empty());
    }
}

#[test]
fn test_client_required_registry_is_complete_and_stable() {
    const LEGACY_TABLES: [&str; 35] = [
        "global_status",
        "session_status",
        "setup_actors",
        "setup_objects",
        "setup_instruments",
        "setup_consumers",
        "events_statements_current",
        "events_statements_history",
        "events_statements_history_long",
        "prepared_statements_instances",
        "events_transactions_current",
        "events_transactions_history",
        "events_transactions_history_long",
        "events_stages_current",
        "events_stages_history",
        "events_stages_history_long",
        "events_statements_summary_by_digest",
        "tidb_profile_cpu",
        "tidb_profile_memory",
        "tidb_profile_mutex",
        "tidb_profile_allocs",
        "tidb_profile_block",
        "tidb_profile_goroutines",
        "tikv_profile_cpu",
        "pd_profile_cpu",
        "pd_profile_memory",
        "pd_profile_mutex",
        "pd_profile_allocs",
        "pd_profile_block",
        "pd_profile_goroutines",
        "session_variables",
        "session_connect_attrs",
        "session_account_connect_attrs",
        "global_variables",
        "status_by_connection",
    ];
    const CLIENT_REQUIRED_TABLES: [(&str, &[&str]); 9] = [
        ("cond_instances", &["name", "object_instance_begin"]),
        (
            "events_waits_current",
            &[
                "thread_id",
                "event_id",
                "end_event_id",
                "event_name",
                "source",
                "timer_start",
                "timer_end",
                "timer_wait",
                "spins",
                "object_schema",
                "object_name",
                "index_name",
                "object_type",
                "object_instance_begin",
                "nesting_event_id",
                "nesting_event_type",
                "operation",
                "number_of_bytes",
                "flags",
            ],
        ),
        (
            "events_waits_history",
            &[
                "thread_id",
                "event_id",
                "end_event_id",
                "event_name",
                "source",
                "timer_start",
                "timer_end",
                "timer_wait",
                "spins",
                "object_schema",
                "object_name",
                "index_name",
                "object_type",
                "object_instance_begin",
                "nesting_event_id",
                "nesting_event_type",
                "operation",
                "number_of_bytes",
                "flags",
            ],
        ),
        (
            "events_waits_history_long",
            &[
                "thread_id",
                "event_id",
                "end_event_id",
                "event_name",
                "source",
                "timer_start",
                "timer_end",
                "timer_wait",
                "spins",
                "object_schema",
                "object_name",
                "index_name",
                "object_type",
                "object_instance_begin",
                "nesting_event_id",
                "nesting_event_type",
                "operation",
                "number_of_bytes",
                "flags",
            ],
        ),
        (
            "accounts",
            &[
                "user",
                "host",
                "current_connections",
                "total_connections",
                "max_session_controlled_memory",
                "max_session_total_memory",
            ],
        ),
        (
            "hosts",
            &[
                "host",
                "current_connections",
                "total_connections",
                "max_session_controlled_memory",
                "max_session_total_memory",
            ],
        ),
        (
            "users",
            &[
                "user",
                "current_connections",
                "total_connections",
                "max_session_controlled_memory",
                "max_session_total_memory",
            ],
        ),
        (
            "binary_log_transaction_compression_stats",
            &[
                "log_type",
                "compression_type",
                "transaction_counter",
                "compressed_bytes_counter",
                "uncompressed_bytes_counter",
                "compression_percentage",
            ],
        ),
        (
            "events_transactions_summary_by_user_by_event_name",
            &[
                "user",
                "event_name",
                "count_star",
                "sum_timer_wait",
                "min_timer_wait",
                "avg_timer_wait",
                "max_timer_wait",
                "count_read_write",
                "sum_timer_read_write",
                "min_timer_read_write",
                "avg_timer_read_write",
                "max_timer_read_write",
                "count_read_only",
                "sum_timer_read_only",
                "min_timer_read_only",
                "avg_timer_read_only",
                "max_timer_read_only",
            ],
        ),
    ];

    let db = build_performance_schema().expect("build performance_schema");
    let ids = table_id_map();
    let registered_names = db
        .tables
        .iter()
        .map(|table| table.name.as_str())
        .collect::<BTreeSet<_>>();
    let mapped_names = ids.keys().copied().collect::<BTreeSet<_>>();
    let unique_ids = ids.values().copied().collect::<BTreeSet<_>>();

    assert_eq!(
        db.tables.len(),
        LEGACY_TABLES.len() + CLIENT_REQUIRED_TABLES.len()
    );
    assert_eq!(
        registered_names, mapped_names,
        "DDL registry and ID map drifted"
    );
    assert_eq!(
        unique_ids.len(),
        ids.len(),
        "performance_schema IDs must be unique"
    );

    for (offset, name) in LEGACY_TABLES.into_iter().enumerate() {
        assert_eq!(
            ids.get(name),
            Some(&(PERFORMANCE_SCHEMA_DB_ID + offset as i64 + 1)),
            "legacy ID changed for {name}",
        );
    }

    for (offset, (name, expected_columns)) in CLIENT_REQUIRED_TABLES.into_iter().enumerate() {
        let table = db
            .tables
            .iter()
            .find(|table| table.name == name)
            .unwrap_or_else(|| panic!("missing client-required table {name}"));
        let expected_id = PERFORMANCE_SCHEMA_DB_ID + LEGACY_TABLES.len() as i64 + offset as i64 + 1;
        assert_eq!(table.id, expected_id, "unstable ID for {name}");
        assert_eq!(
            ids.get(name),
            Some(&expected_id),
            "ID map drifted for {name}"
        );
        assert_eq!(
            table
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            expected_columns,
            "column order drifted for {name}",
        );
    }
}

/// 断言 TiKV/PD profile 相关表已注册（无 HTTP 栈时只查元数据）。
#[test]
fn test_tikv_profile_cpu_table_meta() {
    // Go TestTiKVProfileCPU queries tikv_profile_cpu / pd_profile_* via HTTP.
    // Without the HTTP/failpoint stack, assert the profile tables are registered
    // with the expected names so the virtual-table surface matches Go.
    let db = build_performance_schema().expect("build performance_schema");
    let names: Vec<String> = db
        .tables
        .iter()
        .map(|t| t.name.to_ascii_lowercase())
        .collect();
    for expected in [
        "tikv_profile_cpu",
        "pd_profile_cpu",
        "pd_profile_memory",
        "pd_profile_mutex",
        "pd_profile_allocs",
        "pd_profile_block",
        "pd_profile_goroutines",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing profile table {expected}"
        );
    }
}

/// 断言会话连接属性表已注册。
#[test]
fn test_session_connect_attrs_table_meta() {
    // Go TestSessionConnectAttrs queries SESSION_CONNECT_ATTRS via TestKit.
    let db = build_performance_schema().expect("build performance_schema");
    let names: Vec<String> = db
        .tables
        .iter()
        .map(|t| t.name.to_ascii_lowercase())
        .collect();
    assert!(
        names.iter().any(|n| n == "session_connect_attrs"),
        "missing session_connect_attrs"
    );
    assert!(
        names.iter().any(|n| n == "session_account_connect_attrs"),
        "missing session_account_connect_attrs"
    );
}

/// EvalSimpleAst 就绪后 init 应注册 PERFORMANCE_SCHEMA（Once 可重复调用）。
#[test]
fn test_init_registers_when_eval_ready() {
    set_eval_simple_ast_ready(true);
    // init() is Once; calling it is safe even if already registered.
    crate::init::init();
    let registered = crate::init::registered_databases();
    assert!(
        registered
            .iter()
            .any(|db| db.name.eq_ignore_ascii_case("PERFORMANCE_SCHEMA")),
        "performance_schema should be registered after init"
    );
}

#[test]
fn test_table_accessors_match_go_virtual_table() {
    let meta = TableMeta {
        id: 1,
        database_id: 2,
        name: "example".to_string(),
        columns: vec![ColumnInfo {
            id: 1,
            name: "hidden_col".to_string(),
            offset: 0,
            hidden: true,
        }],
        indices: Vec::new(),
        public: true,
        create_sql: String::new(),
    };
    let table = table_from_meta(&meta).expect("wrap table");

    // Go's perfSchemaTable returns vt.cols from both VisibleCols and
    // FullHiddenColsAndVisibleCols, and nil from HiddenCols.
    assert_eq!(table.columns().len(), table.visible_columns().len());
    assert!(table.hidden_columns().is_empty());
}

#[test]
fn test_unique_index_name_is_preserved() {
    let table = crate::init::parse_create_table(
        "CREATE TABLE example (a INT, UNIQUE KEY `SCHEMA_NAME` (`a`));",
    )
    .expect("parse table");

    assert_eq!(table.indices.len(), 1);
    assert_eq!(table.indices[0].name, "SCHEMA_NAME");
}

struct EmptyRows;

struct ProfileRequestRows {
    observed: Mutex<Vec<String>>,
}

struct IdentityRows;

impl RowSource for IdentityRows {
    fn profile_request_identity(&self) -> ProfileRequestIdentity {
        ProfileRequestIdentity {
            connection_id: 42,
            user: Some("alice".into()),
            client_ip: Some("127.0.0.1".into()),
        }
    }
    fn local_profile(&self, _profile: &str) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(vec![vec![
            Datum::String("profile-node".into()),
            Datum::Unsigned(1),
        ]])
    }
    fn session_variables(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(vec![])
    }
    fn session_connect_attrs(&self, _account: bool) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(vec![])
    }
    fn status_by_connection(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(vec![])
    }
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn local_profile_audit_log_has_session_identity() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedWriter(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let database = build_performance_schema().unwrap();
        let table = database
            .tables
            .iter()
            .find(|table| table.name == "tidb_profile_cpu")
            .unwrap();
        let virtual_table = table_from_meta(table).unwrap();
        virtual_table
            .get_rows(
                virtual_table.columns(),
                &IdentityRows,
                &NoRemote,
                &mut vec![],
            )
            .unwrap();
    });
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(log.contains("performance_schema.tidb_profile_cpu"), "{log}");
    assert!(log.contains("conn=42"), "{log}");
    assert!(log.contains("alice"), "{log}");
    assert!(log.contains("client-ip=\"127.0.0.1\""), "{log}");
}

impl RowSource for ProfileRequestRows {
    fn profile_request_identity(&self) -> super::tables::ProfileRequestIdentity {
        super::tables::ProfileRequestIdentity::default()
    }
    fn local_profile(&self, _profile: &str) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
    fn session_variables(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
    fn session_connect_attrs(&self, _account: bool) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
    fn status_by_connection(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
    fn on_profile_request(&self, table: &str) {
        self.observed.lock().unwrap().push(table.to_owned());
    }
}

#[test]
fn every_local_profile_query_records_its_table() {
    let database = build_performance_schema().unwrap();
    let source = ProfileRequestRows {
        observed: Mutex::new(Vec::new()),
    };
    let expected = [
        "tidb_profile_cpu",
        "tidb_profile_memory",
        "tidb_profile_allocs",
        "tidb_profile_mutex",
        "tidb_profile_block",
        "tidb_profile_goroutines",
    ];
    for name in expected {
        let table = database
            .tables
            .iter()
            .find(|table| table.name == name)
            .unwrap();
        let virtual_table = table_from_meta(table).unwrap();
        virtual_table
            .get_rows(virtual_table.columns(), &source, &NoRemote, &mut Vec::new())
            .unwrap();
    }
    assert_eq!(
        *source.observed.lock().unwrap(),
        expected.map(|name| format!("performance_schema.{name}"))
    );
}

impl RowSource for EmptyRows {
    fn profile_request_identity(&self) -> super::tables::ProfileRequestIdentity {
        super::tables::ProfileRequestIdentity::default()
    }
    fn local_profile(&self, _profile: &str) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }

    fn session_variables(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(vec![vec![
            Datum::String("first-value".to_string()),
            Datum::String("second-value".to_string()),
        ]])
    }

    fn session_connect_attrs(&self, _account: bool) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }

    fn status_by_connection(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
}

struct NoRemote;

impl RemoteProfileClient for NoRemote {
    fn servers(&self, _node_type: &str) -> Result<Vec<ServerInfo>, PerfSchemaError> {
        Ok(Vec::new())
    }

    fn fetch(&self, _url: &str, _allow_follower: bool) -> Result<Vec<u8>, PerfSchemaError> {
        Ok(Vec::new())
    }

    fn parse_profile(
        &self,
        _body: &[u8],
        _goroutines: bool,
    ) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
}

struct RecordingRemote {
    urls: Mutex<Vec<String>>,
}

impl RemoteProfileClient for RecordingRemote {
    fn servers(&self, _node_type: &str) -> Result<Vec<ServerInfo>, PerfSchemaError> {
        Ok(vec![ServerInfo {
            server_type: "pd".to_string(),
            address: "127.0.0.1:2379".to_string(),
            status_address: "127.0.0.1:2379".to_string(),
        }])
    }

    fn fetch(&self, url: &str, _allow_follower: bool) -> Result<Vec<u8>, PerfSchemaError> {
        self.urls
            .lock()
            .expect("recording remote lock")
            .push(url.to_string());
        Ok(Vec::new())
    }

    fn parse_profile(
        &self,
        _body: &[u8],
        _goroutines: bool,
    ) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        Ok(Vec::new())
    }
}

#[test]
fn test_pd_profile_routes_use_pd_api_prefix() {
    let database = build_performance_schema().expect("build performance_schema");
    let remote = RecordingRemote {
        urls: Mutex::new(Vec::new()),
    };
    let source = EmptyRows;
    let mut warnings = Vec::new();

    for name in [
        "pd_profile_cpu",
        "pd_profile_memory",
        "pd_profile_mutex",
        "pd_profile_allocs",
        "pd_profile_block",
        "pd_profile_goroutines",
    ] {
        let table = database
            .tables
            .iter()
            .find(|table| table.name == name)
            .expect("profile table metadata");
        let wrapped = table_from_meta(table).expect("wrap table");
        wrapped
            .get_rows(&table.columns, &source, &remote, &mut warnings)
            .expect("fetch profile");
    }

    let urls = remote.urls.lock().expect("recording remote lock");
    assert!(
        urls.iter()
            .all(|url| url.contains("/pd/api/v1/debug/pprof/"))
    );
}

#[test]
fn test_projection_matches_go_length_guard() {
    let meta = TableMeta {
        id: 1,
        database_id: 2,
        name: "session_variables".to_string(),
        columns: vec![
            ColumnInfo {
                id: 1,
                name: "first".to_string(),
                offset: 0,
                hidden: false,
            },
            ColumnInfo {
                id: 2,
                name: "second".to_string(),
                offset: 1,
                hidden: false,
            },
        ],
        indices: Vec::new(),
        public: true,
        create_sql: String::new(),
    };
    let table = table_from_meta(&meta).expect("wrap table");
    let mut warnings = Vec::new();
    let requested = vec![meta.columns[1].clone(), meta.columns[0].clone()];

    // Go checks only len(cols) == len(vt.cols), so an equal-length request
    // receives fullRows without reordering.
    let rows = table
        .get_rows(&requested, &EmptyRows, &NoRemote, &mut warnings)
        .expect("get rows");
    assert_eq!(
        rows,
        vec![vec![
            Datum::String("first-value".to_string()),
            Datum::String("second-value".to_string()),
        ]]
    );
}

struct RemoteParity {
    requests: Mutex<Vec<(String, bool)>>,
}

impl RemoteProfileClient for RemoteParity {
    fn servers(&self, node_type: &str) -> Result<Vec<ServerInfo>, PerfSchemaError> {
        assert_eq!(node_type, "pd");
        Ok(vec![
            ServerInfo {
                server_type: "pd".to_string(),
                address: "missing:2379".to_string(),
                status_address: String::new(),
            },
            ServerInfo {
                server_type: "pd".to_string(),
                address: "second:2379".to_string(),
                status_address: "b:2379".to_string(),
            },
            ServerInfo {
                server_type: "pd".to_string(),
                address: "first:2379".to_string(),
                status_address: "a:2379".to_string(),
            },
        ])
    }

    fn fetch(&self, url: &str, allow_follower: bool) -> Result<Vec<u8>, PerfSchemaError> {
        self.requests
            .lock()
            .expect("remote parity lock")
            .push((url.to_string(), allow_follower));
        if url.contains("b:2379") {
            return Err(PerfSchemaError::Transport("second failed".to_string()));
        }
        Ok(b"profile".to_vec())
    }

    fn parse_profile(
        &self,
        body: &[u8],
        goroutines: bool,
    ) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        assert_eq!(body, b"profile");
        assert!(goroutines);
        Ok(vec![vec![Datum::String("parsed".to_string())]])
    }
}

#[test]
fn remote_profile_matches_go_warning_sorting_and_row_shape() {
    let remote = RemoteParity {
        requests: Mutex::new(Vec::new()),
    };
    let mut warnings = Vec::new();

    let rows = data_for_remote_profile(
        &remote,
        "pd",
        "/pd/api/v1/debug/pprof/goroutine?debug=2",
        true,
        &mut warnings,
    )
    .expect("remote profile");

    assert_eq!(
        rows,
        vec![vec![
            Datum::String("a:2379".to_string()),
            Datum::String("parsed".to_string()),
        ]]
    );
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].contains("missing:2379"));
    assert!(warnings[1].contains("second failed"));
    let requests = remote.requests.lock().expect("remote parity lock");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|(_, allow_follower)| *allow_follower));
}

#[test]
fn remote_profile_rejects_unknown_node_type_before_discovery() {
    let remote = RemoteParity {
        requests: Mutex::new(Vec::new()),
    };
    let error = data_for_remote_profile(&remote, "tidb", "/debug/pprof", false, &mut Vec::new())
        .expect_err("unsupported node type");
    assert_eq!(
        error,
        PerfSchemaError::UnsupportedNodeType("tidb".to_string())
    );
}

struct RejectingPlugin;

impl VirtualTablePlugin for RejectingPlugin {
    fn create(&self, _meta: &TableMeta) -> Result<crate::tables::PerfSchemaTable, PerfSchemaError> {
        Err(PerfSchemaError::Plugin("plugin selected".to_string()))
    }
}

#[test]
fn plugin_and_invalid_index_paths_match_go_factory_order() {
    let mut meta = TableMeta {
        id: 7,
        database_id: 8,
        name: "PARITY_PLUGIN_TABLE".to_string(),
        columns: Vec::new(),
        indices: vec![IndexInfo {
            id: 1,
            name: "not_public".to_string(),
            columns: Vec::new(),
            public: false,
        }],
        public: true,
        create_sql: String::new(),
    };

    register_plugin_table(&meta.name, Arc::new(RejectingPlugin));
    let plugin_error = table_from_meta(&meta).expect_err("plugin must override default factory");
    unregister_plugin_table(&meta.name);
    assert_eq!(
        plugin_error,
        PerfSchemaError::Plugin("plugin selected".to_string())
    );

    meta.name.make_ascii_lowercase();
    assert_eq!(
        table_from_meta(&meta).expect_err("invalid index state"),
        PerfSchemaError::InvalidIndexState("not_public".to_string())
    );
}

#[test]
fn local_profile_logs_all_tables_before_collecting_nonempty_rows() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedWriter(output.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let database = build_performance_schema().unwrap();
        for (name, profile) in [
            ("tidb_profile_cpu", "cpu"),
            ("tidb_profile_memory", "heap"),
            ("tidb_profile_allocs", "allocs"),
            ("tidb_profile_mutex", "mutex"),
            ("tidb_profile_block", "block"),
            ("tidb_profile_goroutines", "goroutine"),
        ] {
            let table = table_from_meta(
                database
                    .tables
                    .iter()
                    .find(|table| table.name == name)
                    .unwrap(),
            )
            .unwrap();
            let source = AuditedRows {
                output: output.clone(),
                table: name,
                profile,
                fail: false,
            };
            let rows = table
                .get_rows(table.columns(), &source, &NoRemote, &mut vec![])
                .unwrap();
            assert_eq!(
                rows,
                vec![vec![
                    Datum::String("profile-node".into()),
                    Datum::Unsigned(1)
                ]]
            );
        }
        let table = table_from_meta(
            database
                .tables
                .iter()
                .find(|table| table.name == "tidb_profile_cpu")
                .unwrap(),
        )
        .unwrap();
        let source = AuditedRows {
            output: output.clone(),
            table: "tidb_profile_cpu",
            profile: "cpu",
            fail: true,
        };
        assert_eq!(
            table.get_rows(table.columns(), &source, &NoRemote, &mut vec![]),
            Err(PerfSchemaError::Profile("collector failed".into()))
        );
    });
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert_eq!(
        log.matches("profiling request received").count(),
        7,
        "{log}"
    );
    assert_eq!(log.matches("conn=0").count(), 7, "{log}");
    assert!(!log.contains("user="), "{log}");
    assert!(!log.contains("client-ip="), "{log}");
}

struct AuditedRows {
    output: Arc<Mutex<Vec<u8>>>,
    table: &'static str,
    profile: &'static str,
    fail: bool,
}
impl RowSource for AuditedRows {
    fn profile_request_identity(&self) -> ProfileRequestIdentity {
        ProfileRequestIdentity::default()
    }
    fn local_profile(&self, profile: &str) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        assert_eq!(profile, self.profile);
        let log = String::from_utf8(self.output.lock().unwrap().clone()).unwrap();
        assert!(
            log.contains(&format!("performance_schema.{}", self.table)),
            "log must precede collection: {log}"
        );
        if self.fail {
            Err(PerfSchemaError::Profile("collector failed".into()))
        } else {
            Ok(vec![vec![
                Datum::String("profile-node".into()),
                Datum::Unsigned(1),
            ]])
        }
    }
    fn session_variables(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        unreachable!()
    }
    fn session_connect_attrs(&self, _: bool) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        unreachable!()
    }
    fn status_by_connection(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        unreachable!()
    }
}
