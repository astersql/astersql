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

// Lightning 全局（Global）配置加载：CLI 参数与 TOML 轻量字段。
//
// 对应 Go 侧 `GlobalConfig`：解析命令行 flag、读取配置文件中的全局段，
// 再与默认值合并。完整任务配置由 `Config::load_from_global` 后续解码。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    ConfigError, IgnoreColumns, PostOpLevel, Security, get_default_filter, parse_bool, parse_i32,
};

#[derive(Clone, Debug, Default)]
/// Lightning 进程日志配置：级别与输出文件路径。
pub struct LogConfig {
    pub level: String,
    pub file: String,
}

impl LogConfig {
    /// 规范化日志级别：空则默认 info；warning 映射为 warn。
    pub fn adjust(&mut self) {
        if self.level.is_empty() {
            self.level = "info".into();
        }
        if self.level == "warning" {
            self.level = "warn".into();
        }
    }
}

#[derive(Clone, Debug)]
/// 全局 Lightning 应用段：日志、状态地址、服务模式与依赖检查开关。
pub struct GlobalLightning {
    pub log_config: LogConfig,
    pub status_addr: String,
    pub server_mode: bool,
    pub check_requirements: bool,
    pub pprof_port: i32,
}

#[derive(Clone, Debug)]
/// 全局 TiDB 连接信息：主机、端口、账号、PD 地址与日志级别。
/// PD（Placement Driver）负责调度 Region 与存储元数据。
pub struct GlobalTiDB {
    pub host: String,
    pub port: i32,
    pub user: String,
    pub password: String,
    pub status_port: i32,
    pub pd_addr: String,
    pub log_level: String,
}

#[derive(Clone, Debug)]
/// 数据源（mydumper/dumpling 导出）相关全局选项。
pub struct GlobalMydumper {
    pub source_dir: String,
    pub no_schema: bool,
    pub filter: Vec<String>,
    pub ignore_columns: Vec<IgnoreColumns>,
}

#[derive(Clone, Debug, Default)]
/// TiKV Importer / 后端选择与本地排序目录。
pub struct GlobalImporter {
    pub backend: String,
    pub sorted_kv_dir: String,
}

#[derive(Clone, Debug)]
/// 全局配置聚合体：应用、检查点、TiDB、数据源、导入后端、恢复后操作与安全。
pub struct GlobalConfig {
    pub app: GlobalLightning,
    pub checkpoint: GlobalCheckpoint,
    pub tidb: GlobalTiDB,
    pub mydumper: GlobalMydumper,
    pub tikv_importer: GlobalImporter,
    pub post_restore: GlobalPostRestore,
    pub security: Security,
    pub config_file_content: Vec<u8>,
}

#[derive(Clone, Debug)]
/// 检查点（checkpoint）开关：失败重启时从上次进度继续。
pub struct GlobalCheckpoint {
    pub enable: bool,
}

#[derive(Clone, Debug)]
/// 导入完成后的后处理级别：校验和（checksum）与统计信息收集（analyze）。
pub struct GlobalPostRestore {
    pub checksum: PostOpLevel,
    pub analyze: PostOpLevel,
}

/// 构造带合理默认值的全局配置（对齐 Go `NewGlobalConfig`）。
pub fn new_global_config() -> GlobalConfig {
    GlobalConfig {
        app: GlobalLightning {
            log_config: LogConfig::default(),
            status_addr: String::new(),
            server_mode: false,
            check_requirements: true,
            pprof_port: 0,
        },
        checkpoint: GlobalCheckpoint { enable: true },
        tidb: GlobalTiDB {
            host: "127.0.0.1".into(),
            port: 0,
            user: "root".into(),
            password: String::new(),
            status_port: 10080,
            pd_addr: String::new(),
            log_level: "error".into(),
        },
        mydumper: GlobalMydumper {
            source_dir: String::new(),
            no_schema: false,
            filter: get_default_filter(),
            ignore_columns: Vec::new(),
        },
        tikv_importer: GlobalImporter::default(),
        post_restore: GlobalPostRestore {
            checksum: PostOpLevel::Required,
            analyze: PostOpLevel::Optional,
        },
        security: Security::default(),
        config_file_content: Vec::new(),
    }
}

/// Mirrors the command entrypoint helper: help is success, other failures use
/// exit status 2. Kept separate from loading so library users can avoid exits.
/// 命令入口辅助：Help 视为成功退出 0，其它错误打印后以状态码 2 退出。
pub fn must(config: Result<GlobalConfig, ConfigError>) -> GlobalConfig {
    match config {
        Ok(config) => config,
        Err(ConfigError::Help) => std::process::exit(0),
        Err(error) => {
            println!("{error}");
            std::process::exit(2);
        }
    }
}

/// 在临时目录生成带纳秒时间戳的默认日志文件名。
fn timestamp_log_file_name() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("lightning.log.{nanos}"))
}

#[derive(Default)]
/// 额外 CLI flag 注册表，供嵌入命令在解析前声明自定义参数。
pub struct FlagSet {
    additional: HashSet<String>,
}

impl FlagSet {
    /// Allows the embedding command to declare a flag before parsing, matching
    /// the Go `extraFlags` callback timing.
    /// 允许嵌入命令在解析前注册额外 flag，时机对齐 Go `extraFlags` 回调。
    pub fn register(&mut self, name: impl Into<String>) {
        self.additional.insert(name.into());
    }
}

#[derive(Default)]
/// 已解析的 CLI 键值与 `-f` 过滤表达式列表。
struct ParsedFlags {
    values: HashMap<String, String>,
    filters: Vec<String>,
}

impl ParsedFlags {
    /// 取非空字符串 flag；空串视为未设置。
    fn string(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// 解析布尔 flag；缺失时返回默认值。
    fn bool(&self, key: &str, default: bool) -> Result<bool, ConfigError> {
        match self.values.get(key) {
            None => Ok(default),
            Some(value) => parse_bool(key, value),
        }
    }

    /// 解析整型 flag；缺失返回 None。
    fn integer(&self, key: &str) -> Result<Option<i32>, ConfigError> {
        self.values
            .get(key)
            .map(|value| parse_i32(key, value))
            .transpose()
    }
}

/// 从命令行参数与可选 TOML 文件加载全局配置。
///
/// `extra_flags` 可在解析前注册额外 flag；`-V`/`-h` 通过 `ConfigError::Help` 表示。
pub fn load_global_config(
    args: &[String],
    extra_flags: Option<fn(&mut FlagSet)>,
) -> Result<GlobalConfig, ConfigError> {
    let mut registered = FlagSet::default();
    if let Some(register) = extra_flags {
        register(&mut registered);
    }
    let flags = parse_flags(args, &registered)?;
    if flags.bool("V", false)? {
        println!("AsterSQL Lightning");
        return Err(ConfigError::Help);
    }

    let mut config = new_global_config();
    if let Some(path) = flags.string("config") {
        let data = std::fs::read(path).map_err(|error| {
            ConfigError::Invalid(format!("cannot read config file {path}: {error}"))
        })?;
        decode_global_toml(&data, &mut config).map_err(|error| {
            ConfigError::Invalid(format!("cannot parse config file {path}: {error}"))
        })?;
        config.config_file_content = data;
    }

    // Command line values are applied after TOML, and zero/empty values do not
    // overwrite file values, exactly as the Go implementation does.
    // 命令行在 TOML 之后覆盖；空值/零值不覆盖文件中已有配置（对齐 Go）。
    if let Some(value) = flags.string("L") {
        config.app.log_config.level = value.into();
    }
    if let Some(value) = flags.string("log-file") {
        config.app.log_config.file = value.into();
    }
    if config.app.log_config.file.is_empty() {
        config.app.log_config.file = timestamp_log_file_name().to_string_lossy().into_owned();
    }
    if let Some(value) = flags.string("tidb-host") {
        config.tidb.host = value.into();
    }
    if let Some(value) = flags.integer("tidb-port")?.filter(|value| *value != 0) {
        config.tidb.port = value;
    }
    if let Some(value) = flags.integer("tidb-status")?.filter(|value| *value != 0) {
        config.tidb.status_port = value;
    }
    if let Some(value) = flags.string("tidb-user") {
        config.tidb.user = value.into();
    }
    if let Some(value) = flags.string("tidb-password") {
        config.tidb.password = value.into();
    }
    if let Some(value) = flags.string("pd-urls") {
        config.tidb.pd_addr = value.into();
    }
    if let Some(value) = flags.string("d") {
        config.mydumper.source_dir = value.into();
    }
    if flags.bool("server-mode", false)? {
        config.app.server_mode = true;
    }
    if let Some(value) = flags.string("status-addr") {
        config.app.status_addr = value.into();
    }
    if let Some(value) = flags.string("backend") {
        config.tikv_importer.backend = value.into();
    }
    if let Some(value) = flags.string("sorted-kv-dir") {
        config.tikv_importer.sorted_kv_dir = value.into();
    }
    if !flags.bool("enable-checkpoint", true)? {
        config.checkpoint.enable = false;
    }
    if flags.bool("no-schema", false)? {
        config.mydumper.no_schema = true;
    }
    if let Some(value) = flags.string("checksum") {
        config.post_restore.checksum.from_string_value(value)?;
    }
    if let Some(value) = flags.string("analyze") {
        config.post_restore.analyze.from_string_value(value)?;
    }
    if config.app.status_addr.is_empty() && config.app.pprof_port != 0 {
        config.app.status_addr = format!(":{}", config.app.pprof_port);
    }
    if !flags.bool("check-requirements", true)? {
        config.app.check_requirements = false;
    }
    if let Some(value) = flags.string("ca") {
        config.security.ca_path = value.into();
    }
    if let Some(value) = flags.string("cert") {
        config.security.cert_path = value.into();
    }
    if let Some(value) = flags.string("key") {
        config.security.key_path = value.into();
    }
    if flags.bool("redact-info-log", false)? {
        config.security.redact_info_log = true;
    }
    if !flags.filters.is_empty() {
        config.mydumper.filter = flags.filters;
    }
    if config.app.status_addr.is_empty() && config.app.server_mode {
        return Err(ConfigError::Invalid(
            "If server-mode is enabled, the status-addr must be a valid listen address".into(),
        ));
    }
    config.app.log_config.adjust();
    Ok(config)
}

/// 解析 CLI 参数：识别已知/额外 flag、布尔默认、枚举取值，以及 `-f` 过滤器。
fn parse_flags(args: &[String], registered: &FlagSet) -> Result<ParsedFlags, ConfigError> {
    let known: HashSet<&str> = [
        "c",
        "config",
        "V",
        "L",
        "log-file",
        "tidb-host",
        "tidb-port",
        "tidb-user",
        "tidb-password",
        "tidb-status",
        "pd-urls",
        "d",
        "backend",
        "sorted-kv-dir",
        "enable-checkpoint",
        "no-schema",
        "checksum",
        "analyze",
        "check-requirements",
        "ca",
        "cert",
        "key",
        "redact-info-log",
        "status-addr",
        "server-mode",
        "f",
    ]
    .into_iter()
    .collect();
    let boolean: HashSet<&str> = [
        "V",
        "enable-checkpoint",
        "no-schema",
        "check-requirements",
        "redact-info-log",
        "server-mode",
    ]
    .into_iter()
    .collect();
    let choices: HashMap<&str, &[&str]> = HashMap::from([
        (
            "L",
            &["info", "debug", "warn", "warning", "error", "fatal"][..],
        ),
        ("backend", &["local", "tidb", "import-into"][..]),
        (
            "checksum",
            &["required", "optional", "off", "true", "false"][..],
        ),
        (
            "analyze",
            &["required", "optional", "off", "true", "false"][..],
        ),
    ]);
    let mut parsed = ParsedFlags::default();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == "-h" || argument == "--help" {
            return Err(ConfigError::Help);
        }
        let Some(trimmed) = argument
            .strip_prefix('-')
            .map(|value| value.trim_start_matches('-'))
        else {
            return Err(ConfigError::Invalid(format!(
                "unexpected argument {argument}"
            )));
        };
        let (raw_key, inline) = trimmed
            .split_once('=')
            .map_or((trimmed, None), |(key, value)| (key, Some(value)));
        if !known.contains(raw_key) && !registered.additional.contains(raw_key) {
            return Err(ConfigError::Invalid(format!(
                "flag provided but not defined: -{raw_key}"
            )));
        }
        let key = if raw_key == "c" { "config" } else { raw_key };
        let value = if let Some(value) = inline {
            value.to_owned()
        } else if boolean.contains(raw_key) {
            "true".into()
        } else {
            index += 1;
            args.get(index).cloned().ok_or_else(|| {
                ConfigError::Invalid(format!("flag needs an argument: -{raw_key}"))
            })?
        };
        if let Some(allowed) = choices.get(raw_key)
            && !value.is_empty()
            && !allowed.contains(&value.as_str())
        {
            return Err(ConfigError::Invalid(format!(
                "invalid value for -{raw_key}: {value}"
            )));
        }
        if raw_key == "f" {
            parsed.filters.push(value);
        } else {
            // Replacing the shared key means the last -c/-config wins.
            // 同一键重复出现时后者覆盖前者（最后一次 -c/-config 生效）。
            parsed.values.insert(key.into(), value);
        }
        index += 1;
    }
    Ok(parsed)
}

/// 从 TOML 文档填充全局配置字段；任务专属键留给后续完整加载。
fn decode_global_toml(data: &[u8], config: &mut GlobalConfig) -> Result<(), ConfigError> {
    let text = std::str::from_utf8(data).map_err(|error| ConfigError::Parse(error.to_string()))?;
    let root: toml::Value =
        toml::from_str(text).map_err(|error| ConfigError::Parse(error.to_string()))?;

    if let Some(section) = root.get("lightning") {
        set_string(section, "status-addr", &mut config.app.status_addr)?;
        set_bool(section, "server-mode", &mut config.app.server_mode)?;
        set_bool(
            section,
            "check-requirements",
            &mut config.app.check_requirements,
        )?;
        set_i32(section, "pprof-port", &mut config.app.pprof_port)?;
        set_string(section, "level", &mut config.app.log_config.level)?;
        set_string(section, "file", &mut config.app.log_config.file)?;
    }
    if let Some(section) = root.get("checkpoint") {
        set_bool(section, "enable", &mut config.checkpoint.enable)?;
    }
    if let Some(section) = root.get("tidb") {
        set_string(section, "host", &mut config.tidb.host)?;
        set_i32(section, "port", &mut config.tidb.port)?;
        set_string(section, "user", &mut config.tidb.user)?;
        set_string(section, "password", &mut config.tidb.password)?;
        set_i32(section, "status-port", &mut config.tidb.status_port)?;
        set_string(section, "pd-addr", &mut config.tidb.pd_addr)?;
        set_string(section, "log-level", &mut config.tidb.log_level)?;
    }
    if let Some(section) = root.get("mydumper") {
        set_string(section, "data-source-dir", &mut config.mydumper.source_dir)?;
        set_bool(section, "no-schema", &mut config.mydumper.no_schema)?;
        if let Some(value) = section.get("filter") {
            config.mydumper.filter = string_array(value, "mydumper.filter")?;
        }
        if let Some(value) = section.get("ignore-columns") {
            let entries = value.as_array().ok_or_else(|| {
                ConfigError::Parse("mydumper.ignore-columns must be an array of tables".into())
            })?;
            config.mydumper.ignore_columns = entries
                .iter()
                .map(decode_ignore_columns)
                .collect::<Result<_, _>>()?;
        }
    }
    if let Some(section) = root.get("tikv-importer") {
        set_string(section, "backend", &mut config.tikv_importer.backend)?;
        set_string(
            section,
            "sorted-kv-dir",
            &mut config.tikv_importer.sorted_kv_dir,
        )?;
    }
    if let Some(section) = root.get("post-restore") {
        if let Some(value) = section.get("checksum") {
            config
                .post_restore
                .checksum
                .from_string_value(post_op_value(value, "post-restore.checksum")?)?;
        }
        if let Some(value) = section.get("analyze") {
            config
                .post_restore
                .analyze
                .from_string_value(post_op_value(value, "post-restore.analyze")?)?;
        }
    }
    if let Some(section) = root.get("security") {
        set_string(section, "ca-path", &mut config.security.ca_path)?;
        set_string(section, "cert-path", &mut config.security.cert_path)?;
        set_string(section, "key-path", &mut config.security.key_path)?;
        set_bool(
            section,
            "redact-info-log",
            &mut config.security.redact_info_log,
        )?;
    }
    Ok(())
}

fn toml_string<'a>(value: &'a toml::Value, path: &str) -> Result<&'a str, ConfigError> {
    value
        .as_str()
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be a string")))
}

fn post_op_value<'a>(value: &'a toml::Value, path: &str) -> Result<&'a str, ConfigError> {
    if let Some(value) = value.as_str() {
        return Ok(value);
    }
    if let Some(value) = value.as_bool() {
        return Ok(if value { "true" } else { "false" });
    }
    Err(ConfigError::Parse(format!(
        "{path} must be a string or boolean"
    )))
}

fn set_string(section: &toml::Value, key: &str, target: &mut String) -> Result<(), ConfigError> {
    if let Some(value) = section.get(key) {
        *target = toml_string(value, key)?.to_owned();
    }
    Ok(())
}

fn set_bool(section: &toml::Value, key: &str, target: &mut bool) -> Result<(), ConfigError> {
    if let Some(value) = section.get(key) {
        *target = value
            .as_bool()
            .ok_or_else(|| ConfigError::Parse(format!("{key} must be a boolean")))?;
    }
    Ok(())
}

fn set_i32(section: &toml::Value, key: &str, target: &mut i32) -> Result<(), ConfigError> {
    if let Some(value) = section.get(key) {
        let integer = value
            .as_integer()
            .ok_or_else(|| ConfigError::Parse(format!("{key} must be an integer")))?;
        *target = i32::try_from(integer)
            .map_err(|_| ConfigError::Parse(format!("{key} must be an integer")))?;
    }
    Ok(())
}

fn string_array(value: &toml::Value, path: &str) -> Result<Vec<String>, ConfigError> {
    value
        .as_array()
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be an array")))?
        .iter()
        .map(|entry| Ok(toml_string(entry, path)?.to_owned()))
        .collect()
}

fn decode_ignore_columns(value: &toml::Value) -> Result<IgnoreColumns, ConfigError> {
    let table = value
        .as_table()
        .ok_or_else(|| ConfigError::Parse("mydumper.ignore-columns must contain tables".into()))?;
    let mut result = IgnoreColumns::default();
    if let Some(value) = table.get("db") {
        result.db = toml_string(value, "mydumper.ignore-columns.db")?.to_owned();
    }
    if let Some(value) = table.get("table") {
        result.table = toml_string(value, "mydumper.ignore-columns.table")?.to_owned();
    }
    if let Some(value) = table.get("table-filter") {
        result.table_filter = string_array(value, "mydumper.ignore-columns.table-filter")?;
    }
    if let Some(value) = table.get("columns") {
        result.columns = string_array(value, "mydumper.ignore-columns.columns")?;
    }
    Ok(result)
}
