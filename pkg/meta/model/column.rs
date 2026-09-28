// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 列（column）元数据模型。
//
// 定义 `ColumnInfo`、默认值表示、列变更/删除临时命名，以及额外系统列
// （`_tidb_rowid`、物理表 ID、提交时间戳）的构造。

use super::{
    ExtraCommitTSID, ExtraHandleID, ExtraPhysTblID, SchemaState, TableInfo, ast, charset, mysql,
    types,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::mem::size_of;

/// ColumnInfo 序列化版本 0（最旧）。
pub const ColumnInfoVersion0: u64 = 0;
/// ColumnInfo 序列化版本 1。
pub const ColumnInfoVersion1: u64 = 1;
/// ColumnInfo 序列化版本 2（当前最新）。
pub const ColumnInfoVersion2: u64 = 2;
/// 当前代码写入的最新 ColumnInfo 版本号。
pub const CurrLatestColumnInfoVersion: u64 = ColumnInfoVersion2;

/// 列变更（ALTER 过程中）临时列名前缀。
pub(crate) const changingColumnPrefix: &str = "_Col$_";
/// 对象删除过程中的墓碑（tombstone）名前缀。
pub(crate) const removingObjPrefix: &str = "_Tombstone$_";

/// 列默认值的多种运行时表示（布尔/整型/浮点/字节串）。
#[derive(Clone, Debug, PartialEq)]
pub enum DefaultValue {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(Vec<u8>),
}

impl Serialize for DefaultValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Uint(value) => serializer.serialize_u64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::String(value) => serializer.serialize_str(&String::from_utf8_lossy(value)),
        }
    }
}

impl<'de> Deserialize<'de> for DefaultValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DefaultValueVisitor;

        impl<'de> serde::de::Visitor<'de> for DefaultValueVisitor {
            type Value = DefaultValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Go-compatible scalar column default value")
            }
            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(DefaultValue::Bool(value))
            }
            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(DefaultValue::Int(value))
            }
            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(DefaultValue::Uint(value))
            }
            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
                Ok(DefaultValue::Float(value))
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(DefaultValue::String(value.as_bytes().to_vec()))
            }
            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(DefaultValue::String(value.into_bytes()))
            }
        }

        deserializer.deserialize_any(DefaultValueVisitor)
    }
}

mod go_bytes {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            None => serializer.serialize_none(),
            Some(bytes) => serializer.serialize_str(&encode(bytes)),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;
        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Option<Vec<u8>>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("null, a base64 string, or a legacy byte array")
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }
            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                decode(value).map(Some).map_err(E::custom)
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(byte) = seq.next_element()? {
                    bytes.push(byte);
                }
                Ok(Some(bytes))
            }
        }
        deserializer.deserialize_option(BytesVisitor)
    }

    fn encode(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let value = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
            output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
            output.push(if chunk.len() > 1 {
                ALPHABET[((value >> 6) & 63) as usize] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                ALPHABET[(value & 63) as usize] as char
            } else {
                '='
            });
        }
        output
    }

    fn decode(value: &str) -> Result<Vec<u8>, &'static str> {
        if !value.len().is_multiple_of(4) {
            return Err("invalid base64 length");
        }
        let mut output = Vec::with_capacity(value.len() / 4 * 3);
        for chunk in value.as_bytes().chunks_exact(4) {
            let a = sextet(chunk[0])?;
            let b = sextet(chunk[1])?;
            let c = if chunk[2] == b'=' {
                0
            } else {
                sextet(chunk[2])?
            };
            let d = if chunk[3] == b'=' {
                0
            } else {
                sextet(chunk[3])?
            };
            let bits =
                (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
            output.push((bits >> 16) as u8);
            if chunk[2] != b'=' {
                output.push((bits >> 8) as u8);
            }
            if chunk[3] != b'=' {
                output.push(bits as u8);
            }
        }
        Ok(output)
    }

    fn sextet(byte: u8) -> Result<u8, &'static str> {
        match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("invalid base64 character"),
        }
    }
}

/// BIT 等列设置了非法默认值时返回的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidDefaultError {
    column: ast::CIStr,
}
impl fmt::Display for InvalidDefaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid default value for '{}'", self.column.O)
    }
}
impl std::error::Error for InvalidDefaultError {}

/// 列变更过程中的依赖状态信息。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ChangeStateInfo {
    /// 依赖列在表中的偏移。
    #[serde(rename = "relative_col_offset")]
    pub DependencyColumnOffset: isize,
}

/// 单列完整元数据：类型、默认值、生成列表达式、schema 状态等。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ColumnInfo {
    /// 列唯一 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 列名。
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
    /// 列在表中的逻辑偏移（下标）。
    #[serde(rename = "offset")]
    pub Offset: isize,
    /// 建表时的原始默认值。
    #[serde(rename = "origin_default")]
    pub OriginDefaultValue: Option<DefaultValue>,
    /// BIT 类型原始默认值的字节旁路存储。
    #[serde(rename = "origin_default_bit")]
    #[serde(with = "go_bytes")]
    pub OriginDefaultValueBit: Option<Vec<u8>>,
    /// 当前默认值。
    #[serde(rename = "default")]
    pub DefaultValue: Option<DefaultValue>,
    /// BIT 类型当前默认值的字节旁路存储。
    #[serde(rename = "default_bit")]
    #[serde(with = "go_bytes")]
    pub DefaultValueBit: Option<Vec<u8>>,
    /// 默认值是否为表达式（而非常量）。
    #[serde(rename = "default_is_expr")]
    pub DefaultIsExpr: bool,
    /// 生成列表达式字符串。
    #[serde(rename = "generated_expr_string")]
    pub GeneratedExprString: String,
    /// 生成列是否 STORED（否则为 VIRTUAL）。
    #[serde(rename = "generated_stored")]
    pub GeneratedStored: bool,
    /// 生成列所依赖的列名集合。
    #[serde(rename = "dependences")]
    pub Dependences: HashMap<String, ()>,
    /// 列字段类型（FieldType）。
    #[serde(rename = "type")]
    pub FieldType: types::FieldType,
    /// 变更进行中的目标字段类型。
    #[serde(rename = "changing_type", skip_serializing_if = "Option::is_none")]
    pub ChangingFieldType: Option<types::FieldType>,
    /// Schema 状态机当前态。
    #[serde(rename = "state")]
    pub State: SchemaState,
    /// 列注释。
    #[serde(rename = "comment")]
    pub Comment: String,
    /// 是否对用户隐藏（系统/内部列）。
    #[serde(rename = "hidden")]
    pub Hidden: bool,
    /// 变更状态附加信息。
    #[serde(rename = "change_state_info")]
    pub ChangeStateInfo: Option<ChangeStateInfo>,
    /// 元数据版本号。
    #[serde(rename = "version")]
    pub Version: u64,
}

impl Default for ColumnInfo {
    fn default() -> Self {
        Self::New(0, ast::CIStr::default())
    }
}

impl ColumnInfo {
    /// 按 ID 与名称构造空字段类型的列元数据。
    pub fn New(ID: i64, Name: ast::CIStr) -> Self {
        Self {
            ID,
            Name,
            Offset: 0,
            OriginDefaultValue: None,
            OriginDefaultValueBit: None,
            DefaultValue: None,
            DefaultValueBit: None,
            DefaultIsExpr: false,
            GeneratedExprString: String::new(),
            GeneratedStored: false,
            Dependences: HashMap::new(),
            FieldType: types::NewFieldType(mysql::TypeUnspecified),
            ChangingFieldType: None,
            State: SchemaState::default(),
            Comment: String::new(),
            Hidden: false,
            ChangeStateInfo: None,
            Version: 0,
        }
    }

    /// 深拷贝本列元数据。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    /// 返回 MySQL 类型码。
    pub fn GetType(&self) -> u8 {
        self.FieldType.GetType()
    }
    /// 返回类型标志位集合。
    pub fn GetFlag(&self) -> usize {
        self.FieldType.GetFlag()
    }
    /// 返回显示宽度（flen）。
    pub fn GetFlen(&self) -> isize {
        self.FieldType.GetFlen()
    }
    /// 返回小数位数。
    pub fn GetDecimal(&self) -> isize {
        self.FieldType.GetDecimal()
    }
    /// 返回字符集名。
    pub fn GetCharset(&self) -> &str {
        self.FieldType.GetCharset()
    }
    /// 返回校对规则名。
    pub fn GetCollate(&self) -> &str {
        self.FieldType.GetCollate()
    }
    /// 返回 ENUM/SET 等元素列表。
    pub fn GetElems(&self) -> &[String] {
        self.FieldType.GetElems()
    }
    /// 设置 MySQL 类型码。
    pub fn SetType(&mut self, value: u8) {
        self.FieldType.SetType(value);
    }
    /// 覆盖类型标志位。
    pub fn SetFlag(&mut self, value: usize) {
        self.FieldType.SetFlag(value);
    }
    /// 按位或追加标志。
    pub fn AddFlag(&mut self, value: usize) {
        self.FieldType.AddFlag(value);
    }
    /// 按位与保留标志。
    pub fn AndFlag(&mut self, value: usize) {
        self.FieldType.AndFlag(value);
    }
    /// 按位异或翻转标志。
    pub fn ToggleFlag(&mut self, value: usize) {
        self.FieldType.ToggleFlag(value);
    }
    /// 清除指定标志位。
    pub fn DelFlag(&mut self, value: usize) {
        self.FieldType.DelFlag(value);
    }
    /// 设置显示宽度。
    pub fn SetFlen(&mut self, value: isize) {
        self.FieldType.SetFlen(value);
    }
    /// 设置小数位数。
    pub fn SetDecimal(&mut self, value: isize) {
        self.FieldType.SetDecimal(value);
    }
    /// 设置字符集。
    pub fn SetCharset(&mut self, value: String) {
        self.FieldType.SetCharset(value);
    }
    /// 设置校对规则。
    pub fn SetCollate(&mut self, value: String) {
        self.FieldType.SetCollate(value);
    }
    /// 设置 ENUM/SET 元素列表。
    pub fn SetElems(&mut self, value: Vec<String>) {
        self.FieldType.SetElems(value);
    }

    /// 是否为生成列（表达式非空）。
    pub fn IsGenerated(&self) -> bool {
        !self.GeneratedExprString.is_empty()
    }
    /// 是否为虚拟生成列（非 STORED）。
    pub fn IsVirtualGenerated(&self) -> bool {
        self.IsGenerated() && !self.GeneratedStored
    }
    /// 是否处于列变更临时命名中。
    pub fn IsChanging(&self) -> bool {
        self.Name.O.starts_with(changingColumnPrefix)
    }
    /// 是否处于删除墓碑命名中。
    pub fn IsRemoving(&self) -> bool {
        self.Name.O.starts_with(removingObjPrefix)
    }
    /// 从墓碑名还原原始对象名。
    pub fn GetRemovingOriginName(&self) -> String {
        self.Name
            .O
            .strip_prefix(removingObjPrefix)
            .unwrap_or(&self.Name.O)
            .to_owned()
    }
    /// 从变更临时名还原原始列名（去掉前缀与后缀序号）。
    pub fn GetChangingOriginName(&self) -> String {
        let name = self
            .Name
            .O
            .strip_prefix(changingColumnPrefix)
            .unwrap_or(&self.Name.O);
        // `_Col$_Origin_N` → 取最后一个 `_` 之前的 Origin。
        name.rsplit_once('_')
            .map_or_else(|| name.to_owned(), |(origin, _)| origin.to_owned())
    }

    /// 设置建表时原始默认值；BIT 列仅允许字符串/空，并同步字节旁路字段。
    pub fn SetOriginDefaultValue(
        &mut self,
        value: Option<DefaultValue>,
    ) -> Result<(), InvalidDefaultError> {
        self.OriginDefaultValue = value;
        if self.GetType() != mysql::TypeBit {
            return Ok(());
        }
        // BIT 列需要把字符串默认值额外写入 OriginDefaultValueBit，以便跨 JSON 保留原始字节。
        match self.OriginDefaultValue.as_ref() {
            None => Ok(()),
            Some(DefaultValue::String(value)) => {
                self.OriginDefaultValueBit = Some(value.clone());
                Ok(())
            }
            Some(_) => Err(InvalidDefaultError {
                column: self.Name.clone(),
            }),
        }
    }

    /// 读取原始默认值；BIT 列优先返回字节旁路字段。
    pub fn GetOriginDefaultValue(&self) -> Option<DefaultValue> {
        if self.GetType() == mysql::TypeBit {
            if let Some(value) = &self.OriginDefaultValueBit {
                return Some(DefaultValue::String(value.clone()));
            }
        }
        self.OriginDefaultValue.clone()
    }

    /// 设置当前默认值；BIT 列仅允许字符串/空，并同步字节旁路字段。
    pub fn SetDefaultValue(
        &mut self,
        value: Option<DefaultValue>,
    ) -> Result<(), InvalidDefaultError> {
        self.DefaultValue = value;
        if self.GetType() != mysql::TypeBit {
            return Ok(());
        }
        // 与 SetOriginDefaultValue 相同：非字符串默认值对 BIT 非法，但仍保留已写入的 DefaultValue。
        match self.DefaultValue.as_ref() {
            None => Ok(()),
            Some(DefaultValue::String(value)) => {
                self.DefaultValueBit = Some(value.clone());
                Ok(())
            }
            Some(_) => Err(InvalidDefaultError {
                column: self.Name.clone(),
            }),
        }
    }

    /// 读取当前默认值；BIT 列优先返回字节旁路字段。
    pub fn GetDefaultValue(&self) -> Option<DefaultValue> {
        if self.GetType() == mysql::TypeBit {
            if let Some(value) = &self.DefaultValueBit {
                return Some(DefaultValue::String(value.clone()));
            }
        }
        self.DefaultValue.clone()
    }

    /// 生成紧凑类型描述字符串（含 unsigned / zerofill 后缀）。
    pub fn GetTypeDesc(&self) -> String {
        let mut result = self.FieldType.CompactStr();
        if mysql::HasUnsignedFlag(self.GetFlag())
            && self.GetType() != mysql::TypeBit
            && self.GetType() != mysql::TypeYear
        {
            result.push_str(" unsigned");
        }
        if mysql::HasZerofillFlag(self.GetFlag()) && self.GetType() != mysql::TypeYear {
            result.push_str(" zerofill");
        }
        result
    }
}

/// 空 `ColumnInfo` 结构体本身的字节大小（用于估算）。
pub const EmptyColumnInfoSize: i64 = size_of::<ColumnInfo>() as i64;

/// 为变更中的列生成表内唯一的临时名 `_Col$_Origin_N`。
pub fn GenUniqueChangingColumnName(table: &TableInfo, old: &ColumnInfo) -> String {
    let names: HashSet<&str> = table
        .Columns
        .iter()
        .map(|column| column.Name.L.as_str())
        .collect();
    // 递增后缀直到小写名在表中不冲突。
    for suffix in 0.. {
        let candidate = format!("{changingColumnPrefix}{}_{}", old.Name.O, suffix);
        if !names.contains(candidate.to_lowercase().as_str()) {
            return candidate;
        }
    }
    unreachable!()
}

/// 为删除中的对象生成墓碑名；若已是墓碑名则原样返回。
pub fn GenRemovingObjName(name: &str) -> String {
    if name.starts_with(removingObjPrefix) {
        name.to_owned()
    } else {
        format!("{removingObjPrefix}{name}")
    }
}

/// 按小写名在列切片中查找列。
pub fn FindColumnInfo<'a>(columns: &'a [ColumnInfo], name: &str) -> Option<&'a ColumnInfo> {
    let name = name.to_lowercase();
    columns.iter().find(|column| column.Name.L == name)
}
/// 按列 ID 在列切片中查找列。
pub fn FindColumnInfoByID(columns: &[ColumnInfo], id: i64) -> Option<&ColumnInfo> {
    columns.iter().find(|column| column.ID == id)
}

/// 将列初始化为二进制校对的 BIGINT 类型。
fn initExtraLonglong(column: &mut ColumnInfo) {
    column.SetType(mysql::TypeLonglong);
    let (flen, decimal) = mysql::GetDefaultFieldLengthAndDecimal(mysql::TypeLonglong);
    column.SetFlen(flen);
    column.SetDecimal(decimal);
    column.SetCharset(charset::CharsetBin.to_owned());
    column.SetCollate(charset::CollationBin.to_owned());
}

/// 构造隐式行号列 `_tidb_rowid`（主键 + NOT NULL BIGINT）。
pub fn NewExtraHandleColInfo() -> ColumnInfo {
    let mut column = ColumnInfo::New(ExtraHandleID, ast::NewCIStr("_tidb_rowid"));
    column.SetFlag(mysql::PriKeyFlag | mysql::NotNullFlag);
    initExtraLonglong(&mut column);
    column
}
/// 构造额外物理表 ID 列 `_tidb_tid`（NOT NULL BIGINT）。
pub fn NewExtraPhysTblIDColInfo() -> ColumnInfo {
    let mut column = ColumnInfo::New(ExtraPhysTblID, ast::NewCIStr("_tidb_tid"));
    column.SetFlag(mysql::NotNullFlag);
    initExtraLonglong(&mut column);
    column
}
/// 构造额外提交时间戳列 `_tidb_commit_ts`（无符号 BIGINT）。
pub fn NewExtraCommitTSColInfo() -> ColumnInfo {
    let mut column = ColumnInfo::New(ExtraCommitTSID, ast::NewCIStr("_tidb_commit_ts"));
    column.SetFlag(mysql::UnsignedFlag);
    initExtraLonglong(&mut column);
    column
}
