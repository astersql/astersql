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

// Lightning 公共工具：MySQL 连接、SQL 重试、标识符转义与表元数据辅助。
//
// 提供导入过程中常用的数据库访问抽象（`DBExecutor`/`Transaction`）、带重试的 SQL 封装、
// MySQL 标识符/字符串插值，以及基于表结构信息的自增列、索引 DDL 与行数跳过判断。

use crate::{CommonError, Context, IsRetryableError, TLSConfig};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

// 重试间隔：测试 10ms，生产 3s；并定义默认重试次数与 MySQL errno 常量
const retryTimeout: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(3)
};
const defaultMaxRetry: usize = 3;
const ERR_ACCESS_DENIED: u16 = 1045;
const ERR_DUP_UNIQUE: u16 = 1062;
const ERR_DUP_KEY_NAME: u16 = 1061;
const ERR_MULTIPLE_PRI_KEY: u16 = 1068;
const ERR_SPECIFIC_ACCESS_DENIED: u16 = 1227;
/// 列 Flag：自增位、主键位；`UnspecifiedLength` 表示索引列长度未指定。
pub const AUTO_INCREMENT_FLAG: u64 = 1 << 9;
pub const PRI_KEY_FLAG: u64 = 1 << 1;
pub const UnspecifiedLength: i32 = -1;

#[derive(Clone, Debug, Eq, PartialEq)]
/// SQL 绑定参数的简易表示，供 `DBExecutor` 查询/执行使用。
pub enum SQLValue {
    Null,
    Integer(i64),
    Unsigned(u64),
    String(String),
    Bytes(Vec<u8>),
    Boolean(bool),
}

/// 事务句柄：提交或回滚。
pub trait Transaction: Send {
    fn Commit(&mut self) -> Result<(), CommonError>;
    fn Rollback(&mut self) -> Result<(), CommonError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 查询结果：列名、行数据，以及迭代阶段可能产生的错误。
pub struct QueryRows {
    pub Columns: Vec<String>,
    pub Rows: Vec<Vec<String>>,
    pub IterationError: Option<CommonError>,
}

/// MySQL/TiDB 执行器抽象：Ping、查询、事务与 Exec。
pub trait DBExecutor: Send + Sync {
    fn Ping(&self) -> Result<(), CommonError> {
        Ok(())
    }

    fn Close(&self) -> Result<(), CommonError> {
        Ok(())
    }

    fn SetMaxIdleConns(&self, _count: usize) {}

    fn QueryRowContext(
        &self,
        ctx: &Context,
        query: &str,
        args: &[SQLValue],
    ) -> Result<Vec<String>, CommonError>;

    fn QueryContext(
        &self,
        ctx: &Context,
        query: &str,
        args: &[SQLValue],
    ) -> Result<QueryRows, CommonError>;

    fn BeginTx(&self, ctx: &Context) -> Result<Box<dyn Transaction>, CommonError>;

    fn ExecContext(
        &self,
        ctx: &Context,
        query: &str,
        args: &[SQLValue],
    ) -> Result<u64, CommonError>;
}

/// 可注入的 MySQL 连接工厂，便于测试替换真实驱动。
pub type MySQLConnector =
    Arc<dyn Fn(&MySQLConfig) -> Result<Arc<dyn DBExecutor>, CommonError> + Send + Sync>;

#[derive(Clone)]
/// go-sql-driver 风格的连接配置字段。
pub struct MySQLConfig {
    pub User: String,
    pub Passwd: String,
    pub Net: String,
    pub Addr: String,
    pub Params: HashMap<String, String>,
    pub MaxAllowedPacket: usize,
    pub TLS: Option<TLSConfig>,
    pub AllowFallbackToPlaintext: bool,
    pub Connector: Option<MySQLConnector>,
}

impl Default for MySQLConfig {
    fn default() -> Self {
        Self {
            User: String::new(),
            Passwd: String::new(),
            Net: "tcp".to_owned(),
            Addr: String::new(),
            Params: HashMap::new(),
            MaxAllowedPacket: 0,
            TLS: None,
            AllowFallbackToPlaintext: false,
            Connector: None,
        }
    }
}

#[derive(Clone, Default)]
/// Lightning 侧更高层的连接参数，可转换为 `MySQLConfig`。
pub struct MySQLConnectParam {
    pub Host: String,
    pub Port: i32,
    pub User: String,
    pub Password: String,
    pub SQLMode: String,
    pub MaxAllowedPacket: u64,
    pub TLSConfig: Option<TLSConfig>,
    pub AllowFallbackToPlaintext: bool,
    pub Net: String,
    pub Vars: HashMap<String, String>,
    pub Connector: Option<MySQLConnector>,
}

impl MySQLConnectParam {
    /// 转为驱动配置：强制 utf8mb4，并包装 sql_mode / 自定义 Vars。
    pub fn ToDriverConfig(&self) -> MySQLConfig {
        let mut params = HashMap::new();
        params.insert("charset".to_owned(), "utf8mb4".to_owned());
        params.insert("sql_mode".to_owned(), format!("'{}'", self.SQLMode));
        for (key, value) in &self.Vars {
            params.insert(key.clone(), format!("'{value}'"));
        }
        MySQLConfig {
            User: self.User.clone(),
            Passwd: self.Password.clone(),
            Net: if self.Net.is_empty() {
                "tcp".to_owned()
            } else {
                self.Net.clone()
            },
            Addr: join_host_port(&self.Host, self.Port),
            Params: params,
            MaxAllowedPacket: self.MaxAllowedPacket as usize,
            TLS: self.TLSConfig.clone(),
            AllowFallbackToPlaintext: self.AllowFallbackToPlaintext,
            Connector: self.Connector.clone(),
        }
    }

    /// 建立连接并将空闲连接数设为可用 CPU 并行度。
    pub fn Connect(&self) -> Result<Arc<dyn DBExecutor>, CommonError> {
        let mut config = self.ToDriverConfig();
        let db = ConnectMySQL(&mut config)?;
        db.SetMaxIdleConns(
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        );
        Ok(db)
    }
}

/// 拼接 host:port；IPv6 字面量自动加方括号。
fn join_host_port(host: &str, port: i32) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// 通过注入的 Connector 建连并 Ping；失败则 Close。
fn tryConnectMySQL(config: &MySQLConfig) -> Result<Arc<dyn DBExecutor>, CommonError> {
    let connector = config
        .Connector
        .as_ref()
        .ok_or_else(|| CommonError::new("mysql", "no MySQL connector has been configured"))?;
    let db = connector(config)?;
    if let Err(error) = db.Ping() {
        let _ = db.Close();
        return Err(error);
    }
    Ok(db)
}

/// 连接 MySQL；若密码疑似 base64 且遇 1045，则解码后重试一次。
pub fn ConnectMySQL(config: &mut MySQLConfig) -> Result<Arc<dyn DBExecutor>, CommonError> {
    // 首次失败且为访问拒绝时，尝试将密码当作 base64 解码后再连
    let first_error = match tryConnectMySQL(config) {
        Ok(db) => return Ok(db),
        Err(error) => error,
    };
    if first_error.Code == Some(ERR_ACCESS_DENIED)
        && let Ok(decoded) = decode_base64(&config.Passwd)
        && decoded != config.Passwd.as_bytes()
    {
        config.Passwd = String::from_utf8_lossy(&decoded).into_owned();
        if let Ok(db) = tryConnectMySQL(config) {
            return Ok(db);
        }
    }
    Err(first_error)
}

/// 手写 base64 解码，用于访问拒绝后的密码二次尝试。
fn decode_base64(input: &str) -> Result<Vec<u8>, CommonError> {
    if input.len() % 4 != 0 {
        return Err(CommonError::new("base64", "invalid base64 length"));
    }
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    let chunk_count = input.len() / 4;
    for (chunk_index, chunk) in input.as_bytes().chunks_exact(4).enumerate() {
        let a = value(chunk[0]).ok_or_else(|| CommonError::new("base64", "invalid base64"))?;
        let b = value(chunk[1]).ok_or_else(|| CommonError::new("base64", "invalid base64"))?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            value(chunk[2]).ok_or_else(|| CommonError::new("base64", "invalid base64"))?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            value(chunk[3]).ok_or_else(|| CommonError::new("base64", "invalid base64"))?
        };
        if chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(CommonError::new("base64", "invalid base64 padding"));
        }
        if (chunk[2] == b'=' || chunk[3] == b'=') && chunk_index + 1 != chunk_count {
            return Err(CommonError::new("base64", "invalid base64 padding"));
        }
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

/// 判断路径是否为已存在的目录。
pub fn IsDirExists(name: &str) -> bool {
    std::fs::metadata(name).is_ok_and(|metadata| metadata.is_dir())
}

/// 判断目录是否为空（可读且无条目）。
pub fn IsEmptyDir(name: &str) -> bool {
    std::fs::read_dir(name).is_ok_and(|mut entries| entries.next().is_none())
}

/// 对可重试错误最多执行 `defaultMaxRetry` 次；not-found 立即停止。
pub fn Retry<F>(purpose: &str, mut action: F) -> Result<(), CommonError>
where
    F: FnMut() -> Result<(), CommonError>,
{
    let mut last_error = None;
    for retry_count in 0..defaultMaxRetry {
        if retry_count > 0 {
            std::thread::sleep(retryTimeout);
        }
        match action() {
            Ok(()) => return Ok(()),
            Err(error) if is_not_found(&error) => {
                last_error = Some(error);
                break;
            }
            Err(error) if IsRetryableError(Some(&error)) => last_error = Some(error),
            Err(error) => {
                last_error = Some(error);
                break;
            }
        }
    }
    let mut error = last_error.unwrap_or_else(|| CommonError::new("retry", purpose));
    error.Message = format!("{purpose} failed: {}", error.Message);
    Err(error)
}

/// 识别“资源不存在”类错误，避免无意义重试。
fn is_not_found(error: &CommonError) -> bool {
    error.Kind == "not-found" || error.ID == "NotFound"
}

/// 在 `DBExecutor` 上包装重试逻辑的 SQL 助手。
pub struct SQLWithRetry {
    pub DB: Arc<dyn DBExecutor>,
    pub HideQueryLog: bool,
}

impl SQLWithRetry {
    /// 通用重试执行：成功时取出闭包产出的值。
    fn perform<T, F>(&self, purpose: &str, mut action: F) -> Result<T, CommonError>
    where
        F: FnMut() -> Result<T, CommonError>,
    {
        let mut result = None;
        // 将闭包成功值暂存到 result，由外层返回
        Retry(purpose, || {
            action().map(|value| {
                result = Some(value);
            })
        })?;
        Ok(result.expect("successful retry action must produce a value"))
    }

    /// 带重试的单行查询。
    pub fn QueryRow(
        &self,
        ctx: &Context,
        purpose: &str,
        query: &str,
        args: &[SQLValue],
    ) -> Result<Vec<String>, CommonError> {
        self.perform(purpose, || self.DB.QueryRowContext(ctx, query, args))
    }

    /// 带重试查询全部行，并校验行列数一致。
    pub fn QueryStringRows(
        &self,
        ctx: &Context,
        purpose: &str,
        query: &str,
    ) -> Result<Vec<Vec<String>>, CommonError> {
        self.perform(purpose, || {
            let rows = self.DB.QueryContext(ctx, query, &[])?;
            if let Some(error) = rows.IterationError {
                return Err(error);
            }
            if rows.Rows.iter().any(|row| row.len() != rows.Columns.len()) {
                return Err(CommonError::new(
                    "sql",
                    "row column count does not match query metadata",
                ));
            }
            Ok(rows.Rows)
        })
    }

    /// 在事务中执行 `action`：失败回滚，成功提交。
    pub fn Transact<F>(
        &self,
        ctx: &Context,
        purpose: &str,
        mut action: F,
    ) -> Result<(), CommonError>
    where
        F: FnMut(&Context, &mut dyn Transaction) -> Result<(), CommonError>,
    {
        self.perform(purpose, || {
            // 事务：action 失败则 Rollback，成功则 Commit
            let mut transaction = self.DB.BeginTx(ctx).map_err(|mut error| {
                error.Message = format!("begin transaction failed: {}", error.Message);
                error
            })?;
            if let Err(error) = action(ctx, transaction.as_mut()) {
                let _ = transaction.Rollback();
                return Err(error);
            }
            transaction.Commit().map_err(|mut error| {
                error.Message = format!("commit transaction failed: {}", error.Message);
                error
            })
        })
    }

    /// 带重试的 Exec。
    pub fn Exec(
        &self,
        ctx: &Context,
        purpose: &str,
        query: &str,
        args: &[SQLValue],
    ) -> Result<(), CommonError> {
        self.perform(purpose, || {
            self.DB.ExecContext(ctx, query, args).map(|_| ())
        })
    }
}

/// 判断错误是否为上下文取消。
pub fn IsContextCanceledError(error: Option<&CommonError>) -> bool {
    error.is_some_and(|error| {
        matches!(error.Kind.as_str(), "cancelled" | "context") && error.Message.contains("canceled")
    })
}

/// 生成转义后的 `schema.table` 限定表名。
pub fn UniqueTable(schema: &str, table: &str) -> String {
    format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
}

/// 批量转义标识符。
fn escapeIdentifiers(identifiers: &[&str]) -> Vec<String> {
    identifiers
        .iter()
        .map(|identifier| EscapeIdentifier(identifier))
        .collect()
}

/// 类似 sprintf：将 `%s` 替换为已转义标识符，`%%` 输出字面 `%`。
pub fn SprintfWithIdentifiers(format: &str, identifiers: &[&str]) -> String {
    let escaped = escapeIdentifiers(identifiers);
    let mut values = escaped.iter();
    let mut output = String::with_capacity(format.len() + escaped.len() * 2);
    // 手工解析 %s / %%，避免依赖完整 sprintf
    let mut chars = format.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%' && chars.peek() == Some(&'s') {
            chars.next();
            if let Some(value) = values.next() {
                output.push_str(value);
            } else {
                output.push_str("%s");
            }
        } else if character == '%' && chars.peek() == Some(&'%') {
            chars.next();
            output.push('%');
        } else {
            output.push(character);
        }
    }
    output
}

/// 将 `SprintfWithIdentifiers` 结果写入 writer。
pub fn FprintfWithIdentifiers<W: Write>(
    writer: &mut W,
    format: &str,
    identifiers: &[&str],
) -> io::Result<usize> {
    let rendered = SprintfWithIdentifiers(format, identifiers);
    writer.write_all(rendered.as_bytes())?;
    Ok(rendered.len())
}

/// 用反引号包裹并转义 MySQL 标识符。
pub fn EscapeIdentifier(identifier: &str) -> String {
    let mut builder = String::with_capacity(identifier.len() + 2);
    WriteMySQLIdentifier(&mut builder, identifier);
    builder
}

/// 向字符串追加反引号转义后的标识符。
pub fn WriteMySQLIdentifier(builder: &mut String, identifier: &str) {
    builder.push('`');
    for character in identifier.chars() {
        if character == '`' {
            builder.push_str("``");
        } else {
            builder.push(character);
        }
    }
    builder.push('`');
}

/// 将字符串插值为单引号包裹的 MySQL 字面量（单引号加倍）。
pub fn InterpolateMySQLString(value: &str) -> String {
    let mut builder = String::with_capacity(value.len() + 2);
    builder.push('\'');
    for character in value.chars() {
        if character == '\'' {
            builder.push_str("''");
        } else {
            builder.push(character);
        }
    }
    builder.push('\'');
    builder
}

/// 通过 INFORMATION_SCHEMA 检查表是否存在。
pub fn TableExists(
    ctx: &Context,
    db: &dyn DBExecutor,
    schema: &str,
    table: &str,
) -> Result<bool, CommonError> {
    const QUERY: &str =
        "SELECT 1 from INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?";
    match db.QueryRowContext(
        ctx,
        QUERY,
        &[
            SQLValue::String(schema.to_owned()),
            SQLValue::String(table.to_owned()),
        ],
    ) {
        Ok(_) => Ok(true),
        Err(error) if error.Kind == "no-rows" => Ok(false),
        Err(mut error) => {
            error.Message = format!("check table exists failed: {}", error.Message);
            Err(error)
        }
    }
}

/// 通过 INFORMATION_SCHEMA 检查 schema 是否存在。
pub fn SchemaExists(ctx: &Context, db: &dyn DBExecutor, schema: &str) -> Result<bool, CommonError> {
    const QUERY: &str = "SELECT 1 from INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?";
    match db.QueryRowContext(ctx, QUERY, &[SQLValue::String(schema.to_owned())]) {
        Ok(_) => Ok(true),
        Err(error) if error.Kind == "no-rows" => Ok(false),
        Err(mut error) => {
            error.Message = format!("check schema exists failed: {}", error.Message);
            Err(error)
        }
    }
}

/// 向当前进程发送 SIGINT（Unix）。
#[cfg(unix)]
pub fn KillMySelf() -> Result<(), CommonError> {
    let status = std::process::Command::new("kill")
        .args(["-INT", &std::process::id().to_string()])
        .status()
        .map_err(|error| CommonError::new("signal", error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(CommonError::new("signal", "failed to send SIGINT"))
    }
}

/// 非 Unix 平台：KillMySelf 返回不支持错误。
#[cfg(not(unix))]
pub fn KillMySelf() -> Result<(), CommonError> {
    Err(CommonError::new(
        "signal",
        "signaling the current process is unsupported on this platform",
    ))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键值对及可选 RowID，用于编码结果传递。
pub struct KvPair {
    pub Key: Vec<u8>,
    pub Val: Vec<u8>,
    pub RowID: Vec<u8>,
}

/// 将整数 RowID 编码为 TiDB 紧凑整型字节格式。
pub fn EncodeIntRowID(row_id: i64) -> Vec<u8> {
    // 负 RowID：变长补码前缀；非负：单字节或长度前缀紧凑编码
    if row_id < 0 {
        let mut length = 1usize;
        while length < 8 && row_id < -((1_i128 << (length * 8)) - 1) as i64 {
            length += 1;
        }
        let mut encoded = Vec::with_capacity(length + 1);
        encoded.push(8 - length as u8);
        let bytes = row_id.to_be_bytes();
        encoded.extend_from_slice(&bytes[8 - length..]);
        return encoded;
    }
    let value = row_id as u64;
    if value <= 239 {
        return vec![value as u8 + 8];
    }
    let length = ((64 - value.leading_zeros() as usize) + 7) / 8;
    let mut encoded = Vec::with_capacity(length + 1);
    encoded.push(247 + length as u8);
    encoded.extend_from_slice(&value.to_be_bytes()[8 - length..]);
    encoded
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 大小写不敏感字符串：保留原始 `O` 与小写 `L`。
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    /// 由原始字符串构造，同时生成小写形式。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        Self {
            L: original.to_lowercase(),
            O: original,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表列摘要：名称、隐藏/生成列表达式与 Flag 位。
pub struct ColumnInfo {
    pub Name: CIStr,
    pub Hidden: bool,
    pub GeneratedExprString: String,
    pub Flag: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引中的一列：名称、在表中的偏移与前缀长度。
pub struct IndexColumn {
    pub Name: CIStr,
    pub Offset: usize,
    pub Length: i32,
}

impl Default for IndexColumn {
    fn default() -> Self {
        Self {
            Name: CIStr::default(),
            Offset: 0,
            Length: UnspecifiedLength,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// DDL 状态子集：仅区分非公开与 Public。
pub enum SchemaState {
    #[default]
    None,
    Public,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 索引元数据摘要。
pub struct IndexInfo {
    pub Name: CIStr,
    pub Columns: Vec<IndexColumn>,
    pub State: SchemaState,
    pub Primary: bool,
    pub Unique: bool,
    pub Invisible: bool,
    pub Comment: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表结构摘要：主键形态、自增/自随机、列与索引列表。
pub struct SchemaTableInfo {
    pub PKIsHandle: bool,
    pub IsCommonHandle: bool,
    pub ContainsAutoRandomBits: bool,
    pub HasAutoIncrement: bool,
    pub HasClusteredIndex: bool,
    pub Columns: Vec<ColumnInfo>,
    pub Indices: Vec<IndexInfo>,
}

/// 非整型主键且非 common handle 时，表拥有隐式 `_tidb_rowid`。
fn table_has_auto_row_id(info: &SchemaTableInfo) -> bool {
    !info.PKIsHandle && !info.IsCommonHandle
}

/// 表是否需要维护自增/自随机/隐式 RowID 一类自动 ID。
pub fn TableHasAutoID(info: &SchemaTableInfo) -> bool {
    table_has_auto_row_id(info) || info.HasAutoIncrement || info.ContainsAutoRandomBits
}

/// 定位 AUTO_RANDOM 列（整型主键或 common handle 主键首列）。
pub fn GetAutoRandomColumn(info: &SchemaTableInfo) -> Option<&ColumnInfo> {
    if !info.ContainsAutoRandomBits {
        return None;
    }
    if info.PKIsHandle {
        return info
            .Columns
            .iter()
            .find(|column| column.Flag & PRI_KEY_FLAG != 0);
    }
    if info.IsCommonHandle {
        let primary = info.Indices.iter().find(|index| index.Primary)?;
        return primary
            .Columns
            .first()
            .and_then(|column| info.Columns.get(column.Offset));
    }
    None
}

/// 划分导入后可保留与可删除的索引（自增相关索引保留）。
pub fn GetDropIndexInfos(info: &SchemaTableInfo) -> (Vec<&IndexInfo>, Vec<&IndexInfo>) {
    let mut remain_indexes = Vec::new();
    let mut drop_indexes = Vec::new();
    // Public 且非聚簇主键、且不含自增列的索引可在导入后删除重建
    'indexes: for index in &info.Indices {
        if index.State != SchemaState::Public || index.Primary && info.HasClusteredIndex {
            remain_indexes.push(index);
            continue;
        }
        for index_column in &index.Columns {
            if info
                .Columns
                .get(index_column.Offset)
                .is_some_and(|column| column.Flag & AUTO_INCREMENT_FLAG != 0)
            {
                remain_indexes.push(index);
                continue 'indexes;
            }
        }
        drop_indexes.push(index);
    }
    (remain_indexes, drop_indexes)
}

/// 生成 DROP PRIMARY KEY / DROP INDEX SQL。
pub fn BuildDropIndexSQL(db_name: &str, table_name: &str, index: &IndexInfo) -> String {
    if index.Primary {
        SprintfWithIdentifiers("ALTER TABLE %s.%s DROP PRIMARY KEY", &[db_name, table_name])
    } else {
        SprintfWithIdentifiers(
            "ALTER TABLE %s.%s DROP INDEX %s",
            &[db_name, table_name, &index.Name.O],
        )
    }
}

/// 对比当前与目标表结构，生成批量或逐条 ADD INDEX SQL。
pub fn BuildAddIndexSQL(
    table_name: &str,
    current: &SchemaTableInfo,
    desired: &SchemaTableInfo,
) -> (String, Vec<String>) {
    let mut specs = Vec::with_capacity(desired.Indices.len());
    // 跳过目标中已存在于 current 的同名索引，组装 ADD 子句
    'desired_indexes: for index in &desired.Indices {
        if current
            .Indices
            .iter()
            .any(|existing| existing.Name.L == index.Name.L)
        {
            continue 'desired_indexes;
        }
        let mut spec = if index.Primary {
            "ADD PRIMARY KEY ".to_owned()
        } else if index.Unique {
            "ADD UNIQUE KEY ".to_owned()
        } else {
            "ADD KEY ".to_owned()
        };
        if index.Name.L != "primary" {
            spec.push_str(&EscapeIdentifier(&index.Name.O));
        }
        let mut columns = Vec::with_capacity(index.Columns.len());
        for column in &index.Columns {
            let info = &desired.Columns[column.Offset];
            let rendered = if info.Hidden {
                format!("({})", info.GeneratedExprString)
            } else if column.Length != UnspecifiedLength {
                format!("{}({})", EscapeIdentifier(&column.Name.O), column.Length)
            } else {
                EscapeIdentifier(&column.Name.O)
            };
            columns.push(rendered);
        }
        spec.push_str(&format!("({})", columns.join(",")));
        if index.Invisible {
            spec.push_str(" INVISIBLE");
        }
        if !index.Comment.is_empty() {
            spec.push_str(&format!(" COMMENT '{}'", output_format(&index.Comment)));
        }
        specs.push(spec);
    }
    if specs.is_empty() {
        return (String::new(), Vec::new());
    }
    let single = format!("ALTER TABLE {table_name} {}", specs.join(", "));
    let multiple = specs
        .iter()
        .map(|spec| format!("ALTER TABLE {table_name} {spec}"))
        .collect();
    (single, multiple)
}

/// Match `format.OutputFormat`, which is used by the Go implementation when
/// rendering index comments into SQL.
fn output_format(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\0' => output.push_str("\\0"),
            '\'' => output.push_str("''"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\\' => output.push_str("\\\\"),
            _ => output.push(character),
        }
    }
    output
}

/// 是否为重复键名/多重主键/唯一冲突类 MySQL 错误。
pub fn IsDupKeyError(error: &CommonError) -> bool {
    matches!(
        error.Code,
        Some(ERR_DUP_KEY_NAME | ERR_MULTIPLE_PRI_KEY | ERR_DUP_UNIQUE)
    )
}

// TiDB 会话变量名常量
const TIDB_BACKOFF_WEIGHT: &str = "tidb_backoff_weight";
const PD_ENABLE_FOLLOWER_HANDLE_REGION: &str = "pd_enable_follower_handle_region";
const TIDB_EXPLICIT_REQUEST_SOURCE_TYPE: &str = "tidb_explicit_request_source_type";

/// 读取 `tidb_backoff_weight` 会话变量。
pub fn GetBackoffWeightFromDB(ctx: &Context, db: &dyn DBExecutor) -> Result<i32, CommonError> {
    getSessionVariable(ctx, db, TIDB_BACKOFF_WEIGHT)?
        .parse()
        .map_err(|error: std::num::ParseIntError| CommonError::new("parse", error.to_string()))
}

/// 读取 PD follower 处理 Region 开关是否开启。
pub fn GetPDEnableFollowerHandleRegion(
    ctx: &Context,
    db: &dyn DBExecutor,
) -> Result<bool, CommonError> {
    Ok(tidb_opt_on(&getSessionVariable(
        ctx,
        db,
        PD_ENABLE_FOLLOWER_HANDLE_REGION,
    )?))
}

/// 读取显式请求来源类型会话变量。
pub fn GetExplicitRequestSourceTypeFromDB(
    ctx: &Context,
    db: &dyn DBExecutor,
) -> Result<String, CommonError> {
    getSessionVariable(ctx, db, TIDB_EXPLICIT_REQUEST_SOURCE_TYPE)
}

/// 通过 `SHOW VARIABLES LIKE` 读取单个会话变量值。
fn getSessionVariable(
    ctx: &Context,
    db: &dyn DBExecutor,
    variable: &str,
) -> Result<String, CommonError> {
    let query = format!("SHOW VARIABLES LIKE '{}'", variable.replace('\'', "''"));
    let rows = db.QueryContext(ctx, &query, &[])?;
    if let Some(error) = rows.IterationError {
        return Err(error);
    }
    let mut value = String::new();
    for row in rows.Rows {
        if row.len() < 2 {
            return Err(CommonError::new(
                "sql",
                "SHOW VARIABLES returned fewer than two columns",
            ));
        }
        value = row[1].clone();
    }
    Ok(value)
}

/// TiDB 布尔选项：`1` 或 `on`（忽略大小写）视为开启。
fn tidb_opt_on(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("on")
}

/// 判断错误是否表示函数不存在或未选中数据库。
pub fn IsFunctionNotExistErr(error: Option<&CommonError>, function_name: &str) -> bool {
    error.is_some_and(|error| {
        error.Message.contains("No database selected")
            || error
                .Message
                .contains(&format!("{function_name} does not exist"))
    })
}

/// 通过 `show config` 判断 TiKV 是否使用 raft-kv2 存储引擎。
pub fn IsRaftKV2(ctx: &Context, db: &dyn DBExecutor) -> Result<bool, CommonError> {
    const QUERY: &str = "show config where type = 'tikv' and name = 'storage.engine'";
    let rows = db.QueryContext(ctx, QUERY, &[])?;
    if let Some(error) = rows.IterationError {
        return Err(error);
    }
    Ok(rows
        .Rows
        .iter()
        .any(|row| row.get(3).is_some_and(|value| value == "raft-kv2")))
}

/// 是否因缺少 CONFIG 权限导致的 1227 错误。
pub fn IsAccessDeniedNeedConfigPrivilegeError(error: &CommonError) -> bool {
    error.Code == Some(ERR_SPECIFIC_ACCESS_DENIED) && error.Message.contains("CONFIG")
}

/// 若表无自动 ID / 自增主键相关结构，可跳过读取行数以加速导入准备。
pub fn SkipReadRowCount(info: Option<&SchemaTableInfo>) -> bool {
    // 无隐式 RowID/自随机，且主键/唯一键不含自增列时，可跳过行数统计
    let Some(info) = info else {
        return false;
    };
    if table_has_auto_row_id(info) || info.ContainsAutoRandomBits {
        return false;
    }
    for index in &info.Indices {
        if !index.Unique || !index.Primary {
            continue;
        }
        for column in &index.Columns {
            if info
                .Columns
                .get(column.Offset)
                .is_some_and(|column| column.Flag & AUTO_INCREMENT_FLAG != 0)
            {
                return false;
            }
        }
    }
    for column in &info.Columns {
        if column.Flag & PRI_KEY_FLAG != 0 && column.Flag & AUTO_INCREMENT_FLAG != 0 {
            return false;
        }
    }
    true
}

/// 数据块刷盘状态查询接口。
pub trait ChunkFlushStatus {
    fn Flushed(&self) -> bool;
}
