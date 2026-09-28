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

// TTL 扫描/删除 SQL 的拼接状态机、Datum 字面量格式化与 scan 查询生成器。
//
// 将过期时间、主键范围与行集合拼成安全的 SELECT/DELETE；非只读 DELETE 必须带 TTL 过期条件。
// 文件前半为 Go 机械翻译留存（整段注释），可执行实现从 `SqlError` 起。
// `SQLBuilder` 依赖状态机约束子句顺序，防止生成缺少过期条件的危险 DELETE。

// TTL scan/delete SQL 的拼接状态机、Datum 格式化和 scan 查询生成器。
//
// use std::io::Write;
//
// writeHex 对应 Go 的 helper：把 Datum bytes 编码为 SQL 中的 x'hex' 字面量。
// pub fn writeHex(in_: &mut dyn Write, d: types::Datum) -> Result<(), errors::Error> {
//     write!(in_, "x'{}'", hex::EncodeToString(d.GetBytes()))?;
//     Ok(())
// }
//
// writeDatum 对应 Go 中按 FieldType 把 Datum 恢复成 SQL 字面量的逻辑。
// pub fn writeDatum(
//     restoreCtx: &mut format::RestoreCtx,
//     d: types::Datum,
//     ft: &types::FieldType,
// ) -> Result<(), errors::Error> {
//     match ft.GetType() {
//         mysql::TypeBit | mysql::TypeBlob | mysql::TypeLongBlob | mysql::TypeTinyBlob => {
//             return writeHex(&mut restoreCtx.In, d);
//         }
//         mysql::TypeString | mysql::TypeVarString | mysql::TypeVarchar | mysql::TypeEnum | mysql::TypeSet => {
//             if mysql::HasBinaryFlag(ft.GetFlag()) {
//                 return writeHex(&mut restoreCtx.In, d);
//             }
// 非二进制字符串走 sqlescape.EscapeString，保留 Go 的手工加引号方式。
//             write!(restoreCtx.In, "'{}'", sqlescape::EscapeString(d.GetString()))?;
//             return Ok(());
//         }
//         _ => {}
//     }
//
// 其它类型交给 parser ast.ValueExpr 恢复，保持 Go 版本的兜底路径。
//     let expr = ast::NewValueExpr(d.GetValue(), ft.GetCharset(), ft.GetCollate());
//     expr.Restore(restoreCtx)
// }
//
// FormatSQLDatum formats the datum to a value string in sql
// pub fn FormatSQLDatum(d: types::Datum, ft: &types::FieldType) -> Result<String, errors::Error> {
//     let mut sb = String::new();
//     let mut ctx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut sb);
//     writeDatum(&mut ctx, d, ft)?;
//     Ok(sb)
// }
//
// sqlBuilderState 对应 Go 的 iota 状态机，限制 SQL 子句只能按固定顺序写入。
// #[derive(Clone, Copy, Debug, Eq, PartialEq)]
// pub enum sqlBuilderState {
//     writeBegin,
//     writeSelOrDel,
//     writeWhere,
//     writeOrderBy,
//     writeLimit,
//     writeDone,
// }
//
// SQLBuilder is used to build SQLs for TTL
// pub struct SQLBuilder {
//     pub tbl: *mut cache::PhysicalTable,
//     pub sb: String,
//     pub restoreCtx: format::RestoreCtx,
//     pub state: sqlBuilderState,
//
//     pub isReadOnly: bool,
//     pub hasWriteExpireCond: bool,
// }
//
// NewSQLBuilder creates a new TTLSQLBuilder
// pub fn NewSQLBuilder(tbl: *mut cache::PhysicalTable) -> Box<SQLBuilder> {
//     let mut b = Box::new(SQLBuilder {
//         tbl,
//         sb: String::new(),
//         restoreCtx: format::RestoreCtx::default(),
//         state: sqlBuilderState::writeBegin,
//         isReadOnly: false,
//         hasWriteExpireCond: false,
//     });
//     b.restoreCtx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut b.sb);
//     b
// }
//
// impl SQLBuilder {
// Build builds the final sql
//     pub fn Build(&mut self) -> Result<String, errors::Error> {
//         if self.state == sqlBuilderState::writeBegin {
//             return Err(errors::Errorf(format!("invalid state: {:?}", self.state)));
//         }
//
//         if !self.isReadOnly && !self.hasWriteExpireCond {
// check whether the `timeRow < expire_time` condition has been written to make sure this SQL is safe.
// 非只读 DELETE 必须包含过期条件，避免构造出无 TTL 保护的删除语句。
//             return Err(errors::New("expire condition not write"));
//         }
//
//         if self.state != sqlBuilderState::writeDone {
//             self.state = sqlBuilderState::writeDone;
//         }
//
//         Ok(self.sb.clone())
//     }
//
// WriteSelect writes a select statement to select key columns without any condition
//     pub fn WriteSelect(&mut self) -> Result<(), errors::Error> {
//         if self.state != sqlBuilderState::writeBegin {
//             return Err(errors::Errorf(format!("invalid state: {:?}", self.state)));
//         }
//         self.restoreCtx.WritePlain("SELECT LOW_PRIORITY SQL_NO_CACHE ");
//         self.writeColNames(unsafe { (*self.tbl).KeyColumns.clone() }, false);
//         self.restoreCtx.WritePlain(" FROM ");
//         self.writeTblName()?;
//         if let Some(par) = unsafe { (*self.tbl).PartitionDef.as_ref() } {
//             self.restoreCtx.WritePlain(" PARTITION(");
//             self.restoreCtx.WriteName(par.Name.O);
//             self.restoreCtx.WritePlain(")");
//         }
//         self.state = sqlBuilderState::writeSelOrDel;
//         self.isReadOnly = true;
//         Ok(())
//     }
//
// WriteDelete writes a delete statement without any condition
//     pub fn WriteDelete(&mut self) -> Result<(), errors::Error> {
//         if self.state != sqlBuilderState::writeBegin {
//             return Err(errors::Errorf(format!("invalid state: {:?}", self.state)));
//         }
//         self.restoreCtx.WritePlain("DELETE LOW_PRIORITY FROM ");
//         self.writeTblName()?;
//         if let Some(par) = unsafe { (*self.tbl).PartitionDef.as_ref() } {
//             self.restoreCtx.WritePlain(" PARTITION(");
//             self.restoreCtx.WriteName(par.Name.O);
//             self.restoreCtx.WritePlain(")");
//         }
//         self.state = sqlBuilderState::writeSelOrDel;
//         Ok(())
//     }
//
// WriteCommonCondition writes a new condition
//     pub fn WriteCommonCondition(
//         &mut self,
//         cols: Vec<*mut model::ColumnInfo>,
//         op: &str,
//         dp: Vec<types::Datum>,
//     ) -> Result<(), errors::Error> {
//         match self.state {
//             sqlBuilderState::writeSelOrDel => {
//                 self.restoreCtx.WritePlain(" WHERE ");
//                 self.state = sqlBuilderState::writeWhere;
//             }
//             sqlBuilderState::writeWhere => {
//                 self.restoreCtx.WritePlain(" AND ");
//             }
//             _ => return Err(errors::Errorf(format!("invalid state: {:?}", self.state))),
//         }
//
//         self.writeColNames(cols.clone(), cols.len() > 1);
//         self.restoreCtx.WritePlain(" ");
//         self.restoreCtx.WritePlain(op);
//         self.restoreCtx.WritePlain(" ");
//         self.writeDataPoint(cols, dp)
//     }
//
// WriteExpireCondition writes a condition with the time column
//     pub fn WriteExpireCondition(&mut self, expire: time::Time) -> Result<(), errors::Error> {
//         match self.state {
//             sqlBuilderState::writeSelOrDel => {
//                 self.restoreCtx.WritePlain(" WHERE ");
//                 self.state = sqlBuilderState::writeWhere;
//             }
//             sqlBuilderState::writeWhere => {
//                 self.restoreCtx.WritePlain(" AND ");
//             }
//             _ => return Err(errors::Errorf(format!("invalid state: {:?}", self.state))),
//         }
//
//         self.writeColNames(vec![unsafe { (*self.tbl).TimeColumn }], false);
//         self.restoreCtx.WritePlain(" < ");
//         self.restoreCtx.WritePlain("FROM_UNIXTIME(");
//         self.restoreCtx.WritePlain(&strconv::FormatInt(expire.Unix(), 10));
//         self.restoreCtx.WritePlain(")");
//         self.hasWriteExpireCond = true;
//         Ok(())
//     }
//
// WriteInCondition writes an IN condition
//     pub fn WriteInCondition(
//         &mut self,
//         cols: Vec<*mut model::ColumnInfo>,
//         dps: Vec<Vec<types::Datum>>,
//     ) -> Result<(), errors::Error> {
//         match self.state {
//             sqlBuilderState::writeSelOrDel => {
//                 self.restoreCtx.WritePlain(" WHERE ");
//                 self.state = sqlBuilderState::writeWhere;
//             }
//             sqlBuilderState::writeWhere => {
//                 self.restoreCtx.WritePlain(" AND ");
//             }
//             _ => return Err(errors::Errorf(format!("invalid state: {:?}", self.state))),
//         }
//
//         self.writeColNames(cols.clone(), cols.len() > 1);
//         self.restoreCtx.WritePlain(" IN ");
//         self.restoreCtx.WritePlain("(");
//         let mut first = true;
//         for v in dps {
//             if first {
//                 first = false;
//             } else {
//                 self.restoreCtx.WritePlain(", ");
//             }
//             self.writeDataPoint(cols.clone(), v)?;
//         }
//         self.restoreCtx.WritePlain(")");
//         Ok(())
//     }
//
// WriteOrderBy writes order by
//     pub fn WriteOrderBy(&mut self, cols: Vec<*mut model::ColumnInfo>, desc: bool) -> Result<(), errors::Error> {
//         if self.state != sqlBuilderState::writeSelOrDel && self.state != sqlBuilderState::writeWhere {
//             return Err(errors::Errorf(format!("invalid state: {:?}", self.state)));
//         }
//         self.state = sqlBuilderState::writeOrderBy;
//         self.restoreCtx.WritePlain(" ORDER BY ");
//         self.writeColNames(cols, false);
//         if desc {
//             self.restoreCtx.WritePlain(" DESC");
//         } else {
//             self.restoreCtx.WritePlain(" ASC");
//         }
//         Ok(())
//     }
//
// WriteLimit writes the limit
//     pub fn WriteLimit(&mut self, n: i32) -> Result<(), errors::Error> {
//         if self.state != sqlBuilderState::writeSelOrDel
//             && self.state != sqlBuilderState::writeWhere
//             && self.state != sqlBuilderState::writeOrderBy
//         {
//             return Err(errors::Errorf(format!("invalid state: {:?}", self.state)));
//         }
//         self.state = sqlBuilderState::writeLimit;
//         self.restoreCtx.WritePlain(" LIMIT ");
//         self.restoreCtx.WritePlain(&strconv::Itoa(n));
//         Ok(())
//     }
//
//     pub fn writeTblName(&mut self) -> Result<(), errors::Error> {
//         let tn = ast::TableName {
//             Schema: unsafe { (*self.tbl).Schema },
//             Name: unsafe { (*self.tbl).Name },
//         };
//         tn.Restore(&mut self.restoreCtx)
//     }
//
//     pub fn writeColName(&mut self, col: *mut model::ColumnInfo) {
//         self.restoreCtx.WriteName(unsafe { (*col).Name.O });
//     }
//
//     pub fn writeColNames(&mut self, cols: Vec<*mut model::ColumnInfo>, writeBrackets: bool) {
//         if writeBrackets {
//             self.restoreCtx.WritePlain("(");
//         }
//
//         let mut first = true;
//         for col in cols {
//             if first {
//                 first = false;
//             } else {
//                 self.restoreCtx.WritePlain(", ");
//             }
//             self.writeColName(col);
//         }
//
//         if writeBrackets {
//             self.restoreCtx.WritePlain(")");
//         }
//     }
//
//     pub fn writeDataPoint(
//         &mut self,
//         cols: Vec<*mut model::ColumnInfo>,
//         dp: Vec<types::Datum>,
//     ) -> Result<(), errors::Error> {
//         let writeBrackets = cols.len() > 1;
//         if cols.len() != dp.len() {
//             return Err(errors::Errorf(format!("col count not match {} != {}", cols.len(), dp.len())));
//         }
//
//         if writeBrackets {
//             self.restoreCtx.WritePlain("(");
//         }
//
//         let mut first = true;
//         for (i, d) in dp.into_iter().enumerate() {
//             if first {
//                 first = false;
//             } else {
//                 self.restoreCtx.WritePlain(", ");
//             }
// Datum 的 SQL 字面量格式依赖列 FieldType，保持 Go 的逐列配对。
//             writeDatum(&mut self.restoreCtx, d, unsafe { &(*cols[i]).FieldType })?;
//         }
//
//         if writeBrackets {
//             self.restoreCtx.WritePlain(")");
//         }
//
//         Ok(())
//     }
// }
//
// ScanQueryGenerator generates SQLs for scan task
// pub struct ScanQueryGenerator {
//     pub tbl: *mut cache::PhysicalTable,
//     pub expire: time::Time,
//     pub keyRangeStart: Vec<types::Datum>,
//     pub keyRangeEnd: Vec<types::Datum>,
//     pub stack: Option<Vec<Vec<types::Datum>>>,
//     pub limit: i32,
//     pub firstBuild: bool,
//     pub exhausted: bool,
// }
//
// NewScanQueryGenerator creates a new ScanQueryGenerator
// pub fn NewScanQueryGenerator(
//     tbl: *mut cache::PhysicalTable,
//     expire: time::Time,
//     rangeStart: Vec<types::Datum>,
//     rangeEnd: Vec<types::Datum>,
// ) -> Result<Box<ScanQueryGenerator>, errors::Error> {
//     unsafe { (*tbl).ValidateKeyPrefix(rangeStart.clone())? };
//     unsafe { (*tbl).ValidateKeyPrefix(rangeEnd.clone())? };
//
//     Ok(Box::new(ScanQueryGenerator {
//         tbl,
//         expire,
//         keyRangeStart: rangeStart,
//         keyRangeEnd: rangeEnd,
//         stack: None,
//         limit: 0,
//         firstBuild: true,
//         exhausted: false,
//     }))
// }
//
// impl ScanQueryGenerator {
// NextSQL creates next sql of the scan task
//     pub fn NextSQL(
//         &mut self,
//         continueFromResult: Vec<Vec<types::Datum>>,
//         nextLimit: i32,
//     ) -> Result<String, errors::Error> {
//         if self.exhausted {
//             return Err(errors::New("generator is exhausted"));
//         }
//
//         if nextLimit <= 0 {
//             return Err(errors::Errorf(format!("invalid limit '{}'", nextLimit)));
//         }
//
// Go defer 会在 setStack 或 buildSQL 报错时也把 firstBuild 置为 false。
//         let first_build_guard = defer::defer(|| {
//             self.firstBuild = false;
//         });
//
//         if self.stack.is_none() {
//             self.stack = Some(Vec::with_capacity(unsafe { (*self.tbl).KeyColumns.len() }));
//         }
//
//         if continueFromResult.len() >= self.limit as usize {
//             let mut continueFromKey = Vec::new();
//             if !continueFromResult.is_empty() {
//                 continueFromKey = continueFromResult[continueFromResult.len() - 1].clone();
//             }
//             self.setStack(continueFromKey)?;
//         } else {
//             let stack = self.stack.as_mut().unwrap();
//             if !stack.is_empty() {
//                 stack.truncate(stack.len() - 1);
//             }
//             if stack.is_empty() {
//                 self.exhausted = true;
//             }
//         }
//         self.limit = nextLimit;
//         let sql = self.buildSQL();
//         drop(first_build_guard);
//         sql
//     }
//
// IsExhausted returns whether the generator is exhausted
//     pub fn IsExhausted(&self) -> bool {
//         self.exhausted
//     }
//
//     pub fn setStack(&mut self, mut key: Vec<types::Datum>) -> Result<(), errors::Error> {
//         if key.is_empty() {
//             key = self.keyRangeStart.clone();
//         }
//
//         if key.is_empty() {
//             if let Some(stack) = self.stack.as_mut() {
//                 stack.clear();
//             }
//             return Ok(());
//         }
//
//         unsafe { (*self.tbl).ValidateKeyPrefix(key.clone())? };
//
//         let stack = self.stack.as_mut().unwrap();
//         stack.truncate(key.len());
//         for i in 0..key.len() {
// Go 保存 key[0:i+1] 的切片前缀；用 Vec 克隆表达相同前缀栈。
//             if i < stack.len() {
//                 stack[i] = key[0..=i].to_vec();
//             } else {
//                 stack.push(key[0..=i].to_vec());
//             }
//         }
//         Ok(())
//     }
//
//     pub fn buildSQL(&mut self) -> Result<String, errors::Error> {
//         if self.limit <= 0 {
//             return Err(errors::Errorf(format!("invalid limit '{}'", self.limit)));
//         }
//
//         if self.exhausted {
//             return Ok(String::new());
//         }
//
//         let mut b = NewSQLBuilder(self.tbl);
//         b.WriteSelect()?;
//         if let Some(stack) = self.stack.as_ref() {
//             if !stack.is_empty() {
//                 for (i, d) in stack[stack.len() - 1].iter().enumerate() {
//                     let col = vec![unsafe { (*self.tbl).KeyColumns[i] }];
//                     let val = vec![d.clone()];
//                     let err = if i < stack.len() - 1 {
//                         b.WriteCommonCondition(col, "=", val)
//                     } else if self.firstBuild {
// When `g.firstBuild == true`, that means we are querying rows after range start, because range is defined
// as [start, end), we should use ">=" to find the rows including start key.
// 首次构造从 rangeStart 开始，边界包含 start，所以最后一个前缀列用 >=。
//                         b.WriteCommonCondition(col, ">=", val)
//                     } else {
// Otherwise when `g.firstBuild != true`, that means we are continuing with the previous result, we should use
// ">" to exclude the previous row.
// 后续翻页需要排除上一批最后一行，所以最后一个前缀列用 >。
//                         b.WriteCommonCondition(col, ">", val)
//                     };
//                     if let Err(err) = err {
//                         return Err(err);
//                     }
//                 }
//             }
//         }
//
//         if !self.keyRangeEnd.is_empty() {
//             let endCols = unsafe { (*self.tbl).KeyColumns[0..self.keyRangeEnd.len()].to_vec() };
//             b.WriteCommonCondition(endCols, "<", self.keyRangeEnd.clone())?;
//         }
//
//         b.WriteExpireCondition(self.expire)?;
//         b.WriteOrderBy(unsafe { (*self.tbl).KeyColumns.clone() }, false)?;
//         b.WriteLimit(self.limit)?;
//
//         b.Build()
//     }
// }
//
// BuildDeleteSQL builds a delete SQL
// pub fn BuildDeleteSQL(
//     tbl: *mut cache::PhysicalTable,
//     rows: Vec<Vec<types::Datum>>,
//     expire: time::Time,
// ) -> Result<String, errors::Error> {
//     if rows.is_empty() {
//         return Err(errors::New("Cannot build delete SQL with empty rows"));
//     }
//
//     let mut b = NewSQLBuilder(tbl);
//     b.WriteDelete()?;
//
//     b.WriteInCondition(unsafe { (*tbl).KeyColumns.clone() }, rows.clone())?;
//     b.WriteExpireCondition(expire)?;
//     b.WriteLimit(rows.len() as i32)?;
//
//     b.Build()
// }
// */
use std::fmt::{Display, Formatter};

/// SQL 构建过程中的错误信息包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlError(pub String);
impl Display for SqlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SqlError {}
/// 本模块统一的 Result 别名。
pub type Result<T> = std::result::Result<T, SqlError>;

/// 列值的统一内存表示，用于格式化为 SQL 字面量或绑定参数。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Decimal(String),
    Bytes(Vec<u8>),
    String(String),
    Date(String),
    Time(String),
    DateTime(String),
    Timestamp(String),
    Enum(String),
    Set(String),
    Json(String),
    Bool(bool),
}

/// MySQL 列类型分类，决定 Datum 如何编码为 SQL 字面量。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Bit,
    Blob,
    LongBlob,
    TinyBlob,
    String,
    VarString,
    Varchar,
    Enum,
    Set,
    Int,
    UInt,
    Float,
    Decimal,
    Date,
    Time,
    DateTime,
    Timestamp,
    Json,
    Bool,
}
/// 列的 FieldType：类型种类以及是否带 binary 标志。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldType {
    pub kind: FieldKind,
    /// 为 true 时字符串类列按十六进制字面量输出。
    pub binary: bool,
}
impl FieldType {
    /// 构造非 binary 的 FieldType。
    pub const fn new(kind: FieldKind) -> Self {
        Self {
            kind,
            binary: false,
        }
    }
    /// 构造带 binary 标志的 FieldType。
    pub const fn binary(kind: FieldKind) -> Self {
        Self { kind, binary: true }
    }
}

/// TTL 表上的一列：名称与 FieldType。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Column {
    pub name: String,
    pub field_type: FieldType,
}
impl Column {
    /// 由列名与类型构造 `Column`。
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
        }
    }
}

/// TTL 作用的物理表（可含分区名）：主键列与时间列用于拼 SELECT/DELETE。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalTable {
    pub schema: String,
    pub name: String,
    pub key_columns: Vec<Column>,
    /// TTL 过期判定所用的时间列。
    pub time_column: Column,
    /// 若为分区表，写入 `PARTITION(name)` 子句。
    pub partition: Option<String>,
}
impl PhysicalTable {
    /// 构造物理表；主键列为空则报错。
    pub fn new(
        schema: impl Into<String>,
        name: impl Into<String>,
        key_columns: Vec<Column>,
        time_column: Column,
        partition: Option<String>,
    ) -> Result<Self> {
        if key_columns.is_empty() {
            return Err(SqlError(
                "TTL table must have at least one key column".into(),
            ));
        }
        Ok(Self {
            schema: schema.into(),
            name: name.into(),
            key_columns,
            time_column,
            partition,
        })
    }
    /// 校验键前缀长度不超过主键列数。
    pub fn validate_key_prefix(&self, key: &[Datum]) -> Result<()> {
        if key.len() > self.key_columns.len() {
            Err(SqlError(format!(
                "invalid key prefix length {} > {}",
                key.len(),
                self.key_columns.len()
            )))
        } else {
            Ok(())
        }
    }
    /// Go 风格别名：同 `validate_key_prefix`。
    pub fn ValidateKeyPrefix(&self, key: &[Datum]) -> Result<()> {
        self.validate_key_prefix(key)
    }
}

/// 将字节编码为 SQL 中的 `x'hex'` 字面量。
fn write_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 3);
    out.push_str("x'");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out.push('\'');
    out
}
/// 按 MySQL 字符串转义规则处理特殊字符（NUL、换行、引号等）。
fn escape_sql_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\0' => out.push_str("\\0"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{1a}' => out.push_str("\\Z"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}
/// 将 Datum 转为可用于 hex 字面量的字节序列。
fn datum_bytes(datum: &Datum) -> Result<Vec<u8>> {
    match datum {
        Datum::Bytes(v) => Ok(v.clone()),
        Datum::String(v) | Datum::Enum(v) | Datum::Set(v) => Ok(v.as_bytes().to_vec()),
        Datum::Int(v) => Ok(v.to_be_bytes().to_vec()),
        Datum::UInt(v) => Ok(v.to_be_bytes().to_vec()),
        _ => Err(SqlError(
            "datum cannot be represented as binary bytes".into(),
        )),
    }
}
/// 按 FieldType 把 Datum 格式化为 SQL 字面量（二进制走 hex，字符串走转义）。
pub fn write_datum(datum: &Datum, field_type: &FieldType) -> Result<String> {
    use FieldKind::*;
    if matches!(field_type.kind, Bit | Blob | LongBlob | TinyBlob)
        || field_type.binary && matches!(field_type.kind, String | VarString | Varchar | Enum | Set)
    {
        return Ok(write_hex(&datum_bytes(datum)?));
    }
    if matches!(field_type.kind, String | VarString | Varchar | Enum | Set) {
        let value = match datum {
            Datum::String(v) | Datum::Enum(v) | Datum::Set(v) => v,
            Datum::Bytes(v) => match std::str::from_utf8(v) {
                Ok(value) => return Ok(format!("'{}'", escape_sql_string(value))),
                Err(_) => return Ok(write_hex(v)),
            },
            _ => return Err(SqlError("string column requires string datum".into())),
        };
        return Ok(format!("'{}'", escape_sql_string(value)));
    }
    Ok(match datum {
        Datum::Null => "NULL".into(),
        Datum::Int(v) => v.to_string(),
        Datum::UInt(v) => v.to_string(),
        Datum::Float(v) if v.is_finite() => v.to_string(),
        Datum::Float(_) => {
            return Err(SqlError(
                "non-finite float cannot be restored as SQL".into(),
            ));
        }
        Datum::Decimal(v) => v.clone(),
        Datum::Bytes(v) => write_hex(v),
        Datum::String(v)
        | Datum::Date(v)
        | Datum::Time(v)
        | Datum::DateTime(v)
        | Datum::Timestamp(v)
        | Datum::Enum(v)
        | Datum::Set(v)
        | Datum::Json(v) => format!("'{}'", escape_sql_string(v)),
        Datum::Bool(v) => {
            if *v {
                "1".into()
            } else {
                "0".into()
            }
        }
    })
}
/// Go 风格别名：同 `write_datum`，供迁移代码直接调用。
pub fn FormatSQLDatum(datum: &Datum, field_type: &FieldType) -> Result<String> {
    write_datum(datum, field_type)
}

/// 以反引号包裹标识符，内部反引号加倍转义。
fn write_name(output: &mut String, name: &str) {
    output.push('`');
    output.push_str(&name.replace('`', "``"));
    output.push('`');
}

/// SQLBuilder 写入子句的状态机，限制 SELECT/DELETE → WHERE → ORDER BY → LIMIT 的顺序。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum sqlBuilderState {
    writeBegin,
    writeSelOrDel,
    writeWhere,
    writeOrderBy,
    writeLimit,
    writeDone,
}
/// 按固定顺序拼接 TTL 用 SELECT/DELETE SQL 的构建器。
pub struct SQLBuilder<'a> {
    table: &'a PhysicalTable,
    sql: String,
    state: sqlBuilderState,
    is_read_only: bool,
    /// 是否已写入 `time_column < FROM_UNIXTIME(...)`，非只读时 build 要求为 true。
    has_expire_condition: bool,
}
impl<'a> SQLBuilder<'a> {
    /// 从物理表创建处于 `writeBegin` 的构建器。
    pub fn new(table: &'a PhysicalTable) -> Self {
        Self {
            table,
            sql: String::new(),
            state: sqlBuilderState::writeBegin,
            is_read_only: false,
            has_expire_condition: false,
        }
    }
    /// 完成构建：校验状态与过期条件后返回 SQL 字符串。
    pub fn build(&mut self) -> Result<String> {
        if self.state == sqlBuilderState::writeBegin {
            return Err(SqlError(format!("invalid state: {:?}", self.state)));
        }
        // 非只读 DELETE 必须含过期条件，避免无 TTL 保护的全表删除风险。
        if !self.is_read_only && !self.has_expire_condition {
            return Err(SqlError("expire condition not write".into()));
        }
        self.state = sqlBuilderState::writeDone;
        Ok(self.sql.clone())
    }
    /// 在允许写条件的状态下追加 `WHERE` 或 `AND`。
    fn expect_condition_state(&mut self) -> Result<()> {
        match self.state {
            sqlBuilderState::writeSelOrDel => {
                self.sql.push_str(" WHERE ");
                self.state = sqlBuilderState::writeWhere;
                Ok(())
            }
            sqlBuilderState::writeWhere => {
                self.sql.push_str(" AND ");
                Ok(())
            }
            _ => Err(SqlError(format!("invalid state: {:?}", self.state))),
        }
    }
    /// 写入 `` `schema`.`table` ``。
    fn write_table_name(&mut self) {
        write_name(&mut self.sql, &self.table.schema);
        self.sql.push('.');
        write_name(&mut self.sql, &self.table.name);
    }
    /// 写入一列或多列名，多列时可加括号。
    fn write_column_names(&mut self, columns: &[Column], brackets: bool) {
        if brackets {
            self.sql.push('(');
        }
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                self.sql.push_str(", ");
            }
            write_name(&mut self.sql, &column.name);
        }
        if brackets {
            self.sql.push(')');
        }
    }
    /// 按列 FieldType 写入一组 Datum 字面量（多列时加括号）。
    fn write_data_point(&mut self, columns: &[Column], values: &[Datum]) -> Result<()> {
        if columns.len() != values.len() {
            return Err(SqlError(format!(
                "col count not match {} != {}",
                columns.len(),
                values.len()
            )));
        }
        let brackets = columns.len() > 1;
        if brackets {
            self.sql.push('(');
        }
        for (index, (column, value)) in columns.iter().zip(values).enumerate() {
            if index > 0 {
                self.sql.push_str(", ");
            }
            self.sql.push_str(&write_datum(value, &column.field_type)?);
        }
        if brackets {
            self.sql.push(')');
        }
        Ok(())
    }
    /// 写入无条件的 SELECT（仅主键列，LOW_PRIORITY SQL_NO_CACHE）。
    pub fn write_select(&mut self) -> Result<()> {
        if self.state != sqlBuilderState::writeBegin {
            return Err(SqlError(format!("invalid state: {:?}", self.state)));
        }
        self.sql.push_str("SELECT LOW_PRIORITY SQL_NO_CACHE ");
        self.write_column_names(&self.table.key_columns, false);
        self.sql.push_str(" FROM ");
        self.write_table_name();
        if let Some(partition) = &self.table.partition {
            self.sql.push_str(" PARTITION(");
            write_name(&mut self.sql, partition);
            self.sql.push(')');
        }
        self.state = sqlBuilderState::writeSelOrDel;
        self.is_read_only = true;
        Ok(())
    }
    /// 写入无条件的 DELETE（后续必须再写过期条件）。
    pub fn write_delete(&mut self) -> Result<()> {
        if self.state != sqlBuilderState::writeBegin {
            return Err(SqlError(format!("invalid state: {:?}", self.state)));
        }
        self.sql.push_str("DELETE LOW_PRIORITY FROM ");
        self.write_table_name();
        if let Some(partition) = &self.table.partition {
            self.sql.push_str(" PARTITION(");
            write_name(&mut self.sql, partition);
            self.sql.push(')');
        }
        self.state = sqlBuilderState::writeSelOrDel;
        Ok(())
    }
    /// 写入普通比较条件：`cols op values`。
    pub fn write_common_condition(
        &mut self,
        columns: &[Column],
        operator: &str,
        values: &[Datum],
    ) -> Result<()> {
        self.expect_condition_state()?;
        self.write_column_names(columns, columns.len() > 1);
        self.sql.push(' ');
        self.sql.push_str(operator);
        self.sql.push(' ');
        self.write_data_point(columns, values)
    }
    /// 写入 TTL 过期条件：`time_column < FROM_UNIXTIME(expire_unix)`。
    pub fn write_expire_condition(&mut self, expire_unix: i64) -> Result<()> {
        self.expect_condition_state()?;
        let column = self.table.time_column.clone();
        self.write_column_names(std::slice::from_ref(&column), false);
        self.sql.push_str(" < FROM_UNIXTIME(");
        self.sql.push_str(&expire_unix.to_string());
        self.sql.push(')');
        self.has_expire_condition = true;
        Ok(())
    }
    /// 写入主键 IN 列表条件，用于按批删除指定行。
    pub fn write_in_condition(&mut self, columns: &[Column], rows: &[Vec<Datum>]) -> Result<()> {
        self.expect_condition_state()?;
        self.write_column_names(columns, columns.len() > 1);
        self.sql.push_str(" IN (");
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                self.sql.push_str(", ");
            }
            self.write_data_point(columns, row)?;
        }
        self.sql.push(')');
        Ok(())
    }
    /// 写入 ORDER BY，升序或降序。
    pub fn write_order_by(&mut self, columns: &[Column], descending: bool) -> Result<()> {
        if !matches!(
            self.state,
            sqlBuilderState::writeSelOrDel | sqlBuilderState::writeWhere
        ) {
            return Err(SqlError(format!("invalid state: {:?}", self.state)));
        }
        self.state = sqlBuilderState::writeOrderBy;
        self.sql.push_str(" ORDER BY ");
        self.write_column_names(columns, false);
        self.sql.push_str(if descending { " DESC" } else { " ASC" });
        Ok(())
    }
    /// 写入 LIMIT。
    pub fn write_limit(&mut self, limit: i32) -> Result<()> {
        if !matches!(
            self.state,
            sqlBuilderState::writeSelOrDel
                | sqlBuilderState::writeWhere
                | sqlBuilderState::writeOrderBy
        ) {
            return Err(SqlError(format!("invalid state: {:?}", self.state)));
        }
        self.state = sqlBuilderState::writeLimit;
        self.sql.push_str(" LIMIT ");
        self.sql.push_str(&limit.to_string());
        Ok(())
    }
    /// Go 风格别名：同 `build`。
    pub fn Build(&mut self) -> Result<String> {
        self.build()
    }
    /// Go 风格别名：同 `write_select`。
    pub fn WriteSelect(&mut self) -> Result<()> {
        self.write_select()
    }
    /// Go 风格别名：同 `write_delete`。
    pub fn WriteDelete(&mut self) -> Result<()> {
        self.write_delete()
    }
    /// Go 风格别名：同 `write_common_condition`。
    pub fn WriteCommonCondition(&mut self, c: &[Column], o: &str, v: &[Datum]) -> Result<()> {
        self.write_common_condition(c, o, v)
    }
    /// Go 风格别名：同 `write_expire_condition`。
    pub fn WriteExpireCondition(&mut self, e: i64) -> Result<()> {
        self.write_expire_condition(e)
    }
    /// Go 风格别名：同 `write_in_condition`。
    pub fn WriteInCondition(&mut self, c: &[Column], r: &[Vec<Datum>]) -> Result<()> {
        self.write_in_condition(c, r)
    }
    /// Go 风格别名：同 `write_order_by`。
    pub fn WriteOrderBy(&mut self, c: &[Column], d: bool) -> Result<()> {
        self.write_order_by(c, d)
    }
    /// Go 风格别名：同 `write_limit`。
    pub fn WriteLimit(&mut self, n: i32) -> Result<()> {
        self.write_limit(n)
    }
}
/// 创建指向给定物理表的 `SQLBuilder`。
pub fn NewSQLBuilder(table: &PhysicalTable) -> SQLBuilder<'_> {
    SQLBuilder::new(table)
}

/// 按主键范围与过期时间分页生成 TTL 扫描 SELECT。
///
/// 用前缀栈推进半开区间 `[start, end)`；首批用 `>=` 包含起点，后续用 `>` 排除上一批末行。
pub struct ScanQueryGenerator<'a> {
    table: &'a PhysicalTable,
    expire_unix: i64,
    key_range_start: Vec<Datum>,
    key_range_end: Vec<Datum>,
    stack: Option<Vec<Vec<Datum>>>,
    limit: i32,
    first_build: bool,
    exhausted: bool,
}
impl<'a> ScanQueryGenerator<'a> {
    /// 校验起止键前缀后创建生成器。
    pub fn new(
        table: &'a PhysicalTable,
        expire_unix: i64,
        range_start: Vec<Datum>,
        range_end: Vec<Datum>,
    ) -> Result<Self> {
        table.validate_key_prefix(&range_start)?;
        table.validate_key_prefix(&range_end)?;
        Ok(Self {
            table,
            expire_unix,
            key_range_start: range_start,
            key_range_end: range_end,
            stack: None,
            limit: 0,
            first_build: true,
            exhausted: false,
        })
    }
    /// 用当前键（或 range_start）重建前缀栈，供下一句 WHERE 条件使用。
    fn set_stack(&mut self, key: Option<Vec<Datum>>) -> Result<()> {
        // Go falls back to rangeStart only for a nil key. An explicitly empty
        // continuation row is a non-nil, zero-length slice and clears the stack.
        let key = key.unwrap_or_else(|| self.key_range_start.clone());
        if key.is_empty() {
            self.stack.get_or_insert_with(Vec::new).clear();
            return Ok(());
        }
        self.table.validate_key_prefix(&key)?;
        let stack = self.stack.get_or_insert_with(Vec::new);
        stack.clear();
        for index in 0..key.len() {
            // 保存 key[0..=i] 各层前缀，对应复合主键的逐列推进。
            stack.push(key[..=index].to_vec());
        }
        Ok(())
    }
    /// 根据上一批结果生成下一句扫描 SQL；结果不足 limit 时弹出栈并可能标记耗尽。
    pub fn next_sql(
        &mut self,
        continue_from_result: &[Vec<Datum>],
        next_limit: i32,
    ) -> Result<String> {
        if self.exhausted {
            return Err(SqlError("generator is exhausted".into()));
        }
        if next_limit <= 0 {
            return Err(SqlError(format!("invalid limit '{next_limit}'")));
        }
        // 无论成功失败，退出时都将 first_build 置 false（对应 Go defer）。
        let result = (|| {
            self.stack
                .get_or_insert_with(|| Vec::with_capacity(self.table.key_columns.len()));
            if continue_from_result.len() >= self.limit as usize {
                let key = continue_from_result.last().cloned();
                self.set_stack(key)?;
            } else {
                let stack = self.stack.as_mut().unwrap();
                stack.pop();
                if stack.is_empty() {
                    self.exhausted = true;
                }
            }
            self.limit = next_limit;
            self.build_sql()
        })();
        self.first_build = false;
        result
    }
    /// 用当前栈、范围终点与过期条件拼出完整 SELECT。
    fn build_sql(&mut self) -> Result<String> {
        if self.limit <= 0 {
            return Err(SqlError(format!("invalid limit '{}'", self.limit)));
        }
        if self.exhausted {
            return Ok(String::new());
        }
        let mut builder = SQLBuilder::new(self.table);
        builder.write_select()?;
        if let Some(prefix) = self.stack.as_ref().and_then(|stack| stack.last()) {
            for (index, value) in prefix.iter().enumerate() {
                let columns = &self.table.key_columns[index..=index];
                let values = std::slice::from_ref(value);
                // 前缀列用 =；末列首次 >= 含起点，翻页用 > 排除上批末行。
                let operator = if index + 1 < prefix.len() {
                    "="
                } else if self.first_build {
                    ">="
                } else {
                    ">"
                };
                builder.write_common_condition(columns, operator, values)?;
            }
        }
        if !self.key_range_end.is_empty() {
            builder.write_common_condition(
                &self.table.key_columns[..self.key_range_end.len()],
                "<",
                &self.key_range_end,
            )?;
        }
        builder.write_expire_condition(self.expire_unix)?;
        builder.write_order_by(&self.table.key_columns, false)?;
        builder.write_limit(self.limit)?;
        builder.build()
    }
    /// 是否已无更多键范围可扫。
    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }
    /// Go 风格别名：同 `next_sql`。
    pub fn NextSQL(&mut self, r: &[Vec<Datum>], l: i32) -> Result<String> {
        self.next_sql(r, l)
    }
    /// Go 风格别名：同 `is_exhausted`。
    pub fn IsExhausted(&self) -> bool {
        self.is_exhausted()
    }
}
/// 创建扫描查询生成器。
pub fn NewScanQueryGenerator(
    table: &PhysicalTable,
    expire_unix: i64,
    range_start: Vec<Datum>,
    range_end: Vec<Datum>,
) -> Result<ScanQueryGenerator<'_>> {
    ScanQueryGenerator::new(table, expire_unix, range_start, range_end)
}
/// 按行集合与过期时间构建带 IN 条件的 DELETE SQL。
pub fn BuildDeleteSQL(
    table: &PhysicalTable,
    rows: &[Vec<Datum>],
    expire_unix: i64,
) -> Result<String> {
    if rows.is_empty() {
        return Err(SqlError("Cannot build delete SQL with empty rows".into()));
    }
    let mut builder = SQLBuilder::new(table);
    builder.write_delete()?;
    builder.write_in_condition(&table.key_columns, rows)?;
    builder.write_expire_condition(expire_unix)?;
    builder.write_limit(
        i32::try_from(rows.len()).map_err(|_| SqlError("too many rows for delete limit".into()))?,
    )?;
    builder.build()
}
