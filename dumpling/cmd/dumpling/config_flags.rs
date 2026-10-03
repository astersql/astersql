// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! CLI flag definitions / parsing mirroring Go `export.Config.DefineFlags` /
//! `ParseFromFlags`. Kept in this crate because the export package's arm64-safe
//! surface does not yet expose pflag helpers.
//!
//! 这个模块负责把 Go 版 dumpling CLI 的参数表和解析顺序原样搬到 Rust，
//! 并把最终结果回填到 `export::Config`。
//! 之所以不直接放进 `export` crate，是因为当前 arm64-safe 适配层没有暴露
//! 完整的 `pflag` 能力，所以这里承担“命令行胶水层”的职责。

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_dumpling_export::{
    self as export, CaseInsensitive, CompressType, Config, DefaultAnonymousOutputFileTemplateText,
    DefaultRowGroupMemoryLimitBytes, DefaultStatementSize, DefaultTableFilter, FileFormatCSVString,
    GetConfTables, MiB, ParseFileSize, ParseOutputDialect, ParseOutputFileTemplate,
    ParseTableFilter, RAMInBytes, UnspecifiedSize, normalizePartitions, outputTemplateUsesIndex,
    parseParquetCompressType,
};

use crate::stubs::{FlagHelp, FlagSet};

// 下面这组常量保持与 Go flag 名称一致，便于对照文档、脚本和 parity test。
// 常量拆开定义而不是内联字面量，可以避免不同注册/解析位置拼写漂移。
// 后续注释主要解释每一段 flag 负责的功能域，而不是重复 flag 名字本身。
const FLAG_DATABASE: &str = "database";
const FLAG_TABLES_LIST: &str = "tables-list";
// 连接四元组是最常被脚本覆盖的基础参数。
const FLAG_HOST: &str = "host";
const FLAG_USER: &str = "user";
const FLAG_PORT: &str = "port";
const FLAG_PASSWORD: &str = "password";
const FLAG_ALLOW_CLEARTEXT_PASSWORDS: &str = "allow-cleartext-passwords";
// 线程、文件大小和 statement size 共同决定导出切片粒度。
const FLAG_THREADS: &str = "threads";
const FLAG_FILESIZE: &str = "filesize";
const FLAG_STATEMENT_SIZE: &str = "statement-size";
const FLAG_OUTPUT: &str = "output";
// 日志与一致性参数影响观测性和读取视图。
const FLAG_LOGLEVEL: &str = "loglevel";
const FLAG_LOGFILE: &str = "logfile";
const FLAG_LOGFMT: &str = "logfmt";
const FLAG_CONSISTENCY: &str = "consistency";
const FLAG_SNAPSHOT: &str = "snapshot";
// 这组开关控制是否导出视图、序列和按主键排序。
const FLAG_NO_VIEWS: &str = "no-views";
const FLAG_NO_SEQUENCES: &str = "no-sequences";
const FLAG_SORT_BY_PK: &str = "sort-by-pk";
const FLAG_STATUS_ADDR: &str = "status-addr";
const FLAG_ROWS: &str = "rows";
const FLAG_WHERE: &str = "where";
// 文件格式与转义规则会在 writer 侧触发不同分支。
const FLAG_ESCAPE_BACKSLASH: &str = "escape-backslash";
const FLAG_FILETYPE: &str = "filetype";
const FLAG_NO_HEADER: &str = "no-header";
const FLAG_NO_SCHEMAS: &str = "no-schemas";
const FLAG_NO_DATA: &str = "no-data";
const FLAG_CSV_NULL_VALUE: &str = "csv-null-value";
const FLAG_SQL: &str = "sql";
// filter、大小写和空库开关决定哪些 schema/table 被纳入任务。
const FLAG_FILTER: &str = "filter";
const FLAG_CASE_SENSITIVE: &str = "case-sensitive";
const FLAG_DUMP_EMPTY_DATABASE: &str = "dump-empty-database";
const FLAG_TIDB_MEM_QUOTA_QUERY: &str = "tidb-mem-quota-query";
// TLS 三件套只影响数据库连接安全配置。
const FLAG_CA: &str = "ca";
const FLAG_CERT: &str = "cert";
const FLAG_KEY: &str = "key";
// CSV 细节参数必须一起看，避免产生自相矛盾的输出格式。
const FLAG_CSV_SEPARATOR: &str = "csv-separator";
const FLAG_CSV_DELIMITER: &str = "csv-delimiter";
const FLAG_CSV_LINE_TERMINATOR: &str = "csv-line-terminator";
const FLAG_OUTPUT_FILENAME_TEMPLATE: &str = "output-filename-template";
const FLAG_COMPLETE_INSERT: &str = "complete-insert";
const FLAG_PARAMS: &str = "params";
// 这两个隐藏开关主要用于兼容历史行为或内部调试。
const FLAG_READ_TIMEOUT: &str = "read-timeout";
const FLAG_TRANSACTIONAL_CONSISTENCY: &str = "transactional-consistency";
// 下面是压缩、方言、分区和 TiDB Premium 集群相关参数。
const FLAG_COMPRESS: &str = "compress";
const FLAG_CSV_OUTPUT_DIALECT: &str = "csv-output-dialect";
const FLAG_PARTITIONS: &str = "partitions";
const FLAG_PD_ADDR: &str = "pd";
const FLAG_CLUSTER_SSL_CA: &str = "cluster-tls-ca";
const FLAG_CLUSTER_SSL_CERT: &str = "cluster-tls-cert";
const FLAG_CLUSTER_SSL_KEY: &str = "cluster-tls-key";
// Parquet 三个参数共同定义写出块布局和压缩算法。
const FLAG_PARQUET_COMPRESS: &str = "parquet-compress";
const FLAG_PARQUET_PAGE_SIZE: &str = "parquet-page-size";
const FLAG_PARQUET_ROW_GROUP_SIZE: &str = "parquet-row-group-size";

fn timestamp_dir_name() -> String {
    // Go uses time.Now().Format(time.RFC3339). Use UTC here: RFC3339 permits `Z`, and
    // retaining the date/time fields matters for the public path contract.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let seconds_in_day = secs % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    format!("./export-{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Convert days since 1970-01-01 to a proleptic Gregorian date.
fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm, shifted to the Unix epoch.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn human_bytes(n: i64) -> String {
    // 人类可读大小文案交给 export 侧统一生成，避免 CLI 与核心实现格式不一致。
    export::HumanSize(n as f64)
}

/// Go `(*Config).DefineFlags`.
pub fn DefineFlags(flags: &mut FlagSet) {
    // objstore.DefineFlags — no-op in arm64 export BackendOptions stub.
    // 选择导出对象的参数放在最前面，和 Go 帮助输出分组顺序一致。
    flags.StringSliceP(FLAG_DATABASE, 'B', vec![], "Databases to dump");
    flags.StringSliceP(
        FLAG_TABLES_LIST,
        'T',
        vec![],
        "Comma delimited table list to dump; must be qualified table names",
    );
    // 连接参数沿用 dumpling 传统默认值，尤其是 host/user/port 的组合。
    flags.StringP(FLAG_HOST, 'h', "127.0.0.1", "The host to connect to");
    flags.StringP(
        FLAG_USER,
        'u',
        "root",
        "Username with privileges to run the dump",
    );
    flags.IntP(FLAG_PORT, 'P', 4000, "TCP/IP port to connect to");
    flags.StringP(FLAG_PASSWORD, 'p', "", "User password");
    flags.Bool(
        FLAG_ALLOW_CLEARTEXT_PASSWORDS,
        false,
        "Allow passwords to be sent in cleartext (warning: don't use without TLS)",
    );
    // 并发与文件切分参数直接影响导出拆块策略，默认值需对齐 Go。
    flags.IntP(
        FLAG_THREADS,
        't',
        4,
        "Number of concurrent workers to use, default 4",
    );
    flags.StringP(
        FLAG_FILESIZE,
        'F',
        "",
        "The approximate size of output file",
    );
    flags.Uint64P(
        FLAG_STATEMENT_SIZE,
        's',
        DefaultStatementSize,
        "Attempted size of INSERT statement in bytes",
    );
    // 输出目录默认带时间戳，避免多次运行互相覆盖上一次结果。
    let out = timestamp_dir_name();
    flags.StringP(FLAG_OUTPUT, 'o', &out, "Output directory");
    // 日志相关参数只控制 CLI 侧展示与写盘格式，不改变导出语义。
    flags.String(
        FLAG_LOGLEVEL,
        "info",
        "Log level: {debug|info|warn|error|dpanic|panic|fatal}",
    );
    flags.StringP(
        FLAG_LOGFILE,
        'L',
        "",
        "Log file `path`, leave empty to write to console",
    );
    // text/json 两种格式足以覆盖当前 dumpling 的可观测性需求。
    flags.String(FLAG_LOGFMT, "text", "Log `format`: {text|json}");
    // 一致性和快照参数决定读取视图，必须保持与 Go 的枚举和值域相同。
    // 这两项组合错误时，真正的语义校验会留到更靠近导出执行的位置。
    flags.String(
        FLAG_CONSISTENCY,
        export::ConsistencyTypeAuto,
        "Consistency level during dumping: {auto|none|flush|lock|snapshot}",
    );
    flags.String(
        FLAG_SNAPSHOT,
        "",
        "Snapshot position (uint64 or MySQL style string timestamp). Valid only when consistency=snapshot",
    );
    flags.BoolP(FLAG_NO_VIEWS, 'W', true, "Do not dump views");
    flags.Bool(FLAG_NO_SEQUENCES, true, "Do not dump sequences");
    flags.Bool(
        FLAG_SORT_BY_PK,
        true,
        "Sort dump results by primary key through order by sql",
    );
    // status 地址既承载 API server 也承载 pprof，因此默认值要与 Go 文档一致。
    flags.String(
        FLAG_STATUS_ADDR,
        ":8281",
        "dumpling API server and pprof addr",
    );
    flags.Uint64P(
        FLAG_ROWS,
        'r',
        UnspecifiedSize,
        "If specified, dumpling will split table into chunks and concurrently dump them to different files to improve efficiency. For TiDB v3.0+, specify this will make dumpling split table with each file one TiDB region(no matter how many rows is).\nIf not specified, dumpling will dump table without inner-concurrency which could be relatively slow. default unlimited",
    );
    // where/sql/filetype 属于导出内容维度，组合时会影响后续兼容性校验。
    flags.String(FLAG_WHERE, "", "Dump only selected records");
    flags.Bool(
        FLAG_ESCAPE_BACKSLASH,
        true,
        "use backslash to escape special characters",
    );
    flags.String(
        FLAG_FILETYPE,
        "",
        "The type of export file (sql/csv/parquet)",
    );
    flags.Bool(
        FLAG_NO_HEADER,
        false,
        "whether not to dump CSV table header",
    );
    // `no-schemas` 和 `no-data` 允许把 schema/data 两条导出路径拆开使用。
    flags.BoolP(
        FLAG_NO_SCHEMAS,
        'm',
        false,
        "Do not dump table schemas with the data",
    );
    flags.BoolP(FLAG_NO_DATA, 'd', false, "Do not dump table data");
    flags.String(
        FLAG_CSV_NULL_VALUE,
        "\\N",
        "The null value used when export to csv",
    );
    // 这个默认值延续 MySQL 常见导出表示，减少和已有流水线的摩擦。
    flags.StringP(
        FLAG_SQL,
        'S',
        "",
        "Dump data with given sql. This argument doesn't support concurrent dump",
    );
    // 由于 SQL 自定义入口会绕开常规表枚举，所以并发限制写在帮助文案里。
    // Go 同样隐藏 `--sql`，因为它更多服务内部或高级场景，不希望挤占帮助输出。
    let _ = flags.MarkHidden(FLAG_SQL);
    // filter 与 case-sensitive 一起决定表匹配语义，后续解析阶段还会二次规范化。
    flags.StringSliceP(
        FLAG_FILTER,
        'f',
        vec!["*.*".into(), DefaultTableFilter.into()],
        "filter to select which tables to dump",
    );
    flags.StringArray("column-filter", vec![], "Inline TOML column filter rule for data and schema projection. Can be specified multiple times. Example: --column-filter '{ matcher = [\"db.tbl\"], columns = [\"*\", \"!col\"] }'. Unmatched tables are dumped with all columns; column rules are case-insensitive. Mutually exclusive with --column-filter-file and cannot be used with --sql");
    flags.String(
        "column-filter-file",
        "",
        "Path to the column filter TOML file for data and schema projection. Unmatched tables are dumped with all columns; column rules are case-insensitive. Cannot be used with --sql",
    );
    flags.Bool(
        FLAG_CASE_SENSITIVE,
        false,
        "whether the filter should be case-sensitive",
    );
    // 默认为大小写不敏感，更贴近 TiDB/MySQL 用户的直觉体验。
    flags.Bool(
        FLAG_DUMP_EMPTY_DATABASE,
        true,
        "whether to dump empty database",
    );
    // 下面这组参数控制会话资源和 TLS 连接，是连接稳定性的补充配置。
    flags.Uint64(
        FLAG_TIDB_MEM_QUOTA_QUERY,
        UnspecifiedSize,
        "The maximum memory limit for a single SQL statement, in bytes.",
    );
    flags.String(
        FLAG_CA,
        "",
        "The path name to the certificate authority file for TLS connection",
    );
    flags.String(
        FLAG_CERT,
        "",
        "The path name to the client certificate file for TLS connection",
    );
    flags.String(
        FLAG_KEY,
        "",
        "The path name to the client private key file for TLS connection",
    );
    // CSV 相关参数彼此有语义约束，因此集中放在一起方便用户对照。
    flags.String(
        FLAG_CSV_SEPARATOR,
        ",",
        "The separator for csv files, default ','",
    );
    flags.String(
        FLAG_CSV_DELIMITER,
        "\"",
        "The delimiter for values in csv files, default '\"'",
    );
    flags.String(
        FLAG_CSV_LINE_TERMINATOR,
        "\r\n",
        "The line terminator for csv files, default '\\r\\n'",
    );
    flags.String(
        FLAG_OUTPUT_FILENAME_TEMPLATE,
        "",
        "The output filename template (without file extension). When used with --rows/-r or --filesize/-F in split mode, include {{.Index}} (for example: '{{.DB}}.{{.Table}}.{{.Index}}') to avoid overwriting chunk files",
    );
    // 模板默认留空，是为了让后续按导出模式推导最合适的文件名。
    flags.Bool(
        FLAG_COMPLETE_INSERT,
        false,
        "Use complete INSERT statements that include column names",
    );
    // session params 直接透传给下游连接，用 map 形式保留与 Go 相同的表达方式。
    flags.StringToString(
        FLAG_PARAMS,
        HashMap::new(),
        r#"Extra session variables used while dumping, accepted format: --params "character_set_client=latin1,character_set_connection=latin1""#,
    );
    // map 形式可以一次传多项 session 参数，避免为每个变量单独开 flag。
    flags.Bool(FlagHelp, false, "Print help message and quit");
    // help flag 不走 shorthand，是为了避免和其他短参数产生冲突。
    // 这两个隐藏参数保留是为了兼容上游接口，但不鼓励普通用户直接设置。
    flags.Duration(
        FLAG_READ_TIMEOUT,
        Duration::from_secs(15 * 60),
        "I/O read timeout for db connection.",
    );
    let _ = flags.MarkHidden(FLAG_READ_TIMEOUT);
    flags.Bool(
        FLAG_TRANSACTIONAL_CONSISTENCY,
        true,
        "Only support transactional consistency",
    );
    let _ = flags.MarkHidden(FLAG_TRANSACTIONAL_CONSISTENCY);
    // 压缩、CSV 方言、分区和 PD/TLS 参数属于进阶输出与集群控制配置。
    flags.StringP(
        FLAG_COMPRESS,
        'c',
        "",
        "Compress output file type, support 'gzip', 'snappy', 'zstd', 'no-compression' now",
    );
    flags.String(
        FLAG_CSV_OUTPUT_DIALECT,
        "",
        "The dialect of output CSV file, support 'snowflake', 'redshift', 'bigquery' now",
    );
    flags.StringSlice(
        FLAG_PARTITIONS,
        vec![],
        "The table partitions to dump. Every listed partition must exist on all selected base tables; incompatible with --sql. TiDB >= v5.0.0 only",
    );
    // 分区参数独立存在，是因为它和普通表过滤器表达的粒度不同。
    flags.String(
        FLAG_PD_ADDR,
        "",
        "PD endpoints for controlling GC in premium keyspace clusters (comma-separated host:port list; http(s):// is also accepted and normalized)",
    );
    flags.String(
        FLAG_CLUSTER_SSL_CA,
        "",
        "CA certificate path for TLS connections to PD endpoints used by GC control; if empty, reuse --ca",
    );
    flags.String(
        FLAG_CLUSTER_SSL_CERT,
        "",
        "Client certificate path for TLS connections to PD endpoints used by GC control; if empty, reuse --cert",
    );
    flags.String(
        FLAG_CLUSTER_SSL_KEY,
        "",
        "Client private key path for TLS connections to PD endpoints used by GC control; if empty, reuse --key",
    );
    // Parquet 的压缩与块大小默认值要与 export 侧 writer 假设保持一致。
    flags.String(
        FLAG_PARQUET_COMPRESS,
        "snappy",
        "Compress algorithm for parquet file, support 'no-compression', 'snappy', 'gzip', 'zstd'",
    );
    flags.String(
        FLAG_PARQUET_PAGE_SIZE,
        &human_bytes(MiB),
        "Parquet page size in bytes, accepts human-readable units",
    );
    // page size 与 row group size 分开暴露，方便用户在吞吐和内存之间取舍。
    flags.String(
        FLAG_PARQUET_ROW_GROUP_SIZE,
        &human_bytes(DefaultRowGroupMemoryLimitBytes),
        "Parquet row-group memory limit in bytes (flush threshold by accounted in-memory bytes), accepts human-readable units",
    );
}

fn parse_compress_type(s: &str) -> Result<CompressType, String> {
    // Go parser is deliberately case-sensitive and accepts only these historical aliases.
    match s {
        "" | "no-compression" => Ok(CompressType::NoCompression),
        "gzip" | "gz" => Ok(CompressType::Gzip),
        "snappy" => Ok(CompressType::Snappy),
        "zstd" | "zst" => Ok(CompressType::Zstd),
        other => Err(format!("unknown compress type {other}")),
    }
}

fn parse_size_flag(flags: &FlagSet, name: &str) -> Result<i64, String> {
    // parquet page/row-group 复用同一套字节解析逻辑，避免两处错误文案分叉。
    // 这里读取的是字符串 flag，而不是已经解析好的数值字段。
    let s = flags.GetString(name).map_err(|e| e.to_string())?;
    RAMInBytes(&s).map_err(|e| e.msg)
}

/// Go `(*Config).ParseFromFlags`.
pub fn ParseFromFlags(conf: &mut Config, flags: &FlagSet) -> Result<(), String> {
    // 第一阶段只做“按名字取值并回填 Config”，尽量保持和 flag 表一一对应。
    // 这里不做复杂推导，方便定位到底是“读 flag 失败”还是“组合校验失败”。
    conf.Databases = flags.GetStringSlice(FLAG_DATABASE)?;
    conf.Host = flags.GetString(FLAG_HOST)?;
    conf.User = flags.GetString(FLAG_USER)?;
    conf.Port = flags.GetInt(FLAG_PORT)?;
    conf.Password = flags.GetString(FLAG_PASSWORD)?;
    conf.AllowCleartextPasswords = flags.GetBool(FLAG_ALLOW_CLEARTEXT_PASSWORDS)?;
    conf.Threads = flags.GetInt(FLAG_THREADS)?;
    conf.StatementSize = flags.GetUint64(FLAG_STATEMENT_SIZE)?;
    conf.OutputDirPath = flags.GetString(FLAG_OUTPUT)?;
    conf.LogLevel = flags.GetString(FLAG_LOGLEVEL)?;
    conf.LogFile = flags.GetString(FLAG_LOGFILE)?;
    conf.LogFormat = flags.GetString(FLAG_LOGFMT)?;
    conf.Consistency = flags.GetString(FLAG_CONSISTENCY)?;
    conf.Snapshot = flags.GetString(FLAG_SNAPSHOT)?;
    conf.NoViews = flags.GetBool(FLAG_NO_VIEWS)?;
    conf.NoSequences = flags.GetBool(FLAG_NO_SEQUENCES)?;
    conf.SortByPk = flags.GetBool(FLAG_SORT_BY_PK)?;
    conf.StatusAddr = flags.GetString(FLAG_STATUS_ADDR)?;
    conf.Rows = flags.GetUint64(FLAG_ROWS)?;
    conf.Where = flags.GetString(FLAG_WHERE)?;
    conf.EscapeBackslash = flags.GetBool(FLAG_ESCAPE_BACKSLASH)?;
    conf.FileType = flags.GetString(FLAG_FILETYPE)?;
    conf.NoHeader = flags.GetBool(FLAG_NO_HEADER)?;
    conf.NoSchemas = flags.GetBool(FLAG_NO_SCHEMAS)?;
    conf.NoData = flags.GetBool(FLAG_NO_DATA)?;
    conf.CsvNullValue = flags.GetString(FLAG_CSV_NULL_VALUE)?;
    conf.SQL = flags.GetString(FLAG_SQL)?;
    conf.DumpEmptyDatabase = flags.GetBool(FLAG_DUMP_EMPTY_DATABASE)?;
    conf.Security.CAPath = flags.GetString(FLAG_CA)?;
    conf.Security.CertPath = flags.GetString(FLAG_CERT)?;
    conf.Security.KeyPath = flags.GetString(FLAG_KEY)?;
    conf.CsvSeparator = flags.GetString(FLAG_CSV_SEPARATOR)?;
    conf.CsvDelimiter = flags.GetString(FLAG_CSV_DELIMITER)?;
    conf.CsvLineTerminator = flags.GetString(FLAG_CSV_LINE_TERMINATOR)?;
    conf.CompleteInsert = flags.GetBool(FLAG_COMPLETE_INSERT)?;
    conf.ReadTimeout = flags.GetDuration(FLAG_READ_TIMEOUT)?;
    conf.TransactionalConsistency = flags.GetBool(FLAG_TRANSACTIONAL_CONSISTENCY)?;
    conf.TiDBMemQuotaQuery = flags.GetUint64(FLAG_TIDB_MEM_QUOTA_QUERY)?;
    conf.Partitions = flags.GetStringSlice(FLAG_PARTITIONS)?;
    conf.Partitions = normalizePartitions(&conf.Partitions);
    // 分区名先规范化，后面的比较与 writer 逻辑才能基于统一大小写/格式。
    // 如果将来分区语义增强，也应优先在 normalize 结果上扩展。

    // 先做最基础的局部约束校验，尽早阻断明显非法输入。
    if conf.Threads <= 0 {
        return Err(format!(
            "--threads is set to {}. It should be greater than 0",
            conf.Threads
        ));
    }
    if conf.CsvSeparator.is_empty() {
        return Err("--csv-separator is set to \"\". It must not be an empty string".into());
    }
    // CSV 分隔符为空会让后续 writer 无法生成可解析输出，因此单独提前报错。

    // 第二阶段读取那些需要联合推导的原始 flag。
    let tables_list = flags.GetStringSlice(FLAG_TABLES_LIST)?;
    let file_size_str = flags.GetString(FLAG_FILESIZE)?;
    let filters = flags.GetStringSlice(FLAG_FILTER)?;
    let case_sensitive = flags.GetBool(FLAG_CASE_SENSITIVE)?;
    conf.parseColumnFilterOptions(
        &flags.GetStringArray("column-filter")?,
        &flags.GetString("column-filter-file")?,
        case_sensitive,
    )
    .map_err(|e| e.msg)?;
    let mut output_filename_format = flags.GetString(FLAG_OUTPUT_FILENAME_TEMPLATE)?;
    let params = flags.GetStringToString(FLAG_PARAMS)?;
    // `params` 不在前面直接写入，是因为这里要保留一次性合并的上下文。
    // `_case_sensitive` 当前仅用于保留接口语义，与现有过滤器实现仍有差异。

    // 指定 tables-list 时，需要同步构造显式表清单和 table filter。
    conf.SpecifiedTables = !tables_list.is_empty();
    conf.Tables = GetConfTables(&tables_list).map_err(|e| e.msg)?;
    let filter_tables;
    let filter_patterns;
    let (tables_for_filter, patterns_for_filter) = if case_sensitive {
        (tables_list.as_slice(), filters.as_slice())
    } else {
        filter_tables = tables_list
            .iter()
            .map(|value| value.to_lowercase())
            .collect::<Vec<_>>();
        if tables_list.is_empty() {
            filter_patterns = filters
                .iter()
                .map(|value| value.to_lowercase())
                .collect::<Vec<_>>();
            (filter_tables.as_slice(), filter_patterns.as_slice())
        } else {
            // ParseTableFilter uses the original registered defaults to distinguish the
            // implicit filter from an explicitly supplied --filter, just like Go.
            (filter_tables.as_slice(), filters.as_slice())
        }
    };
    let parsed_filter = ParseTableFilter(tables_for_filter, patterns_for_filter)
        .map_err(|e| format!("failed to parse filter: {}", e.msg))?;
    conf.TableFilter = if case_sensitive {
        parsed_filter
    } else {
        CaseInsensitive(parsed_filter)
    };
    // PatternFilter compares exact allow rules, so lowercasing both parsed patterns and
    // runtime names reproduces Go's default case-insensitive wrapper for ASCII SQL names.

    // 文件大小允许为空或人类可读单位文本，交给 export 公共解析器处理。
    conf.FileSize = ParseFileSize(&file_size_str).map_err(|e| e.msg)?;
    // 统一走公共解析器还能保证 CLI 和测试对非法单位报出相同文案。

    // 当使用自定义 SQL 且未显式给模板时，要补上匿名输出模板，避免文件名缺位。
    if output_filename_format.is_empty() && !conf.SQL.is_empty() {
        output_filename_format = DefaultAnonymousOutputFileTemplateText.to_string();
    }
    // 这个补默认值的行为是 Go 兼容点，否则 SQL-only 导出会缺少输出文件名。
    // 模板解析失败时保留原始用户输入，便于 CLI 直接指出是哪段模板有问题。
    let tmpl = ParseOutputFileTemplate(&output_filename_format).map_err(|_| {
        format!(
            "failed to parse output filename template (--output-filename-template '{output_filename_format}')"
        )
    })?;
    let output_split = conf.Rows != UnspecifiedSize || conf.FileSize != UnspecifiedSize;
    // 只要按行数或文件大小拆分，就意味着单表会输出多个 chunk 文件。
    // split 模式必须保证模板里显式出现索引位，否则多个 chunk 会互相覆盖。
    if flags.Changed(FLAG_OUTPUT_FILENAME_TEMPLATE)
        && output_split
        && !outputTemplateUsesIndex(&tmpl, "data")
    {
        // 这里直接在 CLI 层拒绝危险模板，比生成文件时再覆盖更安全。
        return Err(
            "--output-filename-template must include a standalone {{.Index}} outside conditional blocks (for example: '{{.DB}}.{{.Table}}.{{.Index}}') when split mode is enabled by --rows/-r or --filesize/-F; otherwise chunk files may overwrite each other"
                .into(),
        );
    }
    conf.OutputFileTemplate = tmpl;
    // 只有模板通过校验后才回写，避免 Config 中残留一个不可安全使用的模板。

    // 压缩类型接受历史别名，但最终都要收敛到统一的枚举值。
    let compress_type = flags.GetString(FLAG_COMPRESS)?;
    conf.CompressType = parse_compress_type(&compress_type)?;
    // 统一枚举值后，writer 无需再重复处理 CLI 别名分支。

    // CSV 方言只在整表导出 CSV 时生效，防止和 SQL/Parquet 路径产生歧义。
    let dialect = flags.GetString(FLAG_CSV_OUTPUT_DIALECT)?;
    if !dialect.is_empty() && !conf.FileType.eq_ignore_ascii_case(FileFormatCSVString) {
        return Err(format!(
            "{FLAG_CSV_OUTPUT_DIALECT} is only supported when dumping whole table to csv, not compatible with {}",
            conf.FileType
        ));
    }
    conf.CsvOutputDialect = ParseOutputDialect(&dialect).map_err(|e| e.msg)?;
    // 即使方言字符串非空，也必须先通过文件类型门禁再尝试解析。

    // Parquet 相关参数分成压缩算法和两类尺寸限制，分别走对应解析器。
    let parquet_compress = flags.GetString(FLAG_PARQUET_COMPRESS)?;
    conf.ParquetCompressType = parseParquetCompressType(&parquet_compress).map_err(|e| e.msg)?;
    conf.ParquetPageSize = parse_size_flag(flags, FLAG_PARQUET_PAGE_SIZE)?;
    conf.ParquetRowGroupSize = parse_size_flag(flags, FLAG_PARQUET_ROW_GROUP_SIZE)?;

    conf.PDAddr = flags.GetString(FLAG_PD_ADDR)?;
    conf.ClusterSSLCA = flags.GetString(FLAG_CLUSTER_SSL_CA)?;
    conf.ClusterSSLCert = flags.GetString(FLAG_CLUSTER_SSL_CERT)?;
    conf.ClusterSSLKey = flags.GetString(FLAG_CLUSTER_SSL_KEY)?;
    // 这些集群级 TLS 参数只被 GC 控制路径使用，但仍需提前装入 Config。
    // 提前装入能保证后续真正接入 GC 控制时不必再改 CLI 层接口。

    // session params 逐项并入 Config，保留调用方显式设置的键值对。
    for (k, v) in params {
        conf.SessionParams.insert(k.to_lowercase(), v);
    }
    // 不做覆盖保护是因为 CLI 语义本来就是“用户传什么就写什么”。
    // 若调用方重复传同名键，后写入值覆盖前值，符合常见 map 参数预期。

    // BackendOptions.ParseFromFlags — no-op stub.
    // arm64-safe 版本当前不展开对象存储后端解析，因此这里明确标注为空操作。
    // 未来若补齐对象存储支持，这里会是最自然的扩展挂点。
    // 在那之前，CLI 侧只负责把本地导出相关配置稳定地喂给 Config。
    Ok(())
}
