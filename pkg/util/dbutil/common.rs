// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// dbutil 通用数据库访问辅助：配置/DSN、元数据查询、校验和、统计桶、重试与批量删除。
//
// 通过 `QueryExecutor`/`DBExecutor` 抽象访问数据库。

// dbutil 中数据库配置、SQL 拼装、查询扫描、重试和事务辅助流程。

// ========== 可执行实现：QueryExecutor/DBExecutor 版 dbutil 通用辅助 ==========
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

use astersql_infoschema::TableInfo;

use crate::interface::{DBExecutor, DbError, QueryExecutor, QueryResult, Value};
use crate::retry::IsRetryableError;
use crate::types::IsTimeTypeAndNeedDecode;

/// SQL 执行默认最大重试次数。
pub const DefaultRetryTime: usize = 10;
/// SQL 执行默认超时（10 秒）。
pub const DefaultTimeout: Duration = Duration::from_secs(10);
/// 超过该耗时的 SQL 可记慢日志（阈值保留，具体打日志由调用方决定）。
pub const SlowLogThreshold: Duration = Duration::from_millis(200);
/// 单次 DELETE 默认批量行数上限（TiDB 侧限制相关）。
pub const DefaultDeleteRowsNum: u64 = 100_000;

/// 数据库连接配置：主机、账号、库名、可选 tidb_snapshot 与端口。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DBConfig {
    pub Host: String,
    pub User: String,
    pub Password: String,
    pub Schema: String,
    pub Snapshot: String,
    pub Port: u16,
}

/// 序列化为不含密码的 JSON 风格字符串，便于日志输出。
impl fmt::Display for DBConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{\"host\":\"{}\",\"user\":\"{}\",\"schema\":\"{}\",\"snapshot\":\"{}\",\"port\":{}}}",
            json_escape(&self.Host),
            json_escape(&self.User),
            json_escape(&self.Schema),
            json_escape(&self.Snapshot),
            self.Port
        )
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(escaped, "\\u{:04x}", character as u32).unwrap();
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// 从 `MYSQL_*` 环境变量读取配置，缺省为本机 root@3306。
pub fn GetDBConfigFromEnv(schema: &str) -> DBConfig {
    DBConfig {
        Host: env::var("MYSQL_HOST")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_owned()),
        Port: env::var("MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|port| *port != 0)
            .unwrap_or(3306),
        User: env::var("MYSQL_USER")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "root".to_owned()),
        Password: env::var("MYSQL_PSWD").unwrap_or_default(),
        Schema: schema.to_owned(),
        Snapshot: String::new(),
    }
}

/// 拼装 MySQL DSN；可附加 `tidb_snapshot` 与会话变量（值带单引号转义）。
pub fn BuildDSN(config: &DBConfig, variables: &HashMap<String, String>) -> String {
    let mut params = vec!["charset=utf8mb4".to_owned()];
    // 历史读：通过 tidb_snapshot 会话变量绑定到指定快照。
    if !config.Snapshot.is_empty() {
        params.push(format!("tidb_snapshot={}", config.Snapshot));
    }
    let mut variables: Vec<_> = variables.iter().collect();
    variables.sort_by_key(|entry| entry.0);
    params.extend(
        variables
            .into_iter()
            .map(|(key, value)| format!("{key}='{}'", value.replace('\'', "\\'"))),
    );
    format!(
        "{}:{}@tcp({}:{})/{}/?{}",
        config.User,
        config.Password,
        config.Host,
        config.Port,
        config.Schema,
        params.join("&")
    )
}

/// 将查询单元格转为字符串；`Null` 返回 `None`。
fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bytes(value) => Some(String::from_utf8_lossy(value).into_owned()),
        Value::Bool(value) => Some(value.to_string()),
        Value::I64(value) => Some(value.to_string()),
        Value::U64(value) => Some(value.to_string()),
        Value::F64(value) => Some(value.to_string()),
    }
}
/// 将查询单元格转为 i64（含字符串解析路径）。
fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::I64(value) => Some(*value),
        Value::U64(value) => i64::try_from(*value).ok(),
        _ => as_string(value)?.parse().ok(),
    }
}
/// 取结果集首行；无数据时返回“no data found in table”。
fn first_row(result: QueryResult) -> Result<Vec<Value>, DbError> {
    result.rows.into_iter().next().ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "no data found in table".to_owned(),
    })
}

/// 执行 `SHOW CREATE TABLE`，返回建表语句字符串。
pub fn GetCreateTableSQL(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
) -> Result<String, DbError> {
    let row = first_row(db.QueryContext(
        &format!("SHOW CREATE TABLE {}", TableName(schema, table)),
        &[],
    )?)?;
    row.get(1).and_then(as_string).ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: format!("table {table} not found"),
    })
}
/// 统计表行数；可选 WHERE 条件与绑定参数。
pub fn GetRowCount(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    where_clause: &str,
    args: &[Value],
) -> Result<i64, DbError> {
    let mut query = format!("SELECT COUNT(1) cnt FROM {}", TableName(schema, table));
    if !where_clause.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(where_clause);
    }
    first_row(db.QueryContext(&query, args)?)?
        .first()
        .and_then(as_i64)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: format!("table `{schema}`.`{table}` not found"),
        })
}
/// 在限定范围内随机抽样列值，并按列值（可选 collation）排序返回。
pub fn GetRandomValues(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    column: &str,
    number: usize,
    limit_range: &str,
    limit_args: &[Value],
    collation: &str,
) -> Result<Vec<String>, DbError> {
    let range = if limit_range.is_empty() {
        "TRUE"
    } else {
        limit_range
    };
    let collate = if collation.is_empty() {
        String::new()
    } else {
        format!(" COLLATE \"{collation}\"")
    };
    let query = format!(
        "SELECT {0} FROM (SELECT {0}, rand() rand_value FROM {1} WHERE {2} ORDER BY rand_value LIMIT {3})rand_tmp ORDER BY {0}{4}",
        ColumnName(column),
        TableName(schema, table),
        range,
        number,
        collate
    );
    Ok(db
        .QueryContext(&query, limit_args)?
        .rows
        .into_iter()
        .filter_map(|row| row.first().and_then(as_string))
        .collect())
}
/// 查询指定列在范围内的 MIN/MAX；空表返回无数据错误。
pub fn GetMinMaxValue(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    column: &str,
    limit_range: &str,
    args: &[Value],
    collation: &str,
) -> Result<(String, String), DbError> {
    let range = if limit_range.is_empty() {
        "TRUE"
    } else {
        limit_range
    };
    let collate = if collation.is_empty() {
        String::new()
    } else {
        format!(" COLLATE \"{collation}\"")
    };
    let query = format!(
        "SELECT /*!40001 SQL_NO_CACHE */ MIN({0}{1}) as MIN, MAX({0}{1}) as MAX FROM {2} WHERE {3}",
        ColumnName(column),
        collate,
        TableName(schema, table),
        range
    );
    let row = first_row(db.QueryContext(&query, args)?)?;
    match (
        row.first().and_then(as_string),
        row.get(1).and_then(as_string),
    ) {
        (Some(min), Some(max)) => Ok((min, max)),
        _ => Err(DbError {
            code: 0,
            sql_state: None,
            message: "no data found in table".to_owned(),
        }),
    }
}

/// 解析 `±HH:MM[:SS]` 时区偏移为秒数包装类型。
pub fn ParseTimeZoneOffset(value: &str) -> Result<DurationOffset, String> {
    let (negative, value) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let fields: Vec<&str> = value.split(':').collect();
    if fields.len() != 3 {
        return Err("invalid timezone offset".to_owned());
    }
    let hours: u64 = fields[0]
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    let minutes: u64 = fields[1]
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    let seconds_field = fields[2].split('.').next().unwrap_or_default();
    let seconds: u64 = seconds_field
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    if hours >= 24 || minutes >= 60 || seconds >= 60 {
        return Err("invalid timezone offset".to_owned());
    }
    let seconds = (hours * 3600 + minutes * 60 + seconds) as i64;
    Ok(DurationOffset(if negative { -seconds } else { seconds }))
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 相对 UTC 的时区偏移（秒）。
pub struct DurationOffset(pub i64);
/// 通过 `TIMEDIFF(NOW(6), UTC_TIMESTAMP(6))` 读取会话时区偏移。
pub fn GetTimeZoneOffset(db: &dyn QueryExecutor) -> Result<DurationOffset, DbError> {
    let row = db.QueryRowContext(
        "SELECT cast(TIMEDIFF(NOW(6), UTC_TIMESTAMP(6)) as time);",
        &[],
    )?;
    let value = row.first().and_then(as_string).ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "timezone offset is NULL".to_owned(),
    })?;
    ParseTimeZoneOffset(&value).map_err(|message| DbError {
        code: 0,
        sql_state: None,
        message,
    })
}
/// 将偏移格式化为 `+HH:MM` / `-HH:MM`。
pub fn FormatTimeZoneOffset(offset: DurationOffset) -> String {
    let sign = if offset.0 < 0 { '-' } else { '+' };
    let seconds = offset.0.unsigned_abs();
    format!("{sign}{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// 执行返回表名列表的查询，取每行第一列。
fn queryTables(db: &dyn QueryExecutor, query: &str) -> Result<Vec<String>, DbError> {
    Ok(db
        .QueryContext(query, &[])?
        .rows
        .into_iter()
        .filter_map(|row| {
            match (
                row.first().and_then(as_string),
                row.get(1).and_then(as_string),
            ) {
                (Some(table), Some(_)) => Some(table),
                _ => None,
            }
        })
        .collect())
}
/// 列出 schema 下非 VIEW 的基表名。
pub fn GetTables(db: &dyn QueryExecutor, schema: &str) -> Result<Vec<String>, DbError> {
    queryTables(
        db,
        &format!(
            "SHOW FULL TABLES IN `{}` WHERE Table_Type != 'VIEW';",
            escapeName(schema)
        ),
    )
}
/// 列出 schema 下的视图名。
pub fn GetViews(db: &dyn QueryExecutor, schema: &str) -> Result<Vec<String>, DbError> {
    queryTables(
        db,
        &format!(
            "SHOW FULL TABLES IN `{}` WHERE Table_Type = 'VIEW';",
            escapeName(schema)
        ),
    )
}
/// 执行 `SHOW DATABASES` 返回全部库名。
pub fn GetSchemas(db: &dyn QueryExecutor) -> Result<Vec<String>, DbError> {
    Ok(db
        .QueryContext("SHOW DATABASES", &[])?
        .rows
        .into_iter()
        .filter_map(|row| row.first().and_then(as_string))
        .collect())
}

/// 按列拼接计算 `BIT_XOR(CRC32(...))` 数据校验和；空结果按 0 处理。
pub fn GetCRC32Checksum(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    table_info: &TableInfo,
    limit_range: &str,
    args: &[Value],
) -> Result<i64, DbError> {
    let names: Vec<_> = table_info
        .columns
        .iter()
        .map(|column| ColumnName(&column.name.original))
        .collect();
    let nulls: Vec<_> = names.iter().map(|name| format!("ISNULL({name})")).collect();
    let query = format!(
        "SELECT BIT_XOR(CAST(CRC32(CONCAT_WS(',', {}, CONCAT({})))AS UNSIGNED)) AS checksum FROM {} WHERE {};",
        names.join(", "),
        nulls.join(", "),
        TableName(schema, table),
        limit_range
    );
    Ok(first_row(db.QueryContext(&query, args)?)?
        .first()
        .and_then(as_i64)
        .unwrap_or(0))
}

/// TiDB `SHOW STATS_BUCKETS` 中一个直方图桶：上下界与计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bucket {
    pub LowerBound: String,
    pub UpperBound: String,
    pub Count: i64,
}
/// 读取指定表的统计信息桶，按列名聚合为 `Bucket` 列表。
pub fn GetBucketsInfo(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    table_info: &TableInfo,
) -> Result<HashMap<String, Vec<Bucket>>, DbError> {
    let result = db.QueryContext(
        "SHOW STATS_BUCKETS WHERE db_name= ? AND table_name= ?;",
        &[schema.into(), table.into()],
    )?;
    let index = |name: &str| {
        result
            .columns
            .iter()
            .position(|column| column.eq_ignore_ascii_case(name))
    };
    let column_index = index("Column_name").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Column_name column missing".to_owned(),
    })?;
    let count_index = index("Count").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Count column missing".to_owned(),
    })?;
    let lower_index = index("Lower_Bound").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Lower_Bound column missing".to_owned(),
    })?;
    let upper_index = index("Upper_Bound").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Upper_Bound column missing".to_owned(),
    })?;
    let mut buckets: HashMap<String, Vec<Bucket>> = HashMap::new();
    for row in result.rows {
        let column = row
            .get(column_index)
            .and_then(as_string)
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: "Column_name is NULL".to_owned(),
            })?;
        let bucket = Bucket {
            Count: row
                .get(count_index)
                .and_then(as_i64)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Count is NULL or invalid".to_owned(),
                })?,
            LowerBound: row
                .get(lower_index)
                .and_then(as_string)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Lower_Bound is NULL".to_owned(),
                })?,
            UpperBound: row
                .get(upper_index)
                .and_then(as_string)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Upper_Bound is NULL".to_owned(),
                })?,
        };
        buckets.entry(column).or_default().push(bucket);
    }

    // TiDB reports an integer primary-key bucket under the column name rather
    // than `PRIMARY`. Match Go by normalizing that key after scanning rows.
    let has_primary = table_info
        .indices
        .iter()
        .any(|index| index.name.original == "PRIMARY")
        || table_info
            .model_meta
            .as_deref()
            .is_some_and(|meta| meta.Indices.iter().any(|index| index.Primary));
    if has_primary && !buckets.contains_key("PRIMARY") {
        let primary_column = table_info
            .model_meta
            .as_deref()
            .and_then(|meta| meta.Indices.iter().find(|index| index.Primary))
            .and_then(|index| index.Columns.first())
            .map(|column| column.Name.O.clone())
            .or_else(|| {
                (table_info.columns.len() == 1).then(|| table_info.columns[0].name.original.clone())
            })
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: "primary key column metadata missing".to_owned(),
            })?;
        let primary_buckets = buckets.remove(&primary_column).ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: format!("primary key on {primary_column} in buckets info not found"),
        })?;
        buckets.insert("PRIMARY".to_owned(), primary_buckets);
    }
    Ok(buckets)
}

/// 拆分桶边界字符串；时间类型若为 packed 整数则解码为可读时间。
pub fn AnalyzeValuesFromBuckets(value: &str, column_types: &[u8]) -> Result<Vec<String>, String> {
    let mut values: Vec<String> = value
        .trim_matches(['(', ')'])
        .split(", ")
        .map(str::to_owned)
        .collect();
    if values.len() != column_types.len() {
        return Err(format!("analyze value {value} failed"));
    }
    // 可读时间串已含 -/:；纯数字则按 packed 时间解码。
    for (value, column_type) in values.iter_mut().zip(column_types) {
        if IsTimeTypeAndNeedDecode(*column_type) && !is_time_string(value) {
            *value = DecodeTimeInBucket(value)?;
        }
    }
    Ok(values)
}

fn is_time_string(value: &str) -> bool {
    let mut parts = value.split([' ', ':', '-']);
    let components: Vec<_> = parts.by_ref().filter(|part| !part.is_empty()).collect();
    (components.len() == 3 || components.len() == 6)
        && components
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}
/// 将 TiDB 桶中 packed uint64 时间还原为 `YYYY-MM-DD HH:MM:SS[.us]`。
pub fn DecodeTimeInBucket(value: &str) -> Result<String, String> {
    let packed: u64 = value
        .parse()
        .map_err(|error| format!("invalid packed time: {error}"))?;
    if packed == 0 {
        return Ok(String::new());
    }
    let year_month = packed >> 46;
    let year = year_month / 13;
    let month = year_month % 13;
    let day = (packed >> 41) & 31;
    let hour = (packed >> 36) & 31;
    let minute = (packed >> 30) & 63;
    let second = (packed >> 24) & 63;
    let microsecond = packed & ((1 << 24) - 1);
    if microsecond == 0 {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        ))
    } else {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{microsecond:06}"
        ))
    }
}

/// 从 `SHOW MASTER STATUS` 的 Position 读取 TiDB 最新 TSO（时间戳预言）。
pub fn GetTidbLatestTSO(db: &dyn QueryExecutor) -> Result<i64, DbError> {
    let result = db.QueryContext("SHOW MASTER STATUS", &[])?;
    let index = result
        .columns
        .iter()
        .position(|column| column == "Position")
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "Position column missing".to_owned(),
        })?;
    result
        .rows
        .first()
        .and_then(|row| row.get(index))
        .and_then(as_i64)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "get secondary cluster's ts failed".to_owned(),
        })
}
/// 执行 `SELECT version()` 返回数据库版本字符串。
pub fn GetDBVersion(db: &dyn QueryExecutor) -> Result<String, DbError> {
    first_row(db.QueryContext("SELECT version()", &[])?)?
        .first()
        .and_then(as_string)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "can't get the database's version".to_owned(),
        })
}
/// `SHOW VARIABLES LIKE` 读取会话/系统变量值。
pub fn GetSessionVariable(db: &dyn QueryExecutor, variable: &str) -> Result<String, DbError> {
    let result = db.QueryContext(
        &format!("SHOW VARIABLES LIKE '{}'", variable.replace('\'', "''")),
        &[],
    )?;
    let mut value = String::new();
    for row in result.rows {
        if let Some(column) = row.get(1) {
            value = as_string(column).ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: format!("variable {variable} has invalid value"),
            })?;
        }
    }
    Ok(value)
}
/// 读取会话 `sql_mode`。
pub fn GetSQLMode(db: &dyn QueryExecutor) -> Result<String, DbError> {
    GetSessionVariable(db, "sql_mode")
}
/// 根据 version() 是否包含 `tidb` 判断是否为 TiDB。
pub fn IsTiDB(db: &dyn QueryExecutor) -> Result<bool, DbError> {
    Ok(GetDBVersion(db)?.to_ascii_lowercase().contains("tidb"))
}

/// 生成 `` `schema`.`table` ``，内部反引号翻倍转义。
pub fn TableName(schema: &str, table: &str) -> String {
    format!("`{}`.`{}`", escapeName(schema), escapeName(table))
}
/// 生成 `` `column` `` 标识符。
pub fn ColumnName(column: &str) -> String {
    format!("`{}`", escapeName(column))
}
/// MySQL 标识符转义：将 `` ` `` 替换为 `` `` ``。
pub fn escapeName(name: &str) -> String {
    name.replace('`', "``")
}
/// 日志用：把 SQL 中的 `?` 依次替换为带单引号的参数。
pub fn ReplacePlaceholder(template: &str, args: &[String]) -> String {
    let mut parts = template.split('?');
    let mut result = parts.next().unwrap_or_default().to_owned();
    for (index, part) in parts.enumerate() {
        if let Some(value) = args.get(index) {
            result.push('\'');
            result.push_str(value);
            result.push('\'');
        } else {
            result.push('?');
        }
        result.push_str(part);
    }
    result
}

/// 识别库/表/列/索引已存在等可幂等忽略的 DDL 错误码。
fn ignoreDDLError(error: &DbError) -> bool {
    matches!(error.code, 1007 | 1008 | 1050 | 1051 | 1060 | 1061)
}
fn ignoreError(error: &DbError) -> bool {
    ignoreDDLError(error)
}
/// 带重试执行 SQL；可忽略 DDL 错误直接成功，可重试错误短暂休眠后重试。
pub fn ExecSQLWithRetry(db: &dyn DBExecutor, sql: &str, args: &[Value]) -> Result<(), DbError> {
    let mut last = None;
    for attempt in 0..DefaultRetryTime {
        match db.ExecContext(sql, args) {
            Ok(_) => return Ok(()),
            Err(error) if ignoreError(&error) => return Ok(()),
            Err(error) if IsRetryableError(&error) => {
                last = Some(error);
                if attempt + 1 < DefaultRetryTime {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("retry loop always records an error"))
}
/// 在同一事务中顺序执行多条 SQL；任一步失败则 Rollback。
pub fn ExecuteSQLs(
    db: &dyn DBExecutor,
    sqls: &[String],
    args: &[Vec<Value>],
) -> Result<(), DbError> {
    if sqls.len() != args.len() {
        return Err(DbError {
            code: 0,
            sql_state: None,
            message: "sql and args length mismatch".to_owned(),
        });
    }
    let mut transaction = db.BeginTx()?;
    for (sql, args) in sqls.iter().zip(args) {
        if let Err(error) = transaction.ExecContext(sql, args) {
            let _ = transaction.Rollback();
            return Err(error);
        }
    }
    transaction.Commit()
}
/// 按 `DefaultDeleteRowsNum` 分批 DELETE，直到影响行数小于批量上限。
pub fn DeleteRows(
    db: &dyn DBExecutor,
    schema: &str,
    table: &str,
    where_clause: &str,
    args: &[Value],
) -> Result<(), DbError> {
    let sql = format!(
        "DELETE FROM {} WHERE {} limit {};",
        TableName(schema, table),
        where_clause,
        DefaultDeleteRowsNum
    );
    // 影响行数不足一批说明删完；否则继续下一轮 LIMIT 删除。
    loop {
        if db.ExecContext(&sql, args)? < DefaultDeleteRowsNum {
            return Ok(());
        }
    }
}

/// 轻量 parser 配置：目前仅缓存规范化后的 sql_mode 字符串。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParserConfig {
    pub sql_mode: String,
}
/// 规范化逗号分隔的 sql_mode 列表并封装为 `ParserConfig`。
pub fn getParser(sql_mode: &str) -> Result<ParserConfig, String> {
    const KNOWN_MODES: &[&str] = &[
        "REAL_AS_FLOAT",
        "PIPES_AS_CONCAT",
        "ANSI_QUOTES",
        "IGNORE_SPACE",
        "ONLY_FULL_GROUP_BY",
        "NO_UNSIGNED_SUBTRACTION",
        "NO_DIR_IN_CREATE",
        "POSTGRESQL",
        "ORACLE",
        "MSSQL",
        "DB2",
        "MAXDB",
        "NO_KEY_OPTIONS",
        "NO_TABLE_OPTIONS",
        "NO_FIELD_OPTIONS",
        "MYSQL323",
        "MYSQL40",
        "ANSI",
        "NO_AUTO_VALUE_ON_ZERO",
        "NO_BACKSLASH_ESCAPES",
        "STRICT_TRANS_TABLES",
        "STRICT_ALL_TABLES",
        "NO_ZERO_IN_DATE",
        "NO_ZERO_DATE",
        "INVALID_DATES",
        "ERROR_FOR_DIVISION_BY_ZERO",
        "TRADITIONAL",
        "NO_AUTO_CREATE_USER",
        "HIGH_NOT_PRECEDENCE",
        "NO_ENGINE_SUBSTITUTION",
        "PAD_CHAR_TO_FULL_LENGTH",
        "ALLOW_INVALID_DATES",
        "TIME_TRUNCATE_FRACTIONAL",
    ];
    let mut modes = Vec::new();
    for mode in sql_mode
        .split(',')
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    {
        let mode = mode.to_ascii_uppercase();
        if !KNOWN_MODES.contains(&mode.as_str()) {
            return Err(format!("invalid sql mode {sql_mode}"));
        }
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    Ok(ParserConfig {
        sql_mode: modes.join(","),
    })
}
/// 从数据库读取 sql_mode 后构造 `ParserConfig`。
pub fn GetParserForDB(db: &dyn QueryExecutor) -> Result<ParserConfig, DbError> {
    Ok(ParserConfig {
        sql_mode: GetSQLMode(db)?,
    })
}
