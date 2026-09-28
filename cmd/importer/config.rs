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

//! Config / flag / TOML parsing matching `cmd/importer/config.go`.
//! 这个模块把 Go 版 importer 的三层配置入口保留下来，
//! 即默认值、TOML 文件覆盖以及命令行最终覆盖。
//! 之所以仍然采用“两次解析”的老式流程，是为了先拿到 `-config`
//! 指向的文件，再让显式 CLI 参数重新盖掉文件里的值，
//! 从而保持和 Go `flag.FlagSet` 一致的优先级语义。

use std::fs;

use crate::stubs::{Error, Result};

/// NewConfig creates a new config.
/// Rust 版没有直接暴露 Go 的 `FlagSet` 对象，
/// 因此把默认 flag 注册结果折叠进 `default_with_flags`，
/// 让调用方拿到的就是一份可立即参与解析的基线配置。
pub fn NewConfig() -> Config {
    Config::default_with_flags()
}

/// DBConfig is the DB configuration.
/// 这一组字段对应 importer 建立目标 TiDB/MySQL 连接所需的
/// 五元组；默认值、TOML 键名和 CLI 短参数都尽量保持与 Go 对齐。
#[derive(Clone, Debug)]
pub struct DBConfig {
    /// 默认回环地址，匹配 Go 中本地开发场景的初始值。
    pub Host: String,
    /// 默认用户仍是 `root`，便于和历史脚本直接兼容。
    pub User: String,
    /// 密码默认留空，让部署脚本自行决定是否通过参数注入。
    pub Password: String,
    /// 数据库名默认 `test`，和 Go 侧原始 importer 行为一致。
    pub Name: String,
    /// 保留经典 MySQL 端口默认值，避免未显式指定时漂移。
    pub Port: isize,
}

impl Default for DBConfig {
    fn default() -> Self {
        Self {
            Host: "127.0.0.1".into(),
            User: "root".into(),
            Password: String::new(),
            Name: "test".into(),
            Port: 3306,
        }
    }
}

impl DBConfig {
    /// 字符串化逻辑刻意保留 `<nil>` 分支，
    /// 方便和 Go 的 `fmt.Stringer` 输出做对照测试。
    pub fn String(c: Option<&DBConfig>) -> String {
        match c {
            None => "<nil>".into(),
            Some(cfg) => format!(
                "DBConfig({{Host:{} User:{} Password:{} Name:{} Port:{}}})",
                cfg.Host, cfg.User, cfg.Password, cfg.Name, cfg.Port
            ),
        }
    }
}

impl std::fmt::Display for DBConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Self::String(Some(self)))
    }
}

/// DDLConfig is the configuration for ddl statements.
/// 导入器允许直接从参数或配置文件注入建表、建索引 SQL，
/// 因此这里单独拆成子结构，便于和数据连接参数解耦。
#[derive(Clone, Debug, Default)]
pub struct DDLConfig {
    /// 表结构 SQL 往往比其他字段更长，单独保留便于覆盖来源。
    pub TableSQL: String,
    /// 索引 SQL 与建表 SQL 分离，贴合 Go 原始配置布局。
    pub IndexSQL: String,
}

/// SysConfig is the configuration for job/worker count, batch size, etc.
/// 系统配置控制 importer 的并行度和批量提交粒度，
/// 这些值既影响吞吐，也会影响 TiDB 侧事务压力。
#[derive(Clone, Debug)]
pub struct SysConfig {
    /// 日志级别保留字符串形式，和 CLI/TOML 文案直接一致。
    pub LogLevel: String,
    /// worker 数决定同时执行的工作协程数量，对齐 Go 默认值 2。
    pub WorkerCount: isize,
    /// job 总数用于拆分导入任务，默认给出较大的历史值。
    pub JobCount: isize,
    /// batch 表示一次提交包含的记录批次，直接影响事务大小。
    pub Batch: isize,
}

impl Default for SysConfig {
    fn default() -> Self {
        Self {
            LogLevel: "info".into(),
            WorkerCount: 2,
            JobCount: 10000,
            Batch: 1000,
        }
    }
}

/// StatsConfig is the configuration for statistics file.
/// 统计信息文件是可选输入；留空表示不主动加载 stats。
#[derive(Clone, Debug, Default)]
pub struct StatsConfig {
    pub Path: String,
}

/// Config is the configuration.
/// 顶层配置把数据库、DDL、统计信息和系统参数聚合在一起，
/// 并额外保存解析过程需要的内部状态。
#[derive(Clone, Debug)]
pub struct Config {
    /// 数据库连接参数。
    pub DBCfg: DBConfig,
    /// 建表/建索引 SQL 所在配置段。
    pub DDLCfg: DDLConfig,
    /// 统计信息文件路径配置段。
    pub StatsCfg: StatsConfig,
    /// 并发度、批量大小和日志级别配置段。
    pub SysCfg: SysConfig,
    /// 第一轮解析时先提取该路径，随后读取 TOML 文件。
    pub configFile: String,
    /// Leftover positional args after flag parse (Go `FlagSet.Args`).
    /// Go 会把无法继续作为 flag 解析的尾随参数留下来，
    /// Rust 版也显式保留，最后统一报成“无效 flag”。
    pub args: Vec<String>,
    /// 帮助标志在 Go 中会触发专门的错误路径，
    /// 这里单独记下来，便于外围入口按帮助场景处理。
    help_requested: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self::default_with_flags()
    }
}

impl Config {
    fn default_with_flags() -> Self {
        // 这里直接把 Go `FlagSet` 注册出来的默认值固化进结构体，
        // 这样既避免维护一套运行时 flag 注册器，也保留相同的初始配置。
        Self {
            DBCfg: DBConfig::default(),
            DDLCfg: DDLConfig::default(),
            StatsCfg: StatsConfig::default(),
            SysCfg: SysConfig::default(),
            configFile: String::new(),
            args: Vec::new(),
            help_requested: false,
        }
    }

    /// Parse parses flag definitions from the argument list.
    pub fn Parse(&mut self, arguments: &[String]) -> Result<()> {
        // 第一轮只负责提取 `-config` 以及捕获显式 CLI 值，
        // 其结果会决定是否需要加载外部 TOML。
        // Parse first to get config file.
        self.parse_flags(arguments)?;

        // 文件配置处在中间层优先级；
        // 它可以覆盖默认值，但仍然会被第二轮命令行参数盖掉。
        // Load config file if specified.
        if !self.configFile.is_empty() {
            let path = self.configFile.clone();
            self.configFromFile(&path)?;
        }

        // 第二轮重新走同一套 flag 解析逻辑，
        // 保证显式传入的参数总是拥有最高优先级。
        // Parse again to replace with command line options.
        self.parse_flags(arguments)?;

        // 一旦出现剩余位置参数，就说明有内容没有被定义成合法 flag；
        // 这里沿用 Go 的报错口径，把第一个残留参数作为证据返回。
        if !self.args.is_empty() {
            return Err(Error::new(format!("'{}' is an invalid flag", self.args[0])));
        }

        Ok(())
    }

    pub fn String(c: Option<&Config>) -> String {
        match c {
            None => "<nil>".into(),
            Some(cfg) => format!(
                "Config({{FlagSet:{:p} DBCfg:{{Host:{} User:{} Password:{} Name:{} Port:{}}} \
                 DDLCfg:{{TableSQL:{} IndexSQL:{}}} StatsCfg:{{Path:{}}} \
                 SysCfg:{{LogLevel:{} WorkerCount:{} JobCount:{} Batch:{}}} configFile:{}}})",
                cfg as *const Config,
                cfg.DBCfg.Host,
                cfg.DBCfg.User,
                cfg.DBCfg.Password,
                cfg.DBCfg.Name,
                cfg.DBCfg.Port,
                cfg.DDLCfg.TableSQL,
                cfg.DDLCfg.IndexSQL,
                cfg.StatsCfg.Path,
                cfg.SysCfg.LogLevel,
                cfg.SysCfg.WorkerCount,
                cfg.SysCfg.JobCount,
                cfg.SysCfg.Batch,
                cfg.configFile,
            ),
        }
    }

    /// 使用完整 TOML 解析器承载转义、注释、进制整数和表结构，
    /// 再把已知字段覆盖到现有默认值上，以复刻 Go `DecodeFile` 的更新语义。
    fn configFromFile(&mut self, path: &str) -> Result<()> {
        let text = fs::read_to_string(path).map_err(|e| Error::new(e.to_string()))?;
        let root = text
            .parse::<toml::Table>()
            .map_err(|err| Error::new(err.to_string()))?;

        if let Some(db) = toml_section(&root, "db")? {
            apply_toml_string(db, "host", "db.host", &mut self.DBCfg.Host)?;
            apply_toml_string(db, "user", "db.user", &mut self.DBCfg.User)?;
            apply_toml_string(db, "password", "db.password", &mut self.DBCfg.Password)?;
            apply_toml_string(db, "name", "db.name", &mut self.DBCfg.Name)?;
            apply_toml_int(db, "port", "db.port", &mut self.DBCfg.Port)?;
        }
        if let Some(ddl) = toml_section(&root, "ddl")? {
            apply_toml_string(ddl, "table-sql", "ddl.table-sql", &mut self.DDLCfg.TableSQL)?;
            apply_toml_string(ddl, "index-sql", "ddl.index-sql", &mut self.DDLCfg.IndexSQL)?;
        }
        if let Some(stats) = toml_section(&root, "stats")? {
            apply_toml_string(
                stats,
                "stats-file-path",
                "stats.stats-file-path",
                &mut self.StatsCfg.Path,
            )?;
        }
        if let Some(sys) = toml_section(&root, "sys")? {
            apply_toml_string(sys, "log-level", "sys.log-level", &mut self.SysCfg.LogLevel)?;
            apply_toml_int(
                sys,
                "worker-count",
                "sys.worker-count",
                &mut self.SysCfg.WorkerCount,
            )?;
            apply_toml_int(sys, "job-count", "sys.job-count", &mut self.SysCfg.JobCount)?;
            apply_toml_int(sys, "batch", "sys.batch", &mut self.SysCfg.Batch)?;
        }
        Ok(())
    }

    fn parse_flags(&mut self, arguments: &[String]) -> Result<()> {
        // 每次解析前都清空残留参数和帮助标记，
        // 避免第一轮状态泄漏到第二轮，破坏“文件配置再被 CLI 覆盖”的流程。
        self.args.clear();
        self.help_requested = false;
        let mut i = 0;
        while i < arguments.len() {
            let a = &arguments[i];
            if a == "--" {
                // `--` 之后的内容不再尝试解释成 flag，
                // 这与 Go flag 包停止解析的行为保持一致。
                self.args.extend(arguments[i + 1..].iter().cloned());
                break;
            }
            if a == "-help" || a == "--help" {
                // 帮助请求不是普通业务错误；
                // 这里通过专门的 help 错误把控制权交回上层。
                self.help_requested = true;
                return Err(Error::help());
            }
            if a == "-" || !a.starts_with('-') {
                // 第一个裸参数出现后，其余参数全部视为位置参数，
                // 保留给最终的“invalid flag”检查统一处理。
                self.args.extend(arguments[i..].iter().cloned());
                break;
            }
            let (key, inline_val) = split_flag(a)?;
            if key == "help" {
                self.help_requested = true;
                return Err(Error::help());
            }
            match key {
                "config" => {
                    // 配置文件路径必须优先保存下来，
                    // 这样 `Parse` 才能在两轮解析之间插入文件加载。
                    self.configFile = take_val(inline_val, arguments, &mut i, key)?;
                }
                "t" => {
                    // DDL 相关短参数沿用 Go 版单字符旗标，兼容旧脚本。
                    self.DDLCfg.TableSQL = take_val(inline_val, arguments, &mut i, key)?;
                }
                "i" => {
                    self.DDLCfg.IndexSQL = take_val(inline_val, arguments, &mut i, key)?;
                }
                "s" => {
                    // stats 文件路径是可选增强项，不影响其他配置段。
                    self.StatsCfg.Path = take_val(inline_val, arguments, &mut i, key)?;
                }
                "c" => {
                    // 数值型参数统一在这里立即解析，
                    // 让错误尽早暴露，避免带着字符串进入后续流程。
                    self.SysCfg.WorkerCount = take_int_val(inline_val, arguments, &mut i, key)?;
                }
                "n" => {
                    self.SysCfg.JobCount = take_int_val(inline_val, arguments, &mut i, key)?;
                }
                "b" => {
                    self.SysCfg.Batch = take_int_val(inline_val, arguments, &mut i, key)?;
                }
                "h" => {
                    self.DBCfg.Host = take_val(inline_val, arguments, &mut i, key)?;
                }
                "u" => {
                    self.DBCfg.User = take_val(inline_val, arguments, &mut i, key)?;
                }
                "p" => {
                    self.DBCfg.Password = take_val(inline_val, arguments, &mut i, key)?;
                }
                "D" => {
                    self.DBCfg.Name = take_val(inline_val, arguments, &mut i, key)?;
                }
                "P" => {
                    self.DBCfg.Port = take_int_val(inline_val, arguments, &mut i, key)?;
                }
                "L" => {
                    self.SysCfg.LogLevel = take_val(inline_val, arguments, &mut i, key)?;
                }
                _ => {
                    // 未知旗标立即失败，保持与 Go `flag` 默认策略一致，
                    // 不做宽松忽略，避免导入任务默默使用错误配置。
                    return Err(Error::new(format!("flag provided but not defined: -{key}")));
                }
            }
            i += 1;
        }
        Ok(())
    }
}

fn split_flag(a: &str) -> Result<(&str, Option<&str>)> {
    // 同时支持 `-k=v` 与 `--k=v`，但只剥掉一到两个前导 `-`；
    // 三个及以上前导横线必须像 Go `flag` 一样报坏语法。
    let s = a
        .strip_prefix("--")
        .or_else(|| a.strip_prefix('-'))
        .unwrap_or(a);
    if s.is_empty() || s.starts_with('-') || s.starts_with('=') {
        return Err(Error::new(format!("bad flag syntax: {a}")));
    }
    if let Some((k, v)) = s.split_once('=') {
        Ok((k, Some(v)))
    } else {
        Ok((s, None))
    }
}

fn take_val(
    inline: Option<&str>,
    arguments: &[String],
    i: &mut usize,
    key: &str,
) -> Result<String> {
    // 优先消费 `-k=v` 的内联值，避免错误前移索引。
    if let Some(v) = inline {
        return Ok(v.to_string());
    }
    // 若缺少后继参数，则和 Go 一样报“flag needs an argument”，
    // 让上层能够区分“未知旗标”和“值缺失”两类输入错误。
    if *i + 1 >= arguments.len() {
        return Err(Error::new(format!("flag needs an argument: -{key}")));
    }
    *i += 1;
    Ok(arguments[*i].clone())
}

fn take_int_val(
    inline: Option<&str>,
    arguments: &[String],
    i: &mut usize,
    key: &str,
) -> Result<isize> {
    let value = take_val(inline, arguments, i, key)?;
    parse_go_int(&value).map_err(|kind| {
        Error::new(format!(
            "invalid value {value:?} for flag -{key}: {}",
            match kind {
                IntParseError::Syntax => "parse error",
                IntParseError::Range => "value out of range",
            }
        ))
    })
}

enum IntParseError {
    Syntax,
    Range,
}

/// Match `strconv.ParseInt(value, 0, strconv.IntSize)`, including Go literal
/// prefixes and underscore placement.
fn parse_go_int(value: &str) -> std::result::Result<isize, IntParseError> {
    let (negative, unsigned) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = value.strip_prefix('+') {
        (false, rest)
    } else {
        (false, value)
    };
    if unsigned.is_empty() {
        return Err(IntParseError::Syntax);
    }

    let (radix, digits, has_base_prefix) = if let Some(rest) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        (16, rest, true)
    } else if let Some(rest) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        (2, rest, true)
    } else if let Some(rest) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        (8, rest, true)
    } else if unsigned.len() > 1 && unsigned.starts_with('0') {
        (8, unsigned, false)
    } else {
        (10, unsigned, false)
    };

    let chars: Vec<char> = digits.chars().collect();
    let mut normalized = String::with_capacity(digits.len());
    let mut saw_digit = false;
    let mut previous_was_digit = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_' {
            let allowed_after_prefix = has_base_prefix && index == 0;
            let next_is_digit = chars
                .get(index + 1)
                .and_then(|next| next.to_digit(radix))
                .is_some();
            if (!previous_was_digit && !allowed_after_prefix) || !next_is_digit {
                return Err(IntParseError::Syntax);
            }
            previous_was_digit = false;
            continue;
        }
        if ch.to_digit(radix).is_none() {
            return Err(IntParseError::Syntax);
        }
        normalized.push(ch);
        saw_digit = true;
        previous_was_digit = true;
    }
    if !saw_digit || !previous_was_digit {
        return Err(IntParseError::Syntax);
    }

    let magnitude = u128::from_str_radix(&normalized, radix).map_err(|_| IntParseError::Range)?;
    let signed = if negative {
        i128::try_from(magnitude)
            .ok()
            .and_then(i128::checked_neg)
            .ok_or(IntParseError::Range)?
    } else {
        i128::try_from(magnitude).map_err(|_| IntParseError::Range)?
    };
    isize::try_from(signed).map_err(|_| IntParseError::Range)
}

fn toml_section<'a>(root: &'a toml::Table, name: &str) -> Result<Option<&'a toml::Table>> {
    match root.get(name) {
        None => Ok(None),
        Some(toml::Value::Table(table)) => Ok(Some(table)),
        Some(value) => Err(toml_type_error(name, "table", value)),
    }
}

fn apply_toml_string(
    table: &toml::Table,
    key: &str,
    path: &str,
    target: &mut String,
) -> Result<()> {
    if let Some(value) = table.get(key) {
        *target = value
            .as_str()
            .ok_or_else(|| toml_type_error(path, "string", value))?
            .to_string();
    }
    Ok(())
}

fn apply_toml_int(table: &toml::Table, key: &str, path: &str, target: &mut isize) -> Result<()> {
    if let Some(value) = table.get(key) {
        let integer = value
            .as_integer()
            .ok_or_else(|| toml_type_error(path, "integer", value))?;
        *target = isize::try_from(integer)
            .map_err(|_| Error::new(format!("TOML value for {path} is out of range")))?;
    }
    Ok(())
}

fn toml_type_error(path: &str, expected: &str, value: &toml::Value) -> Error {
    Error::new(format!(
        "TOML field {path} must be {expected}, got {}",
        value.type_str()
    ))
}
