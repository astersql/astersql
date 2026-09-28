// Copyright 2023 PingCAP, Inc.
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

// Copyright 2026 AsterSQL.

// mydump 文件路径路由：按规则识别库表、类型与压缩后缀。
//
// 将 dump 目录中的文件名匹配为 schema/table/view/sql/csv/parquet 等
// SourceType，并提取 schema、table、分片 key 与整体压缩格式。支持默认
// mydumper 命名规则与自定义 `[[mydumper.files]]` 正则/常量 path 规则。
use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};
use std::sync::{Arc, Mutex};

use crate::MydumpError as Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 对象存储侧支持的压缩类型（gzip/snappy/zstd/无压缩）。
pub enum CompressType {
    Gzip,
    Snappy,
    Zstd,
    NoCompression,
}

#[derive(Clone, Debug, Default)]
/// 路由过程收集告警/错误消息的简易日志器。
pub struct Logger {
    messages: Arc<Mutex<Vec<String>>>,
}
impl Logger {
    /// 记录告警消息。
    fn warn(&self, message: &str) {
        self.messages.lock().unwrap().push(message.to_owned());
    }

    /// 记录错误消息。
    pub fn error(&self, message: impl Into<String>) {
        self.messages.lock().unwrap().push(message.into());
    }

    /// 返回已记录的全部消息副本。
    pub fn messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().clone()
    }
}

#[derive(Clone, Debug, Default)]
/// 单条文件路由规则（对应配置 [[mydumper.files]]）。
pub struct FileRouteRule {
    /// 常量路径（与 pattern 互斥）；非空时转义为正则。
    pub path: String,
    /// 匹配路径的正则。
    pub pattern: String,
    /// schema 提取模板（如 `$1` / `$schema`）。
    pub schema: String,
    /// table 提取模板。
    pub table: String,
    /// 来源类型模板或字面量（sql/csv/...）。
    pub type_name: String,
    /// 分片序号等 key 模板。
    pub key: String,
    /// 压缩后缀模板。
    pub compression: String,
    /// 是否对 schema/table 做 URL path unescape。
    pub unescape: bool,
}
impl FileRouteRule {
    /// 构造仅含 pattern/schema/table/type 的规则。
    fn new(pattern: &str, schema: &str, table: &str, type_name: &str) -> Self {
        Self {
            pattern: pattern.into(),
            schema: schema.into(),
            table: table.into(),
            type_name: type_name.into(),
            ..Default::default()
        }
    }
}

/// 包装为 Routing 错误。
fn errorf(message: String) -> Error {
    Error::Routing(message)
}
/// 由字符串构造 Routing 错误。
fn error_new(message: &str) -> Error {
    Error::Routing(message.into())
}

// SourceType 对应来源文件用途；Ignore 同时作为未知/不匹配类型的默认值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
/// 来源文件用途分类。
pub enum SourceType {
    Ignore = 0,
    SchemaSchema,
    TableSchema,
    Sql,
    Csv,
    Parquet,
    ViewSchema,
}
impl Default for SourceType {
    fn default() -> Self {
        Self::Ignore
    }
}

/// 库级 schema-create 类型名。
pub const SCHEMA_SCHEMA: &str = "schema-schema";
/// 表结构 schema 类型名。
pub const TABLE_SCHEMA: &str = "table-schema";
/// 视图 schema 类型名。
pub const VIEW_SCHEMA: &str = "view-schema";
/// SQL 数据文件类型名。
pub const TYPE_SQL: &str = "sql";
/// CSV 数据文件类型名。
pub const TYPE_CSV: &str = "csv";
/// Parquet 数据文件类型名。
pub const TYPE_PARQUET: &str = "parquet";
/// 忽略类型名。
pub const TYPE_IGNORE: &str = "ignore";

// Compression 对应文件整体压缩后缀；parquet 内部 codec 不属于这里的整体压缩。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
/// 文件整体压缩格式（后缀级；不含 parquet 内部 codec）。
pub enum Compression {
    None = 0,
    Gz,
    Lz4,
    Zstd,
    Xz,
    Lzo,
    Snappy,
}
impl Default for Compression {
    fn default() -> Self {
        Self::None
    }
}

// to_storage_compress_type 仅映射 objstore 当前支持的 gzip/snappy/zstd/none。
/// 将 Compression 映射为存储层 CompressType。
pub fn to_storage_compress_type(compression: Compression) -> Result<CompressType, Error> {
    match compression {
        Compression::Gz => Ok(CompressType::Gzip),
        Compression::Snappy => Ok(CompressType::Snappy),
        Compression::Zstd => Ok(CompressType::Zstd),
        Compression::None => Ok(CompressType::NoCompression),
        other => Err(errorf(format!(
            "compression {} doesn't have related storage compressType",
            other as i32
        ))),
    }
}
/// PascalCase 别名。
pub fn ToStorageCompressType(compression: Compression) -> Result<CompressType, Error> {
    to_storage_compress_type(compression)
}

/// 解析来源类型字符串。
fn parse_source_type(value: &str) -> Result<SourceType, Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        SCHEMA_SCHEMA => Ok(SourceType::SchemaSchema),
        TABLE_SCHEMA => Ok(SourceType::TableSchema),
        TYPE_SQL => Ok(SourceType::Sql),
        TYPE_CSV => Ok(SourceType::Csv),
        TYPE_PARQUET => Ok(SourceType::Parquet),
        TYPE_IGNORE => Ok(SourceType::Ignore),
        VIEW_SCHEMA => Ok(SourceType::ViewSchema),
        _ => Err(errorf(format!("unknown source type '{value}'"))),
    }
}

impl SourceType {
    /// 返回稳定的类型字符串表示。
    pub fn as_str(self) -> &'static str {
        match self {
            SourceType::SchemaSchema => SCHEMA_SCHEMA,
            SourceType::TableSchema => TABLE_SCHEMA,
            SourceType::Csv => TYPE_CSV,
            SourceType::Sql => TYPE_SQL,
            SourceType::Parquet => TYPE_PARQUET,
            SourceType::ViewSchema => VIEW_SCHEMA,
            SourceType::Ignore => TYPE_IGNORE,
        }
    }
}
impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// parse_compression_on_file_extension 只查看最后一个扩展名；未知后缀按未压缩处理。
/// 按文件名最后一段扩展名推断压缩类型；未知则视为未压缩。
pub fn parse_compression_on_file_extension(filename: &str) -> Compression {
    filename
        .rsplit_once('.')
        .and_then(|(_, extension)| parse_compression_type(extension).ok())
        .unwrap_or(Compression::None)
}
/// PascalCase 别名。
pub fn ParseCompressionOnFileExtension(filename: &str) -> Compression {
    parse_compression_on_file_extension(filename)
}

/// 解析压缩类型字符串。
fn parse_compression_type(value: &str) -> Result<Compression, Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        "gz" | "gzip" => Ok(Compression::Gz),
        "lz4" => Ok(Compression::Lz4),
        "zstd" | "zst" => Ok(Compression::Zstd),
        "xz" => Ok(Compression::Xz),
        "lzo" => Ok(Compression::Lzo),
        "snappy" => Ok(Compression::Snappy),
        "" => Ok(Compression::None),
        _ => Err(errorf(format!("invalid compression type '{value}'"))),
    }
}

// default_file_route_rules 对应 Go 默认规则，先忽略 trigger/post 与备份，再识别 schema、view 和数据文件。
/// 返回与 Go 对齐的默认 mydumper 文件命名路由规则列表。
pub fn default_file_route_rules() -> Vec<FileRouteRule> {
    vec![
        FileRouteRule::new(
            r"(?i).*(-schema-trigger|-schema-post)\.sql(?:\.(\w*?))?$",
            "",
            "",
            TYPE_IGNORE,
        ),
        FileRouteRule::new(
            r"(?i).*\.(sql|csv|parquet)(\.(\w+))?\.(bak|BAK)$",
            "",
            "",
            TYPE_IGNORE,
        ),
        FileRouteRule {
            pattern: r"(?i)^(?:[^/]*/)*([^/.]+)-schema-create\.sql(?:\.(\w*?))?$".into(),
            schema: "$1".into(),
            type_name: SCHEMA_SCHEMA.into(),
            compression: "$2".into(),
            unescape: true,
            ..FileRouteRule::default()
        },
        FileRouteRule {
            pattern: r"(?i)^(?:[^/]*/)*([^/.]+)\.(.*?)-schema\.sql(?:\.(\w*?))?$".into(),
            schema: "$1".into(),
            table: "$2".into(),
            type_name: TABLE_SCHEMA.into(),
            compression: "$3".into(),
            unescape: true,
            ..FileRouteRule::default()
        },
        FileRouteRule {
            pattern: r"(?i)^(?:[^/]*/)*([^/.]+)\.(.*?)-schema-view\.sql(?:\.(\w*?))?$".into(),
            schema: "$1".into(),
            table: "$2".into(),
            type_name: VIEW_SCHEMA.into(),
            compression: "$3".into(),
            unescape: true,
            ..FileRouteRule::default()
        },
        FileRouteRule {
            pattern:
                r"(?i)^(?:[^/]*/)*([^/.]+)\.(.*)\.([0-9]+)\.(snappy|gzip|gz|zstd|zst)\.parquet$"
                    .into(),
            schema: "$1".into(),
            table: "$2".into(),
            type_name: TYPE_PARQUET.into(),
            key: "$3".into(),
            unescape: true,
            ..FileRouteRule::default()
        },
        FileRouteRule {
            pattern:
                r"(?i)^(?:[^/]*/)*([^/.]+)\.(.*?)(?:\.([0-9]+))?\.(sql|csv|parquet)(?:\.(\w+))?$"
                    .into(),
            schema: "$1".into(),
            table: "$2".into(),
            type_name: "$4".into(),
            key: "$3".into(),
            compression: "$5".into(),
            unescape: true,
            ..FileRouteRule::default()
        },
    ]
}

// FileRouter 对应 Go 接口；Ok(None) 表示路径不匹配，Err 表示已匹配但捕获值无效。
/// 文件路由器接口：路径 → 可选 RouteResult。
pub trait FileRouter {
    /// 路由路径；Ok(None) 表示不匹配，Err 表示匹配但捕获无效。
    fn route(&self, path: &str) -> Result<Option<RouteResult>, Error>;
    /// PascalCase 别名。
    fn Route(&self, path: &str) -> Result<Option<RouteResult>, Error> {
        self.route(path)
    }
}

// ChainRouters 保持配置顺序，首个匹配规则立即返回。
/// 按配置顺序串联的多规则路由器，首个匹配即返回。
pub struct ChainRouters {
    routers: Vec<RegexRouter>,
}

impl FileRouter for ChainRouters {
    fn route(&self, path: &str) -> Result<Option<RouteResult>, Error> {
        for router in &self.routers {
            if let Some(result) = router.route(path)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
}

// new_file_router 编译所有规则；任一规则非法时整体构造失败，不留下部分 router。
/// 编译全部规则为 ChainRouters；任一非法则整体失败。
pub fn new_file_router(rules: &[FileRouteRule], logger: Logger) -> Result<ChainRouters, Error> {
    let parser = RegexRouterParser;
    let mut routers = Vec::with_capacity(rules.len());
    for rule in rules {
        routers.push(parser.parse(rule, &logger)?);
    }
    Ok(ChainRouters { routers })
}
/// PascalCase 别名。
pub fn NewFileRouter(rules: &[FileRouteRule], logger: Logger) -> Result<ChainRouters, Error> {
    new_file_router(rules, logger)
}

/// 使用默认规则构造路由器。
pub fn new_default_file_router(logger: Logger) -> Result<ChainRouters, Error> {
    new_file_router(&default_file_route_rules(), logger)
}
/// PascalCase 别名。
pub fn NewDefaultFileRouter(logger: Logger) -> Result<ChainRouters, Error> {
    new_default_file_router(logger)
}

// RegexRouter 对应单条 FileRouter 实现，extractors 按 type/schema/table/key/compression 顺序写入结果。
/// 单条正则路由实现。
pub struct RegexRouter {
    pattern: Regex,
    extractors: Vec<PatternExpander>,
}

impl FileRouter for RegexRouter {
    fn route(&self, path: &str) -> Result<Option<RouteResult>, Error> {
        let captures = match self.pattern.captures(path) {
            Some(value) => value,
            None => return Ok(None),
        };
        let mut result = RouteResult::default();
        for extractor in &self.extractors {
            extractor.expand(&self.pattern, path, &captures, &mut result)?;
        }
        Ok(Some(result))
    }
}

/// 将 FileRouteRule 编译为 RegexRouter 的解析器。
struct RegexRouterParser;

impl RegexRouterParser {
    // parse 对应 Go Parse：path 与 pattern 互斥，常量 path 会转义成正则及字面量模板。
    fn parse(&self, input: &FileRouteRule, logger: &Logger) -> Result<RegexRouter, Error> {
        if input.path.is_empty() && input.pattern.is_empty() {
            return Err(error_new(
                "`path` and `pattern` must not be both empty in [[mydumper.files]]",
            ));
        }
        if !input.path.is_empty() && !input.pattern.is_empty() {
            return Err(error_new(
                "can't set both `path` and `pattern` field in [[mydumper.files]]",
            ));
        }

        let mut rule = input.clone();
        if !rule.path.is_empty() {
            rule.pattern = regex_escape(&rule.path);
            // Go regexp.Expand 中 $$ 表示字面量 $，所以常量 path 模式需把模板中的 $ 全部加倍。
            rule.table = quote_template(&rule.table);
            rule.schema = quote_template(&rule.schema);
            rule.type_name = quote_template(&rule.type_name);
            rule.compression = quote_template(&rule.compression);
            rule.key = quote_template(&rule.key);
        }
        let pattern = Regex::new(&rule.pattern).map_err(|e| errorf(e.to_string()))?;
        let mut router = RegexRouter {
            pattern,
            extractors: Vec::new(),
        };

        self.parse_field_extractor(&mut router, "type", &rule.type_name, Setter::Type)?;
        if rule.type_name == TYPE_IGNORE {
            return Ok(router);
        }

        self.parse_field_extractor(
            &mut router,
            "schema",
            &rule.schema,
            Setter::Schema {
                unescape: rule.unescape,
                logger: logger.clone(),
            },
        )?;
        // DB schema 文件没有 table 捕获，不能把空模板误判为配置错误。
        if rule.type_name != SCHEMA_SCHEMA {
            self.parse_field_extractor(
                &mut router,
                "table",
                &rule.table,
                Setter::Table {
                    unescape: rule.unescape,
                    logger: logger.clone(),
                },
            )?;
        }
        if !rule.key.is_empty() {
            self.parse_field_extractor(&mut router, "key", &rule.key, Setter::Key)?;
        }
        if !rule.compression.is_empty() {
            self.parse_field_extractor(
                &mut router,
                "compression",
                &rule.compression,
                Setter::Compression {
                    path: rule.path.clone(),
                },
            )?;
        }
        Ok(router)
    }
    pub fn Parse(&self, input: &FileRouteRule, logger: &Logger) -> Result<RegexRouter, Error> {
        self.parse(input, logger)
    }

    // parse_field_extractor 先验证模板里的数字/命名捕获，再把 setter 追加到规则。
    fn parse_field_extractor(
        &self,
        router: &mut RegexRouter,
        field: &str,
        template: &str,
        setter: Setter,
    ) -> Result<(), Error> {
        if template.is_empty() {
            return Err(errorf(format!(
                "field '{field}' match pattern can't be empty"
            )));
        }
        self.check_sub_patterns(&router.pattern, template)?;
        router.extractors.push(PatternExpander {
            template: template.into(),
            setter,
        });
        Ok(())
    }

    // check_sub_patterns 对应 expandVariablePattern：接受 $$、$1、${1}、$name 与 ${name}。
    fn check_sub_patterns(&self, pattern: &Regex, template: &str) -> Result<(), Error> {
        for variable in find_expand_variables(template) {
            if variable == "$$" {
                continue;
            }
            let name = variable
                .strip_prefix("${")
                .and_then(|value| value.strip_suffix('}'))
                .or_else(|| variable.strip_prefix('$'))
                .unwrap_or("");
            if let Ok(number) = name.parse::<usize>() {
                if number > pattern.captures_len().saturating_sub(1) {
                    return Err(errorf(format!(
                        "sub pattern capture '{variable}' out of range"
                    )));
                }
            } else if !pattern
                .capture_names()
                .flatten()
                .any(|candidate| candidate == name)
            {
                return Err(errorf(format!("invalid named capture '{variable}'")));
            }
        }
        Ok(())
    }
}

/// 将模板中的 `$` 加倍为 `$$`，避免 Expand 误解释。
fn quote_template(value: &str) -> String {
    value.replace('$', "$$")
}

// Setter 代替 Go 闭包 applyFn，仍在 expand 完成后执行字段专属校验与赋值。
/// 字段赋值器：在 expand 完成后写入 RouteResult 并做类型校验。
enum Setter {
    Type,
    Schema { unescape: bool, logger: Logger },
    Table { unescape: bool, logger: Logger },
    Key,
    Compression { path: String },
}

/// 模板展开器：按捕获组填充模板再交给 Setter。
struct PatternExpander {
    template: String,
    setter: Setter,
}

impl PatternExpander {
    fn expand(
        &self,
        _pattern: &Regex,
        _path: &str,
        captures: &Captures,
        result: &mut RouteResult,
    ) -> Result<(), Error> {
        let mut value = String::new();
        captures.expand(&self.template, &mut value);
        match &self.setter {
            Setter::Type => result.source_type = parse_source_type(&value)?,
            Setter::Schema { unescape, logger } => {
                result.schema = set_routed_value(&value, *unescape, logger)
            }
            Setter::Table { unescape, logger } => {
                result.name = set_routed_value(&value, *unescape, logger)
            }
            Setter::Key => result.key = value,
            Setter::Compression { path } => {
                let compression = parse_compression_type(&value)?;
                if result.source_type == SourceType::Parquet && compression != Compression::None {
                    return Err(errorf(format!(
                        "can't support whole compressed parquet file, should compress parquet files by choosing correct parquet compress writer, path: {path}"
                    )));
                }
                result.compression = compression;
            }
        }
        Ok(())
    }
    pub fn Expand(
        &self,
        pattern: &Regex,
        path: &str,
        captures: &Captures,
        result: &mut RouteResult,
    ) -> Result<(), Error> {
        self.expand(pattern, path, captures, result)
    }
}

// set_routed_value 执行可选 URL path unescape；失败只告警并保留原捕获值。
/// 可选 URL unescape；失败只告警并保留原值。
fn set_routed_value(value: &str, unescape: bool, logger: &Logger) -> String {
    if !unescape {
        return value.into();
    }
    match percent_decode_str(value).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(err) => {
            let _ = err;
            logger.warn("unescape string failed, will be ignored");
            value.into()
        }
    }
}

/// 转义正则元字符。
fn regex_escape(value: &str) -> String {
    regex::escape(value)
}
/// 找出模板中的 `$`/`${}` 展开变量。
fn find_expand_variables(template: &str) -> Vec<String> {
    let re = Regex::new(r"\$(?:\$|[[:alnum:]_]+|\{[[:alnum:]_]+\})").unwrap();
    re.find_iter(template)
        .map(|m| m.as_str().to_owned())
        .collect()
}

// RouteResult 对应 Go 的 filter.Table 嵌入字段与路由附加信息。
#[derive(Clone, Debug)]
/// 路由结果：库表名、分片 key、压缩与来源类型。
pub struct RouteResult {
    /// 目标库名。
    pub schema: String,
    /// 目标表/视图名。
    pub name: String,
    /// 分片 key（如序号）。
    pub key: String,
    /// 整体压缩格式。
    pub compression: Compression,
    /// 来源文件类型。
    pub source_type: SourceType,
}

impl Default for RouteResult {
    fn default() -> Self {
        Self {
            schema: String::new(),
            name: String::new(),
            key: String::new(),
            compression: Compression::None,
            source_type: SourceType::Ignore,
        }
    }
}
/// 导出用：解析 SourceType。
pub fn parseSourceType(value: &str) -> Result<SourceType, Error> {
    parse_source_type(value)
}
/// 导出用：解析 Compression。
pub fn parseCompressionType(value: &str) -> Result<Compression, Error> {
    parse_compression_type(value)
}
/// 导出用：解析单条规则。
pub fn parseFieldExtractor(rule: &FileRouteRule, logger: &Logger) -> Result<RegexRouter, Error> {
    RegexRouterParser.parse(rule, logger)
}
/// 导出用：校验模板捕获是否合法。
pub fn checkSubPatterns(pattern: &str, template: &str) -> Result<(), Error> {
    let regex = Regex::new(pattern).map_err(|e| errorf(e.to_string()))?;
    RegexRouterParser.check_sub_patterns(&regex, template)
}
/// 将 SourceType 转为字符串（测试/调试辅助）。
fn String(source: SourceType) -> String {
    source.to_string()
}
