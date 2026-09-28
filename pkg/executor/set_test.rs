// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `SET` / `SET CONFIG` 执行器单元测试。
//
// 覆盖用户变量、字符集/校对、系统变量校验、实例作用域、noop 警告、
// 集群配置 HTTP 下发、Top SQL 与 service scope 等路径。

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::set::{
    Collation, SetBackend, SetDatum, SetExecutor, SetResultChunk, SetVariableNames, SystemVariable,
    SystemVariableLookup, VarAssignment, loadSnapshotInfoSchemaIfNeeded,
};
use crate::set_config::{
    ConfigHttpResponse, ConfigServerInfo, ConfigValue, SetConfigBackend, SetConfigExec,
    SetConfigPlan,
};

/// 测试用错误：仅包装消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestError(String);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 记录 `reset` 调用次数的结果 Chunk 替身。
#[derive(Default)]
struct TestChunk {
    resets: usize,
}

impl SetResultChunk for TestChunk {
    fn reset(&mut self) {
        self.resets += 1;
    }
}

/// `SetBackend` 内存替身：用户/系统/会话/全局变量与权限、快照等。
struct Backend {
    user: HashMap<String, SetDatum>,
    user_types: HashMap<String, String>,
    system: HashMap<String, SystemVariable>,
    session: HashMap<String, String>,
    global: HashMap<String, String>,
    instance: HashMap<String, String>,
    privileges: HashSet<String>,
    warnings: Vec<String>,
    audit: Vec<(String, String)>,
    global_log: Vec<(bool, String, String)>,
    session_log: Vec<(String, String)>,
    snapshot_log: Vec<u64>,
    snapshot_schema: Option<u64>,
    snapshot_ts: u64,
    txn_read_ts: u64,
    in_txn: bool,
    stale_txn: bool,
    reject_snapshot: bool,
    sem: bool,
    sem_v2: bool,
    noop_enabled: bool,
    legacy_instance: bool,
    service_scope: String,
    task_manager_scopes: Vec<String>,
}

/// `SetConfigBackend` 替身：模拟集群节点与 HTTP POST 配置接口。
#[derive(Default)]
struct ConfigBackend {
    servers: Vec<ConfigServerInfo>,
    requests: Vec<(String, String)>,
    warnings: Vec<TestError>,
    status_code: u16,
}

impl SetConfigBackend for ConfigBackend {
    type Error = TestError;

    fn cluster_servers(&mut self) -> Result<Vec<ConfigServerInfo>, Self::Error> {
        Ok(self.servers.clone())
    }

    fn internal_http_scheme(&self) -> &str {
        "http"
    }

    fn post_json(&mut self, url: &str, body: &str) -> Result<ConfigHttpResponse, Self::Error> {
        self.requests.push((url.to_owned(), body.to_owned()));
        let status_code = if self.status_code == 0 {
            200
        } else {
            self.status_code
        };
        Ok(ConfigHttpResponse {
            status_code,
            status: status_code.to_string(),
            body: b"rejected".to_vec(),
        })
    }

    fn append_warning(&mut self, error: Self::Error) {
        self.warnings.push(error);
    }

    fn error(&self, message: String) -> Self::Error {
        TestError(message)
    }
}

impl Backend {
    /// 预置常用系统变量与全局字符集默认值。
    fn new() -> Self {
        let mut system = HashMap::new();
        for (name, default_value) in [
            ("autocommit", "ON"),
            ("concurrency", "4"),
            ("noop", "OFF"),
            ("cloud_storage_uri", ""),
            ("tidb_service_scope", ""),
            ("tidb_snapshot", "0"),
            ("tx_read_ts", "0"),
            ("transaction_isolation_one_shot", ""),
            ("restricted", "OFF"),
            ("tidb_enable_top_sql", "OFF"),
            ("div_precision_increment", "4"),
        ] {
            system.insert(
                name.to_owned(),
                SystemVariable {
                    name: name.to_owned(),
                    default_value: default_value.to_owned(),
                    is_noop: name == "noop",
                    has_instance_scope: name == "concurrency",
                },
            );
        }
        let global = HashMap::from([
            ("character_set_database".to_owned(), "utf8mb4".to_owned()),
            ("collation_database".to_owned(), "utf8mb4_bin".to_owned()),
            ("autocommit".to_owned(), "ON".to_owned()),
        ]);
        Self {
            user: HashMap::new(),
            user_types: HashMap::new(),
            system,
            session: HashMap::new(),
            global,
            instance: HashMap::new(),
            privileges: HashSet::new(),
            warnings: Vec::new(),
            audit: Vec::new(),
            global_log: Vec::new(),
            session_log: Vec::new(),
            snapshot_log: Vec::new(),
            snapshot_schema: None,
            snapshot_ts: 0,
            txn_read_ts: 0,
            in_txn: false,
            stale_txn: false,
            reject_snapshot: false,
            sem: false,
            sem_v2: false,
            noop_enabled: false,
            legacy_instance: false,
            service_scope: String::new(),
            task_manager_scopes: Vec::new(),
        }
    }
}

impl SetBackend for Backend {
    type Context = ();
    type Error = TestError;
    type Expression = SetDatum;
    type FieldType = String;
    type SnapshotInfoSchema = u64;

    fn variable_names(&self) -> SetVariableNames {
        SetVariableNames {
            set_names: "set names".to_owned(),
            set_charset: "set charset".to_owned(),
            default_charset: "utf8mb4".to_owned(),
            utf8mb4_charset: "utf8mb4".to_owned(),
            set_names_variables: vec![
                "character_set_client".to_owned(),
                "character_set_results".to_owned(),
                "character_set_connection".to_owned(),
            ],
            set_charset_variables: vec![
                "character_set_client".to_owned(),
                "character_set_results".to_owned(),
            ],
            collation_connection: "collation_connection".to_owned(),
            character_set_connection: "character_set_connection".to_owned(),
            charset_database: "character_set_database".to_owned(),
            collation_database: "collation_database".to_owned(),
            cloud_storage_uri: "cloud_storage_uri".to_owned(),
            service_scope: "tidb_service_scope".to_owned(),
            snapshot: "tidb_snapshot".to_owned(),
            txn_read_ts: "tx_read_ts".to_owned(),
            txn_isolation_one_shot: "transaction_isolation_one_shot".to_owned(),
        }
    }

    fn evaluate(&self, expression: &Self::Expression) -> Result<SetDatum, Self::Error> {
        Ok(expression.clone())
    }

    fn expression_type(&self, expression: &Self::Expression) -> Self::FieldType {
        match expression {
            SetDatum::Null => "null",
            SetDatum::String(_) => "string",
        }
        .to_owned()
    }

    fn datum_to_string(&self, datum: &SetDatum) -> Result<String, Self::Error> {
        match datum {
            SetDatum::Null => Ok(String::new()),
            SetDatum::String(value) => Ok(value.clone()),
        }
    }

    fn unset_user_variable(&mut self, name: &str) {
        self.user.remove(name);
        self.user_types.remove(name);
    }

    fn set_user_variable(&mut self, name: &str, value: SetDatum) {
        self.user.insert(name.to_owned(), value);
    }

    fn set_user_variable_type(&mut self, name: &str, field_type: Self::FieldType) {
        self.user_types.insert(name.to_owned(), field_type);
    }

    fn system_variable(&self, name: &str) -> SystemVariableLookup {
        if name == "removed" {
            SystemVariableLookup::Removed
        } else {
            self.system
                .get(name)
                .cloned()
                .map(SystemVariableLookup::Found)
                .unwrap_or(SystemVariableLookup::Unknown)
        }
    }

    fn unknown_system_variable(&self, name: &str) -> Self::Error {
        TestError(format!("unknown system variable {name}"))
    }

    fn required_dynamic_privileges(
        &self,
        _variable: &SystemVariable,
        global: bool,
        _sem_enabled: bool,
    ) -> Vec<String> {
        global
            .then(|| "SYSTEM_VARIABLES_ADMIN".to_owned())
            .into_iter()
            .collect()
    }

    fn sem_enabled(&self) -> bool {
        self.sem
    }

    fn sem_v2_enabled(&self) -> bool {
        self.sem_v2
    }

    fn sem_v2_read_only_variable(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case("restricted")
    }

    fn verify_dynamic_privilege(&self, privilege: &str) -> bool {
        self.privileges.contains(privilege)
    }

    fn access_denied(&self, privilege: &str) -> Self::Error {
        TestError(format!("access denied; need {privilege}"))
    }

    fn noop_variables_enabled(&self) -> bool {
        self.noop_enabled
    }

    fn legacy_instance_scope_enabled(&self) -> bool {
        self.legacy_instance
    }

    fn append_noop_warning(&mut self, variable: &str) {
        self.warnings.push(format!("noop:{variable}"));
    }

    fn append_instance_scope_warning(&mut self, variable: &str) {
        self.warnings.push(format!("instance:{variable}"));
    }

    fn set_global_system_variable(
        &mut self,
        _context: &Self::Context,
        name: &str,
        value: &str,
    ) -> Result<(), Self::Error> {
        self.global.insert(name.to_owned(), value.to_owned());
        if name == "tidb_service_scope" {
            self.service_scope = value.to_owned();
        }
        Ok(())
    }

    fn set_instance_system_variable(
        &mut self,
        _context: &Self::Context,
        name: &str,
        value: &str,
    ) -> Result<(), Self::Error> {
        self.instance.insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn audit_global_variable_event(&mut self, name: &str, value: &str) -> Result<(), Self::Error> {
        self.audit.push((name.to_owned(), value.to_owned()));
        Ok(())
    }

    fn redact_url(&self, value: &str) -> String {
        value
            .split_once('@')
            .map(|(_, tail)| format!("***@{tail}"))
            .unwrap_or_else(|| value.to_owned())
    }

    fn log_global_variable(&self, _instance: bool, _name: &str, _value: &str) {}

    fn current_service_scope(&self) -> String {
        self.service_scope.clone()
    }

    fn initialize_task_manager_for_service_scope(
        &mut self,
        _context: &Self::Context,
        service_scope: &str,
    ) -> Result<(), Self::Error> {
        self.task_manager_scopes.push(service_scope.to_owned());
        Ok(())
    }

    fn global_system_variable_initial_value(&self, name: &str, default: &str) -> String {
        self.global
            .get(name)
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    }

    fn get_global_system_variable(
        &self,
        _context: &Self::Context,
        name: &str,
    ) -> Result<String, Self::Error> {
        self.get_global_system_variable_unscoped(name)
    }

    fn get_global_system_variable_unscoped(&self, name: &str) -> Result<String, Self::Error> {
        self.global
            .get(name)
            .cloned()
            .ok_or_else(|| TestError(format!("missing global variable {name}")))
    }

    fn set_session_system_variable(&mut self, name: &str, value: &str) -> Result<(), Self::Error> {
        self.session.insert(name.to_owned(), value.to_owned());
        if name == "tidb_snapshot" {
            self.snapshot_ts = value.parse().unwrap_or_default();
        } else if name == "tx_read_ts" {
            self.txn_read_ts = value.parse().unwrap_or_default();
        }
        Ok(())
    }

    fn in_transaction(&self) -> bool {
        self.in_txn
    }

    fn transaction_is_staleness(&self) -> bool {
        self.stale_txn
    }

    fn cannot_change_transaction_characteristics(&self) -> Self::Error {
        TestError("cannot change transaction characteristics".to_owned())
    }

    fn snapshot_ts(&self) -> u64 {
        self.snapshot_ts
    }

    fn txn_read_ts(&self) -> u64 {
        self.txn_read_ts
    }

    fn set_snapshot_ts(&mut self, timestamp: u64) {
        self.snapshot_ts = timestamp;
    }

    fn set_txn_read_ts(&mut self, timestamp: u64) {
        self.txn_read_ts = timestamp;
    }

    fn validate_snapshot_read_ts(
        &self,
        _context: &Self::Context,
        _timestamp: u64,
        _stale_read: bool,
        _validate_for_tidb_snapshot: bool,
    ) -> Result<(), Self::Error> {
        if self.reject_snapshot {
            Err(TestError("snapshot rejected".to_owned()))
        } else {
            Ok(())
        }
    }

    fn validate_gc_snapshot(&self, _timestamp: u64) -> Result<(), Self::Error> {
        Ok(())
    }

    fn snapshot_info_schema(
        &self,
        timestamp: u64,
    ) -> Result<Self::SnapshotInfoSchema, Self::Error> {
        Ok(timestamp)
    }

    fn attach_local_temporary_tables(
        &self,
        info_schema: Self::SnapshotInfoSchema,
    ) -> Self::SnapshotInfoSchema {
        info_schema + 1
    }

    fn set_snapshot_info_schema(&mut self, info_schema: Option<Self::SnapshotInfoSchema>) {
        self.snapshot_schema = info_schema;
    }

    fn log_snapshot_info_schema(&self, _timestamp: u64) {}

    fn log_session_variable(&self, _name: &str, _value: &str) {}

    fn default_collation_for_utf8mb4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }

    fn default_collation(&self, charset: &str) -> Result<String, Self::Error> {
        match charset {
            "utf8" => Ok("utf8_bin".to_owned()),
            "latin1" => Ok("latin1_bin".to_owned()),
            "utf8mb4" => Ok("utf8mb4_bin".to_owned()),
            _ => Err(TestError(format!("unknown charset {charset}"))),
        }
    }

    fn collation(&self, name: &str) -> Result<Collation, Self::Error> {
        let charset_name = name
            .strip_suffix("_bin")
            .ok_or_else(|| TestError(format!("unknown collation {name}")))?;
        Ok(Collation {
            name: name.to_owned(),
            charset_name: charset_name.to_owned(),
        })
    }

    fn collation_charset_mismatch(&self, collation: &str, charset: &str) -> Self::Error {
        TestError(format!("collation {collation} is not valid for {charset}"))
    }
}

/// 构造简单变量赋值（非 default / 非 global / 非 instance）。
fn assignment(name: &str, value: SetDatum, system: bool) -> VarAssignment<SetDatum> {
    VarAssignment {
        name: name.to_owned(),
        expression: value,
        extend_value: None,
        is_default: false,
        is_system: system,
        is_global: false,
        is_instance: false,
    }
}

/// 运行 `SetExecutor::Next` 两次并断言 Chunk 被 reset 两次。
fn execute(backend: Backend, vars: Vec<VarAssignment<SetDatum>>) -> Result<Backend, TestError> {
    let mut executor = SetExecutor {
        BaseExecutor: backend,
        vars,
        done: false,
    };
    let mut chunk = TestChunk::default();
    // 第二次 Next 应因 done 直接返回空块，但仍会 reset。
    executor.Next(&(), &mut chunk)?;
    executor.Next(&(), &mut chunk)?;
    assert_eq!(chunk.resets, 2);
    Ok(executor.BaseExecutor)
}

/// 用户变量大小写折叠、类型记录，以及 NULL 触发 unset。
#[test]
fn test_set_var() {
    let backend = execute(
        Backend::new(),
        vec![
            assignment("MiXeD", SetDatum::String("value".to_owned()), false),
            assignment("autocommit", SetDatum::String("OFF".to_owned()), true),
        ],
    )
    .unwrap();
    assert_eq!(
        backend.user.get("mixed"),
        Some(&SetDatum::String("value".to_owned()))
    );
    assert_eq!(
        backend.user_types.get("mixed").map(String::as_str),
        Some("string")
    );
    assert_eq!(
        backend.session.get("autocommit").map(String::as_str),
        Some("OFF")
    );

    let mut backend = Backend::new();
    backend
        .user
        .insert("mixed".to_owned(), SetDatum::String("old".to_owned()));
    let backend = execute(backend, vec![assignment("MIXED", SetDatum::Null, false)]).unwrap();
    assert!(!backend.user.contains_key("mixed"));
}

/// `SET NAMES` 设置字符集/校对，以及校对与字符集不匹配时报错。
#[test]
fn test_set_collation_and_charset() {
    let mut names = assignment("set names", SetDatum::String("utf8mb4".to_owned()), true);
    names.extend_value = Some(SetDatum::String("utf8mb4_bin".to_owned()));
    let backend = execute(Backend::new(), vec![names]).unwrap();
    assert_eq!(
        backend
            .session
            .get("character_set_client")
            .map(String::as_str),
        Some("utf8mb4")
    );
    assert_eq!(
        backend
            .session
            .get("collation_connection")
            .map(String::as_str),
        Some("utf8mb4_bin")
    );

    let mut invalid = assignment("set names", SetDatum::String("utf8".to_owned()), true);
    invalid.extend_value = Some(SetDatum::String("latin1_bin".to_owned()));
    assert!(execute(Backend::new(), vec![invalid]).is_err());
}

/// 未知系统变量、事务中改 read ts、快照校验失败时回滚会话快照。
#[test]
fn test_validate_set_var() {
    assert!(
        execute(
            Backend::new(),
            vec![assignment("unknown", SetDatum::Null, true)]
        )
        .is_err()
    );
    let mut backend = Backend::new();
    backend.in_txn = true;
    assert!(
        execute(
            backend,
            vec![assignment(
                "tx_read_ts",
                SetDatum::String("42".to_owned()),
                true
            )]
        )
        .is_err()
    );

    let mut backend = Backend::new();
    backend.snapshot_ts = 7;
    backend.reject_snapshot = true;
    let mut executor = SetExecutor {
        BaseExecutor: backend,
        vars: vec![assignment(
            "tidb_snapshot",
            SetDatum::String("42".to_owned()),
            true,
        )],
        done: false,
    };
    // 校验失败后会话快照时间戳应保持原值。
    assert!(executor.Next(&(), &mut TestChunk::default()).is_err());
    assert_eq!(executor.BaseExecutor.snapshot_ts, 7);
}

/// 旧版 instance 作用域：写入 instance 并产生 instance 警告。
#[test]
fn test_set_concurrency() {
    let mut backend = Backend::new();
    backend.legacy_instance = true;
    backend
        .privileges
        .insert("SYSTEM_VARIABLES_ADMIN".to_owned());
    let backend = execute(
        backend,
        vec![assignment(
            "concurrency",
            SetDatum::String("16".to_owned()),
            true,
        )],
    )
    .unwrap();
    assert_eq!(
        backend.instance.get("concurrency").map(String::as_str),
        Some("16")
    );
    assert_eq!(backend.warnings, ["instance:concurrency"]);
}

/// noop 变量：未启用时告警，启用后静默接受。
#[test]
fn test_enable_noop_functions_var() {
    let backend = execute(
        Backend::new(),
        vec![assignment("noop", SetDatum::String("ON".to_owned()), true)],
    )
    .unwrap();
    assert_eq!(backend.warnings, ["noop:noop"]);

    let mut enabled = Backend::new();
    enabled.noop_enabled = true;
    let enabled = execute(
        enabled,
        vec![assignment("noop", SetDatum::String("ON".to_owned()), true)],
    )
    .unwrap();
    assert!(enabled.warnings.is_empty());
}

/// `SET CONFIG`：向 TiKV status 地址 POST JSON，并校验非法节点/实例。
#[test]
fn test_set_cluster_config() {
    let backend = ConfigBackend {
        servers: vec![
            ConfigServerInfo {
                server_type: "tikv".to_owned(),
                status_address: "127.0.0.1:20180".to_owned(),
            },
            ConfigServerInfo {
                server_type: "pd".to_owned(),
                status_address: "127.0.0.1:2379".to_owned(),
            },
        ],
        ..ConfigBackend::default()
    };
    let mut executor = SetConfigExec {
        backend,
        plan: SetConfigPlan {
            node_type: "TiKV".to_owned(),
            instance: "127.0.0.1:20180".to_owned(),
            name: "RaftStore.Region-Max-Size".to_owned(),
            value: ConfigValue::String("128MiB".to_owned()),
        },
        json_body: String::new(),
    };
    executor.Open(()).unwrap();
    executor
        .Next((), &mut astersql_util_chunk::Chunk::default())
        .unwrap();
    assert_eq!(
        executor.backend.requests,
        [(
            "http://127.0.0.1:20180/config".to_owned(),
            r#"{"raftstore.region-max-size":"128MiB"}"#.to_owned()
        )]
    );

    executor.plan.node_type = "tidb".to_owned();
    assert!(executor.Open(()).is_err());

    executor.plan.node_type = "tikv".to_owned();
    executor.plan.instance = "missing-port".to_owned();
    assert!(executor.Open(()).is_err());
}

/// 配置项转 JSON 的类型编码，以及 PD 接口非 200 时追加 warning。
#[test]
fn test_set_cluster_config_json_data() {
    assert_eq!(
        crate::set_config::ConvertConfigItem2JSON("x", &ConfigValue::Null),
        Err("cannot set config to null".to_owned())
    );
    assert_eq!(
        crate::set_config::ConvertConfigItem2JSON("x", &ConfigValue::Boolean(true)).unwrap(),
        r#"{"x":true}"#
    );
    assert_eq!(
        crate::set_config::ConvertConfigItem2JSON("x", &ConfigValue::Int(42)).unwrap(),
        r#"{"x":42}"#
    );
    assert_eq!(
        crate::set_config::ConvertConfigItem2JSON("x", &ConfigValue::Real(1.5)).unwrap(),
        r#"{"x":1.5}"#
    );
    assert_eq!(
        crate::set_config::ConvertConfigItem2JSON("x", &ConfigValue::Decimal("12.50".to_owned()))
            .unwrap(),
        r#"{"x":12.50}"#
    );

    let mut executor = SetConfigExec {
        backend: ConfigBackend {
            servers: vec![ConfigServerInfo {
                server_type: "pd".to_owned(),
                status_address: "127.0.0.1:2379".to_owned(),
            }],
            status_code: 500,
            ..ConfigBackend::default()
        },
        plan: SetConfigPlan {
            node_type: "pd".to_owned(),
            instance: String::new(),
            name: "schedule.max-merge-region-size".to_owned(),
            value: ConfigValue::Int(20),
        },
        json_body: String::new(),
    };
    executor.Open(()).unwrap();
    executor
        .Next((), &mut astersql_util_chunk::Chunk::default())
        .unwrap();
    assert_eq!(
        executor.backend.requests[0].0,
        "http://127.0.0.1:2379/pd/api/v1/config"
    );
    assert_eq!(executor.backend.warnings.len(), 1);
}

/// 全局设置 `tidb_enable_top_sql` 需要动态权限。
#[test]
fn test_set_top_sql_variables() {
    let mut backend = Backend::new();
    backend
        .privileges
        .insert("SYSTEM_VARIABLES_ADMIN".to_owned());
    let mut global = assignment(
        "tidb_enable_top_sql",
        SetDatum::String("ON".to_owned()),
        true,
    );
    global.is_global = true;
    let backend = execute(backend, vec![global]).unwrap();
    assert_eq!(
        backend
            .global
            .get("tidb_enable_top_sql")
            .map(String::as_str),
        Some("ON")
    );
}

/// 会话变量 `div_precision_increment`，以及快照 InfoSchema 附着/清空。
#[test]
fn test_div_precision_increment() {
    let backend = execute(
        Backend::new(),
        vec![assignment(
            "div_precision_increment",
            SetDatum::String("12".to_owned()),
            true,
        )],
    )
    .unwrap();
    assert_eq!(
        backend
            .session
            .get("div_precision_increment")
            .map(String::as_str),
        Some("12")
    );

    let mut snapshot_backend = Backend::new();
    // attach_local_temporary_tables 会把 schema 标记 +1。
    loadSnapshotInfoSchemaIfNeeded(&mut snapshot_backend, 55).unwrap();
    assert_eq!(snapshot_backend.snapshot_schema, Some(56));
    loadSnapshotInfoSchemaIfNeeded(&mut snapshot_backend, 0).unwrap();
    assert_eq!(snapshot_backend.snapshot_schema, None);
}

/// `tidb_service_scope` 名大小写不敏感，并触发 task manager 初始化。
#[test]
fn test_set_tidb_service_scope_case_insensitive() {
    let mut backend = Backend::new();
    backend
        .privileges
        .insert("SYSTEM_VARIABLES_ADMIN".to_owned());
    let mut variable = assignment(
        "TiDB_Service_Scope",
        SetDatum::String("Analytics".to_owned()),
        true,
    );
    variable.is_global = true;
    let backend = execute(backend, vec![variable]).unwrap();
    assert_eq!(
        backend.global.get("tidb_service_scope").map(String::as_str),
        Some("Analytics")
    );
    assert_eq!(backend.task_manager_scopes, ["Analytics"]);
}
