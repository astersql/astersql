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
// Copyright 2026 AsterSQL.

// Lightning TiDB 逻辑导入后端。
//
// 将行编码为 INSERT/REPLACE SQL（或预编译占位符），经 SqlExecutor 写入目标 TiDB，
// 而非直接写 TiKV。支持重复键策略、批量分块、错误预算降级与远程表模型拉取。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use backend::{
    Backend, BackendError, CheckCtx, ChunkFlushStatus, ColumnInfo, DatabaseInfo, EngineConfig,
    EngineWriter, LocalWriterConfig, TableInfo, TargetInfoGetter,
};
use encode::{
    Column, Context, Datum, EncodeError, Encoder, EncodingBuilder, EncodingConfig, Row, Rows,
};
use uuid::Uuid;
use verification::{KVChecksum, MakeKVChecksum};

/// 批量写行最大重试次数。
const writeRowsMaxRetryTimes: usize = 3;
/// 预编译语句缓存容量上限。
const prepStmtCacheSize: usize = 100;
/// 拉取远程表模型的建议并发度。
pub const fetchRemoteTableModelsConcurrency: usize = 8;
/// 单次查询远程表模型的批大小。
pub const fetchRemoteTableModelsBatchSize: usize = 32;
/// SQL mode：禁用反斜杠转义（字面量中 `\` 原样保留）。
pub const SQL_MODE_NO_BACKSLASH_ESCAPES: u64 = 1 << 20;
/// SQL mode：严格事务表。
pub const SQL_MODE_STRICT_TRANS_TABLES: u64 = 1 << 21;
/// SQL mode：严格全部表。
pub const SQL_MODE_STRICT_ALL_TABLES: u64 = 1 << 22;
const SQL_MODE_STRICT: u64 = SQL_MODE_STRICT_TRANS_TABLES | SQL_MODE_STRICT_ALL_TABLES;

#[derive(Clone, Debug, PartialEq)]
/// SQL 绑定参数 / 查询结果中的标量值。
pub enum SqlValue {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// SQL 执行错误：可重试、重复键等标志。
pub struct SqlError {
    pub message: String,
    pub retryable: bool,
    pub duplicate: bool,
}

impl SqlError {
    /// 普通错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            duplicate: false,
        }
    }
    /// 可重试错误。
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            duplicate: false,
        }
    }
    /// 重复键（duplicate entry）错误。
    pub fn duplicate(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            duplicate: true,
        }
    }
}

impl fmt::Display for SqlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// 抽象 SQL 执行器（生产环境对接真实 MySQL/TiDB 连接）。
pub trait SqlExecutor: Send + Sync {
    fn execute(&self, query: &str, values: &[SqlValue]) -> Result<u64, SqlError>;
    fn query(&self, query: &str, values: &[SqlValue]) -> Result<Vec<Vec<SqlValue>>, SqlError>;
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单行逻辑导入载荷：字面量 INSERT 片段、预编译片段与绑定值。
pub struct tidbRow {
    pub insertStmt: String,
    pub preparedInsertStmt: String,
    pub values: Vec<SqlValue>,
    pub path: String,
    pub offset: i64,
}

impl tidbRow {
    /// 以字面量语句长度作为行大小估计。
    pub fn Size(&self) -> u64 {
        self.insertStmt.len() as u64
    }
    /// 返回字面量 INSERT 值列表片段。
    pub fn String(&self) -> &str {
        &self.insertStmt
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 多行 tidbRow 集合。
pub struct tidbRows(pub Vec<tidbRow>);

impl tidbRows {
    /// 日志用：各行 insertStmt 列表。
    pub fn MarshalLogArray(&self) -> Vec<String> {
        self.0.iter().map(|row| row.insertStmt.clone()).collect()
    }

    /// 按字节上限与行数上限切块，便于批量写入。
    pub fn splitIntoChunks(&self, splitSize: u64, splitRows: usize) -> Vec<tidbRows> {
        if self.0.is_empty() {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut current = Vec::new();
        let mut size = 0;
        for row in &self.0 {
            if !current.is_empty()
                && (size + row.Size() > splitSize || current.len() >= splitRows.max(1))
            {
                result.push(tidbRows(std::mem::take(&mut current)));
                size = 0;
            }
            size += row.Size();
            current.push(row.clone());
        }
        result.push(tidbRows(current));
        result
    }
}

impl Rows for tidbRows {
    fn Clear(mut self: Box<Self>) -> Box<dyn Rows> {
        self.0.clear();
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Row for tidbRow {
    fn ClassifyAndAppend(
        &self,
        data: &mut Box<dyn Rows>,
        dataChecksum: &mut KVChecksum,
        _indices: &mut Box<dyn Rows>,
        _indexChecksum: &mut KVChecksum,
    ) {
        let rows = data
            .as_any_mut()
            .downcast_mut::<tidbRows>()
            .expect("TiDB rows expected");
        rows.0.push(self.clone());
        dataChecksum.Add(&MakeKVChecksum(self.Size(), 1, 0));
    }
    fn Size(&self) -> u64 {
        self.Size()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// TiDB 后端 EncodingBuilder：产出 tidbEncoder / 空 tidbRows。
pub struct encodingBuilder;

/// 构造 EncodingBuilder 实例。
pub fn NewEncodingBuilder() -> Box<dyn EncodingBuilder> {
    Box::new(encodingBuilder)
}

impl EncodingBuilder for encodingBuilder {
    fn NewEncoder(
        &self,
        _: &Context,
        config: &EncodingConfig,
    ) -> Result<Box<dyn Encoder>, EncodeError> {
        Ok(Box::new(tidbEncoder {
            mode: config.SessionOptions.SQLMode,
            columns: config
                .Table
                .as_ref()
                .map(|table| table.columns().to_vec())
                .unwrap_or_default(),
            columnIndices: Vec::new(),
            columnCount: 0,
            path: config.Path.clone(),
            preparedStatement: config.SessionOptions.LogicalImportPrepStmt,
        }))
    }
    fn MakeEmptyRows(&self) -> Box<dyn Rows> {
        Box::new(tidbRows::default())
    }
}

/// 将 Datum 行编码为 SQL 值列表（字面量或 `?` 占位）。
pub struct tidbEncoder {
    mode: u64,
    columns: Vec<Column>,
    columnIndices: Vec<i32>,
    columnCount: usize,
    path: String,
    preparedStatement: bool,
}

impl tidbEncoder {
    /// 按 SQL mode 转义字节并加单引号。
    fn appendSQLBytes(&self, output: &mut String, value: &[u8]) {
        output.push('\'');
        for byte in value {
            // NO_BACKSLASH_ESCAPES：仅加倍单引号，不处理反斜杠。
            if self.mode & SQL_MODE_NO_BACKSLASH_ESCAPES != 0 {
                if *byte == b'\'' {
                    output.push_str("''");
                } else {
                    output.push(*byte as char);
                }
            } else {
                match *byte {
                    0 => output.push_str("\\0"),
                    b'\x08' => output.push_str("\\b"),
                    b'\n' => output.push_str("\\n"),
                    b'\r' => output.push_str("\\r"),
                    b'\t' => output.push_str("\\t"),
                    26 => output.push_str("\\Z"),
                    b'\'' => output.push_str("''"),
                    b'\\' => output.push_str("\\\\"),
                    value => output.push(value as char),
                }
            }
        }
        output.push('\'');
    }

    /// 将 Datum 追加为 SQL 字面量。
    fn appendSQL(
        &self,
        output: &mut String,
        datum: &Datum,
        column: Option<&Column>,
    ) -> Result<(), EncodeError> {
        if self.mode & SQL_MODE_STRICT != 0 {
            if let Some(column) = column {
                let bytes = match datum {
                    Datum::Bytes(value) | Datum::BinaryLiteral(value) | Datum::Bit(value) => {
                        Some(value.as_slice())
                    }
                    Datum::String(value) | Datum::Json(value) => Some(value.as_bytes()),
                    _ => None,
                };
                if let Some(bytes) = bytes {
                    match column.charset.to_ascii_lowercase().as_str() {
                        "ascii" if !bytes.is_ascii() => {
                            return Err(EncodeError(format!(
                                "incorrect ascii value {:?} for column {}",
                                bytes, column.name
                            )));
                        }
                        "utf8" | "utf8mb4" if std::str::from_utf8(bytes).is_err() => {
                            return Err(EncodeError(format!(
                                "incorrect utf8 value {:?} for column {}",
                                bytes, column.name
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }
        match datum {
            Datum::Null => output.push_str("NULL"),
            Datum::MinNotNull => output.push_str("MINVALUE"),
            Datum::MaxValue => output.push_str("MAXVALUE"),
            Datum::Int(value) => output.push_str(&value.to_string()),
            Datum::UInt(value) | Datum::Enum { value, .. } | Datum::Set { value, .. } => {
                output.push_str(&value.to_string())
            }
            Datum::Float(value) => output.push_str(&formatGoFloat(*value)),
            Datum::Bytes(value) => self.appendSQLBytes(output, value),
            Datum::String(value) | Datum::Json(value) => {
                self.appendSQLBytes(output, value.as_bytes())
            }
            Datum::BinaryLiteral(value) => {
                output.push_str("x'");
                for byte in value {
                    use std::fmt::Write;
                    write!(output, "{byte:02x}").expect("writing to String cannot fail");
                }
                output.push('\'');
            }
            Datum::Bit(value) => {
                let integer = value
                    .iter()
                    .try_fold(0_u64, |accumulator, byte| {
                        accumulator
                            .checked_mul(256)
                            .and_then(|value| value.checked_add(u64::from(*byte)))
                    })
                    .ok_or_else(|| EncodeError("binary literal is too large for BIT".into()))?;
                output.push_str(&integer.to_string());
            }
            Datum::Decimal(value) | Datum::Timestamp(value) | Datum::Duration(value) => {
                output.push('\'');
                output.push_str(value);
                output.push('\'');
            }
        }
        Ok(())
    }

    /// 按表列下标取列定义。
    fn getColumnByIndex(&self, index: usize) -> Option<&Column> {
        self.columns.get(index)
    }
}

impl Encoder for tidbEncoder {
    fn Close(&mut self) {}

    fn Encode(
        &mut self,
        row: &[Datum],
        _rowID: i64,
        columnPermutation: &[i32],
        offset: i64,
    ) -> Result<Box<dyn Row>, EncodeError> {
        // 首次编码时建立「输入列下标 → 表列下标」映射。
        if self.columnIndices.is_empty() {
            self.columnIndices = vec![-1; columnPermutation.len()];
            let mut maximum = -1;
            for (tableIndex, inputIndex) in columnPermutation.iter().copied().enumerate() {
                if inputIndex >= 0 {
                    if inputIndex as usize >= self.columnIndices.len() {
                        return Err(EncodeError("column permutation out of range".into()));
                    }
                    self.columnIndices[inputIndex as usize] = tableIndex as i32;
                    maximum = maximum.max(inputIndex);
                }
            }
            self.columnCount = (maximum + 1) as usize;
        }
        if row.len() < self.columnCount {
            return Err(EncodeError(format!(
                "column count mismatch, expected {}, got {}",
                self.columnCount,
                row.len()
            )));
        }
        if row.len() > self.columnIndices.len() {
            return Err(EncodeError(format!(
                "column count mismatch, at most {} but got {}",
                self.columnIndices.len(),
                row.len()
            )));
        }
        let mut literal = "(".to_string();
        let mut prepared = if self.preparedStatement {
            "(".to_string()
        } else {
            String::new()
        };
        let mut values = Vec::new();
        let mut written = 0;
        for (inputIndex, datum) in row.iter().enumerate() {
            let tableIndex = self.columnIndices[inputIndex];
            if tableIndex < 0 {
                continue;
            }
            if written > 0 {
                literal.push(',');
                if self.preparedStatement {
                    prepared.push(',');
                }
            }
            self.appendSQL(
                &mut literal,
                datum,
                self.getColumnByIndex(tableIndex as usize),
            )?;
            if self.preparedStatement {
                prepared.push('?');
                values.push(datumToSqlValue(datum));
            }
            written += 1;
        }
        literal.push(')');
        if self.preparedStatement {
            prepared.push(')');
        }
        Ok(Box::new(tidbRow {
            insertStmt: literal,
            preparedInsertStmt: prepared,
            values,
            path: self.path.clone(),
            offset,
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Datum → 预编译绑定 SqlValue。
fn datumToSqlValue(datum: &Datum) -> SqlValue {
    match datum {
        Datum::Null => SqlValue::Null,
        Datum::MinNotNull => SqlValue::String("MINVALUE".into()),
        Datum::MaxValue => SqlValue::String("MAXVALUE".into()),
        Datum::Int(value) => SqlValue::Int(*value),
        Datum::UInt(value) | Datum::Enum { value, .. } | Datum::Set { value, .. } => {
            SqlValue::UInt(*value)
        }
        Datum::Float(value) => SqlValue::Float(*value),
        Datum::Bytes(value) | Datum::BinaryLiteral(value) | Datum::Bit(value) => {
            SqlValue::Bytes(value.clone())
        }
        Datum::String(value)
        | Datum::Json(value)
        | Datum::Decimal(value)
        | Datum::Timestamp(value)
        | Datum::Duration(value) => SqlValue::String(value.clone()),
    }
}

/// 对齐 Go `strconv.AppendFloat(value, 'g', -1, 64)` 的常见 SQL 输出。
fn formatGoFloat(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "+Inf".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Inf".into();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }

    // Debug 使用 Rust 的 shortest-roundtrip 浮点格式；补齐 Go 对正指数的 `+`。
    let mut output = format!("{value:?}");
    if let Some(exponent) = output.find('e') {
        if !matches!(output.as_bytes().get(exponent + 1), Some(b'+' | b'-')) {
            output.insert(exponent + 1, '+');
        }
    } else if output.ends_with(".0") {
        output.truncate(output.len() - 2);
    }
    output
}

/// 诊断用：将一行编码为字面量值列表字符串。
pub fn EncodeRowForRecord(
    table: Option<Arc<dyn encode::Table>>,
    sqlMode: u64,
    row: &[Datum],
    columnPermutation: &[i32],
) -> Vec<u8> {
    let config = EncodingConfig {
        SessionOptions: encode::SessionOptions {
            SQLMode: sqlMode,
            ..Default::default()
        },
        Table: table,
        ..Default::default()
    };
    let mut encoder = encodingBuilder
        .NewEncoder(&Context::default(), &config)
        .unwrap();
    encoder
        .Encode(row, 0, columnPermutation, 0)
        .map(|row| {
            row.as_any()
                .downcast_ref::<tidbRow>()
                .unwrap()
                .insertStmt
                .as_bytes()
                .to_vec()
        })
        .unwrap_or_else(|_| formatDatumsForRecord(row))
}

fn formatDatumsForRecord(row: &[Datum]) -> Vec<u8> {
    let mut output = vec![b'('];
    for (index, datum) in row.iter().enumerate() {
        if index > 0 {
            output.extend_from_slice(b", ");
        }
        match datum {
            Datum::Null => output.extend_from_slice(b"NULL"),
            Datum::MinNotNull => output.extend_from_slice(b"MINVALUE"),
            Datum::MaxValue => output.extend_from_slice(b"MAXVALUE"),
            Datum::Int(value) => output.extend_from_slice(value.to_string().as_bytes()),
            Datum::UInt(value) | Datum::Enum { value, .. } | Datum::Set { value, .. } => {
                output.extend_from_slice(value.to_string().as_bytes());
            }
            Datum::Float(value) => output.extend_from_slice(formatGoFloat(*value).as_bytes()),
            Datum::String(value)
            | Datum::Json(value)
            | Datum::Decimal(value)
            | Datum::Timestamp(value)
            | Datum::Duration(value) => {
                output.extend_from_slice(format!("{value:?}").as_bytes());
            }
            Datum::Bytes(value) | Datum::BinaryLiteral(value) | Datum::Bit(value) => {
                output.extend_from_slice(value);
            }
        }
    }
    output.push(b')');
    output
}

/// 从目标库拉取库/表元信息的 TargetInfoGetter 实现。
pub struct targetInfoGetter {
    db: Arc<dyn SqlExecutor>,
}

/// 构造远程目标信息获取器。
pub fn NewTargetInfoGetter(db: Arc<dyn SqlExecutor>) -> Box<dyn TargetInfoGetter> {
    Box::new(targetInfoGetter { db })
}

impl TargetInfoGetter for targetInfoGetter {
    fn FetchRemoteDBModels(&self, _: &Context) -> Result<Vec<DatabaseInfo>, BackendError> {
        self.db
            .query("SHOW DATABASES", &[])
            .map_err(sqlToBackend)?
            .into_iter()
            .map(|row| match row.as_slice() {
                [SqlValue::String(name)] => Ok(DatabaseInfo { name: name.clone() }),
                _ => Err(BackendError::new("unexpected SHOW DATABASES result")),
            })
            .collect()
    }

    fn FetchRemoteTableModels(
        &self,
        _: &Context,
        schemaName: &str,
        tableNames: &[String],
    ) -> Result<HashMap<String, TableInfo>, BackendError> {
        let mut output = HashMap::new();
        for batch in tableNames.chunks(fetchRemoteTableModelsBatchSize) {
            let values = std::iter::once(SqlValue::String(schemaName.into()))
                .chain(batch.iter().cloned().map(SqlValue::String))
                .collect::<Vec<_>>();
            for row in self
                .db
                .query(
                    &format!(
                        "SELECT table_name, column_name, column_type, generation_expression, extra \
                         FROM information_schema.columns \
                         WHERE table_schema = ? AND table_name IN ({}) \
                         ORDER BY table_name, ordinal_position;",
                        std::iter::repeat_n("?", batch.len())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    &values,
                )
                .map_err(sqlToBackend)?
            {
                let [
                    SqlValue::String(table_name),
                    SqlValue::String(column_name),
                    SqlValue::String(column_type),
                    SqlValue::String(generated_expression),
                    SqlValue::String(extra),
                ] = row.as_slice()
                else {
                    return Err(BackendError::new(
                        "unexpected information_schema.columns result",
                    ));
                };
                let table = output
                    .entry(table_name.to_ascii_lowercase())
                    .or_insert_with(|| TableInfo {
                        name: table_name.clone(),
                        public: true,
                        pk_is_handle: true,
                        ..Default::default()
                    });
                table.columns.push(ColumnInfo {
                    name: column_name.clone(),
                    offset: table.columns.len(),
                    public: true,
                    unsigned: column_type
                        .trim_end()
                        .to_ascii_lowercase()
                        .ends_with("unsigned"),
                    auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
                    generated_expression: generated_expression.clone(),
                    ..Default::default()
                });
            }

            let fetched_tables = batch
                .iter()
                .filter_map(|requested| {
                    output
                        .get(&requested.to_ascii_lowercase())
                        .map(|table| table.name.clone())
                })
                .collect::<Vec<_>>();
            for table_name in fetched_tables {
                let qualified_name = format!(
                    "`{}`.`{}`",
                    schemaName.replace('`', "``"),
                    table_name.replace('`', "``")
                );
                let auto_ids = match FetchTableAutoIDInfos(self.db.as_ref(), &qualified_name) {
                    Ok(auto_ids) => auto_ids,
                    Err(_) => {
                        output.remove(&table_name.to_ascii_lowercase());
                        continue;
                    }
                };
                let Some(table) = output.get_mut(&table_name.to_ascii_lowercase()) else {
                    continue;
                };
                for auto_id in auto_ids {
                    let Some(column) = table
                        .columns
                        .iter_mut()
                        .find(|column| column.name == auto_id.Column)
                    else {
                        continue;
                    };
                    match auto_id.IDType.as_str() {
                        "AUTO_INCREMENT" => column.auto_increment = true,
                        "AUTO_RANDOM" => {
                            column.primary_key = true;
                            table.pk_is_handle = true;
                            table.auto_random_bits = 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(output)
    }
    fn CheckRequirements(&self, _: &Context, _: &CheckCtx) -> Result<(), BackendError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 预编译语句缓存键。
struct stmtKey(String);
impl stmtKey {
    /// 返回键的字节视图（对齐 Go 侧 Hash）。
    fn Hash(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 重复键处理策略：报错 / REPLACE / INSERT IGNORE。
pub enum DuplicateResolution {
    Error,
    Replace,
    Ignore,
}

#[derive(Clone, Debug)]
/// TiDB 逻辑导入后端配置。
pub struct TiDBBackendConfig {
    pub onDuplicate: DuplicateResolution,
    pub maxChunkSize: u64,
    pub maxChunkRows: usize,
    pub preparedStatements: bool,
    /// 兼容旧调用者；未显式设置独立阈值时同时作为类型/冲突预算。
    pub errorBudget: usize,
    pub typeErrorThreshold: i64,
    pub conflictThreshold: i64,
    pub maxRecordRows: i64,
    pub errorSchema: String,
    pub taskID: i64,
}

impl Default for TiDBBackendConfig {
    fn default() -> Self {
        Self {
            onDuplicate: DuplicateResolution::Error,
            maxChunkSize: 1024 * 1024,
            maxChunkRows: 1000,
            preparedStatements: false,
            errorBudget: 0,
            typeErrorThreshold: 0,
            conflictThreshold: i64::MAX,
            maxRecordRows: 0,
            errorSchema: String::new(),
            taskID: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorRecordKind {
    Type,
    Duplicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorRecord {
    pub kind: ErrorRecordKind,
    pub tableName: String,
    pub path: String,
    pub offset: i64,
    pub message: String,
    pub rowData: String,
}

/// TiDB 逻辑导入 Backend：拼 SQL、重试、错误预算与预编译缓存。
pub struct tidbBackend {
    db: Arc<dyn SqlExecutor>,
    config: TiDBBackendConfig,
    insertDuplicate: DuplicateResolution,
    statementCache: Arc<Mutex<HashMap<stmtKey, String>>>,
    errors: Arc<Mutex<Vec<String>>>,
    errorRecords: Arc<Mutex<Vec<ErrorRecord>>>,
    remainingTypeErrors: Arc<Mutex<i64>>,
    remainingConflictErrors: Arc<Mutex<i64>>,
    remainingConflictRecords: Arc<Mutex<i64>>,
}

/// 构造 tidbBackend。
pub fn NewTiDBBackend(db: Arc<dyn SqlExecutor>, config: TiDBBackendConfig) -> Arc<tidbBackend> {
    let legacy_budget = config.errorBudget as i64;
    let type_error_threshold = config.typeErrorThreshold.max(legacy_budget);
    let conflict_threshold = config.conflictThreshold.max(legacy_budget);
    let remaining_records = config.maxRecordRows.max(legacy_budget);
    let insert_duplicate =
        if config.onDuplicate == DuplicateResolution::Ignore && config.maxRecordRows > 0 {
            DuplicateResolution::Error
        } else {
            config.onDuplicate
        };
    Arc::new(tidbBackend {
        db,
        config,
        insertDuplicate: insert_duplicate,
        statementCache: Arc::new(Mutex::new(HashMap::new())),
        errors: Arc::new(Mutex::new(Vec::new())),
        errorRecords: Arc::new(Mutex::new(Vec::new())),
        remainingTypeErrors: Arc::new(Mutex::new(type_error_threshold)),
        remainingConflictErrors: Arc::new(Mutex::new(conflict_threshold)),
        remainingConflictRecords: Arc::new(Mutex::new(remaining_records)),
    })
}

impl tidbBackend {
    /// 导入重试间隔；逻辑导入为 0。
    pub fn RetryImportDelay(&self) -> Duration {
        Duration::ZERO
    }
    /// 是否需要导入后处理。
    pub fn ShouldPostProcess(&self) -> bool {
        true
    }

    /// 按块写入多行，可重试；超出错误预算时降级为逐行写。
    pub fn WriteRows(
        &self,
        tableName: &str,
        columnNames: &[String],
        rows: &tidbRows,
    ) -> Result<(), BackendError> {
        // 按配置切块后逐块重试写入。
        for chunk in rows.splitIntoChunks(self.config.maxChunkSize, self.config.maxChunkRows) {
            let mut last = None;
            for _ in 0..writeRowsMaxRetryTimes {
                match self.WriteBatchRowsToDB(tableName, columnNames, &chunk) {
                    Ok(()) => {
                        last = None;
                        break;
                    }
                    Err(error) if error.retryable => last = Some(error),
                    // 错误预算未用尽：降级为逐行写入以隔离坏行。
                    Err(_error)
                        if *self.remainingTypeErrors.lock().unwrap() > 0
                            || self.config.errorBudget > 0
                            || self.config.maxRecordRows > 0
                            || (self.config.onDuplicate == DuplicateResolution::Error
                                && !self.config.errorSchema.is_empty()) =>
                    {
                        self.WriteRowsToDB(tableName, columnNames, &chunk)?;
                        last = None;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if let Some(error) = last {
                return Err(BackendError::new(format!(
                    "batch write rows reach max retry: {error}"
                )));
            }
        }
        Ok(())
    }

    /// 将一块内所有行拼成单条多值 INSERT/REPLACE 执行。
    pub fn WriteBatchRowsToDB(
        &self,
        tableName: &str,
        columns: &[String],
        rows: &tidbRows,
    ) -> Result<(), BackendError> {
        let Some(mut statement) = self.checkAndBuildStmt(rows, tableName, columns) else {
            return Ok(());
        };
        let mut values = Vec::new();
        for (index, row) in rows.0.iter().enumerate() {
            if index > 0 {
                statement.push(',');
            }
            if self.config.preparedStatements {
                statement.push_str(&row.preparedInsertStmt);
                values.extend(row.values.clone());
            } else {
                statement.push_str(&row.insertStmt);
            }
        }
        self.execStmts(
            vec![stmtTask {
                rows: rows.0.clone(),
                statement,
                values,
            }],
            tableName,
            true,
        )
    }

    /// 空行返回 None，否则返回语句前缀（至 VALUES）。
    pub fn checkAndBuildStmt(
        &self,
        rows: &tidbRows,
        tableName: &str,
        columns: &[String],
    ) -> Option<String> {
        (!rows.0.is_empty()).then(|| self.buildStmt(tableName, columns))
    }

    /// 逐行执行 SQL（错误降级路径）。
    pub fn WriteRowsToDB(
        &self,
        tableName: &str,
        columns: &[String],
        rows: &tidbRows,
    ) -> Result<(), BackendError> {
        let Some(prefix) = self.checkAndBuildStmt(rows, tableName, columns) else {
            return Ok(());
        };
        let tasks = rows
            .0
            .iter()
            .map(|row| stmtTask {
                rows: vec![row.clone()],
                statement: format!(
                    "{prefix}{}",
                    if self.config.preparedStatements {
                        &row.preparedInsertStmt
                    } else {
                        &row.insertStmt
                    }
                ),
                values: row.values.clone(),
            })
            .collect();
        self.execStmts(tasks, tableName, false)
    }

    /// 按 DuplicateResolution 生成 INSERT/REPLACE 前缀。
    pub fn buildStmt(&self, tableName: &str, columns: &[String]) -> String {
        let mut output = match self.insertDuplicate {
            DuplicateResolution::Replace => "REPLACE INTO ".to_string(),
            DuplicateResolution::Ignore => "INSERT IGNORE INTO ".to_string(),
            DuplicateResolution::Error => "INSERT INTO ".to_string(),
        };
        output.push_str(tableName);
        if !columns.is_empty() {
            output.push('(');
            output.push_str(
                &columns
                    .iter()
                    .map(|column| format!("`{}`", column.replace('`', "``")))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            output.push(')');
        }
        output.push_str(" VALUES");
        output
    }

    /// 执行语句任务；batch 时失败立即返回，否则可记入错误预算。
    fn execStmts(
        &self,
        tasks: Vec<stmtTask>,
        tableName: &str,
        batch: bool,
    ) -> Result<(), BackendError> {
        for task in tasks {
            if self.config.preparedStatements {
                // 维护预编译语句 LRU 式缓存（满则删任意一项）。
                let key = stmtKey(task.statement.clone());
                let _ = key.Hash();
                let mut cache = self.statementCache.lock().unwrap();
                if cache.len() >= prepStmtCacheSize && !cache.contains_key(&key) {
                    let first = cache.keys().next().cloned();
                    if let Some(first) = first {
                        cache.remove(&first);
                    }
                }
                cache.entry(key).or_insert_with(|| task.statement.clone());
            }
            let attempts = if batch { 1 } else { writeRowsMaxRetryTimes };
            let mut last = None;
            for _ in 0..attempts {
                match self.db.execute(&task.statement, &task.values) {
                    Ok(affected) => {
                        self.recordDuplicateCount(
                            task.rows.len().abs_diff(affected as usize) as i64
                        )?;
                        last = None;
                        break;
                    }
                    Err(error) if error.retryable => last = Some(error),
                    Err(error) => {
                        last = Some(error);
                        break;
                    }
                }
            }
            if let Some(error) = last {
                if batch {
                    return Err(sqlToBackend(error));
                }
                let row = &task.rows[0];
                self.recordRowError(tableName, row, error)?;
            }
        }
        Ok(())
    }

    /// 已记录的行级错误列表。
    pub fn Errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    pub fn ErrorRecords(&self) -> Vec<ErrorRecord> {
        self.errorRecords.lock().unwrap().clone()
    }

    fn recordDuplicateCount(&self, count: i64) -> Result<(), BackendError> {
        if count == 0 {
            return Ok(());
        }
        let mut remaining = self.remainingConflictErrors.lock().unwrap();
        *remaining -= count;
        if *remaining < 0 {
            return Err(BackendError::new(format!(
                "The number of conflict errors exceeds the threshold configured by `conflict.threshold`: '{}'",
                self.config
                    .conflictThreshold
                    .max(self.config.errorBudget as i64)
            )));
        }
        Ok(())
    }

    fn recordRowError(
        &self,
        tableName: &str,
        row: &tidbRow,
        error: SqlError,
    ) -> Result<(), BackendError> {
        self.errors
            .lock()
            .unwrap()
            .push(format!("{}:{}: {}", row.path, row.offset, error));
        let kind = if isDupEntryError(&error) {
            ErrorRecordKind::Duplicate
        } else {
            ErrorRecordKind::Type
        };
        let record = ErrorRecord {
            kind,
            tableName: tableName.into(),
            path: row.path.clone(),
            offset: row.offset,
            message: error.message.clone(),
            rowData: row.insertStmt.clone(),
        };

        match kind {
            ErrorRecordKind::Type => {
                let mut remaining = self.remainingTypeErrors.lock().unwrap();
                *remaining -= 1;
                if *remaining < 0 {
                    let threshold = self
                        .config
                        .typeErrorThreshold
                        .max(self.config.errorBudget as i64);
                    let message = if threshold > 0 {
                        format!(
                            "The number of type errors exceeds the threshold configured by `max-error.type`: '{threshold}': {error}"
                        )
                    } else {
                        error.message
                    };
                    return Err(BackendError::new(message));
                }
            }
            ErrorRecordKind::Duplicate => {
                if self.config.onDuplicate != DuplicateResolution::Error {
                    self.recordDuplicateCount(1)?;
                }
            }
        }

        self.errorRecords.lock().unwrap().push(record.clone());
        self.persistErrorRecord(&record)?;
        if kind == ErrorRecordKind::Duplicate
            && self.config.onDuplicate == DuplicateResolution::Error
        {
            return Err(sqlToBackend(error));
        }
        Ok(())
    }

    fn persistErrorRecord(&self, record: &ErrorRecord) -> Result<(), BackendError> {
        if self.config.errorSchema.is_empty() {
            return Ok(());
        }
        if record.kind == ErrorRecordKind::Duplicate {
            let mut remaining = self.remainingConflictRecords.lock().unwrap();
            if *remaining <= 0 && self.config.maxRecordRows > 0 {
                return Ok(());
            }
            if self.config.maxRecordRows > 0 {
                *remaining -= 1;
            }
        }
        let (query, values) = match record.kind {
            ErrorRecordKind::Type => (
                format!(
                    "INSERT INTO `{}`.type_error_v2 (task_id, table_name, path, offset, error, row_data) VALUES (?, ?, ?, ?, ?, ?)",
                    self.config.errorSchema.replace('`', "``")
                ),
                vec![
                    SqlValue::Int(self.config.taskID),
                    SqlValue::String(record.tableName.clone()),
                    SqlValue::String(record.path.clone()),
                    SqlValue::Int(record.offset),
                    SqlValue::String(record.message.clone()),
                    SqlValue::String(record.rowData.clone()),
                ],
            ),
            ErrorRecordKind::Duplicate => (
                format!(
                    "INSERT INTO `{}`.conflict_records_v2 (task_id, table_name, path, offset, error, row_id, row_data) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    self.config.errorSchema.replace('`', "``")
                ),
                vec![
                    SqlValue::Int(self.config.taskID),
                    SqlValue::String(record.tableName.clone()),
                    SqlValue::String(record.path.clone()),
                    SqlValue::Int(record.offset),
                    SqlValue::String(record.message.clone()),
                    SqlValue::Int(0),
                    SqlValue::String(record.rowData.clone()),
                ],
            ),
        };
        self.db
            .execute(&query, &values)
            .map(|_| ())
            .map_err(sqlToBackend)
    }
}

/// 单次 execute 任务：关联行、语句与绑定值。
struct stmtTask {
    rows: Vec<tidbRow>,
    statement: String,
    values: Vec<SqlValue>,
}

/// 是否为重复键错误。
fn isDupEntryError(error: &SqlError) -> bool {
    error.duplicate
}

/// SqlError → BackendError。
fn sqlToBackend(error: SqlError) -> BackendError {
    BackendError {
        message: error.message,
        retryable: error.retryable,
        duplicate: error.duplicate,
    }
}

impl Backend for tidbBackend {
    fn Close(&self) {}
    fn RetryImportDelay(&self) -> Duration {
        self.RetryImportDelay()
    }
    fn ShouldPostProcess(&self) -> bool {
        self.ShouldPostProcess()
    }
    fn OpenEngine(&self, _: &Context, _: &EngineConfig, _: Uuid) -> Result<(), BackendError> {
        Ok(())
    }
    fn CloseEngine(
        &self,
        _: &Context,
        _: Option<&EngineConfig>,
        _: Uuid,
    ) -> Result<(), BackendError> {
        Ok(())
    }
    fn ImportEngine(&self, _: &Context, _: Uuid, _: i64, _: i64) -> Result<(), BackendError> {
        Ok(())
    }
    fn CleanupEngine(&self, _: &Context, _: Uuid) -> Result<(), BackendError> {
        Ok(())
    }
    fn FlushEngine(&self, _: &Context, _: Uuid) -> Result<(), BackendError> {
        Ok(())
    }
    fn FlushAllEngines(&self, _: &Context) -> Result<(), BackendError> {
        Ok(())
    }
    fn LocalWriter(
        &self,
        _: &Context,
        cfg: &LocalWriterConfig,
        _: Uuid,
    ) -> Result<Box<dyn EngineWriter>, BackendError> {
        Ok(Box::new(Writer {
            backend: Arc::new(tidbBackend {
                db: Arc::clone(&self.db),
                config: self.config.clone(),
                insertDuplicate: self.insertDuplicate,
                statementCache: Arc::clone(&self.statementCache),
                errors: Arc::clone(&self.errors),
                errorRecords: Arc::clone(&self.errorRecords),
                remainingTypeErrors: Arc::clone(&self.remainingTypeErrors),
                remainingConflictErrors: Arc::clone(&self.remainingConflictErrors),
                remainingConflictRecords: Arc::clone(&self.remainingConflictRecords),
            }),
            tableName: cfg.TiDB.TableName.clone(),
        }))
    }
}

/// EngineWriter：将 tidbRows 追加写入指定表。
pub struct Writer {
    backend: Arc<tidbBackend>,
    tableName: String,
}

impl EngineWriter for Writer {
    fn AppendRows(
        &mut self,
        _: &Context,
        columns: &[String],
        rows: &dyn Rows,
    ) -> Result<(), BackendError> {
        let rows = rows
            .as_any()
            .downcast_ref::<tidbRows>()
            .ok_or_else(|| BackendError::new("TiDB writer requires tidbRows"))?;
        self.backend.WriteRows(&self.tableName, columns, rows)
    }
    fn IsSynced(&self) -> bool {
        true
    }
    fn Close(&mut self, _: &Context) -> Result<Option<ChunkFlushStatus>, BackendError> {
        Ok(None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// SHOW TABLE ... NEXT_ROW_ID 解析出的自增/自随机游标信息。
pub struct TableAutoIDInfo {
    pub Column: String,
    pub NextID: u64,
    pub IDType: String,
}

/// 查询表的下一 RowID / AUTO_INCREMENT / AUTO_RANDOM 信息。
pub fn FetchTableAutoIDInfos(
    executor: &dyn SqlExecutor,
    tableName: &str,
) -> Result<Vec<TableAutoIDInfo>, SqlError> {
    executor
        .query(&format!("SHOW TABLE {tableName} NEXT_ROW_ID"), &[])?
        .into_iter()
        .map(|row| {
            let (column, next, id_type) = match row.as_slice() {
                [_, _, SqlValue::String(column), next] => {
                    (column.clone(), parseNextID(next)?, "AUTO_INCREMENT".into())
                }
                [
                    _,
                    _,
                    SqlValue::String(column),
                    next,
                    SqlValue::String(id_type),
                ] => (column.clone(), parseNextID(next)?, id_type.clone()),
                _ => return Err(SqlError::new("unexpected NEXT_ROW_ID result")),
            };
            Ok(TableAutoIDInfo {
                Column: column,
                NextID: next,
                IDType: id_type,
            })
        })
        .collect()
}

fn parseNextID(value: &SqlValue) -> Result<u64, SqlError> {
    match value {
        SqlValue::UInt(value) => Ok(*value),
        SqlValue::Int(value) if *value >= 0 => Ok(*value as u64),
        SqlValue::String(value) => value
            .parse()
            .map_err(|_| SqlError::new("unexpected NEXT_ROW_ID value")),
        _ => Err(SqlError::new("unexpected NEXT_ROW_ID value")),
    }
}
