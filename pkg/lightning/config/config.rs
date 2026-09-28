// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Lightning 任务配置核心：加载、默认值填充与合法性调整。
//
// 对应 Go `lightning/config`：定义后端模式、TLS、校验和/分析等级、冲突处理策略，
// 以及 mydumper 数据源、tikv-importer、checkpoint 等配置段；`adjust` 会按后端补齐
// 缺省并校验 PD 地址、CSV、磁盘配额等约束。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use crate::{
    ByteSize, DEFAULT_BATCH_IMPORT_RATIO, DEFAULT_BATCH_SIZE, DEFAULT_MAX_ALLOWED_PACKET,
    MAX_REGION_SIZE, READ_BLOCK_SIZE,
};

/// TiKV 导入模式标识：开启导入优化路径。
pub const IMPORT_MODE: &str = "import";
/// TiKV 正常模式标识：恢复常规调度与写入路径。
pub const NORMAL_MODE: &str = "normal";
/// 逻辑导入后端：经 TiDB SQL 写入（非直接写 TiKV）。
pub const BACKEND_TIDB: &str = "tidb";
/// 物理导入后端：本地排序 KV 后写入 TiKV（Region 为数据分片单位）。
pub const BACKEND_LOCAL: &str = "local";
/// IMPORT INTO 后端：对齐 TiDB `IMPORT INTO` 语句导入路径。
pub const BACKEND_IMPORT_INTO: &str = "import-into";
/// 断点（checkpoint）驱动：存入 MySQL/TiDB。
pub const CHECKPOINT_DRIVER_MYSQL: &str = "mysql";
/// 断点驱动：落盘为本地文件。
pub const CHECKPOINT_DRIVER_FILE: &str = "file";
/// 磁盘配额无上限哨兵值。
pub const UNLIMITED_QUOTA: ByteSize = ByteSize(i64::MAX);
/// 默认 KV 发送批次大小（字节）。
pub const KV_WRITE_BATCH_SIZE: i64 = 16 * 1024;
/// local 后端默认 range 并发度。
pub const DEFAULT_RANGE_CONCURRENCY: i32 = 16;
/// local 后端默认表级并发度。
pub const DEFAULT_TABLE_CONCURRENCY: i32 = 6;
/// Region 状态检查退避上限（对齐 Go 默认）。
pub const DEFAULT_REGION_CHECK_BACKOFF_LIMIT: i32 = 1800;
/// 一次批量分裂 Region 的默认批大小。
pub const DEFAULT_REGION_SPLIT_BATCH_SIZE: i32 = 4096;
/// 冲突记录默认阈值。
pub const DEFAULT_RECORD_DUPLICATE_THRESHOLD: i64 = 10_000;
/// local 引擎内存缓存默认大小（512MiB）。
pub const DEFAULT_ENGINE_MEM_CACHE_SIZE: ByteSize = ByteSize(512 * 1024 * 1024);
/// local writer 内存缓存默认大小（128MiB）。
pub const DEFAULT_LOCAL_WRITER_MEM_CACHE_SIZE: ByteSize = ByteSize(128 * 1024 * 1024);
/// 本地写入块默认大小（16KiB）。
pub const DEFAULT_BLOCK_SIZE: ByteSize = ByteSize(16 * 1024);
/// 定时切换 TiKV 导入/正常模式的默认间隔。
pub const DEFAULT_SWITCH_TIKV_MODE_INTERVAL: StdDuration = StdDuration::from_secs(300);

#[derive(Debug)]
/// 配置解析、校验或 IO 相关错误。
pub enum ConfigError {
    Invalid(String),
    Parse(String),
    Io(std::io::Error),
    Help,
}

/// 格式化为 Lightning 配置错误前缀或底层消息。
impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => {
                write!(formatter, "[Lightning:Config:ErrInvalidConfig]{message}")
            }
            Self::Parse(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
            Self::Help => formatter.write_str("help requested"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// 将 IO 错误包装为配置错误。
impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 默认表过滤规则：导入全部用户表，排除系统库。
pub fn get_default_filter() -> Vec<String> {
    [
        "*.*",
        "!mysql.*",
        "!sys.*",
        "!INFORMATION_SCHEMA.*",
        "!PERFORMANCE_SCHEMA.*",
        "!METRICS_SCHEMA.*",
        "!INSPECTION_SCHEMA.*",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 运行时 TLS 连接选项（跳过校验、最低版本、ALPN）。
pub struct TlsConfig {
    pub insecure_skip_verify: bool,
    pub min_tls_12: bool,
    pub next_protocols: Vec<String>,
}

#[derive(Clone, Debug, Default)]
/// TLS 证书路径/字节与脱敏日志等安全相关配置。
pub struct Security {
    pub ca_path: String,
    pub cert_path: String,
    pub key_path: String,
    pub redact_info_log: bool,
    pub tls_config: Option<TlsConfig>,
    pub allow_fallback_to_plaintext: bool,
    pub ca_bytes: Vec<u8>,
    pub cert_bytes: Vec<u8>,
    pub key_bytes: Vec<u8>,
}

impl Security {
    /// 根据 CA/证书/密钥是否齐全构建或跳过 TLS 配置。
    pub fn build_tls_config(&mut self) -> Result<(), ConfigError> {
        if self.tls_config.is_some() {
            return Ok(());
        }
        let cert_present = !self.cert_path.is_empty() || !self.cert_bytes.is_empty();
        let key_present = !self.key_path.is_empty() || !self.key_bytes.is_empty();
        if cert_present != key_present {
            return Err(ConfigError::Invalid(
                "TLS certificate and key must be configured together".into(),
            ));
        }
        if self.ca_path.is_empty() && self.ca_bytes.is_empty() && !cert_present && !key_present {
            return Ok(());
        }
        self.tls_config = Some(TlsConfig {
            min_tls_12: true,
            next_protocols: vec!["h2".into(), "http/1.1".into()],
            ..TlsConfig::default()
        });
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// 从 TiDB `/settings` 发现的端口与 PD path。
pub struct TiDbSettings {
    pub port: i32,
    pub path: String,
}

/// 提供 TiDB 设置的抽象，便于测试注入固定结果。
pub trait SettingsProvider {
    fn settings(&self) -> Result<TiDbSettings, ConfigError>;
}

#[derive(Default)]
/// 不可用的 SettingsProvider：调用即报错。
pub struct NoSettings;

impl SettingsProvider for NoSettings {
    fn settings(&self) -> Result<TiDbSettings, ConfigError> {
        Err(ConfigError::Invalid("TiDB settings are unavailable".into()))
    }
}

#[derive(Debug, Default)]
/// `[tidb]` 段：连接信息、SQL_MODE、并发与 TLS。
pub struct DBStore {
    pub host: String,
    pub port: i32,
    pub user: String,
    pub password: String,
    pub status_port: i32,
    pub pd_addr: String,
    pub sql_mode_text: String,
    pub tls: String,
    pub security: Option<Security>,
    pub sql_mode: Vec<String>,
    pub max_allowed_packet: u64,
    pub distsql_scan_concurrency: i32,
    pub build_stats_concurrency: i32,
    pub index_serial_scan_concurrency: i32,
    pub checksum_table_concurrency: i32,
    pub vars: HashMap<String, String>,
    pub io_total_bytes: Option<AtomicU64>,
    pub uuid: String,
}

impl DBStore {
    /// 按后端补齐并发默认值、解析 SQL_MODE/TLS，必要时从 SettingsProvider 发现 port/pd-addr。
    pub fn adjust(
        &mut self,
        importer: &TikvImporter,
        security: &Security,
        settings: &dyn SettingsProvider,
    ) -> Result<(), ConfigError> {
        if importer.backend == BACKEND_LOCAL {
            if self.build_stats_concurrency == 0 {
                self.build_stats_concurrency = 20;
            }
            if self.index_serial_scan_concurrency == 0 {
                self.index_serial_scan_concurrency = 20;
            }
            if self.checksum_table_concurrency == 0 {
                self.checksum_table_concurrency = 2;
            }
        }
        self.sql_mode = parse_sql_mode(&self.sql_mode_text)?;
        let selected = self.security.get_or_insert_with(|| security.clone());
        // 按 tidb.tls 语义配置跳过校验、集群证书或明文。
        match self.tls.as_str() {
            "preferred" => {
                selected.allow_fallback_to_plaintext = true;
                ensure_insecure_tls(selected);
            }
            "skip-verify" => ensure_insecure_tls(selected),
            "cluster" if security.ca_path.is_empty() && security.ca_bytes.is_empty() => {
                return Err(ConfigError::Invalid(
                    "cannot set tidb.tls to cluster without security".into(),
                ));
            }
            "cluster" => selected.build_tls_config()?,
            "" => {}
            "false" => {
                selected.tls_config = None;
                selected.ca_path.clear();
                selected.cert_path.clear();
                selected.key_path.clear();
                selected.ca_bytes.clear();
                selected.cert_bytes.clear();
                selected.key_bytes.clear();
            }
            other => {
                return Err(ConfigError::Invalid(format!(
                    "unsupported tidb.tls config {other}"
                )));
            }
        }
        // local 后端缺省时通过 SettingsProvider 发现 port 与 PD 地址。
        let internal = importer.backend == BACKEND_LOCAL;
        if internal && (self.port <= 0 || self.pd_addr.is_empty()) {
            let discovered = settings.settings().map_err(|error| {
                ConfigError::Invalid(format!(
                    "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {error}"
                ))
            })?;
            if self.port <= 0 {
                self.port = discovered.port;
            }
            if self.pd_addr.is_empty() {
                self.pd_addr = validate_pd_addrs(&discovered.path)?;
            }
        }
        if self.port <= 0 {
            return Err(ConfigError::Invalid("invalid `tidb.port` setting".into()));
        }
        if internal && self.pd_addr.is_empty() {
            return Err(ConfigError::Invalid(
                "invalid `tidb.pd-addr` setting".into(),
            ));
        }
        Ok(())
    }
}

/// 强制跳过证书校验（preferred / skip-verify）。
fn ensure_insecure_tls(security: &mut Security) {
    let tls = security.tls_config.get_or_insert_with(|| TlsConfig {
        min_tls_12: true,
        next_protocols: vec!["h2".into(), "http/1.1".into()],
        ..TlsConfig::default()
    });
    tls.insecure_skip_verify = true;
}

/// 将逗号分隔 SQL_MODE 文本解析为大写模式列表并做字符校验。
fn parse_sql_mode(value: &str) -> Result<Vec<String>, ConfigError> {
    let modes: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(|mode| mode.to_ascii_uppercase())
        .collect();
    if modes.iter().any(|mode| {
        !mode
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    }) {
        return Err(ConfigError::Invalid(
            "tidb.sql-mode must be a valid SQL_MODE".into(),
        ));
    }
    Ok(modes)
}

/// 校验逗号分隔的 `host:port` PD 地址列表。
/// PD（Placement Driver）负责集群元信息与 Region 调度。
fn validate_pd_addrs(value: &str) -> Result<String, ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::Invalid(
            "invalid `tidb.pd-addr` setting".into(),
        ));
    }
    for address in value.split(',') {
        let mut parts = address.trim().split(':');
        let host = parts.next().unwrap_or_default();
        let port = parts.next().unwrap_or_default();
        if host.is_empty() {
            return Err(ConfigError::Invalid(
                "invalid `tidb.pd-addr` setting".into(),
            ));
        }
        // Go only rejects an empty/"0" settings path port here. DNS service
        // names and values outside u16 are left to the downstream PD client.
        if port.is_empty() || port == "0" {
            return Err(ConfigError::Invalid("invalid `tidb.port` setting".into()));
        }
    }
    Ok(value.to_owned())
}

#[derive(Clone, Debug, Default)]
/// 表路由规则：源 schema/table 模式映射到目标，可带源文件路径。
pub struct TableRouteRule {
    pub schema_pattern: String,
    pub table_pattern: String,
    pub target_schema: String,
    pub target_table: String,
    pub source: String,
}

/// 表路由规则列表。
pub type Routes = Vec<TableRouteRule>;

/// 将相对源路径拼到 data-source-dir，并要求 schema 模式非空。
fn adjust_routes(routes: &mut Routes, mydumper: &MydumperRuntime) -> Result<(), ConfigError> {
    for route in routes {
        if !route.source.is_empty() && Path::new(&route.source).is_relative() {
            route.source = Path::new(&mydumper.source_dir)
                .join(&route.source)
                .to_string_lossy()
                .into_owned();
        }
        if route.schema_pattern.is_empty() || route.target_schema.is_empty() {
            return Err(ConfigError::Invalid("invalid table route rule".into()));
        }
    }
    Ok(())
}

#[derive(Debug)]
/// Lightning 单任务完整配置树。
pub struct Config {
    pub task_id: i64,
    pub app: Lightning,
    pub tidb: DBStore,
    pub checkpoint: Checkpoint,
    pub mydumper: MydumperRuntime,
    pub tikv_importer: TikvImporter,
    pub post_restore: PostRestore,
    pub cron: Cron,
    pub routes: Routes,
    pub security: Security,
    pub conflict: Conflict,
}

impl Config {
    /// 导出关键字段的精简 JSON 摘要（用于日志）。
    pub fn string(&self) -> String {
        format!(
            "{{\"task-id\":{},\"tidb\":{{\"host\":\"{}\",\"port\":{},\"pd-addr\":\"{}\"}},\"mydumper\":{{\"data-source-dir\":\"{}\"}},\"tikv-importer\":{{\"backend\":\"{}\"}}}}",
            self.task_id,
            json_escape(&self.tidb.host),
            self.tidb.port,
            json_escape(&self.tidb.pd_addr),
            json_escape(&self.mydumper.source_dir),
            json_escape(&self.tikv_importer.backend),
        )
    }

    /// 临时脱敏 source_dir 中的敏感查询参数后生成摘要。
    pub fn redact(&mut self) -> String {
        let original = std::mem::take(&mut self.mydumper.source_dir);
        self.mydumper.source_dir = redact_url(&original);
        let output = self.string();
        self.mydumper.source_dir = original;
        output
    }

    /// 先加载全局 TOML，再用全局覆盖字段覆盖关键项。
    pub fn load_from_global(&mut self, global: &crate::GlobalConfig) -> Result<(), ConfigError> {
        self.load_from_toml(&global.config_file_content)?;
        self.tidb.host.clone_from(&global.tidb.host);
        self.tidb.port = global.tidb.port;
        self.tidb.user.clone_from(&global.tidb.user);
        self.tidb.password.clone_from(&global.tidb.password);
        self.tidb.status_port = global.tidb.status_port;
        self.tidb.pd_addr.clone_from(&global.tidb.pd_addr);
        self.mydumper.no_schema = global.mydumper.no_schema;
        self.mydumper
            .source_dir
            .clone_from(&global.mydumper.source_dir);
        self.mydumper.filter.clone_from(&global.mydumper.filter);
        self.mydumper
            .ignore_columns
            .clone_from(&global.mydumper.ignore_columns);
        self.tikv_importer
            .backend
            .clone_from(&global.tikv_importer.backend);
        self.tikv_importer
            .sorted_kv_dir
            .clone_from(&global.tikv_importer.sorted_kv_dir);
        self.checkpoint.enable = global.checkpoint.enable;
        self.post_restore.checksum = global.post_restore.checksum;
        self.post_restore.analyze = global.post_restore.analyze;
        self.app.check_requirements = global.app.check_requirements;
        self.security.clone_from(&global.security);
        Ok(())
    }

    /// 从 TOML 字节流填充本配置。
    pub fn load_from_toml(&mut self, data: &[u8]) -> Result<(), ConfigError> {
        crate::toml_codec::load_config_from_toml(self, data)
    }

    /// 调整顺序对齐 Go：importer → app → mydumper → post-restore → TiDB → checkpoint → routes → conflict。
    /// Adjustment order matches Go: importer, app, mydumper, post-restore,
    /// TiDB, checkpoint, routes, then conflict.
    pub fn adjust_with_settings(
        &mut self,
        settings: &dyn SettingsProvider,
    ) -> Result<(), ConfigError> {
        self.tikv_importer.adjust()?;
        self.app.adjust(&self.tikv_importer);
        self.mydumper.adjust()?;
        self.post_restore.adjust(&self.tikv_importer);
        self.tidb
            .adjust(&self.tikv_importer, &self.security, settings)?;
        self.checkpoint.adjust(&self.tidb);
        adjust_routes(&mut self.routes, &self.mydumper)?;
        self.conflict.adjust(&self.tikv_importer)
    }

    /// 当 port/pd-addr 缺失时通过 HTTP 拉取 TiDB `/settings`，对齐 Go `Adjust`。
    /// Fetch TiDB `/settings` over HTTP when port/pd-addr are missing, matching Go `Adjust`.
    pub fn adjust(&mut self) -> Result<(), ConfigError> {
        let settings = HttpSettings {
            host: self.tidb.host.clone(),
            status_port: self.tidb.status_port,
        };
        self.adjust_with_settings(&settings)
    }
}

/// [`Config::adjust`] 使用的 HTTP `/settings` 提供者。
/// HTTP `/settings` provider used by [`Config::adjust`].
pub struct HttpSettings {
    pub host: String,
    pub status_port: i32,
}

impl SettingsProvider for HttpSettings {
    fn settings(&self) -> Result<TiDbSettings, ConfigError> {
        fetch_tidb_settings(&self.host, self.status_port)
    }
}

/// 通过 HTTP GET `/settings` 拉取 TiDB 端口与 PD path。
fn fetch_tidb_settings(host: &str, status_port: i32) -> Result<TiDbSettings, ConfigError> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration as StdDuration;

    let addr = format!("{host}:{status_port}");
    let mut stream = TcpStream::connect(&addr).map_err(|error| {
        ConfigError::Invalid(format!(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {error}"
        ))
    })?;
    stream
        .set_read_timeout(Some(StdDuration::from_secs(5)))
        .ok();
    stream
        .set_write_timeout(Some(StdDuration::from_secs(5)))
        .ok();
    // 最小 HTTP/1.0 请求，解析状态行与 JSON body。
    let request = format!("GET /settings HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| {
        ConfigError::Invalid(format!(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {error}"
        ))
    })?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|error| {
        ConfigError::Invalid(format!(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {error}"
        ))
    })?;
    let text = String::from_utf8_lossy(&raw);
    let Some((_, body)) = text.split_once("\r\n\r\n") else {
        return Err(ConfigError::Invalid(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: empty body"
                .into(),
        ));
    };
    let status_line = text.lines().next().unwrap_or_default();
    if !status_line.contains("200") {
        return Err(ConfigError::Invalid(format!(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {status_line}"
        )));
    }
    let value: serde_json::Value = serde_json::from_str(body.trim()).map_err(|error| {
        ConfigError::Invalid(format!(
            "cannot fetch settings from TiDB, please manually fill in `tidb.port` and `tidb.pd-addr`: {error}"
        ))
    })?;
    Ok(TiDbSettings {
        port: value.get("port").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        path: value
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

#[derive(Debug, Default)]
/// `[lightning]` 应用段：并发、错误上限与元信息 schema。
pub struct Lightning {
    pub table_concurrency: i32,
    pub index_concurrency: i32,
    pub region_concurrency: i32,
    pub io_concurrency: i32,
    pub check_requirements: bool,
    pub meta_schema_name: String,
    pub max_error: MaxError,
    pub max_error_records: i64,
    pub task_info_schema_name: String,
}

impl Lightning {
    /// 按后端补齐表/索引/Region 并发与元 schema 默认值。
    pub fn adjust(&mut self, importer: &TikvImporter) {
        match importer.backend.as_str() {
            BACKEND_TIDB => {
                if self.table_concurrency == 0 {
                    self.table_concurrency = self.region_concurrency;
                }
                if self.index_concurrency == 0 {
                    self.index_concurrency = self.region_concurrency;
                }
            }
            BACKEND_LOCAL => {
                if self.index_concurrency == 0 {
                    self.index_concurrency = 2;
                }
                if self.table_concurrency == 0 {
                    self.table_concurrency = DEFAULT_TABLE_CONCURRENCY;
                }
                if self.meta_schema_name.is_empty() {
                    self.meta_schema_name = "lightning_metadata".into();
                }
                self.region_concurrency = self.region_concurrency.min(cpu_count());
            }
            BACKEND_IMPORT_INTO => {
                if self.meta_schema_name.is_empty() {
                    self.meta_schema_name = "lightning_metadata".into();
                }
                if self.table_concurrency == 0 {
                    self.table_concurrency = self.region_concurrency;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 导入后操作等级：关闭、可选或必须（校验和/ANALYZE 等）。
pub enum PostOpLevel {
    #[default]
    Off,
    Optional,
    Required,
}

impl PostOpLevel {
    /// 从配置字符串解析为 PostOpLevel。
    pub fn from_string_value(&mut self, value: &str) -> Result<(), ConfigError> {
        *self = match value.to_ascii_lowercase().as_str() {
            "off" | "false" => Self::Off,
            "optional" => Self::Optional,
            "required" | "true" => Self::Required,
            _ => return Err(ConfigError::Parse(format!("invalid op level '{value}'"))),
        };
        Ok(())
    }

    /// 序列化为配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Optional => "optional",
            Self::Required => "required",
        }
    }
}

impl fmt::Display for PostOpLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 成功后断点保留策略：删除、重命名或原样保留。
pub enum CheckpointKeepStrategy {
    #[default]
    Remove,
    Rename,
    Origin,
}

impl CheckpointKeepStrategy {
    /// 从配置字符串解析断点保留策略。
    pub fn from_string_value(&mut self, value: &str) -> Result<(), ConfigError> {
        *self = match value.to_ascii_lowercase().as_str() {
            "remove" | "false" => Self::Remove,
            "rename" | "true" => Self::Rename,
            "origin" => Self::Origin,
            _ => {
                return Err(ConfigError::Parse(format!(
                    "invalid checkpoint keep strategy '{value}'"
                )));
            }
        };
        Ok(())
    }
}

#[derive(Debug)]
/// 各类错误允许的最大次数（原子计数，供运行时扣减）。
pub struct MaxError {
    pub syntax: AtomicI64,
    pub charset: AtomicI64,
    pub r#type: AtomicI64,
    pub conflict: AtomicI64,
}

impl Default for MaxError {
    fn default() -> Self {
        Self {
            syntax: AtomicI64::new(0),
            charset: AtomicI64::new(i64::MAX),
            r#type: AtomicI64::new(0),
            conflict: AtomicI64::new(i64::MAX),
        }
    }
}

impl MaxError {
    /// 兼容旧版单一整型：仅设置 type 上限，syntax=0、charset=无限。
    pub fn set_legacy_value(&self, value: i64) {
        self.syntax.store(0, Ordering::Relaxed);
        self.charset.store(i64::MAX, Ordering::Relaxed);
        self.r#type.store(value.max(0), Ordering::Relaxed);
    }

    /// 从 TOML 表字段设置 type 上限。
    pub fn set_table(&self, values: &HashMap<String, i64>) {
        self.syntax.store(0, Ordering::Relaxed);
        self.charset.store(i64::MAX, Ordering::Relaxed);
        self.r#type.store(
            values
                .get("type")
                .copied()
                .filter(|value| *value >= 0)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 主键/唯一键冲突处理策略。
pub enum DuplicateResolutionAlgorithm {
    #[default]
    None,
    Replace,
    Ignore,
    Error,
}

impl DuplicateResolutionAlgorithm {
    /// 解析冲突策略字符串（含 remove/record 等别名）。
    pub fn from_string_value(&mut self, value: &str) -> Result<(), ConfigError> {
        *self = match value.to_ascii_lowercase().as_str() {
            "" | "none" => Self::None,
            "replace" | "remove" | "record" => Self::Replace,
            "ignore" => Self::Ignore,
            "error" => Self::Error,
            _ => {
                return Err(ConfigError::Parse(format!(
                    "invalid conflict.strategy '{value}'"
                )));
            }
        };
        Ok(())
    }

    /// 序列化为配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Replace => "replace",
            Self::Ignore => "ignore",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// KV 对传输压缩类型。
pub enum CompressionType {
    #[default]
    None,
    Gzip,
}

impl CompressionType {
    /// 序列化为配置字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Gzip => "gzip",
        }
    }

    /// 解析压缩类型（目前支持 gzip）。
    pub fn from_string_value(&mut self, value: &str) -> Result<(), ConfigError> {
        *self = match value.to_ascii_lowercase().as_str() {
            "" => Self::None,
            "gz" | "gzip" => Self::Gzip,
            _ => {
                return Err(ConfigError::Parse(format!(
                    "invalid compression-type '{value}', please choose valid option between ['gzip']"
                )));
            }
        };
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 导入完成后的校验和、ANALYZE、compact 等后处理配置。
pub struct PostRestore {
    pub checksum: PostOpLevel,
    pub analyze: PostOpLevel,
    pub level1_compact: bool,
    pub post_process_at_last: bool,
    pub compact: bool,
    pub checksum_via_sql: bool,
}

impl PostRestore {
    /// tidb 后端关闭物理导入相关的后处理项。
    fn adjust(&mut self, importer: &TikvImporter) {
        if importer.backend == BACKEND_TIDB {
            self.checksum = PostOpLevel::Off;
            self.analyze = PostOpLevel::Off;
            self.compact = false;
            self.checksum_via_sql = false;
        }
    }
}

#[derive(Clone, Debug, Default)]
/// mydumper CSV 方言：分隔符、引号、NULL 表示与转义。
pub struct CSVConfig {
    pub fields_terminated_by: String,
    pub fields_enclosed_by: String,
    pub lines_terminated_by: String,
    pub field_null_defined_by: Vec<String>,
    pub header: bool,
    pub header_schema_match: bool,
    pub trim_last_empty_field: bool,
    pub not_null: bool,
    pub backslash_escape: bool,
    pub fields_escaped_by: String,
    pub lines_starting_by: String,
    pub allow_empty_line: bool,
    pub quoted_null_is_text: bool,
    pub unescaped_quote: bool,
}

impl CSVConfig {
    /// 校验分隔符/引号互不为前缀，并规范化转义与 NULL 定义。
    fn adjust(&mut self) -> Result<(), ConfigError> {
        if self.fields_terminated_by.is_empty() {
            return Err(ConfigError::Invalid(
                "`mydumper.csv.separator` must not be empty".into(),
            ));
        }
        if !self.fields_enclosed_by.is_empty()
            && (self
                .fields_terminated_by
                .starts_with(&self.fields_enclosed_by)
                || self
                    .fields_enclosed_by
                    .starts_with(&self.fields_terminated_by))
        {
            return Err(ConfigError::Invalid(
                "`mydumper.csv.separator` and `mydumper.csv.delimiter` must not be prefix of each other".into(),
            ));
        }
        if self.fields_escaped_by.chars().count() > 1 {
            return Err(ConfigError::Invalid(
                "CSV escaped-by must be one character".into(),
            ));
        }
        // backslash-escape 与 escaped-by 互相同步。
        if self.backslash_escape && self.fields_escaped_by.is_empty() {
            self.fields_escaped_by = "\\".into();
        } else if !self.backslash_escape && self.fields_escaped_by == "\\" {
            self.fields_escaped_by.clear();
        }
        if !self.not_null && self.field_null_defined_by.is_empty() {
            self.field_null_defined_by.push(String::new());
        }
        for delimiter in [
            &self.fields_terminated_by,
            &self.fields_enclosed_by,
            &self.lines_terminated_by,
        ] {
            if !self.fields_escaped_by.is_empty() && delimiter == &self.fields_escaped_by {
                let which = if delimiter == &self.fields_terminated_by {
                    "separator"
                } else if delimiter == &self.fields_enclosed_by {
                    "delimiter"
                } else {
                    "terminator"
                };
                return Err(ConfigError::Invalid(format!(
                    "cannot use '{}' both as CSV {which} and `mydumper.csv.escaped-by`",
                    self.fields_escaped_by
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
/// 源文件路由规则：按路径模式映射到 schema/table 与压缩类型。
pub struct FileRouteRule {
    pub pattern: String,
    pub path: String,
    pub schema: String,
    pub table: String,
    pub file_type: String,
    pub key: String,
    pub compression: String,
    pub unescape: bool,
}

#[derive(Clone, Debug, Default)]
/// 按库表或过滤器忽略的列集合。
pub struct IgnoreColumns {
    pub db: String,
    pub table: String,
    pub table_filter: Vec<String>,
    pub columns: Vec<String>,
}

impl IgnoreColumns {
    /// 将忽略列列表转为集合便于查找。
    pub fn columns_map(&self) -> HashSet<String> {
        self.columns.iter().cloned().collect()
    }
}

/// 全部忽略列规则。
pub type AllIgnoreColumns = Vec<IgnoreColumns>;

/// 按库表名或 table_filter 规则查找匹配的忽略列配置。
pub fn get_ignore_columns<'a>(
    items: &'a AllIgnoreColumns,
    database: &str,
    table: &str,
    case_sensitive: bool,
) -> Option<&'a IgnoreColumns> {
    let db = if case_sensitive {
        database.into()
    } else {
        database.to_ascii_lowercase()
    };
    let tbl = if case_sensitive {
        table.into()
    } else {
        table.to_ascii_lowercase()
    };
    items.iter().find(|item| {
        (item.db == db && item.table == tbl)
            || item
                .table_filter
                .iter()
                .any(|rule| table_filter_matches(rule, &db, &tbl))
    })
}

#[derive(Debug, Default)]
/// `[mydumper]` 段：数据源目录、过滤、CSV 与读批大小。
pub struct MydumperRuntime {
    pub read_block_size: ByteSize,
    pub batch_size: ByteSize,
    pub batch_import_ratio: f64,
    pub source_id: String,
    pub source_dir: String,
    pub character_set: String,
    pub csv: CSVConfig,
    pub max_region_size: ByteSize,
    pub filter: Vec<String>,
    pub file_routers: Vec<FileRouteRule>,
    pub no_schema: bool,
    pub case_sensitive: bool,
    pub strict_format: bool,
    pub default_file_rules: bool,
    pub ignore_columns: AllIgnoreColumns,
    pub data_character_set: String,
    pub data_invalid_char_replace: String,
}

impl MydumperRuntime {
    /// 校验 CSV/strict-format，规范化文件路由、字符集与批导入比例。
    fn adjust(&mut self) -> Result<(), ConfigError> {
        self.csv.adjust()?;
        if self.strict_format && self.csv.lines_terminated_by.is_empty() {
            return Err(ConfigError::Invalid(
                "mydumper.strict-format can not be used with empty mydumper.csv.terminator. Please set mydumper.csv.terminator to a non-empty value like \"\\r\\n\"".into(),
            ));
        }
        for rule in &mut self.file_routers {
            let path = Path::new(&rule.path);
            if path.is_absolute() {
                let source = Path::new(&self.source_dir);
                let rel = path.strip_prefix(source).map_err(|_| {
                    ConfigError::Invalid(format!(
                        "file route path '{}' is not in source dir '{}'",
                        rule.path, self.source_dir
                    ))
                })?;
                // Reject paths that escape via ".."
                if rel
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(ConfigError::Invalid(format!(
                        "file route path '{}' is not in source dir '{}'",
                        rule.path, self.source_dir
                    )));
                }
                rule.path = rel.to_string_lossy().into_owned();
            }
        }
        if self.file_routers.is_empty() {
            self.default_file_rules = true;
        }
        if self.data_character_set.is_empty() {
            self.data_character_set = "binary".into();
        }
        parse_charset(&self.data_character_set)?;
        if !(0.0..1.0).contains(&self.batch_import_ratio) {
            self.batch_import_ratio = DEFAULT_BATCH_IMPORT_RATIO;
        }
        if self.read_block_size.0 <= 0 {
            self.read_block_size = READ_BLOCK_SIZE;
        }
        if self.character_set.is_empty() {
            self.character_set = "auto".into();
        }
        for ignored in &mut self.ignore_columns {
            for column in &mut ignored.columns {
                *column = column.to_ascii_lowercase();
            }
        }
        self.adjust_file_path()
    }

    /// 将本地路径规范化为 `file://` URL，或接受受支持的对象存储 URL。
    pub fn adjust_file_path(&mut self) -> Result<(), ConfigError> {
        let supported = [
            "file", "local", "s3", "noop", "gcs", "gs", "azure", "azblob",
        ];
        // Mirror Go: only URL-parse when this is not a Windows volume path.
        let maybe_url = url::Url::parse(&self.source_dir).ok();
        if let Some(u) = maybe_url.as_ref() {
            if !u.scheme().is_empty() && supported.contains(&u.scheme()) {
                // Already a supported storage URL.
                return Ok(());
            }
            // Scheme present but unsupported (and not a bare filesystem path).
            if !u.scheme().is_empty()
                && u.scheme().len() > 1
                && !Path::new(&self.source_dir).exists()
                && !self.source_dir.starts_with('.')
                && !Path::new(&self.source_dir).is_absolute()
            {
                return Err(ConfigError::Invalid(format!(
                    "unsupported data-source-dir url '{}', supported storage types are {}",
                    self.source_dir,
                    supported.join(",")
                )));
            }
        }
        if self.source_dir.is_empty() {
            return Err(ConfigError::Invalid(
                "`mydumper.data-source-dir` is not set".into(),
            ));
        }
        if !Path::new(&self.source_dir).exists() {
            return Err(ConfigError::Invalid(format!(
                "'{}': `mydumper.data-source-dir` does not exist",
                self.source_dir
            )));
        }
        let abs = std::fs::canonicalize(&self.source_dir).map_err(|error| {
            ConfigError::Invalid(format!(
                "covert data-source-dir '{}' to absolute path failed: {error}",
                self.source_dir
            ))
        })?;
        let file_url = url::Url::from_file_path(&abs).map_err(|_| {
            ConfigError::Invalid(format!(
                "covert data-source-dir '{}' to absolute path failed",
                self.source_dir
            ))
        })?;
        self.source_dir = file_url.to_string();
        Ok(())
    }
}

#[derive(Debug)]
/// `[tikv-importer]` 段：后端、排序目录、Region 分裂与冲突策略。
pub struct TikvImporter {
    pub addr: String,
    pub backend: String,
    pub on_duplicate: DuplicateResolutionAlgorithm,
    pub max_kv_pairs: i32,
    pub send_kv_pairs: i32,
    pub send_kv_size: ByteSize,
    pub compress_kv_pairs: CompressionType,
    pub region_split_size: ByteSize,
    pub region_split_keys: i32,
    pub region_split_batch_size: i32,
    pub region_split_concurrency: i32,
    pub region_check_backoff_limit: i32,
    pub sorted_kv_dir: String,
    pub disk_quota: ByteSize,
    pub range_concurrency: i32,
    pub duplicate_resolution: DuplicateResolutionAlgorithm,
    pub incremental_import: bool,
    pub parallel_import: bool,
    pub keyspace_name: String,
    pub add_index_by_sql: bool,
    pub strip_s3_external_id_for_import_sql: bool,
    pub engine_mem_cache_size: ByteSize,
    pub local_writer_mem_cache_size: ByteSize,
    pub store_write_bw_limit: ByteSize,
    pub logical_import_batch_size: ByteSize,
    pub logical_import_batch_rows: i32,
    pub logical_import_prep_stmt: bool,
    pub pause_pd_scheduler_scope: String,
    pub block_size: ByteSize,
}

impl TikvImporter {
    /// 校验后端必填项并按 backend 补齐 local/tidb 相关默认值。
    pub fn adjust(&mut self) -> Result<(), ConfigError> {
        if self.backend.is_empty() {
            return Err(ConfigError::Invalid(
                "tikv-importer.backend must not be empty!".into(),
            ));
        }
        self.backend.make_ascii_lowercase();
        if !self.parallel_import && self.incremental_import {
            self.parallel_import = true;
        }
        // 按后端分别校验逻辑批大小或 sorted-kv-dir / Region 分裂参数。
        match self.backend.as_str() {
            BACKEND_TIDB => {
                if self.logical_import_batch_size.0 <= 0 || self.logical_import_batch_rows <= 0 {
                    return Err(ConfigError::Invalid(
                        "`tikv-importer` logical import batch size/rows must be positive".into(),
                    ));
                }
            }
            BACKEND_LOCAL => {
                if self.region_split_batch_size <= 0 || self.region_split_concurrency <= 0 {
                    return Err(ConfigError::Invalid(format!(
                        "`tikv-importer.region-split-batch-size` got {}, should be larger than 0",
                        self.region_split_batch_size
                    )));
                }
                if self.range_concurrency == 0 {
                    self.range_concurrency = DEFAULT_RANGE_CONCURRENCY;
                }
                if self.engine_mem_cache_size.0 == 0 {
                    self.engine_mem_cache_size = DEFAULT_ENGINE_MEM_CACHE_SIZE;
                }
                if self.local_writer_mem_cache_size.0 == 0 {
                    self.local_writer_mem_cache_size = DEFAULT_LOCAL_WRITER_MEM_CACHE_SIZE;
                }
                if self.block_size.0 == 0 {
                    self.block_size = DEFAULT_BLOCK_SIZE;
                }
                if self.parallel_import && self.add_index_by_sql {
                    return Err(ConfigError::Invalid(
                        "tikv-importer.add-index-using-ddl cannot be used with tikv-importer.parallel-import".into(),
                    ));
                }
                if self.sorted_kv_dir.is_empty() {
                    return Err(ConfigError::Invalid(
                        "tikv-importer.sorted-kv-dir must not be empty!".into(),
                    ));
                }
                match std::fs::metadata(Path::new(&self.sorted_kv_dir)) {
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(ConfigError::Invalid(format!(
                            "tikv-importer.sorted-kv-dir ({}) is not a directory",
                            self.sorted_kv_dir
                        )));
                    }
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(ConfigError::Invalid(format!(
                            "invalid sorted-kv-dir: {error}"
                        )));
                    }
                    _ => {}
                }
            }
            BACKEND_IMPORT_INTO => {}
            _ => {
                return Err(ConfigError::Invalid(format!(
                    "unsupported `tikv-importer.backend` ({})",
                    self.backend
                )));
            }
        }
        self.pause_pd_scheduler_scope.make_ascii_lowercase();
        if !matches!(self.pause_pd_scheduler_scope.as_str(), "table" | "global") {
            return Err(ConfigError::Invalid(
                "pause-pd-scheduler-scope is invalid, valid options: [table, global]".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
/// `[checkpoint]` 段：断点驱动、DSN 与成功后保留策略。
pub struct Checkpoint {
    pub schema: String,
    pub dsn: String,
    pub mysql_param: Option<MySqlConnectParam>,
    pub driver: String,
    pub enable: bool,
    pub keep_after_success: CheckpointKeepStrategy,
}

#[derive(Clone, Debug)]
/// MySQL 断点驱动的连接参数。
pub struct MySqlConnectParam {
    pub host: String,
    pub port: i32,
    pub user: String,
    pub password: String,
    pub max_allowed_packet: u64,
    pub allow_fallback_to_plaintext: bool,
    pub sql_mode: String,
}

impl MySqlConnectParam {
    /// 格式化为 go-sql-driver 风格 DSN。
    pub fn format_dsn(&self) -> String {
        let mode = urlencoding_encode(&format!("'{0}'", self.sql_mode));
        format!(
            "{user}:{password}@tcp({host}:{port})/?charset=utf8mb4&sql_mode={mode}",
            user = self.user,
            password = self.password,
            host = self.host,
            port = self.port,
            mode = mode,
        )
    }
}

/// 对 DSN 中 sql_mode 等值做 percent-encoding。
fn urlencoding_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl Checkpoint {
    /// 补齐 schema/driver 默认值，并按驱动生成 DSN 或 MySQL 参数。
    fn adjust(&mut self, database: &DBStore) {
        if self.schema.is_empty() {
            self.schema = "tidb_lightning_checkpoint".into();
        }
        if self.driver.is_empty() {
            self.driver = CHECKPOINT_DRIVER_FILE.into();
        }
        if self.dsn.is_empty() {
            match self.driver.as_str() {
                CHECKPOINT_DRIVER_MYSQL => {
                    self.mysql_param = Some(MySqlConnectParam {
                        host: database.host.clone(),
                        port: database.port,
                        user: database.user.clone(),
                        password: database.password.clone(),
                        max_allowed_packet: DEFAULT_MAX_ALLOWED_PACKET,
                        allow_fallback_to_plaintext: database
                            .security
                            .as_ref()
                            .is_some_and(|value| value.allow_fallback_to_plaintext),
                        sql_mode: database.sql_mode_text.clone(),
                    });
                }
                CHECKPOINT_DRIVER_FILE => self.dsn = format!("/tmp/{}.pb", self.schema),
                _ => {}
            }
        } else {
            self.dsn = self
                .dsn
                .split('&')
                // 去掉历史遗留的 allowAllFiles=true 参数。
                .filter(|part| !part.eq_ignore_ascii_case("allowAllFiles=true"))
                .collect::<Vec<_>>()
                .join("&");
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 配置用时长包装：以有符号纳秒保存，完整兼容 Go `time.Duration`。
pub struct Duration(pub i64);

impl Duration {
    /// 从 Go duration 文本解析。
    pub fn unmarshal_text(&mut self, text: &[u8]) -> Result<(), ConfigError> {
        self.0 = parse_go_duration(
            std::str::from_utf8(text).map_err(|error| ConfigError::Parse(error.to_string()))?,
        )?;
        Ok(())
    }

    /// 编码为带引号的 Go duration 字符串 JSON。
    pub fn marshal_json(&self) -> Result<Vec<u8>, ConfigError> {
        Ok(format!("\"{}\"", self.go_string()).into_bytes())
    }

    /// 格式对齐 Go `time.Duration.String()`（如 `1m0s`、`13m20s`）。
    /// Format like Go `time.Duration.String()` (e.g. `1m0s`, `13m20s`, `3s`).
    pub fn go_string(&self) -> String {
        if self.0 == 0 {
            return "0s".into();
        }
        let sign = if self.0 < 0 { "-" } else { "" };
        let value = self.0.unsigned_abs();
        if value < 1_000 {
            return format!("{sign}{value}ns");
        }
        if value < 1_000_000 {
            let whole = value / 1_000;
            let fraction = format!("{:03}", value % 1_000)
                .trim_end_matches('0')
                .to_owned();
            return if fraction.is_empty() {
                format!("{sign}{whole}µs")
            } else {
                format!("{sign}{whole}.{fraction}µs")
            };
        }
        if value < 1_000_000_000 {
            let whole = value / 1_000_000;
            let fraction = format!("{:06}", value % 1_000_000)
                .trim_end_matches('0')
                .to_owned();
            return if fraction.is_empty() {
                format!("{sign}{whole}ms")
            } else {
                format!("{sign}{whole}.{fraction}ms")
            };
        }
        let total = value / 1_000_000_000;
        let nanos = value % 1_000_000_000;
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        let seconds = total % 60;
        let mut out = String::new();
        out.push_str(sign);
        if hours > 0 {
            out.push_str(&format!("{hours}h"));
        }
        if minutes > 0 || (hours > 0 && (seconds > 0 || nanos > 0)) {
            out.push_str(&format!("{minutes}m"));
        } else if hours > 0 && seconds == 0 && nanos == 0 {
            out.push_str("0m");
            out.push('0');
            out.push('s');
            return out;
        }
        if seconds > 0 || nanos > 0 || out.is_empty() || hours > 0 || minutes > 0 {
            if nanos == 0 {
                out.push_str(&format!("{seconds}s"));
            } else {
                let frac = format!("{nanos:09}").trim_end_matches('0').to_owned();
                out.push_str(&format!("{seconds}.{frac}s"));
            }
        }
        // Go always emits the zero unit after hours: "1h0m0s" style for exact hours
        // and "1m0s" for exact minutes. Align common Lightning cases.
        if minutes > 0 && seconds == 0 && nanos == 0 && hours == 0 {
            return format!("{sign}{minutes}m0s");
        }
        if hours > 0 && minutes == 0 && seconds == 0 && nanos == 0 {
            return format!("{sign}{hours}h0m0s");
        }
        if hours > 0 && seconds == 0 && nanos == 0 {
            return format!("{sign}{hours}h{minutes}m0s");
        }
        out
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 定时任务间隔：切模式、打进度、查磁盘配额。
pub struct Cron {
    pub switch_mode: Duration,
    pub log_progress: Duration,
    pub check_disk_quota: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 数据文件字符集。
pub enum Charset {
    Binary,
    Utf8Mb4,
    Gb18030,
    Gbk,
    Latin1,
    Ascii,
}

/// 解析 data-character-set 配置值。
pub fn parse_charset(value: &str) -> Result<Charset, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "" | "binary" => Ok(Charset::Binary),
        "utf8" | "utf8mb4" => Ok(Charset::Utf8Mb4),
        "gb18030" => Ok(Charset::Gb18030),
        "gbk" => Ok(Charset::Gbk),
        "latin1" => Ok(Charset::Latin1),
        "ascii" => Ok(Charset::Ascii),
        _ => Err(ConfigError::Parse(format!(
            "found unsupported data-character-set: {value}"
        ))),
    }
}

#[derive(Debug)]
/// `[conflict]` 段：冲突策略、阈值与预检查开关。
pub struct Conflict {
    pub strategy: DuplicateResolutionAlgorithm,
    pub precheck_conflict_before_import: bool,
    pub threshold: i64,
    pub max_record_rows: i64,
}

impl Conflict {
    /// 合并废弃 on-duplicate/duplicate-resolution，并校验与后端的兼容性。
    pub fn adjust(&mut self, importer: &TikvImporter) -> Result<(), ConfigError> {
        // 优先 conflict.strategy，否则回退到 importer 侧废弃字段。
        let mut source = "conflict.strategy";
        if self.strategy == DuplicateResolutionAlgorithm::None {
            if importer.on_duplicate == DuplicateResolutionAlgorithm::None
                && importer.backend == BACKEND_TIDB
            {
                self.strategy = DuplicateResolutionAlgorithm::Error;
            } else if importer.on_duplicate != DuplicateResolutionAlgorithm::None {
                source = "tikv-importer.on-duplicate";
                self.strategy = importer.on_duplicate;
            }
        }
        let from_deprecated = self.strategy == DuplicateResolutionAlgorithm::None
            && importer.duplicate_resolution != DuplicateResolutionAlgorithm::None;
        if from_deprecated {
            self.strategy = importer.duplicate_resolution;
        } else if self.strategy != DuplicateResolutionAlgorithm::None
            && importer.duplicate_resolution != DuplicateResolutionAlgorithm::None
        {
            return Err(ConfigError::Invalid(format!(
                "{source} cannot be used with tikv-importer.duplicate-resolution"
            )));
        }
        if self.strategy == DuplicateResolutionAlgorithm::Ignore
            && importer.backend == BACKEND_LOCAL
        {
            return Err(ConfigError::Invalid(format!(
                "{source} cannot be set to \"ignore\" when use tikv-importer.backend = \"local\""
            )));
        }
        if self.precheck_conflict_before_import && importer.backend == BACKEND_TIDB {
            return Err(ConfigError::Invalid(
                "conflict.precheck-conflict-before-import cannot be set to true when use tikv-importer.backend = \"tidb\"".into(),
            ));
        }
        if self.threshold < 0 {
            self.threshold = match self.strategy {
                DuplicateResolutionAlgorithm::Error | DuplicateResolutionAlgorithm::None => 0,
                DuplicateResolutionAlgorithm::Ignore | DuplicateResolutionAlgorithm::Replace => {
                    DEFAULT_RECORD_DUPLICATE_THRESHOLD
                }
            };
        }
        if self.threshold > 0 && self.strategy == DuplicateResolutionAlgorithm::Error {
            return Err(ConfigError::Invalid(
                "conflict.threshold cannot be set when use conflict.strategy = \"error\"".into(),
            ));
        }
        self.max_record_rows = if self.strategy == DuplicateResolutionAlgorithm::Replace
            && importer.backend == BACKEND_TIDB
        {
            0
        } else {
            self.threshold
        };
        Ok(())
    }
}

/// 构造带合理默认值的新配置（并发度取可用 CPU 数）。
pub fn new_config() -> Config {
    let cpu = cpu_count();
    Config {
        task_id: 0,
        app: Lightning {
            region_concurrency: cpu,
            io_concurrency: 5,
            check_requirements: true,
            task_info_schema_name: "lightning_task_info".into(),
            ..Lightning::default()
        },
        checkpoint: Checkpoint {
            schema: String::new(),
            dsn: String::new(),
            mysql_param: None,
            driver: String::new(),
            enable: true,
            keep_after_success: CheckpointKeepStrategy::Remove,
        },
        tidb: DBStore {
            host: "127.0.0.1".into(),
            user: "root".into(),
            status_port: 10080,
            sql_mode_text: "ONLY_FULL_GROUP_BY,NO_AUTO_CREATE_USER".into(),
            max_allowed_packet: DEFAULT_MAX_ALLOWED_PACKET,
            build_stats_concurrency: 20,
            distsql_scan_concurrency: 15,
            index_serial_scan_concurrency: 20,
            checksum_table_concurrency: 2,
            ..DBStore::default()
        },
        cron: Cron {
            switch_mode: Duration(DEFAULT_SWITCH_TIKV_MODE_INTERVAL.as_nanos() as i64),
            log_progress: Duration(300_000_000_000),
            check_disk_quota: Duration(60_000_000_000),
        },
        mydumper: MydumperRuntime {
            read_block_size: READ_BLOCK_SIZE,
            batch_size: DEFAULT_BATCH_SIZE,
            csv: CSVConfig {
                fields_terminated_by: ",".into(),
                fields_enclosed_by: "\"".into(),
                field_null_defined_by: vec!["\\N".into()],
                header: true,
                header_schema_match: true,
                backslash_escape: true,
                fields_escaped_by: "\\".into(),
                ..CSVConfig::default()
            },
            max_region_size: MAX_REGION_SIZE,
            filter: get_default_filter(),
            data_character_set: "binary".into(),
            data_invalid_char_replace: "�".into(),
            ..MydumperRuntime::default()
        },
        tikv_importer: TikvImporter {
            backend: String::new(),
            max_kv_pairs: 4096,
            send_kv_pairs: 32768,
            send_kv_size: ByteSize(KV_WRITE_BATCH_SIZE),
            region_split_batch_size: DEFAULT_REGION_SPLIT_BATCH_SIZE,
            region_split_concurrency: cpu,
            region_check_backoff_limit: DEFAULT_REGION_CHECK_BACKOFF_LIMIT,
            disk_quota: UNLIMITED_QUOTA,
            pause_pd_scheduler_scope: "table".into(),
            block_size: DEFAULT_BLOCK_SIZE,
            logical_import_batch_size: ByteSize(96 * 1024),
            logical_import_batch_rows: 65_536,
            addr: String::new(),
            on_duplicate: DuplicateResolutionAlgorithm::None,
            compress_kv_pairs: CompressionType::None,
            region_split_size: ByteSize(0),
            region_split_keys: 0,
            sorted_kv_dir: String::new(),
            range_concurrency: 0,
            duplicate_resolution: DuplicateResolutionAlgorithm::None,
            incremental_import: false,
            parallel_import: false,
            keyspace_name: String::new(),
            add_index_by_sql: false,
            strip_s3_external_id_for_import_sql: false,
            engine_mem_cache_size: ByteSize(0),
            local_writer_mem_cache_size: ByteSize(0),
            store_write_bw_limit: ByteSize(0),
            logical_import_prep_stmt: false,
        },
        post_restore: PostRestore {
            checksum: PostOpLevel::Required,
            analyze: PostOpLevel::Optional,
            level1_compact: false,
            post_process_at_last: true,
            compact: false,
            checksum_via_sql: false,
        },
        routes: Vec::new(),
        security: Security::default(),
        conflict: Conflict {
            strategy: DuplicateResolutionAlgorithm::None,
            precheck_conflict_before_import: false,
            threshold: -1,
            max_record_rows: -1,
        },
    }
}

/// 轻量 TOML 扁平化解析：产出 `section.key` → 原始值字符串列表。
pub(crate) fn parse_toml_document(data: &[u8]) -> Result<Vec<(String, String)>, ConfigError> {
    let text = std::str::from_utf8(data).map_err(|error| ConfigError::Parse(error.to_string()))?;
    let mut section = String::new();
    let mut output = Vec::new();
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            ConfigError::Parse(format!("invalid TOML at line {}", line_number + 1))
        })?;
        let full = if section.is_empty() {
            key.trim().to_owned()
        } else {
            format!("{}.{}", section, key.trim())
        };
        output.push((full, value.trim().to_owned()));
    }
    Ok(output)
}

/// 去掉双引号并还原常见转义。
pub(crate) fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

/// 解析布尔配置值。
pub(crate) fn parse_bool(key: &str, value: &str) -> Result<bool, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::Parse(format!("{key} must be a boolean")))
}

/// 解析整型配置值。
pub(crate) fn parse_i32(key: &str, value: &str) -> Result<i32, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::Parse(format!("{key} must be an integer")))
}

/// 仅允许出现在全局配置、任务 TOML 中应忽略的键。
fn is_global_only_key(key: &str) -> bool {
    matches!(
        key,
        "lightning.status-addr"
            | "lightning.server-mode"
            | "lightning.pprof-port"
            | "mydumper.filter"
            | "mydumper.ignore-columns"
    )
}

/// 解析 Go `time.ParseDuration` 子集（支持复合如 `13m20s`）。
fn parse_go_duration(value: &str) -> Result<i64, ConfigError> {
    // Subset of Go time.ParseDuration supporting compound forms like "13m20s".
    if value == "0" {
        return Ok(0);
    }
    let (negative, mut rest) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let mut total_nanos: f64 = 0.0;
    if rest.is_empty() {
        return Err(ConfigError::Parse(format!(
            "time: invalid duration \"{value}\""
        )));
    }
    while !rest.is_empty() {
        let split = rest
            .find(|ch: char| ch.is_ascii_alphabetic() || ch == 'µ' || ch == 'μ')
            .ok_or_else(|| ConfigError::Parse(format!("time: invalid duration \"{value}\"")))?;
        let amount: f64 = rest[..split]
            .parse()
            .map_err(|_| ConfigError::Parse(format!("time: invalid duration \"{value}\"")))?;
        let unit_end = split
            + rest[split..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphabetic() || *ch == 'µ' || *ch == 'μ')
                .map(|ch| ch.len_utf8())
                .sum::<usize>();
        let unit = &rest[split..unit_end];
        let multiplier = match unit {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            other => {
                return Err(ConfigError::Parse(format!(
                    "time: unknown unit \"{other}\" in duration \"{value}\""
                )));
            }
        };
        if amount < 0.0 || !amount.is_finite() {
            return Err(ConfigError::Parse(format!(
                "time: invalid duration \"{value}\""
            )));
        }
        total_nanos += amount * multiplier * 1_000_000_000.0;
        rest = &rest[unit_end..];
    }
    let limit = if negative {
        i64::MAX as f64 + 1.0
    } else {
        i64::MAX as f64
    };
    if !total_nanos.is_finite() || total_nanos > limit {
        return Err(ConfigError::Parse(format!(
            "time: invalid duration \"{value}\""
        )));
    }
    let nanos = total_nanos as u64;
    if negative {
        if nanos == i64::MAX as u64 + 1 {
            Ok(i64::MIN)
        } else {
            Ok(-(nanos as i64))
        }
    } else {
        Ok(nanos as i64)
    }
}

/// 判断 `db.table` 通配规则（可带 `!` 前缀）是否匹配。
fn table_filter_matches(rule: &str, database: &str, table: &str) -> bool {
    let rule = rule.strip_prefix('!').unwrap_or(rule);
    let Some((db, tbl)) = rule.split_once('.') else {
        return false;
    };
    wildcard(db, database) && wildcard(tbl, table)
}

/// `*` 或大小写不敏感相等。
fn wildcard(pattern: &str, value: &str) -> bool {
    pattern == "*" || pattern.eq_ignore_ascii_case(value)
}

/// 脱敏对象存储 URL 中的 access-key 等敏感查询参数。
fn redact_url(value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(value) else {
        return value.to_owned();
    };
    // 按存储协议选择需打码的查询键名。
    let sensitive: &[&str] = match parsed.scheme().to_ascii_lowercase().as_str() {
        "s3" | "ks3" | "oss" => &["access-key", "secret-access-key", "session-token"],
        "azure" | "azblob" => &["account-key", "encryption-key", "sas-token"],
        _ => return value.to_owned(),
    };
    let mut pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    for (key, val) in &mut pairs {
        let normalized = key.to_ascii_lowercase().replace('_', "-");
        if sensitive.contains(&normalized.as_str()) {
            *val = "xxxxxx".into();
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, val) in &pairs {
        serializer.append_pair(key, val);
    }
    let query = serializer.finish();
    parsed.set_query((!query.is_empty()).then_some(query.as_str()));
    parsed.to_string()
}

/// 对摘要 JSON 字符串值做最小转义。
fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 返回可用 CPU 数，供默认并发度使用。
fn cpu_count() -> i32 {
    astersql_util_cpu::GetCPUCount()
}
