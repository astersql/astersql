// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Import SDK 配置类型与函数式选项（functional options）。
//
// 调用方通过 `WithXxx` 闭包原地修改 [`SDKConfig`]，控制并发、SQL mode、
// 文件/表路由、过滤表达式、字符集、CSV 解析参数以及扫描上限等。
// 默认值对齐 Lightning loader，并排除系统 schema。

use astersql_lightning_log as log;
use astersql_lightning_mydump as mydump;
use astersql_parser_mysql as mysql;
/// 表路由规则列表（schema/table 模式匹配到目标名）。
pub type Routes = Vec<TableRouteRule>;
/// 文件路由规则，复用 mydump 定义。
pub type FileRouteRule = mydump::FileRouteRule;

/// 将源 schema/table 模式重写到目标库表名的路由规则。
#[derive(Clone, Debug, Default)]
pub struct TableRouteRule {
    /// 源库名匹配模式。
    pub SchemaPattern: String,
    /// 源表名匹配模式。
    pub TablePattern: String,
    /// 重写后的目标库名。
    pub TargetSchema: String,
    /// 重写后的目标表名。
    pub TargetTable: String,
}

/// CSV 解析与大小估算所用的字段/行分隔、转义与 NULL 约定。
#[derive(Clone, Debug, Default)]
pub struct CSVConfig {
    /// 字段分隔符。
    pub FieldsTerminatedBy: String,
    /// 字段包围符（引号）。
    pub FieldsEnclosedBy: String,
    /// 行结束符。
    pub LinesTerminatedBy: String,
    /// 表示 SQL NULL 的字段字面量列表。
    pub FieldNullDefinedBy: Vec<String>,
    /// 是否将首行视为表头。
    pub Header: bool,
    /// 表头是否须与 schema 列名匹配。
    pub HeaderSchemaMatch: bool,
    /// 是否去掉行末空字段。
    pub TrimLastEmptyField: bool,
    /// 是否禁止 NULL（未匹配 null 字面量时按文本）。
    pub NotNull: bool,
    /// 是否启用反斜杠转义。
    pub BackslashEscape: bool,
    /// 字段转义字符。
    pub FieldsEscapedBy: String,
    /// 行起始前缀。
    pub LinesStartingBy: String,
    /// 是否允许空行。
    pub AllowEmptyLine: bool,
    /// 引号包围的 null 字面量是否当普通文本。
    pub QuotedNullIsText: bool,
    /// 是否容忍未转义的引号。
    pub UnescapedQuote: bool,
}

// SDKOption 对应 Go 的 func(*SDKConfig)，调用时原地修改配置。
/// 函数式选项：接收并原地修改 [`SDKConfig`]。
pub type SDKOption = Box<dyn FnOnce(&mut SDKConfig) + Send>;

// SDKConfig 对应 SDK 内部配置；字段顺序保持 loader 选项在前、通用选项在后。
/// SDK 内部配置：loader 选项在前，通用选项（日志）在后。
#[derive(Clone)]
pub struct SDKConfig {
    // Loader options
    /// 并发创建库表的 worker 数。
    pub(crate) concurrency: i32,
    /// 解析 schema 时采用的 SQL mode。
    pub(crate) sql_mode: mysql::r#const::SQLMode,
    /// 文件路径到库表的路由规则。
    pub(crate) file_route_rules: Vec<FileRouteRule>,
    /// schema/table 重写路由。
    pub(crate) routes: Routes,
    /// Lightning 风格的库表过滤表达式。
    pub(crate) filter: Vec<String>,
    /// dump 文件字符集（auto 表示自动探测）。
    pub(crate) charset: String,
    /// CSV 解析参数（用于真实大小估算等）。
    pub(crate) csv_config: CSVConfig,
    /// CSV 源数据字符集。
    pub(crate) data_character_set: String,
    /// 扫描文件数量上限；None 表示不限制。
    pub(crate) max_scan_files: Option<i32>,
    /// 遇无效文件时跳过表而非向调用方报错。
    pub(crate) skip_invalid_files: bool,
    /// 是否为压缩/Parquet 估算解压或行式后的真实大小。
    pub(crate) estimate_real_size: bool,

    // General options
    /// SDK 使用的日志器。
    pub(crate) logger: log::Logger,
}

// defaultSDKConfig 使用 Lightning 默认值构造 SDK 配置。
/// 使用 Lightning 默认值构造 SDK 配置（并发 4、过滤系统库、utf8mb4 等）。
pub fn defaultSDKConfig() -> SDKConfig {
    SDKConfig {
        concurrency: 4,
        sql_mode: mysql::r#const::SQLMode::default(),
        file_route_rules: Vec::new(),
        routes: Routes::default(),
        // 默认包含所有用户表，并排除 mysql/sys 等系统 schema。
        filter: [
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
        .collect(),
        logger: log::L(),
        charset: "auto".to_owned(),
        csv_config: CSVConfig {
            FieldsTerminatedBy: ",".to_owned(),
            FieldsEnclosedBy: "\"".to_owned(),
            LinesTerminatedBy: String::new(),
            FieldNullDefinedBy: vec![r"\N".to_owned()],
            Header: true,
            HeaderSchemaMatch: true,
            BackslashEscape: true,
            FieldsEscapedBy: "\\".to_owned(),
            ..Default::default()
        },
        data_character_set: "binary".to_owned(),
        max_scan_files: None,
        skip_invalid_files: false,
        // 压缩文件和 Parquet 默认采样估算解压/行式后的真实大小。
        estimate_real_size: true,
    }
}

// WithConcurrency 设置并发创建数据库/表的 worker 数；非正数沿用原配置。
/// 设置并发创建数据库/表的 worker 数；非正数沿用原配置。
pub fn WithConcurrency(n: i32) -> SDKOption {
    Box::new(move |cfg| {
        if n > 0 {
            cfg.concurrency = n;
        }
    })
}

// WithLogger 注入调用方提供的日志器。
/// 注入调用方提供的日志器。
pub fn WithLogger(logger: log::Logger) -> SDKOption {
    Box::new(move |cfg| cfg.logger = logger)
}

// WithSQLMode 设置解析 schema 时采用的 SQL mode。
/// 设置解析 schema 时采用的 SQL mode。
pub fn WithSQLMode(mode: mysql::r#const::SQLMode) -> SDKOption {
    Box::new(move |cfg| cfg.sql_mode = mode)
}

// WithFilter 设置 loader 的文件过滤表达式。
/// 设置 loader 的文件过滤表达式。
pub fn WithFilter(filter: Vec<String>) -> SDKOption {
    Box::new(move |cfg| cfg.filter = filter)
}

// WithFileRouters 设置文件路由规则。
/// 设置文件路由规则。
pub fn WithFileRouters(rules: Vec<FileRouteRule>) -> SDKOption {
    Box::new(move |cfg| cfg.file_route_rules = rules)
}

// WithRoutes 设置表路由规则。
/// 设置表路由规则。
pub fn WithRoutes(routes: Routes) -> SDKOption {
    Box::new(move |cfg| cfg.routes = routes)
}

// WithCharset 设置导入字符集；空字符串不覆盖默认 auto。
/// 设置导入字符集；空字符串不覆盖默认 auto。
pub fn WithCharset(charset: String) -> SDKOption {
    Box::new(move |cfg| {
        if !charset.is_empty() {
            cfg.charset = charset;
        }
    })
}

// WithCSVConfig 设置真实大小估算所使用的 CSV 解析参数。
/// 设置真实大小估算所使用的 CSV 解析参数。
pub fn WithCSVConfig(csv_config: CSVConfig) -> SDKOption {
    Box::new(move |cfg| cfg.csv_config = csv_config)
}

// WithDataCharacterSet 设置 CSV 源数据字符集；空字符串保持当前配置。
/// 设置 CSV 源数据字符集；空字符串保持当前配置。
pub fn WithDataCharacterSet(charset: String) -> SDKOption {
    Box::new(move |cfg| {
        if !charset.is_empty() {
            cfg.data_character_set = charset;
        }
    })
}

// WithMaxScanFiles 设置扫描文件上限；非正值不创建限制。
/// 设置扫描文件上限；非正值不创建限制。
pub fn WithMaxScanFiles(limit: i32) -> SDKOption {
    Box::new(move |cfg| {
        if limit > 0 {
            cfg.max_scan_files = Some(limit);
        }
    })
}

// WithEstimateRealSize 控制是否为压缩和 Parquet 文件估算真实导入大小。
/// 控制是否为压缩和 Parquet 文件估算真实导入大小。
pub fn WithEstimateRealSize(estimate: bool) -> SDKOption {
    Box::new(move |cfg| cfg.estimate_real_size = estimate)
}

// WithSkipInvalidFiles 控制遇到无效文件时是跳过表还是向调用方返回错误。
/// 控制遇到无效文件时是跳过表还是向调用方返回错误。
pub fn WithSkipInvalidFiles(skip: bool) -> SDKOption {
    Box::new(move |cfg| cfg.skip_invalid_files = skip)
}
