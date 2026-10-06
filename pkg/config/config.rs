// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// TiDB server configuration primitives migrated from `config.go`.
//
// The package integration task wires TiKV-, logging-, and tracing-specific
// adapters. This file owns the configuration data, defaults, validation,
// loading, global snapshots, and serialization behavior.
//
// 本模块由 TiDB 的 `config.go` 机械迁移而来，是服务器配置的核心。
// 它负责：
// - 定义各配置分区的数据结构（日志、安全、性能、实例、TiKV 客户端等）；
// - 提供默认值（`Default` 实现）与取值范围校验（`Config::valid`）；
// - 从 TOML 配置文件加载配置（`Config::load`）；
// - 维护进程级全局配置快照（`get_global_config` / `store_global_config`）；
// - 处理配置的 JSON 序列化输出（隐藏已移除/隐藏项）。

pub use astersql_resourcegroup::ruv2::model::StmtWeights;
use astersql_resourcegroup::ruv2::model::default_weights;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool as StdAtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// 单个日志文件的最大尺寸上限（MB）。
pub const MAX_LOG_FILE_SIZE: i64 = 4096;
/// 事务（transaction）中单条键值记录（entry）大小的最大上限：120MB。
pub const MAX_TXN_ENTRY_SIZE_LIMIT: u64 = 120 * 1024 * 1024;
/// 插件审计日志缓冲区大小上限：100MB。
pub const MAX_PLUGIN_AUDIT_LOG_BUFFER_SIZE: i64 = 100 * 1024 * 1024;
/// 插件审计日志刷盘间隔上限：3600 秒。
pub const MAX_PLUGIN_AUDIT_LOG_FLUSH_INTERVAL: i64 = 3600;
/// 事务单条记录大小的默认上限：6MB。
pub const DEF_TXN_ENTRY_SIZE_LIMIT: u64 = 6 * 1024 * 1024;
/// 事务总大小的默认上限：100MB（一次事务写入的全部数据量）。
pub const DEF_TXN_TOTAL_SIZE_LIMIT: u64 = 100 * 1024 * 1024;
/// “超大事务”阈值：100TB，实际上表示不限制。
pub const SUPER_LARGE_TXN_SIZE: u64 = 100 * 1024 * 1024 * 1024 * 1024;
/// 索引键长度默认上限（字节），与 MySQL 的 3072 限制一致。
pub const DEF_MAX_INDEX_LENGTH: i64 = 3072;
/// 索引键长度可配置的最大上限。
pub const DEF_MAX_OF_MAX_INDEX_LENGTH: i64 = 3072 * 4;
/// 单表索引数量默认上限。
pub const DEF_INDEX_LIMIT: i64 = 64;
/// 单表索引数量可配置的最大上限。
pub const DEF_MAX_OF_INDEX_LIMIT: i64 = 64 * 8;
/// SQL 服务默认监听端口（MySQL 协议）。
pub const DEF_PORT: u64 = 4000;
/// 状态服务（HTTP status API）默认监听端口。
pub const DEF_STATUS_PORT: u64 = 10080;
/// SQL 服务默认监听地址。
pub const DEF_HOST: &str = "0.0.0.0";
/// 状态服务默认监听地址。
pub const DEF_STATUS_HOST: &str = "0.0.0.0";
/// 单表列数默认上限。
pub const DEF_TABLE_COLUMN_COUNT_LIMIT: u32 = 1017;
/// 单表列数可配置的最大上限。
pub const DEF_MAX_OF_TABLE_COLUMN_COUNT_LIMIT: u32 = 4096;
/// 统计信息（statistics）加载并发度的默认下限（0 表示自动）。
pub const DEF_STATS_LOAD_CONCURRENCY_LIMIT: i64 = 0;
/// 统计信息加载并发度的最大上限。
pub const DEF_MAX_OF_STATS_LOAD_CONCURRENCY_LIMIT: i64 = 128;
/// 统计信息加载队列长度的最小值。
pub const DEF_STATS_LOAD_QUEUE_SIZE_LIMIT: i64 = 1;
/// 统计信息加载队列长度的最大值。
pub const DEF_MAX_OF_STATS_LOAD_QUEUE_SIZE_LIMIT: i64 = 100_000;
/// DXF（分布式执行框架）资源占用百分比的默认值。
pub const DEF_DXF_RESOURCE_LIMIT: i64 = 100;
pub const DEF_STARTER_MAX_IMPORT_DATA_SIZE: u64 = 25 * 1024 * 1024 * 1024;
pub const RU_REPORT_MODE_RESULT: &str = "result";
pub const RU_REPORT_MODE_FULL: &str = "full";
/// DXF 资源占用百分比允许的最小值。
pub const MIN_DXF_RESOURCE_LIMIT: i64 = 10;
/// DXF 资源占用百分比允许的最大值。
pub const MAX_DXF_RESOURCE_LIMIT: i64 = 100;
/// 默认临时目录。
pub const DEF_TEMP_DIR: &str = "/tmp/tidb";
/// 并发连接令牌（token）数量上限，用于限制同时执行的会话数。
pub const MAX_TOKEN_LIMIT: u64 = 1024 * 1024;
/// `max_allowed_packet` 的粒度单位（必须是 1024 的整数倍）。
pub const MAX_ALLOWED_PACKET_UNIT: u64 = 1024;
/// `max_allowed_packet`（单条 MySQL 协议报文最大长度）的最小值。
pub const MIN_MAX_ALLOWED_PACKET: u64 = MAX_ALLOWED_PACKET_UNIT;
/// `max_allowed_packet` 的最大值：1GB。
pub const MAX_OF_MAX_ALLOWED_PACKET: u64 = 1 << 30;
/// `max_allowed_packet` 的默认值：64MB。
pub const DEF_MAX_ALLOWED_PACKET: u64 = 64 << 20;
/// 集群内部通信 TLS 的 CA 证书环境变量名。
pub const ENV_CLUSTER_CA: &str = "CLUSTER_CA";
/// 集群内部通信 TLS 的证书环境变量名。
pub const ENV_CLUSTER_CERT: &str = "CLUSTER_CERT";
/// 集群内部通信 TLS 的私钥环境变量名。
pub const ENV_CLUSTER_KEY: &str = "CLUSTER_KEY";
/// SQL 客户端连接 TLS 的 CA 证书环境变量名。
pub const ENV_SQL_CA: &str = "SQL_CA";
/// SQL 客户端连接 TLS 的证书环境变量名。
pub const ENV_SQL_CERT: &str = "SQL_CERT";
/// SQL 客户端连接 TLS 的私钥环境变量名。
pub const ENV_SQL_KEY: &str = "SQL_KEY";
/// 落盘（spill，中间结果写入磁盘）文件不加密的方法名。
pub const SPILLED_FILE_ENCRYPTION_METHOD_PLAINTEXT: &str = "plaintext";
/// 落盘文件使用 AES128-CTR 加密的方法名。
pub const SPILLED_FILE_ENCRYPTION_METHOD_AES128_CTR: &str = "aes128-ctr";

/// Statement and DDL RU v2 configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RUV2Config {
    pub report_mode: String,
    pub stmt_weights: StmtWeights,
    pub ddl_weights: DDLWeights,
}

impl Default for RUV2Config {
    fn default() -> Self {
        Self {
            report_mode: RU_REPORT_MODE_RESULT.into(),
            stmt_weights: default_weights(),
            ddl_weights: DDLWeights::default(),
        }
    }
}

/// TiKV client's legacy RU weights are independent of the server statement model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TiKVRUV2Config {
    pub ru_scale: f64,
    pub result_chunk_cells: f64,
    pub executor_l1: f64,
    pub executor_l2: f64,
    pub executor_l3: f64,
    pub executor_l5_insert_rows: f64,
    pub plan_cnt: f64,
    pub plan_derive_stats_paths: f64,
    pub resource_manager_read_cnt: f64,
    pub resource_manager_write_cnt: f64,
    pub write_keys: f64,
    pub session_parser_total: f64,
    pub txn_cnt: f64,
}

impl Default for TiKVRUV2Config {
    fn default() -> Self {
        Self {
            ru_scale: 2.01,
            result_chunk_cells: 0.0001,
            executor_l1: 0.00013278,
            executor_l2: 0.00000383,
            executor_l3: 0.00141739,
            executor_l5_insert_rows: 0.00472572,
            plan_cnt: 0.15392217,
            plan_derive_stats_paths: 0.24968182,
            resource_manager_read_cnt: 0.02072003,
            resource_manager_write_cnt: 0.07179779,
            write_keys: 0.330760861554226,
            session_parser_total: 0.19230499,
            txn_cnt: 0.03013709,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DDLWeights {
    pub txn_kv_bytes: f64,
    pub ingest_kv_bytes: f64,
}

impl Default for DDLWeights {
    fn default() -> Self {
        Self {
            txn_kv_bytes: 1.0,
            ingest_kv_bytes: 1.0,
        }
    }
}

fn valid_ru_weight(section: &str, name: &str, value: f64) -> Result<(), ConfigError> {
    if !value.is_finite() || value < 0.0 {
        return Err(message(format!(
            "ru-v2.{section}.{name} must be finite and non-negative, got {value}"
        )));
    }
    Ok(())
}

impl DDLWeights {
    fn validate(&self) -> Result<(), ConfigError> {
        valid_ru_weight("ddl-weights", "txn-kv-bytes", self.txn_kv_bytes)?;
        valid_ru_weight("ddl-weights", "ingest-kv-bytes", self.ingest_kv_bytes)
    }
}

/// 传递给 TiKV 客户端的配置包装（TiKV 是 TiDB 的分布式键值存储引擎）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ClientConfig {
    pub tikv_client: TiKVClient,
}

/// 描述某个配置分区中已被迁移到 `[instance]` 分区的配置项映射。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceConfigSection {
    /// 原配置分区名，空字符串代表顶层分区。
    pub section_name: String,
    /// 旧配置项名 -> 新系统变量名的映射表。
    pub name_mappings: HashMap<String, String>,
}

/// 返回已从各配置分区迁移到 `[instance]` 分区（对应系统变量）的配置项清单。
/// 用于在加载旧配置文件时给出兼容提示。
pub fn section_moved_to_instance() -> Vec<InstanceConfigSection> {
    [
        (
            "",
            &[
                ("check-mb4-value-in-utf8", "tidb_check_mb4_value_in_utf8"),
                (
                    "enable-collect-execution-info",
                    "tidb_enable_collect_execution_info",
                ),
                ("max-server-connections", "max_connections"),
                ("run-ddl", "tidb_enable_ddl"),
            ][..],
        ),
        (
            "log",
            &[
                ("enable-slow-log", "tidb_enable_slow_log"),
                ("slow-threshold", "tidb_slow_log_threshold"),
                ("record-plan-in-slow-log", "tidb_record_plan_in_slow_log"),
            ][..],
        ),
        (
            "performance",
            &[
                ("force-priority", "tidb_force_priority"),
                ("memory-usage-alarm-ratio", "tidb_memory_usage_alarm_ratio"),
            ][..],
        ),
        (
            "plugin",
            &[("load", "plugin_load"), ("dir", "plugin_dir")][..],
        ),
    ]
    .into_iter()
    .map(|(section, mappings)| InstanceConfigSection {
        section_name: section.into(),
        name_mappings: mappings
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
    })
    .collect()
}

/// 支持序列化/反序列化的原子布尔封装。
/// 对应 Go 侧的 `atomicutil.Bool`，允许配置项在运行时被并发读写。
#[derive(Debug, Default)]
pub struct AtomicBool(StdAtomicBool);

impl AtomicBool {
    pub fn new(value: bool) -> Self {
        Self(StdAtomicBool::new(value))
    }
    pub fn load(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn store(&self, value: bool) {
        self.0.store(value, Ordering::SeqCst)
    }
}

impl Clone for AtomicBool {
    fn clone(&self) -> Self {
        Self::new(self.load())
    }
}
impl PartialEq for AtomicBool {
    fn eq(&self, other: &Self) -> bool {
        self.load() == other.load()
    }
}
impl Eq for AtomicBool {}
impl Serialize for AtomicBool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(self.load())
    }
}
impl<'de> Deserialize<'de> for AtomicBool {
    // 兼容布尔值和字符串形式（"true"/"false"/"null"/空串）的反序列化。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = AtomicBool;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bool or bool string")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(AtomicBool::new(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "" | "null" | "false" => Ok(AtomicBool::new(false)),
                    "true" => Ok(AtomicBool::new(true)),
                    _ => Err(E::custom(format!("Invalid value for bool type: {v}"))),
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// 三态布尔：未设置（UNSET）/ 假（FALSE）/ 真（TRUE）。
/// 用于区分“用户未配置”与“用户显式配置为 false”，对应 Go 的 `nullableBool`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NullableBool {
    /// 是否已被显式设置。
    pub is_valid: bool,
    /// 已设置时的布尔取值。
    pub is_true: bool,
}
impl NullableBool {
    pub const UNSET: Self = Self {
        is_valid: false,
        is_true: false,
    };
    pub const FALSE: Self = Self {
        is_valid: true,
        is_true: false,
    };
    pub const TRUE: Self = Self {
        is_valid: true,
        is_true: true,
    };
    /// 转成普通布尔：仅当显式设置为 true 时返回 true。
    pub fn to_bool(self) -> bool {
        self.is_valid && self.is_true
    }
}
impl Serialize for NullableBool {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.is_valid {
            serializer.serialize_none()
        } else {
            serializer.serialize_bool(self.is_true)
        }
    }
}
impl<'de> Deserialize<'de> for NullableBool {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = NullableBool;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bool, null, or empty string")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(if value {
                    NullableBool::TRUE
                } else {
                    NullableBool::FALSE
                })
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullableBool::UNSET)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(NullableBool::UNSET)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.is_empty() {
                    Ok(NullableBool::UNSET)
                } else {
                    Err(E::custom(format!("Invalid value for bool type: {value}")))
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// 部署模式：云服务不同产品形态下的运行模式。
/// 部分配置项（如 error-msg-extension、external-workload）只在特定模式下允许。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeployMode {
    /// 高级版（默认）。
    #[default]
    Premium,
    /// 高级预留版。
    PremiumReserved,
    /// 入门版（Serverless/Starter），多租户共享形态。
    Starter,
}
impl DeployMode {
    /// 是否为入门版（starter）部署模式。
    pub fn is_starter(&self) -> bool {
        *self == Self::Starter
    }
}

/// 日志文件的输出配置。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct FileLogConfig {
    /// 日志文件路径，为空表示输出到标准输出。
    pub filename: String,
    /// 单个日志文件大小上限（MB），超过后滚动切分。
    pub max_size: i64,
}
impl Default for FileLogConfig {
    fn default() -> Self {
        Self {
            filename: String::new(),
            max_size: 300,
        }
    }
}

/// `[log]` 配置分区：日志级别、格式与慢查询日志等。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Log {
    /// 日志级别：debug/info/warn/error/fatal。
    pub level: String,
    /// 日志格式：text 或 json。
    pub format: String,
    /// 禁用时间戳（已废弃，保留兼容），与 `enable_timestamp` 互斥。
    pub disable_timestamp: NullableBool,
    /// 启用时间戳；优先级高于 `disable_timestamp`。
    pub enable_timestamp: NullableBool,
    /// 禁用错误堆栈输出（已废弃，保留兼容）。
    pub disable_error_stack: NullableBool,
    /// 启用错误堆栈输出；优先级高于 `disable_error_stack`。
    pub enable_error_stack: NullableBool,
    /// 日志文件设置。
    pub file: FileLogConfig,
    /// 慢查询日志（记录执行超过阈值的 SQL）文件路径。
    pub slow_query_file: String,
    /// 通用日志（general log，记录所有 SQL）文件路径。
    pub general_log_file: String,
    /// 日志写入超时（秒），0 表示不超时。
    pub timeout: i64,
}
impl Default for Log {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: "text".into(),
            disable_timestamp: NullableBool::UNSET,
            enable_timestamp: NullableBool::UNSET,
            disable_error_stack: NullableBool::UNSET,
            enable_error_stack: NullableBool::UNSET,
            file: FileLogConfig::default(),
            slow_query_file: "tidb-slow.log".into(),
            general_log_file: String::new(),
            timeout: 0,
        }
    }
}
impl Log {
    /// 计算最终是否禁用时间戳：enable 侧显式设置时以其为准，否则看 disable 侧。
    pub fn disable_timestamp(&self) -> bool {
        if self.enable_timestamp.is_valid {
            !self.enable_timestamp.is_true
        } else {
            self.disable_timestamp.to_bool()
        }
    }
    /// 计算最终是否禁用错误堆栈：两侧都未设置时默认禁用。
    pub fn disable_error_stack(&self) -> bool {
        if self.enable_error_stack.is_valid {
            !self.enable_error_stack.is_true
        } else if self.disable_error_stack.is_valid {
            self.disable_error_stack.is_true
        } else {
            true
        }
    }
}

/// `[security]` 配置分区：TLS 证书、权限跳过与落盘加密等安全相关设置。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Security {
    /// SQL 客户端连接的 CA 证书路径。
    pub ssl_ca: String,
    /// SQL 客户端连接的服务端证书路径。
    pub ssl_cert: String,
    /// SQL 客户端连接的服务端私钥路径。
    pub ssl_key: String,
    /// 集群内部组件通信的 CA 证书路径。
    pub cluster_ssl_ca: String,
    /// 集群内部组件通信的证书路径。
    pub cluster_ssl_cert: String,
    /// 集群内部组件通信的私钥路径。
    pub cluster_ssl_key: String,
    /// 落盘文件加密方式：plaintext 或 aes128-ctr。
    pub spilled_file_encryption_method: String,
    /// 是否跳过权限表校验（跳过后所有连接拥有全部权限，仅用于救援）。
    pub skip_grant_table: bool,
    /// 是否在无证书时自动生成自签名 TLS 证书。
    pub auto_tls: bool,
    /// 自动生成证书时的 RSA 密钥长度。
    pub rsa_key_size: i64,
}
impl Default for Security {
    fn default() -> Self {
        Self {
            ssl_ca: String::new(),
            ssl_cert: String::new(),
            ssl_key: String::new(),
            cluster_ssl_ca: String::new(),
            cluster_ssl_cert: String::new(),
            cluster_ssl_key: String::new(),
            spilled_file_encryption_method: SPILLED_FILE_ENCRYPTION_METHOD_PLAINTEXT.into(),
            skip_grant_table: false,
            auto_tls: false,
            rsa_key_size: 4096,
        }
    }
}

/// `[status]` 配置分区：HTTP 状态服务（监控指标、pprof 等）设置。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Status {
    /// 状态服务监听地址。
    pub status_host: String,
    /// 状态服务监听端口。
    pub status_port: u64,
    /// 是否开启状态上报服务。
    pub report_status: bool,
    /// 是否记录 SQL 涉及的数据库标签。
    pub record_db_label: bool,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            status_host: DEF_STATUS_HOST.into(),
            status_port: DEF_STATUS_PORT,
            report_status: true,
            record_db_label: false,
        }
    }
}

/// `[performance]` 配置分区：性能相关参数（事务限制、统计信息加载等）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Performance {
    /// 可使用的最大 CPU 核数（对应 Go 的 GOMAXPROCS），0 表示不限制。
    pub max_procs: u64,
    /// 最大内存（已废弃）。
    pub max_memory: u64,
    /// 是否允许无等值条件的笛卡尔积连接（cross join）。
    pub cross_join: bool,
    /// 伪估算比率：当统计信息过旧时，优化器按此比率做行数估算。
    pub pseudo_estimate_ratio: f64,
    /// 事务单条键值记录大小上限（字节）。
    pub txn_entry_size_limit: u64,
    /// 事务写入总大小上限（字节）。
    pub txn_total_size_limit: u64,
    /// 统计信息同步加载的并发度。
    pub stats_load_concurrency: i64,
    /// 统计信息同步加载的等待队列长度。
    pub stats_load_queue_size: i64,
    /// 内存使用告警比率（已迁移到 instance 分区）。
    pub memory_usage_alarm_ratio: f64,
    /// 是否启用异步 batch-get（批量点查 TiKV）。
    pub enable_async_batch_get: bool,
    /// 是否开启 TCP_NODELAY（关闭 Nagle 算法降低延迟）。
    pub tcp_no_delay: bool,
    /// 单个事务允许的最大语句数。
    pub stmt_count_limit: u64,
    /// 是否强制使用 MPP（TiFlash 大规模并行计算）执行查询。
    pub enforce_mpp: bool,
    /// 启动时是否强制等待统计信息初始化完成。
    pub force_init_stats: bool,
    /// 是否启用统计信息缓存的内存配额限制。
    pub enable_stats_cache_mem_quota: bool,
}
impl Default for Performance {
    fn default() -> Self {
        Self {
            max_procs: 0,
            max_memory: 0,
            cross_join: true,
            pseudo_estimate_ratio: 0.8,
            txn_entry_size_limit: DEF_TXN_ENTRY_SIZE_LIMIT,
            txn_total_size_limit: DEF_TXN_TOTAL_SIZE_LIMIT,
            stats_load_concurrency: DEF_STATS_LOAD_CONCURRENCY_LIMIT,
            stats_load_queue_size: 1000,
            memory_usage_alarm_ratio: 0.8,
            enable_async_batch_get: true,
            tcp_no_delay: true,
            stmt_count_limit: 5000,
            enforce_mpp: false,
            force_init_stats: true,
            enable_stats_cache_mem_quota: true,
        }
    }
}

/// `[instance]` 配置分区：与实例级系统变量对应的配置项（snake_case 命名）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Instance {
    /// 慢查询阈值（毫秒），超过则记入慢查询日志。
    #[serde(rename = "tidb_slow_log_threshold")]
    pub slow_threshold: u64,
    /// 插件审计日志缓冲区大小（字节）。
    pub plugin_audit_log_buffer_size: i64,
    /// 插件审计日志刷盘间隔（秒）。
    pub plugin_audit_log_flush_interval: i64,
    /// 内存使用告警比率（占系统总内存的比例，0~1）。
    pub memory_usage_alarm_ratio: f64,
    /// 最大客户端连接数，0 表示不限制。
    pub max_connections: u32,
    /// 本实例是否可执行 DDL（数据定义语句，如建表）；可在运行时切换。
    pub tidb_enable_ddl: AtomicBool,
    /// Whether Analyze requests collect execution details from storage responses.
    #[serde(rename = "tidb_enable_collect_execution_info")]
    pub enable_collect_execution_info: AtomicBool,
    /// TiDB 在分布式任务框架中的服务作用域。
    pub tidb_service_scope: String,
}
impl Default for Instance {
    fn default() -> Self {
        Self {
            slow_threshold: 300,
            plugin_audit_log_buffer_size: 0,
            plugin_audit_log_flush_interval: 30,
            memory_usage_alarm_ratio: 0.8,
            max_connections: 0,
            tidb_enable_ddl: AtomicBool::new(true),
            enable_collect_execution_info: AtomicBool::new(true),
            tidb_service_scope: String::new(),
        }
    }
}

/// Async Commit 配置；时间字段沿用 Go `time.Duration` 的纳秒表示。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AsyncCommit {
    /// 启用 Async Commit 时允许的最大键数。
    pub keys_limit: usize,
    /// 启用 Async Commit 时允许的键总大小上限。
    pub total_key_size_limit: u64,
    /// 旧 schema 下仍可安全提交的时间窗口（纳秒）。
    pub safe_window: i64,
    /// 在安全窗口之外允许的时钟漂移（纳秒）。
    pub allowed_clock_drift: i64,
}

impl Default for AsyncCommit {
    fn default() -> Self {
        Self {
            keys_limit: 256,
            total_key_size_limit: 4 * 1024,
            safe_window: 2_000_000_000,
            allowed_clock_drift: 500_000_000,
        }
    }
}

/// `[tikv-client]` 配置分区：访问 TiKV 存储层的客户端参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CoprCache {
    /// Coprocessor 结果缓存容量（MiB）。
    pub capacity_mb: u64,
}

impl Default for CoprCache {
    fn default() -> Self {
        Self { capacity_mb: 1000 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TiKVClient {
    /// 单个 TiKV store 的请求限流阈值，0 表示不限流。
    pub store_limit: i64,
    /// 是否开启 RPC 指标采集。
    pub enable_rpc_metrics: bool,
    /// Async Commit 的限制与 schema 安全窗口。
    pub async_commit: AsyncCommit,
    /// Coprocessor 结果缓存配置。
    pub copr_cache: CoprCache,
    /// RU v2 计费系数。
    pub ruv2: TiKVRUV2Config,
}

impl Default for TiKVClient {
    fn default() -> Self {
        Self {
            store_limit: 0,
            enable_rpc_metrics: false,
            async_commit: AsyncCommit::default(),
            copr_cache: CoprCache::default(),
            ruv2: TiKVRUV2Config::default(),
        }
    }
}

/// `[cse]` 配置分区：列存引擎（Columnar Storage Engine）相关设置。
/// TiFlash 是 TiDB 的列式存储副本，用于加速分析型（OLAP）查询。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Cse {
    /// 列存类型：tiflash / columnar / both。
    pub columnar_store_type: String,
    /// 收集列存副本信息的超时（秒）。
    pub columnar_collect_timeout: u64,
}
impl Default for Cse {
    fn default() -> Self {
        Self {
            columnar_store_type: "tiflash".into(),
            columnar_collect_timeout: 5,
        }
    }
}
impl Cse {
    /// 是否启用 TiFlash 列存副本。
    pub fn is_tiflash_enabled(&self) -> bool {
        matches!(self.columnar_store_type.as_str(), "tiflash" | "both")
    }
    /// 是否启用新一代 columnar 列存。
    pub fn is_columnar_store_enabled(&self) -> bool {
        matches!(self.columnar_store_type.as_str(), "columnar" | "both")
    }
    /// 配置是否合法（至少启用一种列存类型）。
    pub fn valid(&self) -> bool {
        self.is_tiflash_enabled() || self.is_columnar_store_enabled()
    }
}

/// `[isolation-read]` 配置分区：限定查询可读取的存储引擎类型。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct IsolationRead {
    /// 允许读取的引擎列表，取值只能是 tidb / tikv / tiflash。
    pub engines: Vec<String>,
}
impl Default for IsolationRead {
    fn default() -> Self {
        Self {
            engines: vec!["tikv".into(), "tiflash".into(), "tidb".into()],
        }
    }
}

/// `[standby]` 配置分区：待机模式设置（Serverless 场景下按需唤醒实例）。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct Standby {
    /// 是否以待机模式启动。
    pub standby_mode: bool,
    /// 是否允许零后端（无活跃计算节点时缩容到零）。
    pub enable_zero_backend: bool,
}

/// `[experimental]` 配置分区：仍处于实验阶段的功能开关。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Experimental {
    /// 是否允许创建表达式索引。
    #[serde(rename = "allow-expression-index")]
    pub allows_expression_index: bool,
    pub allow_enable_foreign_key_check_in_shared_lock: bool,
    /// 是否启用新字符集功能；与 Go 一样不出现在 JSON 中。
    #[serde(skip_serializing)]
    pub enable_new_charset: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct HostedEmbedding {
    #[serde(skip_serializing_if = "false_flag")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_endpoint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key_path: String,
}

impl HostedEmbedding {
    fn configured(&self) -> bool {
        self.enabled || !self.api_endpoint.is_empty() || !self.api_key_path.is_empty()
    }
}

/// `[transaction-summary]` 配置分区：事务摘要采集设置，用于诊断长事务。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct TrxSummary {
    /// 事务摘要表容量（保留的事务条数）。
    pub transaction_summary_capacity: u32,
    /// 事务执行时长超过该值（毫秒）才记录其 ID 摘要。
    pub transaction_id_digest_min_duration: u64,
}
impl Default for TrxSummary {
    fn default() -> Self {
        Self {
            transaction_summary_capacity: 500,
            transaction_id_digest_min_duration: 2147483647,
        }
    }
}

/// 错误消息扩展规则：当错误消息匹配 `pattern` 正则时，追加 `suffix` 后缀。
/// 仅 starter 部署模式允许配置，用于云上定制错误提示。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ErrorMessageExtension {
    /// 匹配错误消息的正则表达式文本。
    pub pattern: String,
    /// 匹配成功后追加的说明文本。
    pub suffix: String,
    /// 编译后的正则（不参与序列化，由 `prepare_error_message_extensions` 填充）。
    #[serde(skip)]
    regexp: Option<Regex>,
}
impl Default for ErrorMessageExtension {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            suffix: String::new(),
            regexp: None,
        }
    }
}
impl ErrorMessageExtension {
    /// 构造一条扩展规则（正则尚未编译）。
    pub fn new(pattern: impl Into<String>, suffix: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            suffix: suffix.into(),
            regexp: None,
        }
    }

    /// 判断错误消息是否命中本规则（正则未编译时恒为 false）。
    pub fn matches(&self, message: &str) -> bool {
        self.regexp.as_ref().is_some_and(|r| r.is_match(message))
    }
}

/// Matches Go `tikvcfg.TxnLocalLatches` used by mockstore drivers.
/// 事务本地闩锁（latch）：在本节点内对键做冲突预检，减少提交阶段冲突。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TxnLocalLatches {
    /// 是否启用本地闩锁。
    pub enabled: bool,
    /// 闩锁哈希表容量。
    pub capacity: u64,
}

/// 服务器顶层配置结构，对应 TOML 配置文件的全部内容。
/// 未识别的配置项会被收集到 `extra` 中，用于在加载时报错。
/// Go PessimisticTxn configuration, including the internal/external autocommit boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PessimisticTxn {
    pub max_retry_count: u64,
    pub deadlock_history_capacity: u64,
    pub deadlock_history_collect_retryable: bool,
    pub pessimistic_auto_commit: AtomicBool,
    pub constraint_check_in_place_pessimistic: bool,
}

impl Default for PessimisticTxn {
    fn default() -> Self {
        Self {
            max_retry_count: 256,
            deadlock_history_capacity: 10,
            deadlock_history_collect_retryable: false,
            pessimistic_auto_commit: AtomicBool::new(astersql_config_kerneltype::IsNextGen()),
            constraint_check_in_place_pessimistic: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct StarterParams {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub export_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bootstrap_file: String,
    #[serde(default, skip_serializing_if = "false_flag")]
    pub enable_manager_notifier: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manager_addr: String,
    #[serde(skip)]
    pub enable_rg_fallback: bool,
    /// Zero disables the decoded IMPORT INTO data-size limit.
    #[serde(default, skip_serializing_if = "zero_size", with = "starter_byte_size")]
    pub max_import_data_size: u64,
}

fn zero_size(value: &u64) -> bool {
    *value == 0
}

fn false_flag(value: &bool) -> bool {
    !*value
}

mod starter_byte_size {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        let text = astersql_config_configtypes::ByteSize_MarshalText(*value)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(std::str::from_utf8(&text).map_err(serde::ser::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut value = 0;
        astersql_config_configtypes::ByteSize_UnmarshalText(&mut value, text.as_bytes())
            .map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

impl Default for StarterParams {
    fn default() -> Self {
        Self {
            export_id: String::new(),
            bootstrap_file: String::new(),
            enable_manager_notifier: false,
            manager_addr: String::new(),
            enable_rg_fallback: false,
            max_import_data_size: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    /// 存储引擎类型，如 "tikv"（分布式）或 "unistore"（内嵌测试用）。
    pub store: String,
    /// SQL 服务监听地址。
    pub host: String,
    /// 对外通告地址（供其他组件访问本实例）。
    pub advertise_address: String,
    /// Store-location labels used to derive local transaction scope.
    pub labels: HashMap<String, String>,
    /// SQL 服务监听端口。
    pub port: u64,
    /// Optional independent PostgreSQL TCP port; absent means disabled.
    pub postgres_port: Option<u16>,
    /// 状态服务的 CORS 跨域配置。
    pub cors: String,
    /// 存储路径；TiKV 模式下为 PD（Placement Driver，集群元信息与调度中心）地址。
    pub path: String,
    /// Unix domain socket 路径，为空则不启用。
    pub socket: String,
    /// 并发会话执行的令牌上限。
    pub token_limit: u64,
    /// 单条 MySQL 协议报文最大长度（字节）。
    pub max_allowed_packet: u64,
    /// 临时目录。
    pub temp_dir: String,
    /// 落盘临时存储路径（算子内存不足时溢写磁盘）。
    #[serde(rename = "tmp-storage-path")]
    pub temp_storage_path: String,
    /// 部署模式。
    pub deploy_mode: DeployMode,
    pub starter_params: StarterParams,
    pub hosted_embedding: HostedEmbedding,
    /// AutoScaler cluster identity used by hosted embedding billing.
    #[serde(
        default,
        rename = "autoscaler-cluster-id",
        skip_serializing_if = "String::is_empty"
    )]
    pub auto_scaler_cluster_id: String,
    pub enable_storage_class: bool,
    /// DXF（分布式执行框架）资源占比限制。
    pub dxf_resource_limit: i64,
    /// keyspace 名称。keyspace 是多租户下逻辑隔离的键空间。
    pub keyspace_name: String,
    /// TiKV worker 服务地址。
    pub tikv_worker_url: String,
    /// 是否开启遥测数据上报。
    pub enable_telemetry: bool,
    /// 是否启用跨节点 KILL。
    pub enable_global_kill: bool,
    /// 是否使用自动扩缩容器（AutoScaler）调度 TiFlash 计算资源。
    #[serde(rename = "use-autoscaler")]
    pub use_auto_scaler: bool,
    /// 计量数据存储 URI（仅支持 s3/azure 协议）。
    pub metering_storage_uri: String,
    /// 日志配置。
    pub log: Log,
    /// 实例级配置。
    pub instance: Instance,
    /// 安全配置。
    pub security: Security,
    /// 状态服务配置。
    pub status: Status,
    /// 性能配置。
    pub performance: Performance,
    /// TiKV 客户端配置。
    pub tikv_client: TiKVClient,
    pub pessimistic_txn: PessimisticTxn,
    /// 实验功能开关。
    pub experimental: Experimental,
    /// RU v2 计费系数。
    #[serde(rename = "ru-v2")]
    pub ruv2: RUV2Config,
    /// 外部负载配置（仅 starter 模式，见同包其他模块）。
    pub external_workload: super::ExternalWorkload,
    /// 启动时进入修复模式的表清单。
    pub repair_table_list: Vec<String>,
    /// keyspace 可观测性配置（按 keyspace 维度输出指标）。
    pub keyspace_observability: super::KeyspaceObservability,
    /// keyspace 可观测性解析后的运行时值（不序列化）。
    #[serde(skip)]
    pub keyspace_observability_values: super::KeyspaceObservabilityValues,
    /// 索引键长度上限（字节）。
    pub max_index_length: i64,
    /// 单表索引数量上限。
    pub index_limit: i64,
    /// 单表列数上限。
    pub table_column_count_limit: u32,
    /// 是否启用表锁（LOCK TABLES 语法）。
    pub enable_table_lock: bool,
    /// 是否限制 ENUM/SET 单个成员的编码长度。
    pub enable_enum_length_limit: bool,
    /// 延迟清理表锁的时间（毫秒）。
    pub delay_clean_table_lock: u64,
    /// 隔离读配置。
    pub isolation_read: IsolationRead,
    /// 待机模式配置。
    pub standby: Standby,
    /// 是否为 keyspace 激活模式（与 standby 互斥）。
    #[serde(rename = "keyspace-activate")]
    pub keyspace_activate_mode: bool,
    /// 错误消息扩展规则列表。
    pub error_msg_extension: Vec<ErrorMessageExtension>,
    /// 事务摘要配置。
    #[serde(rename = "transaction-summary")]
    pub trx_summary: TrxSummary,
    /// 列存引擎配置。
    pub cse: Cse,
    /// Matches Go `Config.TxnLocalLatches` (toml/json omitted).
    /// 事务本地闩锁配置（不参与 toml/json 序列化）。
    #[serde(skip)]
    pub txn_local_latches: TxnLocalLatches,
    /// 捕获所有未识别的配置项；加载时若非空则报错。
    #[serde(flatten)]
    pub extra: toml::Table,
}
impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            store: "unistore".into(),
            host: DEF_HOST.into(),
            advertise_address: String::new(),
            labels: HashMap::new(),
            port: DEF_PORT,
            postgres_port: None,
            cors: String::new(),
            path: "/tmp/tidb".into(),
            socket: "/tmp/tidb-{Port}.sock".into(),
            token_limit: 1000,
            max_allowed_packet: DEF_MAX_ALLOWED_PACKET,
            temp_dir: DEF_TEMP_DIR.into(),
            temp_storage_path: String::new(),
            deploy_mode: DeployMode::Premium,
            starter_params: StarterParams::default(),
            hosted_embedding: HostedEmbedding::default(),
            auto_scaler_cluster_id: String::new(),
            enable_storage_class: false,
            dxf_resource_limit: DEF_DXF_RESOURCE_LIMIT,
            keyspace_name: String::new(),
            tikv_worker_url: String::new(),
            enable_telemetry: false,
            use_auto_scaler: false,
            metering_storage_uri: String::new(),
            enable_global_kill: true,
            log: Log::default(),
            instance: Instance::default(),
            security: Security::default(),
            status: Status::default(),
            performance: Performance::default(),
            tikv_client: TiKVClient::default(),
            pessimistic_txn: PessimisticTxn::default(),
            experimental: Experimental::default(),
            ruv2: RUV2Config::default(),
            external_workload: super::ExternalWorkload::default(),
            repair_table_list: Vec::new(),
            keyspace_observability: super::KeyspaceObservability::default(),
            keyspace_observability_values: super::KeyspaceObservabilityValues::default(),
            max_index_length: DEF_MAX_INDEX_LENGTH,
            index_limit: DEF_INDEX_LIMIT,
            table_column_count_limit: DEF_TABLE_COLUMN_COUNT_LIMIT,
            enable_table_lock: false,
            enable_enum_length_limit: true,
            delay_clean_table_lock: 0,
            isolation_read: IsolationRead::default(),
            standby: Standby::default(),
            keyspace_activate_mode: false,
            error_msg_extension: Vec::new(),
            trx_summary: TrxSummary::default(),
            cse: Cse::default(),
            txn_local_latches: TxnLocalLatches::default(),
            extra: toml::Table::new(),
        };
        // 默认落盘路径按 主机:端口 编码生成，保证同机多实例互不冲突。
        config.temp_storage_path = encode_def_temp_storage_dir(
            std::env::temp_dir(),
            &config.host,
            &config.status.status_host,
            config.port,
            config.status.status_port,
        );
        config
    }
}

/// 配置加载与校验过程中可能出现的错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 通用错误消息（校验失败等）。
    #[error("{0}")]
    Message(String),
    /// 读取配置文件失败。
    #[error("failed to read config {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// TOML 解析失败。
    #[error("failed to decode config {path}: {source}")]
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl Config {
    /// 根据当前 host/port 重新计算落盘临时存储路径。
    /// 若用户自定义了根目录，则在该目录下按端点信息编码生成子目录。
    pub fn update_temp_storage_path(&mut self) {
        let default = default_temp_storage_dir_name();
        let root = if self.temp_storage_path == default {
            std::env::temp_dir()
        } else {
            PathBuf::from(&self.temp_storage_path)
        };
        self.temp_storage_path = encode_def_temp_storage_dir(
            root,
            &self.host,
            &self.status.status_host,
            self.port,
            self.status.status_port,
        );
    }

    /// 从 TOML 文件加载配置，并做部署模式相关的合法性检查。
    /// 成功后整体替换 `self`。
    pub fn load(&mut self, file: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = file.as_ref();
        let input = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.into(),
            source,
        })?;
        let document: toml::Table = toml::from_str(&input).map_err(|source| ConfigError::Toml {
            path: path.into(),
            source,
        })?;
        let mut loaded: Config = toml::from_str(&input).map_err(|source| ConfigError::Toml {
            path: path.into(),
            source,
        })?;
        // token_limit 为 0 时回落到默认值，并裁剪到上限内。
        if loaded.token_limit == 0 {
            loaded.token_limit = 1000;
        }
        loaded.token_limit = loaded.token_limit.min(MAX_TOKEN_LIMIT);
        // 以下配置项只在特定部署模式下允许出现，通过检查原始文本判断用户是否显式配置。
        if document.contains_key("error-msg-extension") && !loaded.deploy_mode.is_starter() {
            return Err(message(
                "error-msg-extension can only be configured when deploy-mode is starter",
            ));
        }
        if document.contains_key("hosted-embedding") && !loaded.deploy_mode.is_starter() {
            return Err(message(
                "hosted-embedding can only be configured for starter deploy mode",
            ));
        }
        let starter_options = document
            .get("starter-params")
            .and_then(toml::Value::as_table);
        if starter_options.is_some_and(|options| options.contains_key("bootstrap-file"))
            && !loaded.starter_params.bootstrap_file.is_empty()
            && !loaded.deploy_mode.is_starter()
        {
            return Err(message(
                "starter-params.bootstrap-file can only be configured for starter deploy mode",
            ));
        }
        if loaded.deploy_mode.is_starter()
            && !starter_options.is_some_and(|options| options.contains_key("max-import-data-size"))
        {
            loaded.starter_params.max_import_data_size = DEF_STARTER_MAX_IMPORT_DATA_SIZE;
        }
        if document.contains_key("dxf-resource-limit")
            && loaded.deploy_mode != DeployMode::PremiumReserved
        {
            return Err(message(
                "dxf-resource-limit can only be configured when deploy-mode is premium_reserved",
            ));
        }
        // starter 模式下若用户未显式配置，则默认开启零后端。
        let zero_backend_defined = document
            .get("standby")
            .and_then(toml::Value::as_table)
            .is_some_and(|standby| standby.contains_key("enable-zero-backend"));
        if loaded.deploy_mode.is_starter() && !zero_backend_defined {
            loaded.standby.enable_zero_backend = true;
        }
        if !loaded.deploy_mode.is_starter() && document.contains_key("external-workload") {
            return Err(message(
                "external-workload can only be configured when deploy-mode is starter",
            ));
        }
        // `extra` 收集了所有未识别的配置项，非空即视为非法配置。
        if let Some(option) = loaded.extra.keys().next() {
            return Err(message(format!("invalid configuration option: {option}")));
        }
        *self = loaded;
        Ok(())
    }

    /// starter 模式下从环境变量注入集群/SQL 两套 TLS 证书配置。
    pub fn adjust_starter_config(&mut self, starter: bool) -> Result<(), ConfigError> {
        if !starter {
            return Ok(());
        }
        apply_security_env(
            &mut self.security,
            ENV_CLUSTER_CA,
            ENV_CLUSTER_CERT,
            ENV_CLUSTER_KEY,
            true,
        )?;
        apply_security_env(
            &mut self.security,
            ENV_SQL_CA,
            ENV_SQL_CERT,
            ENV_SQL_KEY,
            false,
        )
    }

    /// 全面校验配置合法性：范围检查、模式互斥检查等。
    /// 会顺带做少量规范化（如冲突的新旧日志开关归一、加密方法转小写）。
    pub fn valid(&mut self) -> Result<(), ConfigError> {
        if !matches!(
            self.ruv2.report_mode.as_str(),
            RU_REPORT_MODE_RESULT | RU_REPORT_MODE_FULL
        ) {
            return Err(message(format!(
                "invalid ru-v2.report-mode {:?}, expected result or full",
                self.ruv2.report_mode
            )));
        }
        self.ruv2
            .stmt_weights
            .validate()
            .map_err(|error| message(format!("ru-v2.stmt-weights.{error}")))?;
        self.ruv2.ddl_weights.validate()?;
        // enable/disable 两个新旧开关同时设置且值冲突时，忽略废弃的 disable 侧。
        if self.log.enable_error_stack == self.log.disable_error_stack
            && self.log.enable_error_stack != NullableBool::UNSET
        {
            self.log.disable_error_stack = NullableBool::UNSET;
        }
        if self.log.enable_timestamp == self.log.disable_timestamp
            && self.log.enable_timestamp != NullableBool::UNSET
        {
            self.log.disable_timestamp = NullableBool::UNSET;
        }
        if self.security.skip_grant_table && users::get_effective_uid() != 0 {
            return Err(message(
                "TiDB run with skip-grant-table need root privilege",
            ));
        }
        // 以下配置项仅 starter 部署模式可用。
        if !self.error_msg_extension.is_empty() && !self.deploy_mode.is_starter() {
            return Err(message(
                "error-msg-extension can only be configured when deploy-mode is starter",
            ));
        }
        if !self.deploy_mode.is_starter() {
            if self.external_workload.isConfigured() {
                return Err(message(
                    "external-workload can only be configured when deploy-mode is starter",
                ));
            }
        } else {
            self.external_workload.Valid().map_err(message)?;
        }
        let (_, extension_error) =
            prepare_error_message_extensions(&self.error_msg_extension, false);
        if let Some(error) = extension_error {
            return Err(error);
        }
        if !matches!(self.store.as_str(), "tikv" | "unistore" | "mocktikv") {
            return Err(message(format!(
                "invalid store={}, valid storages=[tikv, unistore, mocktikv]",
                self.store
            )));
        }
        if !self.keyspace_observability.Fields.is_empty() && !self.deploy_mode.is_starter() {
            return Err(message(
                "keyspace-observability.fields can only be configured when deploy-mode is starter",
            ));
        }
        self.keyspace_observability.Valid().map_err(message)?;
        // 待机模式与 keyspace 激活模式互斥。
        if self.standby.standby_mode && self.keyspace_activate_mode {
            return Err(message(
                "can't set standby and keyspace-activate mode at the same time",
            ));
        }
        if self.keyspace_activate_mode && !self.deploy_mode.is_starter() {
            return Err(message(
                "keyspace-activate can only be configured for starter deploy mode",
            ));
        }
        if self.starter_params.enable_manager_notifier && !self.deploy_mode.is_starter() {
            return Err(message(
                "starter-params.enable-manager-notifier can only be configured for starter deploy mode",
            ));
        }
        if !self.starter_params.bootstrap_file.is_empty() && !self.deploy_mode.is_starter() {
            return Err(message(
                "starter-params.bootstrap-file can only be configured for starter deploy mode",
            ));
        }
        if self.hosted_embedding.configured() && !self.deploy_mode.is_starter() {
            return Err(message(
                "hosted-embedding can only be configured for starter deploy mode",
            ));
        }
        if self.starter_params.max_import_data_size > 0 && !self.deploy_mode.is_starter() {
            return Err(message(
                "starter-params.max-import-data-size can only be configured for starter deploy mode",
            ));
        }
        // 下面是各数值配置项的取值范围检查。
        if !(MIN_DXF_RESOURCE_LIMIT..=MAX_DXF_RESOURCE_LIMIT).contains(&self.dxf_resource_limit) {
            return Err(message(format!(
                "dxf-resource-limit should be between {MIN_DXF_RESOURCE_LIMIT} and {MAX_DXF_RESOURCE_LIMIT}"
            )));
        }
        if self.dxf_resource_limit != DEF_DXF_RESOURCE_LIMIT
            && self.deploy_mode != DeployMode::PremiumReserved
        {
            return Err(message(
                "dxf-resource-limit can only be configured when deploy-mode is premium_reserved",
            ));
        }
        if self.deploy_mode.is_starter() && !valid_max_allowed_packet(self.max_allowed_packet) {
            return Err(message(format!(
                "max-allowed-packet should be [{MIN_MAX_ALLOWED_PACKET}, {MAX_OF_MAX_ALLOWED_PACKET}] and a multiple of {MAX_ALLOWED_PACKET_UNIT}"
            )));
        }
        if !(DEF_MAX_INDEX_LENGTH..=DEF_MAX_OF_MAX_INDEX_LENGTH).contains(&self.max_index_length) {
            return Err(message(format!(
                "max-index-length should be [{DEF_MAX_INDEX_LENGTH}, {DEF_MAX_OF_MAX_INDEX_LENGTH}]"
            )));
        }
        if !(DEF_INDEX_LIMIT..=DEF_MAX_OF_INDEX_LIMIT).contains(&self.index_limit) {
            return Err(message(format!(
                "index-limit should be [{DEF_INDEX_LIMIT}, {DEF_MAX_OF_INDEX_LIMIT}]"
            )));
        }
        if self.store == "mocktikv" && !self.instance.tidb_enable_ddl.load() {
            return Err(message("can't disable DDL on mocktikv"));
        }
        if self.log.file.max_size > MAX_LOG_FILE_SIZE {
            return Err(message(format!(
                "invalid max log file size={} which is larger than max={MAX_LOG_FILE_SIZE}",
                self.log.file.max_size
            )));
        }
        if !(DEF_TABLE_COLUMN_COUNT_LIMIT..=DEF_MAX_OF_TABLE_COLUMN_COUNT_LIMIT)
            .contains(&self.table_column_count_limit)
        {
            return Err(message(format!(
                "table-column-limit should be [{DEF_TABLE_COLUMN_COUNT_LIMIT}, {DEF_MAX_OF_TABLE_COLUMN_COUNT_LIMIT}]"
            )));
        }
        if !(0..=MAX_PLUGIN_AUDIT_LOG_BUFFER_SIZE)
            .contains(&self.instance.plugin_audit_log_buffer_size)
        {
            return Err(message(format!(
                "plugin-audit-log-buffer-size should be [0, {MAX_PLUGIN_AUDIT_LOG_BUFFER_SIZE}]"
            )));
        }
        if !(1..=MAX_PLUGIN_AUDIT_LOG_FLUSH_INTERVAL)
            .contains(&self.instance.plugin_audit_log_flush_interval)
        {
            return Err(message(format!(
                "plugin-audit-log-flush-interval should be [1, {MAX_PLUGIN_AUDIT_LOG_FLUSH_INTERVAL}]"
            )));
        }
        if self.performance.txn_total_size_limit > 1 << 40 {
            return Err(message(format!(
                "txn-total-size-limit should be less than {}",
                1_u64 << 40
            )));
        }
        if self.trx_summary.transaction_summary_capacity > 5000 {
            return Err(message(
                "transaction-summary.transaction-summary-capacity should not be larger than 5000",
            ));
        }
        if !(0.0..=1.0).contains(&self.instance.memory_usage_alarm_ratio) {
            return Err(message(
                "tidb_memory_usage_alarm_ratio in [Instance] must be greater than or equal to 0 and less than or equal to 1",
            ));
        }
        if !self.keyspace_name.is_empty() && !valid_keyspace_name(&self.keyspace_name) {
            return Err(message(format!(
                "keyspace name {:?} is invalid",
                self.keyspace_name
            )));
        }
        // 计量存储 URI 仅接受 s3/azure 对象存储协议。
        if !self.metering_storage_uri.is_empty() {
            let uri = url::Url::parse(&self.metering_storage_uri)
                .map_err(|error| message(format!("invalid metering-storage-uri: {error}")))?;
            if !matches!(uri.scheme(), "s3" | "azure") || uri.host_str().is_none() {
                return Err(message("invalid metering-storage-uri"));
            }
        }
        if !(DEF_STATS_LOAD_CONCURRENCY_LIMIT..=DEF_MAX_OF_STATS_LOAD_CONCURRENCY_LIMIT)
            .contains(&self.performance.stats_load_concurrency)
        {
            return Err(message(format!(
                "stats-load-concurrency should be [{DEF_STATS_LOAD_CONCURRENCY_LIMIT}, {DEF_MAX_OF_STATS_LOAD_CONCURRENCY_LIMIT}]"
            )));
        }
        if !(DEF_STATS_LOAD_QUEUE_SIZE_LIMIT..=DEF_MAX_OF_STATS_LOAD_QUEUE_SIZE_LIMIT)
            .contains(&self.performance.stats_load_queue_size)
        {
            return Err(message(format!(
                "stats-load-queue-size should be [{DEF_STATS_LOAD_QUEUE_SIZE_LIMIT}, {DEF_MAX_OF_STATS_LOAD_QUEUE_SIZE_LIMIT}]"
            )));
        }
        // 隔离读引擎列表必须非空且只包含合法引擎名。
        if self.isolation_read.engines.is_empty()
            || self
                .isolation_read
                .engines
                .iter()
                .any(|v| !matches!(v.as_str(), "tidb" | "tikv" | "tiflash"))
        {
            return Err(message(
                "type of [isolation-read]engines should be one of tidb, tikv or tiflash",
            ));
        }
        // 加密方法名先规范为小写再校验。
        self.security
            .spilled_file_encryption_method
            .make_ascii_lowercase();
        if !matches!(
            self.security.spilled_file_encryption_method.as_str(),
            SPILLED_FILE_ENCRYPTION_METHOD_PLAINTEXT | SPILLED_FILE_ENCRYPTION_METHOD_AES128_CTR
        ) {
            return Err(message(
                "unsupported [security]spilled-file-encryption-method",
            ));
        }
        if !self.cse.valid() {
            return Err(message(
                "invalid columnar-store-type, valid types=[tiflash, columnar, both]",
            ));
        }
        // 最后校验日志级别是否为已知取值。
        match self.log.level.to_ascii_lowercase().as_str() {
            "debug" | "info" | "warn" | "warning" | "error" | "fatal" => Ok(()),
            _ => Err(message(format!("unrecognized level: {}", self.log.level))),
        }
    }
}

/// 校验 keyspace 名称：长度不超过 20，且仅含字母数字、下划线、连字符。
fn valid_keyspace_name(name: &str) -> bool {
    name.len() <= 20
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// 从环境变量读取一套 TLS 三元组（CA/证书/私钥）并写入安全配置。
/// `cluster` 为 true 时写入集群侧字段，否则写入 SQL 侧字段。
/// 证书与私钥必须成对出现；设置了 CA 则两者都必须存在。
fn apply_security_env(
    security: &mut Security,
    ca_key: &str,
    cert_key: &str,
    key_key: &str,
    cluster: bool,
) -> Result<(), ConfigError> {
    let ca = std::env::var(ca_key).unwrap_or_default();
    let cert = std::env::var(cert_key).unwrap_or_default();
    let key = std::env::var(key_key).unwrap_or_default();
    if cert.is_empty() != key.is_empty() {
        return Err(message(format!(
            "{cert_key} and {key_key} must be set together"
        )));
    }
    if !ca.is_empty() && (cert.is_empty() || key.is_empty()) {
        return Err(message(format!(
            "both {cert_key} and {key_key} must be set when {ca_key} is set"
        )));
    }
    if cluster {
        if !ca.is_empty() {
            security.cluster_ssl_ca = ca;
        }
        if !cert.is_empty() {
            security.cluster_ssl_cert = cert;
            security.cluster_ssl_key = key;
        }
    } else {
        if !ca.is_empty() {
            security.ssl_ca = ca;
        }
        if !cert.is_empty() {
            security.ssl_cert = cert;
            security.ssl_key = key;
        }
    }
    Ok(())
}

/// 便捷函数：把任意文本包装成 `ConfigError::Message`。
fn message(value: impl Into<String>) -> ConfigError {
    ConfigError::Message(value.into())
}

/// 生成落盘临时目录：`<temp>/<uid>_tidb/<base64(host:port/status_host:status_port)>/tmp-storage`。
/// 把监听端点信息编码进路径，保证同一台机器上多个实例的落盘目录互不冲突。
pub fn encode_def_temp_storage_dir(
    temp_dir: impl AsRef<Path>,
    host: &str,
    status_host: &str,
    port: u64,
    status_port: u64,
) -> String {
    let endpoint = format!("{host}:{port}/{status_host}:{status_port}");
    let encoded = URL_SAFE.encode(endpoint.as_bytes());
    let uid = users::get_current_uid().to_string();
    temp_dir
        .as_ref()
        .join(format!("{uid}_tidb"))
        .join(encoded)
        .join("tmp-storage")
        .to_string_lossy()
        .into_owned()
}
/// 使用默认主机与端口生成的落盘临时目录名，用于判断用户是否修改过该路径。
pub fn default_temp_storage_dir_name() -> String {
    encode_def_temp_storage_dir(
        std::env::temp_dir(),
        DEF_HOST,
        DEF_STATUS_HOST,
        DEF_PORT,
        DEF_STATUS_PORT,
    )
}

/// 编译错误消息扩展规则中的正则并排序。
/// `ignore_invalid` 为 true 时跳过无效规则（只记录首个错误）；否则遇错即返回。
/// 排序规则：pattern 越长越靠前（更精确的规则优先匹配）。
pub fn prepare_error_message_extensions(
    extensions: &[ErrorMessageExtension],
    ignore_invalid: bool,
) -> (Vec<ErrorMessageExtension>, Option<ConfigError>) {
    let mut prepared = Vec::with_capacity(extensions.len());
    let mut first_error = None;
    for configured in extensions {
        let mut extension = configured.clone();
        let compiled = if extension.pattern.trim().is_empty() {
            Err(message("empty error-msg-extension pattern"))
        } else {
            Regex::new(&extension.pattern).map_err(|e| {
                message(format!(
                    "invalid error-msg-extension regexp {:?}: {e}",
                    extension.pattern
                ))
            })
        };
        match compiled {
            Ok(regexp) => {
                extension.regexp = Some(regexp);
                prepared.push(extension);
            }
            Err(error) if ignore_invalid => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(error) => return (Vec::new(), Some(error)),
        }
    }
    // 按 pattern 长度降序排列，长（更具体）的规则优先。
    prepared.sort_by(|a, b| {
        b.pattern
            .len()
            .cmp(&a.pattern.len())
            .then_with(|| a.pattern.cmp(&b.pattern))
            .then_with(|| a.suffix.cmp(&b.suffix))
    });
    (prepared, first_error)
}

/// 已被彻底移除的历史配置项集合，加载旧配置时用于识别并提示。
pub fn removed_config() -> HashSet<&'static str> {
    [
        "pessimistic-txn.ttl",
        "pessimistic-txn.enable",
        "log.file.log-rotate",
        "log.log-slow-query",
        "txn-local-latches",
        "txn-local-latches.enabled",
        "txn-local-latches.capacity",
        "performance.max-memory",
        "max-txn-time-use",
        "experimental.allow-auto-random",
        "enable-redact-log",
        "enable-streaming",
        "performance.mem-profile-interval",
        "security.require-secure-transport",
        "lower-case-table-names",
        "stmt-summary",
        "stmt-summary.enable",
        "stmt-summary.enable-internal-query",
        "stmt-summary.max-stmt-count",
        "stmt-summary.max-sql-length",
        "stmt-summary.refresh-interval",
        "stmt-summary.history-size",
        "enable-batch-dml",
        "mem-quota-query",
        "log.query-log-max-len",
        "performance.committer-concurrency",
        "experimental.enable-global-kill",
        "performance.run-auto-analyze",
        "prepared-plan-cache.enabled",
        "prepared-plan-cache.capacity",
        "prepared-plan-cache.memory-guard-ratio",
        "oom-action",
        "check-mb4-value-in-utf8",
        "enable-collect-execution-info",
        "log.enable-slow-log",
        "log.slow-threshold",
        "log.record-plan-in-slow-log",
        "log.expensive-threshold",
        "performance.force-priority",
        "performance.memory-usage-alarm-ratio",
        "plugin.load",
        "plugin.dir",
        "performance.feedback-probability",
        "performance.query-feedback-limit",
        "oom-use-tmp-storage",
        "max-server-connections",
        "run-ddl",
        "instance.tidb_memory_usage_alarm_ratio",
        "enable-global-index",
    ]
    .into_iter()
    .collect()
}
/// 在配置输出（JSON）中需要隐藏的配置项。
pub fn hide_config() -> &'static [&'static str] {
    &["performance.index-usage-sync-lease"]
}
/// 判断输入文本中是否包含隐藏或已移除的配置项（大小写不敏感）。
pub fn contain_hidden_config(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    hide_config()
        .iter()
        .chain(removed_config().iter())
        .any(|item| lower.contains(item))
}
/// 判断给定配置项列表是否全部属于已移除项（是则可安全忽略）。
pub fn is_all_removed_config_items(items: &[String]) -> bool {
    let removed = removed_config();
    items.iter().all(|item| removed.contains(item.as_str()))
}

/// 进程级全局配置快照，通过 RwLock+Arc 支持并发读取与整体替换。
static GLOBAL_CONFIG: Lazy<RwLock<Arc<Config>>> =
    Lazy::new(|| RwLock::new(Arc::new(Config::default())));
/// 已编译好的错误消息扩展规则缓存，随全局配置一起更新。
static PREPARED_EXTENSIONS: Lazy<RwLock<Arc<Vec<ErrorMessageExtension>>>> =
    Lazy::new(|| RwLock::new(Arc::new(Vec::new())));
/// 创建一份默认配置。
pub fn new_config() -> Config {
    Config::default()
}
/// 获取当前全局配置的只读快照（Arc 克隆，代价极小）。
pub fn get_global_config() -> Arc<Config> {
    GLOBAL_CONFIG
        .read()
        .expect("global config lock poisoned")
        .clone()
}
/// 获取当前已编译的错误消息扩展规则副本。
pub fn get_error_message_extensions() -> Vec<ErrorMessageExtension> {
    PREPARED_EXTENSIONS
        .read()
        .expect("extension lock poisoned")
        .as_ref()
        .clone()
}
/// 替换全局配置，并同步重编译错误消息扩展规则缓存。
pub fn store_global_config(config: Config) {
    let (prepared, _) = prepare_error_message_extensions(&config.error_msg_extension, true);
    *PREPARED_EXTENSIONS
        .write()
        .expect("extension lock poisoned") = Arc::new(prepared);
    *GLOBAL_CONFIG.write().expect("global config lock poisoned") = Arc::new(config);
}
/// 以“拷贝-修改-写回”方式原子地更新全局配置。
pub fn update_global(update: impl FnOnce(&mut Config)) {
    let mut config = get_global_config().as_ref().clone();
    update(&mut config);
    store_global_config(config);
}
/// 返回一个恢复闭包：记录当前全局配置，调用闭包即可回滚（多用于测试）。
pub fn restore_func() -> impl FnOnce() {
    let old = get_global_config();
    move || store_global_config(old.as_ref().clone())
}
/// 表锁功能是否启用。
pub fn table_lock_enabled() -> bool {
    get_global_config().enable_table_lock
}
/// 表锁延迟清理时间（毫秒）。
pub fn table_lock_delay_clean() -> u64 {
    get_global_config().delay_clean_table_lock
}
/// 获取全局配置中的 keyspace 名称。
pub fn get_global_keyspace_name() -> String {
    get_global_config().keyspace_name.clone()
}
/// 从配置中提取 TiKV 客户端配置的包装。
pub fn get_tikv_config(config: &Config) -> ClientConfig {
    ClientConfig {
        tikv_client: config.tikv_client.clone(),
    }
}

/// 删表前是否先执行 admin check（编译期链接标志控制，用于测试构建）。
pub static CHECK_TABLE_BEFORE_DROP: Lazy<StdAtomicBool> = Lazy::new(|| StdAtomicBool::new(false));

/// 对应 Go 通过 ldflags 注入的初始化：设置删表前检查开关并关闭遥测。
pub fn init_by_ld_flags(_edition: &str, check_before_drop: &str) {
    let enabled = check_before_drop == "1";
    CHECK_TABLE_BEFORE_DROP.store(enabled, Ordering::SeqCst);
    update_global(|config| config.enable_telemetry = false);
}
/// 获取生效的 `max_allowed_packet`：仅 starter 模式下允许自定义值，否则用默认值。
pub fn get_max_allowed_packet() -> u64 {
    let config = get_global_config();
    if config.deploy_mode.is_starter() && valid_max_allowed_packet(config.max_allowed_packet) {
        config.max_allowed_packet
    } else {
        DEF_MAX_ALLOWED_PACKET
    }
}
/// 校验 `max_allowed_packet` 是否落在合法范围且为 1024 的整数倍。
pub fn valid_max_allowed_packet(value: u64) -> bool {
    (MIN_MAX_ALLOWED_PACKET..=MAX_OF_MAX_ALLOWED_PACKET).contains(&value)
        && value % MAX_ALLOWED_PACKET_UNIT == 0
}

/// 把全局配置序列化为 JSON 字符串，并剔除已移除/隐藏的配置项。
pub fn get_json_config() -> Result<String, ConfigError> {
    let mut value =
        serde_json::to_value(get_global_config().as_ref()).map_err(|e| message(e.to_string()))?;
    // 逐一移除不应对外暴露的配置路径。
    for path in removed_config()
        .into_iter()
        .chain(hide_config().iter().copied())
    {
        remove_json_path(&mut value, path);
    }
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|e| message(e.to_string()))?;
    String::from_utf8(output).map_err(|e| message(e.to_string()))
}
/// 按点分路径（如 "log.file.max-size"）从 JSON 对象树中删除对应键。
fn remove_json_path(value: &mut Value, path: &str) {
    let mut current = value;
    let mut parts = path.split('.').peekable();
    // 沿路径逐级下钻，到最后一段时执行删除。
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            if let Value::Object(map) = current {
                map.remove(part);
            }
            return;
        }
        match current.get_mut(part) {
            Some(next) => current = next,
            None => return,
        }
    }
}

impl FromStr for DeployMode {
    type Err = ConfigError;
    /// 从命令行/配置字符串解析部署模式。
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "premium" => Ok(Self::Premium),
            "premium_reserved" => Ok(Self::PremiumReserved),
            "starter" => Ok(Self::Starter),
            _ => Err(message(format!("invalid deploy-mode={value}"))),
        }
    }
}

/// Load only the optional listener setting for the entry configuration adapter.
/// Other configuration fields retain their existing entry-loader behavior.
pub fn load_postgres_port(file: impl AsRef<Path>) -> Result<Option<u16>, ConfigError> {
    let path = file.as_ref();
    let input = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.into(),
        source,
    })?;
    #[derive(Deserialize)]
    struct ListenerProjection {
        #[serde(default, rename = "postgres-port")]
        postgres_port: Option<u16>,
    }
    let projection: ListenerProjection =
        toml::from_str(&input).map_err(|source| ConfigError::Toml {
            path: path.into(),
            source,
        })?;
    Ok(projection.postgres_port)
}
